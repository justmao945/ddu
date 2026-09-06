//! Right pane: the selected file's hunks. Header: branch + change
//! stats. The file tree lives in the sidebar's lower layer
//! ([`super::diff_tree`]); this pane always shows the current
//! selection. Diff lines never truncate — long lines scroll
//! horizontally with a visible scrollbar.

use super::diff_tree::plus_minus;
use super::{PANEL_HEADER_PX, meta_text};
use crate::app::AppView;
use crate::diff::{DiffFile, DiffLine};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

pub(crate) fn render(
    this: &AppView,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    // No branch/stat strip when the tree is clean — the body's
    // "No changes" state carries the panel on its own.
    let dirty = this.diff.as_ref().is_some_and(|d| !d.is_empty());
    v_flex()
        .h_full()
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .bg(cx.theme().background)
        // Clicking anywhere in the panel moves focus off the terminal,
        // so its block cursor turns hollow.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _, window, cx| this.window_focus.focus(window, cx)),
        )
        .when(dirty, |el| el.child(header(this, cx)))
        .child(body(this, window, cx))
}

fn header(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let diff = this.diff.as_ref();
    let branch = diff.and_then(|d| d.branch.clone());
    let (files, added, removed) = diff
        .map(|d| {
            (
                d.files.len(),
                d.files.iter().map(|f| f.added).sum::<usize>(),
                d.files.iter().map(|f| f.removed).sum::<usize>(),
            )
        })
        .unwrap_or((0, 0, 0));

    div()
        .h(px(PANEL_HEADER_PX))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        // Left: branch, then the file count.
        .child(
            div()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_sm()
                .font_medium()
                .text_color(cx.theme().foreground.opacity(0.9))
                .child(branch.unwrap_or_else(|| "Changes".into())),
        )
        .when(files > 0, |el| {
            el.child(meta_text(format!("{files} files changed"), cx))
                // Right-aligned +/- totals.
                .child(div().flex_1())
                .child(plus_minus(added, removed, cx))
        })
        .when(files == 0, |el| el.child(div().flex_1()))
}

fn body(this: &AppView, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    let Some(diff) = &this.diff else {
        return empty(this.diff_error.as_deref().unwrap_or("Loading changes…"), cx)
            .into_any_element();
    };
    if diff.is_empty() {
        return empty("No changes — working tree clean.", cx).into_any_element();
    }

    let file_ix = this.diff_file.min(diff.files.len() - 1);
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .child(
            div()
                .id("diff-hunks")
                .size_full()
                // Cross-axis alignment: default stretch would clamp the
                // content column to the viewport width, so taffy would
                // report content_size == viewport and the horizontal
                // scrollbar would never get a range.
                .items_start()
                .overflow_scroll()
                .track_scroll(&this.diff_hunks_scroll)
                .p_2()
                .child(file_diff(&diff.files[file_ix], window, cx)),
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
/// Chrome left of a diff line's text: two number gutters (36px each),
/// the sign column (14px) and the text block's `pl_2`/`pr_3` padding.
const LINE_CHROME: f32 = 36. + 36. + 14. + 8. + 12.;
/// Hunk header horizontal padding (`px_2` on both sides).
const HEADER_CHROME: f32 = 16.;

fn file_diff(file: &DiffFile, window: &mut Window, cx: &mut Context<AppView>) -> impl IntoElement {
    let content_w = measure_content_width(file, window, cx);
    // Explicit width + min_w_full: the scroll container derives its
    // content size from child layout bounds, and taffy fit-content-clamps
    // auto-width children to the viewport, so only a definite width gives
    // the horizontal scrollbar a range. Rows stretch to it (min_w_full),
    // keeping the +/- tint spanning the whole scrollable width.
    let mut hunks = v_flex().gap_2().w(content_w).min_w_full().flex_shrink_0();
    if file.hunks.is_empty() {
        hunks = hunks.child(super::meta_text(
            "No text changes to display (binary, empty file, or metadata change).",
            cx,
        ));
    }
    let mut line_no = 0usize;
    for hunk in &file.hunks {
        let mut h = v_flex()
            .items_start()
            .child(hunk_header(hunk.header.clone(), cx));
        for line in &hunk.lines {
            let id = line_no;
            line_no += 1;
            h = h.child(diff_line(id, line, cx));
        }
        hunks = hunks.child(h);
    }
    if file.truncated {
        hunks = hunks.child(super::meta_text(
            "Preview limited to 5,000 lines. Change totals include the entire file.",
            cx,
        ));
    }
    hunks
}

/// Width the content column needs so the longest line never clips.
/// Candidates are ranked by a display-cell estimate (non-ASCII ~2 cells),
fn diff_line(id: usize, line: &DiffLine, cx: &mut Context<AppView>) -> impl IntoElement {
    let (old, new) = (
        line.old_no.map(|n| n.to_string()).unwrap_or_default(),
        line.new_no.map(|n| n.to_string()).unwrap_or_default(),
    );
    let tint = match line.kind {
        '+' => Some(cx.theme().green.opacity(0.12)),
        '-' => Some(cx.theme().red.opacity(0.12)),
        _ => None,
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
        .font_family(mono)
        .text_xs()
        .when_some(tint, |el, tint| el.bg(tint))
        // Double-click copies the raw line (no gutter chrome).
        .on_click(move |ev, _, cx| {
            if ev.click_count() == 2 {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
            }
        })
        .child(gutter(old, cx))
        .child(gutter(new, cx))
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
                .whitespace_nowrap()
                .text_color(cx.theme().foreground.opacity(0.85))
                .child(line.text.clone()),
        )
}

/// Width the content column needs so the longest line never clips.
/// Candidates are ranked by a display-cell estimate (non-ASCII ~2 cells),
/// then the top few are shaped exactly with the mono font at `text_xs`.
fn measure_content_width(
    file: &DiffFile,
    window: &mut Window,
    cx: &mut Context<AppView>,
) -> Pixels {
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
            candidates.push((estimate(&line.text), line.text.as_str(), LINE_CHROME));
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
/// rows below it instead of hugging the header text.
fn hunk_header(header: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .min_w_full()
        .mb_1()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .bg(cx.theme().foreground.opacity(0.05))
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(cx.theme().foreground.opacity(0.5))
        .child(header)
}

fn gutter(no: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .w(px(36.))
        .flex_shrink_0()
        .text_right()
        .pr_2()
        .text_xs()
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
