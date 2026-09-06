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
use crate::ui::diff_tree::{TREE_MAX_H, TREE_MIN_H};

// Global keyboard actions: new session, dock toggles, close session.
gpui_kit::actions!(
    ddu,
    [
        ToggleDiffTree,
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

mod diff;
mod panels;
mod persist;
mod sessions;
mod workspace;

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
    pub(crate) diff_tree_scroll: ScrollHandle,
    pub(crate) diff_hunks_scroll: ScrollHandle,
    /// The sidebar's vertical split: [project tree | diff tree layer].
    pub(crate) sidebar_split_state: Entity<ResizableState>,
    /// Whether the diff file tree layer is shown under the project
    /// tree in the sidebar.
    pub(crate) show_diff_tree: bool,
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
    /// The tree-selected file driving the right pane; `None` shows the
    /// pane's empty state. Clicking a file sets this and opens the pane.
    pub(crate) diff_file: Option<usize>,
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
    /// Graceful shutdown in flight: live agents were interrupted; when
    /// the last one exits, `finish_shutdown` closes the window (or
    /// exits the process for ⌘Q).
    pub(crate) shutting_down: bool,
    pub(crate) quit_after_shutdown: bool,
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
        // Global shortcuts: ⌘N new session, ⌘B sessions, ⌘T file tree,
        // ⌘R changes, ⌘W close.
        cx.bind_keys([
            KeyBinding::new("cmd-,", OpenSettings, None),
            // ⌘N spawns the default launcher in the active project
            // (guarded: with no projects there is nothing to spawn
            // into); ⌘O adds a project via the folder picker. ⌘T
            // toggles the diff file tree under the project tree.
            KeyBinding::new("cmd-n", NewSession, None),
            KeyBinding::new("cmd-o", AddProject, None),
            KeyBinding::new("cmd-t", ToggleDiffTree, None),
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
        let projects: Vec<Project> = if state.projects.is_empty() {
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
            sidebar_split_state: cx.new(|_| ResizableState::default()),
            show_diff_tree: state.show_diff_tree,
            diff_tree_closed: HashSet::new(),
            hovered_project: None,
            hovered_session: None,
            menu_project: None,
            last_sidebar_size: None,
            last_diff_size: None,
            diff_file: None,
            diff_seed_path: None,
            diff_tree_height_seed: None,
            session_seq: 0,
            diff: None,
            diff_error: None,
            diff_seq: 0,
            shutting_down: false,
            quit_after_shutdown: false,
        };
        // Restore persisted widths; the raw values are clamped by the
        // panel size_range on render, out-of-range ones fall back to the
        // defaults inside `last_sidebar_w`/`last_diff_w`.
        this.last_sidebar_size = state.sidebar_width.map(gpui::px);
        this.last_diff_size = state.diff_width.map(gpui::px);
        // The diff itself is loaded asynchronously; the restored
        // current session's selection and collapsed dirs seed it
        // (per-session state — worktrees can differ). `apply_diff`
        // re-pins the path to an index once it lands. The tree-layer
        // height stays project-scoped layout.
        let restored = this
            .projects
            .get(this.current_project)
            .and_then(|p| p.sessions.first());
        this.diff_seed_path = restored.and_then(|s| s.diff_selected.clone());
        this.diff_tree_closed = restored.map(|s| s.diff_closed.clone()).unwrap_or_default();
        let current_path = this.current_project().path.to_string_lossy().to_string();
        this.diff_tree_height_seed = state
            .project_state
            .get(&current_path)
            .and_then(|s| s.tree_height)
            .map(gpui::px)
            .filter(|h| h.as_f32() >= TREE_MIN_H as f32 && h.as_f32() <= TREE_MAX_H as f32);
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
        // Sidebar splitter drags: persist the diff-tree layer height
        // and clear the seed so it can't fight a later live resize.
        cx.subscribe_in(
            &this.sidebar_split_state,
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
        // Window close (red button): with live agents this blocks and
        // starts a graceful shutdown (Ctrl-C → exit → resume ids saved
        // → window removed); otherwise it persists and closes at once.
        {
            let this = cx.weak_entity();
            window.on_window_should_close(cx, move |window, cx| {
                this.update(cx, |view, cx| view.request_close(window, cx))
                    .unwrap_or(true)
            });
        }
        // Restore the persisted session lists (agents as restartable
        // Done rows, shells live in the open project); the saved
        // selection then decides whether the window resumes an agent
        // or lands on a live shell. With nothing saved, fall back to
        // the configured default launcher.
        if state.projects.iter().any(|p| !p.sessions.is_empty()) {
            this.restore_sessions(&state, window, cx);
            if this.projects[this.current_project].sessions.is_empty() {
                this.spawn_session_of(&cfg.new_session.kind, window, cx);
            } else {
                let n = this.projects[this.current_project].sessions.len();
                this.current_session = state.current_session.min(n.saturating_sub(1));
                let selected = this.current_session().cloned();
                let resumes = selected.is_some_and(|s| {
                    s.term.is_none()
                        && s.cmd.resume.is_some()
                        && matches!(s.status, AgentStatus::Done(_))
                });
                if resumes {
                    cx.defer_in(window, |this, window, cx| {
                        this.resume_current_session(window, cx);
                    });
                } else if let Some(term) = this.current_term() {
                    let focus = term.read(cx).focus.clone();
                    focus.focus(window, cx);
                }
            }
        } else {
            this.spawn_session_of(&cfg.new_session.kind, window, cx);
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
        // The root tracks the window fallback focus: every pane focuses
        // this handle on click (so the terminal cursor goes hollow), and
        // with it in the element tree the global shortcuts (⌘N/⌘T/⌘B/…)
        // keep a dispatch path no matter what was clicked last.
        v_flex()
            .id("app-root")
            .track_focus(&self.window_focus)
            .size_full()
            .bg(cx.theme().background)
            .on_action(cx.listener(|_, _: &OpenSettings, _, cx| ui::settings::open(cx)))
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
                this.close_session(p, six, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleSessions, _, cx| this.toggle_sessions(cx)))
            .on_action(cx.listener(|this, _: &ToggleDiff, _, cx| this.toggle_diff(cx)))
            .on_action(cx.listener(|this, _: &ToggleDiffTree, _, cx| this.toggle_diff_tree(cx)))
            // ⌘Q: graceful shutdown — live agents get Ctrl-C, their
            // resume ids land in state.json, then the process exits.
            .on_action(cx.listener(|this, _: &Quit, _, cx| {
                this.request_quit(cx);
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
