//! Agent terminal stack: real PTY processes parsed into an alacritty
//! grid and painted by a custom element.
//!
//! * [`pty`]   — process + master handles (spawn/resize/kill)
//! * [`grid`]  — `Term` behind a `FairMutex` + the two pump threads
//! * [`input`] — keystroke → escape-sequence encoding
//! * [`element`] — the grid painter (custom `Element`)
//! * [`boxart`] — vector box-drawing/block chars (no font gaps)
//!
//! [`TermSession`] is the gpui entity tying it together: it owns the
//! grid and process, receives pump wakeups, and emits [`TermEvent`]
//! when the child exits.

mod boxart;
mod element;
mod grid;
mod input;
mod pty;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::index::{Column, Line, Point as GridPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::TermMode;
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
    /// Button code held while the child tracks the mouse (xterm 1002
    /// drag reports); None when no button is down.
    mouse_held: Option<u8>,
    /// Last cell reported for motion events — dedupes drag floods.
    mouse_cell: (u32, u32),
    /// Fractional wheel steps carried between events in mouse mode.
    wheel_remainder: f32,
}

impl gpui_kit::EventEmitter<TermEvent> for TermSession {}

impl TermSession {
    /// Spawn `cmd` in a fresh PTY and start its pump threads. Initial
    /// grid size is 80×24; the element's prepaint resizes it to the
    /// panel on the first frame.
    pub fn spawn(cmd: &PtySpawn, cx: &mut App) -> anyhow::Result<Entity<Self>> {
        let (cols, rows) = (80, 24);
        let (wake_tx, wake_rx) = async_channel::bounded::<grid::PumpMsg>(1);
        let (grid, process) = grid::spawn_session(
            cmd,
            cols,
            rows,
            wake_tx,
            gpui_kit::component::theme::Theme::global(cx).is_dark(),
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
                ever_resized: false,
                flush_scheduled: false,
                scroll_remainder: 0.,
                marked_text: None,
                scrollbar_drag: None,
                mouse_held: None,
                mouse_cell: (u32::MAX, u32::MAX),
                wheel_remainder: 0.,
                ime_cursor_bounds: Cell::new(None),
            }
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

        Ok(entity)
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

    /// Right-edge scrollbar track+thumb in window coordinates, or None
    /// when there is no scrollback to navigate.
    pub(crate) fn scrollbar_geometry(&self) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
        let bounds = self.grid_bounds.get()?;
        let (_cols, rows) = self.grid.size();
        let (history, offset) = {
            let term = self.grid.term.lock();
            (term.grid().history_size(), term.grid().display_offset())
        };
        // Full element height, flush with both ends — same rect the
        // element paints against.
        element::scrollbar_geometry(bounds, rows as usize, history, offset)
    }

    /// Left button down on the scrollbar strip: on the thumb starts a
    /// drag, on the bare track pages up/down. True = event consumed.
    pub(crate) fn scrollbar_mouse_down(
        &mut self,
        pos: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((track, thumb)) = self.scrollbar_geometry() else {
            return false;
        };
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
        let travel = f32::from(track.size.height - thumb.size.height);
        let history = self.grid.term.lock().grid().history_size() as f32;
        let frac = if travel <= 0. {
            0.
        } else {
            1. - ((f32::from(pos.y) - grab - f32::from(track.origin.y)) / travel).clamp(0., 1.)
        };
        let target = (frac * history).round() as i32;
        let current = self.grid.term.lock().grid().display_offset() as i32;
        if target != current {
            self.grid.scroll(target - current);
            cx.emit(TermEvent::Wakeup);
        }
        true
    }

    /// End any scrollbar drag (mouse up anywhere).
    pub(crate) fn scrollbar_mouse_up(&mut self) {
        self.scrollbar_drag = None;
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
                        events.borrow_mut().push(*event);
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
}
