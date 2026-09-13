//! The file tree: the sidebar's lower layer. **Every** file in the
//! working tree, grouped by directory, with the poll's changes carrying
//! their `+/−` figures and tint while clean files stay listed and
//! selectable (`docs/FILE_TREE.md` §4.1/§5.1). Two states — `All` and
//! `Changed` — and a default-collapse rule that folds clean directories
//! away so a 20k-file repository opens on its changes.

use super::{diff_file_icon, hover_bg, meta_text, panel_header_px, row_px, scaled, selection_bg};
use gpui_kit::component::button::{Toggle, ToggleVariant, ToggleVariants as _};
use gpui_kit::component::menu::{PopupMenuItem, *};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppView;
use std::collections::HashSet;
use std::rc::Rc;

use crate::diff::tree::{DirNode, FileTree};

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
    // A clean working tree is not an empty tree: the listing is the
    // point (FILE_TREE.md §1), so the only things that keep the layer
    // empty are "no poll yet" and "not a repository".
    let Some(tree) = this.tree() else {
        let note = this.diff_error.as_deref().unwrap_or("Loading files…");
        return empty_layer(note, cx).into_any_element();
    };
    if tree.files() == 0 {
        return empty_layer("No files in the working tree.", cx).into_any_element();
    }
    // The row list is built once per snapshot (and per filter/collapse
    // change) in `build_index`, not per frame: flattening the tree plus
    // its rollups on every render is what made a big repository crawl.
    let Some(index) = this.tree_index.as_ref() else {
        return empty_layer("Loading files…", cx).into_any_element();
    };
    let sizes = Rc::new(vec![size(px(0.), px(row_px())); index.rows.len()]);

    v_flex()
        .size_full()
        .min_w_0()
        .overflow_hidden()
        // Summary strip: the two states, and what the changes total.
        .child(
            div()
                .h(px(panel_header_px()))
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_2()
                .pl(px(4.))
                .pr_2()
                .child(filter_toggle(this, tree, cx))
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

/// The tree's rows, ready for the virtual list: the flattened visible
/// tree (collapse applied) plus the whole-tree `+/−` totals. Rebuilt
/// when the diff changes or a directory is toggled — never per frame.
pub(crate) struct TreeIndex {
    pub(crate) rows: Vec<TreeRow>,
    pub(crate) added: usize,
    pub(crate) removed: usize,
}

/// Which files the layer lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum TreeFilter {
    /// Every file in the working tree — the default once the listing
    /// exists (a clean repository still shows its tree).
    #[default]
    All,
    /// Only the files the diff found: the pre-listing behavior.
    Changed,
}

impl TreeFilter {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Changed => "changed",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "all" => Some(Self::All),
            "changed" => Some(Self::Changed),
            _ => None,
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::All => Self::Changed,
            Self::Changed => Self::All,
        }
    }
}

/// Fold every directory without a changed descendant away, once per
/// session (`FILE_TREE.md` §5.1's default-collapse rule): the layer opens
/// on the changes and their ancestors, and the user's own toggles decide
/// from there. Only dirs are considered, and a dir already in the set —
/// e.g. restored from the session's saved state — is left alone.
pub(crate) fn seed_clean_dirs(tree: &FileTree, closed: &mut HashSet<String>) {
    fn walk(path: &str, node: &DirNode, closed: &mut HashSet<String>) {
        if node.changed_total == 0 {
            closed.insert(path.to_owned());
            return;
        }
        for (name, sub) in &node.dirs {
            let full = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}/{name}")
            };
            walk(&full, sub, closed);
        }
    }
    for (name, sub) in &tree.root.dirs {
        walk(name, sub, closed);
    }
}

/// One rendered row of the flattened tree. `depth` drives the inner
/// indent only — rows themselves stay full-width so hover/selection
/// bands run edge-to-edge (window border to divider). File rows name an
/// entry of [`FileTree::entries`] by index: the row list never copies a
/// path.
pub(crate) enum TreeRow {
    File {
        ix: usize,
        depth: usize,
    },
    Dir {
        path: String,
        depth: usize,
        open: bool,
        /// Files under this directory, and how many of them changed.
        files: usize,
        changed: usize,
    },
}

/// Flatten the listing (through the filter) plus the session's collapsed
/// set into the rows the virtual list renders. O(files) once per
/// snapshot — the render path then only touches the visible slice.
pub(crate) fn build_index(
    tree: &FileTree,
    filter: TreeFilter,
    closed: &HashSet<String>,
) -> TreeIndex {
    let mut rows = Vec::new();
    flatten(&tree.root, "", tree, filter, 0, closed, &mut rows);
    TreeIndex {
        rows,
        added: tree.added(),
        removed: tree.removed(),
    }
}

/// Depth-first flatten of the visible tree: files before subdirs, sorted
/// order kept; collapsed subtrees drop out entirely. `Changed` skips both
/// clean files and whole clean subtrees, which is what makes it cheap on
/// a large repository. `path` is the node's full path (empty at the
/// root).
#[allow(clippy::too_many_arguments)]
fn flatten(
    node: &DirNode,
    path: &str,
    tree: &FileTree,
    filter: TreeFilter,
    depth: usize,
    closed: &HashSet<String>,
    out: &mut Vec<TreeRow>,
) {
    for ix in &node.files {
        if filter == TreeFilter::Changed && !tree.entries[*ix].changed {
            continue;
        }
        out.push(TreeRow::File { ix: *ix, depth });
    }
    for (name, sub) in &node.dirs {
        if filter == TreeFilter::Changed && sub.changed_total == 0 {
            continue;
        }
        let full = if path.is_empty() {
            name.clone()
        } else {
            format!("{path}/{name}")
        };
        let open = !closed.contains(&full);
        out.push(TreeRow::Dir {
            path: full.clone(),
            depth,
            open,
            files: sub.files_total,
            changed: sub.changed_total,
        });
        if open {
            flatten(sub, &full, tree, filter, depth + 1, closed, out);
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

/// Builds the rows of one visible slice, straight off the cached index
/// and the entry list it indexes into.
fn tree_rows(
    this: &AppView,
    range: std::ops::Range<usize>,
    _window: &mut Window,
    cx: &mut Context<AppView>,
) -> Vec<AnyElement> {
    let (Some(tree), Some(index)) = (this.tree(), this.tree_index.as_ref()) else {
        return Vec::new();
    };
    let selected = this.current_diff_path();
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let guide = cx.theme().foreground.opacity(0.12);

    index.rows[range]
        .iter()
        .map(|row| match row {
            TreeRow::File { ix, depth } => {
                let entry = &tree.entries[*ix];
                file_row(
                    entry,
                    Some(entry.path.as_str()) == selected,
                    *depth,
                    active_bg,
                    hov_bg,
                    guide,
                    cx,
                )
                .into_any_element()
            }
            TreeRow::Dir {
                path,
                depth,
                open,
                files,
                changed,
            } => dir_row(
                path, *depth, *open, *files, *changed, hov_bg, guide, cx,
            )
            .into_any_element(),
        })
        .collect()
}

/// The layer's two states, one toggle each: `All 1 204` / `Changed 7`.
/// Standalone toggles rather than a group, like the pane's mode switch
/// (a group swallows its children's clicks).
fn filter_toggle(this: &AppView, tree: &FileTree, cx: &mut Context<AppView>) -> impl IntoElement {
    let active = this.tree_filter;
    h_flex()
        .flex_shrink_0()
        .gap_1()
        .children([TreeFilter::All, TreeFilter::Changed].map(|filter| {
            let count = match filter {
                TreeFilter::All => tree.files(),
                TreeFilter::Changed => tree.changed(),
            };
            Toggle::new(SharedString::from(format!("tree-filter-{}", filter.as_str())))
                .label(format!(
                    "{} {}",
                    match filter {
                        TreeFilter::All => "All",
                        TreeFilter::Changed => "Changed",
                    },
                    grouped(count)
                ))
                .checked(active == filter)
                .with_variant(ToggleVariant::Ghost)
                .with_size(gpui_kit::component::Size::XSmall)
                .tooltip(match filter {
                    TreeFilter::All => "List every file in the working tree",
                    TreeFilter::Changed => "List only the changed files",
                })
                .on_click(cx.listener(move |this, _: &bool, _, cx| {
                    if this.tree_filter != filter {
                        this.toggle_tree_filter(cx);
                    }
                }))
        }))
}

/// `1204` → `1 204`: thin spaces keep long counts readable and the two
/// toggle labels from jumping around.
fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, ch) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out
}

/// A directory row: the name, its own changed-descendant badge (a
/// rolled-up `+/−` across a whole subtree says very little — the count
/// does), and the file total when it holds no changes.
fn dir_row(
    path: &str,
    depth: usize,
    open: bool,
    files: usize,
    changed: usize,
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
        .aria_label(SharedString::from(match changed {
            0 => format!("{path}/ {files} files"),
            n => format!("{path}/ {files} files, {n} changed"),
        }))
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
        .child(dir_badge(files, changed, cx))
}

/// The dir row's trailing figures: `● n` when the subtree changed,
/// otherwise how many files are in it (muted — the badge is a state, the
/// count is not).
fn dir_badge(files: usize, changed: usize, cx: &mut Context<AppView>) -> impl IntoElement {
    let tone = cx.theme().foreground.opacity(0.45);
    h_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .text_sm()
        .font_family(cx.theme().mono_font_family.clone())
        .text_color(tone)
        .when(changed > 0, |el| {
            el.child(div().text_color(cx.theme().yellow).child("●"))
                .child(div().text_color(cx.theme().yellow).child(changed.to_string()))
        })
        .when(changed == 0, |el| el.child(files.to_string()))
}

/// A file row: the name, tinted and carrying `+a/−b` when the diff found
/// the file, muted and bare when it is unchanged — both selectable, both
/// opening the pane.
fn file_row(
    entry: &crate::diff::tree::TreeEntry,
    active: bool,
    depth: usize,
    active_bg: Hsla,
    hov_bg: Hsla,
    guide: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let f = entry;
    let name = f.path.rsplit('/').next().unwrap_or(&f.path).to_string();
    let name_copy = name.clone();
    let path_copy = f.path.clone();
    let path = f.path.clone();
    div()
        .id(SharedString::from(format!("diff-file-{}", f.path)))
        .role(Role::TreeItem)
        .aria_selected(active)
        .aria_label(SharedString::from(if f.changed {
            format!("{} +{} −{}", f.path, f.added, f.removed)
        } else {
            format!("{}", f.path)
        }))
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
            this.select_path(path.clone());
            // Whole-file surface: start reading the newly selected file
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
                .text_color(if f.changed {
                    cx.theme().foreground.opacity(0.9)
                } else {
                    // Unchanged: listed, but visibly not what the agent
                    // touched.
                    cx.theme().foreground.opacity(0.45)
                })
                .child(name),
        )
        .when(f.changed, |el| el.child(plus_minus(f.added, f.removed, cx)))
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
    use super::{build_index, seed_clean_dirs, TreeFilter, TreeIndex, TreeRow};
    use crate::diff::tree::{build as build_tree, FileTree};
    use crate::diff::DiffFile;
    use std::collections::HashSet;

    fn changed(path: &str, added: usize, removed: usize) -> DiffFile {
        DiffFile {
            path: path.to_owned(),
            added,
            removed,
            ..Default::default()
        }
    }

    fn rows(index: &TreeIndex, tree: &FileTree) -> Vec<String> {
        index
            .rows
            .iter()
            .map(|row| match row {
                TreeRow::File { ix, .. } => tree.entries[*ix].path.clone(),
                TreeRow::Dir { path, open, .. } => format!("{path}/{}", if *open { "open" } else { "closed" }),
            })
            .collect()
    }

    /// The default-collapse rule plus the two filters, over one tree:
    /// `All` lists everything (clean directories folded away), `Changed`
    /// lists only the changed subtrees.
    #[test]
    fn clean_dirs_collapse_and_changed_prunes_them() {
        let index = vec![
            "README.md".to_owned(),
            "docs/a.md".to_owned(),
            "docs/deep/b.md".to_owned(),
            "src/changed.rs".to_owned(),
        ];
        let diff = vec![changed("src/changed.rs", 2, 1)];
        let tree = build_tree(index, &diff);
        assert_eq!((tree.files(), tree.changed()), (4, 1));

        // Seeding: only the subtrees with changes stay open.
        let mut closed = HashSet::new();
        seed_clean_dirs(&tree, &mut closed);
        assert_eq!(closed, HashSet::from(["docs".to_owned()]));

        // All: the whole tree, with the clean directory collapsed.
        let all = build_index(&tree, TreeFilter::All, &closed);
        assert_eq!(
            rows(&all, &tree),
            ["README.md", "docs/closed", "src/open", "src/changed.rs"]
        );
        assert_eq!((all.added, all.removed), (2, 1));

        // The dir row carries the file total and the changed count.
        let TreeRow::Dir { files, changed, .. } = &all.rows[1] else {
            panic!("a directory row");
        };
        assert_eq!((*files, *changed), (2, 0));
        let TreeRow::Dir { files, changed, .. } = &all.rows[2] else {
            panic!("a directory row");
        };
        assert_eq!((*files, *changed), (1, 1));

        // Changed: clean files and whole clean subtrees drop out.
        let changed_only = build_index(&tree, TreeFilter::Changed, &closed);
        assert_eq!(rows(&changed_only, &tree), ["src/open", "src/changed.rs"]);

        // An unchanged selection stays put with the filter on All.
        assert!(tree.get("docs/deep/b.md").is_some());
    }

    /// The filter's persisted spelling round-trips, and the chord's step
    /// is a two-state toggle.
    #[test]
    fn filter_spellings_round_trip() {
        for filter in [TreeFilter::All, TreeFilter::Changed] {
            assert_eq!(TreeFilter::parse(filter.as_str()), Some(filter));
            assert_eq!(filter.next().next(), filter);
        }
        assert_eq!(TreeFilter::parse("nope"), None);
        assert_eq!(TreeFilter::default(), TreeFilter::All);
    }

    use crate::app::AppView;
    use crate::config::{Config, State};
    use crate::diff::{DiffHunk, DiffLine, GitDiff};
    use gpui_kit::{Entity, TestAppContext, gpui};

    /// A changed-only tree of a few thousand rows: the layer must be a
    /// real scroller (its rows come from the cached index, the visible
    /// slice from the virtual list) — the scroll region rule that a
    /// padded or unbounded host silently breaks.
    #[test]
    fn a_large_tree_scrolls_and_keeps_its_rows_indexed() {
        fn file(ix: usize) -> DiffFile {
            DiffFile {
                path: format!("many/d{:02}/f{ix:04}.txt", ix / 100),
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
                        // Every file is changed, so no directory is clean
                        // and the default-collapse rule leaves them all
                        // open — the row list is the whole tree.
                        let tree = crate::diff::tree::build(
                            (0..3000)
                                .map(|ix| format!("many/d{:02}/f{ix:04}.txt", ix / 100))
                                .collect(),
                            &files,
                        );
                        v.snapshot = Some(crate::diff::Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files,
                            },
                            tree,
                        });
                        v.show_diff_tree = true;
                        v.rebuild_tree_index();
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
