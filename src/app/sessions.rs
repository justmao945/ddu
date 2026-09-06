//! Session lifecycle: selection, spawning, closing, restart/resume
//! and the PTY exit bookkeeping.

use super::*;

impl AppView {
    pub(crate) fn select_session(
        &mut self,
        project: usize,
        session: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changed_project = self.current_project != project;
        // Write the outgoing session's diff visibility back onto it:
        // the panel state is per-session and must survive a switch.
        let old_show = self.show_diff;
        if let Some(old) = self
            .projects
            .get_mut(self.current_project)
            .and_then(|p| p.sessions.get_mut(self.current_session))
        {
            old.show_diff = old_show;
        }
        if changed_project {
            // Save the outgoing project's placement state, then adopt
            // the incoming one from the global snapshot.
            self.persist(cx);
            let path = self.projects[project].path.to_string_lossy().to_string();
            let entry = cx
                .global::<crate::config::State>()
                .project_state
                .get(&path)
                .cloned()
                .unwrap_or_default();
            self.diff_seed_path = entry.selected_file.clone();
            self.diff_tree_closed = entry.closed_dirs.iter().cloned().collect();
            self.diff_tree_height_seed = entry
                .tree_height
                .map(gpui::px)
                .filter(|h| h.as_f32() >= TREE_MIN_H as f32 && h.as_f32() <= TREE_MAX_H as f32);
        }
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
        // Adopt the incoming session's diff visibility. `set_diff`
        // writes the value back to (now-)current session — the same
        // value, so the round-trip is idempotent.
        let adopt = self
            .projects
            .get(project)
            .and_then(|p| p.sessions.get(self.current_session))
            .map(|s| s.show_diff)
            .unwrap_or(true);
        if adopt != self.show_diff {
            self.set_diff(adopt, cx);
        }
        if changed_project {
            self.reset_diff();
            self.reload_diff(cx);
        }
        if let Some(term) = self.current_term() {
            let focus = term.read(cx).focus.clone();
            focus.focus(window, cx);
        } else {
            self.window_focus.focus(window, cx);
        }
        // Remember the selection (and any diff adopt that flipped the
        // panel) even when no layout changed.
        self.persist(cx);
        cx.notify();
    }

    /// Create a session with the default launcher and focus it.
    pub(crate) fn spawn_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kind = cx
            .global::<crate::config::Config>()
            .new_session
            .kind
            .clone();
        self.spawn_session_of(&kind, window, cx);
    }

    /// Create a session of `kind` (`terminal`/builtin/custom) in project
    /// `project` and focus it.
    pub(crate) fn spawn_session_of(
        &mut self,
        kind: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cmd = match cx.global::<crate::config::Config>().cmd_for(kind) {
            Ok(cmd) => cmd,
            Err(err) => {
                self.window_focus.focus(window, cx);
                eprintln!("[ddu] Cannot launch {kind}: {err}");
                return;
            }
        };
        let Some(project) = self.projects.get(self.current_project) else {
            return;
        };
        let cwd = project.path.clone();
        self.session_seq += 1;
        let seq = self.session_seq;
        let title = cmd.basename();

        let (status, term) = match TermSession::spawn(&cmd.spec(&cwd), cx) {
            Ok(term) => {
                let program = cmd.program.clone();
                cx.subscribe_in(
                    &term,
                    window,
                    move |this, emitter, event: &TermEvent, window, cx| match event {
                        TermEvent::Wakeup => cx.notify(),
                        TermEvent::Exit(code) => {
                            this.on_session_exit(emitter.clone(), *code, &program, window, cx)
                        }
                    },
                )
                .detach();
                let focus = term.read(cx).focus.clone();
                focus.focus(window, cx);
                (AgentStatus::Running, Some(term))
            }
            Err(err) => {
                eprintln!("[ddu] Failed to start {}: {err}", cmd.label());
                (AgentStatus::Error(err.to_string()), None)
            }
        };

        if let Some(project) = self.projects.get_mut(self.current_project) {
            // A freshly created session must be visible immediately.
            self.expanded[self.current_project] = true;
            project.sessions.push(crate::session::AgentSession {
                id: format!("s-{seq}"),
                title,
                status,
                cmd,
                kind: kind.to_string(),
                started: std::time::Instant::now(),
                ended: if term.is_none() {
                    Some(std::time::Instant::now())
                } else {
                    None
                },
                term,
                // New sessions inherit the panel state of the session
                // they replace (the common "new terminal, same
                // workspace" flow) instead of snapping to a default.
                show_diff: self.show_diff,
            });
            self.current_session = project.sessions.len() - 1;
        }
        self.diff_file = 0;
        self.reload_diff(cx);
        cx.notify();
    }

    /// Exit event from a session's PTY pump: mirror the status and notify.
    pub(super) fn on_session_exit(
        &mut self,
        emitter: Entity<TermSession>,
        code: i32,
        _program: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let status = match code {
            0 => AgentStatus::Done(0),
            code if code > 0 => AgentStatus::Done(code),
            code => AgentStatus::Error(format!("abnormal exit {code}")),
        };
        let ended = std::time::Instant::now();
        let mut matched = false;
        for project in self.projects.iter_mut() {
            for session in project.sessions.iter_mut() {
                if session.term.as_ref() == Some(&emitter) {
                    matched = true;
                    session.status = status.clone();
                    session.ended = Some(ended);
                    // The resume id stays on the term; `persist` reads
                    // it from there when saving the session list.
                }
            }
        }
        if matched {
            // The list (including the freshly captured resume id) is
            // saved right away: a crash or kill -9 must not lose it.
            self.persist(cx);
        }
        cx.notify();
    }

    /// Close (and kill if needed) session `six` of project `p`.
    /// A closed current session selects the nearest neighbor.
    pub(crate) fn close_session(
        &mut self,
        p: usize,
        six: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get_mut(p) else {
            return;
        };
        let Some(six) = (six < project.sessions.len()).then_some(six) else {
            return;
        };
        let was_current = p == self.current_project && six == self.current_session;
        let became_empty;
        {
            let session = project.sessions.remove(six);
            if let Some(term) = &session.term {
                term.update(cx, |s, _| s.kill());
            }
            became_empty = project.sessions.is_empty();
        }
        if was_current {
            self.current_session = six.min(self.projects[p].sessions.len().saturating_sub(1));
            self.diff_file = 0;
            self.reload_diff(cx);
            if let Some(term) = self.current_term() {
                let focus = term.read(cx).focus.clone();
                focus.focus(window, cx);
            } else if became_empty {
                // Keep global shortcuts alive when the pane empties out.
                self.window_focus.focus(window, cx);
            }
        } else if p == self.current_project {
            // Keep pointing at the same session when a sibling before it
            // went away.
            self.current_session =
                index_after_removal(self.current_session, six, self.projects[p].sessions.len());
        }
        self.hovered_session = None;
        cx.notify();
    }

    /// Confirm closing a running session, else close immediately.
    pub(crate) fn request_close_session(
        &mut self,
        p: usize,
        six: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get(p) else {
            return;
        };
        let Some(session) = project.sessions.get(six) else {
            return;
        };
        let running = session.status.is_running();
        let title = session.title.clone();
        if !running {
            self.close_session(p, six, window, cx);
            return;
        }
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(format!("Close “{title}”?"))
                .description("The running agent will be stopped.")
                // Enter confirms, same as the footer button.
                .on_ok({
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.close_session(p, six, window, cx));
                        }
                        true
                    }
                })
                // Shared Cancel + danger-confirm recipe — see
                // `ui::dialog_footer`.
                .footer(crate::ui::dialog_footer(
                    "Close Session",
                    "confirm-close",
                    {
                        let this = this.clone();
                        move |_, window, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |v, cx| v.close_session(p, six, window, cx));
                            }
                            window.close_dialog(cx);
                        }
                    },
                ))
        });
    }

    /// Restart the current session with the same command in a fresh PTY.
    pub(crate) fn restart_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.respawn_current_session(None, window, cx);
    }

    /// Restart the current session resuming its agent session
    /// (`--resume <id>`), using the id captured from the last run's
    /// output. No-ops when the last run captured none.
    pub(crate) fn resume_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let resume_id = self.current_session().and_then(|s| {
            s.term
                .as_ref()
                .and_then(|t| t.read(cx).resume_id().map(String::from))
                .or_else(|| s.cmd.resume.clone())
        });
        let Some(id) = resume_id else {
            eprintln!("[ddu] No session id found in the last run's output.");
            return;
        };
        self.respawn_current_session(Some(id), window, cx);
    }

    /// Shared body of restart/resume: kill the old PTY (if alive) and
    /// spawn a fresh one, optionally resuming the agent session.
    pub(super) fn respawn_current_session(
        &mut self,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get_mut(self.current_project) else {
            return;
        };
        let cwd = project.path.clone();
        let Some(session) = project.sessions.get_mut(self.current_session) else {
            return;
        };
        let cmd = match resume {
            Some(id) => session.cmd.clone().with_resume(id),
            None => session.cmd.clone(),
        };
        if let Some(old) = session.term.take() {
            old.update(cx, |s, _| s.kill());
        }
        let (status, term) = match TermSession::spawn(&cmd.spec(&cwd), cx) {
            Ok(term) => {
                let program = cmd.program.clone();
                cx.subscribe_in(
                    &term,
                    window,
                    move |this, emitter, event: &TermEvent, window, cx| match event {
                        TermEvent::Wakeup => cx.notify(),
                        TermEvent::Exit(code) => {
                            this.on_session_exit(emitter.clone(), *code, &program, window, cx)
                        }
                    },
                )
                .detach();
                let focus = term.read(cx).focus.clone();
                focus.focus(window, cx);
                (AgentStatus::Running, Some(term))
            }
            Err(err) => {
                self.window_focus.focus(window, cx);
                eprintln!("[ddu] Failed to restart: {err}");
                (AgentStatus::Error(err.to_string()), None)
            }
        };
        if let Some(session) = project.sessions.get_mut(self.current_session) {
            session.status = status;
            session.title = cmd.basename();
            session.term = term;
            session.started = std::time::Instant::now();
            session.ended = if session.term.is_none() {
                Some(std::time::Instant::now())
            } else {
                None
            };
        }
        cx.notify();
    }
}

/// Preserve the selected identity when an earlier sibling is removed.
pub(super) fn index_after_removal(selected: usize, removed: usize, remaining: usize) -> usize {
    selected
        .saturating_sub(usize::from(removed < selected))
        .min(remaining.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::index_after_removal;
    #[test]
    pub(super) fn removal_keeps_selection_or_nearest_neighbor() {
        assert_eq!(index_after_removal(2, 0, 3), 1);
        assert_eq!(index_after_removal(1, 2, 3), 1);
        assert_eq!(index_after_removal(3, 3, 3), 2);
        assert_eq!(index_after_removal(0, 0, 0), 0);
    }
}
