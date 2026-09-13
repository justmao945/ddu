//! Agent terminal stack: real PTY processes parsed into an alacritty
//! grid and painted by a custom element.
//!
//! * [`pty`]   — process + master handles (spawn/resize/kill)
//! * [`grid`]  — `Term` behind a `FairMutex` + the two pump threads
//! * [`attention`] — `BEL`/`OSC 9`/`OSC 777` markers in the byte stream
//! * [`input`] — keystroke → escape-sequence encoding
//! * [`element`] — the grid painter (custom `Element`)
//! * [`palette`] — the ANSI ramp + the theme's default foreground/background
//! * [`boxart`] — vector box-drawing/block chars (no font gaps)
//!
//! [`TermSession`] is the gpui entity tying it together: it owns the
//! grid and process, receives pump wakeups, and emits [`TermEvent`]
//! when the child exits. What it does is split by concern:
//! [`session`] (PTY lifecycle + IO), [`stream`] (repaint pacing),
//! [`search`] (⌘F over the grid), [`mouse`] (reporting to the child),
//! [`selection`] (text selection), [`scrollbar`] (the overlay bar) and
//! [`ime`] (input-method entry).

mod attention;
mod boxart;
mod element;
mod grid;
#[cfg(test)]
mod harness;
mod ime;
mod input;
mod mouse;
mod palette;
mod pty;
mod scrollbar;
mod search;
mod selection;
mod session;
mod stream;

use std::cell::Cell;
use std::time::{Duration, Instant};

use gpui_kit::*;

use search::TermSearch;

pub use attention::Attention;
pub(crate) use mouse::MouseTracking;
pub use pty::PtySpawn;
pub use search::TermMatch;
#[cfg(test)]
pub(crate) use stream::STREAM_FRAME_MIN;

/// Minimum gap between grid+PTY reflows while a drag is resizing.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(140);

/// Terminal events emitted to subscribers (the app shell).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermEvent {
    /// Grid changed (coalesced); subscribers should re-render.
    Wakeup,
    /// The child printed a "the user is needed" marker (`BEL` /
    /// `OSC 9` / `OSC 777`) — agents emit one when a turn ends, a
    /// question is asked or a run fails. See [`attention`].
    Attention(attention::Attention),
    /// Child exited with the raw exit code (0 = success).
    Exit(i32),
}

pub struct TermSession {
    grid: grid::TermGrid,
    process: Option<pty::PtyProcess>,
    pub(crate) focus: FocusHandle,
    exit: Option<i32>,
    /// Agent session id captured from the startup banner (`session id:
    /// <uuid>`), usable for `--resume` on the same command.
    resume_id: Option<String>,
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
    /// Left-button drag in progress (between mouse down and up).
    selecting: bool,
    /// Terminal element bounds in window coordinates from the last
    /// paint — mouse events map through them into grid cells.
    grid_bounds: Cell<Option<Bounds<Pixels>>>,
    /// Painted content rect (element bounds minus padding, grid
    /// centered) — selection and mouse-report cell mapping use this.
    content_bounds: Cell<Option<Bounds<Pixels>>>,
    /// Scrollbar thumb drag: grab offset (px) below the thumb's top.
    scrollbar_drag: Option<f32>,
    /// Overlay-scrollbar visibility, macOS-style: show while the mouse
    /// hovers the right-edge strip or while scroll activity is recent,
    /// fade out after [`scrollbar::SCROLLBAR_IDLE`].
    scrollbar_hover: bool,
    scrollbar_until: Option<Instant>,
    /// The one-shot repaint that makes the fade-out actually fire is in
    /// flight (see [`TermSession::arm_scrollbar_hide`]).
    scrollbar_hide_armed: bool,
    /// Button code held while the child tracks the mouse (xterm 1002
    /// drag reports); None when no button is down.
    mouse_held: Option<u8>,
    /// Last cell reported for motion events — dedupes drag floods.
    mouse_cell: (u32, u32),
    /// Fractional wheel steps carried between events in mouse mode.
    wheel_remainder: f32,
    /// Find-bar state (⌘F): query, hits and the current hit's index.
    pub(crate) search: TermSearch,
    /// Stream repaint pacing: slow EWMA of this element's own paint cost
    /// per frame (ms), written by the painter and read by the pump
    /// ([`stream::stream_interval`]). 0. = no frame measured yet.
    stream_paint_ms: Cell<f32>,
    /// Output landed while the bar is up — the match set is stale and
    /// the rescan timer (when armed) will rebuild it.
    search_dirty: bool,
    /// The one-shot rescan timer is in flight.
    search_timer_armed: bool,
}

impl gpui_kit::EventEmitter<TermEvent> for TermSession {}
