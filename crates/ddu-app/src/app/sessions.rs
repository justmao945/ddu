//! Session lifecycle: selection, spawning, closing, restart/resume and
//! the PTY exit bookkeeping. Leaving the app is [`super::shutdown`]'s.

use super::diff::lives_in_tree;
use super::*;

impl AppView {
    pub(crate) fn select_session(
        &mut self,
        project: usize,
        session: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Empty workspace (all projects removed): nothing to select.
        if self.projects.get(project).is_none() {
            return;
        }
        // The file being left keeps its place: the pane's rows and its
        // offset are the outgoing session's, and the switch is about to
        // reset them. The layer is the outgoing *project's* — a switch
        // inside one project reads the same rows and the same place.
        self.remember_scroll();
        self.remember_tree_scroll();
        let from = (self.current_project, self.current_session);
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
        // The outgoing session keeps its selected file (the file tree
        // itself is the project's and stays as it is); the incoming
        // session's is adopted.
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
            .global::<ddu_core::config::Config>()
            .new_session
            .kind
            .clone();
        self.spawn_session_of(&kind, window, cx);
    }

    /// Create a session of `kind` (`terminal`/builtin) in project
    /// `project` and focus it.
    pub(crate) fn spawn_session_of(
        &mut self,
        kind: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cmd = match cx.global::<ddu_core::config::Config>().cmd_for(kind) {
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
        // The file the outgoing row was showing keeps its place (see
        // `select_session`).
        self.remember_scroll();
        let old_session = self.current_session;
        self.session_seq += 1;
        let seq = self.session_seq;
        let title = cmd.basename();
        // The grid's history cap is the user's setting; the terminal crate
        // owns no config, so the shell reads it and passes it in.
        let scrollback = cx.global::<ddu_core::config::Config>().terminal_scrollback();

        let (status, term) = match TermSession::spawn(&cmd.spec(&cwd), scrollback, cx) {
            Ok(term) => {
                self.subscribe_term(&term, cmd.program.clone(), window, cx);
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
            project.sessions.push(ddu_core::session::AgentSession {
                id: format!("s-{seq}"),
                title,
                status,
                cmd,
                resume_id: None,
                was_live: false,
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
                view_mode: None,
            });
            self.current_session = project.sessions.len() - 1;
        }
        // The new session starts with an empty diff selection (its own
        // worktree view); the outgoing one keeps its state.
        self.adopt_session_diff(Some((self.current_project, old_session)), cx);
        cx.notify();
    }

    /// Re-point the diff state at the current session: the outgoing
    /// session keeps its selected file and pane mode, the incoming
    /// session's are adopted — that is what a switch has to put back.
    ///
    /// The file *tree* is the project's ([`AppView::tree_open`]), so a
    /// switch inside one project leaves it exactly as it was; only moving
    /// to another project adopts that project's expansion and layer
    /// height. `from` is the outgoing `(project, session)` slot — `None`
    /// when that session was removed (nothing to keep).
    ///
    /// The caller must have remembered the pane's position
    /// ([`AppView::remember_scroll`]) while the outgoing session was still
    /// the current one: the reset below zeroes the pane's offset, and the
    /// incoming file's place is restored on the way in — waiting for its
    /// own rows when it has to (see `pending_scroll`).
    pub(crate) fn adopt_session_diff(
        &mut self,
        from: Option<(usize, usize)>,
        cx: &mut Context<Self>,
    ) {
        let to = (self.current_project, self.current_session);
        if from.is_some_and(|f| f == to) {
            return;
        }
        // Read before the session slot is borrowed mutably.
        let selected_path = self.current_diff_path().map(str::to_owned);
        let has_snapshot = self.snapshot.is_some();
        if let Some((fp, fs)) = from {
            if let Some(s) = self
                .projects
                .get_mut(fp)
                .and_then(|p| p.sessions.get_mut(fs))
            {
                // Only the loaded diff knows the truth about the
                // selection: a save that lands before the first poll
                // (the window-frame debounce does) must not overwrite
                // the stored path with `None`.
                // As in `persist`: never overwrite the stored path
                // with `None` before the diff has loaded.
                if selected_path.is_some() || has_snapshot {
                    s.diff_selected = selected_path;
                }
                s.view_mode = Some(self.view_mode.as_str().to_owned());
            }
        }
        let (seed, mode) = {
            let incoming = self.projects.get(to.0).and_then(|p| p.sessions.get(to.1));
            (
                incoming.and_then(|s| s.diff_selected.clone()),
                incoming
                    .and_then(|s| s.view_mode.as_deref())
                    .and_then(ViewMode::parse),
            )
        };
        // Whether the rows on screen are another working tree's. A `None`
        // slot — a row that was removed — counts as a move: there is no
        // outgoing state to keep, and the fresh project a `+` adds has no
        // tree of its own yet.
        let project_changed = from.map(|(fp, _)| fp) != Some(to.0);
        if project_changed {
            // The splitter under the outgoing project keeps its live
            // height; the incoming project's height is adopted below.
            let live_h = self.live_tree_height(cx);
            if let (Some((fp, _)), Some(h)) = (from, live_h) {
                if let Some(project) = self.projects.get_mut(fp) {
                    project.tree_height = Some(h);
                }
            }
            // The layer's place comes the same way, and it is the
            // **project's**: the rows the offset was measured against are
            // the outgoing project's, and the incoming project comes back
            // to its own once its rows exist (the pending offset below,
            // applied by `rebuild_tree_index`). The outgoing half was read
            // before the move (`remember_tree_scroll`, next to the pane's
            // own bookkeeping).
            self.pending_tree_scroll = self
                .current_project()
                .and_then(|p| p.tree_scroll)
                .map(gpui::px);
            // The adopted height must beat a pinned drag size on the live
            // splitter (a bare seed only wins while the panel is
            // unpinned), so unpin the layer panel and let the next render
            // apply it — the outgoing project's size must not survive
            // into this one either, so the unpin happens even when the
            // incoming project stored no height (its default then wins).
            self.diff_tree_height_seed = self
                .current_project()
                .and_then(|p| p.tree_height)
                .map(gpui::px);
            if self.show_diff_tree {
                self.sidebar_split_state.update(cx, |state, cx| {
                    if state.sizes().len() > 1 {
                        state.reset_panel(1, cx);
                    }
                });
            }
        }
        self.reset_diff(!project_changed);
        // The pane's mode is per session too: the incoming row's mode,
        // stock Diff for a row that never chose one.
        self.view_mode = mode.unwrap_or_default();
        // The file the row had open. This project's diff is usually still
        // loaded — the working tree did not change, only which row is
        // reading it — and then the file goes straight back into the pane,
        // with its place restored by `select_path`. A cold tree (a launch,
        // a move to another project) has nothing to pin the path against,
        // and waits for the first poll to seed it (see `apply_snapshot`).
        if let Some(path) = seed {
            let root = self.current_session_cwd();
            match self.diff() {
                // The path lands now, or it left the working tree and the
                // pane stays empty — which is what the poll's own filter
                // does with a path it cannot find.
                Some(diff) => {
                    if lives_in_tree(root.as_deref(), &diff.files, &path) {
                        self.select_path(path);
                    }
                }
                None => self.diff_seed_path = Some(path),
            }
        }
        self.ensure_file_content(cx);
        self.rebuild_tree_index();
        self.reload_diff(cx);
    }

    /// The working tree the diff poll targets: the current session's.
    /// No active session → `None` — no diff at all (the panels show
    /// their "no session" note instead of a stale project tree).
    pub(super) fn current_session_cwd(&self) -> Option<std::path::PathBuf> {
        self.projects
            .get(self.current_project)
            .and_then(|p| p.sessions.get(self.current_session))
            .map(|s| s.cwd.clone())
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
                    // Agents carry a conversation id; a shell's output
                    // may still contain the word "resume", so only an
                    // agent row collects one.
                    session.resume_id = session
                        .is_agent()
                        .then(|| emitter.read(cx).resume_id().map(String::from))
                        .flatten();
                }
            }
        }
        if matched {
            // The list (including the freshly captured resume id) is
            // saved right away: a crash or kill -9 must not lose it.
            self.persist(cx);
        }
        // Shutdown sequencing: every Exit is a candidate for the last
        if self.shutting_down && self.running_terms().is_empty() {
            self.finish_shutdown(window, cx);
        }
        cx.notify();
    }

    /// Close-session entry point — the one rule, so that a click on `×`
    /// (or ⌘W) always does the same thing:
    ///
    /// * **A live agent** asks first (stopping it is disruptive), and the
    ///   confirm *stops* it: the row stays, now a resumable `Done` entry.
    ///   Closing it again — a dead row by then — removes it. That is the
    ///   two-step: the first close ends a conversation you can come back
    ///   to, the second throws the row away.
    /// * **A dead row, and a plain shell of any state**, just goes: a
    ///   dead agent has nothing left to stop, and a shell holds no
    ///   conversation at all.
    ///
    /// A row's *kind* is what decides, not what is running inside it: an
    /// agent started by hand in a `Terminal` row is a shell row here, so
    /// it closes without a question and leaves nothing behind.
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
        // Plain terminals skip the dialog too: a shell holds no
        // conversation state, closing it needs no confirmation.
        if !session.status.is_running() || session.kind == "terminal" {
            self.close_session(p, six, window, cx);
            return;
        }
        let title = session.title.clone();
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(format!("Close “{title}”?"))
                .description(
                    "The running agent will be interrupted and stopped; its session id is saved so the conversation can be resumed.",
                )
                .on_ok({
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.stop_live_agent(p, six, window, cx));
                        }
                        true
                    }
                })
                .footer(crate::ui::dialog_footer("Close Session", "confirm-close", {
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.stop_live_agent(p, six, window, cx));
                        }
                        window.close_dialog(cx);
                    }
                }))
        });
    }

    /// Confirmed close of a live agent: stop it gracefully and KEEP its
    /// row.
    ///
    /// Deliberately not [`Self::close_session`], and deliberately not
    /// re-reading `is_running`: this path exists because the user closed
    /// a *live* agent, and the row that survives is the one carrying the
    /// resume id — the id the dialog just promised to save, which is the
    /// whole point of keeping the row. The dialog can sit open while the
    /// agent finishes on its own, and a status read at that later moment
    /// used to send the click down the dead-row path instead — the row
    /// (and with it the conversation) silently gone, sometimes. What the
    /// user asked for decides; the row's state at confirm time does not.
    fn stop_live_agent(
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
        // A `Running` row always has a live PTY; one whose term is gone
        // has nothing to stop and keeps nothing either, so it is a dead
        // row after all.
        let Some(term) = session.term.clone() else {
            self.remove_session_row(p, six, window, cx);
            return;
        };
        // Save the resume id BEFORE the stop sequence: if the agent dies
        // without printing its banner (the kill at the end of the
        // escalation), the id from its earlier output is already on disk.
        term.update(cx, |s, _| s.capture_resume_id());
        self.persist(cx);
        // An agent that finished while the dialog was open has nothing
        // left to interrupt: its exit already settled the row.
        if session.status.is_running() {
            Self::escalate_close(vec![term], cx);
        }
        cx.notify();
    }

    /// Close a session that needs no confirmation — a dead row (its
    /// process is already out; only `×` is left to act on) or a plain
    /// shell (a terminal holds no conversation state).
    ///
    /// A live shell's foreground job gets one Ctrl-C before its PTY
    /// drops, and the row goes at once. A live *agent* never arrives
    /// here: [`Self::request_close_session`] sends it through the dialog
    /// to [`Self::stop_live_agent`], whose row survives the stop.
    fn close_session(
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
            // Plain shell: nothing resumable at stake. One Ctrl-C
            // stops a foreground job, then the PTY drops — no
            // agent stop escalation; the row is removed at once.
            term.update(cx, |s, _| s.ctrl(0x03));
            let term = term.clone();
            cx.spawn(async move |this, cx| {
                // A beat so the Ctrl-C lands before the PTY dies.
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                this.update(cx, |_, cx| term.update(cx, |s, _| s.kill()))?;
                anyhow::Ok(())
            })
            .detach();
        }

        // Dead rows and closed terminals: drop the row outright.
        self.remove_session_row(p, six, window, cx);
    }

    /// Remove a session row outright — a dead one, or a terminal whose
    /// PTY just got the stop. Removing the current session shifts the
    /// selection to the nearest neighbor.
    fn remove_session_row(
        &mut self,
        p: usize,
        six: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_current = p == self.current_project && six == self.current_session;
        // A row can go while its file is on screen: the place in that
        // file belongs to the file, so it is read before the switch.
        if was_current {
            self.remember_scroll();
        }
        let became_empty;
        {
            let project = self.projects.get_mut(p).unwrap();
            // The row's title memory goes with it: nothing will ever
            // read this entry again, and the entity id is not reused.
            if let Some(term) = project.sessions[six].term.as_ref() {
                self.row_titles.remove(&term.entity_id());
            }
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

    /// Restart the current session with the same command in a fresh PTY.
    pub(crate) fn restart_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.respawn_current_session(None, window, cx);
    }

    /// Restart the current session resuming its agent session
    /// (`--resume <id>`), using the id captured from the last run's
    /// output. No-ops when the last run captured none.
    pub(crate) fn resume_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.current_resume_id(cx) else {
            eprintln!("[ddu] No session id found in the last run's output.");
            return;
        };
        self.respawn_current_session(Some(id), window, cx);
    }

    /// The current row's conversation id: the live run's captured id
    /// when the PTY is around, else the id stored on the row (restored
    /// rows and dead ones keep it without a terminal).
    pub(crate) fn current_resume_id(&self, cx: &App) -> Option<String> {
        self.current_session().and_then(|s| {
            s.term
                .as_ref()
                .and_then(|t| t.read(cx).resume_id().map(String::from))
                .or_else(|| s.resume_id.clone())
        })
    }

    /// Subscribe the app to one session's terminal events: wakeups
    /// repaint, exit settles the row.
    ///
    /// A wakeup repaints the center pane only for the session it
    /// renders: a background row keeps parsing (its grid must be current
    /// when the row is selected) but changes nothing *there*, so N
    /// streaming agents must not each drive a full-window redraw — that
    /// multiplies the frame rate the pump's throttle exists to bound.
    /// Its OSC title is the exception: the sidebar row shows it whether
    /// or not the grid is on screen, so a title change notifies the
    /// sidebar from a background row too (`note_row_title`). Exit, the
    /// diff poll and interaction all keep their own notify.
    pub(super) fn subscribe_term(
        &mut self,
        term: &Entity<TermSession>,
        program: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.subscribe_in(
            term,
            window,
            move |this, emitter, event: &TermEvent, window, cx| match event {
                TermEvent::Wakeup => {
                    emitter.update(cx, |term, cx| term.note_search_dirty(cx));
                    // The pane is the repaint unit for a stream: notifying
                    // the app would fan out to the cached panels and
                    // rebuild them for output that cannot change them.
                    // A background row's grid is not on screen, so it
                    // paints nothing (see `is_visible_term`).
                    let visible = this.is_visible_term(emitter);
                    if visible {
                        this.terminal_pane.update(cx, |_, cx| cx.notify());
                        // An assistive client reads the sidebar and the
                        // breadcrumb too, and gpui rebuilds its tree from
                        // exactly the elements that prepainted this frame
                        // (`window/a11y.rs`: `begin_frame` clears it), so a
                        // panel kept cached here would drop out of the tree
                        // while the pane streams. Notify them only while a
                        // client is attached — the flag is false otherwise,
                        // which is what keeps the panels cached on stream
                        // frames.
                        if window.is_a11y_active() {
                            this.sidebar.update(cx, |_, cx| cx.notify());
                            this.breadcrumb.update(cx, |_, cx| cx.notify());
                        }
                    }
                    // The one stream output that *is* visible outside
                    // the pane: the OSC title, which agent CLIs spin,
                    // shown in the row and the window breadcrumb. Follow
                    // it wherever it moves — a background row's spinner
                    // is on screen (in the sidebar) just the same, and
                    // only the visible session's also reaches the
                    // breadcrumb.
                    if this.note_row_title(emitter, cx) {
                        this.sidebar.update(cx, |_, cx| cx.notify());
                        if visible {
                            this.breadcrumb.update(cx, |_, cx| cx.notify());
                        }
                    }
                }
                TermEvent::Exit(code) => {
                    this.on_session_exit(emitter.clone(), *code, &program, window, cx)
                }
            },
        )
        .detach();
    }

    /// Shared body of restart/resume: kill the old PTY (if alive) and
    /// spawn a fresh one, optionally resuming the agent session.
    pub(super) fn respawn_current_session(
        &mut self,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Everything the spawn needs, copied out first: subscribing hands
        // the app a callback and needs `&mut self`, which the project
        // borrow above would still be holding.
        let (spec, program, title, old) = {
            let Some(session) = self
                .projects
                .get_mut(self.current_project)
                .and_then(|p| p.sessions.get_mut(self.current_session))
            else {
                return;
            };
            let cwd = session.cwd.clone();
            let spec = match &resume {
                Some(id) => session.cmd.resume_spec(&cwd, id),
                None => session.cmd.spec(&cwd),
            };
            (
                spec,
                session.cmd.program.clone(),
                session.cmd.basename(),
                session.term.take(),
            )
        };
        if let Some(old) = old {
            old.update(cx, |s, _| s.kill());
        }
        let scrollback = cx.global::<ddu_core::config::Config>().terminal_scrollback();
        let (status, term) = match TermSession::spawn(&spec, scrollback, cx) {
            Ok(term) => {
                self.subscribe_term(&term, program, window, cx);
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
        if let Some(session) = self
            .projects
            .get_mut(self.current_project)
            .and_then(|p| p.sessions.get_mut(self.current_session))
        {
            session.status = status;
            session.title = title;
            session.term = term;
            // The previous run is gone for good: only this new one's
            // liveness matters from here on.
            session.was_live = false;
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
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, gpui};
    #[test]
    pub(super) fn removal_keeps_selection_or_nearest_neighbor() {
        assert_eq!(index_after_removal(2, 0, 3), 1);
        assert_eq!(index_after_removal(1, 2, 3), 1);
        assert_eq!(index_after_removal(3, 3, 3), 2);
        assert_eq!(index_after_removal(0, 0, 0), 0);
    }

    /// An app with one session row backed by a live `/bin/cat` PTY, whose
    /// launcher *kind* the caller states — the close paths turn on the
    /// kind, not on what the PTY runs, so a test never needs an agent
    /// binary installed to be an agent row.
    ///
    /// The window context comes back *cloned*: `add_window_view` hands out
    /// a borrow of the one it built, which cannot outlive the context that
    /// owns it.
    ///
    /// The app goes under `gpui_component::Root` exactly as `main.rs`
    /// mounts it — without that first layer there is no dialog stack, and
    /// `open_alert_dialog` panics instead of asking.
    fn app_with_one_session(
        dispatcher: gpui::TestDispatcher,
        kind: &str,
        name: &'static str,
    ) -> (Entity<crate::app::AppView>, VisualTestContext) {
        use crate::app::AppView;
        use ddu_core::config::{Config, LoadWarnings, ProjectConfig, ShellConfig, State};
        use gpui_kit::AppContext as _;
        use gpui_kit::component::Root;
        use std::cell::RefCell;
        use std::rc::Rc;

        let mut cx = TestAppContext::build(dispatcher, Some(name));
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            cx.set_app_identity("dev.just.ddu", "Day Day Up");
            cx.set_global(Config {
                shell: ShellConfig {
                    program: "/bin/cat".into(),
                },
                ..Default::default()
            });
            cx.set_global(LoadWarnings(vec![]));
            cx.set_global(State {
                projects: Some(vec![ProjectConfig {
                    name: "proj".into(),
                    path: std::env::temp_dir(),
                    expanded: true,
                    tree_open: vec![],
                    tree_height: None,
                    tree_scroll: None,
                    sessions: vec![],
                }]),
                ..Default::default()
            });
        });
        let built: Rc<RefCell<Option<Entity<AppView>>>> = Rc::new(RefCell::new(None));
        let slot = built.clone();
        let (_root, vcx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|cx| AppView::new(window, cx));
            *slot.borrow_mut() = Some(view.clone());
            Root::new(view, window, cx)
        });
        let view = built.borrow().clone().expect("the app view");
        // A workspace with no saved sessions is seeded by `AppView::new`
        // itself with the default launcher — one `terminal` row running
        // `/bin/cat` here. The close paths turn on the row's *kind*, so
        // the test relabels that row instead of spawning a second one.
        vcx.update(|_, cx| {
            view.update(cx, |v, _| {
                assert_eq!(
                    v.projects[0].sessions.len(),
                    1,
                    "the launch seeds one session"
                );
                v.projects[0].sessions[0].kind = kind.to_string();
            });
        });
        let vcx = vcx.clone();
        cx.run_until_parked();
        (view, vcx)
    }

    /// Closing a *live agent* asks first, and the confirm stops it without
    /// taking the row: that row is what carries the resume id the dialog
    /// just promised to save, and a second close — a dead row by then — is
    /// what removes it.
    ///
    /// The decision belongs to the click, not to the status at confirm
    /// time: a dialog left open while the agent finishes on its own must
    /// not send the confirmed close down the dead-row path and delete the
    /// row (and the conversation) instead of stopping it.
    #[test]
    fn closing_a_live_agent_asks_first_and_keeps_its_row() {
        use ddu_core::session::AgentStatus;
        use gpui_kit::component::WindowExt as _;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (view, mut vcx) =
                    app_with_one_session(dispatcher, "omp", "close_live_agent_asks");
                let term = vcx.update(|_, cx| {
                    view.read(cx).projects[0].sessions[0]
                        .term
                        .clone()
                        .unwrap()
                });

                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| v.request_close_session(0, 0, window, cx));
                    assert!(
                        window.has_active_dialog(cx),
                        "a live agent is asked about first"
                    );
                });
                assert_eq!(
                    vcx.update(|_, cx| view.read(cx).projects[0].sessions.len()),
                    1,
                    "and nothing is closed behind the dialog"
                );

                // The agent exits while the dialog sits open, and only then
                // does the user confirm.
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.projects[0].sessions[0].status = AgentStatus::Done(0);
                        v.stop_live_agent(0, 0, window, cx);
                    });
                });
                assert_eq!(
                    vcx.update(|_, cx| view.read(cx).projects[0].sessions.len()),
                    1,
                    "the confirmed close keeps the row it promised to keep"
                );

                // The second close: a dead row now, gone outright.
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| v.request_close_session(0, 0, window, cx));
                });
                assert_eq!(
                    vcx.update(|_, cx| view.read(cx).projects[0].sessions.len()),
                    0,
                    "closing the stopped row is what removes it"
                );

                vcx.update(|_, cx| term.update(cx, |s, _| s.kill_and_join()));
                vcx.update(|_, cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                vcx.run_until_parked();
            }),
        );
    }

    /// A plain shell closes at once, no question and nothing left behind:
    /// a terminal row holds no conversation state, so there is nothing to
    /// save and nothing to come back to. (An agent the user started by
    /// hand inside a `Terminal` row is a shell row to this rule — which is
    /// the other half of why two closes can look different.)
    #[test]
    fn a_shell_row_closes_at_once_without_asking() {
        use gpui_kit::component::WindowExt as _;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (view, mut vcx) =
                    app_with_one_session(dispatcher, "terminal", "close_shell_row");
                let term = vcx.update(|_, cx| {
                    view.read(cx).projects[0].sessions[0]
                        .term
                        .clone()
                        .unwrap()
                });

                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| v.request_close_session(0, 0, window, cx));
                    assert!(
                        !window.has_active_dialog(cx),
                        "a shell is not worth a confirm dialog"
                    );
                });
                assert_eq!(
                    vcx.update(|_, cx| view.read(cx).projects[0].sessions.len()),
                    0,
                    "its row goes in the same click"
                );

                vcx.update(|_, cx| term.update(cx, |s, _| s.kill_and_join()));
                vcx.update(|_, cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                vcx.run_until_parked();
            }),
        );
    }

    /// A graceful quit must leave the rows that were alive marked for the
    /// next launch. The shutdown's own saves run AFTER the children exit,
    /// when every row already reads `Done` — so the mark has to be taken
    /// up front, or a relaunch silently restores nothing and the user has
    /// to click Resume on each row.
    #[test]
    fn shutdown_keeps_live_rows_marked_for_the_next_launch() {
        use crate::app::AppView;
        use ddu_core::config::{Config, ProjectConfig, SavedSession, ShellConfig, State};
        use ddu_core::session::AgentStatus;
        use gpui_kit::{TestAppContext, gpui};

        let live_row = SavedSession {
            kind: "terminal".into(),
            title: "zsh".into(),
            resume: None,
            live: Some(true),
            selected_file: None,
            view_mode: None,
        };
        let finished_row = SavedSession {
            kind: "omp".into(),
            title: "omp".into(),
            resume: Some("01a075e1-346f-7b92-b832-745a71ee00ed".into()),
            live: Some(false),
            selected_file: None,
            view_mode: None,
        };
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("shutdown_liveness"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(ddu_core::config::LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![ProjectConfig {
                            name: "p".into(),
                            path: std::env::temp_dir(),
                            expanded: true,
                            tree_open: vec![],
                            tree_height: None,
                            tree_scroll: None,
                            sessions: vec![live_row, finished_row],
                        }]),
                        ..Default::default()
                    });
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                // The quit begins while the shell is still alive.
                vcx.update(|_, cx| view.update(cx, |v, _| v.mark_live_rows()));
                // Its child then exits during the shutdown sequence.
                vcx.update(|_, cx| {
                    view.update(cx, |v, _| {
                        v.projects[0].sessions[0].status = AgentStatus::Done(0);
                    })
                });
                let saved = vcx.update(|_, cx| view.update(cx, |v, cx| v.snapshot(cx)));
                let rows = &saved.projects.as_ref().unwrap()[0].sessions;
                assert_eq!(
                    rows[0].live,
                    Some(true),
                    "the row that was alive must come back"
                );
                assert_eq!(rows[1].live, Some(false), "the finished row must not");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
