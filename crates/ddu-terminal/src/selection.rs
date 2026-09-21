//! Text selection over the grid: window px → cell mapping, the drag
//! (semantic runs on a double click) and the clipboard copy.

use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use gpui_kit::*;

use super::*;

/// How fast a drag held past the pane's edge crawls through the
/// scrollback: one line per tick, so a held pointer reads as a steady
/// ~25 lines/s.
const DRAG_SCROLL_TICK: Duration = Duration::from_millis(40);

impl TermSession {
    /// Map a window-coordinate point to a grid cell, clamped into the
    /// visible area (a drag outside the grid pins to its edge). The
    /// `Side` picks the cell edge nearest the x fraction — alacritty's
    /// anchor convention for drag ends.
    pub fn cell_at(
        &self,
        pos: Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<(GridPoint, Side)> {
        let bounds = self.content_bounds.get()?;
        let m = element::Metrics::new(window, cx);
        let rel = pos - bounds.origin;
        let (cols, rows) = self.grid.size();
        let col = (rel.x / m.cell_width)
            .floor()
            .clamp(0., f32::from(cols.saturating_sub(1)));
        let row = (rel.y / m.line_height)
            .floor()
            .clamp(0., f32::from(rows.saturating_sub(1)));
        let side = if (rel.x / m.cell_width) - col < 0.5 {
            Side::Left
        } else {
            Side::Right
        };
        let offset = self.grid.term.lock().grid().display_offset() as i32;
        Some((
            GridPoint::new(Line(row as i32 - offset), Column(col as usize)),
            side,
        ))
    }

    /// Start a selection on mouse down; a double-click extends to the
    /// semantic (word) run around the cell.
    pub fn begin_selection(
        &mut self,
        cell: GridPoint,
        side: Side,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let ty = if clicks >= 2 {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        self.grid.term.lock().selection = Some(Selection::new(ty, cell, side));
        self.selecting = true;
        cx.emit(TermEvent::Wakeup);
    }

    /// Extend the active drag to the cell under the pointer.
    pub fn grow_selection(&mut self, cell: GridPoint, side: Side, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some(selection) = self.grid.term.lock().selection.as_mut() {
            selection.update(cell, side);
            cx.emit(TermEvent::Wakeup);
        }
    }

    /// Pointer motion of a live drag: extend the selection to the cell
    /// under the pointer and keep the drag alive once the pointer is
    /// past the content's top or bottom edge.
    ///
    /// The clamped cell is what makes the edge case work: after the
    /// viewport has moved, the same window point maps to the row that
    /// scrolled into view, so a drag that runs off the top of the pane
    /// gathers scrollback instead of stopping at the last visible row
    /// (a selection is otherwise bounded by one screen, since a drag
    /// can only address the visible rows).
    pub fn drag_motion(&mut self, pos: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        if let Some((cell, side)) = self.cell_at(pos, window, cx) {
            self.grow_selection(cell, side, cx);
        }
        self.drag_scroll = self
            .content_bounds
            .get()
            .and_then(|area| match pos.y {
                y if y < area.origin.y => Some(1),
                y if y > area.origin.y + area.size.height => Some(-1),
                _ => None,
            })
            .map(|dir| (dir, pos));
        if self.drag_scroll.is_some() {
            self.arm_drag_scroll(window, cx);
        }
    }

    /// Start the repeating drag autoscroll when it is not already
    /// running — one tick in flight, re-armed by the tick itself while
    /// the pointer stays past the edge (the same single-timer shape
    /// [`TermSession::arm_scrollbar_hide`] uses). A pointer *held* past
    /// the edge keeps scrolling: motion events alone would stop the
    /// moment the hand does.
    fn arm_drag_scroll(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.drag_scroll_armed {
            return;
        }
        self.drag_scroll_armed = true;
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(DRAG_SCROLL_TICK).await;
            let _ = this.update_in(cx, |term, window, cx| {
                term.drag_scroll_armed = false;
                let Some((dir, pos)) = term.drag_scroll else {
                    return;
                };
                term.scroll_by(dir as f32, cx);
                term.drag_motion(pos, window, cx);
            });
        })
        .detach();
    }

    /// End the drag; a plain click (empty selection) clears the wash.
    pub fn end_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        self.drag_scroll = None;
        let mut term = self.grid.term.lock();
        if term.selection.as_ref().is_some_and(|s| s.is_empty()) {
            term.selection = None;
        }
        cx.emit(TermEvent::Wakeup);
    }

    pub fn copy_selection(&self, cx: &mut App) -> bool {
        match self.grid.term.lock().selection_to_string() {
            Some(text) if !text.trim().is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            _ => false,
        }
    }

    /// Non-empty mouse selection present (drives Copy enablement).
    pub fn has_selection(&self) -> bool {
        self.grid
            .term
            .lock()
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    // Selective imports only: `use super::*` would pull gpui's `test`
    // proc-macro re-export into scope, shadowing the built-in `#[test]`
    // and recursing forever at expansion.
    use crate::harness::{TestRoot, plant_lines, shutdown, spawn_cat};
    use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, FocusHandle,
        InteractiveElement as _, IntoElement, Modifiers, MouseButton, ParentElement as _,
        Render, Styled as _, TestAppContext, Window, div, gpui, point, px,
    };

    use super::{DRAG_SCROLL_TICK, TermSession};

    /// ⌃⇧C's terminal half: the chord reaches `copy_selection` (bound in
    /// `keys.rs`), and that puts the grid's selection on the clipboard —
    /// the only route from a terminal drag to the system clipboard, so a
    /// break here reads as "the terminal cannot copy".
    #[test]
    fn copy_selection_puts_the_grid_selection_on_the_clipboard() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(
                    dispatcher,
                    Some("copy_selection_puts_the_grid_selection_on_the_clipboard"),
                );
                let cx = &mut cx0;
                let window = cx.add_window(|_, _| TestRoot);
                let window = AnyWindowHandle::from(window);
                let session = cx.update(spawn_cat);
                cx.update_window(window, |_, _window, cx| {
                    session.update(cx, |s, cx| {
                        s.grid.inject_bytes(b"copy me\r\nsecond line\r\n");
                        // A drag across the first line, the way the mouse
                        // handlers build one.
                        let mut term = s.grid.term.lock();
                        let mut selection = Selection::new(
                            SelectionType::Simple,
                            GridPoint::new(Line(0), Column(0)),
                            Side::Left,
                        );
                        selection.update(
                            GridPoint::new(Line(0), Column("copy me".len() - 1)),
                            Side::Right,
                        );
                        term.selection = Some(selection);
                        drop(term);

                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                            "sentinel".into(),
                        ));
                        assert!(s.copy_selection(cx), "the selection is copyable");
                        assert_eq!(
                            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
                            Some("copy me")
                        );
                        // Nothing selected: it copies nothing rather
                        // than blanking the clipboard.
                        s.grid.term.lock().selection = None;
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                            "sentinel".into(),
                        ));
                        assert!(!s.copy_selection(cx));
                        assert_eq!(
                            cx.read_from_clipboard().and_then(|item| item.text()).as_deref(),
                            Some("sentinel")
                        );
                    });
                })
                .unwrap();
                shutdown(&session, cx);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// Hit-test mapping: window px → grid cell, clamped at the edges
    /// and shifted by the scroll offset — an off-by-one here selects
    /// the wrong row/column.
    #[test]
    fn cell_at_maps_pixels_to_grid() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("cell_at_maps_pixels_to_grid"));
                let cx = &mut cx0;
                let window = cx.add_window(|_, _| TestRoot);
                let window = AnyWindowHandle::from(window);
                let session = cx.update(spawn_cat);

                cx.update_window(window, |_, window, cx| {
                    session.update(cx, |s, cx| {
                        // Pretend a paint: content rect at (100, 50).
                        s.content_bounds.set(Some(gpui_kit::Bounds {
                            origin: gpui_kit::point(gpui_kit::px(100.), gpui_kit::px(50.)),
                            size: gpui_kit::size(gpui_kit::px(800.), gpui_kit::px(500.)),
                        }));
                        let m = super::element::Metrics::new(window, cx);
                        let cell_w = f32::from(m.cell_width);
                        let line_h = f32::from(m.line_height);

                        // Center of cell (3, 2) → line 2, column 3.
                        let pos = gpui_kit::point(
                            gpui_kit::px(100. + 3.5 * cell_w),
                            gpui_kit::px(50. + 2.5 * line_h),
                        );
                        let (cell, _side) = s.cell_at(pos, window, cx).expect("inside bounds");
                        assert_eq!((cell.line.0, cell.column.0), (2, 3));

                        // Far outside the grid clamps to the last cell.
                        let pos = gpui_kit::point(gpui_kit::px(5000.), gpui_kit::px(5000.));
                        let (cell, _) = s.cell_at(pos, window, cx).expect("clamped");
                        assert_eq!((cell.line.0, cell.column.0), (23, 79));

                        // Scrolled 5 lines into history: the same screen row
                        // addresses a grid line 5 lower. Fill 40 lines first
                        // so the 24-row grid actually has scrollback.
                        let mut parser = alacritty_terminal::vte::ansi::Processor::<
                            alacritty_terminal::vte::ansi::StdSyncHandler,
                        >::new();
                        let mut term = s.grid.term.lock();
                        for i in 0..40 {
                            for &byte in format!("line{i}\r\n").as_bytes() {
                                parser.advance(&mut *term, byte);
                            }
                        }
                        term.scroll_display(alacritty_terminal::grid::Scroll::Delta(5));
                        drop(term);
                        let pos = gpui_kit::point(
                            gpui_kit::px(100. + 3.5 * cell_w),
                            gpui_kit::px(50. + 2.5 * line_h),
                        );
                        let (cell, _) = s.cell_at(pos, window, cx).expect("scrolled");
                        assert_eq!((cell.line.0, cell.column.0), (-3, 3));
                    });
                })
                .unwrap();

                shutdown(&session, cx);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }


    /// End-to-end: a left-drag across grid cells must set a selection
    /// whose copy text is the swept run — this exercises the same
    /// hitbox → listener → `cell_at` → alacritty-model chain the live
    /// surface uses (`ui::terminal_panel::surface` mirrors these listeners).
    #[test]
    fn mouse_drag_selects_grid_text() {
        struct SelRoot {
            term: Entity<TermSession>,
            focus: gpui_kit::FocusHandle,
        }
        impl Render for SelRoot {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let weak = self.term.downgrade();
                div().size_full().child(
                    div()
                        .h_full()
                        .on_mouse_down(
                            gpui_kit::MouseButton::Left,
                            cx.listener({
                                let weak = weak.clone();
                                move |_, event: &gpui_kit::MouseDownEvent, window, cx| {
                                    if let Some(term) = weak.upgrade() {
                                        term.update(cx, |s, cx| {
                                            if let Some((cell, side)) =
                                                s.cell_at(event.position, window, cx)
                                            {
                                                s.begin_selection(
                                                    cell,
                                                    side,
                                                    event.click_count,
                                                    cx,
                                                );
                                            }
                                        });
                                    }
                                }
                            }),
                        )
                        .on_mouse_move(cx.listener({
                            let weak = weak.clone();
                            move |_, event: &gpui_kit::MouseMoveEvent, window, cx| {
                                if let Some(term) = weak.upgrade() {
                                    term.update(cx, |s, cx| {
                                        if let Some((cell, side)) =
                                            s.cell_at(event.position, window, cx)
                                        {
                                            s.grow_selection(cell, side, cx);
                                        }
                                    });
                                }
                            }
                        }))
                        .on_mouse_up(
                            gpui_kit::MouseButton::Left,
                            cx.listener({
                                let weak = weak.clone();
                                move |_, _, _, cx| {
                                    if let Some(term) = weak.upgrade() {
                                        term.update(cx, |s, cx| s.end_selection(cx));
                                    }
                                }
                            }),
                        )
                        .child(TermSession::element(weak, self.focus.clone())),
                )
            }
        }

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("mouse_drag_selects_grid_text"));
                let cx = &mut cx0;
                let session = cx.update(spawn_cat);
                // Deterministic rows: "row00-abcdefghij" .. "row09-abcdefghij".
                {
                    cx.update(|cx| {
                        let mut parser = alacritty_terminal::vte::ansi::Processor::<
                            alacritty_terminal::vte::ansi::StdSyncHandler,
                        >::new();
                        let mut term = session.read(cx).grid.term.lock();
                        for i in 0..10 {
                            for &byte in format!("row{i:02}-abcdefghij\r\n").as_bytes() {
                                parser.advance(&mut *term, byte);
                            }
                        }
                    });
                }

                let session2 = session.clone();
                let (_view, vcx) = cx.add_window_view(move |_, cx| SelRoot {
                    term: session2,
                    focus: cx.focus_handle(),
                });

                // A rendered frame stashed the element bounds; map row 2,
                // cols 5→10 (side-aware: 5¼ starts left of col 5, 10¾ ends
                // right of col 10) to window pixels.
                let (down, up) = vcx.update(|window, cx| {
                    let bounds = session
                        .read(cx)
                        .content_bounds
                        .get()
                        .expect("painted bounds");
                    let m = super::element::Metrics::new(window, cx);
                    let (w, h) = (f32::from(m.cell_width), f32::from(m.line_height));
                    let at = |col: f32| {
                        gpui_kit::point(
                            bounds.origin.x + gpui_kit::px(col * w),
                            bounds.origin.y + gpui_kit::px(2.5 * h),
                        )
                    };
                    (at(5.25), at(10.75))
                });
                vcx.simulate_mouse_down(
                    down,
                    gpui_kit::MouseButton::Left,
                    gpui_kit::Modifiers::none(),
                );
                vcx.simulate_mouse_move(
                    up,
                    gpui_kit::MouseButton::Left,
                    gpui_kit::Modifiers::none(),
                );
                vcx.simulate_mouse_up(up, gpui_kit::MouseButton::Left, gpui_kit::Modifiers::none());

                let copied =
                    vcx.update(|_, cx| session.read(cx).grid.term.lock().selection_to_string());
                assert_eq!(copied.as_deref(), Some("-abcde"));

                shutdown(&session, cx);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// A drag is not bounded by one screen. Held past the top edge the
    /// viewport crawls through the scrollback — one line per tick with no
    /// further pointer motion, because a *held* pointer is the case a
    /// per-event scroll cannot serve — and the selection grows with it.
    ///
    /// The last leg is the one no element listener can do: the pointer
    /// leaves the pane entirely (upward, still inside the window) and the
    /// drag keeps going. gpui gates an element's own `on_mouse_move` on
    /// the pointer hovering that element, so before the element's raw
    /// window-level listener existed, both the crawl and this leg simply
    /// froze where the pane ended — that was "cannot select more than one
    /// screen of text".
    #[test]
    fn a_drag_past_the_panes_edge_selects_more_than_a_screen() {
        struct PaneRoot {
            term: Entity<TermSession>,
            focus: FocusHandle,
        }
        impl Render for PaneRoot {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                let weak = self.term.downgrade();
                div()
                    .size_full()
                    // Room above the pane, so a drag can leave it upward
                    // while staying inside the window.
                    .child(div().h(px(120.)))
                    .child(
                        div()
                            .h(px(300.))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let weak = weak.clone();
                                    move |_, event: &gpui_kit::MouseDownEvent, window, cx| {
                                        if let Some(term) = weak.upgrade() {
                                            term.update(cx, |s, cx| {
                                                if let Some((cell, side)) =
                                                    s.cell_at(event.position, window, cx)
                                                {
                                                    s.begin_selection(cell, side, 1, cx);
                                                }
                                            });
                                        }
                                    }
                                }),
                            )
                            // Both halves of the app pane's release wiring:
                            // inside the pane, and the outside one that
                            // catches a drag released past its edge.
                            .on_mouse_up(
                                MouseButton::Left,
                                cx.listener({
                                    let weak = weak.clone();
                                    move |_, _, _, cx| {
                                        if let Some(term) = weak.upgrade() {
                                            term.update(cx, |s, cx| s.end_selection(cx));
                                        }
                                    }
                                }),
                            )
                            .on_mouse_up_out(
                                MouseButton::Left,
                                cx.listener({
                                    let weak = weak.clone();
                                    move |_, _, _, cx| {
                                        if let Some(term) = weak.upgrade() {
                                            term.update(cx, |s, cx| s.end_selection(cx));
                                        }
                                    }
                                }),
                            )
                        .child(TermSession::element(weak, self.focus.clone())),
                    )
            }
        }

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(
                    dispatcher,
                    Some("a_drag_past_the_panes_edge_selects_more_than_a_screen"),
                );
                let cx = &mut cx0;
                let session = cx.update(spawn_cat);
                // 120 numbered lines: the grid shows the tail (line09x+)
                // and the rest is scrollback.
                cx.update(|cx| plant_lines(&session, cx, 120));

                let session2 = session.clone();
                let (_view, mut vcx) = cx.add_window_view(move |_, cx| PaneRoot {
                    term: session2,
                    focus: cx.focus_handle(),
                });

                // A rendered frame stashed the element's bounds. Anchor on
                // a visible row, well inside the grid; `above` is still
                // inside the pane (its padding), `away` is outside the
                // pane's own box.
                let (anchor, above, away) = vcx.update(|window, cx| {
                    let bounds = session
                        .read(cx)
                        .content_bounds
                        .get()
                        .expect("painted bounds");
                    let m = super::element::Metrics::new(window, cx);
                    let (w, h) = (f32::from(m.cell_width), f32::from(m.line_height));
                    let x = bounds.origin.x + px(3.5 * w);
                    (
                        point(x, bounds.origin.y + px(8.5 * h)),
                        point(x, bounds.origin.y - px(4.)),
                        point(x, px(40.)),
                    )
                });

                let offset = |vcx: &mut gpui_kit::VisualTestContext| {
                    vcx.update(|_, cx| {
                        session.read(cx).grid.term.lock().grid().display_offset()
                    })
                };
                let copied = |vcx: &mut gpui_kit::VisualTestContext| {
                    vcx.update(|_, cx| {
                        session.read(cx).grid.term.lock().selection_to_string()
                    })
                };
                let tick = |vcx: &mut gpui_kit::VisualTestContext, ticks: usize| {
                    for _ in 0..ticks {
                        vcx.executor().advance_clock(DRAG_SCROLL_TICK);
                        vcx.run_until_parked();
                    }
                };
                let double = |s: &str| s.lines().count() * 2;

                vcx.simulate_mouse_down(anchor, MouseButton::Left, Modifiers::none());
                // A drag inside the content scrolls nothing, however long
                // the pointer is held there.
                tick(&mut vcx, 5);
                assert_eq!(offset(&mut vcx), 0, "a drag inside the grid scrolls nothing");

                // Past the top edge — still over the pane — the viewport
                // crawls and the selection follows it into the scrollback.
                vcx.simulate_mouse_move(above, MouseButton::Left, Modifiers::none());
                tick(&mut vcx, 40);
                let rows = vcx.update(|_, cx| {
                    let s = session.read(cx);
                    let (_, rows) = s.grid.size();
                    rows as usize
                });
                let end = offset(&mut vcx);
                assert!(
                    end >= 35,
                    "a held drag keeps crawling through the scrollback (offset {end})"
                );
                let text = copied(&mut vcx).expect("a live selection copies");
                assert!(
                    double(&text) > rows,
                    "the swept run is more than one screen wide ({rows} rows): {:?}",
                    text
                );

                // The pointer leaves the pane: only the raw window-level
                // listener sees this, and the drag must keep going.
                vcx.simulate_mouse_move(away, MouseButton::Left, Modifiers::none());
                tick(&mut vcx, 20);
                assert!(
                    offset(&mut vcx) > end,
                    "a drag beyond the pane keeps scrolling"
                );

                // Releasing ends the drag *and* the ticker: a stray timer
                // would keep the viewport drifting under the user.
                let end = offset(&mut vcx);
                vcx.simulate_mouse_up(away, MouseButton::Left, Modifiers::none());
                tick(&mut vcx, 10);
                assert_eq!(offset(&mut vcx), end, "the ticker stops with the drag");

                shutdown(&session, cx);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
