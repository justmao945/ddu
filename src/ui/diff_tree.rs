//! The diff file tree: the sidebar's lower layer. Grouped by directory
//! with rolled-up +/− stats, guide-line indentation, and a per-file
//! context menu. Selecting a file drives the right pane's hunks view.

use super::{PANEL_HEADER_PX, ROW_PX, diff_file_icon, hover_bg, meta_text, selection_bg};
use gpui_kit::component::menu::{PopupMenuItem, *};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppView;
use crate::diff::DiffFile;

/// Layer height bounds and default for the sidebar's vertical splitter.
pub(crate) const TREE_DEFAULT_H: f32 = 220.;
pub(crate) const TREE_MIN_H: f32 = 80.;
pub(crate) const TREE_MAX_H: f32 = 480.;

/// Indent added per nesting level (the guide wrapper's left margin).
const LEVEL_INDENT: f32 = 14.;

/// The sidebar's lower layer: the working tree's changed files.
pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let Some(diff) = &this.diff else {
        // No session under the tree → say so; otherwise show the poll
        // error (or the initial "loading" note).
        let note = match this.current_session() {
            None => "No session — pick one in the project tree.",
            _ => this.diff_error.as_deref().unwrap_or("Loading changes…"),
        };
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
                .h(px(PANEL_HEADER_PX))
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
                        .child(tree_level(&tree, selected, 0, &this.diff_tree_closed, cx)),
                )
                .vertical_scrollbar(&this.diff_tree_scroll),
        )
        .into_any_element()
}

/// Short centered note shown when there is nothing to list.
fn empty_layer(text: &str, cx: &mut Context<AppView>) -> impl IntoElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .p_2()
        // Constrain the block so longer notes wrap into a few centered
        // lines instead of one clipped strip.
        .child(meta_text(text.to_string(), cx).max_w(px(160.)))
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
    selected: Option<usize>,
    depth: usize,
    closed: &'a std::collections::HashSet<String>,
    cx: &mut Context<AppView>,
) -> impl IntoElement + use<'a> {
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let guide = cx.theme().foreground.opacity(0.12);

    // Indent comes solely from the nested guide wrappers (+14px per
    // level, border-left as the guide line); rows pad a constant 4px,
    // so depth never compounds.
    let mut level = v_flex().flex_shrink_0().gap_0p5();

    for (ix, f) in &tree.files {
        level = level.child(file_row(
            *ix,
            f,
            Some(*ix) == selected,
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
                .w_full()
                .h(px(ROW_PX))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .pl(px(4.))
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
                        .text_xs()
                        .font_medium()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_color(cx.theme().foreground.opacity(0.9))
                        .child(format!("{name}/")),
                )
                .child(plus_minus(added, removed, cx)),
        );
        if open {
            level = level.child(
                // Nested level: vertical indent guide + one step deeper.
                div()
                    .ml(px(LEVEL_INDENT))
                    .border_l_1()
                    .border_color(guide)
                    .child(tree_level(sub, selected, depth + 1, closed, cx)),
            );
        }
    }

    level
}

fn file_row(
    ix: usize,
    f: &DiffFile,
    active: bool,
    active_bg: Hsla,
    hov_bg: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
    let name_copy = name.clone();
    let path_copy = f.path.clone();
    div()
        .id(("diff-file", ix))
        .w_full()
        .h(px(ROW_PX))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .pl(px(4.))
        .pr_2()
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| el.bg(if active { active_bg } else { hov_bg }))
        .on_click(cx.listener(move |this, _, _, cx| {
            // Selecting a file IS opening the right pane: a closed
            // pane springs open on the first click.
            let changed = this.diff_file != Some(ix);
            if changed {
                this.diff_hunks_scroll.set_offset(point(px(0.), px(0.)));
            }
            this.diff_file = Some(ix);
            if !this.show_diff {
                this.set_diff(true, cx);
            } else {
                cx.notify();
            }
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
