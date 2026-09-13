//! Diff data flow: the periodic working-tree poll, stale-result
//! guarding and selection re-pinning.

use std::time::Duration;

use gpui_kit::component::input::InputState;

use super::*;

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
        self.diff = None;
        self.diff_error = None;
        self.diff_file = None;
        self.diff_limits.clear();
        self.file_view = None;
        self.file_view_key = None;
        self.preview_text = None;
        self.preview_key = None;
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

    pub(super) fn apply_diff(&mut self, result: anyhow::Result<GitDiff>, cx: &mut Context<Self>) {
        // The live selection's path: `None` stays `None` (no click =
        // empty pane — the poll never auto-selects a file).
        let selected = self
            .diff
            .as_ref()
            .and_then(|d| self.diff_file.and_then(|ix| d.files.get(ix)))
            .map(|f| f.path.clone());
        // First paint after a cold start: the persisted selection wins
        // when the path still exists in the working tree.
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
                    .and_then(|path| diff.files.iter().position(|f| &f.path == path));
                if next.map(|ix| diff.files[ix].path.as_str()) != selected.as_deref() {
                    self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
                }
                self.diff_file = next;
                self.diff = Some(diff);
                self.diff_error = None;
                // Every applied diff invalidates the whole-file view and
                // the preview source (they were merged against the
                // previous snapshot).
                self.diff_gen += 1;
                let current = self.current_diff_path().map(str::to_owned);
                let stale = |key: &Option<(String, u64)>| {
                    key.as_ref()
                        .is_some_and(|(p, _)| Some(p.as_str()) != current.as_deref())
                };
                if stale(&self.file_view_key) {
                    self.file_view = None;
                    self.file_view_key = None;
                }
                if stale(&self.preview_key) {
                    self.preview_text = None;
                    self.preview_key = None;
                }
            }
            Err(err) => {
                self.diff = None;
                self.diff_error = Some(err.to_string());
            }
        }
        self.diff_seed_path = None;
        self.rebuild_tree_index();
        // The 3s poll can rewrite the open file under an active
        // search: keep matches and counter truthful.
        self.refresh_diff_search(cx);
        self.ensure_file_content(cx);
    }

    /// Rebuild the sidebar tree's row list (diff changed, or a directory
    /// was toggled). One O(files) pass, off the render path.
    pub(crate) fn rebuild_tree_index(&mut self) {
        self.tree_index = self
            .diff
            .as_ref()
            .map(|diff| crate::ui::diff_tree::build_index(&diff.files, &self.diff_tree_closed));
    }

    /// The selected file's repo-relative path, if one is selected.
    pub(crate) fn current_diff_path(&self) -> Option<&str> {
        self.diff
            .as_ref()
            .and_then(|d| self.diff_file.and_then(|ix| d.files.get(ix)))
            .map(|f| f.path.as_str())
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

    /// The cached whole-file view for the selected file, while it is
    /// still the file's (same path) and still current (same diff
    /// generation).
    pub(crate) fn cached_file_view(&self) -> Option<&crate::diff::view::FileView> {
        let path = self.current_diff_path()?;
        let (cached, generation) = self.file_view_key.as_ref()?;
        (cached == path && *generation == self.diff_gen)
            .then(|| self.file_view.as_deref())
            .flatten()
    }

    /// The cached Markdown source for the selected file, same keying.
    pub(crate) fn cached_preview(&self) -> Option<&str> {
        let path = self.current_diff_path()?;
        let (cached, generation) = self.preview_key.as_ref()?;
        (cached == path && *generation == self.diff_gen)
            .then(|| self.preview_text.as_deref())
            .flatten()
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
        let Some(file) = self
            .diff
            .as_ref()
            .and_then(|d| self.diff_file.and_then(|ix| d.files.get(ix)))
            .cloned()
        else {
            return;
        };
        let path = file.path.clone();
        let generation = self.diff_gen;
        let key = (path.clone(), generation);
        let want_view =
            self.view_mode == ViewMode::File && self.file_view_key.as_ref() != Some(&key);
        let want_preview =
            self.view_mode == ViewMode::Preview && self.preview_key.as_ref() != Some(&key);
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
                        crate::diff::view::FileView::build(&job_root, &job_path, Some(&file))
                    });
                    let source = want_preview
                        .then(|| crate::diff::view::read_source(&job_root, &job_path))
                        .flatten();
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
                    v.preview_text = Some(std::rc::Rc::from(source.as_str()));
                    v.preview_key = Some((path.clone(), generation));
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
            self.diff = None;
            self.diff_error = None;
            return;
        };
        let limits = self.diff_limits.clone();
        let this = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_spawn(async move { git::head_diff(&path, &limits) })
                .await;
            if let Err(e) = &result {
                eprintln!("[ddu] diff err: {e:#}");
            }
            let _ = this.update(cx, |v, cx| {
                if v.diff_seq == seq {
                    v.apply_diff(result, cx);
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
                    .background_spawn(async move { git::head_diff(&path, &limits) })
                    .await;
                this.update(cx, |v, cx| {
                    if v.diff_seq == seq {
                        v.apply_diff(result, cx);
                        cx.notify();
                    }
                })?;
            }
            anyhow::Ok(())
        })
        .detach();
    }
}
