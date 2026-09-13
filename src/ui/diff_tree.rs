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
use std::collections::HashSet;
use std::rc::Rc;

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
    // The row list is built once per diff (and per collapse toggle) in
    // `build_index`, not per frame: a changed-only tree can still be
    // thousands of rows, and flattening plus rolling up stats on every
    // render is exactly what made a big repository crawl.
    let Some(index) = this.tree_index.as_ref() else {
        return empty_layer("Loading changes…", cx).into_any_element();
    };
    let sizes = Rc::new(vec![size(px(0.), px(row_px())); index.rows.len()]);

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
                .child(plus_minus(index.added, index.removed, cx)),
        )
        .child(
            // A flex host, not a plain block: the virtual list inside
            // takes its height from this box, and a block parent leaves
            // it unbounded (the list lays out at content height and the
            // layer clips instead of scrolling — see AGENTS.md).
            v_flex()
                .relative()
                .flex_1()
                .min_h_0()
                .child(
                    // Only the visible slice is built per frame; the
                    // rows themselves come from the cached index, so a
                    // scroll costs one range render.
                    v_virtual_list(
                        cx.entity(),
                        "diff-tree",
                        sizes,
                        |this, range, window, cx| tree_rows(this, range, window, cx),
                    )
                    .track_scroll(&this.diff_tree_scroll)
                    .size_full()
                    // No horizontal padding: hover and selection bands
                    // run edge-to-edge — window border to the divider.
                    .pt_2()
                    .pb_2(),
                )
                .scrollbar(&this.diff_tree_scroll, scroll::ScrollbarAxis::Vertical),
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

/// One level of the file tree, by index: `files` are rows at this
/// depth (indices into the diff's file list), `dirs` maps full
/// directory paths to their nested subtrees (insertion order
/// preserved). Indices, not clones — the built tree has to outlive the
/// borrow of the source slice without copying every hunk with it.
#[derive(Default)]
struct TreeNode {
    files: Vec<usize>,
    dirs: Vec<(String, TreeNode)>,
}

/// The tree's rows, ready for the virtual list: the flattened visible
/// tree (collapse applied) plus the whole-tree `+/−` totals. Rebuilt
/// when the diff changes or a directory is toggled — never per frame.
pub(crate) struct TreeIndex {
    pub(crate) rows: Vec<TreeRow>,
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

/// Group flat file rows into a nested directory tree keyed by full dir
/// path (unique expansion keys, any nesting depth).
fn build_tree(files: &[DiffFile]) -> TreeNode {
    let mut root = TreeNode::default();
    for (ix, f) in files.iter().enumerate() {
        let parts: Vec<&str> = f.path.split('/').collect();
        let mut node = &mut root;
        for depth in 0..parts.len().saturating_sub(1) {
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
        node.files.push(ix);
    }
    root
}

/// Recursive +/− totals for a subtree (dir rows show rolled-up stats).
fn tree_stats(node: &TreeNode, files: &[DiffFile]) -> (usize, usize) {
    let mut stats = (
        node.files.iter().map(|ix| files[*ix].added).sum::<usize>(),
        node.files
            .iter()
            .map(|ix| files[*ix].removed)
            .sum::<usize>(),
    );
    for (_, sub) in &node.dirs {
        let (a, r) = tree_stats(sub, files);
        stats.0 += a;
        stats.1 += r;
    }
    stats
}

/// One rendered row of the flattened tree. `depth` drives the inner
/// indent only — rows themselves stay full-width so hover/selection
/// bands run edge-to-edge (window border to divider).
pub(crate) enum TreeRow {
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

/// Flatten a diff plus the session's collapsed set into the row list the
/// virtual list renders. O(files) once per poll — the render path then
/// only touches the visible slice.
pub(crate) fn build_index(files: &[DiffFile], closed: &HashSet<String>) -> TreeIndex {
    let tree = build_tree(files);
    let mut rows = Vec::new();
    flatten(&tree, files, 0, closed, &mut rows);
    let (added, removed) = tree_stats(&tree, files);
    TreeIndex {
        rows,
        added,
        removed,
    }
}

/// Depth-first flatten of the visible tree: files before subdirs,
/// insertion order kept; collapsed subtrees drop out entirely.
fn flatten(
    tree: &TreeNode,
    files: &[DiffFile],
    depth: usize,
    closed: &HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for ix in &tree.files {
        out.push(TreeRow::File { ix: *ix, depth });
    }
    for (path, sub) in &tree.dirs {
        let (added, removed) = tree_stats(sub, files);
        let open = !closed.contains(path);
        out.push(TreeRow::Dir {
            path: path.clone(),
            depth,
            open,
            added,
            removed,
        });
        if open {
            flatten(sub, files, depth + 1, closed, out);
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

/// Builds the rows of one visible slice, straight off the cached index.
fn tree_rows(
    this: &AppView,
    range: std::ops::Range<usize>,
    _window: &mut Window,
    cx: &mut Context<AppView>,
) -> Vec<AnyElement> {
    let (Some(diff), Some(index)) = (this.diff.as_ref(), this.tree_index.as_ref()) else {
        return Vec::new();
    };
    let selected = this.diff_file.filter(|ix| *ix < diff.files.len());
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let guide = cx.theme().foreground.opacity(0.12);

    index.rows[range]
        .iter()
        .map(|row| match row {
            TreeRow::File { ix, depth } => file_row(
                *ix,
                &diff.files[*ix],
                Some(*ix) == selected,
                *depth,
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
            } => dir_row(
                path, *depth, *open, *added, *removed, hov_bg, guide, cx,
            )
            .into_any_element(),
        })
        .collect()
}

fn dir_row(
    path: &str,
    depth: usize,
    open: bool,
    added: usize,
    removed: usize,
    hov_bg: Hsla,
    guide: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let toggle = path.to_owned();
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
            this.rebuild_tree_index();
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
            // File/Preview mode: start reading the newly selected file
            // (no-op in Diff mode, and cheap when the cache still holds).
            this.ensure_file_content(cx);
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

#[cfg(test)]
mod tests {
    use crate::app::AppView;
    use crate::config::{Config, State};
    use crate::diff::{DiffFile, DiffHunk, DiffLine, GitDiff};
    use gpui_kit::{Entity, TestAppContext, gpui};

    /// A changed-only tree of a few thousand rows: the layer must be a
    /// real scroller (its rows come from the cached index, the visible
    /// slice from the virtual list) — the scroll region rule that a
    /// padded or unbounded host silently breaks.
    #[test]
    fn a_large_tree_scrolls_and_keeps_its_rows_indexed() {
        fn file(ix: usize) -> DiffFile {
            DiffFile {
                path: format!("many/d{}/f{ix}.txt", ix / 100),
                added: 1,
                removed: 0,
                hunks: vec![DiffHunk {
                    header: "@@ -1 +1,2 @@".into(),
                    lines: vec![DiffLine {
                        kind: '+',
                        old_no: None,
                        new_no: Some(2),
                        text: "changed".into(),
                    }],
                }],
                lines_total: 1,
                truncated: false,
            }
        }

        gpui::run_test_once(
            0,
            Box::new(|dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("diff_tree_virtual"));
                let cx = &mut cx0;
                cx.update(gpui_kit::init);
                cx.update(|cx| {
                    cx.set_global(Config::default());
                    cx.set_global(crate::config::LoadWarnings(vec![]));
                    // No saved workspace: the launch seeds the cwd
                    // project and spawns its default session, which is
                    // what puts the tree layer on screen at all.
                    cx.set_global(State::default());
                });
                let (view, vcx): (Entity<AppView>, _) =
                    cx.add_window_view(|window, cx| AppView::new(window, cx));
                // Inject, then draw in the NEXT update: a `cx.notify()`
                // reaches the cached sidebar panel when the update cycle
                // flushes, so the frame that shows the new rows is the
                // one drawn after it.
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        let files: Vec<DiffFile> = (0..3000).map(file).collect();
                        v.show_diff_tree = true;
                        v.diff = Some(GitDiff {
                            branch: None,
                            files,
                        });
                        v.rebuild_tree_index();
                        v.diff_file = Some(0);
                        cx.notify();
                    });
                });
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let (rows, max) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    (
                        v.tree_index.as_ref().map(|i| i.rows.len()).unwrap_or(0),
                        f32::from(v.diff_tree_scroll.base_handle().max_offset().y),
                    )
                });
                // The index is the row list: 3000 files plus one dir row
                // per `dN/` (30 of them, plus `many/`).
                assert_eq!(rows, 3000 + 31, "the tree index survived the frame");
                assert!(
                    max > 0.,
                    "3000 rows in a 220px layer must scroll (max offset {max})"
                );
                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
    }
}
