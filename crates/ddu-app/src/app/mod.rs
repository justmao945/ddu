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

use ddu_diff::Snapshot;
use ddu_core::session::{AgentStatus, Project, initial_projects};
use ddu_terminal::{TermEvent, TermSession};
use crate::ui;
use crate::ui::diff_panel::ViewMode;
use crate::ui::file_tree::{tree_max_h, tree_min_h};
pub(crate) use keys::accel_hint;

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
        CopyFilePath,
        CopyFileContents,
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

/// Longest the duration tick sleeps before re-reading the session set: a
/// session that appears between two turn-overs is then honoured within a
/// minute. The label's own boundaries are still hit exactly — this caps
/// how long the *look* at the rows waits, not where they land.
const LABEL_TICK_CAP: Duration = Duration::from_secs(60);

/// The expansion read on an empty workspace: no project means no tree to
/// expand, and every reader wants a set rather than an `Option` (see
/// [`AppView::tree_open`]).
static NO_TREE_OPEN: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);

mod diff;
pub mod keys;
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
    pub(crate) projects: Vec<ddu_core::session::Project>,
    /// Which project rows are expanded in the sidebar tree.
    pub(crate) expanded: Vec<bool>,
    pub(crate) current_project: usize,
    pub(crate) current_session: usize,
    /// Every session's OSC title as the sidebar row last showed it,
    /// keyed by the term entity: a stream frame that changes a title
    /// (agent CLIs spin it) has to reach the sidebar, which is otherwise
    /// cached/replayed off its own notifications only — the *visible*
    /// session's change also reaches the breadcrumb. Keyed per session
    /// because a row whose title moves is a row the user can see whether
    /// or not its grid is on screen (see `note_row_title`).
    pub(crate) row_titles: std::collections::HashMap<gpui::EntityId, Option<String>>,
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
    /// The tree's rows + totals, rebuilt when the diff, the expansion or
    /// the listing changes (never per frame — see `build_index`).
    pub(crate) tree_index: Option<crate::ui::file_tree::TreeIndex>,
    /// The project's repository, opened once and kept for the session: its
    /// workdir names the root both listings read (the tree's, the quick
    /// open's walk) **and every path they hand back is read against**
    /// (`diff_root` — the project's own directory is not that root when it
    /// sits inside the repository), its index is the quick open's instant
    /// answer, and no repository at all means no tree. Keyed by the
    /// directory it was discovered from: a session switch to another
    /// project re-discovers.
    pub(crate) repo: Option<(std::path::PathBuf, Rc<ddu_diff::git2::Repository>)>,
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
    /// The incoming project's remembered file-tree offset, applied when
    /// its rows first exist after a switch (`rebuild_tree_index`): before
    /// the poll's snapshot there is no index to put an offset on.
    pub(crate) pending_tree_scroll: Option<Pixels>,
    pub(crate) session_seq: usize,
    /// The last applied poll: the diff **and** the full working-tree
    /// listing, always written together (one poll, one snapshot — the
    /// tree and the pane can never disagree about what changed).
    pub(crate) snapshot: Option<Snapshot>,
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
    /// The rendered documents the pane has drawn, keyed by working tree
    /// and path: a Markdown document's scroll lives in its own gpui state
    /// (gpui keeps that state only while the element is rendered), so
    /// holding the state is what brings the document back to the passage
    /// it was left at — across a file switch, a session switch and a poll.
    /// In memory only, like `file_positions`; one table per working tree,
    /// so the read side keys a path by reference and allocates nothing.
    pub(crate) documents: std::collections::HashMap<
        std::path::PathBuf,
        std::collections::HashMap<String, Entity<gpui_kit::base::TextViewState>>,
    >,
    /// Guards against stale poll results overwriting newer ones.
    diff_seq: u64,
    /// Which surface the right pane shows (⌘⇧M toggles it); per session,
    /// persisted like the selection.
    pub(crate) view_mode: ViewMode,
    /// Cached whole-file view for File mode (read off the UI thread),
    /// and the `(path, generation)` it was built for: the pane only
    /// renders it while both still match.
    pub(crate) file_view: Option<std::rc::Rc<ddu_diff::file_view::FileView>>,
    pub(crate) file_view_key: Option<(String, u64)>,
    /// The rendered document's Markdown source — or the reason it could
    /// not be read.    /// read — for the `(path, generation)` it was read for. One field for
    /// both outcomes: a refusal has to be as keyed as a success, or the
    /// pane would band "reading the file" forever on a file it can never
    /// read.
    pub(crate) preview: Option<ddu_diff::file_view::PreviewBuild>,
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
    pub(crate) window_placement: Option<ddu_core::config::WindowPlacement>,
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
pub fn window_min_width() -> f32 {
    sidebar_min() + center_min() + diff_min() + ui::scaled(8.)
}
pub fn window_min_height() -> f32 {
    ui::scaled(400.)
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Global shortcuts, and the user's overrides on top of them
        // (`keys::install` binds the builtin table once per process — a
        // window can be re-created from the dock — then applies whatever
        // `settings.json`'s `keys` map says).
        keys::install(cx);

        let cfg = cx.global::<ddu_core::config::Config>().clone();
        let state = cx.global::<ddu_core::config::State>().clone();
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
                    // The file tree is the project's, so its expansion and
                    // layer height come back with it; the default-expansion
                    // rule re-runs once for the launch (see
                    // `rebuild_tree_index`).
                    tree_open: p.tree_open.iter().cloned().collect(),
                    tree_height: p
                        .tree_height
                        .filter(|h| *h >= tree_min_h() && *h <= tree_max_h()),
                    // The tree is the project's, so its place is too. Only
                    // ever a scrolled-down offset: a positive one would
                    // put the first row below the layer's top.
                    tree_scroll: p.tree_scroll.filter(|y| *y <= 0.),
                    tree_seeded: false,
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
            row_titles: Default::default(),
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
            pending_tree_scroll: None,
            session_seq: 0,
            healed_shell_at: px(0.),
            healed_panes_at: px(0.),
            suppress_resize_records: 0,
            snapshot: None,
            diff_error: None,
            diff_search: search::DiffSearch::new(window, cx),
            file_search: search::FileSearch::new(window, cx),
            file_positions: std::collections::HashMap::new(),
            pending_scroll: None,
            documents: Default::default(),
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
        // Seed the diff pane from the restored current session: the
        // selected *file* and the pane's mode are the row's, while the
        // tree-layer height is the **project's** — the file tree is one
        // per project, shared by every session in it.
        let (seed, mode) = match this.current_session() {
            Some(s) => (
                s.diff_selected.clone(),
                s.view_mode.as_deref().and_then(ViewMode::parse),
            ),
            None => (None, None),
        };
        this.diff_seed_path = seed;
        // The pane's mode is per session and restored with the row.
        this.view_mode = mode.unwrap_or_default();
        this.diff_tree_height_seed = this
            .current_project()
            .and_then(|p| p.tree_height)
            .map(gpui::px);
        // The layer's place comes back the same way — applied when the
        // first poll gives the tree its rows (`rebuild_tree_index`).
        this.pending_tree_scroll = this
            .current_project()
            .and_then(|p| p.tree_scroll)
            .map(gpui::px);

        // Surface settings/state load failures once: bundled launches
        // lose stderr, so corrupt-file/backup warnings would otherwise
        // be silent. Deferred — the dialog layer needs the window's
        // Root, built right after this constructor returns.
        // Set unconditionally at startup, before any window opens.
        let warnings = std::mem::take(&mut cx.global_mut::<ddu_core::config::LoadWarnings>().0);
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

    /// The current project's file-tree expansion: which directories the
    /// layer lists. The tree is the **project's** — one repository, one
    /// working tree, one expansion — so switching sessions inside it
    /// leaves the tree exactly where it was, and a session remembers
    /// only the file it has open.
    pub(crate) fn tree_open(&self) -> &HashSet<String> {
        match self.current_project() {
            Some(project) => &project.tree_open,
            None => &NO_TREE_OPEN,
        }
    }

    /// [`Self::tree_open`] for mutation; `None` on an empty workspace
    /// (there is no tree to expand — and none to render a row from).
    pub(crate) fn tree_open_mut(&mut self) -> Option<&mut HashSet<String>> {
        self.projects
            .get_mut(self.current_project)
            .map(|project| &mut project.tree_open)
    }

    pub(crate) fn current_session(&self) -> Option<&ddu_core::session::AgentSession> {
        self.current_project()?.sessions.get(self.current_session)
    }

    pub(crate) fn current_term(&self) -> Option<Entity<TermSession>> {
        self.current_session().and_then(|s| s.term.clone())
    }

    /// True when `term` is the session the center pane renders.
    ///
    /// Only that session's grid is in the element tree, so only its
    /// output can change what the *pane* shows: background rows keep
    /// parsing (their grid must be current when the row is selected)
    /// but never repaint for it — several streaming agents would
    /// otherwise each drive a full-window redraw and multiply the frame
    /// rate the pump's throttle exists to bound. The one exception is a
    /// row's OSC title, which is on screen for a background session too
    /// (`note_row_title`).
    pub(crate) fn is_visible_term(&self, term: &Entity<TermSession>) -> bool {
        self.current_term().is_some_and(|current| current == *term)
    }

    /// Record `term`'s OSC title; true when the sidebar row showing it
    /// moved.
    ///
    /// The row mirrors the title and agent CLIs animate it (a spinner
    /// glyph), so a stream frame that changes it has to notify the
    /// *cached* sidebar. This holds for a background session as much as
    /// for the one on screen: the row is visible either way, and letting
    /// only the visible session's title through left a background
    /// spinner stepping at the duration tick — visibly jerky — while the
    /// grid it belongs to was off screen and cost nothing to skip. An
    /// unchanged title notifies nobody, so an agent that is alive but
    /// silent still drives no repaints.
    pub(super) fn note_row_title(&mut self, term: &Entity<TermSession>, cx: &mut App) -> bool {
        let title = term.read(cx).title();
        // A session that has never set a title reads `None` here *and* on
        // the row (which falls back to the launcher's name), so a row
        // seen for the first time is not a change — while the first real
        // title, arriving after any number of titleless frames, is one.
        let shown = self.row_titles.entry(term.entity_id()).or_default();
        if *shown == title {
            return false;
        }
        *shown = title;
        true
    }

    /// Repaint the cached shell panels, alongside the app itself.
    ///
    /// The panels are cached child views (`panel_view!`): gpui replays
    /// their last frame until they are notified, so anything that
    /// changes what a panel shows has to come through here or the panel
    /// keeps rendering stale content. Installed as an app-level
    /// observer, so every `cx.notify()` on `AppView` reaches all four
    /// without touching the ~20 call sites. A repaint that concerns one
    /// panel (the duration tick, a stream frame) notifies that panel
    /// directly instead — a full fan-out costs a terminal grid and a
    /// changes pane for content that did not move.
    pub(crate) fn notify_panels(&mut self, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |_, cx| cx.notify());
        self.diff_pane.update(cx, |_, cx| cx.notify());
        self.terminal_pane.update(cx, |_, cx| cx.notify());
        self.breadcrumb.update(cx, |_, cx| cx.notify());
    }

    /// Repaint the sidebar when a run's duration reading turns over.
    ///
    /// The reading is minute-resolution (`AgentSession::elapsed_label`):
    /// a per-second notify — and the app-level fan-out
    /// ([`Self::notify_panels`]) carried that to *every* cached panel —
    /// redrew the terminal grid and the changes pane 60 times for a
    /// string that had not moved, and once an hour for a run past its
    /// first hour. The tick now sleeps to the soonest boundary and
    /// notifies the sidebar alone, the one panel that shows it: measured
    /// 0.58% → 0.08% of a core on an idle visible window, with the
    /// per-second signature gone from the timeline (`docs/UI.md`).
    fn start_ui_tick(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                let wait = this
                    .update(cx, |this, _| this.next_label_change(std::time::Instant::now()))
                    .unwrap_or(LABEL_TICK_CAP);
                cx.background_executor().timer(wait).await;
                let woken = this
                    .update(cx, |this, cx| this.sidebar.update(cx, |_, cx| cx.notify()));
                if woken.is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Time to the soonest `m`/`h`/`d` turn-over in the sidebar, capped
    /// at [`LABEL_TICK_CAP`]: the cap is what notices a session that
    /// appeared since the last wake (or a clock that jumped), and since
    /// the narrowest unit is a minute it costs a wakeup, never a repaint.
    pub(crate) fn next_label_change(&self, now: std::time::Instant) -> Duration {
        self.projects
            .iter()
            .flat_map(|project| project.sessions.iter())
            .filter_map(|session| session.label_change_in(now))
            .min()
            .unwrap_or(LABEL_TICK_CAP)
            .min(LABEL_TICK_CAP)
    }
}
