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

use gpui_kit::*;

pub use pty::PtySpawn;

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

    /// Terminal-set window title (OSC 0), if any.
    pub fn title(&self) -> Option<String> {
        self.grid.meta.lock().title.clone()
    }

    /// Grid + PTY resize; both sides must agree or the child's output
    /// wraps at the wrong width.
    pub(crate) fn resize_if_needed(&mut self, cols: u16, rows: u16) {
        if self.grid.size() == (cols, rows) {
            return;
        }
        self.grid.resize(cols, rows);
        if let Some(process) = &self.process {
            process.resize(cols, rows);
        }
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

