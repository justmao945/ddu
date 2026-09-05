//! App shell view: Zed-style three-pane workspace.
//!
//! State, keyboard actions and session mutations live here; the four
//! surface regions live in [`crate::ui`] as `impl AppView` blocks.
//!
//! Left: session list. Center: the live agent terminal (PTY-backed).
//! Right: the project's git diff (HEAD→workdir, polled).

use std::time::Duration;

use gpui_kit::component::notification::Notification;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::diff::{git, GitDiff};
use crate::session::{initial_projects, AgentStatus, Project};
use crate::terminal::{TermEvent, TermSession};
use crate::ui;

// Global keyboard actions: new session, dock toggles, close session.
gpui_kit::actions!(ddu, [NewSession, ToggleSessions, ToggleDiff, CloseSession]);

/// Seconds between working-tree diff polls.
const DIFF_POLL_SECS: u64 = 3;

pub struct AppView {
    pub(crate) projects: Vec<crate::session::Project>,
    /// Which project rows are expanded in the sidebar tree.
    pub(crate) expanded: Vec<bool>,
    pub(crate) current_project: usize,
    pub(crate) current_session: usize,
    pub(crate) show_sessions: bool,
    pub(crate) show_diff: bool,
    /// Owns the three-pane splitter sizes (see `set_sessions`/`set_diff`).
    pub(crate) resize_state: Entity<ResizableState>,
    /// Project row currently under the mouse (hides/reveals its buttons).
    pub(crate) hovered_project: Option<usize>,
    pub(crate) diff_file: usize,
    pub(crate) session_seq: usize,
    pub(crate) diff: Option<GitDiff>,
    /// Guards against stale poll results overwriting newer ones.
    diff_seq: u64,
}

/// Panel geometry (px): defaults, drag limits and collapse thresholds.
const SIDEBAR_DEFAULT: f32 = 232.;
const SIDEBAR_MAX: f32 = 420.;
const SIDEBAR_MIN: f32 = 150.;
const DIFF_DEFAULT: f32 = 340.;
const DIFF_MAX: f32 = 600.;
const DIFF_MIN: f32 = 200.;
/// The terminal pane never shrinks below this while dragging a divider.
const CENTER_MIN: f32 = 400.;


impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Global shortcuts: cmd-t new session, cmd-\ sessions, cmd-b diff,
        // cmd-w close.
        cx.bind_keys([
            KeyBinding::new("cmd-t", NewSession, None),
            KeyBinding::new("cmd-\\", ToggleSessions, None),
            KeyBinding::new("cmd-b", ToggleDiff, None),
            KeyBinding::new("cmd-w", CloseSession, None),
        ]);

        let cfg = cx.global::<crate::config::Config>().clone();
        let projects: Vec<Project> = if cfg.projects.is_empty() {
            initial_projects()
        } else {
            cfg.projects
                .iter()
                .map(|p| Project { name: p.name.clone(), path: p.path.clone(), sessions: vec![] })
                .collect()
        };
        let mut expanded = projects.iter().map(|_| true).collect::<Vec<_>>();
        for (ix, p) in cfg.projects.iter().enumerate() {
            if ix < expanded.len() {
                expanded[ix] = p.expanded;
            }
        }
        let mut this = Self {
            projects,
            expanded,
            current_project: 0,
            current_session: 0,
            show_sessions: true,
            show_diff: false,
            resize_state: cx.new(|_| ResizableState::default()),
            hovered_project: None,
            diff_file: 0,
            session_seq: 0,
            diff: None,
            diff_seq: 0,
        };
        // Dragging a divider below a side pane's min collapses it on
        // release (`Resized` fires at drag end, sizes are real by then).
        cx.subscribe_in(&this.resize_state, window, |this, _, _: &ResizablePanelEvent, _, cx| {
            let sizes = this.resize_state.read(cx).sizes().clone();
            if this.show_sessions && sizes.first().is_some_and(|w| *w < px(SIDEBAR_MIN)) {
                this.show_sessions = false;
                this.resize_state.update(cx, |state, cx| state.remove_panel(0, cx));
                cx.notify();
            }
            if this.show_diff {
                let ix = usize::from(this.show_sessions) + 1;
                if sizes.get(ix).is_some_and(|w| *w < px(DIFF_MIN)) {
                    this.show_diff = false;
                    this.resize_state.update(cx, |state, cx| state.remove_panel(ix, cx));
                    cx.notify();
                }
            }
        })
        .detach();
        this.start_diff_poll(cx);
        this.start_ui_tick(cx);
        // First session for the first project, honoring the configured
        // default launcher.
        this.spawn_session_of(&cfg.new_session.kind, window, cx);
        this
    }

    pub(crate) fn current_project(&self) -> &Project {
        &self.projects[self.current_project]
    }

    pub(crate) fn current_session(&self) -> Option<&crate::session::AgentSession> {
        self.current_project().sessions.get(self.current_session)
    }

    pub(crate) fn current_term(&self) -> Option<Entity<TermSession>> {
        self.current_session().and_then(|s| s.term.clone())
    }

    pub(crate) fn select_session(
        &mut self,
        project: usize,
        session: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
        self.diff_file = 0;
        self.reload_diff(cx);
        if let Some(term) = self.current_term() {
            let focus = term.read(cx).focus.clone();
            focus.focus(window, cx);
        }
        cx.notify();
    }

    /// Create a session with the default launcher and focus it.
    pub(crate) fn spawn_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let kind = cx.global::<crate::config::Config>().new_session.kind.clone();
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
        let Some(cmd) = cx.global::<crate::config::Config>().cmd_for(kind) else {
            window.push_notification(
                Notification::error(format!("Unknown session type: {kind}")),
                cx,
            );
            return;
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
                cx.subscribe_in(&term, window, move |this, emitter, event: &TermEvent, window, cx| {
                    match event {
                        TermEvent::Wakeup => cx.notify(),
                        TermEvent::Exit(code) => {
                            this.on_session_exit(emitter.clone(), *code, &program, window, cx)
                        }
                    }
                })
                .detach();
                let focus = term.read(cx).focus.clone();
                focus.focus(window, cx);
                (AgentStatus::Running, Some(term))
            }
            Err(err) => {
                window.push_notification(
                    Notification::error(format!("Failed to start {}: {err}", cmd.label())),
                    cx,
                );
                (AgentStatus::Error(err.to_string()), None)
            }
        };

        if let Some(project) = self.projects.get_mut(self.current_project) {
            project.sessions.push(crate::session::AgentSession {
                id: format!("s-{seq}"),
                title,
                status,
                cmd,
                kind: kind.to_string(),
                started: std::time::Instant::now(),
                ended: None,
                term,
            });
            self.current_session = project.sessions.len() - 1;
        }
        self.diff_file = 0;
        self.reload_diff(cx);
        cx.notify();
    }

    /// Opens the system folder picker. Uses rfd's async API on purpose:
    /// the sync picker runs a nested modal runloop inside the click
    pub(crate) fn add_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        window.spawn(cx, async move |cx| {
            let picked = rfd::AsyncFileDialog::new()
                .set_title("Choose a project folder")
                .pick_folder()
                .await
                .map(|handle| handle.path().to_string_lossy().into_owned());
            let _ = view.update_in(cx, |this, _window, cx| {
                if let Some(path) = picked {
                    this.add_project_path(&path, cx);
                }
            });
        })
        .detach();
    }



    pub(crate) fn add_project_path(&mut self, raw: &str, cx: &mut Context<Self>) {
        let path = std::path::PathBuf::from(raw);
        if !path.is_dir() {
            return;
        }
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| raw.to_string());
        if self.projects.iter().any(|p| p.path == path) {
            return;
        }
        self.projects.push(Project { name, path: path.clone(), sessions: vec![] });
        self.expanded.push(true);
        self.current_project = self.projects.len() - 1;
        self.current_session = 0;
        self.diff_file = 0;
        self.persist(cx);
        self.reload_diff(cx);
        cx.notify();
    }

    /// Write current projects + expanded flags into the global config
    /// and save to disk.
    pub(crate) fn persist(&self, cx: &mut App) {
        let mut snapshot = cx.global::<crate::config::Config>().clone();
        snapshot.projects = self
            .projects
            .iter()
            .zip(&self.expanded)
            .map(|(p, ex)| crate::config::ProjectConfig {
                name: p.name.clone(),
                path: p.path.clone(),
                expanded: *ex,
            })
            .collect();
        snapshot.save();
        cx.set_global(snapshot);
    }

    /// Remove a project from the sidebar (config persists the change).
    pub(crate) fn remove_project(&mut self, project: usize, cx: &mut Context<Self>) {
        if project >= self.projects.len() || self.projects.len() == 1 {
            return;
        }
        self.projects.remove(project);
        self.expanded.remove(project);
        self.current_project = self.current_project.min(self.projects.len() - 1);
        self.current_session = 0;
        self.diff_file = 0;
        self.persist(cx);
        self.reload_diff(cx);
        cx.notify();
    }
    /// Exit event from a session's PTY pump: mirror the status and notify.
    fn on_session_exit(
        &mut self,
        emitter: Entity<TermSession>,
        code: i32,
        program: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let status = match code {
            0 => AgentStatus::Done(0),
            code if code > 0 => AgentStatus::Done(code),
            code => AgentStatus::Error(format!("abnormal exit {code}")),
        };
        let ended = std::time::Instant::now();
        for session in self.projects[self.current_project]
            .sessions
            .iter_mut()
            .filter(|s| s.term.as_ref() == Some(&emitter))
        {
            session.status = status.clone();
            session.ended = Some(ended);
        }
        let message = match code {
            0 => format!("{program} finished"),
            code => format!("{program} exited with code {code}"),
        };
        let notification = if code == 0 {
            Notification::info(message)
        } else {
            Notification::warning(message)
        };
        window.push_notification(notification, cx);
        cx.notify();
    }

    /// Close (and kill if needed) the current session.
    fn close_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get_mut(self.current_project) else {
            return;
        };
        if project.sessions.is_empty() {
            return;
        }
        let ix = self.current_session.min(project.sessions.len() - 1);
        let session = project.sessions.remove(ix);
        if let Some(term) = &session.term {
            term.update(cx, |s, _| s.kill());
        }
        self.current_session = self.current_session.min(project.sessions.len().saturating_sub(1));
        self.diff_file = 0;
        window.push_notification(Notification::info(format!("Closed “{}”", session.title)), cx);
        cx.notify();
    }

    /// Restart the current session with the same command in a fresh PTY.
    pub(crate) fn restart_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get_mut(self.current_project) else {
            return;
        };
        let cwd = project.path.clone();
        let Some(session) = project.sessions.get_mut(self.current_session) else {
            return;
        };
        let title = session.title.clone();
        let cmd = session.cmd.clone();
        if let Some(old) = session.term.take() {
            old.update(cx, |s, _| s.kill());
        }
        let (status, term) = match TermSession::spawn(&cmd.spec(&cwd), cx) {
            Ok(term) => {
                let program = cmd.program.clone();
                cx.subscribe_in(&term, window, move |this, emitter, event: &TermEvent, window, cx| {
                    match event {
                        TermEvent::Wakeup => cx.notify(),
                        TermEvent::Exit(code) => {
                            this.on_session_exit(emitter.clone(), *code, &program, window, cx)
                        }
                    }
                })
                .detach();
                let focus = term.read(cx).focus.clone();
                focus.focus(window, cx);
                (AgentStatus::Running, Some(term))
            }
            Err(err) => (AgentStatus::Error(err.to_string()), None),
        };
        if let Some(session) = project.sessions.get_mut(self.current_session) {
            session.status = status;
            session.title = cmd.basename();
            session.term = term;
            session.started = std::time::Instant::now();
            session.ended = None;
        }
        window.push_notification(Notification::info(format!("Restarted “{title}”")), cx);
        cx.notify();
    }

    /// Toggle the sidebar, keeping the splitter slot list in sync.
    pub(crate) fn toggle_sessions(&mut self, cx: &mut Context<Self>) {
        self.set_sessions(!self.show_sessions, cx);
    }

    pub(crate) fn set_sessions(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.show_sessions {
            return;
        }
        self.show_sessions = on;
        self.resize_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(px(SIDEBAR_DEFAULT)), Some(0), cx);
            } else {
                state.remove_panel(0, cx);
            }
        });
        cx.notify();
    }

    /// Toggle the diff panel (slot sits after the always-present center).
    pub(crate) fn toggle_diff(&mut self, cx: &mut Context<Self>) {
        self.set_diff(!self.show_diff, cx);
    }

    pub(crate) fn set_diff(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.show_diff {
            return;
        }
        let ix = usize::from(self.show_sessions) + 1;
        self.show_diff = on;
        self.resize_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(px(DIFF_DEFAULT)), Some(ix), cx);
            } else {
                state.remove_panel(ix, cx);
            }
        });
        cx.notify();
    }
    fn request_close_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.current_session() else {
            return;
        };
        let running = session.status.is_running();
        let title = session.title.clone();
        if !running {
            self.close_current_session(window, cx);
            return;
        }
        let this = cx.weak_entity();
        window.open_alert_dialog(cx, move |alert, _, _| {
            alert
                .title(format!("Close “{title}”?"))
                .description("The running agent will be stopped.")
                .show_cancel(true)
                .on_ok({
                    let this = this.clone();
                    move |_, window, cx| {
                        if let Some(this) = this.upgrade() {
                            this.update(cx, |v, cx| v.close_current_session(window, cx));
                        }
                        true
                    }
                })
        });
    }

    /// Kick off one diff reload; results newer than any in-flight one win.
    pub(crate) fn reload_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_seq += 1;
        let seq = self.diff_seq;
        let path = self.current_project().path.clone();
        let this = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = cx.background_spawn(async move { git::head_diff(&path) }).await;
            if let Err(e) = &result { eprintln!("[ddu] diff err: {e:#}"); }
            let _ = this.update(cx, |v, cx| {
                    if v.diff_seq == seq {
                    v.diff = result.ok();
                    cx.notify();
                }
            });
            anyhow::Ok(())
        })
        .detach();
    }

    /// Periodic working-tree poll so the diff panel stays fresh (M3
    /// acceptance: edits show up within 3s).
    fn start_diff_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(DIFF_POLL_SECS))
                    .await;
                let Some(view) = this.upgrade() else { break };
                let path = view.read_with(cx, |v, _| v.current_project().path.clone());
                let result = cx.background_spawn(async move { git::head_diff(&path) }).await;
                this.update(cx, |v, cx| {
                    let fresh = result.ok();
                    // No change → no notify: keeps idle frames at zero
                    // (poll runs even while the diff panel is hidden).
                    if v.diff != fresh {
                        v.diff = fresh;
                        cx.notify();
                    }
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
    }
    /// One notify per second so elapsed times in the sidebar tick even
    /// while a session produces no output.
    fn start_ui_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        })
        .detach();
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                this.spawn_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleDiff, _, cx| this.toggle_diff(cx)))
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| {
                this.request_close_current_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSessions, _, cx| this.toggle_sessions(cx)))
            .child(ui::title_bar::render(self, cx))
            .child({
                let mut group = h_resizable("main").with_state(&self.resize_state);
                if self.show_sessions {
                    group = group.child(
                        resizable_panel()
                            .size(px(SIDEBAR_DEFAULT))
                            .flex_none()
                            .size_range(px(0.)..px(SIDEBAR_MAX))
                            .child(ui::session_panel::render(self, cx)),
                    );
                }
                group = group.child(
                    resizable_panel()
                        .size_range(px(CENTER_MIN)..px(f32::MAX))
                        .child(ui::terminal::render(self, cx)),
                );
                if self.show_diff {
                    group = group.child(
                        resizable_panel()
                            .size(px(DIFF_DEFAULT))
                            .flex_none()
                            .size_range(px(0.)..px(DIFF_MAX))
                            .child(ui::diff_panel::render(self, cx)),
                    );
                }
                group
            })
            .child(ui::status_bar::render(self, cx))
            // Overlay layers (anchored, no layout impact): dialogs opened via
            // window.open_dialog / open_alert_dialog and notifications are
            // hosted here — gpui-kit requires the app to render these layers.
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
            .children(gpui_kit::component::Root::render_notification_layer(
                window, cx,
            ))
    }
}
