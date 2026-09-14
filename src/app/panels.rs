//! Panel toggles: sidebar, changes pane, diff-tree layer — plus the
//! remembered last-dragged widths each toggle restores.

use super::*;

impl AppView {
    /// Toggle the sidebar, keeping the splitter slot list in sync.
    pub(crate) fn toggle_sessions(&mut self, cx: &mut Context<Self>) {
        self.set_sessions(!self.show_sessions, cx);
    }

    pub(crate) fn set_sessions(&mut self, on: bool, cx: &mut Context<Self>) {
        if on == self.show_sessions {
            return;
        }
        self.show_sessions = on;
        self.persist(cx);
        let restore_w = self.last_sidebar_w();
        if !on {
            // Capture before removal: after `remove_panel(0)` slot 0 is
            // the center+diff region, not the sidebar.
            self.last_sidebar_size = self.shell_state.read(cx).sizes().first().copied();
        }
        // The panes (terminal/diff) live in their own splitter, so a
        // sidebar insert/remove never rescales them — no re-pin needed.
        self.shell_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(0), cx);
            } else {
                state.remove_panel(0, cx);
            }
        });
        cx.notify();
    }

    /// Toggle the diff panel (slot sits after the always-present center).
    pub(crate) fn toggle_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_diff(!self.show_diff, window, cx);
    }

    /// Toggle the diff file tree layer under the project tree.
    pub(crate) fn toggle_diff_tree(&mut self, cx: &mut Context<Self>) {
        self.show_diff_tree = !self.show_diff_tree;
        self.persist(cx);
        cx.notify();
    }

    pub(crate) fn set_diff(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        if on == self.show_diff {
            return;
        }
        // Hiding the pane hides the find bar with it; close it through
        // the real path so focus returns to the window fallback instead
        // of lingering on the now-unmounted input.
        if !on && self.diff_search.open {
            self.close_diff_search(window, cx);
        }
        // Inner splitter slots: the terminal is 0, the diff 1.
        const DIFF_IX: usize = 1;
        self.show_diff = on;
        self.persist(cx);
        let restore_w = self.last_diff_w();
        if !on {
            // Capture before removal: the slot shifts after `remove_panel`.
            self.last_diff_size = self.panes_state.read(cx).sizes().get(DIFF_IX).copied();
        }
        // The sidebar lives in the outer splitter, so a diff
        // insert/remove never rescales it — no re-pin needed.
        self.panes_state.update(cx, |state, cx| {
            if on {
                state.insert_panel(Some(restore_w), Some(DIFF_IX), cx);
            } else {
                state.remove_panel(DIFF_IX, cx);
            }
        });
        cx.notify();
    }

    pub(super) fn last_sidebar_w(&self) -> Pixels {
        self.last_sidebar_size
            .filter(|w| *w >= px(sidebar_min()))
            .unwrap_or(px(sidebar_default()))
    }

    pub(super) fn last_diff_w(&self) -> Pixels {
        self.last_diff_size
            .filter(|w| *w >= px(diff_min()))
            .unwrap_or(px(diff_default()))
    }
}

/// The shell renders the sidebar, the terminal pane and the breadcrumb
/// as **cached child views** (see `panel_view!`): gpui replays a cached
/// subtree — layout, paint, hitboxes, listeners, key contexts — until the
/// view is notified, which is what keeps a streaming terminal off the
/// sidebar. The changes pane is deliberately *not* cached: it is the app's
/// only selectable surface, and a replayed subtree never re-registers its
/// window-selection participants (see `panel_view!` and
/// `a_selection_in_the_changes_pane_survives_a_frame_it_did_not_ask_for`).
/// These tests pin both halves of that contract, plus the gpui behavior
/// they rest on.
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

    /// A stream wakeup repaints the terminal pane alone: neither the
    /// cached sidebar nor the changes pane is notified, or every frame of
    /// a stream would rebuild them (the whole point of the split; the
    /// changes pane re-renders per frame either way, but nothing may make
    /// it *rebuild* a stream frame — that is the terminal pane's job
    /// alone). A background session with an unchanged title repaints
    /// nothing at all.
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

    /// A background row's spinner is on screen even though its grid is
    /// not: the row shows the OSC title, agent CLIs spin it, and a cached
    /// row that is only refreshed by the 1 Hz ui tick reads as jerky the
    /// moment the user switches away from the session doing the work.
    /// Its title change must therefore reach the sidebar from a
    /// background session too — and reach nothing else: the grid is off
    /// screen, so the pane has nothing to repaint, and an *unchanged*
    /// title must notify nobody (an alive-but-silent agent drives no
    /// frames).
    #[test]
    fn a_background_rows_title_follows_its_own_stream() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let (mut cx0, view, counts) =
                    app_with_panel_counters(dispatcher, 2, "background_title_reaches_the_row");
                let cx = &mut cx0;
                let background = cx.update(|cx| {
                    view.read(cx).projects[0].sessions[1]
                        .term
                        .clone()
                        .unwrap()
                });
                let seen = |counts: &[Rc<Cell<usize>>; 3]| counts.clone().map(|c| c.get());
                let title = |text: &str, cx: &mut gpui_kit::App| {
                    background.update(cx, |term, _| {
                        term.inject_bytes(format!("\x1b]0;{text}\x07").as_bytes());
                    });
                };

                // Each injection is a burst of its own: the pump paints the
                // first wakeup of a burst at once, so the clock has to move
                // past the interval between them (the spawn's own wakeups
                // already set the last-frame mark).
                cx.executor().advance_clock(crate::terminal::STREAM_FRAME_MIN);
                cx.update(|cx| title("⠋ working", cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [1, 0, 0],
                    "a background row's new title repaints the row alone                      (sidebar, changes pane, terminal pane)"
                );

                cx.executor().advance_clock(crate::terminal::STREAM_FRAME_MIN);
                cx.update(|cx| title("⠋ working", cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [1, 0, 0],
                    "the same title again leaves the cached row alone"
                );

                cx.executor().advance_clock(crate::terminal::STREAM_FRAME_MIN);
                cx.update(|cx| title("⠙ working", cx));
                cx.run_until_parked();
                assert_eq!(
                    seen(&counts),
                    [2, 0, 0],
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

    /// A selection made in the changes pane has to outlive the frames it
    /// did not ask for.
    ///
    /// gpui's window selection keeps a participant only while it
    /// re-registers: `TextSelectionLayer` sweeps every participant that
    /// did not register during the frame (`WindowSelectionState::finish_frame`),
    /// and a *cached* subtree re-registers nothing on the frames it
    /// replays. The shell therefore mounts the changes pane uncached —
    /// the app's only selectable surface — so the rows land in the
    /// window selection on every frame; a cached pane dropped the
    /// selection one frame after the drag that made it (the highlight
    /// blinking off while a session streamed).
    #[test]
    fn a_selection_in_the_changes_pane_survives_a_frame_it_did_not_ask_for() {
        use crate::app::{AppView, Selection};
        use crate::config::{Config, LoadWarnings, ShellConfig, State};
        use crate::diff::{DiffFile, DiffHunk, DiffLine, GitDiff};
        use gpui_kit::base::TextSelection;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("pane_selection_survives_a_frame"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    // A silent shell: the reader thread must not race the
                    // test scheduler with a prompt.
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "/bin/cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, vcx) = cx.add_window_view(|window, cx| AppView::new(window, cx));
                let lines: Vec<DiffLine> = (1..=40)
                    .map(|n| DiffLine {
                        kind: '+',
                        old_no: None,
                        new_no: Some(n),
                        text: format!("selected line {n:02} with some words on it"),
                    })
                    .collect();
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let files = vec![DiffFile {
                            path: "src.rs".into(),
                            added: 40,
                            removed: 0,
                            hunks: vec![DiffHunk {
                                header: "@@ -0,0 +1,40 @@".into(),
                                lines,
                            }],
                            lines_total: 40,
                            truncated: false,
                        }];
                        v.show_diff = true;
                        v.selection = Some(Selection {
                            path: "src.rs".into(),
                            changed: Some(0),
                        });
                        v.view_mode = crate::ui::diff_panel::ViewMode::Diff;
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                        });
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                });

                let pane = vcx
                    .debug_bounds("pane-diff")
                    .expect("the changes pane lays out");
                let row_y = pane.origin.y + gpui_kit::px(crate::ui::panel_header_px() + 40.);
                let from = gpui_kit::point(pane.origin.x + gpui_kit::px(30.), row_y);
                let to = gpui_kit::point(pane.origin.x + gpui_kit::px(220.), row_y + gpui_kit::px(40.));
                vcx.simulate_mouse_down(from, gpui_kit::MouseButton::Left, Default::default());
                vcx.simulate_mouse_move(
                    to,
                    Some(gpui_kit::MouseButton::Left),
                    Default::default(),
                );
                vcx.simulate_mouse_up(to, gpui_kit::MouseButton::Left, Default::default());
                let selected = vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                    TextSelection::selected_text(window, cx)
                });
                assert!(
                    selected.contains("selected line"),
                    "the drag selects rows, got {selected:?}"
                );

                // Frames the pane did not ask for — a stream frame looks
                // like this: the window draws, nothing notifies the pane.
                for _ in 0..3 {
                    vcx.update(|window, cx| {
                        let _ = window.draw(cx);
                    });
                }
                let after = vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                    TextSelection::selected_text(window, cx)
                });
                assert!(
                    after.contains("selected line"),
                    "the selection must outlive an untouched frame, got {after:?}"
                );

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
