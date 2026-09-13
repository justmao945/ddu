//! Diff data flow: the periodic working-tree poll, stale-result
//! guarding and selection re-pinning.

use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::input::InputState;

use super::*;
use crate::diff::git;

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
            crate::diff::tree::search(&self.file_search.paths, &query, FILE_SEARCH_MAX);
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

    /// Remember where the file being left was scrolled, and put the
    /// newly selected one back where it was left. Both are keyed by
    /// `(working tree, path, mode)`: a file reads the same way in every
    /// session of a project, and switching files is the thing this
    /// makes cheap — a re-rendered stream puts a file back at the top
    /// unless its own position is remembered.
    fn remember_scroll(&mut self) {
        // A restore still waiting for its rows means the offset on the
        // handle belongs to the *previous* file (or to a clamped
        // fallback): the remembered value is the truth, so leave it.
        if self
            .pending_scroll
            .as_ref()
            .is_some_and(|(path, mode, _)| {
                Some(path.as_str()) == self.current_diff_path() && *mode == self.view_mode
            })
        {
            return;
        }
        let (Some(root), Some(path)) = (self.current_session_cwd(), self.current_diff_path()) else {
            return;
        };
        let key = (root, path.to_owned(), self.view_mode);
        let offset = self.diff_hunks_scroll.offset();
        // The top is the default anyway: keeping it would only grow the
        // map.
        if offset == point(px(0.), px(0.)) {
            self.file_positions.remove(&key);
        } else {
            self.file_positions.insert(key, offset);
        }
    }

    /// Where `path` was left in the current working tree and mode — the
    /// top-left corner of its rows, or the top when it was never
    /// scrolled. One small allocation per file switch buys the map a
    /// borrowable key; nothing here runs per frame.
    fn saved_scroll(&self, path: &str) -> Point<Pixels> {
        let top = point(px(0.), px(0.));
        let Some(root) = self.current_session_cwd() else {
            return top;
        };
        self.file_positions
            .get(&(root, path.to_owned(), self.view_mode))
            .copied()
            .unwrap_or(top)
    }

    /// Put the selection's remembered position back — or defer it until
    /// the rows it belongs to are on screen.
    ///
    /// The deferral is the whole point: the rows mount in stages (a
    /// changed file first shows the poll's hunks as a fallback, then its
    /// whole-file view lands a moment later), and the virtual list clamps
    /// an offset to whatever content is *currently* mounted. Applied too
    /// early, a deep position is clamped against the fallback's few rows
    /// and the clamp outlives the content that caused it — the file then
    /// opens nowhere near where it was left.
    fn restore_scroll(&mut self) {
        self.pending_scroll = None;
        let Some(path) = self.current_diff_path().map(str::to_owned) else {
            return;
        };
        let offset = self.saved_scroll(&path);
        if offset == point(px(0.), px(0.)) {
            self.diff_hunks_scroll.set_offset(offset);
        } else if self.stream_is_final() {
            self.diff_hunks_scroll.set_offset(offset);
        } else {
            self.pending_scroll = Some((path, self.view_mode, offset));
        }
    }

    /// Whether the pane is showing the rows a remembered position was
    /// measured against: Diff mode reads its rows straight off the poll,
    /// File mode needs the file's own view (the hunks it falls back to
    /// meanwhile are a different row list). A rendered document and an
    /// image have no rows of their own — the pane's scroll is not theirs.
    fn stream_is_final(&self) -> bool {
        match self.surface() {
            crate::ui::diff_panel::Surface::Diff => self.diff().is_some(),
            crate::ui::diff_panel::Surface::File => matches!(
                self.cached_file_view(),
                Some(crate::diff::view::FileView::Text(_))
            ),
            crate::ui::diff_panel::Surface::Preview => true,
        }
    }

    /// Apply a deferred restore once its rows are the ones on screen.
    fn apply_pending_scroll(&mut self) {
        let Some((path, mode, offset)) = self.pending_scroll.clone() else {
            return;
        };
        if self.current_diff_path() != Some(path.as_str()) || self.view_mode != mode {
            self.pending_scroll = None;
            return;
        }
        if self.stream_is_final() {
            self.diff_hunks_scroll.set_offset(offset);
            self.pending_scroll = None;
        }
    }

    pub(super) fn reset_diff(&mut self) {
        self.snapshot = None;
        self.diff_error = None;
        self.selection = None;
        self.tree_seeded = false;
        self.diff_limits.clear();
        self.file_view = None;
        self.file_view_key = None;
        self.preview = None;
        self.tree_index = None;
        self.diff_tree_open.clear();
        self.diff_search.matches.clear();
        self.pending_scroll = None;
        // The quick open's path list belongs to the working tree the
        // session that owned it; the next open rebuilds it.
        self.file_search.paths.clear();
        self.file_search.matches.clear();
        self.file_search.current = 0;
        self.diff_tree_scroll.set_offset(point(px(0.), px(0.)));
        self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
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

    /// Grow a truncated file's line budget ×4 and reload at once — the
    /// pane's infinite-scroll step, kicked when the rendered range
    /// reaches the cap note. No-op at the hard ceiling: the note stays.
    pub(crate) fn expand_diff_limit(&mut self, path: &str, cx: &mut Context<Self>) {
        let cur = self
            .diff_limits
            .get(path)
            .copied()
            .unwrap_or(crate::diff::MAX_LINES_PER_FILE);
        if cur >= crate::diff::EXPAND_MAX_LINES {
            return;
        }
        self.diff_limits
            .insert(path.to_owned(), (cur * 4).min(crate::diff::EXPAND_MAX_LINES));
        self.reload_diff(cx);
    }

    /// Apply one poll. Returns whether anything moved — an **idle** poll
    /// must not repaint: a 3 s tick that re-rendered the pane and rebuilt
    /// the row caches produced a visible flash over whatever the user was
    /// reading, for a diff that had not changed at all.
    pub(super) fn apply_snapshot(
        &mut self,
        result: anyhow::Result<crate::diff::Snapshot>,
        cx: &mut Context<Self>,
    ) -> bool {
        // The live selection's path: `None` stays `None` (no click =
        // empty pane — the poll never auto-selects a file).
        let selected = self.selection.as_ref().map(|s| s.path.clone());
        // First paint after a cold start: the persisted selection wins
        // when the path still exists in the working tree.
        let seed = if self.diff_seed_path.is_some() && self.snapshot.is_none() {
            self.diff_seed_path.clone()
        } else {
            None
        };
        let selected = selected.or(seed);
        let moved = match result {
            Ok(snapshot) => {
                // The selection is a path in the *tree*, so a clean file
                // stays selected across polls; only a path that left the
                // working tree clears the pane.
                // The selection is a path in the tree: a file on disk,
                // or one the diff still knows (a file deleted in the
                // workdir stays readable through its hunks).
                let root = self.current_session_cwd();
                let next = selected
                    .as_ref()
                    .filter(|path| {
                        snapshot.diff.files.iter().any(|f| f.path == path.as_str())
                            || root
                                .as_ref()
                                .is_some_and(|root| root.join(path).is_file())
                    })
                    .map(|path| crate::app::Selection {
                        path: path.clone(),
                        changed: snapshot.diff.files.iter().position(|f| &f.path == path),
                    });
                let moved = self.snapshot.as_ref() != Some(&snapshot) || next != self.selection;
                // The selection moved on its own — a restored row's path
                // landing on the first poll, or the path leaving the
                // working tree. Same bookkeeping as a click: remember
                // where the outgoing file was, put the incoming one back
                // where it was left.
                if next.as_ref().map(|s| s.path.as_str()) != selected.as_deref() {
                    self.remember_scroll();
                    let restored = next.as_ref().map(|s| self.saved_scroll(&s.path));
                    if let Some(offset) = restored {
                        self.diff_hunks_scroll.set_offset(offset);
                    }
                }
                self.selection = next;
                self.snapshot = Some(snapshot);
                let had_error = self.diff_error.take().is_some();
                moved || had_error
            }
            Err(err) => {
                let text = err.to_string();
                let moved = self.snapshot.take().is_some() || self.diff_error.as_deref() != Some(&text);
                self.diff_error = Some(text);
                moved
            }
        };
        if !moved {
            // Nothing changed: leave the caches, the row list and the
            // frame exactly as they are.
            self.diff_seed_path = None;
            return false;
        }
        // A newer snapshot invalidates the whole-file view and the
        // preview source (they were merged against the previous one);
        // the path check drops a build for a file that is no longer
        // selected.
        self.diff_gen += 1;
        let current = self.current_diff_path().map(str::to_owned);
        if self
            .file_view_key
            .as_ref()
            .is_some_and(|(p, _)| Some(p.as_str()) != current.as_deref())
        {
            self.file_view = None;
            self.file_view_key = None;
        }
        if self
            .preview
            .as_ref()
            .is_some_and(|b| Some(b.key.0.as_str()) != current.as_deref())
        {
            self.preview = None;
        }
        self.diff_seed_path = None;
        self.rebuild_tree_index();
        // The 3s poll can rewrite the open file under an active
        // search: keep matches and counter truthful.
        self.refresh_diff_search(cx);
        self.ensure_file_content(cx);
        self.apply_pending_scroll();
        true
    }

    /// Rebuild the sidebar tree's rows (a new snapshot, a directory
    /// toggle). The listing is lazy — the root plus every *expanded*
    /// directory, nothing else — so this costs the visible tree, not the
    /// repository (`FILE_TREE.md` §4.1). The first snapshot of a session
    /// also opens the changes' ancestors (`seed_open`).
    pub(crate) fn rebuild_tree_index(&mut self) {
        let root = self.current_session_cwd();
        let Some(root) = root else {
            self.tree_index = None;
            return;
        };
        let Some(repo) = self.repo_for(&root) else {
            self.tree_index = None;
            return;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.tree_index = None;
            return;
        };
        let changes = crate::diff::tree::Changes::of(&snapshot.diff.files);
        if !self.tree_seeded {
            self.tree_seeded = true;
            crate::ui::diff_tree::seed_open(&mut self.diff_tree_open, &changes);
        }
        // Field-level borrows: the listing closure reads the repo and the
        // diff while the rows land in `tree_index`.
        let open = &self.diff_tree_open;
        let index = crate::ui::diff_tree::build_index(open, |dir| {
            crate::diff::tree::list_dir(&repo, dir, &changes)
        });
        self.tree_index = Some(index);
    }

    /// The project's repository handle, discovered once per directory and
    /// kept for the tree's listings. `None` when the path is not in a
    /// repository — the tree then shows the poll's error, as before.
    pub(crate) fn repo_for(&mut self, root: &std::path::Path) -> Option<Rc<git2::Repository>> {
        if let Some((key, repo)) = &self.repo {
            if key == root {
                return Some(repo.clone());
            }
        }
        let repo = Rc::new(git2::Repository::discover(root).ok()?);
        self.repo = Some((root.to_path_buf(), repo.clone()));
        Some(repo)
    }

    /// The project's diff, if a poll has landed.
    pub(crate) fn diff(&self) -> Option<&crate::diff::GitDiff> {
        self.snapshot.as_ref().map(|s| &s.diff)
    }

    /// The selection's diff record, when the selected file has one.
    pub(crate) fn selected_diff_file(&self) -> Option<&crate::diff::DiffFile> {
        let changed = self.selection.as_ref()?.changed?;
        self.diff()?.files.get(changed)
    }

    /// The rows a mode falls back to when its own view is not ready yet:
    /// the merged whole-file view when it is cached (a clean file has
    /// nothing else), otherwise the diff's hunks if the file has any.
    pub(crate) fn file_rows<'a>(
        &'a self,
        file: Option<&'a crate::diff::DiffFile>,
    ) -> Option<crate::diff::RowStream<'a>> {
        match (self.cached_file_view(), file) {
            (Some(crate::diff::view::FileView::Text(view)), _) => {
                Some(crate::diff::RowStream::view(view))
            }
            (_, Some(file)) => Some(crate::diff::RowStream::diff(file)),
            (_, None) => None,
        }
    }

    /// The selected file's repo-relative path, if one is selected.
    pub(crate) fn current_diff_path(&self) -> Option<&str> {
        self.selection.as_ref().map(|s| s.path.as_str())
    }

    /// Select a path (a tree row click): re-pin the diff index, and open
    /// the pane if it is closed.
    pub(crate) fn select_path(&mut self, path: String) {
        let changed = self
            .diff()
            .and_then(|d| d.files.iter().position(|f| f.path == path));
        let same = self.selection.as_ref().is_some_and(|s| s.path == path);
        if !same {
            self.remember_scroll();
        }
        self.selection = Some(crate::app::Selection { path, changed });
        if !same {
            self.restore_scroll();
        }
    }

    /// What the pane renders right now: the mode, and the Markdown
    /// question answered by the file itself (`docs/FILE_TREE.md` §4.2).
    pub(crate) fn surface(&self) -> crate::ui::diff_panel::Surface {
        // A file nobody changed has nothing to diff: Diff mode shows the
        // file itself (the rendered document, for Markdown) rather than
        // the empty hunks pane that would hide it.
        let mode = if self.view_mode == ViewMode::Diff && self.selected_diff_file().is_none() {
            ViewMode::File
        } else {
            self.view_mode
        };
        crate::ui::diff_panel::surface_of(mode, self.current_diff_path())
    }

    /// The directory the selected file lives in — what a Markdown
    /// document's relative image URLs resolve against.
    pub(crate) fn preview_base(&self) -> std::path::PathBuf {
        let root = self.diff_root();
        match self.current_diff_path() {
            Some(path) => root.join(path).parent().map(|p| p.to_path_buf()).unwrap_or(root),
            None => root,
        }
    }

    /// The working tree the diff's paths are relative to.
    pub(crate) fn diff_root(&self) -> std::path::PathBuf {
        self.current_session_cwd().unwrap_or_default()
    }

    /// Whether a cached build (or in-flight refusal) is still about the
    /// selected file, and still current.
    fn preview_is_current(&self, build: &crate::diff::view::PreviewBuild) -> bool {
        self.current_diff_path() == Some(build.key.0.as_str()) && build.key.1 <= self.diff_gen
    }

    /// The cached whole-file view for the selected file. A view built for
    /// an *older* generation still renders: its replacement is being built
    /// off-thread and blanking the pane in the meantime is the flash the
    /// poll used to cause.
    pub(crate) fn cached_file_view(&self) -> Option<&crate::diff::view::FileView> {
        let path = self.current_diff_path()?;
        let (cached, generation) = self.file_view_key.as_ref()?;
        (cached == path && *generation <= self.diff_gen)
            .then(|| self.file_view.as_deref())
            .flatten()
    }

    /// The rendered document's Markdown source, same keying
    /// as [`Self::cached_file_view`]. `None` while it is still being
    /// read — or forever, when [`Self::preview_refusal`] holds the
    /// reason.
    pub(crate) fn cached_preview(&self) -> Option<&str> {
        let build = self.preview.as_ref().filter(|b| self.preview_is_current(b))?;
        let Ok(text) = &build.source else {
            return None;
        };
        Some(text.as_ref())
    }

    /// Why the rendered document cannot show the selected file (too large, binary, or
    /// gone): the pane bands this above the rows it falls back to,
    /// exactly as File mode does.
    pub(crate) fn preview_refusal(&self) -> Option<crate::diff::view::Unreadable> {
        let build = self.preview.as_ref().filter(|b| self.preview_is_current(b))?;
        build.source.as_ref().err().copied()
    }

    /// Switch the pane's surface. Per session: persisted with the row.
    pub(crate) fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        if self.current_diff_path().is_none() || self.view_mode == mode {
            return;
        }
        // The stream's shape changes with the mode, so each mode keeps
        // its own position: the hunks' place and the whole file's place
        // are both worth coming back to.
        self.remember_scroll();
        self.view_mode = mode;
        self.restore_scroll();
        self.ensure_file_content(cx);
        self.refresh_diff_search(cx);
        self.persist(cx);
        cx.notify();
    }

    /// ⌘⇧M and the header's button: the hunks ⇄ the whole file (which a
    /// Markdown file renders).
    pub(crate) fn toggle_view_mode(&mut self, cx: &mut Context<Self>) {
        let next = self.view_mode.next();
        self.set_view_mode(next, cx);
    }

    /// Start the background read behind the whole-file surface when the cache
    /// no longer matches the selection. An 8 MiB file is milliseconds of
    /// IO plus a full line split, so it never runs on the UI thread; the
    /// pane shows the previous content (or a "Reading the file…" band)
    /// until it lands.
    pub(crate) fn ensure_file_content(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.current_session_cwd() else {
            return;
        };
        let Some(path) = self.current_diff_path().map(str::to_owned) else {
            return;
        };
        // A clean file has no diff record: the merged view degrades to
        // its own lines untinted, which is exactly what File mode should
        // show for a file nobody changed.
        let file = self.selected_diff_file().cloned();
        let generation = self.diff_gen;
        let key = (path.clone(), generation);
        // Keyed on the *surface*, not the mode: a file nobody changed
        // renders through File mode whatever the mode says (see
        // `surface`), and an image needs its view built for the same
        // reason a text file does.
        use crate::ui::diff_panel::Surface;
        let surface = self.surface();
        let want_view =
            surface == Surface::File && self.file_view_key.as_ref() != Some(&key);
        // A rendered document needs both: the document is what shows, the
        // rows are what a refused source falls back to.
        let want_preview =
            surface == Surface::Preview && self.preview.as_ref().map(|b| &b.key) != Some(&key);
        if !want_view && !want_preview {
            return;
        }
        let this = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            // The job gets its own handles: the update below still needs
            // `path` to tell whether this build is still the right one.
            let (job_root, job_path) = (root.clone(), path.clone());
            let built = cx
                .background_spawn(async move {
                    let view = want_view.then(|| {
                        crate::diff::view::FileView::build(&job_root, &job_path, file.as_ref())
                    });
                    let source = want_preview
                        .then(|| crate::diff::view::read_source(&job_root, &job_path));
                    (view, source)
                })
                .await;
            let (view, source) = built;
            let _ = this.update(cx, |v, cx| {
                if v.diff_gen != generation || v.current_diff_path() != Some(path.as_str()) {
                    return;
                }
                if let Some(view) = view {
                    v.file_view = Some(std::rc::Rc::new(view));
                    v.file_view_key = Some((path.clone(), generation));
                }
                if let Some(source) = source {
                    v.preview = Some(crate::diff::view::PreviewBuild {
                        key: (path.clone(), generation),
                        source: source.map(|text| std::rc::Rc::from(text.as_str())),
                    });
                }
                // The rows this file's remembered position belongs to are
                // here now: put the pane back where it was left.
                v.apply_pending_scroll();
                v.refresh_diff_search(cx);
                cx.notify();
            });
            anyhow::Ok(())
        })
        .detach();
    }

    /// Kick off one diff reload; results newer than any in-flight one
    /// win. No active session → no diff at all (the panels show their
    /// "no session" note; a stale project diff must not linger).
    pub(crate) fn reload_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_seq += 1;
        let seq = self.diff_seq;
        let Some(path) = self.current_session_cwd() else {
            self.snapshot = None;
            self.diff_error = None;
            return;
        };
        let limits = self.diff_limits.clone();
        let this = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_spawn(async move { git::snapshot(&path, &limits) })
                .await;
            if let Err(e) = &result {
                eprintln!("[ddu] diff err: {e:#}");
            }
            let _ = this.update(cx, |v, cx| {
                if v.diff_seq == seq && v.apply_snapshot(result, cx) {
                    cx.notify();
                }
            });
            anyhow::Ok(())
        })
        .detach();
    }

    /// Periodic working-tree poll so the diff panel stays fresh (M3
    /// acceptance: edits show up within 3s).
    pub(super) fn start_diff_poll(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(DIFF_POLL_SECS))
                    .await;
                let Some(view) = this.upgrade() else { break };
                let (path, seq) = view.read_with(cx, |v, _| (v.current_session_cwd(), v.diff_seq));
                // No active session this tick: nothing to poll.
                let Some(path) = path else { continue };
                let limits = view.read_with(cx, |v, _| v.diff_limits.clone());
                let result = cx
                    .background_spawn(async move { git::snapshot(&path, &limits) })
                    .await;
                this.update(cx, |v, cx| {
                    if v.diff_seq == seq && v.apply_snapshot(result, cx) {
                        cx.notify();
                    }
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    // Named imports, never `use super::*`: the parent module globs
    // gpui-kit, and a glob of *it* brings gpui's own `#[test]` attribute
    // into scope, which expands into itself (rustc: "recursion limit
    // reached while expanding `#[test]`"). Every test module in `app`
    // is written this way for that reason.
    use crate::app::AppView;
    use crate::config::{Config, LoadWarnings, ProjectConfig, ShellConfig, State};
    use crate::diff::{DiffFile, DiffHunk, DiffLine, GitDiff, Snapshot};
    use crate::ui::diff_panel::ViewMode;
    use gpui_kit::{Entity, TestAppContext, VisualTestContext, gpui, point, px};

    /// A working tree on disk (the listing is the filesystem's) plus the
    /// changed-file records the poll would have produced for it: one
    /// changed file, one clean, one untracked.
    fn workspace(name: &str) -> (std::path::PathBuf, Vec<DiffFile>) {
        let root = std::env::temp_dir().join(format!("ddu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("mkdir");
        std::fs::write(root.join("README.md"), "# hi\n").expect("write");
        std::fs::write(root.join("src/app.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("src/other.rs"), "fn other() {}\n").expect("write");
        // Untracked: not in the index, and the poll is what knows it.
        std::fs::write(root.join("fresh.txt"), "new\n").expect("write");
        let repo = git2::Repository::init(&root).expect("init");
        let mut index = repo.index().expect("index");
        index.add_path(std::path::Path::new("README.md")).expect("add");
        index.add_path(std::path::Path::new("src/app.rs")).expect("add");
        index.add_path(std::path::Path::new("src/other.rs")).expect("add");
        index.write().expect("write index");
        drop(index);

        let changed = vec![DiffFile {
            path: "src/app.rs".to_owned(),
            added: 1,
            removed: 0,
            hunks: vec![DiffHunk {
                header: "@@ -1 +1,2 @@".into(),
                lines: vec![
                    DiffLine {
                        kind: ' ',
                        old_no: Some(1),
                        new_no: Some(1),
                        text: "fn main() {}".into(),
                    },
                    DiffLine {
                        kind: '+',
                        old_no: None,
                        new_no: Some(2),
                        text: "// changed".into(),
                    },
                ],
            }],
            lines_total: 1,
            truncated: false,
        },
        // The untracked file the poll reports: no hunks (there is no
        // HEAD side to diff against) and one added line.
        DiffFile {
            path: "fresh.txt".to_owned(),
            added: 1,
            removed: 0,
            hunks: Vec::new(),
            lines_total: 0,
            truncated: false,
        }];
        (root, changed)
    }

    /// One AppView on that working tree, with the poll's snapshot applied
    /// and a frame drawn.
    fn app(
        dispatcher: gpui::TestDispatcher,
        name: &'static str,
        cwd: std::path::PathBuf,
        changed: Vec<DiffFile>,
    ) -> (TestAppContext, Entity<AppView>, VisualTestContext) {
        let mut cx0 = TestAppContext::build(dispatcher, Some(name));
        let cx = &mut cx0;
        cx.update(gpui_kit::init);
        cx.update(|cx| {
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
                    path: cwd.clone(),
                    expanded: true,
                    sessions: vec![],
                }]),
                ..Default::default()
            });
        });
        let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
        vcx.update(|window, cx| {
            view.update(cx, |v, cx| {
                v.show_sessions = true;
                v.show_diff_tree = true;
                v.projects[0].path = cwd.clone();
                v.projects[0].sessions[0].cwd = cwd.clone();
                v.snapshot = Some(Snapshot {
                    diff: GitDiff {
                        branch: None,
                        files: changed.clone(),
                    },
                });
                v.tree_seeded = false;
                v.diff_tree_open.clear();
                v.rebuild_tree_index();
                cx.notify();
            });
            let _ = window.draw(cx);
        });
        // The borrow `add_window_view` hands back ends with this
        // function: the context is cloned out so a test can drive the
        // window (focus, hover) while it also pumps `cx0`.
        let vcx = vcx.clone();
        (cx0, view, vcx)
    }

    /// The quick open searches the working tree's own paths — the index's
    /// tracked files *and* the untracked ones only the poll knows — and
    /// committing a hit selects the file with its ancestors opened in the
    /// tree, so the row is where the search left it.
    #[test]
    fn quick_open_searches_the_working_tree_and_opens_the_hit() {
        let (root, changed) = workspace("quick-open");
        let root_for_app = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let (mut cx0, view, mut vcx) =
                    app(dispatcher, "quick_open", root_for_app, changed);
                let cx = &mut cx0;
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.open_file_search(window, cx);
                        let paths = &v.file_search.paths;
                        assert!(paths.iter().any(|p| p == "src/other.rs"), "tracked");
                        assert!(paths.iter().any(|p| p == "fresh.txt"), "untracked");
                    });
                });
                // Typed, not assigned: `InputState::set_value` suppresses
                // the change event, and the point of this test is that the
                // bar re-ranks on every keystroke.
                vcx.simulate_input("app.rs");
                cx.run_until_parked();
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        assert_eq!(
                            v.file_search.current_path(),
                            Some("src/app.rs"),
                            "typing re-ranks against the working tree"
                        );
                        v.commit_file_search(window, cx);
                        assert_eq!(v.current_diff_path(), Some("src/app.rs"));
                        assert!(
                            v.diff_tree_open.contains("src"),
                            "the hit's ancestors open in the tree"
                        );
                        assert!(!v.file_search.open, "the bar closes on the pick");
                    });
                });
                // The keys a palette is actually used with: step the
                // cursor with the arrows and open what it landed on. The
                // field is a single-line input, so `down` is nothing to
                // it and the palette's own binding is what lands (see
                // `app::tests::the_palette_steps_with_the_arrows_the_field_gives_up`).
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| v.open_file_search(window, cx));
                });
                vcx.simulate_input("rs");
                view.update(cx, |v, _| {
                    assert!(
                        v.file_search.matches.len() >= 2,
                        "the arrows need somewhere to go"
                    );
                });
                vcx.simulate_keystrokes("down");
                view.update(cx, |v, _| assert_eq!(v.file_search.current, 1, "down steps"));
                vcx.simulate_keystrokes("up");
                view.update(cx, |v, _| assert_eq!(v.file_search.current, 0, "up steps back"));
                vcx.simulate_keystrokes("down");
                let cursor = view.update(cx, |v, _| {
                    v.file_search.current_path().map(str::to_owned)
                });
                vcx.simulate_keystrokes("enter");
                view.update(cx, |v, _| {
                    assert!(!v.file_search.open, "enter closes the palette");
                    assert_eq!(
                        v.current_diff_path().map(str::to_owned),
                        cursor,
                        "enter opens the file the cursor is on"
                    );
                });
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Switching files and coming back lands where the file was left, and
    /// each mode keeps its own place: the hunks' row and the whole file's
    /// are different addresses.
    #[test]
    fn a_file_comes_back_where_it_was_left() {
        let (root, changed) = workspace("positions");
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) =
                    app(dispatcher, "positions", root.clone(), changed.clone());
                let cx = &mut cx0;
                let top = point(px(0.), px(0.));
                let deep = point(px(0.), px(-140.));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let _ = window;
                        v.select_path("src/app.rs".to_owned());
                        v.diff_hunks_scroll.set_offset(deep);
                        // Another file starts where a file starts…
                        v.select_path("src/other.rs".to_owned());
                        assert_eq!(v.diff_hunks_scroll.offset(), top);
                        // …and the one left behind comes back to its own
                        // row — but only once its *own* rows are up. File
                        // mode shows the poll's hunks until the file's read
                        // lands, and an offset applied to those few rows is
                        // clamped away for good, so the restore waits.
                        v.select_path("src/app.rs".to_owned());
                        assert_eq!(
                            v.pending_scroll.as_ref().map(|(path, _, offset)| (path.as_str(), *offset)),
                            Some(("src/app.rs", deep)),
                            "the position waits for the file's rows"
                        );
                        assert_eq!(v.diff_hunks_scroll.offset(), top, "nothing applied yet");
                        assert_eq!(
                            v.diff_hunks_scroll.offset(),
                            top,
                            "the fallback's rows are not where a position lands"
                        );

                        // The read lands: the position comes with it.
                        let app_view = crate::diff::view::FileView::build(
                            &root,
                            "src/app.rs",
                            changed.iter().find(|f| f.path == "src/app.rs"),
                        );
                        v.file_view = Some(std::rc::Rc::new(app_view));
                        v.file_view_key = Some(("src/app.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        assert_eq!(v.diff_hunks_scroll.offset(), deep, "the rows arrive");
                        assert!(v.pending_scroll.is_none(), "and the wait is over");

                        // The mode is part of the address: the whole file
                        // has its own place, and going back to the hunks
                        // returns to theirs.
                        v.set_view_mode(ViewMode::Diff, cx);
                        assert_eq!(v.diff_hunks_scroll.offset(), top);
                        v.diff_hunks_scroll.set_offset(point(px(0.), px(-60.)));
                        v.set_view_mode(ViewMode::File, cx);
                        assert_eq!(v.diff_hunks_scroll.offset(), deep);
                        v.set_view_mode(ViewMode::Diff, cx);
                        assert_eq!(v.diff_hunks_scroll.offset(), point(px(0.), px(-60.)));
                    });
                });
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
