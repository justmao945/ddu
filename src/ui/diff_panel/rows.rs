//! The pane's rows: the virtual list's declared sizes, the per-row build
//! (gutter, sign column, selection participant), the syntax runs a row
//! paints, and the content-width measurement the horizontal scrollbar
//! rides on.

use super::*;


/// Builds the rows of one visible slice. The walk is the same
/// [`DiffFile::rows`] order `diff::match_rows` numbers, so a row's
/// item index IS its search index; a hunk header is spaced iff it is
/// not the file's first row.
/// Declared row heights for the virtual list. Line rows are single
/// nowrap lines of `text_sm`, so one height fits all of them (the number
/// gutter carries the row's only extra: `pt(2px)`); hunk header bands add
/// their padding, with extra top spacing between hunks; a note row is a
/// bare line. Computed with the framework's own text-style math so the
/// declared sizes match what the rows lay out at.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct RowHeights {
    line: Pixels,
    header: Pixels,
    header_spaced: Pixels,
    meta: Pixels,
}

impl RowHeights {
    pub(super) fn new(window: &Window) -> Self {
        let mut style = window.text_style().clone();
        style.font_size = rems(0.875).into();
        let lh = style.line_height_in_pixels(window.rem_size());
        Self {
            line: lh + px(2.),
            // py_1 band + pb_1 wrapper.
            header: lh + px(8. + 4.),
            // + pt_2 between hunks.
            header_spaced: lh + px(8. + 4. + 8.),
            meta: lh,
        }
    }
}

/// Height of the row at `ix`, in the stream's item order.
pub(super) fn row_height(stream: &RowStream<'_>, ix: usize, heights: RowHeights) -> Pixels {
    match stream.row(ix) {
        Some(PaneRow::Header(_)) if ix > 0 => heights.header_spaced,
        Some(PaneRow::Header(_)) => heights.header,
        Some(PaneRow::Line(_)) => heights.line,
        // The trailing note row, and any index past the end.
        _ => heights.meta,
    }
}

/// Every row's height, in the stream's item order — what the virtual
/// list is handed so it can place a slice without measuring the rows it
/// does not build. Widths are unused (it measures one row for the cross
/// axis); the rows carry the definite `content_w` themselves, which is
/// what gives the horizontal scrollbar its range and keeps the +/-
/// tint bands uniform.
pub(super) fn row_sizes(stream: &RowStream<'_>, heights: RowHeights) -> Vec<gpui_kit::Size<Pixels>> {
    (0..stream.len())
        .map(|ix| size(px(0.), row_height(stream, ix, heights)))
        .collect()
}

/// Builds the rows of one visible slice. A row's item index IS its
/// search index, whichever surface produced the stream, so the find
/// bar's `scroll_to_item` and the highlight set need no mode-specific
/// logic.
pub(super) fn render_rows(
    this: &AppView,
    range: std::ops::Range<usize>,
    content_w: Pixels,
    gutter_w: Pixels,
    heights: RowHeights,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> Vec<AnyElement> {
    let (Some(stream), _) = pane_rows(this) else {
        return Vec::new();
    };
    let matches = &this.diff_search.matches;
    let current_row = matches.get(this.diff_search.current).copied();
    let mut rows: Vec<AnyElement> = Vec::with_capacity(range.end.saturating_sub(range.start));
    for ix in range.clone() {
        let Some(row) = stream.row(ix) else {
            continue;
        };
        rows.push(match row {
            PaneRow::Header(header) => row_box(
                hunk_header(header.to_string(), ix > 0, cx),
                content_w,
                row_height(&stream, ix, heights),
            )
            .into_any_element(),
            PaneRow::Line(line) => {
                let hit = matches.binary_search(&ix).is_ok();
                let runs = line_runs(&stream, ix, cx);
                row_box(
                    diff_line(ix, line, hit, current_row == Some(ix), gutter_w, runs, cx),
                    content_w,
                    heights.line,
                )
                .into_any_element()
            }
            PaneRow::Note => {
                row_box(meta_row(&note_text(this, &stream), cx), content_w, heights.meta)
                    .into_any_element()
            }
        });
    }
    // The rendered slice reached the cap note: grow this file's budget
    // and reload (next frame — this closure runs mid-layout, the entity
    // is on the stack).
    if let Some(note_ix) = stream.note_ix() {
        if range.start <= note_ix && note_ix < range.end && note_expandable(this, &stream) {
            if let Some(path) = this.current_diff_path().map(str::to_owned) {
                cx.on_next_frame(window, move |this, _window, cx| {
                    this.expand_diff_limit(&path, cx);
                });
            }
        }
    }
    rows
}

pub(super) fn row_box<E: Styled>(el: E, w: Pixels, h: Pixels) -> E {
    el.w(w).min_w_full().h(h)
}

/// One-line note row (binary file, truncation cap): clipped to its
/// row box instead of wrapping, the virtual list owns its height.
pub(super) fn meta_row(text: &str, cx: &App) -> Div {
    crate::ui::meta_text(text, cx)
        .whitespace_nowrap()
        .overflow_hidden()
}

pub(super) fn diff_line(
    id: usize,
    line: &DiffLine,
    hit: bool,
    current: bool,
    gutter_w: Pixels,
    runs: Option<Vec<TextRun>>,
    cx: &mut Context<AppView>,
) -> Stateful<Div> {
    // The gutter numbers the file as it is *now*: a context or added
    // line carries its line number in the working tree, a deleted line
    // carries none (it is not in the file any more).
    let number = line.new_no.map(|n| n.to_string()).unwrap_or_default();
    let tint = match line.kind {
        '+' => Some(cx.theme().green.opacity(0.12)),
        '-' => Some(cx.theme().red.opacity(0.12)),
        _ => None,
    };
    // Find matches repaint the row: the current match strongest, every
    // other hit subtler. Search yellow wins over the +/- tint while
    // the bar is up — the sign glyphs still carry the +/- colors — and
    // vanish with it, restoring the plain diff look.
    let search_bg = if current {
        Some(cx.theme().yellow.opacity(0.30))
    } else if hit {
        Some(cx.theme().yellow.opacity(0.13))
    } else {
        None
    };
    let mono = cx.theme().mono_font_family.clone();
    let sign_color = match line.kind {
        '+' => cx.theme().green,
        '-' => cx.theme().red,
        _ => cx.theme().foreground.opacity(0.0),
    };
    let text = line.text.clone();

    div()
        .id(("diff-line", id))
        .flex()
        .items_start()
        .min_w_full()
        .font_family(mono.clone())
        .text_sm()
        .when_some(search_bg.or(tint), |el, bg| el.bg(bg))
        .child(gutter(number, gutter_w, cx))
        .child(
            div()
                .w(px(14.))
                .flex_shrink_0()
                .text_color(sign_color)
                .child(line.kind.to_string()),
        )
        .child(
            div()
                .flex_shrink_0()
                .pl_2()
                .pr_3()
                // Each line's text is its own window-selection
                // participant, ordered by `document_order`, so drag
                // selection spans lines and ⌘C copies them joined
                // with newlines (no gutter or sign in the copy).
                .child(line_text(id, text, mono, runs, cx)),
        )
}

/// One line's text: the syntax-highlighted element when the row has runs
/// for it, the plain selectable run otherwise (Diff mode, a spliced
/// deletion, a file the build has no grammar for). Both are one selection
/// participant ordered by the row index — that is what the drag selection
/// and the copy path ride on.
pub(super) fn line_text(
    id: usize,
    text: String,
    mono: SharedString,
    runs: Option<Vec<TextRun>>,
    cx: &App,
) -> AnyElement {
    let style = TextStyleRefinement {
        font_family: Some(mono),
        font_size: Some(rems(0.875).into()),
        color: Some(cx.theme().foreground.opacity(0.85)),
        white_space: Some(WhiteSpace::Nowrap),
        ..Default::default()
    };
    match runs {
        Some(runs) => crate::ui::code_text::CodeText::new(("diff-text", id), text, runs, style)
            .document_order(id as u64)
            .into_any_element(),
        None => SelectableText::new(("diff-text", id), text)
            .document_order(id as u64)
            .text_style(style)
            .into_any_element(),
    }
}

/// The row's syntax runs, or `None` when there is nothing to color: the
/// view parsed no grammar for this file, or the row is not in it (a
/// spliced deletion). Runs tile the whole line — a gap between tokens
/// takes the pane's own text color — because `TextRun`s are what the
/// shaper lays out, not an overlay on top of a styled string.
pub(super) fn line_runs(stream: &crate::diff::RowStream<'_>, ix: usize, cx: &App) -> Option<Vec<TextRun>> {
    let Some(crate::diff::PaneRow::Line(line)) = stream.row(ix) else {
        return None;
    };
    let text = line.text.as_str();
    let styles = stream.row_styles(ix, cx.theme().highlight_theme.as_ref());
    if styles.is_empty() {
        return None;
    }
    let font = gpui_kit::font(cx.theme().mono_font_family.clone());
    let base = cx.theme().foreground.opacity(0.85);
    let mut runs = Vec::with_capacity(styles.len() + 2);
    let mut at = 0usize;
    for (range, style) in styles {
        let start = clamp_to_boundary(text, range.start.min(text.len()));
        if start > at {
            runs.push(run(&text[at..start], &font, base));
        }
        let end = clamp_to_boundary(text, range.end.min(text.len()).max(start));
        let mut styled = run(&text[start..end], &font, style.color.unwrap_or(base));
        if let Some(weight) = style.font_weight {
            styled.font.weight = weight;
        }
        if let Some(slant) = style.font_style {
            styled.font.style = slant;
        }
        runs.push(styled);
        at = end;
    }
    if at < text.len() {
        runs.push(run(&text[at..], &font, base));
    }
    Some(runs)
}

/// A run's color and font; the length is the slice's byte length, which
/// is what gpui's shaper counts.
pub(super) fn run(text: &str, font: &Font, color: Hsla) -> TextRun {
    TextRun {
        len: text.len(),
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

/// `index` moved down to the nearest char boundary: the highlighter clips
/// its ranges to boundaries, but a panicking slice is not a risk worth
/// taking on text the pane does not own.
pub(super) fn clamp_to_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Longest lines shaped exactly per render (bound on text-system calls).
pub(super) const MEASURE_CANDIDATES: usize = 16;
/// Chrome left of a line's text besides the number gutter (its width is
/// measured per stream — see `gutter_width`): the sign column (14px) and
/// the text block's `pl_2`/`pr_3` padding.
pub(super) const LINE_CHROME_EXTRAS: f32 = 14. + 8. + 12.;
/// Hunk header horizontal padding (`px_2` on both sides).
pub(super) const HEADER_CHROME: f32 = 16.;

/// Width the content column needs so the longest line never clips.
/// Candidates come from the stream — a whole-file view hands back the
/// handful it ranked when it was built, so a 200k-line file costs the
/// same here as a small one — are ordered by a display-cell estimate
/// (non-ASCII ~2 cells) and the top few are shaped exactly with the mono
/// font at `text_sm`.
pub(super) fn measure_content_width(
    stream: &RowStream<'_>,
    gutter_w: Pixels,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> Pixels {
    let line_chrome = f32::from(gutter_w) + LINE_CHROME_EXTRAS;
    let font = Font {
        family: cx.theme().mono_font_family.clone(),
        ..Default::default()
    };
    let size = px(0.75 * f32::from(window.rem_size()));

    let mut candidates: Vec<(usize, &str, f32)> = stream
        .width_candidates()
        .into_iter()
        .map(|(text, is_header)| {
            (
                crate::diff::cells(text),
                text,
                if is_header { HEADER_CHROME } else { line_chrome },
            )
        })
        .collect();
    candidates.sort_unstable_by(|a, b| b.0.cmp(&a.0));

    let mut max_w = px(0.);
    for (_, text, chrome) in candidates.into_iter().take(MEASURE_CANDIDATES) {
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color: cx.theme().foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let w = window
            .text_system()
            .shape_line(text.into(), size, &[run], None)
            .width()
            + px(chrome);
        if w > max_w {
            max_w = w;
        }
    }
    max_w
}

/// The `@@ …` hunk header as a full-width band: subtle background,
/// muted mono text, spanning the whole content column like the code
/// rows below it instead of hugging the header text. `spaced` adds the
/// inter-hunk gap the flattened layout lost with the per-hunk wrappers.
/// The spacing lives in the wrapper's padding — not margins — because
/// a `v_virtual_list` item is laid out as a root: root margins never
/// apply, and the wrapper's own box is what `RowHeights` counts.
pub(super) fn hunk_header(header: String, spaced: bool, cx: &mut Context<AppView>) -> Div {
    div()
        .when(spaced, |el| el.pt_2())
        .pb_1()
        .child(
            div()
                .w_full()
                .px_2()
                .py_1()
                .rounded(cx.theme().radius)
                .bg(cx.theme().foreground.opacity(0.05))
                .text_sm()
                .font_family(cx.theme().mono_font_family.clone())
                .text_color(cx.theme().foreground.opacity(0.5))
                .child(header),
        )
}

/// Gutter width for the stream's largest line number, measured in the
/// mono font at `text_sm`: the stock 36px (scaled) fits three digits, and
/// a wider number would wrap into the next row (row heights are fixed).
/// Floored at the stock width so small diffs don't shift.
pub(super) fn gutter_width(stream: &RowStream<'_>, window: &mut Window, cx: &mut Context<AppView>) -> Pixels {
    let digits = stream.max_line_no().to_string().len();
    let sample: SharedString = "8".repeat(digits).into();
    let run = TextRun {
        len: sample.len(),
        font: Font {
            family: cx.theme().mono_font_family.clone(),
            ..Default::default()
        },
        color: cx.theme().foreground,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let size = px(0.875 * f32::from(window.rem_size()));
    let digits_w = window
        .text_system()
        .shape_line(sample, size, &[run], None)
        .width();
    // `pr_2` keeps the digits off the sign column, plus a pixel of slack.
    // One gutter, not two: the floor only has to keep a one-digit file
    // from looking cramped.
    (digits_w + px(8. + 1.)).max(px(scaled(24.)))
}

pub(super) fn gutter(no: String, w: Pixels, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .w(w)
        .flex_shrink_0()
        .text_right()
        .pr_2()
        // Fixed-height row: a number wider than the gutter clips instead
        // of wrapping onto the next row (measured width prevents it).
        .whitespace_nowrap()
        .overflow_hidden()
        .text_sm()
        .pt(px(2.))
        .text_color(cx.theme().foreground.opacity(0.35))
        .child(no)
}
