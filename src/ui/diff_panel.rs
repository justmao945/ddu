//! Right pane: the selected file's hunks. Header: file path + change
//! stats. The file tree lives in the sidebar's lower layer
//! ([`super::diff_tree`]), whose summary strip carries the
//! working-tree totals; this pane always shows the current selection.
//! Diff lines never truncate — long lines scroll horizontally with a
//! visible scrollbar.

use std::rc::Rc;

use super::{panel_header_px, scaled};
use super::diff_tree::plus_minus;
use super::panel_view;
use crate::app::AppView;
use crate::diff::view::FileView;
use crate::diff::{
    DiffLine, NoteKind, PaneRow, RowStream, EXPAND_MAX_LINES, MAX_LINES_PER_FILE,
};
use gpui_kit::base::SelectableText;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::text::TextView;
use gpui_kit::component::Sizable as _;
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
    // The strip only makes sense with a selected file: without one the
    // body's empty state carries the panel on its own. A clean file (the
    // listing lists them all) has a strip too — the mode toggles still
    // apply to it.
    let has_file = this.current_diff_path().is_some() && this.diff().is_some();
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
    let path = this.current_diff_path().unwrap_or_default();
    let file = this.selected_diff_file();
    // Ellipsize the *directory*, never the file name: at the pane's
    // default width the strip is at capacity, and ".." where the file
    // should be is worse than a shortened parent. The prefix shrinks
    // and ellipsizes, the name holds its width.
    let (dir, name) = match path.rfind('/') {
        Some(ix) => (&path[..=ix], &path[ix + 1..]),
        None => ("", path),
    };

    div()
        .h(px(panel_header_px()))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_sm()
                .font_medium()
                .whitespace_nowrap()
                .when(!dir.is_empty(), |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().foreground.opacity(0.5))
                            .child(dir.to_string()),
                    )
                })
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .text_ellipsis()
                        .text_color(cx.theme().foreground.opacity(0.9))
                        .child(name.to_string()),
                ),
        )
        // The figures, then the view switch last: the button that changes
        // what the pane shows belongs at the far right, past the file's
        // numbers.
        //
        // No line-count chip: at the pane's default width the strip is
        // already at capacity, and a header that ellipsizes the file's
        // name to print its length has its priorities backwards (File
        // mode's gutter numbers every line anyway).
        .child(match file {
            // An unchanged file has no figures — say so instead of
            // printing `+0 −0`.
            None => div()
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().foreground.opacity(0.45))
                .child("unchanged")
                .into_any_element(),
            Some(f) => plus_minus(f.added, f.removed, cx).into_any_element(),
        })
        .child(mode_button(this, cx))
}

/// The view switch: one icon button, at the far right of the strip. It
/// wears the surface it switches *to* — a document for the whole file, the
/// two-versions glyph for the hunks — so the pane says what pressing it
/// gets you, and `⌘⇧M` drives the same toggle. Labeled text buttons were
/// the wrong trade at this width: "Diff File Preview" cost the file path
/// its room to print its own name.
fn mode_button(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let (icon, label) = match this.view_mode.next() {
        ViewMode::File => (
            IconName::FileText,
            "Show the whole file".to_string(),
        ),
        _ => (IconName::Replace, "Show the diff".to_string()),
    };
    let hint = format!("{label} ({})", crate::app::accel_hint("M"));
    // The Button itself takes no debug selector (its interactivity is
    // applied to an inner element), so the box carries it for tests.
    div()
        .debug_selector(|| "view-mode-toggle-box".into())
        .child(
            Button::new("view-mode-toggle")
                .tab_stop(false)
                .icon(icon)
                .ghost()
                .small()
                .tooltip(hint.clone())
                .accessibility_label(hint)
                .on_click(
                    cx.listener(|this, _: &gpui::ClickEvent, _, cx| this.toggle_view_mode(cx)),
                ),
        )
}

/// What the user picks: the file's hunks, or the file itself. The mode
/// is per session (persisted) and both read the same [`RowStream`]
/// contract, so the find bar, `scroll_to_item` and the drag selection
/// behave identically in either.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum ViewMode {
    /// The file's hunks — the original pane, unchanged.
    #[default]
    Diff,
    /// The whole file with the diff's changes tinted in place.
    File,
}

impl ViewMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::File => "file",
        }
    }

    /// `preview` is the pre-merge spelling of `file`: a `.md` file used
    /// to need a mode of its own, and state written back then still
    /// restores — as the whole-file mode that renders it.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "diff" => Some(Self::Diff),
            "file" | "preview" => Some(Self::File),
            _ => None,
        }
    }

    /// Where the pane's view button (and `⌘⇧M`) goes from here.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Diff => Self::File,
            Self::File => Self::Diff,
        }
    }
}

/// What the pane actually renders for the current selection. Markdown is
/// not a mode the user picks: a `.md` file *is* its rendered document in
/// File mode, and its source is what the toggle would show if it could
/// get at it (an unreadable one falls back to the rows, banded).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Surface {
    Diff,
    File,
    Preview,
}

impl Surface {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Diff => "diff",
            Self::File => "file",
            Self::Preview => "preview",
        }
    }
}

/// Which of the three the pane renders: the mode, and — for File mode —
/// whether the file is a rendered document.
pub(crate) fn surface_of(mode: ViewMode, path: Option<&str>) -> Surface {
    match mode {
        ViewMode::Diff => Surface::Diff,
        ViewMode::File if path.is_some_and(is_markdown) => Surface::Preview,
        ViewMode::File => Surface::File,
    }
}

/// The rendered-document gate.
pub(crate) fn is_markdown(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [".md", ".markdown", ".mdx"]
        .iter()
        .any(|ext| lower.ends_with(ext))
}

const NOTE_READING: &str = "Reading the file…";

/// The mode's row stream for the selected file, plus the reason the
/// requested mode could not be shown (the pane renders the diff's hunks
/// and bands the reason above them). `None` rows mean the pane has
/// nothing of its own to render — the rendered document takes the body
/// instead.
pub(crate) fn pane_rows(this: &AppView) -> (Option<RowStream<'_>>, Option<&'static str>) {
    if this.current_diff_path().is_none() {
        return (None, None);
    }
    let file = this.selected_diff_file();
    match this.surface() {
        // The rendered document's body is not rows — but a source that
        // cannot be read renders rows like File mode does, banding the
        // same reason. `None` rows here mean the Markdown view takes the
        // body; the pane's `body` asks this first.
        Surface::Preview => match (this.cached_preview(), this.preview_refusal()) {
            (Some(_), _) => (None, None),
            (None, refusal) => (
                this.file_rows(file),
                Some(refusal.map_or(NOTE_READING, |r| r.note())),
            ),
        },
        // Diff mode is only reached with a diff to show (a file nobody
        // changed renders as the file itself — see `AppView::surface`),
        // so there are no "no hunks" rows to fall back to here.
        Surface::Diff => match file {
            Some(file) => (Some(RowStream::diff(file)), None),
            None => (this.file_rows(None), None),
        },
        Surface::File => match this.cached_file_view() {
            // An image is drawn by the pane's body, not streamed as
            // rows — and it has no note, because it is not standing in
            // for anything.
            Some(FileView::Image) => (None, None),
            Some(FileView::Text(_)) => (this.file_rows(file), None),
            Some(view) => (
                this.file_rows(file),
                Some(view.refusal().map_or(NOTE_READING, |r| r.note())),
            ),
            None => (this.file_rows(file), Some(NOTE_READING)),
        },
    }
}

fn body(this: &AppView, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    // The pane mirrors the tree: no active session → no diff content.
    if this.current_session().is_none() {
        return empty("No active session — select one in the project tree.", cx)
            .into_any_element();
    }
    let Some(diff) = this.diff() else {
        return empty(this.diff_error.as_deref().unwrap_or("Loading files…"), cx)
            .into_any_element();
    };
    let _ = diff;
    // No selection, no content: the pane stays empty until a tree
    // click picks a file.
    if this.current_diff_path().is_none() {
        return empty("Select a file in the tree.", cx).into_any_element();
    }
    // An image file is drawn as it is: File mode (or Diff mode on a
    // file nobody changed, which the surface rule folds into it).
    if this.surface() == Surface::File
        && matches!(this.cached_file_view(), Some(FileView::Image))
    {
        return image_body(this, cx).into_any_element();
    }
    // The rendered document replaces the rows only when its source is
    // actually there: a document that could not be read falls through to
    // `pane_rows`, which shows the file's rows and bands the reason.
    if this.surface() == Surface::Preview && this.cached_preview().is_some() {
        return preview_body(this).into_any_element();
    }
    let selected_path = this.current_diff_path().unwrap_or_default().to_owned();
    let (stream, note) = pane_rows(this);
    let Some(stream) = stream else {
        // No rows at all: the note (an unchanged file in Diff mode) or
        // the generic empty state carries the pane.
        return empty(note.unwrap_or("Select a file in the tree."), cx).into_any_element();
    };
    let gutter_w = gutter_width(&stream, window, cx);
    let content_w = measure_content_width(&stream, gutter_w, window, cx);
    let heights = RowHeights::new(window);
    let sizes = Rc::new(row_sizes(&stream, heights));
    let surface = this.surface();

    v_flex()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .when_some(note, |el, note| el.child(notice_band(note, cx)))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .child(
                    // v_virtual_list builds only the visible slice per
                    // frame: row heights are declared up front (see
                    // `RowHeights`), so scrolling even a 200k-line file
                    // costs one range render instead of rebuilding every
                    // row. Item indices mirror the mode's `RowStream`
                    // walk (plus the note row), which is what keeps
                    // search's `scroll_to_item` landing on the right row
                    // in every mode.
                    v_virtual_list(
                        cx.entity(),
                        SharedString::from(format!("diff-rows-{}", surface.as_str())),
                        sizes,
                        move |this, range, window, cx| {
                            render_rows(
                                this, range, content_w, gutter_w, heights, window, cx,
                            )
                        },
                    )
                    .track_scroll(&this.diff_hunks_scroll)
                    .size_full()
                    .p_2(),
                )
                .scrollbar(&this.diff_hunks_scroll, scroll::ScrollbarAxis::Both)
                .context_menu({
                    let path = selected_path.clone();
                    let contents = file_text(this);
                    move |menu, _, _| {
                        let path = path.clone();
                        let contents = contents.clone();
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
                }),
        )
        .into_any_element()
}

/// The one-line band above the rows: why the pane is showing something
/// other than what the mode asked for (binary, oversized, deleted,
/// still reading).
fn notice_band(text: &str, cx: &Context<AppView>) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .px_3()
        .py_1()
        .bg(cx.theme().foreground.opacity(0.05))
        .text_xs()
        .text_color(cx.theme().foreground.opacity(0.5))
        .child(text.to_string())
}

/// The rendered document's body: the file's Markdown source, rendered by
/// gpui-component's own text view (its parser handles the GFM set —
/// tables, task lists, strikethrough). Kept to the same size cap as
/// File mode, so a runaway document cannot stall a frame.
fn preview_body(this: &AppView) -> impl IntoElement {
    let text = this.cached_preview().map(|text| text.to_owned()).unwrap_or_default();
    // Images in the document resolve against the *document's* directory.
    let base = this.preview_base();
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        // Prose needs margins: the rows carry their own `p_2`, and a
        // document rendered flush against the pane's edges reads as
        // clipped text rather than a page.
        .p_3()
        .child(
            TextView::markdown(
                SharedString::from(format!(
                    "md-preview-{}",
                    this.current_diff_path().unwrap_or_default()
                )),
                text,
            )
            .selectable(true)
            .scrollable(true)
            .plugin(crate::ui::markdown::LocalImages::new(base)),
        )
        .into_any_element()
}

/// An image file, fitted to the pane: `Contain`, so it is never
/// distorted, centred in whatever space is left over.
fn image_body(this: &AppView, _cx: &mut Context<AppView>) -> impl IntoElement {
    let path = this
        .current_diff_path()
        .map(|path| this.diff_root().join(path))
        .unwrap_or_default();
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        .child(img(path).max_w_full().max_h_full().object_fit(ObjectFit::Contain))
        .into_any_element()
}

/// Builds the rows of one visible slice. The walk is the same
/// [`DiffFile::rows`] order `diff::match_rows` numbers, so a row's
/// item index IS its search index; a hunk header is spaced iff it is
/// not the file's first row.
/// Declared row heights for the virtual list. Line rows are single
/// nowrap lines of `text_sm`, so one height fits all of them (the number
/// gutters carry the row's only extra: `pt(2px)`); hunk header bands add
/// their padding, with extra top spacing between hunks; a note row is a
/// bare line. Computed with the framework's own text-style math so the
/// declared sizes match what the rows lay out at.
#[derive(Clone, Copy, PartialEq)]
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

/// Height of the row at `ix`, in the stream's item order.
fn row_height(stream: &RowStream<'_>, ix: usize, heights: RowHeights) -> Pixels {
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
fn row_sizes(stream: &RowStream<'_>, heights: RowHeights) -> Vec<gpui_kit::Size<Pixels>> {
    (0..stream.len())
        .map(|ix| size(px(0.), row_height(stream, ix, heights)))
        .collect()
}

/// The diff-line budget in force for the selected file.
fn line_budget(this: &AppView) -> usize {
    this.current_diff_path()
        .and_then(|path| this.diff_limits.get(path).copied())
        .unwrap_or(MAX_LINES_PER_FILE)
}

/// Whether the note row's slice should grow the file's budget on sight.
fn note_expandable(this: &AppView, stream: &RowStream<'_>) -> bool {
    matches!(
        stream.note(),
        Some(NoteKind::DiffCapped | NoteKind::TintsCapped)
    ) && line_budget(this) < EXPAND_MAX_LINES
}

/// The note row's text: the stream says which note it appended, the pane
/// says what it reads like (and how far the budget has grown).
fn note_text(this: &AppView, stream: &RowStream<'_>) -> String {
    match stream.note() {
        None => String::new(),
        Some(NoteKind::NoTextChanges) => {
            "No text changes to display (binary, empty file, or metadata change).".to_owned()
        }
        Some(NoteKind::EmptyFile) => "Empty file.".to_owned(),
        Some(note @ (NoteKind::DiffCapped | NoteKind::TintsCapped)) => {
            let limit = line_budget(this);
            if limit < EXPAND_MAX_LINES {
                return if note == NoteKind::TintsCapped {
                    "Loading the file's changes…".to_owned()
                } else {
                    "Loading more lines…".to_owned()
                };
            }
            if note == NoteKind::TintsCapped {
                format!(
                    "Change tints stop at {} lines. Change totals include the entire file.",
                    grouped(limit)
                )
            } else {
                format!(
                    "Preview limited to {} lines. Change totals include the entire file.",
                    grouped(limit)
                )
            }
        }
    }
}

/// The selected file's text for "Copy File Contents": the real file when
/// File mode has read it, otherwise the diff's own reconstruction (every
/// line the diff did not remove).
fn file_text(this: &AppView) -> String {
    if let Some(FileView::Text(view)) = this.cached_file_view() {
        return view
            .rows
            .iter()
            .filter_map(|row| match row {
                crate::diff::view::ViewRow::Line(line) if line.kind != '-' => {
                    Some(line.text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    // Changed but not yet read (or unreadable): reconstruct from the
    // diff's own lines. A clean file has no diff to fall back to.
    this.selected_diff_file()
        .map(|file| {
            file.hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.kind != '-')
                .map(|l| l.text.clone())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// Builds the rows of one visible slice. A row's item index IS its
/// search index, whichever surface produced the stream, so the find
/// bar's `scroll_to_item` and the highlight set need no mode-specific
/// logic.
fn render_rows(
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
                row_box(
                    diff_line(ix, line, hit, current_row == Some(ix), gutter_w, cx),
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

/// Longest lines shaped exactly per render (bound on text-system calls).
const MEASURE_CANDIDATES: usize = 16;
/// Chrome left of a line's text besides the two number gutters (their
/// width is measured per stream — see `gutter_width`): the sign column
/// (14px) and the text block's `pl_2`/`pr_3` padding.
const LINE_CHROME_EXTRAS: f32 = 14. + 8. + 12.;
/// Hunk header horizontal padding (`px_2` on both sides).
const HEADER_CHROME: f32 = 16.;

/// Width the content column needs so the longest line never clips.
/// Candidates come from the stream — a whole-file view hands back the
/// handful it ranked when it was built, so a 200k-line file costs the
/// same here as a small one — are ordered by a display-cell estimate
/// (non-ASCII ~2 cells) and the top few are shaped exactly with the mono
/// font at `text_sm`.
fn measure_content_width(
    stream: &RowStream<'_>,
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

/// Gutter width for the stream's largest line number, measured in the
/// mono font at `text_sm`: the stock 36px (scaled) fits three digits, and
/// a wider number would wrap into the next row (row heights are fixed).
/// Floored at the stock width so small diffs don't shift.
fn gutter_width(stream: &RowStream<'_>, window: &mut Window, cx: &mut Context<AppView>) -> Pixels {
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

    /// The mode is what the user picks, the surface is what it renders:
    /// Markdown is not a third mode, it is what File mode *is* for a `.md`
    /// file.
    #[test]
    fn file_mode_renders_markdown_and_preview_parses_as_file() {
        use super::{Surface, ViewMode, surface_of};
        assert_eq!(surface_of(ViewMode::Diff, Some("README.md")), Surface::Diff);
        assert_eq!(
            surface_of(ViewMode::File, Some("README.md")),
            Surface::Preview
        );
        assert_eq!(
            surface_of(ViewMode::File, Some("docs/Notes.MDX")),
            Surface::Preview
        );
        assert_eq!(surface_of(ViewMode::File, Some("src/main.rs")), Surface::File);
        assert_eq!(surface_of(ViewMode::File, None), Surface::File);
        // No selection, or a state file written before the merge: both
        // land on a surface that can render something.
        assert_eq!(surface_of(ViewMode::Diff, None), Surface::Diff);
        assert_eq!(ViewMode::parse("preview"), Some(ViewMode::File));
        assert_eq!(ViewMode::parse("file"), Some(ViewMode::File));
        assert_eq!(ViewMode::parse("nonsense"), None);
    }

    /// The mode switch is one toggle, wherever it is driven from: the
    /// header's far-right icon button and `⌘⇧M` call the same method, and
    /// a Markdown file renders as its document in File mode.
    #[test]
    fn the_view_toggle_switches_surface_on_a_markdown_file() {
        use crate::app::{AppView, ToggleViewMode};
        use crate::config::{Config, ShellConfig, State};
        use crate::diff::{DiffFile, GitDiff};
        use gpui::Entity;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("view_toggle"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    // A silent shell: the test drives the render tree, and
                    // a prompt arriving on the PTY thread mid-frame trips
                    // the test scheduler.
                    cx.set_global(Config {
                        shell: ShellConfig {
                            program: "/bin/cat".into(),
                        },
                        ..Default::default()
                    });
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, mut vcx): (Entity<AppView>, _) =
                    cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|window, cx| {
                    view.update(cx, |v, cx| {
                        let files = vec![DiffFile {
                            path: "README.md".into(),
                            added: 1,
                            removed: 0,
                            ..Default::default()
                        }];
                        v.show_diff = true;
                        v.selection = Some(crate::app::Selection {
                            path: files[0].path.clone(),
                            changed: Some(0),
                        });
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                            
                        });
                        cx.notify();
                    });
                    let _ = window.draw(cx);
                });
                let mode =
                    |vcx: &mut gpui::VisualTestContext| vcx.update(|_, cx| view.read(cx).view_mode);
                let surface =
                    |vcx: &mut gpui::VisualTestContext| vcx.update(|_, cx| view.read(cx).surface());
                assert_eq!(mode(&mut vcx), super::ViewMode::Diff);
                assert_eq!(surface(&mut vcx), super::Surface::Diff);
                vcx.dispatch_action(ToggleViewMode);
                assert_eq!(
                    mode(&mut vcx),
                    super::ViewMode::File,
                    "⌘⇧M switches to the whole file"
                );
                assert_eq!(
                    surface(&mut vcx),
                    super::Surface::Preview,
                    "and a Markdown file renders there"
                );
                vcx.dispatch_action(ToggleViewMode);
                assert_eq!(
                    mode(&mut vcx),
                    super::ViewMode::Diff,
                    "and back to the hunks"
                );
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

    /// Preview's decision table, which is what the pane renders by: the
    /// Markdown view when the source is there, the file's own rows with
    /// the reason when it is not, and the reading note while the read is
    /// still in flight. A refused preview must not be an empty pane.
    #[test]
    fn preview_falls_back_to_rows_and_bands_the_reason() {
        use crate::app::AppView;
        use crate::config::{Config, State};
        use crate::diff::view::{PreviewBuild, Unreadable};
        use crate::diff::{DiffFile, DiffHunk, DiffLine, GitDiff};
        use gpui::Entity;

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("preview_refusal"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_global(Config::default());
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    cx.set_global(State::default());
                });
                let (view, vcx): (Entity<AppView>, _) =
                    cx.add_window_view(|window, cx| AppView::new(window, cx));
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        let files = vec![DiffFile {
                                path: "notes.md".into(),
                                added: 1,
                                removed: 0,
                                hunks: vec![DiffHunk {
                                    header: "@@ -0,0 +1 @@".into(),
                                    lines: vec![DiffLine {
                                        kind: '+',
                                        old_no: None,
                                        new_no: Some(1),
                                        text: "# Title".into(),
                                    }],
                                }],
                            lines_total: 1,
                            truncated: false,
                        }];
                        v.selection = Some(crate::app::Selection {
                            path: files[0].path.clone(),
                            changed: Some(0),
                        });
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                            
                        });
                        // The file is Markdown: File mode renders it.
                        v.view_mode = super::ViewMode::File;
                        cx.notify();
                    });
                });
                let key = ("notes.md".to_owned(), 0);

                // Still reading: the diff's rows, and the pane says why.
                let (stream, note) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (rows, note) = super::pane_rows(v);
                    (rows.map(|s| s.rows()), note)
                });
                // The fallback is the diff's own rows: one hunk header
                // plus its one line.
                assert_eq!(stream, Some(2));
                assert_eq!(note, Some(super::NOTE_READING));

                // Refused: same rows, the refusal's own note.
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.preview = Some(PreviewBuild {
                            key: (key.0.clone(), v.diff_gen),
                            source: Err(Unreadable::TooLarge),
                        });
                        cx.notify();
                    });
                });
                let (stream, note) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (rows, note) = super::pane_rows(v);
                    (rows.map(|s| s.rows()), note)
                });
                assert_eq!(stream, Some(2));
                assert_eq!(note, Some(Unreadable::TooLarge.note()));

                // Read: the body hands the file to the Markdown view, so
                // the pane has no rows of its own to render.
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.preview = Some(PreviewBuild {
                            key: (key.0.clone(), v.diff_gen),
                            source: Ok("# Title".into()),
                        });
                        cx.notify();
                    });
                });
                let (stream, note, read) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (rows, note) = super::pane_rows(v);
                    (rows.map(|s| s.rows()), note, v.cached_preview().is_some())
                });
                assert_eq!((stream, note), (None, None));
                assert!(read);

                // A clean file: the tree lists it, so it is selectable —
                // and Diff mode shows the file itself, not an empty hunks
                // pane (the surface folds a file with no diff into File
                // mode; here the file's own view is still being read).
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.view_mode = super::ViewMode::Diff;
                        v.selection = Some(crate::app::Selection {
                            path: "clean.md".to_owned(),
                            changed: None,
                        });
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files: Vec::new(),
                            },
                                                    });
                        cx.notify();
                    });
                });
                let (surface, note) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let (_, note) = super::pane_rows(v);
                    (v.surface(), note)
                });
                assert_eq!(surface, super::Surface::Preview, "a clean .md document");
                assert_eq!(note, Some(super::NOTE_READING));

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }

}
