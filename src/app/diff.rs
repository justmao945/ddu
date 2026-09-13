//! Diff data flow: the periodic working-tree poll, stale-result
//! guarding and selection re-pinning.

use std::time::Duration;

use gpui_kit::component::input::InputState;

use super::*;
use crate::diff::git;

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


impl AppView {
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
        self.diff_tree_closed.clear();
        self.diff_search.matches.clear();
        self.diff_tree_scroll.set_offset(point(px(0.), px(0.)));
        self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
    }

    /// Open the find bar (or refocus it when already open); the whole
    /// query is selected so typing replaces it.
    pub(crate) fn open_diff_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A rendered document has no row list to match against: ⌘F in
        // Preview reopens in File mode.
        if self.view_mode == ViewMode::Preview {
            self.set_view_mode(ViewMode::File, cx);
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
                let next = selected
                    .as_ref()
                    .filter(|path| snapshot.tree.get(path).is_some())
                    .map(|path| crate::app::Selection {
                        path: path.clone(),
                        changed: snapshot.diff.files.iter().position(|f| &f.path == path),
                    });
                let moved = self.snapshot.as_ref() != Some(&snapshot) || next != self.selection;
                if next.as_ref().map(|s| s.path.as_str()) != selected.as_deref() {
                    self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
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
        true
    }

    /// Rebuild the sidebar tree's row list (a new snapshot, a filter
    /// switch, or a directory toggle). One O(files) pass, off the render
    /// path; the first snapshot of a session also folds the clean
    /// directories away (see `seed_clean_dirs`).
    pub(crate) fn rebuild_tree_index(&mut self) {
        self.tree_index = self.snapshot.as_ref().map(|snapshot| {
            if !self.tree_seeded {
                self.tree_seeded = true;
                crate::ui::diff_tree::seed_clean_dirs(
                    &snapshot.tree,
                    &mut self.diff_tree_closed,
                );
            }
            crate::ui::diff_tree::build_index(
                &snapshot.tree,
                self.tree_filter,
                &self.diff_tree_closed,
            )
        });
    }

    /// The project's diff, if a poll has landed.
    pub(crate) fn diff(&self) -> Option<&crate::diff::GitDiff> {
        self.snapshot.as_ref().map(|s| &s.diff)
    }

    /// The full working-tree listing, if a poll has landed.
    pub(crate) fn tree(&self) -> Option<&crate::diff::tree::FileTree> {
        self.snapshot.as_ref().map(|s| &s.tree)
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
            self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
        }
        self.selection = Some(crate::app::Selection { path, changed });
    }

    /// ⌘⇧F / the strip's toggles: All ⇄ Changed.
    pub(crate) fn toggle_tree_filter(&mut self, cx: &mut Context<Self>) {
        self.tree_filter = self.tree_filter.next();
        self.rebuild_tree_index();
        self.persist(cx);
        cx.notify();
    }

    /// The mode the pane actually renders. Preview is Markdown-only, so
    /// a non-Markdown selection falls back to File while the mode itself
    /// survives — returning to a `.md` file returns to the preview.
    pub(crate) fn effective_view_mode(&self) -> ViewMode {
        match self.current_diff_path() {
            Some(path) if self.view_mode.available(path) => self.view_mode,
            _ => ViewMode::File,
        }
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

    /// Preview mode's Markdown source for the selected file, same keying
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

    /// Why Preview cannot show the selected file (too large, binary, or
    /// gone): the pane bands this above the rows it falls back to,
    /// exactly as File mode does.
    pub(crate) fn preview_refusal(&self) -> Option<crate::diff::view::Unreadable> {
        let build = self.preview.as_ref().filter(|b| self.preview_is_current(b))?;
        build.source.as_ref().err().copied()
    }

    /// Switch the pane's surface. Per session: persisted with the row.
    pub(crate) fn set_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        let Some(path) = self.current_diff_path().map(str::to_owned) else {
            return;
        };
        let mode = if mode.available(&path) {
            mode
        } else {
            ViewMode::File
        };
        if self.view_mode == mode {
            return;
        }
        self.view_mode = mode;
        // The stream's shape changes with the mode, so the old offset
        // means nothing in the new one.
        self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
        self.ensure_file_content(cx);
        self.refresh_diff_search(cx);
        self.persist(cx);
        cx.notify();
    }

    /// ⌘⇧M: Diff → File → Preview (Markdown only) → Diff.
    pub(crate) fn cycle_view_mode(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.current_diff_path().map(str::to_owned) else {
            return;
        };
        let next = self.view_mode.next(&path);
        self.set_view_mode(next, cx);
    }

    /// Start the background read behind File/Preview mode when the cache
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
        let want_view =
            self.view_mode == ViewMode::File && self.file_view_key.as_ref() != Some(&key);
        let want_preview = self.view_mode == ViewMode::Preview
            && self.preview.as_ref().map(|b| &b.key) != Some(&key);
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
