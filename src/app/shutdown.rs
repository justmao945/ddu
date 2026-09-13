//! Graceful exit: a window close or ⌘Q with live agents behind it asks
//! first, then interrupts each row, saves their resume ids and only then
//! closes the window (or exits the process). Everything the confirmed
//! close/quit path touches lives here.

use super::*;

impl AppView {

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
        self.begin_shutdown(cx);
    }

    /// Kick off the shutdown sequence: remember which rows are still
    /// alive (they must come back running on the next launch), save
    /// every live session's resume id first (a later kill must not
    /// lose it), then run the stop escalation on all of them at once.
    fn begin_shutdown(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn mark_live_rows(&mut self) {
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
    pub(super) fn escalate_close(terms: Vec<Entity<TermSession>>, cx: &mut Context<Self>) {
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
    pub(super) fn finish_shutdown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shutting_down = false;
        self.persist(cx);
        if self.quit_after_shutdown {
            std::process::exit(0);
        } else {
            window.remove_window();
        }
    }
}
