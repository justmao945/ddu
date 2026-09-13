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
pub(crate) fn scrollbar_geometry(
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
    pub(crate) fn scrollbar_visible(&self) -> bool {
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
        self.scrollbar_until = Some(Instant::now() + SCROLLBAR_IDLE);
        self.arm_scrollbar_hide(cx);
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
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Mouse over the right-edge strip (hover keeps the thumb up).
    pub(crate) fn scrollbar_hover_at(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let hovered = self
            .scrollbar_geometry()
            .map(|(track, _)| {
                let mut hit = track;
                hit.origin.x -= px(4.);
                hit.size.width += px(8.);
                hit.contains(&pos)
            })
            .unwrap_or(false);
        let changed = self.scrollbar_hover != hovered;
        self.scrollbar_hover = hovered;
        if hovered {
            self.reveal_scrollbar(cx);
        }
        if changed {
            cx.notify();
        }
    }

    pub(crate) fn scrollbar_geometry(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
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
    pub(crate) fn scrollbar_mouse_down(
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
    pub(crate) fn scrollbar_mouse_drag(
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

    /// End any scrollbar drag (mouse up anywhere).
    pub(crate) fn scrollbar_mouse_up(&mut self, cx: &mut Context<Self>) {
        self.scrollbar_drag = None;
        self.reveal_scrollbar(cx);
    }
}

#[cfg(test)]
mod scrollbar_tests {
    use super::{SCROLLBAR_W, scrollbar_geometry};
    use gpui_kit::{Bounds, point, px, size};

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
}
