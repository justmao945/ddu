//! Agent terminal stack: real PTY processes parsed into an alacritty
//! grid and painted by a custom element.
//!
//! * [`pty`]   — process + master handles (spawn/resize/kill)
//! * [`grid`]  — `Term` behind a `FairMutex` + the two pump threads
//! * [`input`] — keystroke → escape-sequence encoding
//! * [`element`] — the grid painter (custom `Element`)
//!
//! [`TermSession`] is the gpui entity tying it together: it owns the
//! grid and process, receives pump wakeups, and emits [`TermEvent`]
//! when the child exits.

mod element;
mod grid;
mod input;
mod pty;

use std::cell::Cell;
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui_kit::*;

pub use pty::PtySpawn;

/// Minimum gap between grid+PTY reflows while a drag is resizing.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(140);

/// Terminal events emitted to subscribers (the app shell).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermEvent {
    /// Grid changed (coalesced); subscribers should re-render.
    Wakeup,
    /// Child exited with the raw exit code (0 = success).
    Exit(i32),
}

pub struct TermSession {
    grid: grid::TermGrid,
    process: Option<pty::PtyProcess>,
    pub(crate) focus: FocusHandle,
    exit: Option<i32>,
    /// Grid/PTY resize target waiting for the debounce window to close.
    pending_resize: Option<(u16, u16)>,
    last_resize: Instant,
    ever_resized: bool,
    flush_scheduled: bool,
    scroll_remainder: f32,
    /// IME preedit ("marked") text: painted underlined at the cursor
    /// until the input method commits or cancels it.
    marked_text: Option<String>,
    /// Cursor rect in window coordinates from the last paint — the
    /// platform anchors the IME candidate popup to it.
    ime_cursor_bounds: Cell<Option<Bounds<Pixels>>>,
}

impl gpui_kit::EventEmitter<TermEvent> for TermSession {}

impl TermSession {
    /// Spawn `cmd` in a fresh PTY and start its pump threads. Initial
    /// grid size is 80×24; the element's prepaint resizes it to the
    /// panel on the first frame.
    pub fn spawn(cmd: &PtySpawn, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let (cols, rows) = (80, 24);
        let (wake_tx, wake_rx) = async_channel::bounded::<grid::PumpMsg>(1);
        let (grid, process) = grid::spawn_session(cmd, cols, rows, wake_tx, gpui_kit::component::theme::Theme::global(cx).is_dark())?;

        let entity = cx.new(|cx| {
            cx.observe_global::<gpui_kit::component::theme::Theme>(|this: &mut Self, cx| {
                this.grid.dark.store(gpui_kit::component::theme::Theme::global(cx).is_dark(), std::sync::atomic::Ordering::Relaxed);
            }).detach();
            Self {
            grid,
            process: Some(process),
            focus: cx.focus_handle().tab_stop(false),
            exit: None,
            pending_resize: None,
            last_resize: Instant::now(),
            ever_resized: false,
            flush_scheduled: false,
            scroll_remainder: 0.,
            marked_text: None,
            ime_cursor_bounds: Cell::new(None),
        }});

        // Foreground pump: coalesced wakeups → notify; exit → event.
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(msg) = wake_rx.recv().await {
                match msg {
                    grid::PumpMsg::Wakeup => {
                        let _ = weak.update(cx, |_, cx| {
                            cx.notify();
                            cx.emit(TermEvent::Wakeup);
                        });
                    }
                    grid::PumpMsg::Exit(code) => {
                        let _ = weak.update(cx, |s, cx| {
                            s.exit = Some(code);
                            cx.emit(TermEvent::Exit(code));
                            cx.notify();
                        });
                        break;
                    }
                }
            }
            anyhow::Ok(())
        })
        .detach();

        Ok(entity)
    }

    /// Exit code once the child has been reaped.
    pub fn exit(&self) -> Option<i32> {
        self.exit
    }

    /// True when the PTY delivered bytes within `window` — the "agent is
    /// producing output" signal behind the sidebar spinner.
    pub fn active_within(&self, window: Duration) -> bool {
        let last = self.grid.activity.load(std::sync::atomic::Ordering::Relaxed);
        grid::now_ms().saturating_sub(last) <= window.as_millis() as u64
    }

    /// Terminal-set window title (OSC 0), if any.
    pub fn title(&self) -> Option<String> {
        self.grid.meta.lock().title.clone()
    }

    /// Stage a grid+PTY resize. Applies immediately when the last resize
    /// is older than the debounce window; otherwise the latest target is
    /// flushed by a short timer — a window drag then reflows the grid at
    /// most ~7×/s instead of once per pixel.
    pub(crate) fn request_resize(&mut self, cols: u16, rows: u16, cx: &mut Context<Self>) {
        if self.grid.size() == (cols, rows) {
            self.pending_resize = None;
            return;
        }
        self.pending_resize = Some((cols, rows));
        if !self.ever_resized || self.last_resize.elapsed() >= RESIZE_DEBOUNCE {
            self.flush_resize(cx);
        } else if !self.flush_scheduled {
            self.flush_scheduled = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(RESIZE_DEBOUNCE).await;
                let _ = this.update(cx, |s, cx| s.flush_resize(cx));
            })
            .detach();
        }
    }

    fn flush_resize(&mut self, cx: &mut Context<Self>) {
        self.flush_scheduled = false;
        let Some((cols, rows)) = self.pending_resize.take() else {
            return;
        };
        self.last_resize = Instant::now();
        self.ever_resized = true;
        if self.grid.size() == (cols, rows) {
            return;
        }
        self.grid.resize(cols, rows);
        if let Some(process) = &self.process {
            process.resize(cols, rows);
        }
        cx.notify();
    }

    /// Send keystrokes/paste bytes to the child.
    pub fn write(&self, bytes: &[u8]) {
        self.grid.write(bytes);
    }

    /// Paste `text` into the child, wrapped in bracketed-paste markers
    /// when the child enabled that mode — shells then treat pasted
    /// newlines as text instead of executing them.
    pub fn paste_text(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        let bracketed = self
            .grid
            .term
            .lock()
            .mode()
            .contains(alacritty_terminal::term::TermMode::BRACKETED_PASTE);
        let mut bytes = Vec::with_capacity(text.len() + 12);
        if bracketed {
            bytes.extend_from_slice(b"\x1b[200~");
        }
        bytes.extend_from_slice(text.as_bytes());
        if bracketed {
            bytes.extend_from_slice(b"\x1b[201~");
        }
        self.grid.write(&bytes);
        self.grid.scroll_to_bottom();
    }

    /// Scroll the viewport (positive = towards history).
    pub fn scroll_by(&mut self, lines: f32, cx: &mut Context<Self>) {
        self.scroll_remainder += lines;
        let whole = self.scroll_remainder.trunc() as i32;
        self.scroll_remainder -= whole as f32;
        if whole != 0 {
            self.grid.scroll(whole);
            cx.notify();
        }
    }

    /// Scroll back to the live bottom.
    pub fn scroll_to_bottom(&self) {
        self.grid.scroll_to_bottom();
    }

    /// Kill the child. The exit still comes through [`TermEvent`].
    pub fn kill(&mut self) {
        if let Some(mut process) = self.process.take() {
            process.kill();
        }
    }

    /// Encode a keystroke into PTY bytes (`None` = not ours to handle).
    pub(crate) fn encode_keystroke(keystroke: &Keystroke) -> Option<Vec<u8>> {
        input::encode(keystroke)
    }

    /// Create the grid-painting element for this session.
    pub(crate) fn element(
        weak: WeakEntity<Self>,
        focus: FocusHandle,
    ) -> impl IntoElement {
        element::TerminalElement::new(weak, focus)
    }
}

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

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        None
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
    // and recursing forever at expansion (same convention as
    // `element.rs::palette_tests`).
    use super::{PtySpawn, TermSession};
    use gpui_kit::component::theme::Theme;
    use gpui_kit::{
        App, AnyWindowHandle, AppContext as _, Context, Entity, EntityInputHandler as _, IntoElement, Render, Styled as _, TestAppContext, Window, div, gpui,
    };
    use std::time::{Duration, Instant};

    /// Minimal root so `cx.add_window` has something to host; the IME
    /// handler calls below only need a live window, not a paint.
    struct TestRoot;
    impl Render for TestRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn grid_text(session: &Entity<TermSession>, cx: &App) -> String {
        let term = session.read(cx).grid.term.clone();
        let term = term.lock();
        let mut text = String::new();
        let mut last_line: Option<i32> = None;
        for indexed in term.renderable_content().display_iter {
            if last_line.is_some_and(|l| l != indexed.point.line.0) {
                text.push('\n');
            }
            last_line = Some(indexed.point.line.0);
            text.push(indexed.cell.c);
        }
        text
    }

    /// IME acceptance: preedit stays out of the PTY, a commit lands as
    /// UTF-8 bytes (a `cat` child echoes them back onto the grid), and
    /// unmark drops a cancelled composition.
    #[test]
    fn ime_commit_reaches_pty_as_utf8() {
        // `#[gpui::test]` is unusable here (see the import note), so
        // drive the same harness by hand.
        gpui::run_test_once(0, Box::new(|dispatcher| {
            let mut cx0 = TestAppContext::build(dispatcher, Some("ime_commit_reaches_pty_as_utf8"));
            let cx = &mut cx0;
            let window = cx.add_window(|_, _| TestRoot);
            let window = AnyWindowHandle::from(window);
            let session = cx
                .update(|cx| {
                    cx.set_global(Theme::default());
                    TermSession::spawn(
                        &PtySpawn {
                            program: "cat".into(),
                            args: vec![],
                            cwd: std::env::temp_dir(),
                        },
                        cx,
                    )
                })
                .expect("spawn cat");

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
            let compact = |s: String| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
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
            assert!(echoed, "cat never echoed the committed IME text; grid: {:?}", compact(cx.update(|cx| grid_text(&session, cx))));
        }));
    }
}
