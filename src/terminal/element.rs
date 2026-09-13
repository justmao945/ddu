//! The terminal renderer: a custom [`Element`] that paints the visible
//! window of the alacritty grid as shaped mono lines and owns the
//! grid↔panel resize handshake.

use gpui_kit::component::ActiveTheme as _;

use gpui_kit::*;

use alacritty_terminal::grid::Dimensions as _;
use alacritty_terminal::term::RenderableContent;
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::vte::ansi::CursorShape;

use super::palette::TerminalPalette;
use super::scrollbar::scrollbar_geometry;
use super::{TermMatch, TermSession};

/// Line height as a factor of the mono font size.
pub(crate) const LINE_HEIGHT_FACTOR: f32 = 1.45;
/// Grid inset inside the panel, all sides.
pub(crate) const PAD: f32 = 10.;

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
        // The theme's mono family already mirrors `terminal_font`
        // (`ui::apply_mono_typography`), falling back to the platform
        // stock mono face when unset.
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
    /// The visible window's text, captured for the accessibility tree (see
    /// [`Self::a11y_synthetic_children`]). Captured only while an assistive
    /// client is attached: reading the grid and building the string costs
    /// more than the paint the stream throttle exists to bound, so it must
    /// not run on the normal path.
    type PrepaintState = Option<SharedString>;

    fn id(&self) -> Option<ElementId> {
        Some("terminal-grid".into())
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    /// The grid as a terminal: assistive technology reads the text, which
    /// this element supplies as one synthetic node per visible row
    /// ([`Self::a11y_synthetic_children`]) rather than as glyph children
    /// (the glyphs are vector strokes, not text nodes).
    fn a11y_role(&self) -> Option<Role> {
        Some(Role::Terminal)
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

        if !window.is_a11y_active() {
            return None;
        }
        let session = self.session.upgrade()?;
        let term = session.read(cx).grid.term.clone();
        let term_lock = term.lock();
        let mut content = term_lock.renderable_content();
        let mut text = String::new();
        let mut last_line: Option<i32> = None;
        for indexed in &mut content.display_iter {
            if last_line != Some(indexed.point.line.0) {
                // A row's trailing blanks are padding, not content.
                while text.ends_with(' ') {
                    text.pop();
                }
                text.push('\n');
                last_line = Some(indexed.point.line.0);
            }
            let cell = indexed.cell;
            // A wide char's spacer and the NUL padding past a line's end
            // are not text (`paint_grid` skips the same cells).
            if cell.c == '\0'
                || cell.flags.contains(Flags::WIDE_CHAR_SPACER)
                || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            text.push(cell.c);
        }
        drop(content);
        drop(term_lock);
        while text.ends_with('\n') || text.ends_with(' ') {
            text.pop();
        }
        Some(text.into())
    }

    fn a11y_synthetic_children(
        &mut self,
        prepaint: &mut Self::PrepaintState,
        builder: &mut A11ySubtreeBuilder,
    ) {
        let Some(text) = prepaint else {
            return;
        };
        // The grid's text *is* this node's value: macOS maps the role to
        // AXTextArea and reads the value (per-row child nodes are pruned
        // there — verified), and this callback is the one accessibility
        // hook that runs after prepaint, which is where the grid is read.
        builder.parent_node().set_value(text.to_string());
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
        let paint_start = std::time::Instant::now();
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
        // Find-bar hits for this frame: an Rc grab, replaced wholesale
        // on every rescan.
        let (search_matches, search_current) = {
            let s = session.read(cx);
            (s.search.matches.clone(), s.search.current)
        };
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
        paint_grid(
            &mut content,
            &m,
            &palette,
            origin,
            &search_matches,
            search_current,
            window,
            cx,
        );
        paint_cursor(
            &cursor, cursor_row, &m, &palette, origin, focused, window, cx,
        );
        // Right-edge scrollbar thumb — macOS-style overlay: shows while
        // the mouse hovers the strip or during scroll/drag activity;
        // fades after the idle window even in the scrollback (the next
        // scroll tick re-shows the position). Same geometry the mouse
        // handlers hit-test against.
        let (rows, history) = {
            let g = term_lock.grid();
            (g.screen_lines(), g.history_size())
        };
        // Inline the state reads (no term re-lock while held here).
        let engaged = session.read(cx).scrollbar_engaged();
        if session.read(cx).scrollbar_activity() {
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
        // Feed the stream throttle: pacing the repaint rate is the one
        // lever that scales the *whole* window redraw (layout, scene,
        // Metal) rather than just this element.
        session.update(cx, |s, _| s.note_paint_cost(paint_start.elapsed()));
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
    search: &[TermMatch],
    search_current: usize,
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

        // Find-bar wash: above the cell backgrounds, below the
        // selection wash. Hits are sorted by line, so each visible row
        // binary-searches the slice for its absolute line (`row_ix` in
        // grid coordinates is the row minus the scroll offset, the
        // same mapping `cell_at` uses). Columns are cell columns, so
        // the rect math is the cell math.
        if !search.is_empty() {
            let abs_line = row_ix as i32 - content.display_offset as i32;
            let lo = search.partition_point(|hit| hit.line < abs_line);
            let hi = search.partition_point(|hit| hit.line <= abs_line);
            if lo < hi {
                let yellow = cx.theme().yellow;
                for (k, hit) in search[lo..hi].iter().enumerate() {
                    let x0 = hit.start.min(row.len());
                    let x1 = hit.end.min(row.len()).max(x0 + 1);
                    let rect = Bounds {
                        origin: point(origin.x + m.cell_width * x0 as f32, y),
                        size: size(
                            m.cell_width * (x1 - x0) as f32,
                            m.line_height,
                        ),
                    };
                    let current = lo + k == search_current;
                    window.paint_quad(fill(
                        rect,
                        yellow.opacity(if current { 0.30 } else { 0.14 }),
                    ));
                    if current {
                        // Outline so the active hit reads even on a
                        // row full of colored cells.
                        let mut outline = fill(rect, yellow.opacity(0.0));
                        outline.border_color = yellow.opacity(0.85);
                        window.paint_quad(outline);
                    }
                }
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
