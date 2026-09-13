//! App shell view: Zed-style three-pane workspace.
//!
//! State and the constructor live here; everything else is a submodule by
//! concern — `keys` (bindings), `render` (the frame), `sessions`/`shutdown`
//! (the PTY rows), `diff`/`pane`/`search` (the right pane and both bars),
//! `persist`, `panels`, `workspace` — and the four surface regions live in
//! [`crate::ui`] as `impl AppView` blocks.
//!
//! Left: session list. Center: the live agent terminal (PTY-backed).
//! Right: the project's git diff (HEAD→workdir, polled).

use std::collections::HashSet;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::base::input;
use gpui_kit::base::{TextSelection, TextSelectionLayer};
use gpui_kit::component::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::diff::Snapshot;
use crate::session::{AgentStatus, Project, initial_projects};
use crate::terminal::{TermEvent, TermSession};
use crate::ui;
use crate::ui::diff_panel::ViewMode;
use crate::ui::file_tree::{tree_max_h, tree_min_h};
pub(crate) use keys::accel_hint;
use keys::key_bindings;

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
        FontLarger,
        FontSmaller,
        Quit,
        DiffSearch,
        DiffSearchNext,
        DiffSearchPrev,
        FileSearch,
        FileSearchNext,
        FileSearchPrev,
        ToggleViewMode,
        TermSearch,
        TermSearchNext,
        TermSearchPrev,
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

/// The pane's selection (see [`AppView::selection`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    pub path: String,
    /// Index into `snapshot.diff.files` — `None` for an unchanged file.
    pub changed: Option<usize>,
}

/// Seconds between working-tree diff polls.
const DIFF_POLL_SECS: u64 = 3;

mod diff;
mod keys;
mod pane;
mod panels;
mod persist;
mod render;
mod search;
mod sessions;
mod shutdown;
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
    /// The visible session's OSC title as last painted, with the
    /// session it belongs to: a stream frame that changes the title
    /// (agent CLIs spin it) has to reach the sidebar row and the
    /// breadcrumb, which are otherwise cached/replayed off their own
    /// notifications only.
    pub(crate) shown_title: Option<(gpui::EntityId, Option<String>)>,
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
    pub(crate) diff_tree_scroll: VirtualListScrollHandle,
    pub(crate) diff_hunks_scroll: VirtualListScrollHandle,
    /// Project/session tree scroll (sidebar upper layer) — drives its
    /// auto-hide scrollbar.
    pub(crate) sessions_scroll: ScrollHandle,
    /// The sidebar's vertical split: [project tree | diff tree layer].
    pub(crate) sidebar_split_state: Entity<ResizableState>,
    /// Whether the diff file tree layer is shown under the project
    /// tree in the sidebar.
    pub(crate) show_diff_tree: bool,
    /// Directories expanded in the file tree: the layer is lazy, so this
    /// is the *only* reason a directory is ever listed (the root aside).
    pub(crate) diff_tree_open: std::collections::HashSet<String>,
    /// The tree's rows + totals, rebuilt when the diff, the expansion or
    /// the listing changes (never per frame — see `build_index`).
    pub(crate) tree_index: Option<crate::ui::file_tree::TreeIndex>,
    /// The project's repository, opened once and kept for the tree's
    /// listings (`list_dir` needs git's own ignore rules). Keyed by the
    /// directory it was discovered from: a session switch to another
    /// project re-discovers.
    pub(crate) repo: Option<(std::path::PathBuf, Rc<git2::Repository>)>,
    pub(crate) hovered_project: Option<usize>,
    /// Project whose `...` menu is open: keeps the row's buttons mounted
    /// while the mouse travels into the popup (the popup occludes the
    /// row, so hover alone would unmount the trigger and kill the menu).
    pub(crate) menu_project: Option<usize>,
    /// Last dragged width of the sidebar / diff pane, restored on
    /// toggle-open instead of snapping back to the default.
    pub(crate) last_sidebar_size: Option<Pixels>,
    pub(crate) last_diff_size: Option<Pixels>,
    /// The container size the shell / panes splitter was last healed
    /// for: the drift this corrects comes from a *container* resize that
    /// landed while every slot was still pinned, so one heal per
    /// container size is exactly right — and it keeps the correction out
    /// of a drag's way, which never changes the container.
    healed_shell_at: Pixels,
    healed_panes_at: Pixels,
    /// Resized events the render-time width re-assertion emits on
    /// purpose: the subscription must not record those as user drags
    /// (nothing changed semantically, and persisting a default would
    /// pin it against later text-scale changes). Counted, since one
    /// render pass can correct both splitters.
    pub(crate) suppress_resize_records: u8,
    /// The pane's selection: a path in the working tree — clean files are
    /// selectable, the tree lists them — plus the index of its diff
    /// record when the poll found one, so the pane never searches a
    /// multi-thousand-file diff per frame.
    pub(crate) selection: Option<Selection>,
    /// Session row currently under the mouse: reveals its delete button.
    pub(crate) hovered_session: Option<(usize, usize)>,
    /// Left-button drag in progress (window-level). The
    /// `TextSelectionLayer` picks up mouse events through
    /// `Window::on_mouse_event`, whose listeners exist only for one
    /// frame after a render — they die unless the window keeps
    /// repainting. Keeping the gesture alive with a refresh on every
    /// move (terminals do the same via element callbacks, which are
    /// persistent) is what makes diff drag-selection track the cursor.
    pub(crate) selection_active: bool,
    /// Persisted selected-file path, pinned against the first loaded
    /// diff (files move between sessions), then cleared.
    pub(crate) diff_seed_path: Option<String>,
    /// The current session's persisted tree-pane height, applied once
    /// the diff splitter has been laid out (its panels are created at
    /// render time); refreshed on every session switch.
    pub(crate) diff_tree_height_seed: Option<Pixels>,
    pub(crate) session_seq: usize,
    /// The last applied poll: the diff **and** the full working-tree
    /// listing, always written together (one poll, one snapshot — the
    /// tree and the pane can never disagree about what changed).
    pub(crate) snapshot: Option<Snapshot>,
    /// Whether the clean directories have been folded away for the
    /// current session yet — the default-collapse rule runs once per
    /// session, on the first snapshot that carries a tree, and never
    /// overrides a directory the user has touched since.
    pub(crate) tree_seeded: bool,
    pub(crate) diff_error: Option<String>,
    /// The right pane's find bar (⌘F): input entity, match list and
    /// cycle position. Lives in [`diff`]'s module; always constructed,
    /// the `open` flag folds its visibility.
    pub(crate) diff_search: search::DiffSearch,
    /// The file tree's quick open (⌘P): the sidebar layer's search bar,
    /// the working tree's path list and the ranked hits.
    pub(crate) file_search: search::FileSearch,
    /// Where each file was last scrolled to, keyed by working tree,
    /// path and view mode. Switching files and coming back lands where
    /// the file was left; in memory only — a relaunch starts at the top.
    pub(crate) file_positions:
        std::collections::HashMap<(std::path::PathBuf, String, ViewMode), Point<Pixels>>,
    /// A remembered position whose rows are not on screen yet (see
    /// `AppView::restore_scroll`): `(path, mode, offset)`.
    pub(crate) pending_scroll: Option<(String, ViewMode, Point<Pixels>)>,
    /// Guards against stale poll results overwriting newer ones.
    diff_seq: u64,
    /// Which surface the right pane shows (⌘⇧M toggles it); per session,
    /// persisted like the selection.
    pub(crate) view_mode: ViewMode,
    /// Cached whole-file view for File mode (read off the UI thread),
    /// and the `(path, generation)` it was built for: the pane only
    /// renders it while both still match.
    pub(crate) file_view: Option<std::rc::Rc<crate::diff::file_view::FileView>>,
    pub(crate) file_view_key: Option<(String, u64)>,
    /// The rendered document's Markdown source — or the reason it could
    /// not be read.    /// read — for the `(path, generation)` it was read for. One field for
    /// both outcomes: a refusal has to be as keyed as a success, or the
    /// pane would band "reading the file" forever on a file it can never
    /// read.
    pub(crate) preview: Option<crate::diff::file_view::PreviewBuild>,
    /// Bumped on every applied diff: what the two caches above (and
    /// their in-flight builds) compare against.
    pub(crate) diff_gen: u64,
    /// Per-path line budgets for truncated files: reaching the cap note
    /// grows the entry ×4 (see `expand_diff_limit`); empty = every file
    /// at `diff::MAX_LINES_PER_FILE`. Runtime-only, cleared with the diff.
    pub(crate) diff_limits: std::collections::HashMap<String, usize>,
    /// Graceful shutdown in flight: live agents were interrupted; when
    /// the last one exits, `finish_shutdown` closes the window (or
    /// exits the process for ⌘Q).
    pub(crate) shutting_down: bool,
    pub(crate) quit_after_shutdown: bool,
    /// Latest window frame (move/resize/zoom/fullscreen), tracked by an
    /// observer so `persist` can record it; seeded from the snapshot so
    /// a quit before any frame change keeps the restored placement.
    pub(crate) window_placement: Option<crate::config::WindowPlacement>,
    /// Debounce guard for the save scheduled on each frame change.
    window_geom_seq: u64,
    /// Shell panels as cached child views (see [`panel_view!`]): a
    /// stream frame notifies `terminal_pane` alone, and every other
    /// notify fans out to all of them via [`AppView::notify_panels`].
    pub(crate) sidebar: Entity<ui::session_panel::PanelView>,
    pub(crate) diff_pane: Entity<ui::diff_panel::PanelView>,
    pub(crate) terminal_pane: Entity<ui::terminal_panel::PanelView>,
    /// The title bar's "project — session title" text: its own cached
    /// view, because it mirrors the visible session's OSC title (which
    /// agent CLIs spin) and must not drag the panels into that repaint —
    /// `AppView::notify_panels` would, since it hangs off `cx.notify()`.
    pub(crate) breadcrumb: Entity<ui::title_bar::PanelView>,
}

/// Panel geometry (px): defaults, drag limits and collapse thresholds.
/// All values are base sizes at text-scale factor 1.0 — geometry scales
/// with the desktop text scale (`ui::scaled`), same as row heights.
///
/// `size_range` min values are load-bearing twice: they clamp drags and
/// emit CSS `min_w`, so a panel can never render below its min even when
/// the window itself is squeezed (flex then shrinks the center pane).
fn sidebar_default() -> f32 {
    ui::scaled(200.)
}
/// Compact session list: rows are narrow, so dragging far past ~300px
/// (base) only starves the terminal for no gain.
fn sidebar_max() -> f32 {
    ui::scaled(300.)
}
fn sidebar_min() -> f32 {
    ui::scaled(150.)
}
fn diff_default() -> f32 {
    ui::scaled(340.)
}
/// Floor for the diff pane's adaptive max: even on a small window the
/// pane may reach this wide.
fn diff_max() -> f32 {
    ui::scaled(600.)
}
fn diff_min() -> f32 {
    ui::scaled(200.)
}
/// The terminal pane never shrinks below this while dragging a divider.
/// Keep it honest: at a large text scale this scales up with the fonts, and
/// whatever it takes, the diff pane cannot be dragged past
/// `container - center_min`, so a generous value here is what makes the
/// right divider feel stuck near the default width.
fn center_min() -> f32 {
    ui::scaled(280.)
}
/// Window minimum: all three panes at min plus two resize handles.
pub(crate) fn window_min_width() -> f32 {
    sidebar_min() + center_min() + diff_min() + ui::scaled(8.)
}
pub(crate) fn window_min_height() -> f32 {
    ui::scaled(400.)
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Global shortcuts: secondary+N new session, +B sessions, +T
        // file tree, +R changes, +W close (⌘ on macOS, ⌃ elsewhere).
        // (OpenSettings lives at app level in main.rs so the Settings
        // menu item can resolve its hint.)
        cx.bind_keys(key_bindings());

        let cfg = cx.global::<crate::config::Config>().clone();
        let state = cx.global::<crate::config::State>().clone();
        // `None` (no state file yet) seeds the cwd project; `Some` —
        // even an empty vec — is the workspace the user last had.
        let projects: Vec<Project> = match &state.projects {
            None => initial_projects(),
            Some(saved) => saved
                .iter()
                .map(|p| Project {
                    name: p.name.clone(),
                    path: p.path.clone(),
                    sessions: vec![],
                })
                .collect(),
        };
        let mut expanded = projects.iter().map(|_| true).collect::<Vec<_>>();
        for (ix, p) in state.projects.as_deref().unwrap_or(&[]).iter().enumerate() {
            if ix < expanded.len() {
                expanded[ix] = p.expanded;
            }
        }
        let window_focus = cx.focus_handle().tab_stop(false);
        let current_project = state.current_project.min(projects.len().saturating_sub(1));
        // Cached panel views (see `panel_view!`). They render for this
        // very entity: `weak_entity` exists before `Self` does, and a
        // panel only reads it at render time.
        let app = cx.weak_entity();
        let sidebar = cx.new(|_| ui::session_panel::PanelView::new(app.clone()));
        let diff_pane = cx.new(|_| ui::diff_panel::PanelView::new(app.clone()));
        let terminal_pane = cx.new(|_| ui::terminal_panel::PanelView::new(app.clone()));
        let breadcrumb = cx.new(|_| ui::title_bar::PanelView::new(app));
        let mut this = Self {
            window_focus,
            projects,
            expanded,
            current_project,
            current_session: 0,
            shown_title: None,
            show_sessions: !state.hidden_sessions,
            // Diff visibility is per-session live, but the last saved
            // value seeds the restored window.
            show_diff: state.show_diff,
            shell_state: cx.new(|_| ResizableState::default()),
            panes_state: cx.new(|_| ResizableState::default()),
            diff_tree_scroll: VirtualListScrollHandle::new(),
            diff_hunks_scroll: VirtualListScrollHandle::new(),
            sessions_scroll: ScrollHandle::new(),
            sidebar_split_state: cx.new(|_| ResizableState::default()),
            show_diff_tree: state.show_diff_tree,
            diff_tree_open: HashSet::new(),
            tree_index: None,
            repo: None,
            hovered_project: None,
            hovered_session: None,
            menu_project: None,
            last_sidebar_size: None,
            last_diff_size: None,
            selection: None,
            selection_active: false,
            diff_seed_path: None,
            diff_tree_height_seed: None,
            session_seq: 0,
            healed_shell_at: px(0.),
            healed_panes_at: px(0.),
            suppress_resize_records: 0,
            snapshot: None,
            tree_seeded: false,
            diff_error: None,
            diff_search: search::DiffSearch::new(window, cx),
            file_search: search::FileSearch::new(window, cx),
            file_positions: std::collections::HashMap::new(),
            pending_scroll: None,
            diff_seq: 0,
            view_mode: ViewMode::default(),
            file_view: None,
            file_view_key: None,
            preview: None,
            diff_gen: 0,
            diff_limits: std::collections::HashMap::new(),
            shutting_down: false,
            quit_after_shutdown: false,
            window_placement: state.window,
            window_geom_seq: 0,
            sidebar,
            diff_pane,
            terminal_pane,
            breadcrumb,
        };
        // Restore persisted widths; the raw values are clamped by the
        // panel size_range on render, out-of-range ones fall back to the
        // defaults inside `last_sidebar_w`/`last_diff_w`.
        this.last_sidebar_size = state.sidebar_width.map(gpui::px);
        this.last_diff_size = state.diff_width.map(gpui::px);
        // Per-session diff state (selection, collapsed dirs, tree-layer
        // height) seeds from the restored current session below — the
        // rows don't exist until `restore_sessions` runs.
        this.window_focus.focus(window, cx);
        // Every app-level notify fans out to the cached panels (see
        // `panel_view!` and `notify_panels`). Streaming output is the one
        // path that doesn't come through here: `subscribe_term` notifies
        // the terminal pane directly.
        cx.observe_self(|this, cx| this.notify_panels(cx)).detach();
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
                            // Render-time re-assertion, not a drag:
                            // don't record/persist it.
                            if this.suppress_resize_records > 0 {
                                this.suppress_resize_records -= 1;
                                return;
                            }
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
        // Quick-open keystrokes: re-rank the hits against the working
        // tree. Same contract as the find bar's subscription — the
        // input emits on every edit, and AppView is not borrowed while
        // it does.
        {
            let input = this.file_search.input.clone();
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &input::InputEvent, _, cx| {
                    if matches!(event, input::InputEvent::Change) {
                        this.refresh_file_search(cx);
                    }
                },
            )
            .detach();
        }
        // Find-bar keystrokes: recompute matches against the open
        // file. `InputEvent::Change` fires on every edit, so the
        // counter/highlight track typing live; AppView isn't borrowed
        // while the input emits, so a direct refresh is safe (unlike
        // the splitter handlers above, which can re-enter).
        {
            let input = this.diff_search.input.clone();
            cx.subscribe_in(
                &input,
                window,
                |this, _, event: &input::InputEvent, _, cx| {
                    if matches!(event, input::InputEvent::Change) {
                        this.refresh_diff_search(cx);
                        if !this.diff_search.matches.is_empty() {
                            this.diff_search.current = 0;
                            this.diff_hunks_scroll
                                .scroll_to_item(this.diff_search.matches[0], ScrollStrategy::Nearest);
                        }
                    }
                },
            )
            .detach();
        }
        // Record the window frame (move/resize/zoom/fullscreen) as it
        // changes; `persist` writes it, and a debounced save covers a
        // crash between the change and the next explicit persist.
        cx.observe_window_bounds(window, |this, window, cx| {
            this.note_window_frame(window, cx);
        })
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
        // Restore the persisted session lists — every row that was still
        // running comes back live (see `restore_sessions`), the finished
        // ones idle as resumable `Done` rows. The saved selection then
        // only decides focus. With nothing saved, fall back to the
        // configured default launcher.
        if state
            .projects
            .as_ref()
            .is_some_and(|ps| ps.iter().any(|p| !p.sessions.is_empty()))
        {
            this.restore_sessions(&state, window, cx);
            if this.projects[this.current_project].sessions.is_empty() {
                this.spawn_session_of(&cfg.new_session.kind, window, cx);
            } else {
                let n = this.projects[this.current_project].sessions.len();
                this.current_session = state.current_session.min(n.saturating_sub(1));
                if let Some(term) = this.current_term() {
                    let focus = term.read(cx).focus.clone();
                    focus.focus(window, cx);
                }
            }
        } else {
            this.spawn_session_of(&cfg.new_session.kind, window, cx);
        }
        // Seed the diff pane from the restored current session —
        // selection, collapsed dirs and the tree-layer height are all
        // per-session; a fresh spawn carries none (default height).
        let (seed, closed, height, mode) = match this.current_session() {
            Some(s) => (
                s.diff_selected.clone(),
                s.diff_open.clone(),
                s.diff_tree_height,
                s.view_mode.as_deref().and_then(ViewMode::parse),
            ),
            None => (None, Default::default(), None, None),
        };
        this.diff_seed_path = seed;
        // The pane's mode is per session and restored with the row.
        this.view_mode = mode.unwrap_or_default();
        this.diff_tree_open = closed;
        this.diff_tree_height_seed = height
            .map(gpui::px)
            .filter(|h| h.as_f32() >= tree_min_h() && h.as_f32() <= tree_max_h());

        // Surface settings/state load failures once: bundled launches
        // lose stderr, so corrupt-file/backup warnings would otherwise
        // be silent. Deferred — the dialog layer needs the window's
        // Root, built right after this constructor returns.
        // Set unconditionally at startup, before any window opens.
        let warnings = std::mem::take(&mut cx.global_mut::<crate::config::LoadWarnings>().0);
        if !warnings.is_empty() {
            let message = warnings.join("\n");
            window
                .spawn(cx, async move |cx| {
                    let _ = cx.update(move |window, cx| {
                        window.open_alert_dialog(cx, move |alert, _, _| {
                            alert
                                .title("Couldn't Load Settings or State")
                                .description(message.clone())
                                .footer(crate::ui::alert_ok_footer())
                        });
                    });
                })
                .detach();
        }
        this
    }

    /// `None` once the user removed the last project (empty workspace).
    pub(crate) fn current_project(&self) -> Option<&Project> {
        self.projects.get(self.current_project)
    }

    pub(crate) fn current_session(&self) -> Option<&crate::session::AgentSession> {
        self.current_project()?.sessions.get(self.current_session)
    }

    pub(crate) fn current_term(&self) -> Option<Entity<TermSession>> {
        self.current_session().and_then(|s| s.term.clone())
    }

    /// True when `term` is the session the center pane renders.
    ///
    /// Only that session's grid is in the element tree, so only its
    /// output can change what a frame shows: background rows keep
    /// parsing (their grid must be current when the row is selected)
    /// but must not ask for repaints — several streaming agents would
    /// otherwise each drive a full-window redraw and multiply the
    /// frame rate the pump's throttle exists to bound.
    pub(crate) fn is_visible_term(&self, term: &Entity<TermSession>) -> bool {
        self.current_term().is_some_and(|current| current == *term)
    }

    /// Record the visible session's OSC title; true when it changed
    /// since the last stream frame.
    ///
    /// The sidebar row and the breadcrumb mirror the title, and agent
    /// CLIs animate it (a spinner glyph). Both are outside the terminal
    /// pane, so a stream frame has to notify them — but nothing else:
    /// output that does not change the title must not rebuild them.
    /// Keyed on the session so a switch (same title, other session)
    /// still counts as a change.
    pub(super) fn note_shown_title(
        &mut self,
        term: &Entity<TermSession>,
        cx: &mut App,
    ) -> bool {
        let title = term.read(cx).title();
        let id = term.entity_id();
        if self
            .shown_title
            .as_ref()
            .is_some_and(|(prev_id, prev)| *prev_id == id && *prev == title)
        {
            return false;
        }
        self.shown_title = Some((id, title));
        true
    }

    /// Repaint the cached shell panels, alongside the app itself.
    ///
    /// The panels are cached child views (`panel_view!`): gpui replays
    /// their last frame until they are notified, so anything that
    /// changes what a panel shows has to come through here or the panel
    /// keeps rendering stale content. Installed as an app-level
    /// observer, so every `cx.notify()` on `AppView` reaches all three
    /// without touching the ~20 call sites.
    pub(crate) fn notify_panels(&mut self, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |_, cx| cx.notify());
        self.diff_pane.update(cx, |_, cx| cx.notify());
        self.terminal_pane.update(cx, |_, cx| cx.notify());
        self.breadcrumb.update(cx, |_, cx| cx.notify());
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
