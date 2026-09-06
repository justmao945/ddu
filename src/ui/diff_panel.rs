//! Right pane: working-tree changes. Header: branch + change stats +
//! refresh. Below: the file tree (capped height, indent guide lines)
//! above a divider, then the selected file's diff. Both areas scroll
//! in both axes with visible scrollbars; diff lines never truncate —
//! long lines scroll horizontally.

use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::{PANEL_HEADER_PX, ROW_PX, hover_bg, meta_text, selection_bg};
use crate::app::AppView;
use crate::diff::{DiffFile, DiffLine};

/// Cap on the file-tree height; the tree scrolls beyond it.
const FILE_TREE_MAX_H: f32 = 220.;

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
        .when(dirty, |el| el.child(header(this, cx)))
        .child(body(this, window, cx))
}

/// Panel header: branch + `N files · +A −R` (hidden entirely when clean).
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
    let mono = cx.theme().mono_font_family.clone();

    div()
        .h(px(PANEL_HEADER_PX))
        .flex_shrink_0()
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .when_some(
            Some(branch.unwrap_or_else(|| "Changes".into())),
            |el, branch| {
                el.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_sm()
                        .font_medium()
                        .text_color(cx.theme().foreground.opacity(0.9))
                        .child(branch),
                )
            },
        )
        .when(files > 0, |el| {
            el.child(
                h_flex()
                    .items_center()
                    .flex_shrink_0()
                    .gap_2()
                    .font_family(mono)
                    .text_xs()
                    .child(
                        div()
                            .text_color(cx.theme().green)
                            .child(format!("+{added}")),
                    )
                    .child(
                        div()
                            .text_color(cx.theme().red)
                            .child(format!("−{removed}")),
                    )
                    .child(meta_text(format!("{files} files"), cx)),
            )
        })
        .child(div().flex_1())
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
        .flex()
        .flex_col()
        // Scrollbars are siblings of the scroll area so they stay pinned
        // to its viewport while content moves underneath.
        .child({
            let tree = build_tree(&diff.files);
            let rows = diff.files.len() + tree.dirs.len();
            div()
                .relative()
                .h(px((rows as f32 * (ROW_PX + 2.) + 16.).min(FILE_TREE_MAX_H)))
                .flex_shrink_0()
                .min_w_0()
                .overflow_hidden()
                .border_b_1()
                .border_color(cx.theme().border)
                .child(
                    div()
                        .id("diff-tree-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&this.diff_tree_scroll)
                        .p_2()
                        .child(tree_level(&tree, file_ix, 0, cx)),
                )
                .vertical_scrollbar(&this.diff_tree_scroll)
        })
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .overflow_hidden()
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
                .scrollbar(&this.diff_hunks_scroll, scroll::ScrollbarAxis::Both),
        )
        .into_any_element()
}

/// One level of the file tree. `files` are entries at this depth,
/// `dirs` maps directory names to their nested entries. Owns clones so
/// the built tree outlives the borrow of the source slice.
#[derive(Clone)]
struct TreeNode {
    files: Vec<(usize, DiffFile)>,
    dirs: Vec<(String, Vec<(usize, DiffFile)>)>,
}

/// Group flat file rows into directory tree nodes.
fn build_tree(files: &[DiffFile]) -> TreeNode {
    let mut top: Vec<(usize, DiffFile)> = Vec::new();
    let mut dirs: Vec<(String, Vec<(usize, DiffFile)>)> = Vec::new();
    for (ix, f) in files.iter().enumerate() {
        match f.path.split_once('/') {
            Some((dir, _rest)) => {
                let key = dir.to_string();
                if let Some(slot) = dirs.iter_mut().find(|(d, _)| *d == key) {
                    slot.1.push((ix, f.clone()));
                } else {
                    dirs.push((key, vec![(ix, f.clone())]));
                }
            }
            None => top.push((ix, f.clone())),
        }
    }
    TreeNode { files: top, dirs }
}

/// Render one depth level: files first, then dirs with an indent
/// guide line running down their children.
fn tree_level(
    tree: &TreeNode,
    selected: usize,
    depth: usize,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let radius = cx.theme().radius;
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let guide = cx.theme().foreground.opacity(0.12);
    let indent = 14. * depth as f32;
    let radius_f = f32::from(radius);

    let mut level = v_flex().flex_shrink_0().items_stretch().gap_0p5();

    for (ix, f) in &tree.files {
        level = level.child(file_row(
            *ix,
            f,
            *ix == selected,
            indent,
            radius_f,
            active_bg,
            hov_bg,
            cx,
        ));
    }

    for (dir, children) in &tree.dirs {
        let sub = TreeNode {
            files: children.clone(),
            dirs: vec![],
        };
        let added: usize = children.iter().map(|(_, f)| f.added).sum();
        let removed: usize = children.iter().map(|(_, f)| f.removed).sum();
        level = level
            .child(
                div()
                    .h(px(ROW_PX))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pl(px(indent + 4.))
                    .pr_2()
                    .rounded(radius)
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .with_size(gpui_kit::component::Size::XSmall),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .font_medium()
                            .whitespace_nowrap()
                            .text_color(cx.theme().foreground.opacity(0.9))
                            .child(format!("{dir}/")),
                    )
                    .child(div().flex_1())
                    .child(plus_minus(added, removed, cx)),
            )
            // Nested level: vertical indent guide + deeper indent.
            .child(
                div()
                    .ml(px(indent + 7.))
                    .pl(px(indent + 8.))
                    .border_l_1()
                    .border_color(guide)
                    .child(tree_level(&sub, selected, depth + 1, cx)),
            );
    }

    level
}

#[allow(clippy::too_many_arguments)]
fn file_row(
    ix: usize,
    f: &DiffFile,
    active: bool,
    indent: f32,
    radius: f32,
    active_bg: Hsla,
    hov_bg: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
    // Full width so the `+N −N` stats pin to the panel's right edge
    // (items_stretch on the parent) instead of trailing the filename.
    div()
        .id(("diff-file", ix))
        .w_full()
        .h(px(ROW_PX))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl(px(indent + 4.))
        .pr_2()
        .rounded(px(radius))
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| el.bg(if active { active_bg } else { hov_bg }))
        .on_click(cx.listener(move |this, _, _, cx| {
            if this.diff_file != ix {
                this.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
            }
            this.diff_file = ix;
            cx.notify();
        }))
        .child(Icon::new(IconName::File).with_size(gpui_kit::component::Size::XSmall))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(name),
        )
        .child(plus_minus(f.added, f.removed, cx))
}

fn plus_minus(added: usize, removed: usize, cx: &mut Context<AppView>) -> impl IntoElement {
    let mono = cx.theme().mono_font_family.clone();
    // Tabular figures in a fixed track: +/- counts align vertically
    // across rows, GitHub-style.
    div()
        .flex_shrink_0()
        .flex()
        .justify_end()
        .text_xs()
        .font_family(mono)
        .child(
            div()
                .min_w(px(34.))
                .text_right()
                .text_color(cx.theme().green)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .min_w(px(34.))
                .text_right()
                .text_color(cx.theme().red)
                .child(format!("−{removed}")),
        )
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
    for hunk in &file.hunks {
        let mut h = v_flex()
            .items_start()
            .child(hunk_header(hunk.header.clone(), cx));
        for line in &hunk.lines {
            h = h.child(diff_line(line, cx));
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

fn hunk_header(header: String, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .mb_1()
        .px_2()
        .py_1()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .text_xs()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(cx.theme().foreground.opacity(0.5))
        .child(header)
}

fn diff_line(line: &DiffLine, cx: &mut Context<AppView>) -> impl IntoElement {
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

    div()
        .flex()
        .items_start()
        .min_w_full()
        .font_family(mono)
        .text_xs()
        .when_some(tint, |el, tint| el.bg(tint))
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
