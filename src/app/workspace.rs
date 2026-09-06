//! Project rows: adding (folder picker / typed path) and removal.

use super::sessions::index_after_removal;
use super::*;

impl AppView {
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
        let from = (self.current_project, self.current_session);
        if let Some(ix) = self.projects.iter().position(|p| p.path == path) {
            self.current_project = ix;
            self.current_session = 0;
            self.adopt_session_diff(Some(from), cx);
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
        self.adopt_session_diff(None, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Menu entry point for removing a project: silent when fine, an
    /// explanatory dialog when it still has running sessions. Removing
    /// the last project leaves an empty workspace (persisted as
    /// `Some([])`, so the cwd project is NOT re-seeded next launch).
    pub(crate) fn request_remove_project(
        &mut self,
        p: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if p >= self.projects.len() {
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
                    .footer(crate::ui::alert_ok_footer())
            });
            return;
        }
        self.remove_project(p, cx);
    }

    /// Remove a project from the sidebar (config persists the change).
    pub(crate) fn remove_project(&mut self, project: usize, cx: &mut Context<Self>) {
        if project >= self.projects.len() {
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
        // The removed project's diff dies with it; the project now
        // current adopts its own session state (or starts empty).
        self.adopt_session_diff(None, cx);
        self.persist(cx);
        cx.notify();
    }
}
