//! Mouse reporting to the child (xterm 1000/1002/1003): the tracking
//! level it asked for, press/release/wheel/motion encoding, and the
//! screen-cell mapping motion needs (no scrollback offset).

use alacritty_terminal::term::TermMode;
use gpui_kit::*;

use super::*;

/// Mouse tracking level requested by the child app (xterm private
/// modes): clicks only, clicks+drag motion, or all motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseTracking {
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

impl TermSession {

    /// What mouse traffic the child asked for (xterm 1000/1002/1003).
    pub fn mouse_tracking(&self) -> MouseTracking {
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
    pub fn mouse_button(
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
    pub fn mouse_wheel(
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
    pub fn mouse_motion(
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
}

#[cfg(test)]
mod tests {
    // Selective imports only: `use super::*` would pull gpui's `test`
    // proc-macro re-export into scope, shadowing the built-in `#[test]`
    // and recursing forever at expansion.



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
}
