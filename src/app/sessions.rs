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
        let from = (self.current_project, self.current_session);
        if changed_project {
            // Save the outgoing project's placement state, then adopt
            // the incoming project's tree-layer height from the global
            // snapshot (selection/collapse come from the session).
            self.persist(cx);
            let path = self.projects[project].path.to_string_lossy().to_string();
            let entry = cx
                .global::<crate::config::State>()
                .project_state
                .get(&path)
                .cloned()
                .unwrap_or_default();
            self.diff_tree_height_seed = entry
                .tree_height
                .map(gpui::px)
                .filter(|h| h.as_f32() >= TREE_MIN_H as f32 && h.as_f32() <= TREE_MAX_H as f32);
        }
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
        // The diff tree is per-session: the outgoing session keeps its
        // selection/collapse state, the incoming session's is adopted.
        self.adopt_session_diff(Some(from), cx);
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
        let old_session = self.current_session;
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
                cwd,
                diff_selected: None,
                diff_closed: Default::default(),
            });
            self.current_session = project.sessions.len() - 1;
        }
        // The new session starts with an empty diff selection (its own
        // worktree view); the outgoing one keeps its state.
        self.adopt_session_diff(Some((self.current_project, old_session)), cx);
        cx.notify();
    }

    /// Re-point the diff state at the current session: the outgoing
    /// session keeps its selection and collapsed dirs (worktrees mean
    /// each session's changes are its own), the incoming session's
    /// state is adopted. `from` is the outgoing `(project, session)`
    /// slot — `None` when that session was removed (nothing to keep).
    pub(crate) fn adopt_session_diff(
        &mut self,
        from: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        let to = (self.current_project, self.current_session);
        if from.is_some_and(|f| f == to) {
            return;
        }
        if let Some((fp, fs)) = from {
            if let Some(s) = self
                .projects
                .get_mut(fp)
                .and_then(|p| p.sessions.get_mut(fs))
            {
                s.diff_selected = self
                    .diff
                    .as_ref()
                    .and_then(|d| self.diff_file.and_then(|ix| d.files.get(ix)))
                    .map(|f| f.path.to_string());
                s.diff_closed = self.diff_tree_closed.clone();
            }
        }
        let (seed, closed) = {
            let incoming = self.projects.get(to.0).and_then(|p| p.sessions.get(to.1));
            (
                incoming.and_then(|s| s.diff_selected.clone()),
                incoming.map(|s| s.diff_closed.clone()).unwrap_or_default(),
            )
        };
        self.diff_seed_path = seed;
        self.reset_diff();
        self.diff_tree_closed = closed;
        self.reload_diff(cx);
    }

    /// The working tree the diff poll targets: the current session's
    /// (worktrees later; today the project root it spawned in).
    pub(super) fn current_session_cwd(&self) -> std::path::PathBuf {
        self.projects
            .get(self.current_project)
            .and_then(|p| p.sessions.get(self.current_session))
            .map(|s| s.cwd.clone())
            .unwrap_or_else(|| self.current_project().path.clone())
    }

    /// Exit event from a session's PTY pump: mirror the status and notify.
    pub(super) fn on_session_exit(
        &mut self,
        emitter: Entity<TermSession>,
        code: i32,
        _program: &str,
        window: &mut Window,
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
        crate::config::debug_log(&format!(
            "exit: code={code} matched={matched} running={} shutting_down={}",
            self.running_terms().len(),
            self.shutting_down
        ));
        // Shutdown sequencing: every Exit is a candidate for the last
        if self.shutting_down && self.running_terms().is_empty() {
            self.finish_shutdown(window, cx);
        }
        cx.notify();
    }

    /// Close-session entry point: a live agent first gets the confirm
    /// dialog (stopping is disruptive); a dead row goes right away.
    pub(crate) fn request_close_session(
        &mut self,
        p: usize,
        six: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.projects.get(p).and_then(|pr| pr.sessions.get(six)) else {
            return;
        };
        if !session.status.is_running() {
            self.close_session(p, six, window, cx);
            return;
        }
        let title = session.title.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(format!("Close “{title}”?"))
                .description(
                    "The running agent will be stopped (Ctrl-C); its session id is saved so the conversation can be resumed.",
                )
                .on_ok({
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.close_session(p, six, window, cx));
                        }
                        true
                    }
                })
                .footer(crate::ui::dialog_footer("Close Session", "confirm-close", {
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.close_session(p, six, window, cx));
                        }
                        window.close_dialog(cx);
                    }
                }))
        });
    }

    /// Close session `six` of project `p`. A live agent is stopped
    /// gracefully (Ctrl-C into the PTY): the child exits, prints its
    /// resume banner, the id is captured on the Exit event and saved —
    /// and the row STAYS as a resumable Done entry. A dead row has
    /// nothing left to save and is removed outright; removing a
    /// current session shifts the selection to the nearest neighbor.
    pub(crate) fn close_session(
        &mut self,
        p: usize,
        six: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self
            .projects
            .get(p)
            .and_then(|pr| pr.sessions.get(six))
            .cloned()
        else {
            return;
        };
        if let (Some(term), true) = (&session.term, session.status.is_running()) {
            term.update(cx, |s, _| s.interrupt());
            // The exit event persists the row with its resume id.
            cx.notify();
            return;
        }

        // Dead row: drop it.
        let was_current = p == self.current_project && six == self.current_session;
        let became_empty;
        {
            let project = self.projects.get_mut(p).unwrap();
            project.sessions.remove(six);
            became_empty = project.sessions.is_empty();
        }
        if was_current {
            self.current_session = six.min(self.projects[p].sessions.len().saturating_sub(1));
            // The closed session's diff state dies with it; the
            // neighbor now current adopts its own (or starts empty).
            self.adopt_session_diff(None, cx);
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
        self.persist(cx);
        cx.notify();
    }

    /// Live PTYs of sessions whose status is still Running — the ones a
    /// shutdown must wait for.
    pub(crate) fn running_terms(&self) -> Vec<Entity<TermSession>> {
        self.projects
            .iter()
            .flat_map(|p| p.sessions.iter())
            .filter(|s| s.status.is_running())
            .filter_map(|s| s.term.clone())
            .collect()
    }

    /// Window close (red button): always confirms first. The confirm
    /// handler runs the exit process — live agents get Ctrl-C, their
    /// resume ids land in state.json, then the window goes away.
    /// Returns false while the dialog or the exit process is live.
    pub(crate) fn request_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.shutting_down {
            return false;
        }
        self.confirm_exit(false, window, cx);
        false
    }

    /// ⌘Q: the same confirm dialog, ending in a process exit (the
    /// reliable route here — the platform terminate path never
    /// completes under this app's setup).
    pub(crate) fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.confirm_exit(true, window, cx);
    }

    /// Confirm dialog shared by window close and ⌘Q.
    fn confirm_exit(&mut self, quit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.shutting_down {
            return;
        }
        let live = self.running_terms().len();
        let what = if quit { "Quit" } else { "Close window" };
        let description = if live > 0 {
            format!(
                "{live} session{} running. They will be stopped now and their session ids saved so the conversations can be resumed.",
                if live == 1 { " is" } else { "s are" }
            )
        } else {
            "The workspace layout is saved; sessions stay resumable.".to_string()
        };
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(format!("{what}?"))
                .description(description.clone())
                .on_ok({
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.begin_exit(quit, window, cx));
                        }
                        true
                    }
                })
                .footer(crate::ui::dialog_footer(
                    if quit { "Quit" } else { "Close" },
                    "confirm-exit",
                    {
                        let this = this.clone();
                        move |_, window, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |v, cx| v.begin_exit(quit, window, cx));
                            }
                            window.close_dialog(cx);
                        }
                    },
                ))
        });
    }

    /// Confirmed exit: with live agents start the graceful shutdown
    /// (the Exiting overlay shows meanwhile); otherwise finish at once.
    pub(crate) fn begin_exit(&mut self, quit: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.shutting_down {
            return;
        }
        self.quit_after_shutdown = quit;
        if self.running_terms().is_empty() {
            self.finish_shutdown(window, cx);
            return;
        }
        self.shutting_down = true;
        crate::config::debug_log(&format!(
            "begin_exit: quit={quit} live={}",
            self.running_terms().len()
        ));
        self.begin_shutdown(cx);
    }

    /// Kick off the shutdown sequence: Ctrl-C to every live agent now;
    /// agents that gate exit behind a second Ctrl-C get one nudge at
    /// 2s; anything still alive at 6s is killed outright (its Exit
    /// still fires, the tail may still carry the resume id).
    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        crate::config::debug_log("begin_shutdown: interrupt all");
        for term in self.running_terms() {
            term.update(cx, |s, _| s.interrupt());
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let alive = this.update(cx, |this, cx| {
                let alive = this.running_terms();
                for term in &alive {
                    term.update(cx, |s, _| s.interrupt());
                }
                !alive.is_empty()
            })?;
            if alive {
                cx.background_executor().timer(Duration::from_secs(4)).await;
                this.update(cx, |this, cx| {
                    for term in this.running_terms() {
                        term.update(cx, |s, _| s.kill());
                    }
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
        cx.notify();
    }

    /// The last live agent is out: save everything (resume ids were
    /// captured on each Exit), then close the window or quit.
    fn finish_shutdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shutting_down = false;
        crate::config::debug_log("finish_shutdown: persist + close");
        self.persist(cx);
        if self.quit_after_shutdown {
            std::process::exit(0);
        } else {
            window.remove_window();
        }
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
            crate::config::debug_log("resume: NO ID — aborting");
            eprintln!("[ddu] No session id found in the last run's output.");
            return;
        };
        crate::config::debug_log(&format!("resume: respawning with id={id}"));
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
        let Some(session) = project.sessions.get_mut(self.current_session) else {
            return;
        };
        let cwd = session.cwd.clone();
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
