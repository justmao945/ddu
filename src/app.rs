//! App shell view: Zed-style three-pane workspace.
//!
//! State, keyboard actions and session mutations live here; the four
//! surface regions live in [`crate::ui`] as `impl AppView` blocks.
//!
//! Left: session list. Center: agent terminal (no tab bar, no composer —
//! the center pane IS the terminal; M2 embeds a real PTY here).
//! Right: git diff panel.

use gpui_kit::component::notification::Notification;
use gpui_kit::component::*;
use gpui_kit::*;

use crate::session::mock_projects;
use crate::ui;

// Global keyboard actions (M1: new session, dock toggles, close session).
gpui_kit::actions!(ddu, [NewSession, ToggleSessions, ToggleDiff, CloseSession]);

pub struct AppView {
    pub(crate) projects: Vec<crate::session::Project>,
    pub(crate) current_project: usize,
    pub(crate) current_session: usize,
    pub(crate) show_sessions: bool,
    pub(crate) show_diff: bool,
    pub(crate) diff_file: usize,
    pub(crate) session_seq: usize,
}

impl AppView {
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Global shortcuts: cmd-t new session, cmd-\ sessions, cmd-b diff,
        // cmd-w close.
        cx.bind_keys([
            KeyBinding::new("cmd-t", NewSession, None),
            KeyBinding::new("cmd-\\", ToggleSessions, None),
            KeyBinding::new("cmd-b", ToggleDiff, None),
            KeyBinding::new("cmd-w", CloseSession, None),
        ]);

        Self {
            projects: mock_projects(),
            current_project: 0,
            current_session: 0,
            show_sessions: true,
            show_diff: true,
            diff_file: 0,
            session_seq: 100,
        }
    }

    pub(crate) fn current_project(&self) -> &crate::session::Project {
        &self.projects[self.current_project]
    }

    pub(crate) fn current_session(&self) -> Option<&crate::session::AgentSession> {
        self.current_project().sessions.get(self.current_session)
    }

    pub(crate) fn select_session(
        &mut self,
        project: usize,
        session: usize,
        cx: &mut Context<Self>,
    ) {
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
        self.diff_file = 0;
        cx.notify();
    }

    pub(crate) fn add_session(&mut self, cx: &mut Context<Self>) {
        self.session_seq += 1;
        let n = self.session_seq;
        if let Some(project) = self.projects.get_mut(self.current_project) {
            project.sessions.push(crate::session::AgentSession {
                id: format!("mock-{n}"),
                title: format!("session {n}"),
                status: crate::session::AgentStatus::Running,
                cmd: "claude".to_string(),
                transcript: vec!["$ claude".to_string()],
                diff_files: vec![],
            });
            self.current_session = project.sessions.len() - 1;
            self.diff_file = 0;
        }
        cx.notify();
    }

    fn close_current_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.projects.get_mut(self.current_project) else {
            return;
        };
        if project.sessions.is_empty() {
            return;
        }
        let ix = self.current_session.min(project.sessions.len() - 1);
        let title = project.sessions[ix].title.clone();
        project.sessions.remove(ix);
        self.current_session = self.current_session.min(project.sessions.len().saturating_sub(1));
        self.diff_file = 0;
        window.push_notification(Notification::info(format!("Closed “{title}”")), cx);
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
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(|this, _: &NewSession, _, cx| {
                this.add_session(cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleDiff, _, cx| {
                this.show_diff = !this.show_diff;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &CloseSession, window, cx| {
                this.request_close_current_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSessions, _, cx| {
                this.show_sessions = !this.show_sessions;
                cx.notify();
            }))
            .child(ui::title_bar::render(self, cx))
            .child(
                h_resizable("main")
                    .child(
                        resizable_panel()
                            .size(px(232.))
                            .flex_none()
                            .visible(self.show_sessions)
                            .child(ui::session_panel::render(self, cx)),
                    )
                    .child(resizable_panel().child(ui::terminal::render(self, cx)))
                    .child(
                        resizable_panel()
                            .size(px(340.))
                            .flex_none()
                            .visible(self.show_diff)
                            .child(ui::diff_panel::render(self, cx)),
                    ),
            )
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
