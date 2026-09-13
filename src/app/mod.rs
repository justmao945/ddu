//! App shell view: Zed-style three-pane workspace.
//!
//! State, keyboard actions and session mutations live here; the four
//! surface regions live in [`crate::ui`] as `impl AppView` blocks.
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
use crate::ui::diff_tree::{tree_max_h, tree_min_h};

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
    pub(crate) tree_index: Option<crate::ui::diff_tree::TreeIndex>,
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
    pub(crate) diff_search: diff::DiffSearch,
    /// Guards against stale poll results overwriting newer ones.
    diff_seq: u64,
    /// Which surface the right pane shows (⌘⇧M toggles it); per session,
    /// persisted like the selection.
    pub(crate) view_mode: ViewMode,
    /// Cached whole-file view for File mode (read off the UI thread),
    /// and the `(path, generation)` it was built for: the pane only
    /// renders it while both still match.
    pub(crate) file_view: Option<std::rc::Rc<crate::diff::view::FileView>>,
    pub(crate) file_view_key: Option<(String, u64)>,
    /// The rendered document's Markdown source — or the reason it could
    /// not be read.    /// read — for the `(path, generation)` it was read for. One field for
    /// both outcomes: a refusal has to be as keyed as a success, or the
    /// pane would band "reading the file" forever on a file it can never
    /// read.
    pub(crate) preview: Option<crate::diff::view::PreviewBuild>,
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
    pub(crate) terminal_pane: Entity<ui::terminal::PanelView>,
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

/// Copy / paste / find, the three chords the terminal surface shares
/// with the shell it runs. macOS keeps the ⌘ chords; elsewhere they
/// move into the ⌃⇧ space, because ⌃C is the shell's SIGINT and ⌃V / ⌃F
/// belong to readline — a terminal that swallowed those would be
/// unusable. ⌃⇧* is exactly the space Linux terminals already reserve
/// for their own actions.
pub(crate) const COPY_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-c"
} else {
    "ctrl-shift-c"
};
pub(crate) const PASTE_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-v"
} else {
    "ctrl-shift-v"
};
pub(crate) const FIND_ACCEL: &str = if cfg!(target_os = "macos") {
    "cmd-f"
} else {
    "ctrl-shift-f"
};

/// Shortcut label for user-facing text (`⌘N` on macOS, `Ctrl+N` elsewhere).
pub(crate) fn accel_hint(key: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("⌘{key}")
    } else {
        format!("Ctrl+{key}")
    }
}

/// The full key binding table, in one place so `bind_keys` and the
/// routing regression test share it. Bindings for the same keystroke
/// form a fallback chain: gpui dispatches them in precedence order
/// (deepest matching context slice first, ties to the later entry)
/// until one handler stops propagation — see gpui's
/// `bindings_for_input` / `dispatch_key`.
///
/// Every app shortcut is a `secondary-` chord: ⌘ on macOS, ⌃ elsewhere.
/// A chord a binding claims never reaches the PTY — gpui's bubble-phase
/// action dispatch stops propagation before the terminal's key listener
/// runs — so on Linux these override the shell's own readline keys while
/// the terminal holds focus (`⌃R` toggles the changes pane, not reverse
/// search); the shell keeps everything the app leaves unbound.
fn key_bindings() -> Vec<KeyBinding> {
    vec![
        // Secondary (⌘ / ⌃) + N spawns the default launcher in the
        // active project (guarded: with no projects there is nothing
        // to spawn into); + O adds a project via the folder picker;
        // + T toggles the diff file tree under the project tree.
        KeyBinding::new("secondary-n", NewSession, None),
        KeyBinding::new("secondary-o", AddProject, None),
        KeyBinding::new("secondary-t", ToggleDiffTree, None),
        KeyBinding::new("secondary-b", ToggleSessions, None),
        KeyBinding::new("secondary-r", ToggleDiff, None),
        KeyBinding::new("secondary-w", CloseSession, None),
        // Cycle the right pane's surface: diff hunks → whole file →
        // rendered Markdown (Markdown files only).
        KeyBinding::new("secondary-shift-m", ToggleViewMode, None),
        // Secondary +/− (with their shifted variants) zoom the
        // terminal font size; persisted like the settings field.
        KeyBinding::new("secondary-=", FontLarger, None),
        KeyBinding::new("secondary-+", FontLarger, None),
        KeyBinding::new("secondary--", FontSmaller, None),
        KeyBinding::new("secondary-_", FontSmaller, None),
        // Secondary 1..9: select the Nth session in the current
        // project. Prefixed so bare digits keep reaching the PTY.
        KeyBinding::new("secondary-1", SelectSession1, None),
        KeyBinding::new("secondary-2", SelectSession2, None),
        KeyBinding::new("secondary-3", SelectSession3, None),
        KeyBinding::new("secondary-4", SelectSession4, None),
        KeyBinding::new("secondary-5", SelectSession5, None),
        KeyBinding::new("secondary-6", SelectSession6, None),
        KeyBinding::new("secondary-7", SelectSession7, None),
        KeyBinding::new("secondary-8", SelectSession8, None),
        KeyBinding::new("secondary-9", SelectSession9, None),
        // Terminal-scoped: these beat gpui-component Root's global
        // Tab/Shift-Tab focus cycling (deeper key context wins), so
        // the PTY gets real tab/backtab bytes and focus never jumps
        // to sidebar buttons mid-session. Paste is unbound globally.
        KeyBinding::new("tab", TermTab, Some("Terminal")),
        KeyBinding::new("shift-tab", TermBacktab, Some("Terminal")),
        KeyBinding::new(PASTE_ACCEL, TermPaste, Some("Terminal")),
        // Copy grabs the mouse selection when one exists (the
        // handler propagates otherwise); PTYs never see it.
        KeyBinding::new(COPY_ACCEL, TermCopy, Some("Terminal")),
        // Outside the terminal, copy grabs the active diff-pane
        // text selection (window-scoped `TextSelection`); the
        // handler propagates when nothing is selected.
        KeyBinding::new(COPY_ACCEL, input::Copy, None),
        // Two find bars share one chord by focus. gpui ranks a
        // binding by the deepest stack slice its predicate needs: a
        // named context sitting at the focused element scores len, a
        // predicate-less binding always scores len, and ties go to
        // the later binding — so a bare None here would permanently
        // out-rank the Terminal binding below. `!Terminal` matches
        // one slice *shallower* than `Terminal` itself (the largest
        // slice excluding it), so the positive binding always wins
        // by exactly one level whenever the terminal surface or its
        // find bar holds focus, and the diff binding wins anywhere
        // else ("Terminal" absent from every slice). When a bar's
        // input already holds focus its own action refocuses it
        // with the query selected, matching platform find bars.
        KeyBinding::new(FIND_ACCEL, TermSearch, Some("Terminal")),
        KeyBinding::new(FIND_ACCEL, DiffSearch, Some("!Terminal")),
        // The diff pane's find bar: once its input holds focus the
        // "DiffSearch" context is on the dispatch path. Enter/
        // Shift-Enter come from the input itself (it dispatches
        // the `Enter` action), handled on the bar in
        // `ui::diff_panel`.
        KeyBinding::new("secondary-g", DiffSearchNext, Some("DiffSearch")),
        KeyBinding::new("secondary-shift-g", DiffSearchPrev, Some("DiffSearch")),
        // The terminal bar's match-cycling: its "TerminalSearch"
        // context (set on the bar in `ui::terminal`) is deeper
        // than the surface's "Terminal", so these win while its
        // input holds focus.
        KeyBinding::new("secondary-g", TermSearchNext, Some("TerminalSearch")),
        KeyBinding::new("secondary-shift-g", TermSearchPrev, Some("TerminalSearch")),
        // The standalone settings window: Escape / secondary+W close
        // it (the deeper context beats the global close-session one).
        KeyBinding::new("escape", CloseSettings, Some("SettingsWindow")),
        KeyBinding::new("secondary-w", CloseSettings, Some("SettingsWindow")),
    ]
}

#[cfg(test)]
mod tests {
    use super::{COPY_ACCEL, FIND_ACCEL, PASTE_ACCEL, key_bindings};
    use gpui_kit::{KeyContext, Keymap, Keystroke};

    /// The fallback chain gpui would dispatch for `keystroke`, in
    /// precedence order, given a context stack (bottom → top, as the
    /// dispatch tree builds it).
    fn chain(keystroke: &str, stack: &[&str]) -> Vec<String> {
        let keymap = Keymap::new(key_bindings());
        let contexts = stack
            .iter()
            .map(|c| KeyContext::parse(c).unwrap())
            .collect::<Vec<_>>();
        let keystrokes = vec![Keystroke::parse(keystroke).unwrap()];
        let (bindings, _) = keymap.bindings_for_input(&keystrokes, &contexts);
        bindings
            .iter()
            .map(|b| b.action().name().to_string())
            .collect()
    }

    /// Highest-precedence action gpui would dispatch for `keystroke`.
    fn winner(keystroke: &str, stack: &[&str]) -> String {
        chain(keystroke, stack).first().cloned().unwrap_or_default()
    }

    /// The find chord must open the search bar of whichever pane holds
    /// focus. Regression: the diff binding used a predicate-less
    /// context, which ties any named context on depth and wins the
    /// later-binding tiebreak — so the chord in the terminal opened the
    /// diff pane's bar.
    #[test]
    fn find_routes_by_focus() {
        // Terminal surface focused.
        assert_eq!(winner(FIND_ACCEL, &["Root", "Terminal"]), "ddu::TermSearch");
        // Terminal find bar's input focused (its own context on top
        // of the surface's).
        assert_eq!(
            winner(FIND_ACCEL, &["Root", "Terminal", "TerminalSearch"]),
            "ddu::TermSearch"
        );
        // Diff find bar's input focused.
        assert_eq!(winner(FIND_ACCEL, &["Root", "DiffSearch"]), "ddu::DiffSearch");
        // Anything else (sidebar rows don't take focus, so the
        // gpui-component Root context stays on the path).
        assert_eq!(winner(FIND_ACCEL, &["Root"]), "ddu::DiffSearch");
        // Note: gpui's `Not` predicate never evaluates against an
        // empty context stack (its eval guards on a non-empty slice),
        // but the dispatch path always includes at least the Root
        // context, so the unbound case can't occur in the app.
    }

    /// Copy's fallback chain: the diff-pane copy runs first and yields
    /// to the terminal copy when a terminal selection exists.
    #[test]
    fn copy_falls_back_to_terminal() {
        assert_eq!(
            chain(COPY_ACCEL, &["Root", "Terminal"]),
            vec!["input::Copy", "ddu::TermCopy"]
        );
        assert_eq!(chain(COPY_ACCEL, &["Root"]), vec!["input::Copy"]);
    }

    /// The terminal shares the keyboard with the shell it runs, so the
    /// chords readline and the OS own must stay unbound inside it:
    /// Ctrl-C is SIGINT, Ctrl-D ends input, Ctrl-Z suspends, and
    /// Ctrl-V/ Ctrl-F/ Ctrl-\\ belong to readline. macOS is exempt —
    /// the PTY never sees ⌘ chords, so the shared chords are already
    /// out of the PTY's way.
    #[test]
    fn shell_control_keys_stay_with_the_shell() {
        if cfg!(target_os = "macos") {
            return;
        }
        for key in ["ctrl-c", "ctrl-d", "ctrl-v", "ctrl-f", "ctrl-z", "ctrl-\\"] {
            assert_eq!(
                winner(key, &["Root", "Terminal"]),
                "",
                "{key} must reach the shell, not an app action"
            );
        }
        // What the terminal surface does claim is copy/paste/find, in
        // the shifted space no shell reads.
        let term = ["Root", "Terminal"];
        assert!(chain(COPY_ACCEL, &term).contains(&"ddu::TermCopy".to_string()));
        assert!(chain(PASTE_ACCEL, &term).contains(&"ddu::TermPaste".to_string()));
        assert_eq!(winner(FIND_ACCEL, &term), "ddu::TermSearch");
    }
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
        let terminal_pane = cx.new(|_| ui::terminal::PanelView::new(app.clone()));
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
            diff_search: diff::DiffSearch::new(window, cx),
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

    /// Re-assert the recorded sidebar / diff widths after the *window*
    /// changed size: a resize that lands while every slot was still
    /// pinned (startup's burst, a monitor switch) leaves the panels at
    /// proportions nobody asked for.
    ///
    /// This must **not** run from `render`. A drag changes a panel's size
    /// several times a second while the persisted record only catches up a
    /// tick later, so a render in between (a stream frame, the 1 s clock,
    /// the poll) would "correct" a live drag back to the width it started
    /// at — the divider that sometimes refuses to be dragged. Window
    /// bounds change for reasons a drag cannot, which is exactly the
    /// trigger this healing wants.
    fn heal_splitter_widths(&mut self, cx: &mut Context<Self>) {
        // A drag redistributes within one container: it never changes
        // this, which is what keeps the healing from fighting one.
        let shell_container = self.shell_state.read(cx).container_size();
        let panes_container = self.panes_state.read(cx).container_size();
        // Re-measured rather than nudged. gpui-base pins a slot at its
        // *first* measured bounds, and a slot measured while the group had
        // another shape — the center alone, before the diff pane existed,
        // where it measures the whole container — keeps that number for
        // good. A drag reads the pair as its starting widths and hands the
        // changed space to the sibling, so a pair that is stale (or that no
        // longer sums to the container) puts the divider wherever that
        // arithmetic lands, not at the pointer. Dropping the state re-pins
        // both slots against a layout with the current shape, and the sized
        // panel re-asserts its recorded width as its initial size.
        if shell_container > px(0.)
            && (shell_container != self.healed_shell_at
                || self.shell_state.read(cx).sizes().len() != self.shell_len())
        {
            self.healed_shell_at = shell_container;
            self.shell_state.update(cx, |state, _| state.clear());
        }
        if panes_container > px(0.)
            && (panes_container != self.healed_panes_at
                || self.panes_state.read(cx).sizes().len() != self.panes_len())
        {
            self.healed_panes_at = panes_container;
            self.panes_state.update(cx, |state, _| state.clear());
        }
    }

    /// How many slots a splitter has this frame — what its state is
    /// re-measured for when the two disagree.
    fn shell_len(&self) -> usize {
        if self.show_sessions { 2 } else { 1 }
    }

    fn panes_len(&self) -> usize {
        if self.show_diff { 2 } else { 1 }
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

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The root tracks the window fallback focus: every pane focuses
        // this handle on click (so the terminal cursor goes hollow), and
        // with it in the element tree the global shortcuts (⌘N/⌘T/⌘B/…)
        // keep a dispatch path no matter what was clicked last.
        v_flex()
            .id("app-root")
            .relative()
            .track_focus(&self.window_focus)
            .size_full()
            .bg(cx.theme().background)
            // Left-drag gesture fence: keep the window repainting while a
            // selection drag is in flight. `TextSelectionLayer` observes
            // mouse events via `Window::on_mouse_event`, whose listeners
            // live only for one frame after a render — without a steady
            // frame stream the remaining move events of a drag are dropped
            // and selection freezes until some unrelated repaint. Terminal
            // selection uses persistent element callbacks, so it never had
            // this problem; this fence gives the window-level listeners the
            // same continuous frame stream.
            .on_mouse_down(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, _| {
                    this.selection_active = true;
                }),
            )
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, _| {
                if this.selection_active {
                    window.refresh();
                }
            }))
            .on_mouse_up(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.selection_active = false;
                }),
            )
            .on_mouse_up_out(
                gpui_kit::MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| {
                    this.selection_active = false;
                }),
            )
            .on_action(cx.listener(|_, _: &OpenSettings, _, cx| ui::settings::open(cx)))
            .on_action(|_: &FontLarger, _, cx| ui::settings::bump_font_size(1., cx))
            .on_action(|_: &FontSmaller, _, cx| ui::settings::bump_font_size(-1., cx))
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
            .on_action(cx.listener(|this, _: &ToggleDiff, window, cx| this.toggle_diff(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleDiffTree, _, cx| this.toggle_diff_tree(cx)))
            .on_action(cx.listener(|this, _: &ToggleViewMode, _, cx| this.toggle_view_mode(cx)))
            // The diff pane's find bar. ⌘G/⌘⇧G resolve only while the
            // bar's input holds focus (the "DiffSearch" key context);
            // the handlers live here, not on the bar, so they keep
            // working if focus drifts to the pane mid-search.
            .on_action(cx.listener(|this, _: &DiffSearch, window, cx| {
                this.open_diff_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &DiffSearchNext, _, cx| {
                this.diff_search_step(false, cx);
            }))
            .on_action(cx.listener(|this, _: &DiffSearchPrev, _, cx| {
                this.diff_search_step(true, cx);
            }))
            // ⌘Q: confirm dialog, then graceful shutdown — live agents
            // get Ctrl-C, their resume ids land in state.json, then the
            // process exits.
            .on_action(cx.listener(|this, _: &Quit, window, cx| {
                this.request_quit(window, cx);
            }))
            // ⌘C outside the terminal: copy the diff pane's window
            // text selection (the terminal has its own TermCopy path
            // via its deeper key context).
            .on_action(|_: &input::Copy, window, cx| {
                let text = TextSelection::selected_text(window, cx);
                if text.is_empty() {
                    cx.propagate();
                    return;
                }
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            })
            // Window-scoped text selection (the diff pane's
            // `SelectableText` runs register here). Must prepaint
            // before any of them — first child of the root.
            .child(TextSelectionLayer)
            .child(ui::title_bar::bar(self))
            .child({
                // Two nested splitters. The sidebar column owns a status
                // strip, so the LEFT divider runs to the window's bottom
                // edge; the terminal/changes region wraps its splitter
                let panes_min = center_min() + if self.show_diff { diff_min() } else { 0. } + ui::scaled(8.);
                // The diff pane flexes with the window: its drag cap
                // scales with the viewport (a fixed cap reads cramped
                // on a big display), floored at DIFF_MAX.
                let diff_max = (window.viewport_size().width.as_f32() * 0.6).max(diff_max());
                // A container (window) resize proportionally rescales
                // every splitter panel that has a recorded size —
                // gpui-base's `adjust_to_container_size` bails only
                // while some panel is unpinned, and its
                // `update_panel_size` pins EVERY slot at its first
                // measured bounds. Keep the flex slots (the region,
                // the center pane) unpinned so the recorded sidebar /
                // diff widths survive window resizes verbatim; the
                // flex slot absorbs the whole delta.
                self.shell_state.update(cx, |state, cx| {
                    if state.sizes().len() > 1 {
                        state.reset_panel(1, cx);
                    }
                });
                self.panes_state.update(cx, |state, cx| {
                    if state.sizes().len() > 1 {
                        state.reset_panel(0, cx);
                    }
                });
                // Heal drift baked in while every slot was still pinned
                // (the launch resize burst): re-assert the recorded
                // widths, once per container size. A drag cannot trigger
                // it — a drag leaves the container alone — which is what
                // keeps a mid-drag frame from yanking the divider back.
                self.heal_splitter_widths(cx);
                let mut shell = h_resizable("shell").with_state(&self.shell_state);
                if self.show_sessions {
                    shell = shell.child(
                        resizable_panel()
                            .size(self.last_sidebar_w())
                            .flex_none()
                            .size_range(px(sidebar_min())..px(sidebar_max()))
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
                                            .child(
                                        self.sidebar
                                            .clone()
                                            .cached(ui::session_panel::root_style()),
                                    ),
                                    )
                                    .child(ui::status_bar::render_sidebar(self, cx)),
                            ),
                    );
                }
                let mut panes = h_resizable("panes").with_state(&self.panes_state);
                panes = panes.child(
                    resizable_panel()
                        .size_range(px(center_min())..px(f32::MAX))
                        .child(
                            div()
                                .debug_selector(|| "pane-center".into())
                                .h_full()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .child(self.terminal_pane.clone().cached(ui::terminal::root_style())),
                        ),
                );
                if self.show_diff {
                    // `.flex_none()`, like the sidebar: this is the sized
                    // pane, and an unsized flex sibling (the center) gets a
                    // flex *base* of the whole container — so leaving this
                    // one growable made the two shrink against each other
                    // and the divider land wherever that math put it
                    // instead of at the pointer (and the recorded width
                    // never matched the laid-out one). Held at its recorded
                    // width, the center absorbs the window delta.
                    //
                    // That width is the recorded one capped to what the
                    // container can hold beside the terminal's minimum: the
                    // record keeps the preference, so a wider window restores
                    // it. This is the panel's *initial* size, which is what
                    // the layout reads right after the state in
                    // `heal_splitter_widths` is dropped (a container resize,
                    // a pane toggle) — the steady state reads the record.
                    let room =
                        self.panes_state.read(cx).container_size() - px(center_min() + ui::scaled(8.));
                    let diff_w = if room > px(0.) {
                        self.last_diff_w().min(room)
                    } else {
                        self.last_diff_w()
                    };
                    panes = panes.child(
                        resizable_panel()
                            .size(diff_w)
                            .flex_none()
                            .size_range(px(diff_min())..px(diff_max))
                            .child(
                                div()
                                    .debug_selector(|| "pane-diff".into())
                                    .size_full()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .child(self.diff_pane.clone().cached(ui::diff_panel::root_style())),
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
            // Exit in progress (confirmed close/⌘Q): a dimming overlay
            // while agents are interrupted and their resume ids saved.
            .when(self.shutting_down, |el| {
                let n = self.running_terms().len();
                let detail = if n > 0 {
                    format!(
                        "Stopping {n} session{} and saving resume ids…",
                        if n == 1 { "" } else { "s" }
                    )
                } else {
                    "Saving session ids…".to_string()
                };
                el.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui_kit::black().opacity(0.45))
                        .child(
                            // The card, not bare text: the message must
                            // read at a glance while the window dies.
                            v_flex()
                                .items_center()
                                .gap_2()
                                .px_8()
                                .py_6()
                                .rounded_lg()
                                .bg(cx.theme().popover)
                                .border_1()
                                .border_color(cx.theme().border)
                                .shadow_lg()
                                .child(
                                    h_flex()
                                        .items_center()
                                        .gap_2()
                                        .text_base()
                                        .text_color(cx.theme().popover_foreground)
                                        .child(Spinner::new())
                                        .child("Exiting…"),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(detail),
                                ),
                        ),
                )
            })
    }
}

/// The shell renders its three panels as **cached child views** (see
/// `panel_view!`): gpui replays a cached subtree — layout, paint,
/// hitboxes, listeners, key contexts — until the view is notified, which
/// is what keeps a streaming terminal off the sidebar and the changes
/// pane. These tests pin both halves of that contract, plus the gpui
/// behavior they rest on.
#[cfg(test)]
mod panel_cache_tests {
    use crate::app::AppView;
    use crate::config::{Config, LoadWarnings, ProjectConfig, ShellConfig, State};
    use gpui_kit::{
        AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render,
        StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, gpui,
    };
    use std::cell::Cell;
    use std::rc::Rc;

    struct Child {
        renders: Rc<Cell<usize>>,
    }
    impl Render for Child {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().size_full()
        }
    }

    struct Parent {
        child: Entity<Child>,
        renders: Rc<Cell<usize>>,
    }
    impl Render for Parent {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.renders.set(self.renders.get() + 1);
            div().size_full().child(
                self.child
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            )
        }
    }

    /// The premise: gpui skips a cached child view's render while its
    /// parent re-renders, and re-runs it once that view is notified.
    #[test]
    fn cached_child_view_is_reused_across_a_parent_notify() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("cached_child_view_is_reused"));
                let cx = &mut cx0;
                let child_renders = Rc::new(Cell::new(0usize));
                let (parent, vcx) = cx.add_window_view({
                    let child_renders = child_renders.clone();
                    move |_window, cx| {
                        let child = cx.new(|_| Child {
                            renders: child_renders,
                        });
                        // The shell's fan-out (see `AppView::notify_panels`),
                        // which is what makes a parent-level notify reach
                        // the cached panels.
                        cx.observe_self({
                            let child = child.clone();
                            move |_, cx| child.update(cx, |_, cx| cx.notify())
                        })
                        .detach();
                        Parent {
                            child,
                            renders: Rc::new(Cell::new(0usize)),
                        }
                    }
                });
                let draw = |vcx: &mut gpui_kit::VisualTestContext| {
                    vcx.update(|window, cx| {
                        let _ = window.draw(cx);
                    })
                };
                draw(&mut vcx.clone());
                assert_eq!(child_renders.get(), 1, "first frame renders the child");
                draw(&mut vcx.clone());
                assert_eq!(
                    child_renders.get(),
                    1,
                    "an idle redraw must not re-render the cached child"
                );
                vcx.update(|_, cx| parent.update(cx, |_, cx| cx.notify()));
                draw(&mut vcx.clone());
                assert_eq!(
                    child_renders.get(),
                    2,
                    "notifying the parent re-renders the child once"
                );

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// One AppView with `rows` silent sessions (`/bin/cat` never prints,
    /// so no reader-thread wakeup races the test scheduler) and notify
    /// counters attached to the three cached panels.
    fn app_with_panel_counters(
        dispatcher: gpui::TestDispatcher,
        rows: usize,
        name: &'static str,
    ) -> (TestAppContext, Entity<AppView>, [Rc<Cell<usize>>; 3]) {
        let mut cx0 = TestAppContext::build(dispatcher, Some(name));
        let cx = &mut cx0;
        cx.update(gpui_kit::init);
        cx.update(|cx| {
            cx.set_app_identity("dev.just.ddu", "Day Day Up");
            cx.set_global(Config {
                shell: ShellConfig {
                    program: "/bin/cat".into(),
                },
                ..Default::default()
            });
            cx.set_global(LoadWarnings(vec![]));
            cx.set_global(State {
                projects: Some(vec![ProjectConfig {
                    name: "proj".into(),
                    path: std::env::temp_dir(),
                    expanded: true,
                    sessions: vec![],
                }]),
                ..Default::default()
            });
        });
        let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
        let counts: [Rc<Cell<usize>>; 3] = Default::default();
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
            for _ in 1..rows {
                view.update(cx, |v, cx| v.spawn_session_of("terminal", window, cx));
            }
            // Row 0 is the session on screen.
            view.update(cx, |v, cx| v.select_session(0, 0, window, cx));
        });
        view.update(cx, |v, cx| {
            let panels = (
                v.sidebar.clone(),
                v.diff_pane.clone(),
                v.terminal_pane.clone(),
            );
            cx.observe(&panels.0, {
                let counter = counts[0].clone();
                move |_, _, _| counter.set(counter.get() + 1)
            })
            .detach();
            cx.observe(&panels.1, {
                let counter = counts[1].clone();
                move |_, _, _| counter.set(counter.get() + 1)
            })
            .detach();
            cx.observe(&panels.2, {
                let counter = counts[2].clone();
                move |_, _, _| counter.set(counter.get() + 1)
            })
            .detach();
        });
        cx.run_until_parked();
        for counter in &counts {
            counter.set(0);
        }
        (cx0, view, counts)
    }

    /// A stream wakeup repaints the terminal pane alone: the cached
    /// sidebar and changes pane must not even be notified, or every
    /// frame of a stream would rebuild them (the whole point of the
    /// split). A background session repaints nothing at all.
    #[test]
    fn stream_wakeup_repaints_only_the_terminal_pane() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, view, counts) =
                    app_with_panel_counters(dispatcher, 2, "stream_wakeup_repaints_pane");
                let cx = &mut cx0;
                let (visible, background) = cx.update(|cx| {
                    let v = view.read(cx);
                    (
                        v.projects[0].sessions[0].term.clone().unwrap(),
                        v.projects[0].sessions[1].term.clone().unwrap(),
                    )
                });
                let seen = |counts: &[Rc<Cell<usize>>; 3]| counts.clone().map(|c| c.get());
                let wake = |term: &Entity<crate::terminal::TermSession>, cx: &mut gpui_kit::App| {
                    term.update(cx, |_, cx| cx.emit(crate::terminal::TermEvent::Wakeup));
                };
                cx.update(|cx| wake(&visible, cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [0, 0, 1],
                    "the visible session's stream notifies the pane only                      (sidebar, changes pane, terminal pane)"
                );

                cx.update(|cx| wake(&background, cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [0, 0, 1],
                    "a background row's stream repaints nothing"
                );

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// A stream frame that changes the OSC title reaches the sidebar
    /// row and the breadcrumb: both mirror the title, agent CLIs spin
    /// it, and a cached row that misses the notify shows a frozen
    /// spinner. An unchanged title must rebuild neither, and neither
    /// may reach the changes pane — that is what the split buys.
    #[test]
    fn a_title_change_follows_the_stream_into_the_sidebar() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, view, counts) =
                    app_with_panel_counters(dispatcher, 1, "a_title_change_reaches_the_sidebar");
                let cx = &mut cx0;
                let term = cx.update(|cx| {
                    view.read(cx).projects[0].sessions[0]
                        .term
                        .clone()
                        .unwrap()
                });
                let seen = |counts: &[Rc<Cell<usize>>; 3]| counts.clone().map(|c| c.get());
                // Titles arrive as OSC 0 in the byte stream; planting
                // the bytes wakes the pump like the reader thread does.
                let title = |text: &str, cx: &mut gpui_kit::App| {
                    term.update(cx, |term, _| {
                        term.inject_bytes(format!("\x1b]0;{text}\x07").as_bytes());
                    });
                };

                cx.update(|cx| title("⠋ working", cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [1, 0, 1],
                    "a new title repaints the row beside the pane                      (sidebar, changes pane, terminal pane)"
                );

                // Same title on the next stream frame: the pane repaints
                // (new bytes), the row must not.
                cx.executor().advance_clock(crate::terminal::STREAM_FRAME_MIN);
                cx.update(|cx| title("⠋ working", cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [1, 0, 2],
                    "an unchanged title leaves the cached row alone"
                );

                cx.executor().advance_clock(crate::terminal::STREAM_FRAME_MIN);
                cx.update(|cx| title("⠙ working", cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [2, 0, 3],
                    "the next spinner glyph reaches the row again"
                );

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// The pane divider must land where the pointer is, from the first
    /// move of the first drag.
    ///
    /// Regression, three parts of one bug. The right pane is the *sized*
    /// panel of its group, so it needs `.flex_none()`: an unsized flex
    /// sibling (the center) gets a flex base of the whole container, and
    /// leaving both flexible made them shrink against each other — the pane
    /// never got the width it asked for. gpui-base then pins each slot at
    /// its first measured bounds, and a slot measured while the group had
    /// another shape (the center alone, before the diff pane existed, where
    /// it measures the whole container) kept that number, so the drag's
    /// starting pair was stale and the changed space went to the wrong
    /// sibling. Both are needed for the divider to track the pointer.
    #[test]
    fn the_diff_divider_tracks_the_pointer() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("diff_divider_tracks"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_app_identity("dev.just.ddu", "Day Day Up");
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "/bin/cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![ProjectConfig {
                            name: "proj".into(),
                            path: std::env::temp_dir(),
                            expanded: true,
                            sessions: vec![],
                        }]),
                        ..Default::default()
                    });
                });
                let (view, mut vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                let draw = |vcx: &mut VisualTestContext| {
                    vcx.update(|window, cx| {
                        view.update(cx, |_, cx| cx.notify());
                        let _ = window.draw(cx);
                    });
                };
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.show_sessions = true;
                        v.show_diff = true;
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                });
                draw(&mut vcx);
                draw(&mut vcx);
                // The recorded pair describes the layout on screen: a stale
                // one (the center measuring the whole container) is what
                // sends the drag's space to the wrong sibling.
                let (first, last, container) = vcx.update(|_, cx| {
                    let state = view.read(cx).panes_state.read(cx);
                    (
                        state.sizes()[0],
                        state.sizes()[1],
                        state.container_size(),
                    )
                });
                let diff_box = vcx.debug_bounds("pane-diff").expect("the diff pane");
                assert_eq!(last, diff_box.size.width, "the record is the laid-out width");
                assert_eq!(
                    first + last,
                    container,
                    "and the pair adds up to the container"
                );
                let left = diff_box.origin.x;
                let y = gpui_kit::px(50.);
                vcx.simulate_mouse_down(
                    gpui_kit::point(left - gpui_kit::px(1.), y),
                    gpui_kit::MouseButton::Left,
                    gpui_kit::Modifiers::default(),
                );
                // The first move only starts the drag (gpui's threshold).
                vcx.simulate_mouse_move(
                    gpui_kit::point(left + gpui_kit::px(6.), y),
                    Some(gpui_kit::MouseButton::Left),
                    gpui_kit::Modifiers::default(),
                );
                for delta in [40., 100., -60.] {
                    let pointer = left + gpui_kit::px(delta);
                    vcx.simulate_mouse_move(
                        gpui_kit::point(pointer, y),
                        Some(gpui_kit::MouseButton::Left),
                        gpui_kit::Modifiers::default(),
                    );
                    draw(&mut vcx);
                    let box_ = vcx.debug_bounds("pane-diff").expect("the diff pane");
                    assert_eq!(
                        box_.origin.x, pointer,
                        "the divider is where the pointer is ({delta:+})"
                    );
                }
                // Dragged past its minimum the pane stops at the minimum:
                // the divider is at the pane's own edge, not off to the
                // right of the window.
                let pointer = left + gpui_kit::px(600.);
                vcx.simulate_mouse_move(
                    gpui_kit::point(pointer, y),
                    Some(gpui_kit::MouseButton::Left),
                    gpui_kit::Modifiers::default(),
                );
                draw(&mut vcx);
                let box_ = vcx.debug_bounds("pane-diff").expect("the diff pane");
                let min = crate::ui::scaled(200.);
                assert!(
                    (box_.size.width.as_f32() - min).abs() <= 1.,
                    "the pane stops at its minimum width, got {:?}",
                    box_.size.width
                );
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// Selecting a row pins the path, and the diff record only when the
    /// poll found one: the tree lists clean files, so a selection without
    /// a diff is normal (the pane shows the file itself).
    #[test]
    fn select_path_pins_clean_and_changed_files_alike() {
        use crate::diff::{DiffFile, GitDiff, Snapshot};
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("select_path_pins"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_global(Config::default());
                    cx.set_global(LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        let files = vec![DiffFile {
                            path: "changed.txt".into(),
                            added: 2,
                            removed: 1,
                            ..Default::default()
                        }];
                        v.snapshot = Some(Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                        });
                        v.select_path("clean.txt".to_owned());
                        assert_eq!(v.current_diff_path(), Some("clean.txt"));
                        assert_eq!(
                            v.selection.as_ref().and_then(|s| s.changed),
                            None,
                            "a clean file has no diff record"
                        );
                        assert!(v.selected_diff_file().is_none());

                        v.select_path("changed.txt".to_owned());
                        assert_eq!(
                            v.selection.as_ref().and_then(|s| s.changed),
                            Some(0),
                            "a changed file pins its index"
                        );
                        assert_eq!(
                            v.selected_diff_file().map(|f| f.added),
                            Some(2),
                            "and the index points at the runner"
                        );
                        cx.notify();
                    });
                });
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// An idle poll changes nothing: no repaint, no cache invalidation, no
    /// tree rebuild. Regression for the periodic flash — every 3 s tick
    /// used to bump the generation the pane's caches compare against, so
    /// File mode blanked to "Reading the file…" and rebuilt while the
    /// diff had not moved an inch.
    #[test]
    fn an_idle_poll_moves_nothing() {
        use crate::diff::{DiffFile, GitDiff, Snapshot};
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("idle_poll_moves_nothing"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_global(Config::default());
                    cx.set_global(LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|_, cx| {
                    let snapshot = Snapshot {
                        diff: GitDiff {
                            branch: Some("main".into()),
                            files: vec![DiffFile {
                                path: "a.txt".into(),
                                added: 1,
                                removed: 0,
                                ..Default::default()
                            }],
                        },
                    };
                    let first = view.update(cx, |v, cx| {
                        v.apply_snapshot(Ok(snapshot.clone()), cx)
                    });
                    let generation = view.read(cx).diff_gen;
                    assert!(first, "the first snapshot is a change");
                    assert_eq!(view.read(cx).current_diff_path(), None, "no click, no selection");

                    // The same poll again: not a change.
                    let second = view.update(cx, |v, cx| {
                        v.apply_snapshot(Ok(snapshot.clone()), cx)
                    });
                    assert!(!second, "an identical snapshot must move nothing");
                    assert_eq!(
                        view.read(cx).diff_gen,
                        generation,
                        "no cache invalidation"
                    );

                    // A real edit does move it.
                    let mut edited = snapshot.clone();
                    edited.diff.files[0].added = 2;
                    let third = view.update(cx, |v, cx| v.apply_snapshot(Ok(edited), cx));
                    assert!(third, "an edit is a change");
                    assert_eq!(view.read(cx).diff_gen, generation + 1);
                });
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// Dragging the sidebar's handle with a render landing mid-gesture
    /// must leave the divider where the pointer put it.
    ///
    /// Regression: the recorded width only catches up a tick after the
    /// drag, and the render path used to "correct" a panel back to that
    /// record — so any render mid-drag (a stream frame, the clock, the
    /// poll) yanked the divider back and the splitter looked
    /// undraggable.
    #[test]
    fn a_dragged_splitter_is_not_reverted_by_a_render() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("dragged_splitter_survives_render"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_app_identity("dev.just.ddu", "Day Day Up");
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "/bin/cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(LoadWarnings(vec![]));
                    cx.set_global(State {
                        projects: Some(vec![ProjectConfig {
                            name: "proj".into(),
                            path: std::env::temp_dir(),
                            expanded: true,
                            sessions: vec![],
                        }]),
                        ..Default::default()
                    });
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|window, cx| {
                    view.update(cx, |v, _| v.show_sessions = true);
                    // Two frames: the splitter states measure their slots
                    // on the first and honor them on the second.
                    let _ = window.draw(cx);
                    let _ = window.draw(cx);
                });
                // Grab the divider where it actually is.
                let boundary = vcx.update(|_, cx| {
                    let sizes = view.read(cx).shell_state.read(cx).sizes().clone();
                    sizes[0]
                });
                vcx.simulate_mouse_down(
                    gpui_kit::point(boundary - gpui_kit::px(1.), gpui_kit::px(50.)),
                    gpui_kit::MouseButton::Left,
                    gpui_kit::Modifiers::default(),
                );
                let dragged = boundary + gpui_kit::px(40.);
                // Two moves: the first starts the drag (gpui's threshold),
                // the second is the one that resizes.
                vcx.simulate_mouse_move(
                    gpui_kit::point(boundary + gpui_kit::px(10.), gpui_kit::px(50.)),
                    Some(gpui_kit::MouseButton::Left),
                    gpui_kit::Modifiers::default(),
                );
                vcx.simulate_mouse_move(
                    gpui_kit::point(dragged, gpui_kit::px(50.)),
                    Some(gpui_kit::MouseButton::Left),
                    gpui_kit::Modifiers::default(),
                );
                // A render while the button is still down (the executor
                // is not pumped, so the record has not caught up).
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let mid = vcx.update(|_, cx| view.read(cx).shell_state.read(cx).sizes()[0]);
                assert_eq!(mid, dragged, "the drag survives a mid-gesture frame");
                vcx.simulate_mouse_up(
                    gpui_kit::point(dragged, gpui_kit::px(50.)),
                    gpui_kit::MouseButton::Left,
                    gpui_kit::Modifiers::default(),
                );
                let after = vcx.update(|_, cx| view.read(cx).shell_state.read(cx).sizes()[0]);
                assert_eq!(after, dragged, "and the release leaves it there");
                // The same drag after a window resize: the recorded pair
                // has to be re-measured against the new container, or the
                // stale sibling width makes the splitter's arithmetic cap
                // the sidebar short of the pointer.
                vcx.simulate_resize(gpui_kit::size(gpui_kit::px(760.), gpui_kit::px(800.)));
                vcx.update(|window, cx| {
                    view.update(cx, |_, cx| cx.notify());
                    let _ = window.draw(cx);
                });
                let boundary = vcx.update(|_, cx| view.read(cx).shell_state.read(cx).sizes()[0]);
                let dragged = boundary - gpui_kit::px(30.);
                vcx.simulate_mouse_down(
                    gpui_kit::point(boundary - gpui_kit::px(1.), gpui_kit::px(50.)),
                    gpui_kit::MouseButton::Left,
                    gpui_kit::Modifiers::default(),
                );
                vcx.simulate_mouse_move(
                    gpui_kit::point(boundary - gpui_kit::px(10.), gpui_kit::px(50.)),
                    Some(gpui_kit::MouseButton::Left),
                    gpui_kit::Modifiers::default(),
                );
                vcx.simulate_mouse_move(
                    gpui_kit::point(dragged, gpui_kit::px(50.)),
                    Some(gpui_kit::MouseButton::Left),
                    gpui_kit::Modifiers::default(),
                );
                let after = vcx.update(|_, cx| view.read(cx).shell_state.read(cx).sizes()[0]);
                assert_eq!(after, dragged, "and it still tracks after a resize");
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
