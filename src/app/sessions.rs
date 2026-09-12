//! Session lifecycle: selection, spawning, closing, restart/resume
//! and the PTY exit bookkeeping.

use super::*;
use crate::terminal::Attention;

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
        let from = (self.current_project, self.current_session);
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

    /// Create a session of `kind` (`terminal`/builtin) in project
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
            project.sessions.push(crate::session::AgentSession {
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
                diff_closed: Default::default(),
                diff_tree_height: None,
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
        // The splitter under the outgoing session keeps its live
        // height — read it before borrowing the session slot mutably.
        let live_h = self.live_tree_height(cx);
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
                // Hidden layer reports no live height — keep the stored one.
                if let Some(h) = live_h {
                    s.diff_tree_height = Some(h);
                }
            }
        }
        let (seed, closed, height) = {
            let incoming = self.projects.get(to.0).and_then(|p| p.sessions.get(to.1));
            (
                incoming.and_then(|s| s.diff_selected.clone()),
                incoming.map(|s| s.diff_closed.clone()).unwrap_or_default(),
                incoming.and_then(|s| s.diff_tree_height),
            )
        };
        self.diff_seed_path = seed;
        // The adopted height must beat a pinned drag size on the live
        // splitter (a bare seed only wins while the panel is unpinned),
        // so unpin the layer panel and let the next render apply it.
        self.diff_tree_height_seed = height
            .map(gpui::px)
            .filter(|h| h.as_f32() >= TREE_MIN_H as f32 && h.as_f32() <= TREE_MAX_H as f32);
        if self.diff_tree_height_seed.is_some() && self.show_diff_tree {
            self.sidebar_split_state.update(cx, |state, cx| {
                if state.sizes().len() > 1 {
                    state.reset_panel(1, cx);
                }
            });
        }
        self.reset_diff();
        self.diff_tree_closed = closed;
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

    /// Attention marker from a session's PTY: the agent's own "your
    /// turn" signal (turn finished, question asked, run failed). Raise
    /// a desktop notification when that terminal is not in front of
    /// the user — the signal is already on screen otherwise.
    pub(super) fn on_session_attention(
        &mut self,
        emitter: Entity<TermSession>,
        signal: &Attention,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.shutting_down || !cx.global::<crate::config::Config>().notify_on_attention() {
            return;
        }
        let Some(slot) = self.session_slot(&emitter) else {
            return;
        };
        // The terminal in front of the user needs no toast: the marker
        // is already on screen. Anything else — another session, the
        // app in the background — does.
        let on_screen =
            window.is_window_active() && slot == (self.current_project, self.current_session);
        crate::config::debug_log(&format!(
            "attention: body={:?} title={:?} slot={slot:?} on_screen={on_screen}",
            signal.body, signal.title
        ));
        if on_screen {
            return;
        }
        let project = &self.projects[slot.0];
        let session = &project.sessions[slot.1];
        cx.show_system_notification(SystemNotification {
            // One notification per session row: a newer marker from the
            // same agent replaces the older toast instead of stacking.
            tag: format!("ddu-session-{}", session.id).into(),
            title: format!("{} · {}", project.name, session.title).into(),
            body: if signal.body.is_empty() {
                "Needs your attention".into()
            } else {
                signal.body.clone().into()
            },
            actions: Vec::new(),
        });
    }

    /// The `(project, session)` row owning `term`, if it is still listed.
    fn session_slot(&self, term: &Entity<TermSession>) -> Option<(usize, usize)> {
        self.projects.iter().enumerate().find_map(|(p, project)| {
            project
                .sessions
                .iter()
                .position(|s| s.term.as_ref() == Some(term))
                .map(|six| (p, six))
        })
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
    /// gracefully: the escalation (Esc → 2×Ctrl-C → 2×Ctrl-D → kill)
    /// makes the child exit and print its resume banner; the id is
    /// captured and saved — and the row STAYS as a resumable Done
    /// entry. A plain terminal just gets one Ctrl-C (stops a
    /// foreground job) before its PTY drops. Dead rows and closed
    /// terminals are removed outright; removing a current session
    /// shifts the selection to the nearest neighbor.
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
            if session.kind == "terminal" {
                // Plain shell: nothing resumable at stake. One Ctrl-C
                // stops a foreground job, then the PTY drops — no
                // agent stop escalation; the row is removed at once.
                crate::config::debug_log(&format!(
                    "close: terminal ctrl-c + kill (row {}:{six})",
                    p
                ));
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
            } else {
                // Save the resume id BEFORE the stop sequence: if the
                // agent dies without printing its banner (the kill at
                // the end of the escalation), the id from its earlier
                // output is already on disk.
                term.update(cx, |s, _| s.capture_resume_id());
                self.persist(cx);
                // The exit event persists the row again with the banner id.
                Self::escalate_close(vec![term.clone()], cx);
                cx.notify();
                return;
            }
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

    /// Live PTYs of agent sessions — the only sessions a quit confirm
    /// protects. Plain shells hold no conversation state, so closing
    /// them loses nothing and needs no dialog.
    pub(crate) fn running_agent_terms(&self) -> Vec<Entity<TermSession>> {
        self.projects
            .iter()
            .flat_map(|p| p.sessions.iter())
            .filter(|s| s.status.is_running() && s.kind != "terminal")
            .filter_map(|s| s.term.clone())
            .collect()
    }

    /// Window close (red button): confirms only when live agent
    /// sessions are at stake, then runs the exit process — live agents
    /// get Ctrl-C, their resume ids land in state.json, then the window
    /// goes away. Returns false while the dialog or the exit process is
    /// live.
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
        let live = self.running_agent_terms().len();
        if live == 0 {
            // Only shells (or nothing) live: nothing resumable is at
            // stake, so skip the dialog and exit straight away.
            self.begin_exit(quit, window, cx);
            return;
        }
        let what = if quit { "Quit" } else { "Close window" };
        let description = format!(
            "{live} agent session{} running. They will be stopped now and their session ids saved so the conversations can be resumed.",
            if live == 1 { " is" } else { "s are" }
        );
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

    /// Kick off the shutdown sequence: remember which rows are still
    /// alive (they must come back running on the next launch), save
    /// every live session's resume id first (a later kill must not
    /// lose it), then run the stop escalation on all of them at once.
    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
        crate::config::debug_log("begin_shutdown: capture ids, escalate");
        self.mark_live_rows();
        let terms = self.running_terms();
        for term in &terms {
            term.update(cx, |s, _| s.capture_resume_id());
        }
        self.persist(cx);
        Self::escalate_close(terms, cx);
        cx.notify();
    }

    /// Remember every row whose PTY is still alive: it must come back
    /// running on the next launch. Ordered BEFORE the stop escalation —
    /// once the children exit, their rows are `Done` and only this flag
    /// still knows they were live at close.
    fn mark_live_rows(&mut self) {
        for project in self.projects.iter_mut() {
            for session in project.sessions.iter_mut() {
                if session.status.is_running() {
                    session.was_live = true;
                }
            }
        }
    }

    /// Graceful-stop sequence for live terms, shared by single-session
    /// close and window/app shutdown:
    ///
    /// 1. **Esc** — break an ongoing generation/prompt so the agent
    ///    is back at its input line.
    /// 2. **2× Ctrl-C**, 400ms apart — agents quit on this (Claude
    ///    only exits on the second press).
    /// 3. **2× Ctrl-D** after Ctrl-C had 1.5s to work — EOF on an
    ///    empty prompt closes omp, codex and plain shells.
    /// 4. **kill** at ~5s — sweeps whatever ignored all of it. The
    ///    Exit event still fires, so the tail may still carry the
    ///    resume id.
    ///
    /// Every stage is a no-op for terms that already exited.
    fn escalate_close(terms: Vec<Entity<TermSession>>, cx: &mut Context<Self>) {
        crate::config::debug_log(&format!("escalate_close: {} live", terms.len()));
        for term in &terms {
            term.update(cx, |s, _| s.ctrl(0x1b));
        }
        cx.spawn(async move |this, cx| {
            // Two Ctrl-C, spaced like a human double-press.
            for _ in 0..2 {
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
                this.update(cx, |_, cx| {
                    for term in &terms {
                        term.update(cx, |s, _| s.ctrl(0x03));
                    }
                })?;
            }
            // Ctrl-C had its chance; two Ctrl-D on the prompt.
            cx.background_executor()
                .timer(Duration::from_millis(1500))
                .await;
            for _ in 0..2 {
                this.update(cx, |_, cx| {
                    for term in &terms {
                        term.update(cx, |s, _| s.ctrl(0x04));
                    }
                })?;
                cx.background_executor()
                    .timer(Duration::from_millis(400))
                    .await;
            }
            // Anything still alive at ~5s is killed outright.
            cx.background_executor()
                .timer(Duration::from_millis(1600))
                .await;
            this.update(cx, |_, cx| {
                for term in &terms {
                    term.update(cx, |s, _| s.kill());
                }
            })?;
            anyhow::Ok(())
        })
        .detach();
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
        let Some(id) = self.current_resume_id(cx) else {
            crate::config::debug_log("resume: NO ID — aborting");
            eprintln!("[ddu] No session id found in the last run's output.");
            return;
        };
        crate::config::debug_log(&format!("resume: respawning with id={id}"));
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
    /// repaint, attention raises a toast, exit settles the row.
    ///
    /// Only a wakeup from the session the center pane renders may
    /// repaint: a background row keeps parsing (its grid must be current
    /// when the row is selected) but changes nothing on screen, so N
    /// streaming agents must not each drive a full-window redraw — that
    /// multiplies the frame rate the pump's throttle exists to bound.
    /// Background rows pick up their OSC title/status on the next
    /// repaint (the 1 Hz tick); exit, attention, the diff poll and
    /// interaction all keep their own notify.
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
                    // Background rows repaint nothing (see
                    // `is_visible_term`).
                    if this.is_visible_term(emitter) {
                        this.terminal_pane.update(cx, |_, cx| cx.notify());
                    }
                }
                TermEvent::Attention(signal) => {
                    this.on_session_attention(emitter.clone(), signal, window, cx)
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
        let (status, term) = match TermSession::spawn(&spec, cx) {
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
    #[test]
    pub(super) fn removal_keeps_selection_or_nearest_neighbor() {
        assert_eq!(index_after_removal(2, 0, 3), 1);
        assert_eq!(index_after_removal(1, 2, 3), 1);
        assert_eq!(index_after_removal(3, 3, 3), 2);
        assert_eq!(index_after_removal(0, 0, 0), 0);
    }

    /// A graceful quit must leave the rows that were alive marked for the
    /// next launch. The shutdown's own saves run AFTER the children exit,
    /// when every row already reads `Done` — so the mark has to be taken
    /// up front, or a relaunch silently restores nothing and the user has
    /// to click Resume on each row.
    #[test]
    fn shutdown_keeps_live_rows_marked_for_the_next_launch() {
        use crate::app::AppView;
        use crate::config::{Config, ProjectConfig, SavedSession, ShellConfig, State};
        use crate::session::AgentStatus;
        use gpui_kit::{TestAppContext, gpui};

        let live_row = SavedSession {
            kind: "terminal".into(),
            title: "zsh".into(),
            resume: None,
            live: Some(true),
            selected_file: None,
            closed_dirs: vec![],
            tree_height: None,
        };
        let finished_row = SavedSession {
            kind: "omp".into(),
            title: "omp".into(),
            resume: Some("01a075e1-346f-7b92-b832-745a71ee00ed".into()),
            live: Some(false),
            selected_file: None,
            closed_dirs: vec![],
            tree_height: None,
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
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![ProjectConfig {
                            name: "p".into(),
                            path: std::env::temp_dir(),
                            expanded: true,
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

    /// End to end for the notification path: an agent-side "your turn"
    /// marker from a background session reaches the OS notification
    /// center, named after that row. The marker's byte-level trip (PTY
    /// → scanner → channel) is covered by
    /// `pty_attention_signals_reach_the_pump` outside the harness; here
    /// the event is emitted straight from the session entity so nothing
    /// crosses into the deterministic scheduler (a real shell's prompt
    /// bytes wake the pump task from the reader thread, which the test
    /// scheduler rejects as nondeterministic). `/bin/cat` with no args
    /// blocks on stdin forever and never prints — both sessions stay
    /// silent, keeping the whole run on the test thread.
    #[test]
    #[cfg(unix)]
    fn attention_raises_a_desktop_notification() {
        use crate::app::AppView;
        use crate::config::{Config, ProjectConfig, ShellConfig, State};
        use crate::terminal::Attention;
        use gpui_kit::{TestAppContext, gpui};

        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("attention_notify"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_app_identity("dev.just.ddu", "Day Day Up");
                    cx.set_global(Config {
                        shell: ShellConfig {
                            // Silent child: no prompt bytes, no
                            // cross-thread scheduler wakeups.
                            program: "/bin/cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![ProjectConfig {
                            name: "proj".into(),
                            path: std::env::temp_dir(),
                            expanded: true,
                            sessions: vec![],
                        }]),
                        ..Default::default()
                    });
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                // AppView::new spawned row 0; add the background row and
                // leave the selection on row 0 — a toast is owed exactly
                // because that terminal is not the one on screen.
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                    view.update(cx, |v, cx| {
                        v.spawn_session_of("terminal", window, cx);
                        v.select_session(0, 0, window, cx);
                    });
                });
                cx.update(|cx| {
                    let term = view.update(cx, |v, _| {
                        v.projects[0].sessions[1]
                            .term
                            .clone()
                            .expect("second session spawned")
                    });
                    term.update(cx, |_, cx| {
                        cx.emit(crate::terminal::TermEvent::Attention(Attention {
                            title: None,
                            body: "turn complete".into(),
                        }));
                    });
                });
                cx.run_until_parked();

                let notes = cx.shown_system_notifications();
                assert_eq!(notes.len(), 1, "one toast for the marker");
                assert_eq!(notes[0].title, "proj · cat");
                assert_eq!(notes[0].body, "turn complete");
                assert_eq!(notes[0].tag, "ddu-session-s-2");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
