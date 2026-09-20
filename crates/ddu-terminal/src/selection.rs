//! Text selection over the grid: window px → cell mapping, the drag
//! (semantic runs on a double click) and the clipboard copy.

use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use gpui_kit::*;

use super::*;

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

    /// End the drag; a plain click (empty selection) clears the wash.
    pub fn end_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
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
    use crate::harness::{TestRoot, shutdown, spawn_cat};
    use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
    use alacritty_terminal::selection::{Selection, SelectionType};
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
        ParentElement as _, Render, Styled as _, TestAppContext, Window, div, gpui,
    };

    use super::TermSession;



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
}
