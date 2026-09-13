//! The diff file tree: the sidebar's lower layer. Grouped by directory
//! with rolled-up +/− stats, guide-line indentation, and a per-file
//! context menu. Selecting a file drives the right pane's hunks view.

use super::{diff_file_icon, hover_bg, meta_text, panel_header_px, row_px, scaled, selection_bg};
use gpui_kit::component::menu::{PopupMenuItem, *};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppView;
use crate::diff::DiffFile;

/// Layer height bounds and default for the sidebar's vertical splitter
/// (base sizes at factor 1.0 — see `scaled`).
pub(crate) fn tree_default_h() -> f32 {
    scaled(220.)
}
pub(crate) fn tree_min_h() -> f32 {
    scaled(80.)
}
pub(crate) fn tree_max_h() -> f32 {
    scaled(480.)
}

/// Indent added per nesting level, base at factor 1.0 (the guide
/// wrapper's left margin).
fn level_indent() -> f32 {
    scaled(14.)
}

/// The sidebar's lower layer: the working tree's changed files.
pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // The tree answers "this session's changes": with no active
    // session there is nothing to show, even if the last poll left
    // a stale project diff behind.
    if this.current_session().is_none() {
        return empty_layer("No active session — select one in the project tree.", cx)
            .into_any_element();
    }
    let Some(diff) = &this.diff else {
        // The poll error, or the initial "loading" note.
        let note = this.diff_error.as_deref().unwrap_or("Loading changes…");
        return empty_layer(note, cx).into_any_element();
    };
    if diff.is_empty() {
        return empty_layer("No changes — working tree clean.", cx).into_any_element();
    }

    let selected = this.diff_file.filter(|ix| *ix < diff.files.len());
    let tree = build_tree(&diff.files);
    let (added, removed) = tree_stats(&tree);
    v_flex()
        .size_full()
        .min_w_0()
        .overflow_hidden()
        // Summary strip: what the working tree changes in total.
        .child(
            div()
                .h(px(panel_header_px()))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .pl(px(4.))
                .pr_2()
                .child(meta_text(format!("{} files changed", diff.files.len()), cx))
                .child(div().flex_1())
                .child(plus_minus(added, removed, cx)),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    div()
                        .id("diff-tree-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&this.diff_tree_scroll)
                        // No horizontal padding: hover and selection
                        // bands run edge-to-edge — window border to
                        // the divider.
                        .pt_2()
                        .pb_2()
                        .child(tree_rows(&diff.files, &this.diff_tree_closed, &tree, selected, cx)),
                )
                .vertical_scrollbar(&this.diff_tree_scroll),
        )
        .into_any_element()
}

/// Short note shown when there is nothing to list. The text wraps
/// and stays inside the layer however narrow the sidebar gets.
fn empty_layer(text: &str, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .size_full()
        .overflow_hidden()
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        .child(meta_text(text.to_string(), cx).w_full().text_center())
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

/// One rendered row of the flattened tree. `depth` drives the inner
/// indent only — rows themselves stay full-width so hover/selection
/// bands run edge-to-edge (window border to divider).
enum TreeRow {
    File {
        ix: usize,
        depth: usize,
    },
    Dir {
        path: String,
        depth: usize,
        open: bool,
        added: usize,
        removed: usize,
    },
}

/// Depth-first flatten of the visible tree: files before subdirs,
/// insertion order kept; collapsed subtrees drop out entirely.
fn flatten(
    tree: &TreeNode,
    depth: usize,
    closed: &std::collections::HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for (ix, _) in &tree.files {
        out.push(TreeRow::File { ix: *ix, depth });
    }
    for (path, sub) in &tree.dirs {
        let (added, removed) = tree_stats(sub);
        let open = !closed.contains(path);
        out.push(TreeRow::Dir {
            path: path.clone(),
            depth,
            open,
            added,
            removed,
        });
        if open {
            flatten(sub, depth + 1, closed, out);
        }
    }
}

/// Indent guide stripes: one vertical line per ancestor level, right
/// where the nested guide borders used to sit. Per-row segments join
/// into continuous lines across a subtree's rows (no vertical gap
/// between rows).
fn guides(depth: usize, color: Hsla) -> Vec<Div> {
    (1..=depth)
        .map(|lvl| {
            div()
                .absolute()
                .left(px(level_indent() * lvl as f32))
                .top_0()
                .bottom_0()
                .w(px(1.))
                .bg(color)
        })
        .collect()
}

/// The flattened, full-width row list inside the scroll area.
fn tree_rows(
    files: &[DiffFile],
    closed: &std::collections::HashSet<String>,
    tree: &TreeNode,
    selected: Option<usize>,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let guide = cx.theme().foreground.opacity(0.12);

    let mut rows = Vec::new();
    flatten(tree, 0, closed, &mut rows);

    v_flex().flex_shrink_0().children(rows.into_iter().map(|row| {
        match row {
            TreeRow::File { ix, depth } => file_row(
                ix,
                &files[ix],
                Some(ix) == selected,
                depth,
                active_bg,
                hov_bg,
                guide,
                cx,
            )
            .into_any_element(),
            TreeRow::Dir {
                path,
                depth,
                open,
                added,
                removed,
            } => dir_row(path, depth, open, added, removed, hov_bg, guide, cx).into_any_element(),
        }
    }))
}

fn dir_row(
    path: String,
    depth: usize,
    open: bool,
    added: usize,
    removed: usize,
    hov_bg: Hsla,
    guide: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    let toggle = path.clone();
    div()
        .id(SharedString::from(format!("diff-dir-{path}")))
        .role(Role::TreeItem)
        .aria_expanded(open)
        .aria_label(SharedString::from(format!(
            "{path}/ +{added} −{removed}"
        )))
        .relative()
        .w_full()
        .h(px(row_px()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1()
        // Indent is inner padding: the row (and its hover band) spans
        // the full layer width at every depth.
        .pl(px(4. + level_indent() * depth as f32))
        .pr_2()
        .cursor_pointer()
        .hover(move |el| el.bg(hov_bg))
        .on_click(cx.listener(move |this, _, _, cx| {
            // All-open default: presence in the set = collapsed.
            if !this.diff_tree_closed.remove(&toggle) {
                this.diff_tree_closed.insert(toggle.clone());
            }
            cx.notify();
        }))
        .children(guides(depth, guide))
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
                .flex_1()
                .min_w_0()
                .text_sm()
                .font_medium()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_color(cx.theme().foreground.opacity(0.9))
                .child(format!("{name}/")),
        )
        .child(plus_minus(added, removed, cx))
}

fn file_row(
    ix: usize,
    f: &DiffFile,
    active: bool,
    depth: usize,
    active_bg: Hsla,
    hov_bg: Hsla,
    guide: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
    let name_copy = name.clone();
    let path_copy = f.path.clone();
    div()
        .id(("diff-file", ix))
        .role(Role::TreeItem)
        .aria_selected(active)
        .aria_label(SharedString::from(format!(
            "{} +{} −{}",
            f.path, f.added, f.removed
        )))
        .relative()
        .w_full()
        .h(px(row_px()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl(px(4. + level_indent() * depth as f32))
        .pr_2()
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| el.bg(if active { active_bg } else { hov_bg }))
        .on_click(cx.listener(move |this, _, window, cx| {
            // Selecting a file IS opening the right pane: a closed
            // pane springs open on the first click.
            let changed = this.diff_file != Some(ix);
            if changed {
                this.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
            }
            this.diff_file = Some(ix);
            // An open find bar re-anchors to the newly shown file
            // (the query persists across files).
            this.refresh_diff_search(cx);
            if !this.show_diff {
                this.set_diff(true, window, cx);
            } else {
                cx.notify();
            }
        }))
        .children(guides(depth, guide))
        .child(
            diff_file_icon(&f.path)
                .with_size(gpui_kit::component::Size::XSmall)
                .text_color(cx.theme().foreground.opacity(0.6)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
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

/// Right-aligned tabular `+N −N` figures in a fixed track: the counts
/// align vertically across rows, GitHub-style.
pub(crate) fn plus_minus(
    added: usize,
    removed: usize,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let mono = cx.theme().mono_font_family.clone();
    div()
        .flex_shrink_0()
        .flex()
        .justify_end()
        .text_sm()
        .font_family(mono)
        .child(
            div()
                .min_w(px(scaled(34.)))
                .text_right()
                .text_color(cx.theme().green)
                .child(format!("+{added}")),
        )
        .child(
            div()
                .min_w(px(scaled(34.)))
                .text_right()
                .text_color(cx.theme().red)
                .child(format!("−{removed}")),
        )
}
