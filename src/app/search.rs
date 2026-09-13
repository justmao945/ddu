//! The two search bars: the right pane's find bar (⌘F over the selected
//! file's rows) and the sidebar's quick open (⌘P over the working tree's
//! paths). Each owns its input entity, its match list and the position
//! the counter shows; the handlers are driven by the input's own events
//! and by the keys bound in [`super::keys`].

use gpui_kit::component::input::InputState;

use super::*;

/// How many quick-open hits are kept: past this the list is a scroll
/// rather than an answer, and the user is expected to type one more
/// character.
pub(crate) const FILE_SEARCH_MAX: usize = 200;
/// State behind the diff pane's find bar (⌘F). `open` folds the bar's
/// whole lifecycle: closed → hidden, matches stay empty.
pub(crate) struct DiffSearch {
    pub input: Entity<InputState>,
    pub open: bool,
    /// Child-row indices into the pane's scroll container (see
    /// [`crate::diff::match_rows`]); one entry per occurrence, so the
    /// highlight set dedupes while the counter counts.
    pub matches: Vec<usize>,
    /// Index into `matches` the counter shows and the scroll targets.
    pub current: usize,
}
impl DiffSearch {
    pub fn new(window: &mut Window, cx: &mut Context<AppView>) -> Self {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Find in diff", window, cx);
            input
        });
        Self {
            input,
            open: false,
            matches: Vec::new(),
            current: 0,
        }
    }
}
/// The file tree's quick open (⌘P): the input, the working tree's
/// searchable paths, and the ranked matches against the input's value.
/// `open` folds the bar's whole lifecycle, as it does for the find bar:
/// closed → not rendered, matches empty.
pub(crate) struct FileSearch {
    pub input: Entity<InputState>,
    pub open: bool,
    /// The hit list's scroll position: a palette is narrow and shows a
    /// window of the hits, so the cursor is what scrolls it. A virtual
    /// list's handle, because that is what the hits are — the list is
    /// capped (`FILE_SEARCH_MAX`), not short.
    pub scroll: VirtualListScrollHandle,
    /// Every path the search can reach: the index's tracked files plus
    /// whatever the poll found untracked — the same universe the tree
    /// lists, and no walk to get it (the index is already in memory).
    /// Built when the bar opens.
    pub paths: Vec<String>,
    /// Indices into `paths`, best match first.
    pub matches: Vec<usize>,
    /// Index into `matches`: what Enter opens, what the counter shows.
    pub current: usize,
}
impl FileSearch {
    pub fn new(window: &mut Window, cx: &mut Context<AppView>) -> Self {
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_placeholder("Find file", window, cx);
            input
        });
        Self {
            input,
            open: false,
            scroll: VirtualListScrollHandle::new(),
            paths: Vec::new(),
            matches: Vec::new(),
            current: 0,
        }
    }

    /// The path under the cursor, if the query has an answer.
    pub fn current_path(&self) -> Option<&str> {
        self.paths.get(*self.matches.get(self.current)?).map(String::as_str)
    }
}

impl AppView {
    /// Open the quick open (or refocus it when already open). The
    /// palette floats over the workspace, so the panels stay as they
    /// are — nothing is revealed, and nothing is hidden. The whole query
    /// is selected, so typing replaces it.
    pub(crate) fn open_file_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // No session, no working tree to search.
        if self.current_session().is_none() {
            return;
        }
        self.file_search.open = true;
        self.file_search.scroll.set_offset(point(px(0.), px(0.)));
        self.refresh_file_paths();
        self.file_search.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.refresh_file_search(cx);
    }

    pub(crate) fn close_file_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.file_search.open = false;
        self.file_search.matches.clear();
        self.file_search.current = 0;
        // Hand focus back to the window fallback: the strip unmounts
        // with the bar, and a handle left on a removed input would keep
        // eating keystrokes.
        self.window_focus.focus(window, cx);
        cx.notify();
    }

    /// Rebuild the searchable path list from the session's working tree:
    /// the index (tracked files) plus the poll's untracked ones. Sorted
    /// and deduplicated; a non-UTF-8 path is left out rather than
    /// lossily compared.
    pub(crate) fn refresh_file_paths(&mut self) {
        let mut paths: Vec<String> = Vec::new();
        if let Some(root) = self.current_session_cwd() {
            if let Some(repo) = self.repo_for(&root) {
                if let Ok(index) = repo.index() {
                    paths.extend(
                        index
                            .iter()
                            .filter_map(|entry| String::from_utf8(entry.path.clone()).ok()),
                    );
                }
            }
        }
        if let Some(diff) = self.diff() {
            paths.extend(diff.files.iter().map(|file| file.path.clone()));
        }
        paths.sort_unstable();
        paths.dedup();
        self.file_search.paths = paths;
    }

    /// Re-rank the matches against the input's current value. Called on
    /// keystrokes and when the bar opens; the list re-ranks per
    /// keystroke, so the cursor goes back to the best match.
    pub(crate) fn refresh_file_search(&mut self, cx: &mut Context<Self>) {
        let query = self.file_search.input.read(cx).value().to_string();
        self.file_search.matches =
            crate::diff::listing::search(&self.file_search.paths, &query, FILE_SEARCH_MAX);
        self.file_search.current = 0;
        // A new ranking is read from its top: keeping the old offset
        // would open the list part-way down an answer the user has not
        // seen yet.
        self.file_search.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    /// Step through the hits, wrapping at both ends.
    pub(crate) fn file_search_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let len = self.file_search.matches.len();
        if len == 0 {
            return;
        }
        self.file_search.current = if back {
            (self.file_search.current + len - 1) % len
        } else {
            (self.file_search.current + 1) % len
        };
        // The cursor is the way the list scrolls: the palette shows a
        // window of the hits, and stepping past its edge has to bring
        // the next row into it.
        self.file_search
            .scroll
            .scroll_to_item(self.file_search.current, ScrollStrategy::Nearest);
        cx.notify();
    }

    /// Open the match under the cursor: the tree opens the path's
    /// ancestors so the file is where the search left it, the pane takes
    /// the selection, and the bar closes.
    pub(crate) fn commit_file_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.file_search.current_path().map(str::to_owned) else {
            return;
        };
        let mut rest = path.as_str();
        while let Some(cut) = rest.rfind('/') {
            rest = &rest[..cut];
            self.diff_tree_open.insert(rest.to_owned());
        }
        self.select_path(path);
        self.rebuild_tree_index();
        self.ensure_file_content(cx);
        self.refresh_diff_search(cx);
        self.close_file_search(window, cx);
        if !self.show_diff {
            self.set_diff(true, window, cx);
        }
    }

    /// Open the find bar (or refocus it when already open); the whole
    /// query is selected so typing replaces it.
    pub(crate) fn open_diff_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A rendered document has no row list to match against: ⌘F over
        // one leaves File mode for the hunks, which are rows.
        if self.surface() == crate::ui::diff_panel::Surface::Preview {
            self.set_view_mode(ViewMode::Diff, cx);
        }
        self.diff_search.open = true;
        self.diff_search.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.refresh_diff_search(cx);
        cx.notify();
    }

    pub(crate) fn close_diff_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.diff_search.open = false;
        self.diff_search.matches.clear();
        self.diff_search.current = 0;
        // Hand focus back to the window fallback so global shortcuts
        // keep a dispatch path and the terminal cursor goes hollow.
        self.window_focus.focus(window, cx);
        cx.notify();
    }

    /// Step through matches, wrapping at both ends. `back` walks
    /// Shift-Enter / ⌘⇧G direction.
    pub(crate) fn diff_search_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let len = self.diff_search.matches.len();
        if len == 0 {
            return;
        }
        self.diff_search.current = if back {
            (self.diff_search.current + len - 1) % len
        } else {
            (self.diff_search.current + 1) % len
        };
        self.diff_search_jump(cx);
    }

    /// Recompute matches from the input's current value against the
    /// selected file. Called on keystrokes, diff refreshes and file
    /// switches; with `jump`, the first match is scrolled into view
    /// (query edits), otherwise the current index is kept clamped.
    pub(crate) fn refresh_diff_search(&mut self, cx: &mut Context<Self>) {
        self.diff_search.matches.clear();
        if self.diff_search.open {
            // The match list is a list of the *rendered* stream's row
            // indices, so it is rebuilt per mode — markers and notes
            // included (both streams number their rows the same way).
            let query = self.diff_search.input.read(cx).value().to_string();
            if let (Some(stream), _) = crate::ui::diff_panel::pane_rows(self) {
                self.diff_search.matches = stream.match_indices(&query);
            }
        }
        self.diff_search.current = self
            .diff_search
            .current
            .min(self.diff_search.matches.len().saturating_sub(1));
        cx.notify();
    }

    /// Point the scroll container at the current match's row.
    pub(crate) fn diff_search_jump(&mut self, cx: &mut Context<Self>) {
        if let Some(&row) = self.diff_search.matches.get(self.diff_search.current) {
            self.diff_hunks_scroll
                .scroll_to_item(row, ScrollStrategy::Nearest);
        }
        cx.notify();
    }
}
