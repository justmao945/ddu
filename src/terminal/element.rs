//! The terminal renderer: a custom [`Element`] that paints the visible
//! window of the alacritty grid as shaped mono lines and owns the
//! grid↔panel resize handshake.

use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::{Color as TermColor, CursorShape, NamedColor};

use super::TermSession;

/// Line height as a factor of the mono font size.
pub(crate) const LINE_HEIGHT_FACTOR: f32 = 1.45;
/// Grid inset inside the panel, all sides.
pub(crate) const PAD: f32 = 10.;
/// Right-edge scrollbar strip width — the mouse hit zone only; the
/// thumb is drawn centered in the 16px bar zone the Base scrollbars
/// (diff panes) use, so both look identical side by side.
pub(crate) const SCROLLBAR_W: f32 = 6.;
/// Minimum thumb height so short scrollback stays grabbable (Base's
/// `MIN_THUMB_SIZE`).
const THUMB_MIN: f32 = 48.;
/// Thumb width at rest / while hovered or dragged, with the
/// right-edge insets that keep it centered in the 16px bar zone.
const THUMB_W: f32 = 6.;
const THUMB_W_ACTIVE: f32 = 8.;
const THUMB_EDGE_INSET: f32 = 5.;
const THUMB_EDGE_INSET_ACTIVE: f32 = 4.;

/// Scrollbar track + thumb rects for the grid area, or None when there
/// is no scrollback. `engaged` (hover or drag) widens the thumb from
/// 6px to 8px like the Base scrollbar; `display_offset` 0 pins it to
/// the bottom (live), `history` to the top.
pub(crate) fn scrollbar_geometry(
    area: Bounds<Pixels>,
    screen_lines: usize,
    history: usize,
    display_offset: usize,
    engaged: bool,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>)> {
    if history == 0 {
        return None;
    }
    let track = Bounds {
        origin: point(
            area.origin.x + area.size.width - px(SCROLLBAR_W),
            area.origin.y,
        ),
        size: size(px(SCROLLBAR_W), area.size.height),
    };
    let thumb_h = (f32::from(track.size.height) * screen_lines as f32
        / (screen_lines + history) as f32)
        .max(THUMB_MIN);
    let travel = (f32::from(track.size.height) - thumb_h).max(0.);
    let frac = display_offset.min(history) as f32 / history as f32;
    let top = track.origin.y + px(travel * (1. - frac));
    let (w, inset) = if engaged {
        (THUMB_W_ACTIVE, THUMB_EDGE_INSET_ACTIVE)
    } else {
        (THUMB_W, THUMB_EDGE_INSET)
    };
    let thumb = Bounds {
        origin: point(area.origin.x + area.size.width - px(inset + w), top),
        size: size(px(w), px(thumb_h)),
    };
    Some((track, thumb))
}

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
pub(crate) struct Metrics {
    pub(crate) line_height: Pixels,
    pub(crate) cell_width: Pixels,
    font: Font,
    font_size: Pixels,
}

impl Metrics {
    pub(crate) fn new(window: &Window, cx: &App) -> Self {
        let theme = cx.theme();
        // Configured terminal font wins; empty = the system mono face.
        let family = cx
            .global::<crate::config::Config>()
            .terminal_font
            .clone()
            .filter(|f| !f.trim().is_empty())
            .unwrap_or_else(|| theme.mono_font_family.to_string());
        let font = font(SharedString::from(family));
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
        Self {
            line_height,
            cell_width,
            font,
            font_size,
        }
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
        let style = Style {
            flex_grow: 1.,
            size: size(relative(1.).into(), relative(1.).into()),
            ..Default::default()
        };
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
            session.update(cx, |s, cx| s.request_resize(cols, rows, cx));
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
        let Some(session) = self.session.upgrade() else {
            return;
        };
        // Register the IME input handler for this frame (a no-op unless
        // focused): composed input — CJK input methods, long-press
        // accents, the emoji picker — commits through it into the PTY.
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, session.clone()),
            cx,
        );
        let m = Metrics::new(window, cx);
        // Same thumb tokens the Base scrollbar styles resolve to (the
        // diff panes): resting/hover colors + the theme corner radius.
        let thumb_rest = cx.theme().tokens.scrollbar_thumb;
        let thumb_hover = cx.theme().tokens.scrollbar_thumb_hover;
        let thumb_radius = cx.theme().radius;
        let palette = TerminalPalette::new(cx.theme());
        window.paint_quad(fill(bounds, palette.bg));
        let focused = self.focus.is_focused(window);

        // Clone the grid Arc out of the entity borrow so painting can
        // take `&mut App` freely.
        let marked = session.read(cx).marked_text.clone();
        let term = session.read(cx).grid.term.clone();
        let term_lock = term.lock();
        let mut content = term_lock.renderable_content();
        let cursor = content.cursor;
        // Mouse hit-testing maps window points through these bounds.
        session.read(cx).grid_bounds.set(Some(bounds));

        // Center the grid inside the padded element: the floor()ed
        // row/col count otherwise leaves up to a full cell of slack
        // below the last line, which reads as a dead band at the
        // bottom of the pane. Split the slack evenly on both axes.
        let (cols, rows) = {
            let g = term_lock.grid();
            (g.columns(), g.screen_lines())
        };
        let slack_w = (bounds.size.width - px(2. * PAD) - m.cell_width * cols as f32).max(px(0.));
        let slack_h = (bounds.size.height - px(2. * PAD) - m.line_height * rows as f32).max(px(0.));
        let origin = bounds.origin + point(px(PAD) + slack_w / 2., px(PAD) + slack_h / 2.);
        // The content rect the grid actually paints into — selection
        // and mouse-report hit-testing map points against this, not
        // the padded element bounds.
        session.read(cx).content_bounds.set(Some(Bounds {
            origin,
            size: size(m.cell_width * cols as f32, m.line_height * rows as f32),
        }));
        // `display_iter` starts at the topmost visible line (grid line
        // `-display_offset`), so the cursor's screen row is its grid
        // line plus the scroll offset — without this the block cursor
        // and the IME overlay land rows above the text once scrolled.
        let cursor_row = cursor.point.line.0 + content.display_offset as i32;
        // Stash the cursor rect so `bounds_for_range` can anchor the
        // platform's IME candidate popup at the insertion point.
        let cursor_bounds =
            (cursor.shape != CursorShape::Hidden && cursor_row >= 0).then(|| Bounds {
                origin: point(
                    origin.x + px(f32::from(m.cell_width) * cursor.point.column.0 as f32),
                    origin.y + px(f32::from(m.line_height) * cursor_row as f32),
                ),
                size: size(m.cell_width, m.line_height),
            });
        session.read(cx).ime_cursor_bounds.set(cursor_bounds);
        paint_grid(&mut content, &m, &palette, origin, window, cx);
        paint_cursor(
            &cursor, cursor_row, &m, &palette, origin, focused, window, cx,
        );
        // Right-edge scrollbar thumb — macOS-style overlay: shows while
        // the mouse hovers the strip, during scroll/drag activity, or
        // while scrolled back into history; fades after the idle
        // window. Same geometry the mouse handlers hit-test against.
        let (rows, history) = {
            let g = term_lock.grid();
            (g.screen_lines(), g.history_size())
        };
        // Inline the state reads (no term re-lock while held here).
        let engaged = session.read(cx).scrollbar_engaged();
        if session.read(cx).scrollbar_activity() || content.display_offset > 0 {
            if let Some((_track, thumb)) =
                scrollbar_geometry(bounds, rows, history, content.display_offset, engaged)
            {
                // Rounded ends + the hover color mirror the diff panes'
                // Base scrollbars; a radius past half the thumb width
                // is clamped by the renderer, giving capsule ends.
                let mut quad =
                    fill(thumb, if engaged { thumb_hover } else { thumb_rest });
                quad.corner_radii = Corners::all(thumb_radius);
                window.paint_quad(quad);
            }
        }
        if let Some(marked) = marked.filter(|t| !t.is_empty()) {
            paint_marked(
                &marked, &cursor, cursor_row, &m, &palette, origin, window, cx,
            );
        }
    }
}

/// Split the visible cells into rows and paint background + glyphs.
///
/// Each row is laid out segment-by-segment at exact cell boundaries:
/// ASCII runs shape naturally (mono face = cell_width per char), while a
/// `WIDE_CHAR` cell plus its `WIDE_CHAR_SPACER` partner forms a
/// two-character run shaped with `force_width = 2 * cell_width`, so the
/// CJK glyph occupies exactly two grid columns instead of overflowing
/// into the next cell. Zero-width cells (combining marks) and spacer
/// cells are skipped as separate paint targets — they are covered by
/// their base run's shaping.
fn paint_grid(
    content: &mut RenderableContent<'_>,
    m: &Metrics,
    palette: &TerminalPalette,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let selection = content.selection.clone();
    // Collect visible cells row-major, remembering where each line starts.
    let mut cells: Vec<&Cell> = Vec::new();
    let mut selected: Vec<bool> = Vec::new();
    let mut line_starts: Vec<usize> = Vec::new();
    let mut last_line: Option<i32> = None;
    for indexed in &mut content.display_iter {
        if last_line != Some(indexed.point.line.0) {
            line_starts.push(cells.len());
            last_line = Some(indexed.point.line.0);
        }
        selected.push(
            selection
                .as_ref()
                .is_some_and(|s| s.contains(indexed.point)),
        );
        cells.push(indexed.cell);
    }
    line_starts.push(cells.len());

    for (row_ix, range) in line_starts.windows(2).enumerate() {
        let row = &cells[range[0]..range[1]];
        let sel = &selected[range[0]..range[1]];
        let y = origin.y + px(f32::from(m.line_height) * row_ix as f32);

        // Cell backgrounds first, merged into one rect per same-color
        // run — a wide char and its spacer (or any adjacent same-bg
        // cells) share a seamless band, no hairline seams.
        let mut col = 0;
        while col < row.len() {
            let bg = StyleKey::of(row[col], palette).bg;
            let start = col;
            while col < row.len() && StyleKey::of(row[col], palette).bg == bg {
                col += 1;
            }
            if let Some(bg) = bg {
                window.paint_quad(fill(
                    Bounds {
                        origin: point(origin.x + m.cell_width * start as f32, y),
                        size: size(m.cell_width * (col - start) as f32, m.line_height),
                    },
                    bg,
                ));
            }
        }

        // Selection wash above backgrounds, under glyphs — runs even
        // for blank rows, which the text pass below skips.
        let mut col = 0;
        while col < row.len() {
            if !sel[col] {
                col += 1;
                continue;
            }
            let start = col;
            while col < row.len() && sel[col] {
                col += 1;
            }
            window.paint_quad(fill(
                Bounds {
                    origin: point(origin.x + m.cell_width * start as f32, y),
                    size: size(m.cell_width * (col - start) as f32, m.line_height),
                },
                palette.selection,
            ));
        }

        if row.iter().all(|c| c.c == ' ') {
            continue;
        }

        // Split the row into paintable segments at style boundaries,
        // widening wide-char groups to their two-column footprint.
        // Box-drawing/block chars paint as vector rects (font glyphs
        // leave vertical gaps at this line height).
        let mut segs: Vec<Seg> = Vec::new();
        let mut ix = 0;
        while ix < row.len() {
            let cell = row[ix];
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
                || cell.c == '\0'
            {
                ix += 1;
                continue;
            }
            if super::boxart::is_vector(cell.c) {
                let key = StyleKey::of(cell, palette);
                segs.push(Seg::Vector {
                    c: cell.c,
                    fg: if key.dim {
                        key.fg.opacity(0.65)
                    } else {
                        key.fg
                    },
                });
                ix += 1;
                continue;
            }
            let wide = cell.flags.contains(Flags::WIDE_CHAR);
            // Same-style run extent. A wide char paints alone (its spacer
            // cell is covered by the forced two-column shaping); ASCII
            // continues while style matches and no wide cell intervenes.
            let mut run_end = ix + 1;
            while run_end < row.len()
                && !row[run_end].flags.contains(Flags::WIDE_CHAR)
                && !row[run_end]
                    .flags
                    .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                && row[run_end].c != '\0'
                && !super::boxart::is_vector(row[run_end].c)
                && StyleKey::of(row[run_end], palette) == StyleKey::of(cell, palette)
            {
                run_end += 1;
            }
            let text: String = row[ix..run_end].iter().map(|c| c.c).collect();
            segs.push(Seg::Text {
                text,
                key: StyleKey::of(cell, palette),
                // A wide char owns two columns: itself + its spacer.
                cols: if wide { 2. } else { (run_end - ix) as f32 },
                force_width: wide.then(|| px(f32::from(m.cell_width) * 2.)),
            });
            ix = run_end;
        }
        let mut x = origin.x;
        for seg in segs {
            match seg {
                Seg::Vector { c, fg } => {
                    super::boxart::paint(
                        c,
                        Bounds {
                            origin: point(x, y),
                            size: size(m.cell_width, m.line_height),
                        },
                        fg,
                        window,
                    );
                    x += m.cell_width;
                }
                Seg::Text {
                    text,
                    key,
                    cols,
                    force_width,
                } => {
                    if text.is_empty() {
                        x += px(f32::from(m.cell_width) * cols);
                        continue;
                    }
                    let run = key.into_run(text.len(), &m.font);
                    let shaped = window.text_system().shape_line(
                        text.into(),
                        m.font_size,
                        &[run],
                        force_width,
                    );
                    // Backgrounds were already painted as merged row runs above.
                    let _ = shaped.paint(
                        point(x, y),
                        m.line_height,
                        TextAlign::Left,
                        Some(shaped.width()),
                        window,
                        cx,
                    );
                    x += px(f32::from(m.cell_width) * cols);
                }
            }
        }
    }
}

/// One paintable segment within a row.
enum Seg {
    /// A run of same-styled cells shaped as text.
    Text {
        text: String,
        key: StyleKey,
        /// Grid columns this segment covers (wide chars count double).
        cols: f32,
        /// `Some(w)` pins shaping to an exact pixel width (wide-char runs).
        force_width: Option<Pixels>,
    },
    /// A single box-drawing/block cell drawn as vector rects.
    Vector { c: char, fg: Hsla },
}

/// IME preedit ("marked") text: underlined, on a subtle wash, painted
/// over the cells at the cursor — alacritty-style overlay; the grid
/// content underneath is left in place since the preedit is not yet
/// terminal content.
fn paint_marked(
    text: &str,
    cursor: &alacritty_terminal::term::RenderableCursor,
    cursor_row: i32,
    m: &Metrics,
    palette: &TerminalPalette,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    if cursor_row < 0 {
        return;
    }
    let x = origin.x + px(f32::from(m.cell_width) * cursor.point.column.0 as f32);
    let y = origin.y + px(f32::from(m.line_height) * cursor_row as f32);
    let run = TextRun {
        len: text.len(),
        font: m.font.clone(),
        color: palette.fg,
        background_color: None,
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(palette.fg),
            wavy: false,
        }),
        strikethrough: None,
    };
    let shaped =
        window
            .text_system()
            .shape_line(text.to_string().into(), m.font_size, &[run], None);
    let bounds = Bounds {
        origin: point(x, y),
        size: size(shaped.width(), m.line_height),
    };
    window.paint_quad(fill(bounds, palette.fg.opacity(0.12)));
    let _ = shaped.paint(
        point(x, y),
        m.line_height,
        TextAlign::Left,
        Some(shaped.width()),
        window,
        cx,
    );
}
fn paint_cursor(
    cursor: &alacritty_terminal::term::RenderableCursor,
    cursor_row: i32,
    m: &Metrics,
    palette: &TerminalPalette,
    origin: Point<Pixels>,
    focused: bool,
    window: &mut Window,
    _cx: &mut App,
) {
    if cursor.shape == alacritty_terminal::vte::ansi::CursorShape::Hidden || cursor_row < 0 {
        return;
    }
    let x = origin.x + px(f32::from(m.cell_width) * cursor.point.column.0 as f32);
    let y = origin.y + px(f32::from(m.line_height) * cursor_row as f32);
    let bounds = Bounds {
        origin: point(x, y),
        size: size(m.cell_width, m.line_height),
    };
    // Focused AND the window active: solid block BUT translucent — a
    // fully opaque cursor covers the glyph (and any completion/IME
    // preview) underneath; 0.62 keeps the cell readable while still
    // reading as a cursor. Anything else (unfocused pane, another
    // window frontmost): pure hollow outline, no fill — the cell (and
    // the glyph under the cursor) stays fully readable.
    if focused && window.is_window_active() {
        window.paint_quad(fill(bounds, palette.cursor.opacity(0.62)));
    } else {
        window.paint_quad(outline(bounds, palette.cursor, BorderStyle::Solid));
    }
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
        let color = if self.dim {
            self.fg.opacity(0.65)
        } else {
            self.fg
        };
        TextRun {
            len,
            font,
            color,
            // Painted as merged row runs in `paint_grid`, not per segment.
            background_color: None,
            underline: self.underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: None,
        }
    }
}
/// Maps alacritty cell colors onto a fixed terminal palette.
///
/// Terminals need stable, saturated ANSI colors — theme-derived tints
/// wash out agent CLIs' output. Both palettes are Zed's official
/// "One Dark" / "One Light" terminal ANSI ramps
/// (zed-industries/zed `assets/themes/one/one.json`).
struct TerminalPalette {
    fg: Hsla,
    bg: Hsla,
    /// Block cursor fill / unfocused outline.
    cursor: Hsla,
    /// Selection wash — opaque, contrasts with `bg` in both modes
    /// (VSCode dark / macOS light selection blues).
    selection: Hsla,
    base: [Hsla; 16],
}

impl TerminalPalette {
    fn new(theme: &Theme) -> Self {
        match theme.mode {
            ThemeMode::Dark => Self {
                fg: rgb(0xabb2bf).into(),
                bg: rgb(0x282c34).into(),
                cursor: rgb(0x61afef).into(),
                selection: rgb(0x264f78).into(),
                base: one_dark_palette(),
            },
            ThemeMode::Light => Self {
                fg: rgb(0x2a2c33).into(),
                bg: rgb(0xfafafa).into(),
                cursor: rgb(0x2f5af3).into(),
                selection: rgb(0xb3d7ff).into(),
                base: one_light_palette(),
            },
        }
    }
}

/// Zed "One Dark" terminal ANSI colors: dim row 0-7, bright row 8-15.
const ONE_DARK: [u32; 16] = [
    0x282c34, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf, //
    0x636d83, 0xEA858B, 0xAAD581, 0xFFD885, 0x85C1FF, 0xD398EB, 0x6ED5DE, 0xfafafa,
];

/// Zed "One Light" terminal ANSI colors: dim row 0-7, bright row 8-15.
const ONE_LIGHT: [u32; 16] = [
    0x000000, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0x950095, 0x0997b3, 0xbbbbbb, //
    0x555555, 0xde3e35, 0x3f953a, 0xd2b67c, 0x2f5af3, 0xa00095, 0x0bbcd6, 0xffffff,
];

fn one_dark_palette() -> [Hsla; 16] {
    ONE_DARK.map(|c| rgb(c).into())
}

fn one_light_palette() -> [Hsla; 16] {
    ONE_LIGHT.map(|c| rgb(c).into())
}

impl TerminalPalette {
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
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
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

#[cfg(test)]
mod palette_tests {
    use super::{NamedColor, dimmed_index};
    #[test]
    fn standard_ansi_colors_do_not_fall_back_to_white() {
        for (color, expected) in [
            NamedColor::Black,
            NamedColor::Red,
            NamedColor::Green,
            NamedColor::Yellow,
            NamedColor::Blue,
            NamedColor::Magenta,
            NamedColor::Cyan,
            NamedColor::White,
        ]
        .into_iter()
        .zip(0..8)
        {
            assert_eq!(dimmed_index(color), expected);
        }
        assert_eq!(dimmed_index(NamedColor::BrightRed), 9);
        assert_eq!(dimmed_index(NamedColor::DimRed), 1);
    }
}

#[cfg(test)]
mod scrollbar_tests {
    use super::{SCROLLBAR_W, scrollbar_geometry};
    use gpui_kit::{Bounds, point, px, size};

    fn area() -> Bounds<gpui_kit::Pixels> {
        Bounds {
            origin: point(px(10.), px(10.)),
            size: size(px(500.), px(240.)),
        }
    }

    #[test]
    fn no_scrollback_no_scrollbar() {
        assert!(scrollbar_geometry(area(), 24, 0, 0, false).is_none());
    }

    #[test]
    fn thumb_tracks_the_scroll_fraction() {
        // 24 rows visible, 60 in scrollback: thumb = 240·24/84 ≈ 68.6px
        // (history small enough to stay above the 48px minimum).
        let (track, bottom) = scrollbar_geometry(area(), 24, 60, 0, false).unwrap();
        let (_, top) = scrollbar_geometry(area(), 24, 60, 60, false).unwrap();
        let thumb_h = f32::from(bottom.size.height);
        assert!((thumb_h - 240. * 24. / 84.).abs() < 0.5);
        // Live bottom (offset 0) pins the thumb to the track bottom...
        assert!((f32::from(bottom.origin.y) - (10. + 240. - thumb_h)).abs() < 0.5);
        // ...and full history (offset == history) to the track top.
        assert!((f32::from(top.origin.y) - 10.).abs() < 0.01);
        // The hit-test strip hugs the area's right edge...
        assert!((f32::from(track.origin.x) - (10. + 500. - SCROLLBAR_W)).abs() < 0.01);
        // ...while the resting thumb is centered in the 16px bar zone
        // (6px wide, 5px off the edge).
        assert!((f32::from(bottom.origin.x) - (10. + 500. - 5. - 6.)).abs() < 0.01);
    }

    #[test]
    fn engaged_thumb_widens_toward_the_edge() {
        let (_, engaged) = scrollbar_geometry(area(), 24, 100, 0, true).unwrap();
        assert!((f32::from(engaged.size.width) - 8.).abs() < 0.01);
        assert!((f32::from(engaged.origin.x) - (10. + 500. - 4. - 8.)).abs() < 0.01);
    }

    #[test]
    fn huge_scrollback_keeps_grabbable_thumb() {
        let (_, thumb) = scrollbar_geometry(area(), 24, 100_000, 50_000, false).unwrap();
        assert_eq!(f32::from(thumb.size.height), 48.);
    }
}
