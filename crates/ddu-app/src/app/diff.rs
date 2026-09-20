//! Diff data flow: the periodic working-tree poll, stale-result
//! guarding, the selection re-pin, and the file tree's row index.

use std::rc::Rc;
use std::time::Duration;

use super::*;
use ddu_diff::git;

/// Whether `path` is still part of the working tree the pane reads: the
/// diff knows it (a file deleted in the workdir stays readable through
/// its hunks), or it is on disk under `root`. The one rule both consumers
/// of a remembered path obey — the poll keeping a session's selection,
/// and a switch handing one back to the pane.
pub(super) fn lives_in_tree(
    root: Option<&std::path::Path>,
    files: &[ddu_diff::DiffFile],
    path: &str,
) -> bool {
    files.iter().any(|f| f.path == path) || root.is_some_and(|root| root.join(path).is_file())
}

impl AppView {

    /// Drop what the outgoing session's pane was showing: its selection,
    /// the find bar's hits on it, and the offset it sat at. A session
    /// keeps none of that across a switch — the incoming row has its own
    /// file, and `adopt_session_diff` puts it back.
    ///
    /// What belongs to the **working tree** is dropped only when the tree
    /// itself changed (`same_tree` false): the poll, the file-view and
    /// preview caches, the limits, the tree's rows and the tree's own
    /// scroll. Another session of the same project reads exactly the same
    /// rows — that is what "the tree is the project's" means — so a switch
    /// inside one project leaves it, and the incoming file's view, where
    /// it is.
    pub(super) fn reset_diff(&mut self, same_tree: bool) {
        self.selection = None;
        self.diff_search.matches.clear();
        self.pending_scroll = None;
        self.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
        if same_tree {
            return;
        }
        self.snapshot = None;
        self.diff_error = None;
        self.diff_limits.clear();
        self.file_view = None;
        self.file_view_key = None;
        self.preview = None;
        self.tree_index = None;
        self.diff_tree_scroll.set_offset(point(px(0.), px(0.)));
        // The quick open's path list belongs to the working tree the
        // session that owned it; the next open walks for the new one.
        self.file_search.forget_paths();
        self.file_search.matches.clear();
        self.file_search.current = 0;
    }

    /// Grow a truncated file's line budget ×4 and reload at once — the
    /// pane's infinite-scroll step, kicked when the rendered range
    /// reaches the cap note. No-op at the hard ceiling: the note stays.
    pub(crate) fn expand_diff_limit(&mut self, path: &str, cx: &mut Context<Self>) {
        let cur = self
            .diff_limits
            .get(path)
            .copied()
            .unwrap_or(ddu_diff::MAX_LINES_PER_FILE);
        if cur >= ddu_diff::EXPAND_MAX_LINES {
            return;
        }
        self.diff_limits
            .insert(path.to_owned(), (cur * 4).min(ddu_diff::EXPAND_MAX_LINES));
        self.reload_diff(cx);
    }

    /// Apply one poll. Returns whether anything moved — an **idle** poll
    /// must not repaint: a 3 s tick that re-rendered the pane and rebuilt
    /// the row caches produced a visible flash over whatever the user was
    /// reading, for a diff that had not changed at all.
    pub(super) fn apply_snapshot(
        &mut self,
        result: anyhow::Result<ddu_diff::Snapshot>,
        cx: &mut Context<Self>,
    ) -> bool {
        // The live selection's path: `None` stays `None` (no click =
        // empty pane — the poll never auto-selects a file). A session
        // switch seeds it with the row's own file, which the poll then
        // lands on the first time it applies.
        let wanted = self.selection.as_ref().map(|s| s.path.clone());
        let seed = if self.diff_seed_path.is_some() && self.snapshot.is_none() {
            self.diff_seed_path.clone()
        } else {
            None
        };
        let wanted = wanted.or(seed);
        let moved = match result {
            Ok(snapshot) => {
                // The selection is a path in the *tree*: a file on disk,
                // or one the diff still knows (a file deleted in the
                // workdir stays readable through its hunks). A clean file
                // therefore keeps its selection across polls, and only a
                // path that left the working tree clears the pane.
                let root = self.current_session_cwd();
                let next = wanted
                    .as_ref()
                    .filter(|path| lives_in_tree(root.as_deref(), &snapshot.diff.files, path))
                    .map(|path| crate::app::Selection {
                        path: path.clone(),
                        changed: snapshot.diff.files.iter().position(|f| &f.path == path),
                    });
                let moved = self.snapshot.as_ref() != Some(&snapshot) || next != self.selection;
                // The pane's file moved on its own — a restored row's
                // path landing on the first poll, or the path leaving the
                // working tree. Same bookkeeping as a click: remember
                // where the outgoing file was, put the incoming one back
                // where it was left — and let `restore_scroll` defer that
                // until the rows it was measured against are the ones on
                // screen, or the list clamps it to the hunks the pane
                // falls back to and the file opens at the wrong place.
                //
                // Compared against the *live* selection, not the wanted
                // path: on a seeded restore those are the same path, and
                // the position the session left behind is exactly what
                // such a poll has to put back.
                let was = self.selection.as_ref().map(|s| s.path.clone());
                if next.as_ref().map(|s| s.path.as_str()) != was.as_deref() {
                    self.remember_scroll();
                    self.selection = next;
                    self.restore_scroll();
                } else {
                    self.selection = next;
                }
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
    /// repository (`FILE_TREE.md` §4.1). The project's first snapshot also
    /// opens the changes' ancestors (`seed_open`).
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
        // The repository is what names the tree's root: a session may sit
        // in a subdirectory, and the paths the diff and the poll carry are
        // workdir-relative.
        let Some(workdir) = repo.workdir().map(std::path::Path::to_path_buf) else {
            self.tree_index = None;
            return;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            self.tree_index = None;
            return;
        };
        let changes = ddu_diff::listing::Changes::of(&snapshot.diff.files);
        // The default-expansion rule runs once per **project** — the tree
        // is the project's — and `seed_open` only adds, so it can never
        // re-open a directory the user has collapsed since it ran.
        if let Some(project) = self.projects.get_mut(self.current_project) {
            if !project.tree_seeded {
                project.tree_seeded = true;
                crate::ui::file_tree::seed_open(&mut project.tree_open, &changes);
            }
        }
        // Field-level borrows: the listing closure reads the diff while the
        // rows land in `tree_index`.
        let open = self.tree_open();
        let index = crate::ui::file_tree::build_index(open, |dir| {
            ddu_diff::listing::list_dir(&workdir, dir, &changes)
        });
        self.tree_index = Some(index);
        // The project's layer comes back where it was left. This is the
        // only moment its rows exist — before the poll's first snapshot
        // there is no index to put an offset on — and the list is the
        // finished one (the seeding rule above has already run).
        if let Some(offset) = self.pending_tree_scroll.take() {
            self.diff_tree_scroll.set_offset(point(px(0.), offset));
        }
    }

    /// The project's repository handle, discovered once per directory and
    /// kept for the session. `None` when the path is not in a repository —
    /// the tree then shows the poll's error, as before.
    pub(crate) fn repo_for(&mut self, root: &std::path::Path) -> Option<Rc<ddu_diff::git2::Repository>> {
        if let Some((key, repo)) = &self.repo {
            if key == root {
                return Some(repo.clone());
            }
        }
        let repo = Rc::new(ddu_diff::git2::Repository::discover(root).ok()?);
        self.repo = Some((root.to_path_buf(), repo.clone()));
        Some(repo)
    }

    /// The project's diff, if a poll has landed.
    pub(crate) fn diff(&self) -> Option<&ddu_diff::GitDiff> {
        self.snapshot.as_ref().map(|s| &s.diff)
    }

    /// The selection's diff record, when the selected file has one.
    pub(crate) fn selected_diff_file(&self) -> Option<&ddu_diff::DiffFile> {
        let changed = self.selection.as_ref()?.changed?;
        self.diff()?.files.get(changed)
    }

    /// The rows a mode falls back to when its own view is not ready yet:
    /// the merged whole-file view when it is cached (a clean file has
    /// nothing else), otherwise the diff's hunks if the file has any.
    pub(crate) fn file_rows<'a>(
        &'a self,
        file: Option<&'a ddu_diff::DiffFile>,
    ) -> Option<ddu_diff::RowStream<'a>> {
        match (self.cached_file_view(), file) {
            (Some(ddu_diff::file_view::FileView::Text(view)), _) => {
                Some(ddu_diff::RowStream::view(view))
            }
            (_, Some(file)) => Some(ddu_diff::RowStream::diff(file)),
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
    use crate::app::{AppView, CopyFileContents, CopyFilePath};
    use ddu_core::config::{Config, LoadWarnings, ProjectConfig, ShellConfig, State};
    use ddu_diff::{DiffFile, DiffHunk, DiffLine, GitDiff, Snapshot};
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
        let repo = ddu_diff::git2::Repository::init(&root).expect("init");
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
                    tree_open: vec![],
                    tree_height: None,
                    tree_scroll: None,
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
                v.projects[0].tree_seeded = false;
                v.projects[0].tree_open.clear();
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

    /// The two copy commands behind the tree's and the pane's context
    /// menus do what their menu labels say, on the pane's selected file —
    /// that is what lets the items carry those actions, and so show the
    /// chords that run them.
    #[test]
    fn the_copy_commands_put_the_selected_file_on_the_clipboard() {
        let (root, changed) = workspace("copy-commands");
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let (mut cx0, view, mut vcx) = app(dispatcher, "copy_commands", root, changed);
                let cx = &mut cx0;
                vcx.update(|window, cx| {
                    view.update(cx, |v, _| v.select_path("src/app.rs".to_owned()));
                    // Draw outside the lease: rendering an AppView while
                    // one is already leased is a re-entrancy panic.
                    let _ = window.draw(cx);
                });
                let clipboard = |vcx: &mut VisualTestContext| {
                    vcx.update(|_, cx| {
                        cx.read_from_clipboard().and_then(|item| item.text())
                    })
                };
                vcx.update(|_, cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("sentinel".into())));
                vcx.dispatch_action(CopyFilePath);
                assert_eq!(
                    clipboard(&mut vcx).as_deref(),
                    Some("src/app.rs"),
                    "Copy File Path copies the selection's path"
                );
                vcx.dispatch_action(CopyFileContents);
                assert_eq!(
                    clipboard(&mut vcx).as_deref(),
                    Some("fn main() {}\n// changed"),
                    "Copy File Contents copies the file's own lines, not the path"
                );
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// The quick open answers on its first frame from the paths git knows
    /// — the index's tracked files *and* the untracked ones only the poll
    /// knows — and committing a hit selects the file with its ancestors
    /// opened in the tree, so the row is where the search left it.
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
                        let paths = v.file_search.search.paths();
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
                            v.tree_open().contains("src"),
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

    /// The quick open's universe is the walk of the working tree, not git's
    /// view of it: a file the ignore rules hide — and the directories on
    /// the way to it — becomes searchable when the walk lands, the query
    /// re-ranks under it, and the cursor stays on its own path rather than
    /// being sent back to the top. A landing for a bar that has closed, or
    /// one kicked before this open, is dropped.
    #[test]
    fn the_walk_lands_under_the_palette_and_the_cursor_keeps_its_path() {
        let (root, changed) = workspace("walk-ignored");
        // Ignored, and absent from the poll's diff: the walk is the only
        // thing that can reach it.
        std::fs::write(root.join(".gitignore"), "ignored/\n").expect("write");
        std::fs::create_dir_all(root.join("ignored")).expect("mkdir");
        std::fs::write(root.join("ignored/secret.rs"), "fn secret() {}\n").expect("write");
        let walked = ddu_diff::listing::walk_files(&root);
        assert!(
            walked.iter().any(|p| p == "ignored/secret.rs"),
            "the walk reaches what git hides"
        );
        let root_for_app = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let (mut cx0, view, mut vcx) = app(dispatcher, "walk_ignored", root_for_app, changed);
                let cx = &mut cx0;
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.open_file_search(window, cx);
                        assert!(
                            !v.file_search
                                .search
                                .paths()
                                .iter()
                                .any(|p| p == "ignored/secret.rs"),
                            "git's view is what the palette opens on"
                        );
                        let seq = v.file_search.paths_seq;
                        v.file_search.input.update(cx, |input, cx| input.set_value("secret", window, cx));
                        v.refresh_file_search(cx);
                        assert_eq!(v.file_search.matches.len(), 0, "nothing git knows matches");

                        v.apply_file_paths(seq, walked.clone(), cx);
                        assert_eq!(
                            v.file_search.current_path(),
                            Some("ignored/secret.rs"),
                            "the landed walk is searched, and it reaches the ignored file"
                        );

                        // Stepped, then landed again: the cursor keeps its
                        // own path instead of being reset to the best hit.
                        v.file_search.input.update(cx, |input, cx| input.set_value("rs", window, cx));
                        v.refresh_file_search(cx);
                        v.file_search_step(false, cx);
                        let cursor = v.file_search.current_path().map(str::to_owned);
                        assert_eq!(v.file_search.current, 1, "stepped off the best hit");
                        v.apply_file_paths(seq, walked.clone(), cx);
                        assert_eq!(v.file_search.current_path().map(str::to_owned), cursor);
                        assert_eq!(v.file_search.current, 1, "the path kept its place");

                        // A landing kicked before this open, and one that
                        // arrives after the bar closed: both are dropped.
                        v.file_search.search.set_paths(vec!["sentinel.rs".to_owned()]);
                        v.apply_file_paths(seq + 1, walked.clone(), cx);
                        assert_eq!(
                            v.file_search.search.paths(),
                            ["sentinel.rs"],
                            "a landing from an older open is not this bar's answer"
                        );
                        v.open_file_search(window, cx);
                        let seq = v.file_search.paths_seq;
                        v.close_file_search(window, cx);
                        v.apply_file_paths(seq, walked.clone(), cx);
                        assert!(
                            v.file_search.search.paths().is_empty(),
                            "a closed bar is not answered"
                        );
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

    /// The pane's scroll region must be able to put the file's first row
    /// back at the top: the wheel over the rows, the wheel over the
    /// scrollbar's own column (where the overview marks live), and the
    /// thumb dragged to the top of the track.
    #[test]
    fn the_pane_returns_to_the_top() {
        let (root, mut changed) = workspace("scroll-top");
        // A file long enough that the pane really scrolls, and changed, so
        // the overview strip is drawn in the scrollbar's own column.
        let long: String = (0..400).map(|i| format!("let x{i} = {i};\n")).collect();
        std::fs::write(root.join("src/long.rs"), &long).expect("write");
        changed.push(DiffFile {
            path: "src/long.rs".to_owned(),
            added: 1,
            removed: 1,
            hunks: vec![DiffHunk {
                header: "@@ -100,3 +100,3 @@".into(),
                // The file already holds the workdir side (that is what a
                // diff means): the deleted line's text is HEAD's, and the
                // added one's is the file's own.
                lines: vec![
                    DiffLine {
                        kind: ' ',
                        old_no: Some(100),
                        new_no: Some(100),
                        text: "let x99 = 99;".into(),
                    },
                    DiffLine {
                        kind: '-',
                        old_no: Some(101),
                        new_no: None,
                        text: "let x100 = OLD;".into(),
                    },
                    DiffLine {
                        kind: '+',
                        old_no: None,
                        new_no: Some(101),
                        text: "let x100 = 100;".into(),
                    },
                    DiffLine {
                        kind: ' ',
                        old_no: Some(102),
                        new_no: Some(102),
                        text: "let x101 = 101;".into(),
                    },
                ],
            }],
            lines_total: 400,
            truncated: false,
        });
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) =
                    app(dispatcher, "scroll_top", root.clone(), changed.clone());
                let cx = &mut cx0;
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.show_diff = true;
                        v.select_path("src/long.rs".to_owned());
                        v.file_view = Some(std::rc::Rc::new(
                            ddu_diff::file_view::FileView::build(
                                &root,
                                "src/long.rs",
                                Some(&changed[2]),
                            ),
                        ));
                        v.file_view_key = Some(("src/long.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                });
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let top = point(px(0.), px(0.));
                let pane = vcx.debug_bounds("pane-diff").expect("the pane is on screen");
                let inside = point(pane.left() + px(30.), pane.center().y);
                let offset = |vcx: &mut VisualTestContext| {
                    vcx.update(|_, cx| view.update(cx, |v, _| v.diff_hunks_scroll.offset()))
                };
                let wheel = |vcx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>, dy: f32| {
                    vcx.simulate_event(gpui::ScrollWheelEvent {
                        position: at,
                        delta: gpui::ScrollDelta::Pixels(point(px(0.), px(dy))),
                        ..Default::default()
                    });
                };
                let draw = |vcx: &mut VisualTestContext| {
                    vcx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                };
                let overlay = vcx
                    .debug_bounds("scrollbar-overlay")
                    .expect("the pane's scrollbar is on screen");
                // The strip is in the scrollbar's own column, which is what
                // makes a press in that column ambiguous.
                let marks = vcx.debug_bounds("diff-overview").expect("the change marks");
                assert!(marks.right() > overlay.right() - px(12.), "marks sit in the track's column");

                // Down, then up past the top: the wheel over the rows.
                wheel(&mut vcx, inside, -50000.);
                assert!(offset(&mut vcx).y < px(-1000.), "the wheel scrolled down");
                wheel(&mut vcx, inside, 50000.);
                assert_eq!(offset(&mut vcx), top, "the wheel returns to the top");
                // … and over the scrollbar's own column.
                wheel(&mut vcx, inside, -50000.);
                wheel(&mut vcx, point(overlay.right() - px(6.), overlay.center().y), 50000.);
                assert_eq!(offset(&mut vcx), top, "the wheel over the track returns to the top");

                // The thumb: grab it at the bottom of the track, drag to
                // the very top.
                wheel(&mut vcx, inside, -50000.);
                draw(&mut vcx);
                let track_x = overlay.right() - px(6.);
                let thumb = point(track_x, overlay.bottom() - px(20.));
                vcx.simulate_mouse_move(thumb, None, gpui::Modifiers::default());
                draw(&mut vcx);
                let deep = offset(&mut vcx);
                vcx.simulate_mouse_down(thumb, gpui::MouseButton::Left, gpui::Modifiers::default());
                assert_eq!(
                    offset(&mut vcx),
                    deep,
                    "the press grabbed the thumb instead of jumping the track"
                );
                let top_of_track = point(track_x, overlay.top() + px(2.));
                vcx.simulate_mouse_move(
                    top_of_track,
                    Some(gpui::MouseButton::Left),
                    gpui::Modifiers::default(),
                );
                vcx.simulate_mouse_up(
                    top_of_track,
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                assert_eq!(offset(&mut vcx), top, "the thumb dragged to the top");

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
                        let app_view = ddu_diff::file_view::FileView::build(
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

    /// A second, dead row in the app's project: something to switch to
    /// without a second PTY, sharing the project's one working tree.
    fn second_session(cwd: &std::path::Path) -> ddu_core::session::AgentSession {
        ddu_core::session::AgentSession {
            id: "s-2".into(),
            title: "cat".into(),
            status: ddu_core::session::AgentStatus::Done(0),
            cmd: ddu_core::session::AgentCmd {
                program: "/bin/cat".into(),
                args: vec![],
            },
            resume_id: None,
            was_live: false,
            kind: "terminal".into(),
            started: std::time::Instant::now(),
            ended: Some(std::time::Instant::now()),
            term: None,
            cwd: cwd.to_path_buf(),
            diff_selected: None,
            view_mode: None,
        }
    }

    /// The file view a session's File-mode pane renders, built the way the
    /// background pass builds it.
    fn built_view(
        root: &std::path::Path,
        path: &str,
        changed: &[DiffFile],
    ) -> std::rc::Rc<ddu_diff::file_view::FileView> {
        std::rc::Rc::new(ddu_diff::file_view::FileView::build(
            root,
            path,
            changed.iter().find(|f| f.path == path),
        ))
    }

    /// Switching sessions is what a row's memory is for: each one comes
    /// back to **its own file**, in the place it was left. The switch
    /// zeroes the pane's offset with the rest of the outgoing session's
    /// state, so the position has to be read before the switch — and the
    /// restore then waits for its own rows on the way in.
    #[test]
    fn a_session_switch_restores_its_file_and_place() {
        let (root, changed) = workspace("session-restore");
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) =
                    app(dispatcher, "session_restore", root.clone(), changed.clone());
                let cx = &mut cx0;
                let top = point(px(0.), px(0.));
                let deep = point(px(0.), px(-140.));
                let deeper = point(px(0.), px(-260.));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.projects[0].sessions.push(second_session(&root));

                        // Row 0 reads `src/app.rs`, deep in the file.
                        v.select_path("src/app.rs".to_owned());
                        v.file_view = Some(built_view(&root, "src/app.rs", &changed));
                        v.file_view_key = Some(("src/app.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        v.diff_hunks_scroll.set_offset(deep);

                        // Away: the new row has no file of its own yet.
                        v.select_session(0, 1, window, cx);
                        assert_eq!(v.current_diff_path(), None);
                        assert_eq!(v.diff_hunks_scroll.offset(), top);

                        // Back: the row's file is on screen again in the
                        // same call — the project's diff never went away —
                        // and lands where it was left.
                        v.select_session(0, 0, window, cx);
                        assert_eq!(
                            v.current_diff_path(),
                            Some("src/app.rs"),
                            "the incoming row's file comes back with it"
                        );
                        assert_eq!(v.diff_hunks_scroll.offset(), deep, "and so does its place");

                        // A file whose view was never built waits for its
                        // rows instead of being clamped against the hunks
                        // the pane falls back to.
                        v.select_path("src/other.rs".to_owned());
                        assert_eq!(v.diff_hunks_scroll.offset(), top, "a new file starts at the top");
                        v.diff_hunks_scroll.set_offset(deeper);
                        v.select_session(0, 1, window, cx);
                        v.select_session(0, 0, window, cx);
                        assert_eq!(v.current_diff_path(), Some("src/other.rs"));
                        assert_eq!(
                            v.pending_scroll
                                .as_ref()
                                .map(|(path, _, offset)| (path.as_str(), *offset)),
                            Some(("src/other.rs", deeper)),
                            "the session's place in its file survives the switch"
                        );

                        // The read lands: the pane is back where the
                        // session left it.
                        v.file_view = Some(built_view(&root, "src/other.rs", &changed));
                        v.file_view_key = Some(("src/other.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        assert_eq!(v.diff_hunks_scroll.offset(), deeper, "the rows arrive");
                        assert!(v.pending_scroll.is_none(), "and the wait is over");
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

    /// The file tree is one per project — one repository, one working
    /// tree — so a session switch leaves it exactly as it was: the
    /// expansion the user worked out is not a session's to keep, and the
    /// tree does not jump back to the top under a switch either. Only the
    /// **file** is per session.
    #[test]
    fn a_session_switch_leaves_the_projects_file_tree_alone() {
        let (root, changed) = workspace("session-tree");
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) =
                    app(dispatcher, "session_tree", root.clone(), changed.clone());
                let cx = &mut cx0;
                let scrolled = point(px(0.), px(-40.));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        v.projects[0].sessions.push(second_session(&root));
                        // The seeding rule opened `src/` for the change;
                        // the user then scrolled the layer.
                        assert!(v.tree_open().contains("src"), "seeded on the change");
                        v.diff_tree_scroll.set_offset(scrolled);
                        // A file of row 0, so the switch has something to
                        // leave behind.
                        v.select_path("src/app.rs".to_owned());
                        let seeded = v.tree_index.as_ref().expect("the tree is built").rows.len();

                        v.select_session(0, 1, window, cx);
                        assert_eq!(
                            v.tree_index.as_ref().map(|i| i.rows.len()),
                            Some(seeded),
                            "the incoming row lists the project's tree, not its own"
                        );
                        assert!(
                            v.tree_open().contains("src"),
                            "the expansion is the project's, not the session's"
                        );
                        assert_eq!(
                            v.diff_tree_scroll.offset(),
                            scrolled,
                            "and the layer does not jump to the top"
                        );
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

    /// The same round trip with the pane's own read in the loop: no test
    /// hands the cache a prebuilt view here — `ensure_file_content` reads
    /// the file in the background the way the app does, and the deferred
    /// position lands with the rows. A switch seeds the incoming row's
    /// file exactly like a tree click, so this is the app's own path.
    #[test]
    fn a_session_switch_restores_the_files_place_off_the_read() {
        let (root, changed) = workspace("session-read");
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) = app(
                    dispatcher,
                    "session_read",
                    root.clone(),
                    changed.clone(),
                );
                let cx = &mut cx0;
                let top = point(px(0.), px(0.));
                let deep = point(px(0.), px(-140.));
                let deeper = point(px(0.), px(-260.));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let mut second = second_session(&root);
                        second.diff_selected = Some("src/other.rs".to_owned());
                        v.projects[0].sessions.push(second);
                        v.select_path("src/app.rs".to_owned());
                        // What the tree's own click handler does next.
                        v.ensure_file_content(cx);
                    });
                    // Frames are what clamp an offset against the rows they
                    // drew: the round trip has to survive them.
                    let _ = window.draw(cx);
                });
                cx.run_until_parked();
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        assert!(
                            matches!(
                                v.cached_file_view(),
                                Some(ddu_diff::file_view::FileView::Text(_))
                            ),
                            "the background read landed"
                        );
                        v.diff_hunks_scroll.set_offset(deep);
                        v.select_session(0, 1, window, cx);
                    });
                    let _ = window.draw(cx);
                });
                cx.run_until_parked();
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        assert_eq!(v.current_diff_path(), Some("src/other.rs"));
                        assert_eq!(v.diff_hunks_scroll.offset(), top, "the new file starts at the top");
                        assert_eq!(
                            v.saved_scroll("src/app.rs"),
                            deep,
                            "the file left behind keeps its place"
                        );
                        v.diff_hunks_scroll.set_offset(deeper);
                        v.select_session(0, 0, window, cx);
                    });
                    let _ = window.draw(cx);
                });
                cx.run_until_parked();
                vcx.update(|window, cx| {
                    view.update(cx, |v, _| {
                        assert_eq!(v.current_diff_path(), Some("src/app.rs"));
                        assert_eq!(
                            v.diff_hunks_scroll.offset(),
                            deep,
                            "the file comes back where it was left, off its own read"
                        );
                        assert!(v.pending_scroll.is_none(), "with nothing left waiting");
                    });
                    let _ = window.draw(cx);
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

    /// A rendered document is a surface of its own, and so is its place:
    /// gpui keeps a text view's scroll only while that view is rendered,
    /// so the pane draws the document from a state it holds per file —
    /// and a document left mid-way comes back to that passage. The pane's
    /// row handle has nothing to do with it: a position recorded there
    /// under the document's key would belong to rows the pane is not
    /// showing.
    #[test]
    fn a_document_keeps_its_own_place() {
        let (root, changed) = workspace("document-place");
        std::fs::write(root.join("notes.md"), "a paragraph of prose\n\n".repeat(400))
            .expect("write");
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) = app(
                    dispatcher,
                    "document_place",
                    root.clone(),
                    changed.clone(),
                );
                let cx = &mut cx0;
                // The document's state, and where its own scroll sits.
                let document = |vcx: &mut gpui_kit::VisualTestContext| {
                    vcx.update(|_, cx| view.read(cx).document_state().cloned())
                };
                let offset = |vcx: &mut gpui_kit::VisualTestContext,
                              state: &Entity<gpui_kit::base::TextViewState>| {
                    let state = state.clone();
                    vcx.update(|_, cx| {
                        state.read_with(cx, |state, _| {
                            state.list_state().scroll_px_offset_for_scrollbar()
                        })
                    })
                };
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let _ = window;
                        v.select_path("notes.md".to_owned());
                        v.ensure_file_content(cx);
                    });
                });
                cx.run_until_parked();
                vcx.update(|window, cx| {
                    view.update(cx, |v, _| {
                        assert_eq!(
                            v.surface(),
                            crate::ui::diff_panel::Surface::Preview,
                            "a Markdown file renders as a document"
                        );
                    });
                    let _ = window.draw(cx);
                });
                let state = document(&mut vcx).expect("the document's state is kept");
                let first = state.entity_id();

                // Scroll the document, the way its own scrollbar does.
                state.read_with(cx, |state, _| {
                    state
                        .list_state()
                        .set_offset_from_scrollbar(point(px(0.), px(-600.)));
                });
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                assert!(
                    offset(&mut vcx, &state).y < px(0.),
                    "the document is scrolled"
                );
                assert_eq!(
                    vcx.update(|_, cx| view
                        .read(cx)
                        .documents
                        .values()
                        .map(std::collections::HashMap::len)
                        .sum::<usize>()),
                    1,
                    "one state for the one document visited"
                );

                // Away to a file of rows, and back: the document is drawn
                // from the same state, so it opens where it was left.
                vcx.update(|window, cx| {
                    view.update(cx, |v, _| v.select_path("src/app.rs".to_owned()));
                    let _ = window.draw(cx);
                });
                cx.run_until_parked();
                vcx.update(|window, cx| {
                    view.update(cx, |v, _| v.select_path("notes.md".to_owned()));
                    let _ = window.draw(cx);
                });
                cx.run_until_parked();
                let back = document(&mut vcx).expect("the document's state is still there");
                assert_eq!(back.entity_id(), first, "drawn from the state it had");
                assert!(
                    offset(&mut vcx, &back).y < px(0.),
                    "and it is still scrolled"
                );
                assert!(
                    !vcx.update(|_, cx| view.read(cx).file_positions.contains_key(&(
                        root.clone(),
                        "notes.md".to_owned(),
                        ViewMode::File
                    ))),
                    "a document's place is not recorded on the row handle"
                );
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Two sessions whose rows each left a different file open: the
    /// switch is a round trip through the pane's own state, and both
    /// files have to come back where they were left. The bookkeeping
    /// runs on the *shared* scroll handle, and the outgoing file's place
    /// has to be read while the handle still holds it — a reset that
    /// zeroes the handle before the incoming file is even known turns
    /// the next read into "the outgoing file was at the top", which is
    /// how a remembered position used to be erased on the way out.
    #[test]
    fn a_session_switch_keeps_each_files_place() {
        let (root, changed) = workspace("session-two-files");
        let root_for_closure = root.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let (mut cx0, view, mut vcx) = app(
                    dispatcher,
                    "session_two_files",
                    root.clone(),
                    changed.clone(),
                );
                let cx = &mut cx0;
                let deep = point(px(0.), px(-140.));
                let deeper = point(px(0.), px(-260.));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let mut second = second_session(&root);
                        second.diff_selected = Some("src/other.rs".to_owned());
                        v.projects[0].sessions.push(second);

                        // Row 0 reads `src/app.rs`, deep in the file.
                        v.select_path("src/app.rs".to_owned());
                        v.file_view = Some(built_view(&root, "src/app.rs", &changed));
                        v.file_view_key = Some(("src/app.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        v.diff_hunks_scroll.set_offset(deep);

                        // Away: row 1 has a file of its own, and the file
                        // left behind keeps the place it was left at.
                        v.select_session(0, 1, window, cx);
                        assert_eq!(v.current_diff_path(), Some("src/other.rs"));
                        assert_eq!(
                            v.saved_scroll("src/app.rs"),
                            deep,
                            "the file left behind keeps its place"
                        );

                        // Row 1's file, deep in its own right.
                        v.file_view = Some(built_view(&root, "src/other.rs", &changed));
                        v.file_view_key = Some(("src/other.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        v.diff_hunks_scroll.set_offset(deeper);

                        // Back: row 0's file, exactly where it was.
                        v.select_session(0, 0, window, cx);
                        assert_eq!(v.current_diff_path(), Some("src/app.rs"));
                        assert_eq!(
                            v.saved_scroll("src/other.rs"),
                            deeper,
                            "the file left behind keeps its place"
                        );
                        v.file_view = Some(built_view(&root, "src/app.rs", &changed));
                        v.file_view_key = Some(("src/app.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        assert_eq!(v.diff_hunks_scroll.offset(), deep, "row 0's file");

                        // And away again: row 1's file is still where it
                        // was left, not at the top.
                        v.select_session(0, 1, window, cx);
                        assert_eq!(v.current_diff_path(), Some("src/other.rs"));
                        v.file_view = Some(built_view(&root, "src/other.rs", &changed));
                        v.file_view_key = Some(("src/other.rs".to_owned(), v.diff_gen));
                        v.apply_pending_scroll();
                        assert_eq!(v.diff_hunks_scroll.offset(), deeper, "row 1's file");
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

    /// The layer is the project's, so its place is too: moving to another
    /// project and back puts the tree back where it was left, and the
    /// rows it is measured against — the poll's snapshot — are what it
    /// waits for.
    #[test]
    fn a_project_keeps_its_file_trees_place() {
        let (root, changed) = workspace("project-tree-scroll");
        let other = std::env::temp_dir().join(format!("ddu-tree-scroll-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&other);
        std::fs::create_dir_all(other.join("lib")).expect("mkdir");
        std::fs::write(other.join("lib/one.rs"), "fn one() {}\n").expect("write");
        ddu_diff::git2::Repository::init(&other).expect("init");
        let root_for_closure = root.clone();
        let other_for_closure = other.clone();
        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let root = root_for_closure;
                let other = other_for_closure;
                let (mut cx0, view, mut vcx) = app(
                    dispatcher,
                    "project_tree_scroll",
                    root.clone(),
                    changed.clone(),
                );
                let cx = &mut cx0;
                let mine = point(px(0.), px(-120.));
                let theirs = point(px(0.), px(-300.));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let mut second = ddu_core::session::Project::new("other".into(), other.clone());
                        second.sessions.push(second_session(&other));
                        v.projects.push(second);
                        v.expanded.push(true);
                        v.diff_tree_scroll.set_offset(mine);

                        // Away: the other project's own layer starts at
                        // its own top.
                        v.select_session(1, 0, window, cx);
                        assert_eq!(v.current_diff_path(), None);
                        assert_eq!(v.diff_tree_scroll.offset(), point(px(0.), px(0.)));
                        // Its poll lands: rows exist, and the tree is
                        // scrolled by hand.
                        v.snapshot = Some(Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files: vec![],
                            },
                        });
                        v.rebuild_tree_index();
                        v.diff_tree_scroll.set_offset(theirs);
                        // A save records it against the project it is the
                        // project's place *of*.
                        v.persist(cx);
                        assert_eq!(v.projects[1].tree_scroll, Some(-300.));

                        // Back: the first project's tree comes back where
                        // it was left, once its own rows are there.
                        v.select_session(0, 0, window, cx);
                        assert_eq!(v.current_diff_path(), None);
                        // The switch's zeroed handle must not overwrite the
                        // place being returned to: until those rows exist,
                        // a save keeps what the project had.
                        v.persist(cx);
                        assert_eq!(v.projects[0].tree_scroll, Some(-120.));
                        v.snapshot = Some(Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files: changed.clone(),
                            },
                        });
                        v.rebuild_tree_index();
                        assert_eq!(
                            v.diff_tree_scroll.offset(),
                            mine,
                            "the first project's layer is where it was left"
                        );

                        // And the other project's, the other way.
                        v.select_session(1, 0, window, cx);
                        v.snapshot = Some(Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files: vec![],
                            },
                        });
                        v.rebuild_tree_index();
                        assert_eq!(
                            v.diff_tree_scroll.offset(),
                            theirs,
                            "the other project's layer is where it was left"
                        );
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
        let _ = std::fs::remove_dir_all(&other);
    }
}
