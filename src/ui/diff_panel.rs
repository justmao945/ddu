//! Right pane: the selected file's hunks. Header: branch + change
//! stats. The file tree lives in the sidebar's lower layer
//! ([`super::diff_tree`]); this pane always shows the current
//! selection. Diff lines never truncate — long lines scroll
//! horizontally with a visible scrollbar.

use std::rc::Rc;

use super::{panel_header_px, scaled};
use super::diff_tree::plus_minus;
use super::panel_view;
use crate::app::AppView;
use crate::diff::{DiffFile, DiffLine, DiffRow};
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{self, Input};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// The pane's layout box: one definition, read by the pane's own root
/// element and by the shell's cached mount (`panel_view!` explains why
/// the composer has to state it).
pub(crate) fn root_style() -> StyleRefinement {
    StyleRefinement::default()
        .flex()
        .flex_col()
        .size_full()
        .min_w_0()
        // Anchor for the find bar's absolute overlay.
        .relative()
        .overflow_hidden()
}

panel_view!(
    /// Right pane: the selected file's changes.
    PanelView,
    render
);

pub(crate) fn render(
    this: &AppView,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> AnyElement {
    // The file strip only makes sense with a selected file — without
    // one the body's empty state carries the panel on its own.
    let has_file = this
        .diff
        .as_ref()
        .is_some_and(|d| !d.is_empty() && this.diff_file.is_some_and(|ix| ix < d.files.len()));
    let mut root = div();
    *root.style() = root_style();
    root.bg(cx.theme().background)
        // Clicking anywhere in the panel moves focus off the terminal,
        // so its block cursor goes hollow. The find bar stops its own
        // mouse downs (see `find_bar`) so this can't steal focus back
        // from the input mid-keystroke.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.window_focus.focus(window, cx)),
        )
        .when(has_file, |el| el.child(header(this, cx)))
        .child(body(this, window, cx))
        .when(this.diff_search.open, |el| el.child(find_bar(this, cx)))
        .into_any_element()
}

fn header(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane shows exactly one file: the tree selection. Its path
    // leads the header, its +/- figures close the line. The totals
    // ("N files changed") live atop the tree layer.
    let file = this
        .diff
        .as_ref()
        .and_then(|d| this.diff_file.and_then(|ix| d.files.get(ix)));

    div()
        .h(px(panel_header_px()))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_sm()
                .font_medium()
                .text_color(cx.theme().foreground.opacity(0.9))
                .child(file.map(|f| f.path.clone()).unwrap_or_default()),
        )
        .child(div().flex_1())
        .when_some(file, |el, f| el.child(plus_minus(f.added, f.removed, cx)))
}

fn body(this: &AppView, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane mirrors the tree: no active session → no diff content.
    if this.current_session().is_none() {
        return empty("No active session — select one in the project tree.", cx)
            .into_any_element();
    }
    let Some(diff) = &this.diff else {
        return empty(this.diff_error.as_deref().unwrap_or("Loading changes…"), cx)
            .into_any_element();
    };
    if diff.is_empty() {
        return empty("No changes — working tree clean.", cx).into_any_element();
    }
    // No selection, no content: the pane stays empty until a tree
    // click picks a file.
    let Some(file_ix) = this.diff_file.filter(|ix| *ix < diff.files.len()) else {
        return empty("Select a file in the tree.", cx).into_any_element();
    };
    let file = &diff.files[file_ix];
    let gutter_w = gutter_width(file, window, cx);
    let content_w = measure_content_width(file, gutter_w, window, cx);
    let heights = RowHeights::new(window);
    let sizes = Rc::new(row_sizes(file, heights));


    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .child(
            // v_virtual_list builds only the visible slice per frame:
            // row heights are declared up front (see `RowHeights`), so
            // scrolling even a huge diff costs one range render instead
            // of rebuilding every row. Item indices still mirror the
            // `DiffFile::rows` walk (plus the binary/truncation notes),
            // which is what keeps search's `scroll_to_item` landing on
            // the right row.
            v_virtual_list(
                cx.entity(),
                "diff-hunks",
                sizes,
                move |this, range, window, cx| {
                    render_rows(this, file_ix, range, content_w, gutter_w, heights, window, cx)
                },
            )
            .track_scroll(&this.diff_hunks_scroll)
            .size_full()
            .p_2(),
        )
        .scrollbar(&this.diff_hunks_scroll, scroll::ScrollbarAxis::Both)
        .context_menu({
            let path = diff.files[file_ix].path.clone();
            let lines = diff.files[file_ix]
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.kind != '-')
                .map(|l| l.text.clone())
                .collect::<Vec<_>>();
            move |menu, _, _| {
                let path = path.clone();
                let contents = lines.join("\n");
                menu.item(
                    PopupMenuItem::new("Copy File Path")
                        .icon(Icon::new(IconName::Copy))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                        }),
                )
                .item(
                    PopupMenuItem::new("Copy File Contents")
                        .icon(Icon::new(IconName::Copy))
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(contents.clone()));
                        }),
                )
            }
        })
        .into_any_element()
}

/// Longest lines shaped exactly per render (bound on text-system calls).
const MEASURE_CANDIDATES: usize = 16;
/// Chrome left of a diff line's text besides the two number gutters
/// (their width is measured per file — see `gutter_width`): the sign
/// column (14px) and the text block's `pl_2`/`pr_3` padding.
const LINE_CHROME_EXTRAS: f32 = 14. + 8. + 12.;
/// Hunk header horizontal padding (`px_2` on both sides).
const HEADER_CHROME: f32 = 16.;

/// Declared row heights for the virtual list. Line rows are single
/// nowrap lines of `text_sm`, so one height fits all of them (the
/// number gutters carry the row's only extra: `pt(2px)`); hunk header
/// bands add their padding, with extra top spacing between hunks.
/// Computed with the framework's own text-style math so the declared
/// sizes match what the rows lay out at.
#[derive(Clone, Copy)]
struct RowHeights {
    line: Pixels,
    header: Pixels,
    header_spaced: Pixels,
    meta: Pixels,
}

impl RowHeights {
    fn new(window: &Window) -> Self {
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

/// Every row's height, in the pane's item order: per hunk the `@@`
/// header then its lines, in [`DiffFile::rows`] order, with the
/// binary/empty note standing alone and the truncation note appended
/// last. Widths are unused by `v_virtual_list` (it measures one row
/// for the cross axis); the rows carry the definite `content_w`
/// themselves, which is what gives the horizontal scrollbar its range
/// and keeps the +/- tint bands uniform.
fn row_sizes(file: &DiffFile, heights: RowHeights) -> Vec<gpui_kit::Size<Pixels>> {
    if file.hunks.is_empty() {
        return vec![size(px(0.), heights.meta)];
    }
    let extra = if file.truncated { 1 } else { 0 };
    let mut sizes =
        Vec::with_capacity(file.hunks.iter().map(|h| h.lines.len() + 1).sum::<usize>() + extra);
    for (hunk_ix, hunk) in file.hunks.iter().enumerate() {
        sizes.push(size(
            px(0.),
            if hunk_ix == 0 {
                heights.header
            } else {
                heights.header_spaced
            },
        ));
        sizes.extend(std::iter::repeat_n(size(px(0.), heights.line), hunk.lines.len()));
    }
    if file.truncated {
        sizes.push(size(px(0.), heights.meta));
    }
    sizes
}

/// Builds the rows of one visible slice. The walk is the same
/// [`DiffFile::rows`] order `diff::match_rows` numbers, so a row's
/// item index IS its search index; a hunk header is spaced iff it is
/// not the file's first row.
fn render_rows(
    this: &AppView,
    file_ix: usize,
    range: std::ops::Range<usize>,
    content_w: Pixels,
    gutter_w: Pixels,
    heights: RowHeights,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> Vec<AnyElement> {
    let Some(diff) = &this.diff else {
        return Vec::new();
    };
    let Some(file) = diff.files.get(file_ix) else {
        return Vec::new();
    };
    let matches = &this.diff_search.matches;
    let current_row = matches.get(this.diff_search.current).copied();
    if file.hunks.is_empty() {
        return if range.start == 0 {
            vec![row_box(
                meta_row("No text changes to display (binary, empty file, or metadata change).", cx),
                content_w,
                heights.meta,
            )
            .into_any_element()]
        } else {
            Vec::new()
        };
    }
    let mut rows: Vec<AnyElement> = Vec::with_capacity(range.end - range.start);
    for (row_ix, row) in file
        .rows()
        .enumerate()
        .skip(range.start)
        .take(range.end - range.start)
    {
        rows.push(match row {
            DiffRow::Header(header) => row_box(
                hunk_header(header.to_string(), row_ix > 0, cx),
                content_w,
                if row_ix > 0 {
                    heights.header_spaced
                } else {
                    heights.header
                },
            )
            .into_any_element(),
            DiffRow::Line(line) => {
                let hit = matches.binary_search(&row_ix).is_ok();
                row_box(
                    diff_line(row_ix, line, hit, current_row == Some(row_ix), gutter_w, cx),
                    content_w,
                    heights.line,
                )
                .into_any_element()
            }
        });
    }
    if file.truncated {
        let note_ix = file.hunks.iter().map(|h| h.lines.len() + 1).sum::<usize>();
        if range.start <= note_ix && note_ix < range.end {
            let limit = this
                .diff_limits
                .get(&file.path)
                .copied()
                .unwrap_or(crate::diff::MAX_LINES_PER_FILE);
            let note = if file.lines_total >= limit && limit < crate::diff::EXPAND_MAX_LINES {
                // The rendered slice reached the cap note: grow the
                // file's budget and reload (next frame — this closure
                // runs mid-layout, the entity is on the stack).
                let path = file.path.clone();
                cx.on_next_frame(window, move |this, _window, cx| {
                    this.expand_diff_limit(&path, cx);
                });
                "Loading more lines…".to_owned()
            } else {
                format!(
                    "Preview limited to {} lines. Change totals include the entire file.",
                    grouped(limit)
                )
            };
            rows.push(row_box(meta_row(&note, cx), content_w, heights.meta).into_any_element());
        }
    }
    rows
}

/// Every row carries the file's definite width plus the full-width
/// minimum, so row bands stay uniform and the list's measured cross
/// size gives the horizontal scrollbar a real range. The height is
/// the one declared to `v_virtual_list`.
fn row_box<E: Styled>(el: E, w: Pixels, h: Pixels) -> E {
    el.w(w).min_w_full().h(h)
}

/// One-line note row (binary file, truncation cap): clipped to its
/// row box instead of wrapping, the virtual list owns its height.
fn meta_row(text: &str, cx: &App) -> Div {
    super::meta_text(text, cx)
        .whitespace_nowrap()
        .overflow_hidden()
}

fn diff_line(
    id: usize,
    line: &DiffLine,
    hit: bool,
    current: bool,
    gutter_w: Pixels,
    cx: &mut Context<AppView>,
) -> Stateful<Div> {
    let (old, new) = (
        line.old_no.map(|n| n.to_string()).unwrap_or_default(),
        line.new_no.map(|n| n.to_string()).unwrap_or_default(),
    );
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
        .child(gutter(old, gutter_w, cx))
        .child(gutter(new, gutter_w, cx))
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
                .child(
                    SelectableText::new(("diff-text", id), text)
                        .document_order(id as u64)
                        .text_style(TextStyleRefinement {
                            font_family: Some(mono),
                            font_size: Some(rems(0.875).into()),
                            color: Some(cx.theme().foreground.opacity(0.85)),
                            white_space: Some(WhiteSpace::Nowrap),
                            ..Default::default()
                        }),
                ),
        )
}

/// Width the content column needs so the longest line never clips.
/// Candidates are ranked by a display-cell estimate (non-ASCII ~2 cells),
/// then the top few are shaped exactly with the mono font at `text_sm`.
fn measure_content_width(
    file: &DiffFile,
    gutter_w: Pixels,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> Pixels {
    let line_chrome = f32::from(gutter_w) * 2. + LINE_CHROME_EXTRAS;
    let font = Font {
        family: cx.theme().mono_font_family.clone(),
        ..Default::default()
    };
    let size = px(0.75 * f32::from(window.rem_size()));
    let estimate = |s: &str| {
        s.chars()
            .fold(0usize, |n, c| n + if c.is_ascii() { 1 } else { 2 })
    };

    let mut candidates: Vec<(usize, &str, f32)> = Vec::new();
    for hunk in &file.hunks {
        candidates.push((estimate(&hunk.header), hunk.header.as_str(), HEADER_CHROME));
        for line in &hunk.lines {
            candidates.push((estimate(&line.text), line.text.as_str(), line_chrome));
        }
    }
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
fn hunk_header(header: String, spaced: bool, cx: &mut Context<AppView>) -> Div {
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

/// The ⌘F find bar: floats over the pane's top-right so the header and
/// content never shift. Enter/Shift-Enter and Escape arrive as actions
/// dispatched by the input itself; the "DiffSearch" key context puts
/// ⌘G/⌘⇧G (bound in `app`) on the dispatch path while the input holds
/// focus, and `track_focus` keeps the context honest about when that
/// is.
fn find_bar(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let search = &this.diff_search;
    let input = search.input.clone();
    let focus = input.read(cx).focus_handle(cx).clone();
    let total = search.matches.len();
    let has_query = !input.read(cx).value().is_empty();
    let has_matches = total > 0;
    let counter: SharedString = if !has_query {
        "".into()
    } else if !has_matches {
        "No results".into()
    } else {
        format!("{}/{}", search.current.min(total - 1) + 1, total).into()
    };

    h_flex()
        .id("diff-find-bar")
        .occlude()
        .absolute()
        .top_2()
        .right_3()
        .track_focus(&focus)
        .key_context("DiffSearch")
        // The pane root focuses the window fallback on any mouse down;
        // without stopping propagation a click into the input would
        // bounce focus right back out.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
        )
        .on_action(cx.listener(|this, enter: &input::Enter, _, cx| {
            this.diff_search_step(enter.shift, cx);
        }))
        .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
            this.close_diff_search(window, cx);
        }))
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .bg(cx.theme().popover)
        .border_1()
        .border_color(cx.theme().border)
        .shadow_lg()
        .child(
            Input::new(&input)
                .aria_label("Find in diff")
                .small()
                .w(px(180.))
                .appearance(true)
                .focus_bordered(false)
                .cleanable(true),
        )
        .child(
            div()
                .id("diff-find-counter")
                .role(Role::Label)
                .aria_label(counter.clone())
                .min_w(px(52.))
                .text_center()
                .text_xs()
                .text_color(cx.theme().foreground.opacity(if has_matches { 0.55 } else { 0.35 }))
                .child(counter),
        )
        .child(
            Button::new("diff-find-prev")
                .accessibility_label("Previous match")
                .xsmall()
                .ghost()
                .icon(IconName::ChevronLeft)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.diff_search_step(true, cx))),
        )
        .child(
            Button::new("diff-find-next")
                .accessibility_label("Next match")
                .xsmall()
                .ghost()
                .icon(IconName::ChevronRight)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.diff_search_step(false, cx))),
        )
        .child(
            Button::new("diff-find-close")
                .accessibility_label("Close find bar")
                .xsmall()
                .ghost()
                .icon(IconName::Close)
                .on_click(cx.listener(|this, _, window, cx| this.close_diff_search(window, cx))),
        )
}

/// Gutter width for the file's largest line number, measured in the mono
/// font at `text_sm`: the stock 36px (scaled) fits three digits, and a
/// wider number would wrap into the next row (row heights are fixed).
/// Floored at the stock width so small diffs don't shift.
fn gutter_width(file: &DiffFile, window: &mut Window, cx: &mut Context<AppView>) -> Pixels {
    let max_no = file
        .hunks
        .iter()
        .flat_map(|h| h.lines.iter())
        .filter_map(|l| l.old_no.max(l.new_no))
        .max()
        .unwrap_or(0);
    let digits = max_no.to_string().len();
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
    (digits_w + px(8. + 1.)).max(px(scaled(36.)))
}

/// `1234567` → `1,234,567` (the truncation note's line budget).
fn grouped(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn gutter(no: String, w: Pixels, cx: &mut Context<AppView>) -> impl IntoElement {
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

fn empty(text: &str, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .flex_1()
        .flex()
        .items_start()
        .justify_center()
        .p_4()
        .text_sm()
        .text_color(cx.theme().foreground.opacity(0.4))
        // Wrap instead of clipping when the panel is narrow.
        .child(div().max_w_full().child(text.to_string()))
}

#[cfg(test)]
mod tests {
    use gpui_kit::base::{ScrollbarMode, SelectableText, TextSelection, TextSelectionLayer};
    use gpui_kit::component::theme::Theme;
    use gpui_kit::base::v_flex;
    use gpui_kit::{
        Context, IntoElement, Modifiers, MouseButton, ParentElement, Render, Styled,
        TestAppContext, TextStyleRefinement, WhiteSpace, Window, div, gpui, point, px, red,
    };

    /// The diff pane's line text: its own selection participant with
    /// `document_order` in reading order, shaped exactly like the real
    /// rows (mono, 12px, nowrap) so layout and copy mirror the surface.
    fn line_text(order: usize, text: &str) -> impl IntoElement {
        SelectableText::new(("diff-text", order), text)
            .document_order(order as u64)
            .text_style(TextStyleRefinement {
                font_family: Some("Menlo".into()),
                font_size: Some(px(12.).into()),
                color: Some(red()),
                white_space: Some(WhiteSpace::Nowrap),
                ..Default::default()
            })
    }

    struct DiffTextRoot;

    impl Render for DiffTextRoot {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().child(TextSelectionLayer).child(
                v_flex()
                    .child(div().w(px(300.)).h(px(20.)).child(line_text(0, "alpha beta")))
                    .child(div().w(px(300.)).h(px(20.)).child(line_text(1, "gamma delta"))),
            )
        }
    }

    #[test]
    fn drag_selection_spans_lines_and_copy_joins_with_newline() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("diff_selection_selectable_text"));
                let cx = &mut cx0;
                let (_view, vcx) = cx.add_window_view(|_, _| DiffTextRoot);
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });

                // Rows are pinned: line 1 at y∈[0,20), line 2 at
                // y∈[20,40). Drag from line 1 into line 2: both
                // participants project the band (the middle/mid lines
                // come out whole — per-character projection is
                // gpui-base's own contract), and the window selection
                // joins the two lines with a newline in document order.
                vcx.simulate_mouse_down(
                    point(px(2.), px(3.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                vcx.simulate_mouse_move(
                    point(px(150.), px(25.)),
                    Some(MouseButton::Left),
                    Modifiers::none(),
                );
                vcx.simulate_mouse_up(
                    point(px(150.), px(25.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );

                let copied = vcx.update(|window, cx| TextSelection::selected_text(window, cx));
                assert_eq!(copied, "alpha beta\ngamma delta");

                // Clicking elsewhere clears the selection.
                vcx.simulate_mouse_down(
                    point(px(150.), px(200.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                vcx.simulate_mouse_up(
                    point(px(150.), px(200.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                let cleared = vcx.update(|window, cx| TextSelection::selected_text(window, cx));
                assert_eq!(cleared, "");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    #[test]
    fn selection_tracks_cursor_across_frame_loop() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("diff_selection_frame_loop"));
                let cx = &mut cx0;
                let (_view, vcx) = cx.add_window_view(|_, _| DiffTextRoot);
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });

                // A live drag is an interleaved (event → render) stream:
                // each render re-registers the window-level selection
                // listeners, so every move event lands and the selection
                // follows the cursor step by step. (With stale listeners
                // only the first move after a render would be handled,
                // freezing the selection until an unrelated repaint.)
                vcx.simulate_mouse_down(
                    point(px(2.), px(3.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.simulate_mouse_move(
                    point(px(60.), px(3.)),
                    Some(MouseButton::Left),
                    Modifiers::none(),
                );
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.simulate_mouse_move(
                    point(px(60.), px(25.)),
                    Some(MouseButton::Left),
                    Modifiers::none(),
                );
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                vcx.simulate_mouse_up(
                    point(px(60.), px(25.)),
                    MouseButton::Left,
                    Modifiers::none(),
                );

                let copied = vcx.update(|window, cx| TextSelection::selected_text(window, cx));
                assert_eq!(copied, "alpha beta\ngamma delta");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    #[test]
    fn startup_forces_hover_show_mode_on_base_scrollbars() {
        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 =
                    TestAppContext::build(dispatcher, Some("scrollbar_hover_show_mode"));
                let cx = &mut cx0;
                cx.update(|cx| {
                    gpui_kit::init(cx);
                    // Mirrors main.rs startup: theme change, then the
                    // app-level scrollbar-mode override.
                    Theme::change(Theme::global(cx).mode, None, cx);
                    Theme::set_scrollbar_mode(ScrollbarMode::Hover, cx);
                });

                cx.update(|cx| {
                    assert_eq!(Theme::global(cx).scrollbar_mode, ScrollbarMode::Hover);
                    let base = gpui_kit::base::Theme::global(cx);
                    assert_eq!(base.scrollbar.mode(), ScrollbarMode::Hover);
                });
            }),
        );
    }

}
