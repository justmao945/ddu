//! IME wiring: the terminal is not a text document, so only the two
//! mutation entry points do something — a commit goes straight to the
//! PTY as UTF-8, a preedit (marked text) is stashed for the element to
//! paint underlined at the cursor. Everything read-shaped answers
//! "empty document", and the candidate popup anchors to the cursor rect
//! the last paint left behind.

use std::ops::Range;

use gpui_kit::*;

use super::*;

/// IME wiring: the terminal is not a text document, so only the two
/// mutation entry points do something — a commit goes straight to the
/// PTY as UTF-8, a preedit (marked text) is stashed for the element to
/// paint underlined at the cursor. Everything read-shaped answers
/// "empty document".
impl EntityInputHandler for TermSession {
    fn text_for_range(
        &mut self,
        _: Range<usize>,
        _: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    /// A caret at offset 0 of that empty document. The platform's IME
    /// placement pulls (`ime_candidate_bounds` / `selected_bounds`)
    /// give up when there is no selection, so answering `None` leaves
    /// the candidate popup unanchored: on Wayland no
    /// `set_cursor_rectangle` is ever sent and fcitx5 keeps the popup
    /// wherever it last drew it. Reporting an empty selection instead
    /// routes those pulls into [`Self::bounds_for_range`], which answers
    /// with the grid cursor's cell.
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_text
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked_text.take().is_some() {
            cx.emit(TermEvent::Wakeup);
            cx.notify();
        }
    }

    /// IME commit: the finished string goes to the PTY as UTF-8.
    fn replace_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = None;
        if !text.is_empty() {
            self.write(text.as_bytes());
            self.scroll_to_bottom();
        }
        cx.emit(TermEvent::Wakeup);
        cx.notify();
    }

    /// IME preedit: remember for the underlined overlay at the cursor.
    fn replace_and_mark_text_in_range(
        &mut self,
        _: Option<Range<usize>>,
        new_text: &str,
        _: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!new_text.is_empty()).then(|| new_text.to_string());
        cx.emit(TermEvent::Wakeup);
        cx.notify();
    }

    /// Anchor the IME candidate popup at the terminal cursor.
    fn bounds_for_range(
        &mut self,
        _: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.ime_cursor_bounds.get().or(Some(element_bounds))
    }

    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

#[cfg(test)]
mod tests {
    // Selective imports only: `use super::*` would pull gpui's `test`
    // proc-macro re-export into scope, shadowing the built-in `#[test]`
    // and recursing forever at expansion.
    use std::time::{Duration, Instant};

    use crate::terminal::harness::{TestRoot, grid_text, spawn_cat};
    use gpui_kit::{AnyWindowHandle, AppContext as _, EntityInputHandler as _, TestAppContext, gpui};


    /// IME acceptance: preedit stays out of the PTY, a commit lands as
    /// UTF-8 bytes (a `cat` child echoes them back onto the grid), and
    /// unmark drops a cancelled composition.
    #[test]
    fn ime_commit_reaches_pty_as_utf8() {
        // `#[gpui::test]` is unusable here (see the import note), so
        // drive the same harness by hand.
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("ime_commit_reaches_pty_as_utf8"));
                let cx = &mut cx0;
                let window = cx.add_window(|_, _| TestRoot);
                let window = AnyWindowHandle::from(window);
                let session = cx.update(spawn_cat);

                cx.update_window(window, |_, window, cx| {
                    session.update(cx, |s, cx| {
                        s.replace_and_mark_text_in_range(None, "nihao", None, window, cx);
                        assert_eq!(s.marked_text_range(window, cx), Some(0..5));
                        s.replace_text_in_range(None, "你好", window, cx);
                        assert_eq!(s.marked_text_range(window, cx), None);

                        s.replace_and_mark_text_in_range(None, "x", None, window, cx);
                        assert_eq!(s.marked_text_range(window, cx), Some(0..1));
                        s.unmark_text(window, cx);
                        assert_eq!(s.marked_text_range(window, cx), None);
                    });
                })
                .unwrap();

                let deadline = Instant::now() + Duration::from_secs(4);
                let mut echoed = false;
                // Wide chars occupy two cells; the spacer cell reads as a
                // space, so compare with whitespace stripped.
                let compact =
                    |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
                while Instant::now() < deadline {
                    if compact(cx.update(|cx| grid_text(&session, cx))).contains("你好") {
                        echoed = true;
                        break;
                    }
                    // Real PTY + pump threads: poll like the grid tests do.
                    std::thread::sleep(Duration::from_millis(20));
                }
                cx.run_until_parked();
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
                assert!(
                    echoed,
                    "cat never echoed the committed IME text; grid: {:?}",
                    compact(cx.update(|cx| grid_text(&session, cx)))
                );
            }),
        );
    }
}
