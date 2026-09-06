//! App shell view: Zed-style three-pane workspace.
//!
//! State, keyboard actions and session mutations live here; the four
//! surface regions live in [`crate::ui`] as `impl AppView` blocks.
//!
//! Left: session list. Center: the live agent terminal (PTY-backed).
//! Right: the project's git diff (HEAD→workdir, polled).

use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::diff::{GitDiff, git};
use crate::session::{AgentStatus, Project, initial_projects};
use crate::terminal::{TermEvent, TermSession};
use crate::ui;

// Global keyboard actions: new session, dock toggles, close session.
gpui_kit::actions!(
    ddu,
    [
        NewSession,
        ToggleSessions,
        ToggleDiff,
        CloseSession,
        OpenSettings,
        TermTab,
        TermBacktab,
        TermPaste,
        TermCopy,
        CloseSettings
    ]
);

/// Seconds between working-tree diff polls.
const DIFF_POLL_SECS: u64 = 3;

pub struct AppView {
    /// Holds focus when no session exists so global shortcuts (⌘B/⌘R/⌘T)
    /// keep working on the empty state.
    pub(crate) window_focus: gpui::FocusHandle,
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
    pub(crate) diff_tree_scroll: ScrollHandle,
    pub(crate) diff_hunks_scroll: ScrollHandle,
    /// Vertical splitter between the diff tree and the file content.
    pub(crate) diff_split_state: Entity<ResizableState>,
    /// Directory paths collapsed in the diff tree (all default open).
    pub(crate) diff_tree_closed: std::collections::HashSet<String>,
    pub(crate) hovered_project: Option<usize>,
    /// Project whose `...` menu is open: keeps the row's buttons mounted
    /// while the mouse travels into the popup (the popup occludes the
    /// row, so hover alone would unmount the trigger and kill the menu).
    pub(crate) menu_project: Option<usize>,
    /// Last dragged width of the sidebar / diff pane, restored on
    /// toggle-open instead of snapping back to the default.
    pub(crate) last_sidebar_size: Option<Pixels>,
    pub(crate) last_diff_size: Option<Pixels>,
    /// Session row currently under the mouse: reveals its delete button.
    pub(crate) hovered_session: Option<(usize, usize)>,
    pub(crate) diff_file: usize,
    pub(crate) session_seq: usize,
    pub(crate) diff: Option<GitDiff>,
    pub(crate) diff_error: Option<String>,
    /// Guards against stale poll results overwriting newer ones.
    diff_seq: u64,
}

/// Panel geometry (px): defaults, drag limits and collapse thresholds.
///
/// `size_range` min values are load-bearing twice: they clamp drags and
/// emit CSS `min_w`, so a panel can never render below its min even when
/// the window itself is squeezed (flex then shrinks the center pane).
const SIDEBAR_DEFAULT: f32 = 200.;
const SIDEBAR_MAX: f32 = 420.;
const SIDEBAR_MIN: f32 = 150.;
const DIFF_DEFAULT: f32 = 340.;
const DIFF_MAX: f32 = 600.;
const DIFF_MIN: f32 = 200.;
/// The terminal pane never shrinks below this while dragging a divider.
const CENTER_MIN: f32 = 400.;
/// Window minimum: all three panes at min plus two resize handles.
pub(crate) const WINDOW_MIN_WIDTH: f32 = SIDEBAR_MIN + CENTER_MIN + DIFF_MIN + 8.;
pub(crate) const WINDOW_MIN_HEIGHT: f32 = 400.;

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Global shortcuts: cmd-t new session, cmd-b sessions, cmd-r diff,
        // cmd-w close.
        cx.bind_keys([
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-t", NewSession, None),
            KeyBinding::new("cmd-b", ToggleSessions, None),
            KeyBinding::new("cmd-r", ToggleDiff, None),
            KeyBinding::new("cmd-w", CloseSession, None),
            // Terminal-scoped: these beat gpui-component Root's global
            // Tab/Shift-Tab focus cycling (deeper key context wins), so
            // the PTY gets real tab/backtab bytes and focus never jumps
            // to sidebar buttons mid-session. ⌘V is unbound globally.
            KeyBinding::new("tab", TermTab, Some("Terminal")),
            KeyBinding::new("shift-tab", TermBacktab, Some("Terminal")),
            KeyBinding::new("cmd-v", TermPaste, Some("Terminal")),
            // ⌘C copies the mouse selection when one exists (the
            // handler propagates otherwise); PTYs never see it.
            KeyBinding::new("cmd-c", TermCopy, Some("Terminal")),
            // The standalone settings window: Escape/⌘W close it (the
            // deeper context beats the global ⌘W → CloseSession).
            KeyBinding::new("escape", CloseSettings, Some("SettingsWindow")),
            KeyBinding::new("cmd-w", CloseSettings, Some("SettingsWindow")),
        ]);

        let cfg = cx.global::<crate::config::Config>().clone();
        let projects: Vec<Project> = if cfg.projects.is_empty() {
            initial_projects()
        } else {
            cfg.projects
                .iter()
                .map(|p| Project {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    sessions: vec![],
                })
                .collect()
        };
        let mut expanded = projects.iter().map(|_| true).collect::<Vec<_>>();
        for (ix, p) in cfg.projects.iter().enumerate() {
            if ix < expanded.len() {
                expanded[ix] = p.expanded;
            }
        }
        let window_focus = cx.focus_handle().tab_stop(false);
        let mut this = Self {
            window_focus,
            projects,
            expanded,
            current_project: 0,
            current_session: 0,
            show_sessions: !cfg.hidden_sessions,
            show_diff: cfg.show_diff,
            resize_state: cx.new(|_| ResizableState::default()),
            diff_tree_scroll: ScrollHandle::new(),
            diff_hunks_scroll: ScrollHandle::new(),
            diff_split_state: cx.new(|_| ResizableState::default()),
            diff_tree_closed: std::collections::HashSet::new(),
            hovered_project: None,
            hovered_session: None,
            menu_project: None,
            last_sidebar_size: None,
            last_diff_size: None,
            diff_file: 0,
            session_seq: 0,
            diff: None,
            diff_error: None,
            diff_seq: 0,
        };
        this.window_focus.focus(window, cx);
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
        let changed_project = self.current_project != project;
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
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
                window.push_notification(Notification::error(err.to_string()), cx);
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
                window.push_notification(
                    Notification::error(format!("Failed to start {}: {err}", cmd.label())),
                    cx,
                );
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
        window
            .spawn(cx, async move |cx| {
                let picked = rfd::AsyncFileDialog::new()
                    .set_title("Choose a project folder")
                    .pick_folder()
                    .await
                    .map(|handle| handle.path().to_string_lossy().into_owned());
                let _ = view.update_in(cx, |this, window, cx| {
                    if let Some(path) = picked {
                        this.add_project_path(&path, cx);
                        this.select_session(this.current_project, this.current_session, window, cx);
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
        if let Some(ix) = self.projects.iter().position(|p| p.path == path) {
            self.current_project = ix;
            self.current_session = 0;
            self.reset_diff();
            self.reload_diff(cx);
            cx.notify();
            return;
        }
        self.projects.push(Project {
            name,
            path: path.clone(),
            sessions: vec![],
        });
        self.expanded.push(true);
        self.current_project = self.projects.len() - 1;
        self.current_session = 0;
        self.reset_diff();
        self.persist(cx);
        self.reload_diff(cx);
        cx.notify();
    }

    /// Write current projects + expanded flags into the global config
    /// and save to disk.
    pub(crate) fn persist(&self, cx: &mut App) {
        let mut snapshot = cx.global::<crate::config::Config>().clone();
        snapshot.hidden_sessions = !self.show_sessions;
        snapshot.show_diff = self.show_diff;
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

    /// Menu entry point for removing a project: silent when fine, an
    /// explanatory dialog when not (running sessions, last project left).
    pub(crate) fn request_remove_project(
        &mut self,
        p: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if p >= self.projects.len() {
            return;
        }
        if self.projects.len() == 1 {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Cannot Remove Project")
                    .description("At least one project must stay open.")
            });
            return;
        }
        if self.projects[p]
            .sessions
            .iter()
            .any(|s| s.status.is_running())
        {
            window.open_alert_dialog(cx, |alert, _, _| {
                alert
                    .title("Cannot Remove Project")
                    .description("Close its running sessions first.")
            });
            return;
        }
        self.remove_project(p, cx);
    }

    /// Remove a project from the sidebar (config persists the change).
    pub(crate) fn remove_project(&mut self, project: usize, cx: &mut Context<Self>) {
        if project >= self.projects.len() || self.projects.len() == 1 {
            return;
        }
        let removed = self.projects.remove(project);
        for session in removed.sessions {
            if let Some(term) = session.term {
                term.update(cx, |s, _| s.kill());
            }
        }
        self.expanded.remove(project);
        let was_current = project == self.current_project;
        self.current_project =
            index_after_removal(self.current_project, project, self.projects.len());
        if was_current {
            self.current_session = 0;
        }
        self.hovered_project = None;
        self.hovered_session = None;
        self.menu_project = None;
        self.reset_diff();
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
        let mut matched = false;
        for session in self
            .projects
            .iter_mut()
            .flat_map(|p| p.sessions.iter_mut())
            .filter(|s| s.term.as_ref() == Some(&emitter))
        {
            matched = true;
            session.status = status.clone();
            session.ended = Some(ended);
        }
        if !matched {
            return;
        }
        // Only agents merit a toast: the exit banner already covers the
        // terminal, and a shell exits every time the user types `exit`.
        let is_agent = self
            .projects
            .iter()
            .flat_map(|p| p.sessions.iter())
            .any(|s| s.term.as_ref() == Some(&emitter) && s.is_agent());
        if !is_agent {
            cx.notify();
            return;
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
        let title;
        {
            let session = project.sessions.remove(six);
            if let Some(term) = &session.term {
                term.update(cx, |s, _| s.kill());
            }
            became_empty = project.sessions.is_empty();
            title = session.title.clone();
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
        window.push_notification(Notification::info(format!("Closed “{title}”")), cx);
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
                // Custom footer: the app's compact button recipe instead
                // of the stock large OK/Cancel pair.
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("cancel-close")
                                .label("Cancel")
                                .outline()
                                .small()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-close")
                                .label("Close Session")
                                .danger()
                                .small()
                                .on_click({
                                    let this = this.clone();
                                    move |_, window, cx| {
                                        if let Some(this) = this.upgrade() {
                                            this.update(cx, |v, cx| {
                                                v.close_session(p, six, window, cx)
                                            });
                                        }
                                        window.close_dialog(cx);
                                    }
                                }),
                        ),
                )
        });
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
                window.push_notification(
                    Notification::error(format!("Failed to restart: {err}")),
                    cx,
                );
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
        if self.current_term().is_some() {
            window.push_notification(Notification::info(format!("Restarted “{title}”")), cx);
        }
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
        self.persist(cx);
        let restore_w = self.last_sidebar_w();
        if !on {
            // Capture before removal: after `remove_panel(0)` slot 0 is
            // the center pane, not the sidebar.
            self.last_sidebar_size = self.resize_state.read(cx).sizes().first().copied();
        }
        // `insert_panel`/`remove_panel` redistribute every slot's width
        // proportionally (gpui-base `ResizableState`), which visually
        // resizes the untouched diff pane. Re-pin the diff to its old
        // width afterwards: `resize_panel` on the last panel takes the
        // freed/given space only from its left neighbor — the center.
        let old_diff_ix = usize::from(!on) + 1;
        let diff_w = if self.show_diff {
            self.resize_state.read(cx).sizes().get(old_diff_ix).copied()
        } else {
            None
        };
        self.resize_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(0), cx);
            } else {
                state.remove_panel(0, cx);
            }
        });
        if let Some(w) = diff_w {
            // Diff sits at index 2 (sidebar open) resp. 1 (closed) after
            // the mutation; both are the last slot.
            self.pin_panel(usize::from(on) + 1, w, cx);
        }
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
        self.persist(cx);
        let restore_w = self.last_diff_w();
        if !on {
            // Capture before removal: the slot shifts after `remove_panel`.
            self.last_diff_size = self.resize_state.read(cx).sizes().get(ix).copied();
        }
        // Pin the untouched sidebar across the toggle, like `set_sessions`:
        // `insert_panel`/`remove_panel` would otherwise rescale it.
        let sidebar_w = if self.show_sessions {
            self.resize_state.read(cx).sizes().first().copied()
        } else {
            None
        };
        self.resize_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(ix), cx);
            } else {
                state.remove_panel(ix, cx);
            }
        });
        if let Some(w) = sidebar_w {
            self.pin_panel(0, w, cx);
        }
        cx.notify();
    }

    /// Force panel `ix` to `w`, letting the center pane absorb the change
    /// (via `resize_panel`'s drag-space math) instead of staying skewed by
    /// `insert_panel`/`remove_panel`'s proportional redistribution.
    fn pin_panel(&self, ix: usize, w: Pixels, cx: &mut Context<Self>) {
        let this = cx.weak_entity();
        let state = self.resize_state.downgrade();
        cx.spawn(async move |_, cx| {
            let _ = state.update_in(cx, |state, window, cx| {
                state.resize_panel(ix, w, window, cx);
            });
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    fn last_sidebar_w(&self) -> Pixels {
        self.last_sidebar_size
            .filter(|w| *w >= px(SIDEBAR_MIN))
            .unwrap_or(px(SIDEBAR_DEFAULT))
    }

    fn last_diff_w(&self) -> Pixels {
        self.last_diff_size
            .filter(|w| *w >= px(DIFF_MIN))
            .unwrap_or(px(DIFF_DEFAULT))
    }

    fn reset_diff(&mut self) {
        self.diff = None;
        self.diff_error = None;
        self.diff_file = 0;
        self.diff_tree_closed.clear();
        self.diff_tree_scroll.set_offset(point(px(0.), px(0.)));
        self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
    }

    fn apply_diff(&mut self, result: anyhow::Result<GitDiff>) {
        let selected = self
            .diff
            .as_ref()
            .and_then(|d| d.files.get(self.diff_file))
            .map(|f| f.path.clone());
        match result {
            Ok(diff) => {
                let next = selected
                    .as_ref()
                    .and_then(|path| diff.files.iter().position(|f| &f.path == path))
                    .unwrap_or(0);
                if selected.as_ref() != diff.files.get(next).map(|f| &f.path) {
                    self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
                }
                self.diff_file = next;
                self.diff = Some(diff);
                self.diff_error = None;
            }
            Err(err) => {
                self.diff = None;
                self.diff_error = Some(err.to_string());
            }
        }
    }

    /// Kick off one diff reload; results newer than any in-flight one win.
    pub(crate) fn reload_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_seq += 1;
        let seq = self.diff_seq;
        let path = self.current_project().path.clone();
        let this = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_spawn(async move { git::head_diff(&path) })
                .await;
            if let Err(e) = &result {
                eprintln!("[ddu] diff err: {e:#}");
            }
            let _ = this.update(cx, |v, cx| {
                if v.diff_seq == seq {
                    v.apply_diff(result);
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
                let (path, seq) =
                    view.read_with(cx, |v, _| (v.current_project().path.clone(), v.diff_seq));
                let result = cx
                    .background_spawn(async move { git::head_diff(&path) })
                    .await;
                this.update(cx, |v, cx| {
                    if v.diff_seq == seq {
                        v.apply_diff(result);
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
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
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
            .on_action(cx.listener(|_, _: &OpenSettings, _, cx| ui::settings_window::open(cx)))
            .on_action(cx.listener(|this, _: &NewSession, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                this.spawn_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleDiff, _, cx| this.toggle_diff(cx)))
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| {
                if window.has_active_dialog(cx) {
                    window.close_dialog(cx);
                    return;
                }
                let p = this.current_project;
                let six = this.current_session;
                this.request_close_session(p, six, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSessions, _, cx| this.toggle_sessions(cx)))
            .child(ui::title_bar::render(self, cx))
            .child({
                // Each column owns its status strip, so the resize
                // dividers run all the way to the window's bottom edge.
                let column = |content: AnyElement, strip: AnyElement| {
                    v_flex()
                        .size_full()
                        .min_w_0()
                        .overflow_hidden()
                        .child(div().flex_1().min_h_0().min_w_0().child(content))
                        .child(strip)
                };
                let mut group = h_resizable("main").with_state(&self.resize_state);
                if self.show_sessions {
                    group = group.child(
                        resizable_panel()
                            .size(px(SIDEBAR_DEFAULT))
                            .flex_none()
                            .size_range(px(SIDEBAR_MIN)..px(SIDEBAR_MAX))
                            .child(column(
                                ui::session_panel::render(self, cx).into_any_element(),
                                ui::status_bar::render_sidebar(self, cx).into_any_element(),
                            )),
                    );
                }
                group = group.child(
                    resizable_panel()
                        .size_range(px(CENTER_MIN)..px(f32::MAX))
                        .child(column(
                            ui::terminal::render(self, cx).into_any_element(),
                            ui::status_bar::render_center(self, cx).into_any_element(),
                        )),
                );
                if self.show_diff {
                    group = group.child(
                        resizable_panel()
                            .size(px(DIFF_DEFAULT))
                            .flex_none()
                            .size_range(px(DIFF_MIN)..px(DIFF_MAX))
                            .child(column(
                                ui::diff_panel::render(self, window, cx).into_any_element(),
                                ui::status_bar::render_diff(self, cx).into_any_element(),
                            )),
                    );
                }
                div().flex_1().min_h_0().overflow_hidden().child(group)
            })
            // Overlay layers (anchored, no layout impact): dialogs opened via
            // window.open_dialog / open_alert_dialog and notifications are
            // hosted here — gpui-kit requires the app to render these layers.
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
            .children(gpui_kit::component::Root::render_notification_layer(
                window, cx,
            ))
    }
}

/// Preserve the selected identity when an earlier sibling is removed.
fn index_after_removal(selected: usize, removed: usize, remaining: usize) -> usize {
    selected
        .saturating_sub(usize::from(removed < selected))
        .min(remaining.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::index_after_removal;
    #[test]
    fn removal_keeps_selection_or_nearest_neighbor() {
        assert_eq!(index_after_removal(2, 0, 3), 1);
        assert_eq!(index_after_removal(1, 2, 3), 1);
        assert_eq!(index_after_removal(3, 3, 3), 2);
        assert_eq!(index_after_removal(0, 0, 0), 0);
    }
}
