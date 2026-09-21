//! The grid's overlay scrollbar: geometry shared with the painter, the
//! macOS-style show/hide, and the thumb/page drags. Both halves live
//! here — the painting rects and the hit handling — so the session and
//! the element can never disagree about where the thumb is.

use std::time::Instant;

use alacritty_terminal::grid::Dimensions as _;

use gpui_kit::*;

use super::*;

/// How long the overlay thumb stays up after the last scroll/hover
/// activity (same 2s hold the Base scrollbars in the diff panes use).
const SCROLLBAR_IDLE: Duration = Duration::from_secs(2);
/// Right-edge scrollbar strip width — the mouse hit zone only; the
/// thumb is drawn centered in the 16px bar zone the Base scrollbars
/// (diff panes) use, so both look identical side by side.
pub(crate) const SCROLLBAR_W: f32 = 6.;
/// Minimum thumb height so short scrollback stays grabbable (Base's
/// `MIN_THUMB_SIZE`).
const THUMB_MIN: f32 = 48.;
/// Thumb width at rest / while hovered or dragged, with the
/// right-edge insets that keep it centered in the 16px bar zone.
const THUMB_W: f32 = 6.;
const THUMB_W_ACTIVE: f32 = 8.;
const THUMB_EDGE_INSET: f32 = 5.;
const THUMB_EDGE_INSET_ACTIVE: f32 = 4.;
/// Scrollbar track + thumb rects for the grid area, or None when there
/// is no scrollback. `engaged` (hover or drag) widens the thumb from
/// 6px to 8px like the Base scrollbar; `display_offset` 0 pins it to
/// the bottom (live), `history` to the top.
pub fn scrollbar_geometry(
    area: Bounds<Pixels>,
    screen_lines: usize,
    history: usize,
    display_offset: usize,
    engaged: bool,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
    if history == 0 {
        return None;
    }
    let track = Bounds {
        origin: point(
            area.origin.x + area.size.width - px(SCROLLBAR_W),
            area.origin.y,
        ),
        size: size(px(SCROLLBAR_W), area.size.height),
    };
    let thumb_h = (f32::from(track.size.height) * screen_lines as f32
        / (screen_lines + history) as f32)
        .max(THUMB_MIN);
    let travel = (f32::from(track.size.height) - thumb_h).max(0.);
    let frac = display_offset.min(history) as f32 / history as f32;
    let top = track.origin.y + px(travel * (1. - frac));
    let (w, inset) = if engaged {
        (THUMB_W_ACTIVE, THUMB_EDGE_INSET_ACTIVE)
    } else {
        (THUMB_W, THUMB_EDGE_INSET)
    };
    let thumb = Bounds {
        origin: point(area.origin.x + area.size.width - px(inset + w), top),
        size: size(px(w), px(thumb_h)),
    };
    Some((track, thumb))
}

impl TermSession {

    /// Overlay-scrollbar visibility, macOS-style: visible while the
    /// mouse hovers the right-edge strip or while a drag / recent scroll
    /// activity is live; fades out once the idle window closes — even
    /// when the viewport sits in the scrollback (standard overlay
    /// behavior: position is re-shown by the next scroll tick).
    pub fn scrollbar_visible(&self) -> bool {
        self.scrollbar_activity()
    }

    /// Hover/drag/recent-scroll part of the visibility test — no term
    /// mutex, so the painter (which holds the lock) can call this.
    pub(crate) fn scrollbar_activity(&self) -> bool {
        self.scrollbar_drag.is_some()
            || self.scrollbar_hover
            || self
                .scrollbar_until
                .is_some_and(|until| Instant::now() < until)
    }


    /// Hover or drag specifically — the state that widens the thumb.
    /// The idle timer alone must not (it would stay wide until the
    /// hold expires).
    pub(crate) fn scrollbar_engaged(&self) -> bool {
        self.scrollbar_drag.is_some() || self.scrollbar_hover
    }

    /// Scroll activity happened: keep the thumb up for another idle
    /// window.
    pub(super) fn reveal_scrollbar(&mut self, cx: &mut Context<Self>) {
        let was = self.scrollbar_look();
        self.scrollbar_until = Some(Instant::now() + SCROLLBAR_IDLE);
        self.arm_scrollbar_hide(cx);
        self.wake_overlay_change(was, cx);
    }

    /// One-shot repaint at the end of the idle window — without it
    /// nothing re-rendered at the deadline and the thumb lingered until
    /// an unrelated frame. A reveal during the wait re-arms for the new
    /// deadline; hover/drag keep re-arming while they last.
    fn arm_scrollbar_hide(&mut self, cx: &mut Context<Self>) {
        if self.scrollbar_hide_armed {
            return;
        }
        self.scrollbar_hide_armed = true;
        let wait = self
            .scrollbar_until
            .map(|until| until.saturating_duration_since(Instant::now()))
            .unwrap_or(SCROLLBAR_IDLE)
            // A lapsed deadline with the mouse still parked means hover
            // keeps the bar up: poll again, never busy-loop.
            .max(Duration::from_millis(250));
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |term, cx| {
                term.scrollbar_hide_armed = false;
                if term.scrollbar_activity() {
                    term.arm_scrollbar_hide(cx);
                    // Still up (hover or a live drag): the deadline
                    // moved, the frame did not.
                    return;
                }
                // The idle window closed with the pointer away and no
                // drag: this frame is where the thumb goes out, and no
                // later one asks for it.
                cx.emit(TermEvent::Wakeup);
            });
        })
        .detach();
    }

    /// Mouse over the right-edge strip (hover keeps the thumb up). The
    /// pointer is *anywhere* in the window here — the element's raw
    /// motion listener is the caller, so leaving the strip — or the
    /// whole pane — clears the hover instead of leaving the thumb up
    /// for good (an element-gated `on_mouse_move` never sees the exit).
    pub fn scrollbar_hover_at(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let was = self.scrollbar_look();
        let hovered = self
            .scrollbar_geometry()
            .map(|(track, _)| {
                let mut hit = track;
                hit.origin.x -= px(4.);
                hit.size.width += px(8.);
                hit.contains(&pos)
            })
            .unwrap_or(false);
        self.scrollbar_hover = hovered;
        if hovered {
            self.scrollbar_until = Some(Instant::now() + SCROLLBAR_IDLE);
            self.arm_scrollbar_hide(cx);
        }
        self.wake_overlay_change(was, cx);
    }

    /// What the overlay looks like this frame: up or not, and widened
    /// (hovered/dragged) or not. Anything that moves this needs a frame
    /// of its own.
    fn scrollbar_look(&self) -> (bool, bool) {
        (self.scrollbar_visible(), self.scrollbar_engaged())
    }

    /// Repaint the pane when the overlay's look changed. A visibility
    /// flip is invisible to `cx.notify()` (see the crate docs: a
    /// session is not a view, so no view is marked dirty and the cached
    /// pane replays its old frame) — `Wakeup` is the shell's repaint
    /// signal, and the two callers above are the reveal, the hover
    /// change and the idle fade.
    fn wake_overlay_change(&mut self, was: (bool, bool), cx: &mut Context<Self>) {
        if was != self.scrollbar_look() {
            cx.emit(TermEvent::Wakeup);
        }
    }

    pub fn scrollbar_geometry(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let bounds = self.grid_bounds.get()?;
        let (_cols, rows) = self.grid.size();
        let (history, offset) = {
            let term = self.grid.term.lock();
            (term.grid().history_size(), term.grid().display_offset())
        };
        // Full element height, flush with both ends — same rect the
        // element paints against.
        scrollbar_geometry(bounds, rows as usize, history, offset, self.scrollbar_engaged())
    }

    /// Left button down on the scrollbar strip: on the thumb starts a
    /// drag, on the bare track pages up/down. True = event consumed.
    /// A hidden (auto-hidden) scrollbar never intercepts — the click
    /// falls through to text selection.
    pub fn scrollbar_mouse_down(
        &mut self,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.scrollbar_visible() {
            return false;
        }
        let Some((track, thumb)) = self.scrollbar_geometry() else {
            return false;
        };
        self.reveal_scrollbar(cx);
        if thumb.contains(&pos) {
            self.scrollbar_drag = Some(f32::from(pos.y - thumb.origin.y));
        } else if track.contains(&pos) {
            let (_cols, rows) = self.grid.size();
            let page = rows as i32 - 1;
            self.grid
                .scroll(if pos.y < thumb.origin.y { page } else { -page });
            cx.emit(TermEvent::Wakeup);
        } else {
            return false;
        }
        true
    }

    /// Drag the thumb to the pointer's scroll fraction. True while a
    /// scrollbar drag is active (the caller then skips selection).
    pub fn scrollbar_mouse_drag(
        &mut self,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(grab) = self.scrollbar_drag else {
            return false;
        };
        let Some((track, thumb)) = self.scrollbar_geometry() else {
            return false;
        };
        self.reveal_scrollbar(cx);
        let travel = f32::from(track.size.height - thumb.size.height);
        let frac = if travel <= 0. {
            0.
        } else {
            1. - ((f32::from(pos.y) - grab - f32::from(track.origin.y)) / travel).clamp(0., 1.)
        };
        let history = self.grid.term.lock().grid().history_size() as f32;
        let target = (frac * history).round() as i32;
        let current = self.grid.term.lock().grid().display_offset() as i32;
        if target != current {
            self.grid.scroll(target - current);
            cx.emit(TermEvent::Wakeup);
        }
        true
    }

    /// End any scrollbar drag (mouse up anywhere). Only a real drag
    /// keeps the thumb up afterwards: the app wires this to both the
    /// pane's mouse up and its mouse-up-outside, and a click that had
    /// nothing to do with the strip (anywhere in the window) must not
    /// reveal the overlay.
    pub fn scrollbar_mouse_up(&mut self, cx: &mut Context<Self>) {
        if self.scrollbar_drag.take().is_some() {
            self.reveal_scrollbar(cx);
        }
    }
}

#[cfg(test)]
mod scrollbar_tests {
    // Selective imports only, for the reason every terminal test module
    // states: a glob of the gpui prelude would bring gpui's `test`
    // attribute into scope, which expands into itself.
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use super::{SCROLLBAR_W, scrollbar_geometry};
    use crate::harness::{TestRoot, plant_lines, shutdown, spawn_cat};
    use crate::TermSession;
    use gpui_kit::{
        AnyWindowHandle, App, AppContext as _, Bounds, Entity, Modifiers, Pixels, Point,
        TestAppContext, gpui, point, px, size,
    };

    fn area() -> Bounds<gpui_kit::Pixels> {
        Bounds {
            origin: point(px(10.), px(10.)),
            size: size(px(500.), px(240.)),
        }
    }

    #[test]
    fn no_scrollback_no_scrollbar() {
        assert!(scrollbar_geometry(area(), 24, 0, 0, false).is_none());
    }

    #[test]
    fn thumb_tracks_the_scroll_fraction() {
        // 24 rows visible, 60 in scrollback: thumb = 240·24/84 ≈ 68.6px
        // (history small enough to stay above the 48px minimum).
        let (track, bottom) = scrollbar_geometry(area(), 24, 60, 0, false).unwrap();
        let (_, top) = scrollbar_geometry(area(), 24, 60, 60, false).unwrap();
        let thumb_h = f32::from(bottom.size.height);
        assert!((thumb_h - 240. * 24. / 84.).abs() < 0.5);
        // Live bottom (offset 0) pins the thumb to the track bottom...
        assert!((f32::from(bottom.origin.y) - (10. + 240. - thumb_h)).abs() < 0.5);
        // ...and full history (offset == history) to the track top.
        assert!((f32::from(top.origin.y) - 10.).abs() < 0.01);
        // The hit-test strip hugs the area's right edge...
        assert!((f32::from(track.origin.x) - (10. + 500. - SCROLLBAR_W)).abs() < 0.01);
        // ...while the resting thumb is centered in the 16px bar zone
        // (6px wide, 5px off the edge).
        assert!((f32::from(bottom.origin.x) - (10. + 500. - 5. - 6.)).abs() < 0.01);
    }

    #[test]
    fn engaged_thumb_widens_toward_the_edge() {
        let (_, engaged) = scrollbar_geometry(area(), 24, 100, 0, true).unwrap();
        assert!((f32::from(engaged.size.width) - 8.).abs() < 0.01);
        assert!((f32::from(engaged.origin.x) - (10. + 500. - 4. - 8.)).abs() < 0.01);
    }

    #[test]
    fn huge_scrollback_keeps_grabbable_thumb() {
        let (_, thumb) = scrollbar_geometry(area(), 24, 100_000, 50_000, false).unwrap();
        assert_eq!(f32::from(thumb.size.height), 48.);
    }

    // ── the overlay's repaint contract ────────────────────────────────
    //
    // The pane is mounted `Entity::cached`, and the session is not a
    // view: `cx.notify()` on it marks no view dirty, so the pane replays
    // its recorded frame and the thumb neither appears nor goes away.
    // `TermEvent::Wakeup` is what the shell turns into that repaint
    // (`AppView::subscribe_term`), and these two tests pin every flip of
    // the overlay's look to it.

    /// Bounds a paint would have stashed, with scrollback behind them
    /// (no history, no scrollbar).
    fn painted(session: &Entity<TermSession>, cx: &mut App) -> (Point<Pixels>, Point<Pixels>) {
        session.update(cx, |s, _| {
            s.grid_bounds.set(Some(Bounds {
                origin: point(px(100.), px(50.)),
                size: size(px(800.), px(500.)),
            }));
        });
        plant_lines(session, cx, 100);
        // The strip (`SCROLLBAR_W` at the right edge, hit-tested ±4px),
        // and a point well inside the pane.
        (
            point(px(100. + 800. - SCROLLBAR_W / 2.), px(300.)),
            point(px(400.), px(300.)),
        )
    }

    /// The panes' cached-frame rule reaches the overlay too: hovering the
    /// strip is a *render* change with no stream behind it, so without
    /// the wakeup the thumb simply never appears (measured on the running
    /// app before this: four seconds of hovering the strip drew nothing).
    #[test]
    fn the_overlay_wakes_the_pane_when_its_look_changes() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(
                    dispatcher,
                    Some("the_overlay_wakes_the_pane_when_its_look_changes"),
                );
                let cx = &mut cx0;
                let window = cx.add_window(|_, _| TestRoot);
                let window = AnyWindowHandle::from(window);
                let session = cx.update(spawn_cat);
                let (strip, inside) = cx.update(|cx| painted(&session, cx));

                let wakeups = Rc::new(Cell::new(0usize));
                cx.update(|cx| {
                    let wakeups = wakeups.clone();
                    cx.subscribe(&session, move |_, event: &crate::TermEvent, _| {
                        if *event == crate::TermEvent::Wakeup {
                            wakeups.set(wakeups.get() + 1);
                        }
                    })
                    .detach();
                });
                let hover = |cx: &mut TestAppContext, pos| {
                    cx.update_window(window, |_, _, cx| {
                        session.update(cx, |s, cx| s.scrollbar_hover_at(pos, cx));
                    })
                    .unwrap();
                };
                // (visible, engaged, hovered)
                let look = |cx: &mut TestAppContext| {
                    cx.update(|cx| {
                        let s = session.read(cx);
                        (
                            s.scrollbar_visible(),
                            s.scrollbar_engaged(),
                            s.scrollbar_hover,
                        )
                    })
                };

                // On the strip: up *and* widened, on one frame.
                hover(cx, strip);
                assert_eq!(wakeups.get(), 1, "the reveal needs a frame of its own");
                assert_eq!(look(cx), (true, true, true));

                // Same hover again (the pointer keeps moving over the
                // strip): the look did not change, so no frame is owed.
                hover(cx, strip);
                assert_eq!(wakeups.get(), 1, "a re-hover repaints nothing");

                // Off the strip but still inside the pane: the thumb drops
                // back to its resting width, and only a *drag* would hold
                // it up.
                hover(cx, inside);
                assert_eq!(wakeups.get(), 2, "the un-widen needs a frame");
                assert_eq!(
                    look(cx),
                    (true, false, false),
                    "recent scroll activity holds it up, back at rest, for the idle window"
                );

                // The idle window closes with the pointer away: the frame
                // this one-shot timer asks for is the only one that takes
                // the thumb out, so it must be a wakeup.
                cx.update(|cx| {
                    session.update(cx, |s, _| s.scrollbar_until = Some(Instant::now()));
                });
                cx.executor().advance_clock(Duration::from_secs(3));
                cx.run_until_parked();
                assert_eq!(wakeups.get(), 3, "the fade-out needs a frame");
                assert_eq!(look(cx).0, false, "the idle window closed: the thumb is out");

                shutdown(&session, cx);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// The hover follows the pointer *outside* the pane. The pane's own
    /// element listener only sees motion while the pointer hovers it, so
    /// the flag used to stay set the moment the pointer left the pane —
    /// and the thumb stayed up, widened, until some later reveal reset
    /// the deadline (that is what the element's raw window-level listener
    /// is for).
    #[test]
    fn the_hover_clears_when_the_pointer_leaves_the_pane() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(
                    dispatcher,
                    Some("the_hover_clears_when_the_pointer_leaves_the_pane"),
                );
                let cx = &mut cx0;
                let window = cx.add_window(|_, _| TestRoot);
                let window = AnyWindowHandle::from(window);
                let session = cx.update(spawn_cat);
                let (strip, _) = cx.update(|cx| painted(&session, cx));
                // The pointer left the pane: over the sidebar, say.
                let away = point(px(20.), px(300.));

                let moved = |cx: &mut TestAppContext, pos| {
                    cx.update_window(window, |_, window, cx| {
                        session.update(cx, |s, cx| {
                            s.pointer_moved(pos, &Modifiers::none(), window, cx);
                        });
                    })
                    .unwrap();
                };

                let look = |cx: &mut TestAppContext| {
                    cx.update(|cx| {
                        let s = session.read(cx);
                        (s.scrollbar_hover, s.scrollbar_engaged())
                    })
                };

                moved(cx, strip);
                assert_eq!(look(cx), (true, true));

                moved(cx, away);
                assert_eq!(
                    look(cx),
                    (false, false),
                    "the pointer is not on the strip any more"
                );

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
