//! The terminal renderer: a custom [`Element`] that paints the visible
//! window of the alacritty grid as shaped mono lines and owns the
//! grid↔panel resize handshake.

use gpui_kit::component::theme::Theme;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::vte::ansi::{Color as TermColor, NamedColor};

use super::TermSession;

/// Line height as a factor of the mono font size.
const LINE_HEIGHT_FACTOR: f32 = 1.45;
/// Grid inset inside the panel, all sides.
const PAD: f32 = 10.;

pub(crate) struct TerminalElement {
    session: WeakEntity<TermSession>,
    focus: FocusHandle,
}

impl TerminalElement {
    pub(crate) fn new(session: WeakEntity<TermSession>, focus: FocusHandle) -> Self {
        Self { session, focus }
    }
}

/// Font metrics for the grid, derived from the theme's mono face.
struct Metrics {
    line_height: Pixels,
    cell_width: Pixels,
    font: Font,
    font_size: Pixels,
}

impl Metrics {
    fn new(window: &Window, cx: &App) -> Self {
        let theme = cx.theme();
        let font = font(theme.mono_font_family.clone());
        let font_size = theme.mono_font_size;
        let line_height = px(f32::from(font_size) * LINE_HEIGHT_FACTOR);
        let run = TextRun {
            len: 1,
            font: font.clone(),
            color: rgb(0x000000).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let cell_width = window
            .text_system()
            .shape_line("M".into(), font_size, &[run], None)
            .width()
            .max(px(1.));
        Self { line_height, cell_width, font, font_size }
    }
}

impl IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = LayoutId;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        Some("terminal-grid".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.flex_grow = 1.;
        style.size = size(relative(1.).into(), relative(1.).into());
        let layout_id = window.request_layout(style, [], cx);
        (layout_id, layout_id)
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        window.compute_layout(
            *request_layout,
            size(
                AvailableSpace::Definite(bounds.size.width),
                AvailableSpace::Definite(bounds.size.height),
            ),
            cx,
        );

        // Grid ↔ panel resize handshake: recompute cols/rows from the
        // final bounds. No `notify` here — the drag that changed the
        // bounds already re-renders subsequent frames.
        let m = Metrics::new(window, cx);
        let usable_w = (bounds.size.width - px(2. * PAD)).max(px(0.));
        let usable_h = (bounds.size.height - px(2. * PAD)).max(px(0.));
        let cols = ((usable_w / m.cell_width).floor() as u16).max(2);
        let rows = ((usable_h / m.line_height).floor() as u16).max(2);
        if let Some(session) = self.session.upgrade() {
            session.update(cx, |s, _| s.resize_if_needed(cols, rows));
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(session) = self.session.upgrade() else { return };
        let m = Metrics::new(window, cx);
        let theme = cx.theme();
        let palette = TerminalPalette::new(theme);
        let focused = self.focus.is_focused(window);

        // Clone the grid Arc out of the entity borrow so painting can
        // take `&mut App` freely.
        let term = session.read(cx).grid.term.clone();
        let term_lock = term.lock();
        let mut content = term_lock.renderable_content();
        let cursor = content.cursor;

        let origin = bounds.origin + point(px(PAD), px(PAD));
        paint_grid(&mut content, &m, &palette, origin, window, cx);
        paint_cursor(&cursor, &m, origin, focused, window, cx);
    }
}

/// Split the visible cells into rows, coalesce same-style runs per row
/// and paint background + glyphs.
fn paint_grid(
    content: &mut RenderableContent<'_>,
    m: &Metrics,
    palette: &TerminalPalette,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    // Collect visible cells row-major, remembering where each line starts.
    let mut cells: Vec<&Cell> = Vec::new();
    let mut line_starts: Vec<usize> = Vec::new();
    let mut last_line: Option<i32> = None;
    for indexed in &mut content.display_iter {
        if last_line != Some(indexed.point.line.0) {
            line_starts.push(cells.len());
            last_line = Some(indexed.point.line.0);
        }
        cells.push(indexed.cell);
    }
    line_starts.push(cells.len());

    for (row_ix, range) in line_starts.windows(2).enumerate() {
        let row = &cells[range[0]..range[1]];
        if row.iter().all(|c| c.c == ' ') {
            continue;
        }
        let y = point(
            origin.x,
            origin.y + px(f32::from(m.line_height) * row_ix as f32),
        );

        let mut text = String::with_capacity(row.len() * 2);
        let mut runs: Vec<TextRun> = Vec::new();
        let mut open: Option<(StyleKey, usize)> = None;
        for cell in row {
            let key = StyleKey::of(cell, palette);
            match &mut open {
                Some((k, len)) if *k == key => *len += cell.c.len_utf8(),
                Some((k, len)) => {
                    let (k, len) = (*k, *len);
                    runs.push(k.into_run(len, &m.font));
                    open = Some((key, cell.c.len_utf8()));
                }
                None => open = Some((key, cell.c.len_utf8())),
            }
            text.push(cell.c);
        }
        if let Some((k, len)) = open.take() {
            runs.push(k.into_run(len, &m.font));
        }
        if text.is_empty() {
            continue;
        }

        let shaped = window.text_system().shape_line(text.into(), m.font_size, &runs, None);
        let align = TextAlign::Left;
        let _ = shaped.paint_background(y, m.line_height, align, None, window, cx);
        let _ = shaped.paint(y, m.line_height, align, None, window, cx);
    }
}

fn paint_cursor(
    cursor: &alacritty_terminal::term::RenderableCursor,
    m: &Metrics,
    origin: Point<Pixels>,
    focused: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let x = origin.x + px(f32::from(m.cell_width) * cursor.point.column.0 as f32);
    let y = origin.y + px(f32::from(m.line_height) * cursor.point.line.0 as f32);
    let bounds = Bounds {
        origin: point(x, y),
        size: size(m.cell_width, m.line_height),
    };
    let accent = cx.theme().accent;
    window.paint_quad(fill(
        bounds,
        if focused { accent.opacity(0.30) } else { transparent_black() },
    ));
    window.paint_quad(outline(
        bounds,
        accent.opacity(if focused { 0.9 } else { 0.4 }),
        BorderStyle::Solid,
    ));
}

/// Everything that forces a style change between cells.
#[derive(Clone, Copy, PartialEq)]
struct StyleKey {
    fg: Hsla,
    bg: Option<Hsla>,
    bold: bool,
    italic: bool,
    underline: bool,
    dim: bool,
}

impl StyleKey {
    fn of(cell: &Cell, palette: &TerminalPalette) -> Self {
        let flags = cell.flags;
        let mut fg = palette.color(cell.fg);
        let mut bg = palette.color(cell.bg);
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        let bg = (bg != palette.bg).then_some(bg);
        Self {
            fg,
            bg,
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.contains(Flags::UNDERLINE),
            dim: flags.contains(Flags::DIM),
        }
    }

    fn into_run(self, len: usize, font: &Font) -> TextRun {
        let mut font = font.clone();
        if self.bold {
            font.weight = FontWeight::BOLD;
        }
        if self.italic {
            font.style = FontStyle::Italic;
        }
        let color = if self.dim { self.fg.opacity(0.65) } else { self.fg };
        TextRun {
            len,
            font,
            color,
            background_color: self.bg,
            underline: self.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: None,
        }
    }
}

/// Maps alacritty cell colors onto the current theme.
struct TerminalPalette {
    fg: Hsla,
    bg: Hsla,
    base: [Hsla; 16],
}

impl TerminalPalette {
    fn new(theme: &Theme) -> Self {
        // Agent CLIs lean on saturated brights; the classic VS Code
        // palette reads fine against both theme modes. Theme tokens
        // are used where they exist (red/green/blue/foreground).
        let base = [
            rgb(0x555555).into(),           // black
            theme.red,                      // red
            theme.green,                    // green
            rgb(0xd7af00).into(),           // yellow
            theme.blue,                     // blue
            rgb(0xbc3fbc).into(),           // magenta
            rgb(0x11a8cd).into(),           // cyan
            theme.foreground.opacity(0.85), // white
            theme.foreground.opacity(0.45), // bright black
            theme.red.opacity(1.15),        // bright red
            theme.green.opacity(1.15),      // bright green
            rgb(0xf5f543).into(),           // bright yellow
            theme.blue.opacity(1.15),       // bright blue
            rgb(0xd670d6).into(),           // bright magenta
            rgb(0x29b8db).into(),           // bright cyan
            rgb(0xffffff).into(),           // bright white
        ];
        Self { fg: theme.foreground, bg: theme.background, base }
    }

    fn color(&self, c: TermColor) -> Hsla {
        match c {
            TermColor::Named(NamedColor::Foreground) => self.fg,
            TermColor::Named(NamedColor::Background) => self.bg,
            TermColor::Named(NamedColor::Cursor) => self.fg,
            TermColor::Named(named) => self.base[dimmed_index(named)],
            TermColor::Indexed(i) => match i {
                0..=15 => self.base[i as usize],
                16..=231 => {
                    let i = i - 16;
                    let (r, g, b) = (i / 36, (i % 36) / 6, i % 6);
                    let v = |x: u8| if x == 0 { 0 } else { 55 + 40 * x };
                    rgb_u24(v(r), v(g), v(b)).into()
                }
                gray => {
                    let g = 8 + 10 * (gray.saturating_sub(232));
                    rgb_u24(g, g, g).into()
                }
            },
            TermColor::Spec(rgb) => rgb_u24(rgb.r, rgb.g, rgb.b).into(),
        }
    }
}

fn dimmed_index(named: NamedColor) -> usize {
    match named {
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        NamedColor::DimBlack => 0,
        NamedColor::DimRed => 1,
        NamedColor::DimGreen => 2,
        NamedColor::DimYellow => 3,
        NamedColor::DimBlue => 4,
        NamedColor::DimMagenta => 5,
        NamedColor::DimCyan => 6,
        NamedColor::DimWhite => 7,
        _ => 7,
    }
}

fn rgb_u24(r: u8, g: u8, b: u8) -> Rgba {
    rgb(((r as u32) << 16) | ((g as u32) << 8) | b as u32)
}
