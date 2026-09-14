//! Diff data flow: the periodic working-tree poll, stale-result
//! guarding, the selection re-pin, and the file tree's row index.

use std::rc::Rc;
use std::time::Duration;

use super::*;
use crate::diff::git;

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
        let changes = crate::diff::listing::Changes::of(&snapshot.diff.files);
        if !self.tree_seeded {
            self.tree_seeded = true;
            crate::ui::file_tree::seed_open(&mut self.diff_tree_open, &changes);
        }
        // Field-level borrows: the listing closure reads the repo and the
        // diff while the rows land in `tree_index`.
        let open = &self.diff_tree_open;
        let index = crate::ui::file_tree::build_index(open, |dir| {
            crate::diff::listing::list_dir(&repo, dir, &changes)
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
            (Some(crate::diff::file_view::FileView::Text(view)), _) => {
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
                            crate::diff::file_view::FileView::build(
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
                        let app_view = crate::diff::file_view::FileView::build(
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
