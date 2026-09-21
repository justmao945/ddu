//! Stream repaint pacing: the pump paints a burst's first wakeup at once
//! and the rest at most once per interval, stretched while a frame's
//! terminal paint is expensive (the EWMA [`TermSession::note_paint_cost`]
//! writes).

use std::time::Duration;

use super::*;

/// Minimum spacing between stream-driven repaints. A flooding child
/// emits a wakeup per read chunk, and each one would repaint the whole
/// window at display-link rate — during a stream the main thread
/// spends a large share of its time in `Window::draw` (measured ~40%
/// under a 5 MB/s flood). The pump paints the first wakeup of a burst
/// immediately and the rest at most once per interval, flushed by a
/// trailing timer so the burst still ends on the freshest frame.
/// Idle wakeups (a keystroke echo) find the window elapsed and repaint
/// immediately; interaction-driven repaints (scroll/select/paste) are
/// emitted from entity methods and never pass through this throttle.
///
/// 20 fps: a stream is text nobody reads character-by-character, and
/// both CPU and GPU scale with frames drawn (gpui repaints every
/// primitive each frame). What the eye actually follows — the spinner
/// in the pane, and the sidebar row that mirrors the same OSC title —
/// moves at 20 fps with it and reads as smooth; the sidebar's *jerkier*
/// case was never this number, it was the row not being notified at all
/// (see `AppView::subscribe_term`). Keystrokes, scrolling and selection
/// bypass this throttle entirely, so nothing interactive is capped.
pub const STREAM_FRAME_MIN: Duration = Duration::from_millis(50);
/// Interval the stream throttle stretches to when a frame's terminal
/// paint is expensive (see [`stream_interval`]).
const STREAM_FRAME_MAX: Duration = Duration::from_millis(100);
/// Paint cost (ms, per frame) at which the interval takes its next step,
/// paired with the interval it steps to: 15 fps past 4 ms of terminal
/// paint, 10 fps past 9 ms.
///
/// Sizing: this element's paint is roughly 40% of a window redraw, so
/// the steps only engage when a frame costs ~10 ms and up — a window
/// several times the size of the measured one, or a machine already
/// loaded. The thresholds used to start at 2.5 ms, which is *below* what
/// a full-screen TUI repaint costs on a 1400×900 window (p50 1.8 ms,
/// p90 3.6 ms): every agent turn sat pinned at 15 fps, i.e. *below* the
/// floor, which is what made the pane look worse than the floor implies.
const STREAM_FRAME_STEPS: [(f32, Duration); 2] =
    [(4., Duration::from_millis(66)), (9., STREAM_FRAME_MAX)];
/// Spacing between stream repaints for a frame whose terminal paint
/// costs `paint_ms` (a slow EWMA, see [`TermSession::note_paint_cost`]):
/// the floor while paint is cheap, then one step down per entry in
/// [`STREAM_FRAME_STEPS`]. The paint cost of a frame does not depend on
/// the interval, so the level is stable.
pub(super) fn stream_interval(paint_ms: f32) -> Duration {
    STREAM_FRAME_STEPS
        .iter()
        .rev()
        .find(|(cost, _)| paint_ms >= *cost)
        .map_or(STREAM_FRAME_MIN, |(_, interval)| *interval)
}

impl TermSession {

    /// Record one frame's terminal paint cost — the input to the stream
    /// throttle's interval ([`stream_interval`]). A slow EWMA: a single
    /// slow frame (a font fallback raster, a scheduler hiccup) must not
    /// re-pace the stream, and wall-clock cost is the right signal — a
    /// frame that took long because the main thread was descheduled is
    /// exactly a frame worth drawing less often.
    pub(crate) fn note_paint_cost(&self, cost: Duration) {
        let ms = cost.as_secs_f32() * 1000.;
        let prev = self.stream_paint_ms.get();
        self.stream_paint_ms
            .set(if prev == 0. { ms } else { prev * 0.75 + ms * 0.25 });
    }
}

#[cfg(test)]
mod tests {
    // Selective imports only: `use super::*` would pull gpui's `test`
    // proc-macro re-export into scope, shadowing the built-in `#[test]`
    // and recursing forever at expansion.
    use crate::harness::{shutdown, spawn_cat};
    use gpui_kit::{TestAppContext, gpui};



    /// Stream repaint throttle: a burst of output wakeups inside one
    /// frame window repaints once immediately plus one trailing flush,
    /// not once per chunk; after the window passes the next wakeup
    /// repaints immediately. Driven through `inject_bytes` (same
    /// grid→channel path as the reader thread) with the pump's
    /// executor clock faked, so the timing is exact. `cat` stays
    /// silent — no reader-thread wakeups race the test scheduler.
    #[test]
    fn stream_repaints_are_throttled() {
        use super::STREAM_FRAME_MIN;
        use std::cell::Cell;
        use std::rc::Rc;
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("stream_repaints_are_throttled"));
                let cx = &mut cx0;
                let session = cx.update(spawn_cat);
                // Repaints are counted as `Wakeup`s, which is the signal
                // the shell repaints the pane on (`AppView::subscribe_term`):
                // a `cx.notify()` on the session marks no view dirty, so
                // observing *that* counted a call that painted nothing.
                let paints = Rc::new(Cell::new(0usize));
                cx.update(|cx| {
                    let paints = paints.clone();
                    cx.subscribe(&session, move |_, event: &crate::TermEvent, _| {
                        if *event == crate::TermEvent::Wakeup {
                            paints.set(paints.get() + 1);
                        }
                    })
                    .detach();
                });
                cx.run_until_parked();
                paints.set(0);

                // Eight chunks in one frame window (drained one by one:
                // the capacity-1 wake channel coalesces a burst the pump
                // never gets to between sends): the first repaints now,
                // the other seven coalesce behind one flush.
                for _ in 0..8 {
                    cx.update(|cx| session.read(cx).inject_bytes(b"line\r\n"));
                    cx.run_until_parked();
                }
                assert_eq!(paints.get(), 1, "burst repaints once immediately");

                cx.executor().advance_clock(STREAM_FRAME_MIN);
                cx.run_until_parked();
                assert_eq!(paints.get(), 2, "trailing flush repaints the burst tail");

                // Past the window the next chunk repaints immediately.
                cx.executor().advance_clock(STREAM_FRAME_MIN);
                cx.update(|cx| session.read(cx).inject_bytes(b"later\r\n"));
                cx.run_until_parked();
                assert_eq!(paints.get(), 3, "idle wakeup repaints immediately");

                shutdown(&session, cx);
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }


    /// The interval steps with the cost of a frame's terminal paint: a
    /// cheap frame keeps the 20 fps floor, an expensive one backs the
    /// stream off (CPU and GPU both scale with frames drawn).
    #[test]
    fn stream_interval_steps_with_frame_cost() {
        use super::{STREAM_FRAME_MAX, STREAM_FRAME_MIN, STREAM_FRAME_STEPS, stream_interval};
        assert_eq!(stream_interval(0.), STREAM_FRAME_MIN, "unmeasured: floor");
        assert_eq!(
            stream_interval(STREAM_FRAME_STEPS[0].0 - 0.1),
            STREAM_FRAME_MIN,
            "cheap frame: floor"
        );
        // What a full-screen TUI repaint costs on a normal window
        // (measured p50 1.8 ms / p90 3.6 ms at 1400×900): the floor, or
        // every agent turn stutters at 15 fps like it used to.
        assert_eq!(
            stream_interval(3.6),
            STREAM_FRAME_MIN,
            "a full-screen repaint keeps the floor"
        );
        assert_eq!(
            stream_interval(STREAM_FRAME_STEPS[0].0),
            STREAM_FRAME_STEPS[0].1
        );
        assert_eq!(
            stream_interval(STREAM_FRAME_STEPS[1].0),
            STREAM_FRAME_STEPS[1].1
        );
        assert_eq!(stream_interval(40.), STREAM_FRAME_MAX, "clamped at the ceiling");
    }


    /// Expensive frames stretch the stream interval: the same burst that
    /// flushes at the floor must not flush until the stretched
    /// interval has passed. Paced through the pump's faked executor
    /// clock, like [`stream_repaints_are_throttled`].
    #[test]
    fn expensive_frames_stretch_the_stream_interval() {
        use std::cell::Cell;
        use std::rc::Rc;
        use std::time::Duration;
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(
                    dispatcher,
                    Some("expensive_frames_stretch_the_stream_interval"),
                );
                let cx = &mut cx0;
                let session = cx.update(spawn_cat);
                // Repaints are counted as `Wakeup`s, which is the signal
                // the shell repaints the pane on (`AppView::subscribe_term`):
                // a `cx.notify()` on the session marks no view dirty, so
                // observing *that* counted a call that painted nothing.
                let paints = Rc::new(Cell::new(0usize));
                cx.update(|cx| {
                    let paints = paints.clone();
                    cx.subscribe(&session, move |_, event: &crate::TermEvent, _| {
                        if *event == crate::TermEvent::Wakeup {
                            paints.set(paints.get() + 1);
                        }
                    })
                    .detach();
                });
                cx.run_until_parked();
                paints.set(0);

                // A paint at the first step's cost seeds the EWMA one
                // step down: the interval becomes 66 ms, not the 50 ms
                // floor.
                cx.update(|cx| {
                    session
                        .read(cx)
                        .note_paint_cost(Duration::from_secs_f32(super::STREAM_FRAME_STEPS[0].0 / 1000.))
                });

                for _ in 0..4 {
                    cx.update(|cx| session.read(cx).inject_bytes(b"line\r\n"));
                    cx.run_until_parked();
                }
                assert_eq!(paints.get(), 1, "burst repaints once immediately");

                cx.executor().advance_clock(super::STREAM_FRAME_MIN);
                cx.run_until_parked();
                assert_eq!(
                    paints.get(),
                    1,
                    "the floor must not flush a stretched interval"
                );

                cx.executor()
                    .advance_clock(super::STREAM_FRAME_STEPS[0].1 - super::STREAM_FRAME_MIN);
                cx.run_until_parked();
                assert_eq!(paints.get(), 2, "the stretched interval flushes its tail");

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
