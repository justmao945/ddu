//! Agent terminal stack: real PTY processes parsed into an alacritty
//! grid and painted by a custom element.
//!
//! * [`pty`]   — process + master handles (spawn/resize/kill)
//! * [`grid`]  — `Term` behind a `FairMutex` + the two pump threads
//! * [`attention`] — `BEL`/`OSC 9`/`OSC 777` markers in the byte stream
//! * [`input`] — keystroke → escape-sequence encoding
//! * [`element`] — the grid painter (custom `Element`)
//! * [`boxart`] — vector box-drawing/block chars (no font gaps)
//!
//! [`TermSession`] is the gpui entity tying it together: it owns the
//! grid and process, receives pump wakeups, and emits [`TermEvent`]
//! when the child exits.

mod attention;
mod boxart;
mod element;
mod grid;
mod input;
mod pty;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::grid::Scroll as GridScroll;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;

use std::time::{Duration, Instant};

use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::*;

pub use attention::Attention;
pub use pty::PtySpawn;

/// Minimum gap between grid+PTY reflows while a drag is resizing.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(140);
/// How long the overlay thumb stays up after the last scroll/hover
/// activity (same 2s hold the Base scrollbars in the diff panes use).
const SCROLLBAR_IDLE: Duration = Duration::from_secs(2);
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
pub(crate) const STREAM_FRAME_MIN: Duration = Duration::from_millis(50);
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
fn stream_interval(paint_ms: f32) -> Duration {
    STREAM_FRAME_STEPS
        .iter()
        .rev()
        .find(|(cost, _)| paint_ms >= *cost)
        .map_or(STREAM_FRAME_MIN, |(_, interval)| *interval)
}

/// Hard cap on find-bar hits; past it the counter just shows the cap.
const SEARCH_MAX_MATCHES: usize = 500;
/// Output-driven match rescans ride this one-shot timer so a stream of
/// PTY chunks (one wakeup each, up to display-link rate) rescans a few
/// times per second instead of per chunk; query edits rescan
/// immediately (keystroke rate) via `refresh_search`.
const SEARCH_RESCAN: Duration = Duration::from_millis(150);

/// One find-bar hit in the grid: an absolute line (negative =
/// history, alacritty's `Line` convention) plus a half-open
/// cell-column range the painter can turn into a rect directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TermMatch {
    pub line: i32,
    pub start: usize,
    pub end: usize,
}

/// Find-bar state for one session's grid (⌘F while the terminal holds
/// focus). `open` folds the whole lifecycle: closed → the bar is
/// unmounted and `matches` stays empty.
pub(crate) struct TermSearch {
    pub open: bool,
    /// Created lazily on first open — `InputState` needs a `Window`,
    /// which `spawn` doesn't have.
    pub input: Option<Entity<InputState>>,
    /// Last scanned query; output wakeups rescan only when non-empty.
    pub query: String,
    /// `Rc` so the painter can grab the whole set per frame with a
    /// refcount bump; every rescan replaces it wholesale.
    pub matches: Rc<Vec<TermMatch>>,
    /// Index into `matches` the counter shows and the viewport targets.
    pub current: usize,
}

impl TermSearch {
    fn new() -> Self {
        Self {
            open: false,
            input: None,
            query: String::new(),
            matches: Rc::new(Vec::new()),
            current: 0,
        }
    }
}

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
    /// fade out after [`SCROLLBAR_IDLE`].
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
    /// ([`stream_interval`]). 0. = no frame measured yet.
    stream_paint_ms: Cell<f32>,
    /// Output landed while the bar is up — the match set is stale and
    /// the rescan timer (when armed) will rebuild it.
    search_dirty: bool,
    /// The one-shot rescan timer is in flight.
    search_timer_armed: bool,
}

impl gpui_kit::EventEmitter<TermEvent> for TermSession {}

impl TermSession {
    /// Spawn `cmd` in a fresh PTY and start its pump threads. Initial
    /// grid size is 80×24; the element's prepaint resizes it to the
    /// panel on the first frame. Scrollback comes from the user config
    /// (`Settings → Terminal → Scrollback`).
    pub fn spawn(cmd: &PtySpawn, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let (cols, rows) = (80, 24);
        let scrollback = cx.global::<crate::config::Config>().terminal_scrollback();
        let (wake_tx, wake_rx) = async_channel::bounded::<grid::PumpMsg>(1);
        // Attention markers are events, not repaint hints: they get
        // their own queue so the capacity-1 wakeup coalescing can never
        // swallow a notification.
        let (attention_tx, attention_rx) = async_channel::bounded::<attention::Attention>(16);
        let (grid, process) = grid::spawn_session(
            cmd,
            cols,
            rows,
            wake_tx,
            gpui_kit::component::theme::Theme::global(cx).is_dark(),
            scrollback,
            attention_tx,
        )?;

        let entity = cx.new(|cx| {
            cx.observe_global::<gpui_kit::component::theme::Theme>(|this: &mut Self, cx| {
                this.grid.dark.store(
                    gpui_kit::component::theme::Theme::global(cx).is_dark(),
                    std::sync::atomic::Ordering::Relaxed,
                );
                // The palette is resolved from the live theme at paint
                // time — wake so existing sessions repaint immediately.
                cx.emit(TermEvent::Wakeup);
            })
            .detach();
            Self {
                grid,
                process: Some(process),
                focus: cx.focus_handle().tab_stop(false),
                exit: None,
                resume_id: None,
                pending_resize: None,
                last_resize: Instant::now(),
                selecting: false,
                grid_bounds: Cell::new(None),
                content_bounds: Cell::new(None),
                scrollbar_drag: None,
                scrollbar_hover: false,
                scrollbar_until: None,
                scrollbar_hide_armed: false,
                mouse_held: None,
                mouse_cell: (u32::MAX, u32::MAX),
                wheel_remainder: 0.,
                marked_text: None,
                ime_cursor_bounds: Cell::new(None),
                ever_resized: false,
                flush_scheduled: false,
                scroll_remainder: 0.,
                search: TermSearch::new(),
                stream_paint_ms: Cell::new(0.),
                search_dirty: false,
                search_timer_armed: false,
            }
        });

        // Foreground pump: coalesced wakeups → notify; exit → event.
        // Stream floods are repaint-throttled (see [`stream_interval`] /
        // `STREAM_FRAME_MIN`): the first wakeup of a burst paints
        // immediately, the rest are flushed once per interval by a
        // trailing timer.
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            // `last_frame` is read through the (fake-clock-aware)
            // executor clock so throttle behavior is testable; the
            // cells are single-threaded foreground state.
            let last_frame = Rc::new(Cell::new(None::<Instant>));
            let flush_armed = Rc::new(Cell::new(false));
            while let Ok(msg) = wake_rx.recv().await {
                match msg {
                    grid::PumpMsg::Wakeup => {
                        // Pace against what a frame costs this process:
                        // an expensive terminal paint steps the interval
                        // down (see [`stream_interval`]).
                        let interval = stream_interval(
                            weak.update(cx, |s, _| s.stream_paint_ms.get())
                                .unwrap_or(0.),
                        );
                        let now = cx.background_executor().now();
                        let prev = last_frame.get();
                        if prev.map_or(true, |t| now.duration_since(t) >= interval) {
                            last_frame.set(Some(now));
                            let _ = weak.update(cx, |_, cx| {
                                cx.notify();
                                cx.emit(TermEvent::Wakeup);
                            });
                        } else if !flush_armed.replace(true) {
                            let weak = weak.clone();
                            let last_frame = last_frame.clone();
                            let flush_armed = flush_armed.clone();
                            let delay = interval - now.duration_since(prev.unwrap());
                            cx.spawn(async move |cx| {
                                cx.background_executor().timer(delay).await;
                                flush_armed.set(false);
                                last_frame.set(Some(cx.background_executor().now()));
                                let _ = weak.update(cx, |_, cx| {
                                    cx.notify();
                                    cx.emit(TermEvent::Wakeup);
                                });
                            })
                            .detach();
                        }
                    }
                    grid::PumpMsg::Exit(code) => {
                        let _ = weak.update(cx, |s, cx| {
                            s.exit = Some(code);
                            // Agents print their session id in the
                            // banner / exit footer (`session id: …`,
                            // `--resume <id>`, `Session ID: <id>`);
                            // the last scan of the output tail finds it.
                            if s.resume_id.is_none() {
                                s.resume_id = grid::extract_resume_id(&s.grid.recent.tail());
                            }
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

        // Attention pump: the reader thread's BEL/OSC markers, one
        // subscriber event each. Runs until the entity is gone.
        let weak = entity.downgrade();
        cx.spawn(async move |cx| {
            while let Ok(signal) = attention_rx.recv().await {
                if weak
                    .update(cx, |_, cx| cx.emit(TermEvent::Attention(signal)))
                    .is_err()
                {
                    break;
                }
            }
            anyhow::Ok(())
        })
        .detach();

        Ok(entity)
    }

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

    /// Exit code once the child has been reaped.
    pub fn exit(&self) -> Option<i32> {
        self.exit
    }

    /// Agent session id for `--resume` (`None` when the child never
    /// printed one in the captured tail).
    pub fn resume_id(&self) -> Option<&str> {
        self.resume_id.as_deref()
    }

    /// Scan the recent output tail for the agent's resume id if not yet
    /// captured. Agents print it early (banner / "resume this session
    /// with …"), but only the exit path used to look — closing the
    /// window mid-run lost it. Persist calls this before reading.
    pub(crate) fn capture_resume_id(&mut self) {
        if self.resume_id.is_none() {
            self.resume_id = grid::extract_resume_id(&self.grid.recent.tail());
        }
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
        cx.emit(TermEvent::Wakeup);
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
        // Any wheel traffic counts as scrollbar activity — even a
        // whole==0 trickle must refresh the idle window.
        self.reveal_scrollbar(cx);
        if whole != 0 {
            self.grid.scroll(whole);
            cx.emit(TermEvent::Wakeup);
        }
    }
    /// Map a window-coordinate point to a grid cell, clamped into the
    /// visible area (a drag outside the grid pins to its edge). The
    /// `Side` picks the cell edge nearest the x fraction — alacritty's
    /// anchor convention for drag ends.
    pub(crate) fn cell_at(
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
    pub(crate) fn begin_selection(
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
    pub(crate) fn grow_selection(&mut self, cell: GridPoint, side: Side, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        if let Some(selection) = self.grid.term.lock().selection.as_mut() {
            selection.update(cell, side);
            cx.emit(TermEvent::Wakeup);
        }
    }

    /// End the drag; a plain click (empty selection) clears the wash.
    pub(crate) fn end_selection(&mut self, cx: &mut Context<Self>) {
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

    pub(crate) fn copy_selection(&self, cx: &mut App) -> bool {
        match self.grid.term.lock().selection_to_string() {
            Some(text) if !text.trim().is_empty() => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                true
            }
            _ => false,
        }
    }

    /// Non-empty mouse selection present (drives Copy enablement).
    pub(crate) fn has_selection(&self) -> bool {
        self.grid
            .term
            .lock()
            .selection
            .as_ref()
            .is_some_and(|s| !s.is_empty())
    }

    /// True while the find bar's input holds focus — the terminal
    /// surface's key encoder must stay silent then, or every typed
    /// character would land in the grid AND the query box.
    pub(crate) fn search_input_focused(&self, window: &Window, cx: &App) -> bool {
        self.search
            .input
            .as_ref()
            .is_some_and(|input| input.focus_handle(cx).is_focused(window))
    }

    /// Plant content straight into the grid (see [`grid::TermGrid::
    /// inject_bytes`]) — how a test sets up output without treating the
    /// child to a prompt.
    #[cfg(test)]
    pub(crate) fn inject_bytes(&self, bytes: &[u8]) {
        self.grid.inject_bytes(bytes);
    }

    /// Open (or refocus) the find bar; the whole query is selected so
    /// typing replaces it. The bar's input is created here on first
    /// open — `InputState` needs a `Window`, which `spawn` lacks — and
    /// its keystrokes rescan matches live.
    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.input.is_none() {
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx);
                input.set_placeholder("Find in terminal", window, cx);
                input
            });
            cx.subscribe(
                &input,
                |this: &mut Self, _, event: &InputEvent, cx: &mut Context<Self>| {
                    if matches!(event, InputEvent::Change) {
                        this.refresh_search(cx);
                        if !this.search.matches.is_empty() {
                            this.search.current = 0;
                            this.search_reveal(cx);
                        } else {
                            this.search.current = 0;
                            cx.notify();
                        }
                    }
                },
            )
            .detach();
            self.search.input = Some(input);
        }
        self.search.open = true;
        if let Some(input) = &self.search.input {
            input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
        }
        self.refresh_search(cx);
        cx.notify();
    }

    /// Close the bar and hand focus back to the terminal surface (the
    /// session's own handle, so the cursor goes solid again and PTY
    /// keystrokes flow).
    pub(crate) fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.open = false;
        self.search.matches = Rc::new(Vec::new());
        self.search.current = 0;
        self.search_dirty = false;
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Step through matches, wrapping at both ends (Enter/Shift-Enter,
    /// ⌘G/⌘⇧G, the bar's chevrons).
    pub(crate) fn search_step(&mut self, back: bool, cx: &mut Context<Self>) {
        let len = self.search.matches.len();
        if len == 0 {
            return;
        }
        self.search.current = if back {
            (self.search.current + len - 1) % len
        } else {
            (self.search.current + 1) % len
        };
        self.search_reveal(cx);
    }

    /// Recompute matches from the input's current value against this
    /// session's grid. Called on keystrokes, open and the rescan
    /// timer; `current` stays clamped when the set shrank.
    pub(crate) fn refresh_search(&mut self, cx: &mut Context<Self>) {
        let query = self
            .search
            .input
            .as_ref()
            .map(|input| input.read(cx).value().to_string())
            .unwrap_or_default();
        self.search.matches = if self.search.open {
            Rc::new(grid::search_grid(
                &self.grid.term,
                &query,
                SEARCH_MAX_MATCHES,
            ))
        } else {
            Rc::new(Vec::new())
        };
        self.search.query = query;
        self.search.current = self
            .search
            .current
            .min(self.search.matches.len().saturating_sub(1));
        cx.notify();
    }

    /// Output landed while the bar is up: mark the match set stale and
    /// (re)arm the one-shot rescan timer. Cheap no-op when closed or
    /// the query is empty, so per-chunk wakeups cost an open-check.
    pub(crate) fn note_search_dirty(&mut self, cx: &mut Context<Self>) {
        if !self.search.open || self.search.query.is_empty() {
            return;
        }
        self.search_dirty = true;
        self.arm_search_timer(cx);
    }

    fn arm_search_timer(&mut self, cx: &mut Context<Self>) {
        if self.search_timer_armed {
            return;
        }
        self.search_timer_armed = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEARCH_RESCAN).await;
            let _ = this.update(cx, |term, cx| {
                term.search_timer_armed = false;
                if term.search_dirty {
                    term.search_dirty = false;
                    term.refresh_search(cx);
                }
            });
        })
        .detach();
    }

    /// Scroll the viewport to the current hit. Already-visible hits
    /// keep the viewport put; hits above park two rows below the top
    /// (context reads downward), hits below land on the last row.
    pub(crate) fn search_reveal(&mut self, cx: &mut Context<Self>) {
        let Some(&hit) = self.search.matches.get(self.search.current) else {
            cx.notify();
            return;
        };
        let mut term = self.grid.term.lock();
        let grid = term.grid();
        let screen = grid.screen_lines() as i32;
        let history = grid.history_size() as i32;
        let offset = grid.display_offset() as i32;
        let target = if (-offset..=screen - 1 - offset).contains(&hit.line) {
            offset
        } else if hit.line < -offset {
            (2 - hit.line).min(history)
        } else {
            (screen - 1 - hit.line).max(0)
        };
        if target != offset {
            term.scroll_display(GridScroll::Delta(target - offset));
        }
        cx.notify();
    }

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
    fn reveal_scrollbar(&mut self, cx: &mut Context<Self>) {
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
        element::scrollbar_geometry(bounds, rows as usize, history, offset, self.scrollbar_engaged())
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

    /// What mouse traffic the child asked for (xterm 1000/1002/1003).
    pub(crate) fn mouse_tracking(&self) -> MouseTracking {
        let mode = *self.grid.term.lock().mode();
        if mode.contains(TermMode::MOUSE_MOTION) {
            MouseTracking::Motion
        } else if mode.contains(TermMode::MOUSE_DRAG) {
            MouseTracking::Drag
        } else if mode.contains(TermMode::MOUSE_REPORT_CLICK) {
            MouseTracking::Click
        } else {
            MouseTracking::None
        }
    }

    /// Forward a button press/release to a mouse-tracking child.
    /// True = consumed (the caller skips selection/scrollbar/menu).
    pub(crate) fn mouse_button(
        &mut self,
        button: MouseButton,
        press: bool,
        pos: Point<Pixels>,
        modifiers: &Modifiers,
        window: &Window,
        cx: &App,
    ) -> bool {
        if self.mouse_tracking() == MouseTracking::None {
            return false;
        }
        let code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            _ => return false,
        };
        let Some((col, row)) = self.screen_cell_at(pos, window, cx) else {
            return false;
        };
        let sgr = self.grid.term.lock().mode().contains(TermMode::SGR_MOUSE);
        if press {
            self.mouse_held = Some(code);
        } else {
            self.mouse_held = None;
        }
        if let Some(bytes) = encode_mouse(code, col, row, modifiers, sgr, press) {
            self.grid.write(&bytes);
        }
        true
    }

    /// Forward wheel steps as button 64/65 reports. True = consumed.
    pub(crate) fn mouse_wheel(
        &mut self,
        lines: f32,
        pos: Point<Pixels>,
        modifiers: &Modifiers,
        window: &Window,
        cx: &App,
    ) -> bool {
        if self.mouse_tracking() == MouseTracking::None {
            return false;
        }
        let Some((col, row)) = self.screen_cell_at(pos, window, cx) else {
            return false;
        };
        let sgr = self.grid.term.lock().mode().contains(TermMode::SGR_MOUSE);
        self.wheel_remainder += lines;
        let whole = self.wheel_remainder.trunc() as i32;
        self.wheel_remainder -= whole as f32;
        let code = if whole > 0 { 64 } else { 65 };
        for _ in 0..whole.abs() {
            if let Some(bytes) = encode_mouse(code, col, row, modifiers, sgr, true) {
                self.grid.write(&bytes);
            }
        }
        true
    }

    /// Forward pointer motion: drag reports while a button is held in
    /// 1002 mode, all motion in 1003 mode. True = the child tracks
    /// motion (caller must not grow the text selection).
    pub(crate) fn mouse_motion(
        &mut self,
        pos: Point<Pixels>,
        modifiers: &Modifiers,
        window: &Window,
        cx: &App,
    ) -> bool {
        match self.mouse_tracking() {
            MouseTracking::None | MouseTracking::Click => return false,
            MouseTracking::Drag if self.mouse_held.is_none() => return false,
            _ => {}
        }
        let Some((col, row)) = self.screen_cell_at(pos, window, cx) else {
            return false;
        };
        let cell = (col as u32, row as u32);
        if cell == self.mouse_cell {
            return true;
        }
        self.mouse_cell = cell;
        // 32 = motion bit; 35 = no button held (1003 hover reports).
        let code = 32 + self.mouse_held.map_or(3, |b| b);
        let sgr = self.grid.term.lock().mode().contains(TermMode::SGR_MOUSE);
        if let Some(bytes) = encode_mouse(code, col, row, modifiers, sgr, true) {
            self.grid.write(&bytes);
        }
        true
    }

    /// Map a window point to 1-based screen (col, row) — the coordinate
    /// space mouse reports use. Unlike `cell_at`, no scrollback offset.
    fn screen_cell_at(
        &self,
        pos: Point<Pixels>,
        window: &Window,
        cx: &App,
    ) -> Option<(usize, usize)> {
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
        Some((col as usize, row as usize))
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

    /// One control byte into the PTY (Esc/^C/^D …): ^C is SIGINT to
    /// the foreground process group, ^D EOF on the input. No-op on an
    /// already-exited child.
    pub(crate) fn ctrl(&self, byte: u8) {
        if self.exit.is_some() {
            return;
        }
        if let Some(process) = &self.process {
            process.writer().write(&[byte]);
        }
    }

    /// Encode a keystroke into PTY bytes (`None` = not ours to handle).
    pub(crate) fn encode_keystroke(keystroke: &Keystroke) -> Option<Vec<u8>> {
        input::encode(keystroke)
    }

    /// Create the grid-painting element for this session.
    pub(crate) fn element(weak: WeakEntity<Self>, focus: FocusHandle) -> impl IntoElement {
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

/// Mouse tracking level requested by the child app (xterm private
/// modes): clicks only, clicks+drag motion, or all motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MouseTracking {
    None,
    /// 1000: press + release.
    Click,
    /// 1002: + motion while a button is held.
    Drag,
    /// 1003: + all pointer motion.
    Motion,
}

/// Encode one mouse report: SGR 1006 when the child enabled it,
/// otherwise the legacy X11 form (dropped past column/row 223).
///
/// `code` is the pre-modifier button code (0 left / 1 middle / 2 right /
/// 64 wheel-up / 65 wheel-down, +32 motion bit); `press` selects the
/// SGR `M`/`m` suffix — legacy encodes release as button 3.
fn encode_mouse(
    code: u8,
    col: usize,
    row: usize,
    modifiers: &Modifiers,
    sgr: bool,
    press: bool,
) -> Option<Vec<u8>> {
    let mut code = code
        + if modifiers.shift { 4 } else { 0 }
        + if modifiers.alt { 8 } else { 0 }
        + if modifiers.control { 16 } else { 0 };
    let (x, y) = (col + 1, row + 1);
    if sgr {
        let suffix = if press { 'M' } else { 'm' };
        // SGR release reports the released button itself (`m` suffix).
        return Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes());
    }
    if !press && code < 64 {
        code = 3; // legacy release = button 3
    }
    if x > 223 || y > 223 {
        return None;
    }
    let (b, xb, yb) = (code + 32, x as u8 + 32, y as u8 + 32);
    Some(vec![0x1b, b'[', b'M', b, xb, yb])
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
        AnyWindowHandle, App, AppContext as _, Context, Entity, EntityInputHandler as _,
        InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
        TestAppContext, Window, div, gpui,
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

    #[test]
    fn mouse_report_encoding() {
        use super::encode_mouse;
        use gpui_kit::Modifiers;
        let none = Modifiers::none();
        // SGR 1006: press `M`, release `m`, 1-based coords.
        assert_eq!(
            encode_mouse(0, 0, 0, &none, true, true),
            Some(b"\x1b[<0;1;1M".to_vec())
        );
        assert_eq!(
            encode_mouse(0, 0, 0, &none, true, false),
            Some(b"\x1b[<0;1;1m".to_vec())
        );
        assert_eq!(
            encode_mouse(64, 2, 3, &none, true, true),
            Some(b"\x1b[<64;3;4M".to_vec())
        );
        // Shift adds 4 to the button code.
        let shift = Modifiers {
            shift: true,
            ..Modifiers::none()
        };
        assert_eq!(
            encode_mouse(0, 0, 0, &shift, true, true),
            Some(b"\x1b[<4;1;1M".to_vec())
        );
        // Legacy X11: ESC [ M + 32-offset bytes; release = button 3.
        assert_eq!(
            encode_mouse(0, 0, 0, &none, false, false),
            Some(vec![0x1b, b'[', b'M', b'#', b'!', b'!'])
        );
        assert_eq!(
            encode_mouse(2, 4, 9, &none, false, true),
            Some(vec![0x1b, b'[', b'M', b'"', b'%', b'*'])
        );
        // Past the 223 ceiling legacy must drop the event.
        assert_eq!(encode_mouse(0, 300, 0, &none, false, true), None);
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
                let session = cx
                    .update(|cx| {
                        cx.set_global(Theme::default());
                        cx.set_global(crate::config::Config::default());
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

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// Render-affecting mutations must emit `Wakeup`: the app shell
    /// repaints solely off that event, so a bare `cx.notify()` here
    /// means mouse selection / wheel scroll never become visible.
    #[test]
    fn render_mutations_emit_wakeup() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("render_mutations_emit_wakeup"));
                let cx = &mut cx0;
                let session = cx
                    .update(|cx| {
                        cx.set_global(Theme::default());
                        cx.set_global(crate::config::Config::default());
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
                let events = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
                cx.update(|cx| {
                    let events = events.clone();
                    cx.subscribe(&session, move |_, event: &super::TermEvent, _| {
                        events.borrow_mut().push(event.clone());
                    })
                    .detach();
                });

                cx.update(|cx| {
                    session.update(cx, |s, cx| {
                        s.begin_selection(
                            super::GridPoint::new(super::Line(0), super::Column(0)),
                            super::Side::Left,
                            1,
                            cx,
                        );
                        s.scroll_by(2., cx);
                        s.end_selection(cx);
                    });
                });

                let log = events.borrow();
                let wakeups = log
                    .iter()
                    .filter(|e| **e == super::TermEvent::Wakeup)
                    .count();
                assert!(
                    wakeups >= 3,
                    "begin/scroll/end each emit Wakeup, got {log:?}"
                );
            }),
        );
    }

    /// End-to-end: a left-drag across grid cells must set a selection
    /// whose copy text is the swept run — this exercises the same
    /// hitbox → listener → `cell_at` → alacritty-model chain the live
    /// surface uses (`ui::terminal::surface` mirrors these listeners).
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
                let session = cx
                    .update(|cx| {
                        cx.set_global(Theme::default());
                        cx.set_global(crate::config::Config::default());
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

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
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
                let session = cx
                    .update(|cx| {
                        cx.set_global(Theme::default());
                        cx.set_global(crate::config::Config::default());
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
                let session = cx
                    .update(|cx| {
                        cx.set_global(Theme::default());
                        cx.set_global(crate::config::Config::default());
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
                let paints = Rc::new(Cell::new(0usize));
                cx.update(|cx| {
                    let paints = paints.clone();
                    cx.observe(&session, move |_, _| paints.set(paints.get() + 1)).detach();
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
                let session = cx
                    .update(|cx| {
                        cx.set_global(Theme::default());
                        cx.set_global(crate::config::Config::default());
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
                let paints = Rc::new(Cell::new(0usize));
                cx.update(|cx| {
                    let paints = paints.clone();
                    cx.observe(&session, move |_, _| paints.set(paints.get() + 1)).detach();
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

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
