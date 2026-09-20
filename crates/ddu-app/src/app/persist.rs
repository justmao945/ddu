//! The workspace snapshot: writing `state.json` and restoring the
//! persisted session lists at launch.

use super::*;

impl AppView {
    /// Write current projects + expanded flags + panel geometry into the
    /// global state snapshot and save to disk. Save errors are reported
    /// (notification or stderr) but never crash the app.
    pub(crate) fn persist(&mut self, cx: &mut App) {
        // Sync the pane's own state onto the current session's slot: only
        // the selected file and the mode are a session's (the file tree
        // belongs to the project).
        let selected_path = self.current_diff_path().map(str::to_owned);
        let has_snapshot = self.snapshot.is_some();
        if let Some(s) = self
            .projects
            .get_mut(self.current_project)
            .and_then(|p| p.sessions.get_mut(self.current_session))
        {
            // Only the loaded diff knows the truth about the
            // selection: a save that lands before the first poll
            // (the window-frame debounce does) must not overwrite
            // the stored path with `None`.
            // The stored path is only overwritten once the diff has
            // loaded: a save landing before the first poll (the
            // window-frame debounce does) knows nothing about the
            // working tree, and writing `None` there would drop the
            // row's selection.
            if selected_path.is_some() || has_snapshot {
                s.diff_selected = selected_path;
            }
            s.view_mode = Some(self.view_mode.as_str().to_owned());
        }
        // The tree's expansion lives on the project already (it is the
        // project's), so only the layer height has to be written back —
        // and only while the layer is on screen: a hidden one reports no
        // live height, and the stored one must stay.
        if let Some(h) = self.live_tree_height(cx) {
            if let Some(project) = self.projects.get_mut(self.current_project) {
                project.tree_height = Some(h);
            }
        }
        // The layer's place is written back the same way — but never while
        // a restore is still waiting for rows: a switch zeroes the handle,
        // and the incoming project's place would be overwritten by that
        // zero before it is ever applied (`pending_tree_scroll`).
        if self.show_diff_tree && self.pending_tree_scroll.is_none() {
            let offset = self.diff_tree_scroll.offset().y.as_f32();
            if let Some(project) = self.projects.get_mut(self.current_project) {
                project.tree_scroll = Some(offset);
            }
        }
        let snapshot = self.snapshot(cx);
        // Per-session tree heights ride the session slots above; the
        // window frame is tracked live by `note_window_frame`.
        if let Err(err) = snapshot.save() {
            ddu_core::config::report_error(err, cx);
        }
        cx.set_global(snapshot);
    }

    /// The workspace as it goes to disk: every project with its session
    /// rows (a row still running is marked `live` so the next launch
    /// spawns it again), the panel geometry and the window frame.
    pub(super) fn snapshot(&self, cx: &mut App) -> ddu_core::config::State {
        let mut snapshot = cx.global::<ddu_core::config::State>().clone();
        snapshot.hidden_sessions = !self.show_sessions;
        snapshot.sidebar_width = self.last_sidebar_size.map(|w| w.as_f32());
        snapshot.diff_width = self.last_diff_size.map(|w| w.as_f32());
        snapshot.current_project = self.current_project;
        // The row the user was looking at: without this the launch
        // always opens session 0 of the project.
        snapshot.current_session = self.current_session;
        snapshot.show_diff = self.show_diff;
        snapshot.show_diff_tree = self.show_diff_tree;
        snapshot.window = self.window_placement;
        // Always `Some` once this window persists: the saved list
        // (even empty) is the authoritative workspace on next launch.
        snapshot.projects = Some(
            self.projects
                .iter()
                .zip(&self.expanded)
                .map(|(p, ex)| ddu_core::config::ProjectConfig {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    expanded: *ex,
                    // The file tree is the project's, so its expansion and
                    // layer height ride the project entry.
                    tree_open: p.tree_open.iter().cloned().collect(),
                    tree_height: p.tree_height,
                    tree_scroll: p.tree_scroll,
                    // The whole session list survives the restart, with
                    // the rows that were still running marked `live` so
                    // the next launch spawns them again. An agent's id
                    // comes from its captured output (the live id beats
                    // the row's last known one); shells never carry one.
                    sessions: p
                        .sessions
                        .iter()
                        .map(|s| ddu_core::config::SavedSession {
                            kind: s.kind.clone(),
                            title: s.title.clone(),
                            resume: if s.is_agent() {
                                s.term
                                    .as_ref()
                                    .and_then(|t| {
                                        // Agents keep running at window
                                        // close: scan the output tail
                                        // now, or the next launch has
                                        // nothing to resume.
                                        t.update(cx, |term, _| term.capture_resume_id());
                                        t.read(cx).resume_id().map(String::from)
                                    })
                                    .or_else(|| s.resume_id.clone())
                            } else {
                                None
                            },
                            live: Some(s.status.is_running() || s.was_live),
                            selected_file: s.diff_selected.clone(),
                            view_mode: s.view_mode.clone(),
                        })
                        .collect(),
                })
                .collect(),
        );
        snapshot
    }

    /// Rebuild every project's session rows from the persisted
    /// snapshot. A row that was still running when the workspace was
    /// last saved (`live`) comes back running in its own project — an
    /// agent resumed with its captured conversation id, a shell fresh
    /// (a shell persists nothing but its existence, so a fresh one
    /// matches the pre-close state). Rows that had already finished
    /// restore as `Done` rows waiting for a click. Rows whose launcher
    /// kind was removed from the settings are dropped; the selection
    /// clamps to what remains.
    pub(super) fn restore_sessions(
        &mut self,
        state: &ddu_core::config::State,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.current_project;
        let mut seq = self.session_seq;
        for ix in 0..self.projects.len() {
            let Some(saved) = state
                .projects
                .as_ref()
                .and_then(|ps| ps.get(ix))
                .map(|p| p.sessions.clone())
            else {
                continue;
            };
            for s in saved {
                let cmd = match cx.global::<ddu_core::config::Config>().cmd_for(&s.kind) {
                    Ok(cmd) => cmd,
                    Err(_) => continue,
                };
                seq += 1;
                let now = std::time::Instant::now();
                let cwd = self.projects[ix].path.clone();
                let selected = s.selected_file.clone();
                let (status, term) = if restores_running(&s, ix == current) {
                    let spec = match s.resume.as_deref() {
                        Some(id) if s.kind != "terminal" => cmd.resume_spec(&cwd, id),
                        _ => cmd.spec(&cwd),
                    };
                    let scrollback =
                        cx.global::<ddu_core::config::Config>().terminal_scrollback();
                    match TermSession::spawn(&spec, scrollback, cx) {
                        Ok(term) => {
                            self.subscribe_term(&term, cmd.program.clone(), window, cx);
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
                self.projects[ix]
                    .sessions
                    .push(ddu_core::session::AgentSession {
                        id: format!("restored-{ix}-{seq}"),
                        title: s.title.clone(),
                        status,
                        cmd,
                        resume_id: s.resume.clone(),
                        // A row that comes back running is live in its own
                        // right now; `status` carries that until a close.
                        was_live: false,
                        kind: s.kind,
                        started: now,
                        ended: if term.is_none() { Some(now) } else { None },
                        term,
                        cwd,
                        diff_selected: selected,
                        view_mode: s.view_mode.clone(),
                    });
            }
        }
        self.session_seq = seq;
    }

    /// Record where the current project's file tree is scrolled, from the
    /// live handle. Called by every path that moves `current_project`,
    /// next to [`AppView::remember_scroll`] for the pane: the offset on
    /// the handle belongs to the project leaving the screen, and after
    /// the move it is the incoming project's. A hidden layer reports
    /// nothing, so its stored place stays.
    pub(super) fn remember_tree_scroll(&mut self) {
        if !self.show_diff_tree {
            return;
        }
        let offset = self.diff_tree_scroll.offset().y.as_f32();
        if let Some(project) = self.projects.get_mut(self.current_project) {
            project.tree_scroll = Some(offset);
        }
    }

    /// The splitter's live tree-layer height: `None` while the layer
    /// is hidden (its placeholder size must not overwrite the stored
    /// one) or before the splitter has laid out.
    pub(super) fn live_tree_height(&self, cx: &App) -> Option<f32> {
        if !self.show_diff_tree {
            return None;
        }
        self.sidebar_split_state
            .read(cx)
            .sizes()
            .get(1)
            .copied()
            .filter(|h| *h > px(0.))
            .map(|h| h.as_f32())
    }

    /// Window frame changed (move/resize/zoom/fullscreen): record the
    /// placement for the next persist, and schedule a debounced save so
    /// a crash between the change and the next explicit persist still
    /// keeps it.
    pub(super) fn note_window_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let bounds = window.window_bounds();
        let frame = bounds.get_bounds();
        let mode = match bounds {
            WindowBounds::Windowed(_) => ddu_core::config::WindowMode::Windowed,
            WindowBounds::Maximized(_) => ddu_core::config::WindowMode::Maximized,
            WindowBounds::Fullscreen(_) => ddu_core::config::WindowMode::Fullscreen,
        };
        self.window_placement = Some(ddu_core::config::WindowPlacement {
            mode,
            x: frame.origin.x.as_f32(),
            y: frame.origin.y.as_f32(),
            w: frame.size.width.as_f32(),
            h: frame.size.height.as_f32(),
        });
        self.window_geom_seq += 1;
        let seq = self.window_geom_seq;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.window_geom_seq == seq {
                    this.persist(cx);
                }
            });
        })
        .detach();
    }
}

/// Whether a persisted row comes back running on this launch: it was
/// live at the last close. Workspaces saved before the `live` flag
/// existed carry `None`, and there a shell in the project the window
/// opens on still respawns — its long-standing behavior (a shell
/// persists nothing but its existence, so a fresh one matches the
/// pre-close state); agents of such files stay `Done` rows, since a
/// finished conversation must not start on its own.
fn restores_running(s: &ddu_core::config::SavedSession, opening_project: bool) -> bool {
    match s.live {
        Some(live) => live,
        None => s.kind == "terminal" && opening_project,
    }
}

#[cfg(test)]
mod tests {
    use ddu_core::config::SavedSession;

    fn row(kind: &str, resume: Option<&str>, live: bool) -> SavedSession {
        SavedSession {
            kind: kind.into(),
            title: kind.into(),
            resume: resume.map(String::from),
            live: Some(live),
            selected_file: None,
            view_mode: None,
}
    }

    /// A workspace saved before the flag existed: no liveness on any row.
    fn legacy_row(kind: &str) -> SavedSession {
        SavedSession {
            live: None,
            ..row(kind, None, false)
        }
    }

    /// The user-visible contract of a launch: everything that was still
    /// running comes back running, in every project, and nothing that
    /// had finished starts on its own.
    #[test]
    fn only_rows_that_were_live_come_back_running() {
        // Live agent: back, with the conversation to resume.
        assert!(super::restores_running(
            &row("omp", Some("01a075e1-346f-7b92-b832-745a71ee00ed"), true),
            false
        ));
        // Live agent whose id never got captured: back as a fresh chat.
        assert!(super::restores_running(&row("claude", None, true), false));
        // Finished agent: stays a Done row, even when selected.
        assert!(!super::restores_running(
            &row("omp", Some("01a075e1-346f-7b92-b832-745a71ee00ed"), false),
            true
        ));
        // Live shell in a background project: back too.
        assert!(super::restores_running(&row("terminal", None, true), false));
        // A shell the user exited stays exited — the explicit `false`
        // must beat the legacy rule below, or every launch resurrects
        // rows nobody asked for.
        assert!(!super::restores_running(&row("terminal", None, false), true));
    }

    /// Workspaces written before the `live` flag know nothing about
    /// liveness: the shell in the project the window opens on respawns
    /// as it always did, everything else waits.
    #[test]
    fn legacy_workspaces_keep_their_old_launch_behavior() {
        assert!(super::restores_running(&legacy_row("terminal"), true));
        assert!(!super::restores_running(&legacy_row("terminal"), false));
        assert!(!super::restores_running(&legacy_row("omp"), true));
    }

    /// End to end through the real launch path. Every row that was
    /// running when the workspace was saved comes back running — in
    /// whichever project, not just the one the window opens on — while a
    /// row that had already finished stays a `Done` row carrying its
    /// conversation, even when it is the selected one. The saved
    /// snapshot then marks the same liveness again, so the behavior
    /// repeats every launch.
    #[test]
    fn a_live_row_restores_running_and_is_saved_live_again() {
        use crate::app::AppView;
        use ddu_core::config::{Config, ProjectConfig, ShellConfig, State};
        use ddu_core::session::AgentStatus;
        use gpui_kit::{TestAppContext, gpui};

        let dir = std::env::temp_dir().join("ddu-restore-test");
        let agent_id = "01a075e1-346f-7b92-b832-745a71ee00ed";
        let finished_agent = SavedSession {
            kind: "omp".into(),
            title: "omp".into(),
            resume: Some(agent_id.into()),
            live: Some(false),
            selected_file: None,
            view_mode: None,
        };
        // A shell in a project the window does NOT open on: the old
        // restore left those as `Done` rows, which is exactly the
        // "click Resume to start it" complaint.
        let live_shell = SavedSession {
            kind: "terminal".into(),
            title: "zsh".into(),
            resume: None,
            live: Some(true),
            selected_file: None,
            view_mode: None,
        };
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("restore_live_rows"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    // `cat` keeps the restored shell trivially alive;
                    // the agent row never spawns, so its real binary is
                    // moot.
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(ddu_core::config::LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![
                            ProjectConfig {
                                name: "ddu".into(),
                                path: dir.clone(),
                                expanded: true,
                                tree_open: vec![],
                                tree_height: None,
                                // The launch's project carries its layer's
                                // place into the first build.
                                tree_scroll: Some(-96.),
                                sessions: vec![finished_agent],
                            },
                            ProjectConfig {
                                name: "other".into(),
                                path: dir.clone(),
                                expanded: true,
                                tree_open: vec![],
                                tree_height: None,
                                // The layer's place comes back with the
                                // project, like its expansion and height.
                                tree_scroll: Some(-160.),
                                sessions: vec![live_shell],
                            },
                        ]),
                        ..Default::default()
                    });
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let (dead, live, scroll) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    (
                        v.projects[0].sessions[0].clone(),
                        v.projects[1].sessions[0].clone(),
                        v.projects[1].tree_scroll,
                    )
                });
                assert_eq!(scroll, Some(-160.), "the file tree's place survives the launch");
                assert_eq!(
                    vcx.update(|_, cx| view.read(cx).pending_tree_scroll),
                    Some(gpui::px(-96.)),
                    "the launch's file tree comes back where it was left"
                );
                assert!(live.term.is_some(), "the live shell comes back");
                assert_eq!(live.status, AgentStatus::Running);
                assert!(dead.term.is_none(), "the finished agent stays put");
                assert_eq!(dead.status, AgentStatus::Done(0));
                assert_eq!(
                    dead.resume_id.as_deref(),
                    Some(agent_id),
                    "its conversation stays resumable"
                );

                // The next save records the liveness that this launch
                // sees: the running shell stays `live`, the finished
                // agent does not.
                let saved = vcx.update(|_, cx| view.update(cx, |v, cx| v.snapshot(cx)));
                let projects = saved.projects.as_ref().unwrap();
                assert_eq!(projects[0].sessions[0].live, Some(false), "finished stays finished");
                assert_eq!(
                    projects[0].sessions[0].resume.as_deref(),
                    Some(agent_id),
                    "the conversation id survives the save"
                );
                assert_eq!(
                    projects[1].sessions[0].live,
                    Some(true),
                    "the running shell is saved live"
                );

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// The row the user was looking at is the row the next launch opens.
    ///
    /// The snapshot has to record the live index — it recorded only the
    /// project, so every relaunch reopened the first row — and the
    /// launch has to honor it.
    #[test]
    fn the_active_session_survives_a_relaunch() {
        use crate::app::AppView;
        use ddu_core::config::{Config, ProjectConfig, ShellConfig, State};
        use gpui_kit::{TestAppContext, gpui};

        let dir = std::env::temp_dir().join("ddu-active-session-test");
        let done = |title: &str| SavedSession {
            kind: "omp".into(),
            title: title.into(),
            resume: None,
            live: Some(false),
            selected_file: None,
            view_mode: None,
        };
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("active_session"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    // Nothing comes back running, so no binary is needed.
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(ddu_core::config::LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![ProjectConfig {
                            name: "ddu".into(),
                            path: dir.clone(),
                            expanded: true,
                            tree_open: vec![],
                            tree_height: None,
                            tree_scroll: None,
                            sessions: vec![done("first"), done("second"), done("third")],
                        }]),
                        ..Default::default()
                    });
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let saved = vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        assert_eq!(v.current_session, 0, "the saved state opens row 0");
                        v.current_session = 2;
                        v.snapshot(cx)
                    })
                });
                assert_eq!(
                    saved.current_session, 2,
                    "the snapshot records the row in use"
                );

                // Relaunch on what that save wrote: the row comes back.
                cx.update(|cx| cx.set_global(saved.clone()));
                let (second, scx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                let active = scx.update(|window, cx| {
                    let _ = window.draw(cx);
                    second.read(cx).current_session
                });
                assert_eq!(active, 2, "the launch opens the saved row");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
