//! The workspace snapshot: writing `state.json` and restoring the
//! persisted session lists at launch.

use super::*;

impl AppView {
    /// Write current projects + expanded flags + panel geometry into the
    /// global state snapshot and save to disk. Save errors are reported
    /// (notification or stderr) but never crash the app.
    pub(crate) fn persist(&mut self, cx: &mut App) {
        // Sync the live diff state onto the current session's slot:
        // selection and collapsed dirs are per-session (worktrees can
        // differ), and this slot is what the snapshot serializes.
        if let Some(s) = self
            .projects
            .get_mut(self.current_project)
            .and_then(|p| p.sessions.get_mut(self.current_session))
        {
            s.diff_selected = self
                .diff
                .as_ref()
                .and_then(|d| self.diff_file.and_then(|ix| d.files.get(ix)))
                .map(|f| f.path.to_string());
            s.diff_closed = self.diff_tree_closed.clone();
        }
        let mut snapshot = cx.global::<crate::config::State>().clone();
        snapshot.hidden_sessions = !self.show_sessions;
        snapshot.sidebar_width = self.last_sidebar_size.map(|w| w.as_f32());
        snapshot.diff_width = self.last_diff_size.map(|w| w.as_f32());
        snapshot.current_project = self.current_project;
        snapshot.show_diff = self.show_diff;
        snapshot.show_diff_tree = self.show_diff_tree;
        snapshot.projects = self
            .projects
            .iter()
            .zip(&self.expanded)
            .map(|(p, ex)| crate::config::ProjectConfig {
                name: p.name.clone(),
                path: p.path.clone(),
                expanded: *ex,
                // The whole session list survives the restart. The
                // resume id comes from the agent's captured output
                // (the live id beats a stale command hint).
                sessions: p
                    .sessions
                    .iter()
                    .map(|s| crate::config::SavedSession {
                        kind: s.kind.clone(),
                        title: s.title.clone(),
                        resume: s
                            .term
                            .as_ref()
                            .and_then(|t| {
                                // Agents keep running at window close:
                                // scan the output tail now, or the
                                // next launch has nothing to resume.
                                t.update(cx, |term, _| term.capture_resume_id());
                                t.read(cx).resume_id().map(String::from)
                            })
                            .or_else(|| s.cmd.resume.clone()),
                        selected_file: s.diff_selected.clone(),
                        closed_dirs: s.diff_closed.iter().cloned().collect(),
                    })
                    .collect(),
            })
            .collect();
        let path = self.current_project().path.to_string_lossy().to_string();
        let entry = snapshot
            .project_state
            .entry(path)
            .or_insert_with(crate::config::ProjectState::default);
        // The diff-tree layer is the sidebar splitter's second panel.
        // While the layer is hidden the splitter reports placeholder
        // sizes — keep the last visible height instead.
        entry.tree_height = if self.show_diff_tree {
            self.sidebar_split_state
                .read(cx)
                .sizes()
                .get(1)
                .copied()
                .filter(|h| *h > px(0.))
                .map(|h| h.as_f32())
        } else {
            entry.tree_height
        };
        // Stale state for removed projects must not accumulate.
        let live: std::collections::BTreeSet<_> = self
            .projects
            .iter()
            .map(|p| p.path.to_string_lossy().into_owned())
            .collect();
        snapshot.project_state.retain(|path, _| live.contains(path));
        if let Err(err) = snapshot.save() {
            crate::config::report_error(err, cx);
        }
        cx.set_global(snapshot);
    }

    /// Rebuild every project's session rows from the persisted
    /// snapshot. Agents restore as `Done` rows carrying their resume
    /// id (the selected one auto-resumes from [`Self::new`]); shell
    /// rows respawn live in the project the window opens on — a shell
    /// persists nothing but its existence, so a fresh one matches the
    /// pre-close state — and idle as restartable `Done` rows in the
    /// other projects. Rows whose launcher kind was removed from the
    /// settings are dropped; the selection clamps to what remains.
    pub(super) fn restore_sessions(
        &mut self,
        state: &crate::config::State,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.current_project;
        let mut seq = self.session_seq;
        for (ix, project) in self.projects.iter_mut().enumerate() {
            let Some(saved) = state.projects.get(ix).map(|p| p.sessions.clone()) else {
                continue;
            };
            for s in saved {
                let cmd = match cx.global::<crate::config::Config>().cmd_for(&s.kind) {
                    Ok(cmd) => cmd,
                    Err(_) => continue,
                };
                seq += 1;
                let now = std::time::Instant::now();
                let cwd = project.path.clone();
                let selected = s.selected_file.clone();
                let closed: std::collections::HashSet<String> =
                    s.closed_dirs.iter().cloned().collect();
                if s.kind == "terminal" {
                    let live = ix == current;
                    let (status, term) = if live {
                        match TermSession::spawn(&cmd.spec(&cwd), cx) {
                            Ok(term) => {
                                let program = cmd.program.clone();
                                cx.subscribe_in(
                                    &term,
                                    window,
                                    move |this, emitter, event: &TermEvent, window, cx| match event
                                    {
                                        TermEvent::Wakeup => cx.notify(),
                                        TermEvent::Exit(code) => this.on_session_exit(
                                            emitter.clone(),
                                            *code,
                                            &program,
                                            window,
                                            cx,
                                        ),
                                    },
                                )
                                .detach();
                                (AgentStatus::Running, Some(term))
                            }
                            Err(err) => {
                                eprintln!("[ddu] Failed to restore {}: {err}", cmd.label());
                                (AgentStatus::Error(err.to_string()), None)
                            }
                        }
                    } else {
                        (AgentStatus::Done(0), None)
                    };
                    project.sessions.push(crate::session::AgentSession {
                        id: format!("restored-{ix}-{seq}"),
                        title: s.title.clone(),
                        status,
                        cmd,
                        kind: s.kind,
                        started: now,
                        ended: if term.is_none() { Some(now) } else { None },
                        term,
                        cwd,
                        diff_selected: selected,
                        diff_closed: closed,
                    });
                } else {
                    let cmd = match &s.resume {
                        Some(id) => cmd.with_resume(id.clone()),
                        None => cmd,
                    };
                    project.sessions.push(crate::session::AgentSession {
                        id: format!("restored-{ix}-{seq}"),
                        title: s.title.clone(),
                        status: AgentStatus::Done(0),
                        cmd,
                        kind: s.kind,
                        started: now,
                        ended: Some(now),
                        term: None,
                        cwd,
                        diff_selected: selected,
                        diff_closed: closed,
                    });
                }
            }
        }
        self.session_seq = seq;
    }
}
