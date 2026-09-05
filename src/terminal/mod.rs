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
}

impl gpui_kit::EventEmitter<TermEvent> for TermSession {}

impl TermSession {
    /// Spawn `cmd` in a fresh PTY and start its pump threads. Initial
    /// grid size is 80×24; the element's prepaint resizes it to the
    /// panel on the first frame.
    pub fn spawn(cmd: &PtySpawn, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let (cols, rows) = (80, 24);
        let (wake_tx, wake_rx) = async_channel::bounded::<grid::PumpMsg>(1);
        let (grid, process) = grid::spawn_session(cmd, cols, rows, wake_tx)?;

        let entity = cx.new(|cx| Self {
            grid,
            process: Some(process),
            focus: cx.focus_handle(),
            exit: None,
            pending_resize: None,
            last_resize: Instant::now(),
            ever_resized: false,
            flush_scheduled: false,
        });

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

    /// Scroll the viewport (positive = towards history).
    pub fn scroll(&self, lines: i32) {
        self.grid.scroll(lines);
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

