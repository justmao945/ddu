//! Right pane: working-tree changes. Header: branch + change stats +
//! refresh. Below: the file tree (capped height, indent guide lines)
//! above a divider, then the selected file's diff. Both areas scroll
//! in both axes with visible scrollbars; diff lines never truncate —
//! long lines scroll horizontally.

use super::{PANEL_HEADER_PX, ROW_PX, diff_file_icon, hover_bg, meta_text, selection_bg};
use crate::app::AppView;
use crate::diff::{DiffFile, DiffLine};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Tree pane height bounds and default for the vertical splitter.
pub(crate) const FILE_TREE_DEFAULT_H: f32 = 220.;
pub(crate) const FILE_TREE_MIN_H: f32 = 80.;
pub(crate) const FILE_TREE_MAX_H: f32 = 480.;

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

    let tree = build_tree(&diff.files);
    div()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .child(
            v_resizable("diff-split")
                .with_state(&this.diff_split_state)
                // Tree pane: draggable height between the header and the content.
                .child(
                    resizable_panel()
                        // Persisted height wins on the first render of a
                        // session; the panel keeps whatever the user
                        // drags afterwards, and the live value is
                        // persisted by the AppView resize subscription.
                        .size(
                            this.diff_tree_height_seed
                                .unwrap_or(px(FILE_TREE_DEFAULT_H)),
                        )
                        .size_range(px(FILE_TREE_MIN_H)..px(FILE_TREE_MAX_H))
                        .child(
                            div()
                                .relative()
                                .size_full()
                                .min_w_0()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .id("diff-tree-scroll")
                                        .size_full()
                                        .overflow_y_scroll()
                                        .track_scroll(&this.diff_tree_scroll)
                                        .p_2()
                                        .child(tree_level(
                                            &tree,
                                            file_ix,
                                            0,
                                            &this.diff_tree_closed,
                                            cx,
                                        )),
                                )
                                .vertical_scrollbar(&this.diff_tree_scroll),
                        ),
                )
                // Content pane: whatever height the splitter leaves.
                .child(
                    resizable_panel().size_range(px(120.)..px(f32::MAX)).child(
                        div()
                            .relative()
                            .size_full()
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
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    path.clone(),
                                                ));
                                            }),
                                    )
                                    .item(
                                        PopupMenuItem::new("Copy File Contents")
                                            .icon(Icon::new(IconName::Copy))
                                            .on_click(move |_, _, cx| {
                                                cx.write_to_clipboard(ClipboardItem::new_string(
                                                    contents.clone(),
                                                ));
                                            }),
                                    )
                                }
                            }),
                    ),
                ),
        )
        .into_any_element()
}

/// One level of the file tree. `files` are entries at this depth,
/// `dirs` maps full directory paths to their nested subtrees (insertion
/// order preserved). Owns clones so the built tree outlives the borrow
/// of the source slice.
#[derive(Clone, Default)]
struct TreeNode {
    files: Vec<(usize, DiffFile)>,
    dirs: Vec<(String, TreeNode)>,
}

/// Group flat file rows into a nested directory tree keyed by full dir
/// path (unique expansion keys, any nesting depth).
fn build_tree(files: &[DiffFile]) -> TreeNode {
    let mut root = TreeNode::default();
    for (ix, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split('/').collect();
        let mut node = &mut root;
        for (depth, _) in parts.iter().enumerate().take(parts.len() - 1) {
            let full = parts[..=depth].join("/");
            let pos = match node.dirs.iter().position(|(p, _)| *p == full) {
                Some(p) => p,
                None => {
                    node.dirs.push((full, TreeNode::default()));
                    node.dirs.len() - 1
                }
            };
            node = &mut node.dirs[pos].1;
        }
        node.files.push((ix, f.clone()));
    }
    root
}

/// Recursive +/− totals for a subtree (dir rows show rolled-up stats).
fn tree_stats(node: &TreeNode) -> (usize, usize) {
    let mut stats = (
        node.files.iter().map(|(_, f)| f.added).sum::<usize>(),
        node.files.iter().map(|(_, f)| f.removed).sum::<usize>(),
    );
    for (_, sub) in &node.dirs {
        let (a, r) = tree_stats(sub);
        stats.0 += a;
        stats.1 += r;
    }
    stats
}
fn tree_level<'a>(
    tree: &'a TreeNode,
    selected: usize,
    depth: usize,
    closed: &'a std::collections::HashSet<String>,
    cx: &mut Context<AppView>,
) -> impl IntoElement + use<'a> {
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

    for (path, sub) in &tree.dirs {
        let open = !closed.contains(path);
        let name = path.rsplit('/').next().unwrap_or(path);
        let (added, removed) = tree_stats(sub);
        let toggle = path.clone();
        level = level.child(
            div()
                .id(SharedString::from(format!("diff-dir-{path}")))
                .h(px(ROW_PX))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .pl(px(indent + 4.))
                .pr_2()
                .rounded(radius)
                .cursor_pointer()
                .hover(move |el| el.bg(hov_bg))
                .on_click(cx.listener(move |this, _, _, cx| {
                    // All-open default: presence in the set = collapsed.
                    if !this.diff_tree_closed.remove(&toggle) {
                        this.diff_tree_closed.insert(toggle.clone());
                    }
                    cx.notify();
                }))
                .child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .with_size(gpui_kit::component::Size::XSmall),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .font_medium()
                        .whitespace_nowrap()
                        .text_color(cx.theme().foreground.opacity(0.9))
                        .child(format!("{name}/")),
                )
                .child(div().flex_1())
                .child(plus_minus(added, removed, cx)),
        );
        if open {
            level = level.child(
                // Nested level: vertical indent guide + deeper indent.
                div()
                    .ml(px(indent + 7.))
                    .pl(px(indent + 8.))
                    .border_l_1()
                    .border_color(guide)
                    .child(tree_level(sub, selected, depth + 1, closed, cx)),
            );
        }
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
    let name_copy = name.clone();
    let path_copy = f.path.clone();
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
        .child(
            diff_file_icon(&f.path)
                .with_size(gpui_kit::component::Size::XSmall)
                .text_color(cx.theme().foreground.opacity(0.6)),
        )
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
        .context_menu(move |menu, _, _| {
            let name_copy = name_copy.clone();
            let path_copy = path_copy.clone();
            menu.item(
                PopupMenuItem::new("Copy File Name")
                    .icon(Icon::new(IconName::Copy))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(name_copy.clone()));
                    }),
            )
            .item(
                PopupMenuItem::new("Copy Path")
                    .icon(Icon::new(IconName::Copy))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(path_copy.clone()));
                    }),
            )
        })
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
