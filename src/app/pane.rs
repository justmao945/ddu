//! What the right pane shows: which surface (the file's hunks or the
//! whole file), the whole-file view and the rendered document cached per
//! `(path, diff generation)`, and where each file was left scrolled.
//!
//! The poll feeds these caches ([`super::diff`]); renders only read them.



use super::*;

impl AppView {

    /// Remember where the file being left was scrolled, and put the
    /// newly selected one back where it was left. Both are keyed by
    /// `(working tree, path, mode)`: a file reads the same way in every
    /// session of a project, and switching files is the thing this
    /// makes cheap — a re-rendered stream puts a file back at the top
    /// unless its own position is remembered.
    pub(super) fn remember_scroll(&mut self) {
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
    pub(super) fn saved_scroll(&self, path: &str) -> Point<Pixels> {
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
    pub(super) fn restore_scroll(&mut self) {
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
                Some(crate::diff::file_view::FileView::Text(_))
            ),
            crate::ui::diff_panel::Surface::Preview => true,
        }
    }

    /// Apply a deferred restore once its rows are the ones on screen.
    pub(super) fn apply_pending_scroll(&mut self) {
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
    fn preview_is_current(&self, build: &crate::diff::file_view::PreviewBuild) -> bool {
        self.current_diff_path() == Some(build.key.0.as_str()) && build.key.1 <= self.diff_gen
    }

    /// The cached whole-file view for the selected file. A view built for
    /// an *older* generation still renders: its replacement is being built
    /// off-thread and blanking the pane in the meantime is the flash the
    /// poll used to cause.
    pub(crate) fn cached_file_view(&self) -> Option<&crate::diff::file_view::FileView> {
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
    pub(crate) fn preview_refusal(&self) -> Option<crate::diff::read::Unreadable> {
        let build = self.preview.as_ref().filter(|b| self.preview_is_current(b))?;
        build.source.as_ref().err().copied()
    }

    /// The selected file's contents, for the clipboard: the rendered
    /// document's own source when the pane holds it (Preview mode reads
    /// the file from disk), otherwise the file view File mode built,
    /// otherwise the diff's own reconstruction (every line the diff did
    /// not remove). Empty when nothing is selected or nothing has been
    /// read — the caller then copies nothing, rather than overwriting
    /// the clipboard with an empty string.
    pub(crate) fn selected_file_text(&self) -> String {
        if let Some(source) = self.cached_preview() {
            return source.to_owned();
        }
        if let Some(crate::diff::file_view::FileView::Text(view)) = self.cached_file_view() {
            return view
                .rows
                .iter()
                .filter_map(|row| {
                    (row.line.kind != '-').then_some(row.line.text.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        // Changed but not yet read (or unreadable): reconstruct from the
        // diff's own lines. A clean file has no diff to fall back to.
        self.selected_diff_file()
            .map(|file| {
                file.hunks
                    .iter()
                    .flat_map(|h| h.lines.iter())
                    .filter(|l| l.kind != '-')
                    .map(|l| l.text.clone())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
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
                        crate::diff::file_view::FileView::build(&job_root, &job_path, file.as_ref())
                    });
                    let source = want_preview
                        .then(|| crate::diff::read::read_source(&job_root, &job_path));
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
                    v.preview = Some(crate::diff::file_view::PreviewBuild {
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
}
