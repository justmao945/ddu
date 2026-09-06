//! App shell view: Zed-style three-pane workspace.
//!
//! State, keyboard actions and session mutations live here; the four
//! surface regions live in [`crate::ui`] as `impl AppView` blocks.
//!
//! Left: session list. Center: the live agent terminal (PTY-backed).
//! Right: the project's git diff (HEAD→workdir, polled).

use std::collections::HashSet;
use std::time::Duration;

use gpui_kit::component::*;
use gpui_kit::*;

use crate::diff::{GitDiff, git};
use crate::session::{AgentStatus, Project, initial_projects};
use crate::terminal::{TermEvent, TermSession};
use crate::ui;
use crate::ui::diff_panel::{FILE_TREE_MAX_H, FILE_TREE_MIN_H};

// Global keyboard actions: new session, dock toggles, close session.
gpui_kit::actions!(
    ddu,
    [
        NewSession,
        AddProject,
        ToggleSessions,
        ToggleDiff,
        CloseSession,
        OpenSettings,
        TermTab,
        TermBacktab,
        TermPaste,
        TermCopy,
        CloseSettings,
        Quit,
        SelectSession1,
        SelectSession2,
        SelectSession3,
        SelectSession4,
        SelectSession5,
        SelectSession6,
        SelectSession7,
        SelectSession8,
        SelectSession9
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
    /// Outer splitter: [sidebar | center+diff region]. The sidebar
    /// column carries its own status strip, so this divider runs to
    /// the window's bottom edge (see `set_sessions`).
    pub(crate) shell_state: Entity<ResizableState>,
    /// Inner splitter inside the center+diff region:
    /// [terminal | changes]. It ends above the region's unified status
    /// strip, so this divider never cuts through it (see `set_diff`).
    pub(crate) panes_state: Entity<ResizableState>,
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
    /// Persisted selected-file path, pinned against the first loaded
    /// diff (files move between sessions), then cleared.
    pub(crate) diff_seed_path: Option<String>,
    /// Persisted tree-pane height, applied once the diff splitter has
    /// been laid out (its panels are created at render time).
    pub(crate) diff_tree_height_seed: Option<Pixels>,
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
            // ⌘T spawns the default launcher in the active project
            // (guarded: with no projects there is nothing to spawn
            // into); ⌘N is the same action for muscle memory. ⌘O adds
            // a project via the folder picker.
            KeyBinding::new("cmd-t", NewSession, None),
            KeyBinding::new("cmd-n", NewSession, None),
            KeyBinding::new("cmd-o", AddProject, None),
            KeyBinding::new("cmd-b", ToggleSessions, None),
            KeyBinding::new("cmd-r", ToggleDiff, None),
            KeyBinding::new("cmd-w", CloseSession, None),
            // ⌘1..⌘9: select the Nth session in the current project.
            // Prefixed "cmd" so bare digits keep reaching the PTY.
            KeyBinding::new("cmd-1", SelectSession1, None),
            KeyBinding::new("cmd-2", SelectSession2, None),
            KeyBinding::new("cmd-3", SelectSession3, None),
            KeyBinding::new("cmd-4", SelectSession4, None),
            KeyBinding::new("cmd-5", SelectSession5, None),
            KeyBinding::new("cmd-6", SelectSession6, None),
            KeyBinding::new("cmd-7", SelectSession7, None),
            KeyBinding::new("cmd-8", SelectSession8, None),
            KeyBinding::new("cmd-9", SelectSession9, None),
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
        let state = cx.global::<crate::config::State>().clone();
        let mut projects: Vec<Project> = if state.projects.is_empty() {
            initial_projects()
        } else {
            state
                .projects
                .iter()
                .map(|p| Project {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    sessions: vec![],
                })
                .collect()
        };
        let mut expanded = projects.iter().map(|_| true).collect::<Vec<_>>();
        for (ix, p) in state.projects.iter().enumerate() {
            if ix < expanded.len() {
                expanded[ix] = p.expanded;
            }
        }
        let window_focus = cx.focus_handle().tab_stop(false);
        // Rebuild each project's finished agent row from the persisted
        // resume hint: after an app restart the row is a Done session
        // with no live PTY — clicking its Resume button re-spawns the
        // agent with `--resume <id>`.
        for (ix, project) in projects.iter_mut().enumerate() {
            let Some(saved) = state.projects.get(ix).and_then(|p| p.last_agent.clone()) else {
                continue;
            };
            let saved_kind = saved.kind.clone();
            let cmd = cx
                .global::<crate::config::Config>()
                .cmd_for(&saved_kind)
                .ok();
            if let Some(cmd) = cmd {
                project.sessions.push(crate::session::AgentSession {
                    id: format!("restored-{ix}"),
                    title: saved.title.clone(),
                    status: AgentStatus::Done(0),
                    cmd: cmd.with_resume(saved.resume_id),
                    kind: saved_kind,
                    started: std::time::Instant::now(),
                    ended: Some(std::time::Instant::now()),
                    term: None,
                    // Seed the restored row with the diff visibility
                    // saved when the previous run quit.
                    show_diff: state.show_diff,
                });
            }
        }
        let current_project = state.current_project.min(projects.len().saturating_sub(1));
        let mut this = Self {
            window_focus,
            projects,
            expanded,
            current_project,
            current_session: 0,
            show_sessions: !state.hidden_sessions,
            // Diff visibility is per-session live, but the last saved
            // value seeds the restored window.
            show_diff: state.show_diff,
            shell_state: cx.new(|_| ResizableState::default()),
            panes_state: cx.new(|_| ResizableState::default()),
            diff_tree_scroll: ScrollHandle::new(),
            diff_hunks_scroll: ScrollHandle::new(),
            diff_split_state: cx.new(|_| ResizableState::default()),
            diff_tree_closed: HashSet::new(),
            hovered_project: None,
            hovered_session: None,
            menu_project: None,
            last_sidebar_size: None,
            last_diff_size: None,
            diff_file: 0,
            diff_seed_path: None,
            diff_tree_height_seed: None,
            session_seq: 0,
            diff: None,
            diff_error: None,
            diff_seq: 0,
        };
        // Restore persisted widths; the raw values are clamped by the
        // panel size_range on render, out-of-range ones fall back to the
        // defaults inside `last_sidebar_w`/`last_diff_w`.
        this.last_sidebar_size = state.sidebar_width.map(gpui::px);
        this.last_diff_size = state.diff_width.map(gpui::px);
        // The diff itself is loaded asynchronously; remember the
        // persisted selection until it lands, then `apply_diff` re-pins
        // by path (files move/rename between sessions).
        let current_path = this.current_project().path.to_string_lossy().to_string();
        let project_state = state.project_state.get(&current_path);
        this.diff_seed_path = project_state.and_then(|s| s.selected_file.clone());
        this.diff_tree_closed = project_state
            .map(|s| s.closed_dirs.iter().cloned().collect())
            .unwrap_or_default();
        this.diff_tree_height_seed = project_state
            .and_then(|s| s.tree_height)
            .map(gpui::px)
            .filter(|h| {
                h.as_f32() >= FILE_TREE_MIN_H as f32 && h.as_f32() <= FILE_TREE_MAX_H as f32
            });
        this.window_focus.focus(window, cx);
        this.start_diff_poll(cx);
        this.start_ui_tick(cx);
        // Panel drags: capture the new width into the persisted state.
        // Handlers are deferred a tick — a state update during
        // `set_diff`/`set_sessions` emits this event synchronously, and
        // a direct `view.update` there would re-enter AppView's own
        // update (panic: "already being updated").
        for state in [&this.shell_state, &this.panes_state] {
            let resize = state.clone();
            cx.subscribe_in(
                state,
                window,
                move |_, _, _: &ResizablePanelEvent, _, cx| {
                    let resize = resize.clone();
                    cx.spawn(async move |this, cx| {
                        let _ = this.update(cx, |this, cx| {
                            let sizes = resize.read(cx).sizes();
                            // Outer drag: slot 0 is the sidebar.
                            match resize.entity_id() == this.shell_state.entity_id() {
                                true => {
                                    if this.show_sessions {
                                        this.last_sidebar_size = sizes.first().copied();
                                    }
                                }
                                // Inner drag: the diff is the last slot.
                                false => {
                                    if this.show_diff {
                                        this.last_diff_size = sizes.last().copied();
                                    }
                                }
                            }
                            this.persist(cx);
                        });
                    })
                    .detach();
                },
            )
            .detach();
        }
        // Tree/content splitter drags: persist the tree pane height and
        // clear the seed so it can't fight a later live resize.
        cx.subscribe_in(
            &this.diff_split_state,
            window,
            move |_, _, _: &ResizablePanelEvent, _, cx| {
                cx.spawn(async move |this, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.diff_tree_height_seed = None;
                        this.persist(cx);
                    });
                })
                .detach();
            },
        )
        .detach();
        // Window close (red button / ⌘W on the window): persist before
        // the view goes away. The OS-level terminate path (⌘Q via the
        // app menu) has no hook, hence the `CmdQ` binding above too.
        {
            let this = cx.weak_entity();
            window.on_window_should_close(cx, move |_, cx| {
                let _ = this.update(cx, |view, cx| view.persist(cx));
                true
            });
        }
        // First session for the first project, honoring the configured
        // default launcher — unless the saved selection is a restored
        // agent row (Done + a persisted `--resume <id>`): auto-resume
        // it instead of idling on a Resume button, and skip the extra
        // default shell so reopening never accumulates stray rows.
        let restored = this.projects[this.current_project]
            .sessions
            .get(state.current_session)
            .is_some_and(|s| {
                s.term.is_none()
                    && s.cmd.resume.is_some()
                    && matches!(s.status, AgentStatus::Done(_))
            });
        if restored {
            this.current_session = state.current_session;
            cx.defer_in(window, |this, window, cx| {
                this.resume_current_session(window, cx);
            });
        } else {
            this.spawn_session_of(&cfg.new_session.kind, window, cx);
            // Restore the last-selected session. `spawn_session_of` makes
            // the fresh session current; if the saved index still exists
            // in the restored rows, prefer it so the window reopens
            // where the user left off.
            let n = this.projects[this.current_project].sessions.len();
            this.current_session = state.current_session.min(n.saturating_sub(1));
        }
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
        // Write the outgoing session's diff visibility back onto it:
        // the panel state is per-session and must survive a switch.
        let old_show = self.show_diff;
        if let Some(old) = self
            .projects
            .get_mut(self.current_project)
            .and_then(|p| p.sessions.get_mut(self.current_session))
        {
            old.show_diff = old_show;
        }
        if changed_project {
            // Save the outgoing project's placement state, then adopt
            // the incoming one from the global snapshot.
            self.persist(cx);
            let path = self.projects[project].path.to_string_lossy().to_string();
            let entry = cx
                .global::<crate::config::State>()
                .project_state
                .get(&path)
                .cloned()
                .unwrap_or_default();
            self.diff_seed_path = entry.selected_file.clone();
            self.diff_tree_closed = entry.closed_dirs.iter().cloned().collect();
            self.diff_tree_height_seed = entry.tree_height.map(gpui::px).filter(|h| {
                h.as_f32() >= FILE_TREE_MIN_H as f32 && h.as_f32() <= FILE_TREE_MAX_H as f32
            });
        }
        self.current_project = project;
        let n = self.projects[project].sessions.len();
        self.current_session = session.min(n.saturating_sub(1));
        // Adopt the incoming session's diff visibility. `set_diff`
        // writes the value back to (now-)current session — the same
        // value, so the round-trip is idempotent.
        let adopt = self
            .projects
            .get(project)
            .and_then(|p| p.sessions.get(self.current_session))
            .map(|s| s.show_diff)
            .unwrap_or(true);
        if adopt != self.show_diff {
            self.set_diff(adopt, cx);
        }
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
                // New sessions inherit the panel state of the session
                // they replace (the common "new terminal, same
                // workspace" flow) instead of snapping to a default.
                show_diff: self.show_diff,
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

    /// Write current projects + expanded flags + panel geometry into the
    /// global state snapshot and save to disk. Save errors are reported
    /// (notification or stderr) but never crash the app.
    pub(crate) fn persist(&self, cx: &mut App) {
        let mut snapshot = cx.global::<crate::config::State>().clone();
        snapshot.hidden_sessions = !self.show_sessions;
        snapshot.sidebar_width = self.last_sidebar_size.map(|w| w.as_f32());
        snapshot.diff_width = self.last_diff_size.map(|w| w.as_f32());
        snapshot.current_project = self.current_project;
        snapshot.current_session = self.current_session;
        snapshot.show_diff = self.show_diff;
        let old_projects = &snapshot.projects;
        snapshot.projects = self
            .projects
            .iter()
            .zip(&self.expanded)
            .map(|(p, ex)| crate::config::ProjectConfig {
                name: p.name.clone(),
                path: p.path.clone(),
                expanded: *ex,
                // Keep the resume hint of a project whose path matches;
                // it was written when the agent exited.
                last_agent: old_projects
                    .iter()
                    .find(|op| op.path == p.path)
                    .and_then(|op| op.last_agent.clone()),
            })
            .collect();
        let path = self.current_project().path.to_string_lossy().to_string();
        let entry = snapshot
            .project_state
            .entry(path)
            .or_insert_with(crate::config::ProjectState::default);
        entry.selected_file = self
            .diff
            .as_ref()
            .and_then(|d| d.files.get(self.diff_file))
            .map(|f| f.path.clone())
            .or_else(|| self.diff_seed_path.clone());
        entry.closed_dirs = self.diff_tree_closed.iter().cloned().collect();
        entry.tree_height = self
            .diff_split_state
            .read(cx)
            .sizes()
            .first()
            .copied()
            .map(|h| h.as_f32());
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
                    .footer(crate::ui::alert_ok_footer())
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
                    .footer(crate::ui::alert_ok_footer())
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
        _program: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let status = match code {
            0 => AgentStatus::Done(0),
            code if code > 0 => AgentStatus::Done(code),
            code => AgentStatus::Error(format!("abnormal exit {code}")),
        };
        let ended = std::time::Instant::now();
        let mut matched = false;
        let mut agent_hint: Option<(usize, String, String)> = None; // (project, kind, title)
        for (p_ix, project) in self.projects.iter_mut().enumerate() {
            for session in project.sessions.iter_mut() {
                if session.term.as_ref() == Some(&emitter) {
                    matched = true;
                    session.status = status.clone();
                    session.ended = Some(ended);
                    // Capture the resume hint for the sidebar row and
                    // the state snapshot (agents only — shells need no
                    // resume).
                    if session.is_agent()
                        && let Some(id) = session
                            .term
                            .as_ref()
                            .and_then(|t| t.read(cx).resume_id().map(String::from))
                    {
                        agent_hint = Some((p_ix, session.kind.clone(), id));
                    }
                }
            }
        }
        if !matched {
            return;
        }
        if let Some((p_ix, kind, resume_id)) = agent_hint {
            let title = format!("{} · finished", kind);
            self.persist_last_agent(p_ix, kind, resume_id, title, cx);
        }
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
        {
            let session = project.sessions.remove(six);
            if let Some(term) = &session.term {
                term.update(cx, |s, _| s.kill());
            }
            became_empty = project.sessions.is_empty();
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
                // Shared Cancel + danger-confirm recipe — see
                // `ui::dialog_footer`.
                .footer(crate::ui::dialog_footer(
                    "Close Session",
                    "confirm-close",
                    {
                        let this = this.clone();
                        move |_, window, cx| {
                            if let Some(this) = this.upgrade() {
                                this.update(cx, |v, cx| v.close_session(p, six, window, cx));
                            }
                            window.close_dialog(cx);
                        }
                    },
                ))
        });
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
            eprintln!("[ddu] No session id found in the last run's output.");
            return;
        };
        self.respawn_current_session(Some(id), window, cx);
    }

    /// Shared body of restart/resume: kill the old PTY (if alive) and
    /// spawn a fresh one, optionally resuming the agent session.
    fn respawn_current_session(
        &mut self,
        resume: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.projects.get_mut(self.current_project) else {
            return;
        };
        let cwd = project.path.clone();
        let Some(session) = project.sessions.get_mut(self.current_session) else {
            return;
        };
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

    /// Write the finished agent's resume hint into the state snapshot
    /// (per-project `last_agent`) and save.
    fn persist_last_agent(
        &self,
        project: usize,
        kind: String,
        resume_id: String,
        title: String,
        cx: &mut App,
    ) {
        let mut snapshot = cx.global::<crate::config::State>().clone();
        let Some(entry) = snapshot.projects.get_mut(project) else {
            return;
        };
        entry.last_agent = Some(crate::config::SavedAgent {
            kind,
            resume_id,
            title,
        });
        if let Err(err) = snapshot.save() {
            crate::config::report_error(err, cx);
        }
        cx.set_global(snapshot);
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
            // the center+diff region, not the sidebar.
            self.last_sidebar_size = self.shell_state.read(cx).sizes().first().copied();
        }
        // The panes (terminal/diff) live in their own splitter, so a
        // sidebar insert/remove never rescales them — no re-pin needed.
        self.shell_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(0), cx);
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
        // Inner splitter slots: the terminal is 0, the diff 1.
        const DIFF_IX: usize = 1;
        self.show_diff = on;
        // Remember the panel state on the current session: switching
        // sessions must restore each session's own diff visibility.
        if let Some(s) = self
            .projects
            .get_mut(self.current_project)
            .and_then(|p| p.sessions.get_mut(self.current_session))
        {
            s.show_diff = on;
        }
        self.persist(cx);
        let restore_w = self.last_diff_w();
        if !on {
            // Capture before removal: the slot shifts after `remove_panel`.
            self.last_diff_size = self.panes_state.read(cx).sizes().get(DIFF_IX).copied();
        }
        // The sidebar lives in the outer splitter, so a diff
        // insert/remove never rescales it — no re-pin needed.
        self.panes_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(DIFF_IX), cx);
            } else {
                state.remove_panel(DIFF_IX, cx);
            }
        });
        cx.notify();
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
        // First paint after a cold start: the persisted selection wins
        // when the path still exists in the working tree; afterwards
        // switching projects keeps overwriting it via `diff_file`.
        let seed = if self.diff_seed_path.is_some() && self.diff.is_none() {
            self.diff_seed_path.clone()
        } else {
            None
        };
        let selected = selected.or(seed);
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
        self.diff_seed_path = None;
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
                // ⌘N on an empty workspace is a no-op: spawning needs
                // an active project to create the session in.
                if this.projects.is_empty() {
                    return;
                }
                this.spawn_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &AddProject, window, cx| {
                if window.has_active_dialog(cx) {
                    return;
                }
                // Same folder-picker flow as the sidebar button.
                this.add_project(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession1, window, cx| {
                this.select_session(this.current_project, 0, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession2, window, cx| {
                this.select_session(this.current_project, 1, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession3, window, cx| {
                this.select_session(this.current_project, 2, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession4, window, cx| {
                this.select_session(this.current_project, 3, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession5, window, cx| {
                this.select_session(this.current_project, 4, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession6, window, cx| {
                this.select_session(this.current_project, 5, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession7, window, cx| {
                this.select_session(this.current_project, 6, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession8, window, cx| {
                this.select_session(this.current_project, 7, window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectSession9, window, cx| {
                this.select_session(this.current_project, 8, window, cx);
            }))
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
            .on_action(cx.listener(|this, _: &ToggleDiff, _, cx| this.toggle_diff(cx)))
            // ⌘Q: save the workspace snapshot, then quit. `persist` runs
            // before `quit` so state.json reflects the final layout.
            // `cx.quit()` routes through the platform terminate path,
            // which macOS AppKit intercepts and (in this app's setup)
            // never completes; a direct exit is the reliable route.
            // `on_window_should_close` above already persisted, but the
            // keybinding is the user-visible path — persist one more time
            // so panel geometry from the last interaction lands on disk.
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.persist(cx);
                std::process::exit(0);
            }))
            .child(ui::title_bar::render(self, cx))
            .child({
                // Two nested splitters. The sidebar column owns a status
                // strip, so the LEFT divider runs to the window's bottom
                // edge; the terminal/changes region wraps its splitter
                // ABOVE one unified strip, so the RIGHT divider stops at
                // the strip's top edge and all strips sit on one line.
                let panes_min = CENTER_MIN + if self.show_diff { DIFF_MIN } else { 0. } + 8.;
                let mut shell = h_resizable("shell").with_state(&self.shell_state);
                if self.show_sessions {
                    shell = shell.child(
                        resizable_panel()
                            .size(self.last_sidebar_w())
                            .flex_none()
                            .size_range(px(SIDEBAR_MIN)..px(SIDEBAR_MAX))
                            .child(
                                v_flex()
                                    .size_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_h_0()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .child(ui::session_panel::render(self, cx)),
                                    )
                                    .child(ui::status_bar::render_sidebar(self, cx)),
                            ),
                    );
                }
                let mut panes = h_resizable("panes").with_state(&self.panes_state);
                panes = panes.child(
                    resizable_panel()
                        .size_range(px(CENTER_MIN)..px(f32::MAX))
                        .child(
                            div()
                                .size_full()
                                .min_w_0()
                                .overflow_hidden()
                                .child(ui::terminal::render(self, cx)),
                        ),
                );
                if self.show_diff {
                    panes = panes.child(
                        resizable_panel()
                            .size(self.last_diff_w())
                            .flex_none()
                            .size_range(px(DIFF_MIN)..px(DIFF_MAX))
                            .child(
                                div()
                                    .size_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(ui::diff_panel::render(self, window, cx)),
                            ),
                    );
                }
                let region = v_flex()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .child(div().flex_1().min_h_0().overflow_hidden().child(panes))
                    .child(ui::status_bar::render_center(self, cx));
                div().flex_1().min_h_0().overflow_hidden().child(
                    shell.child(
                        resizable_panel()
                            .size_range(px(panes_min)..px(f32::MAX))
                            .child(region),
                    ),
                )
            })
            // Overlay layers (anchored, no layout impact): dialogs opened via
            // window.open_dialog / open_alert_dialog are hosted here —
            // gpui-kit requires the app to render these layers.
            .children(gpui_kit::component::Root::render_dialog_layer(window, cx))
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
