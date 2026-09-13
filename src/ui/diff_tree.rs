//! The file tree: the sidebar's lower layer. **Every** file in the
//! working tree, grouped by directory, with the poll's changes carrying
//! their `+/−` figures and tint while clean files stay listed and
//! selectable (`docs/FILE_TREE.md` §4.1/§5.1).
//!
//! Lazy, and merged: the rows are the directories the user has opened,
//! listed on demand ([`crate::diff::tree::list_dir`]) with the diff's
//! changes folded straight into them — one tree, no separate "changes"
//! list and no eager walk of the repository. It opens on its changes
//! (`seed_open`: the ancestors of every changed file), so a 20k-file
//! repository starts where the agent left off.

use std::collections::HashSet;
use std::rc::Rc;

use super::{diff_file_icon, hover_bg, meta_text, row_px, scaled, selection_bg};
use gpui_kit::base::input;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::menu::{PopupMenuItem, *};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::app::AppView;
use crate::diff::tree::{Child, Changes};

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

/// The sidebar's lower layer: the working tree's files and changes.
pub(crate) fn render(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    // The tree answers "this session's changes": with no active
    // session there is nothing to show, even if the last poll left
    // a stale project diff behind.
    if this.current_session().is_none() {
        return empty_layer("No active session — select one in the project tree.", cx)
            .into_any_element();
    }
    // The quick open answers from the working tree's own path list, so
    // it stands in for the tree entirely — and renders with or without
    // a poll's listing behind it.
    if this.file_search.open {
        return search_layer(this, cx).into_any_element();
    }
    // A clean working tree is not an empty tree: the listing is the
    // point (FILE_TREE.md §1), so the only things that keep the layer
    // empty are "no poll yet" and "not a repository".
    let Some(index) = this.tree_index.as_ref() else {
        let note = this.diff_error.as_deref().unwrap_or("Loading files…");
        return empty_layer(note, cx).into_any_element();
    };
    if index.rows.is_empty() {
        return empty_layer("No files in the working tree.", cx).into_any_element();
    }
    let sizes = Rc::new(vec![size(px(0.), px(row_px())); index.rows.len()]);

    v_flex()
        .size_full()
        .min_w_0()
        .overflow_hidden()
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

/// The layer while the quick open is up: the bar, then the hits it
/// found — or the note that says why there are none. The tree is not
/// rendered underneath: the search replaces it rather than filtering
/// what happens to be expanded.
fn search_layer(this: &AppView, cx: &mut Context<AppView>) -> AnyElement {
    let query = this.file_search.input.read(cx).value().trim().to_owned();
    // One map per render, not one scan per row: the hit rows carry the
    // `+/−` figures of the files the poll found changed, and a linear
    // search of the diff for each of 200 rows is the slow way to say
    // "changed".
    let changed: std::collections::HashMap<&str, (usize, usize)> = this
        .diff()
        .map(|diff| {
            diff.files
                .iter()
                .map(|f| (f.path.as_str(), (f.added, f.removed)))
                .collect()
        })
        .unwrap_or_default();
    let rows: Vec<AnyElement> = this
        .file_search
        .matches
        .iter()
        .enumerate()
        .filter_map(|(hit, ix)| {
            let path = this.file_search.paths.get(*ix)?;
            Some(
                search_row(
                    hit,
                    path,
                    changed.get(path.as_str()).copied(),
                    hit == this.file_search.current,
                    cx,
                )
                .into_any_element(),
            )
        })
        .collect();
    let body: AnyElement = if rows.is_empty() {
        empty_layer(
            if query.is_empty() {
                "Type to search the working tree."
            } else {
                "No files match."
            },
            cx,
        )
        .into_any_element()
    } else {
        // A plain list, not a virtual one: the hits are capped
        // (`FILE_SEARCH_MAX`), so there is nothing to virtualize.
        div()
            .id("file-search-hits")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&this.diff_tree_scroll)
            .pt_2()
            .pb_2()
            .children(rows)
            .into_any_element()
    };
    v_flex()
        .size_full()
        .min_w_0()
        .overflow_hidden()
        .child(file_search_bar(this, cx))
        .child(
            v_flex()
                .relative()
                .flex_1()
                .min_h_0()
                .child(body)
                .scrollbar(&this.diff_tree_scroll, scroll::ScrollbarAxis::Vertical),
        )
        .into_any_element()
}

/// The quick-open strip: a search input, the hit counter, and the three
/// buttons the find bars also carry (previous, next, close). ⌘G/⌘⇧G
/// drive the same steps while the input holds focus — the plain arrows
/// belong to the input's own caret, which is why the bar uses the chord
/// the find bars already taught.
fn file_search_bar(this: &AppView, cx: &mut Context<AppView>) -> impl IntoElement {
    let search = &this.file_search;
    let input = search.input.clone();
    let focus = input.read(cx).focus_handle(cx).clone();
    let total = search.matches.len();
    let has_matches = total > 0;
    let typed = !input.read(cx).value().trim().is_empty();
    let counter: SharedString = if !typed {
        "".into()
    } else if !has_matches {
        "No results".into()
    } else {
        format!("{}/{}", search.current.min(total - 1) + 1, total).into()
    };
    let chord = crate::app::accel_hint("G");

    h_flex()
        .id("file-search-bar")
        .flex_shrink_0()
        .track_focus(&focus)
        .key_context("FileSearch")
        // The layer's root focuses the window fallback on any mouse
        // down; without stopping propagation a click into the input
        // would bounce focus right back out.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|_, _: &MouseDownEvent, _, cx| cx.stop_propagation()),
        )
        .on_action(cx.listener(|this, enter: &input::Enter, window, cx| {
            if enter.shift {
                this.file_search_step(true, cx);
            } else {
                this.commit_file_search(window, cx);
            }
        }))
        .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
            this.close_file_search(window, cx);
        }))
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .child(
            Input::new(&input)
                .aria_label("Search files")
                .small()
                .flex_1()
                .min_w_0()
                .appearance(true)
                .focus_bordered(false)
                .cleanable(true),
        )
        .child(
            div()
                .id("file-search-counter")
                .role(Role::Label)
                .aria_label(counter.clone())
                .min_w(px(scaled(46.)))
                .text_center()
                .text_xs()
                .text_color(cx.theme().foreground.opacity(if has_matches { 0.55 } else { 0.35 }))
                .child(counter),
        )
        .child(
            Button::new("file-search-prev")
                .accessibility_label(format!("Previous file ({chord} ⇧)"))
                .xsmall()
                .ghost()
                .icon(IconName::ChevronLeft)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.file_search_step(true, cx))),
        )
        .child(
            Button::new("file-search-next")
                .accessibility_label(format!("Next file ({chord})"))
                .xsmall()
                .ghost()
                .icon(IconName::ChevronRight)
                .disabled(!has_matches)
                .on_click(cx.listener(|this, _, _, cx| this.file_search_step(false, cx))),
        )
        .child(
            Button::new("file-search-close")
                .accessibility_label("Close file search")
                .xsmall()
                .ghost()
                .icon(IconName::Close)
                .on_click(cx.listener(|this, _, window, cx| this.close_file_search(window, cx))),
        )
}

/// One hit: the path with its directories muted and the file's own name
/// bright — the name is what the query was about — plus the `+/−`
/// figures when the poll found the file changed. Clicking it is Enter.
fn search_row(
    hit: usize,
    path: &str,
    changed: Option<(usize, usize)>,
    active: bool,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let (dir, name) = match path.rfind('/') {
        Some(ix) => (&path[..=ix], &path[ix + 1..]),
        None => ("", path),
    };
    let hov_bg = hover_bg(cx);
    let active_bg = selection_bg(cx);
    div()
        .id(("file-search-hit", hit))
        .role(Role::ListItem)
        .aria_label(SharedString::from(match changed {
            Some((added, removed)) => format!("{path}{}", figures(added, removed, ' ')),
            None => path.to_owned(),
        }))
        .aria_selected(active)
        .w_full()
        .h(px(row_px()))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .cursor_pointer()
        .map(|el| if active { el.bg(active_bg) } else { el })
        .hover(move |el| el.bg(if active { active_bg } else { hov_bg }))
        .on_click(cx.listener(move |this, _, window, cx| {
            // Clicking a hit is Enter on it: the bar's cursor moves
            // there and the pick commits.
            this.file_search.current = hit;
            this.commit_file_search(window, cx);
        }))
        .child(
            diff_file_icon(path)
                .with_size(gpui_kit::component::Size::XSmall)
                .text_color(cx.theme().foreground.opacity(0.6)),
        )
        .child(
            h_flex()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_sm()
                .whitespace_nowrap()
                .when(!dir.is_empty(), |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_color(cx.theme().foreground.opacity(0.45))
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
        .when_some(changed, |el, (added, removed)| {
            el.when(added > 0 || removed > 0, |el| {
                el.child(plus_minus(added, removed, cx))
            })
        })
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

/// The tree's rows, ready for the virtual list: the expanded
/// directories' listings (the diff folded in) plus the whole-tree `+/−`
/// totals. Rebuilt when the diff, the expansion or the listing changes —
/// never per frame.
pub(crate) struct TreeIndex {
    pub(crate) rows: Vec<TreeRow>,
}

/// One rendered row of the flattened tree. `depth` drives the inner
/// indent only — rows themselves stay full-width so hover/selection
/// bands run edge-to-edge (window border to divider).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TreeRow {
    File {
        path: String,
        depth: usize,
        added: usize,
        removed: usize,
        changed: bool,
    },
    Dir {
        path: String,
        depth: usize,
        open: bool,
        /// Changed files anywhere under this directory.
        changed: usize,
    },
}

/// Flatten the **open** directories into the rows the virtual list
/// renders, listing each one through `list` — the root always, and every
/// other directory the user has expanded. Nothing else is read: this is
/// the whole reason a 40k-file repository draws in a few hundred rows.
pub(crate) fn build_index(
    open: &HashSet<String>,
    mut list: impl FnMut(&str) -> Vec<Child>,
) -> TreeIndex {
    let mut rows = Vec::new();
    walk("", 0, open, &mut list, &mut rows);
    TreeIndex { rows }
}

/// Depth-first flatten of one directory: subdirs before files, name order
/// from the listing, collapsed subtrees never listed.
fn walk(
    dir: &str,
    depth: usize,
    open: &HashSet<String>,
    list: &mut impl FnMut(&str) -> Vec<Child>,
    out: &mut Vec<TreeRow>,
) {
    for child in list(dir) {
        let path = if dir.is_empty() {
            child.name().to_owned()
        } else {
            format!("{dir}/{}", child.name())
        };
        match child {
            Child::File {
                added,
                removed,
                changed,
                ..
            } => out.push(TreeRow::File {
                path,
                depth,
                added,
                removed,
                changed,
            }),
            Child::Dir { changed, .. } => {
                let is_open = open.contains(&path);
                out.push(TreeRow::Dir {
                    path: path.clone(),
                    depth,
                    open: is_open,
                    changed,
                });
                if is_open {
                    walk(&path, depth + 1, open, list, out);
                }
            }
        }
    }
}

/// Open every directory on the way to a changed file — the default
/// expansion, once per session (`FILE_TREE.md` §5.1): the layer opens on
/// the changes and their ancestors, and the user's own toggles decide
/// from there.
pub(crate) fn seed_open(open: &mut HashSet<String>, changes: &Changes<'_>) {
    for path in changes.paths() {
        let mut rest = path;
        while let Some(cut) = rest.rfind('/') {
            rest = &rest[..cut];
            open.insert(rest.to_owned());
        }
    }
}

/// The layer's rows for one visible slice, straight off the cached index.
fn tree_rows(
    this: &AppView,
    range: std::ops::Range<usize>,
    _window: &mut Window,
    cx: &mut Context<AppView>,
) -> Vec<AnyElement> {
    let Some(index) = this.tree_index.as_ref() else {
        return Vec::new();
    };
    let selected = this.current_diff_path();
    let active_bg = selection_bg(cx);
    let hov_bg = hover_bg(cx);
    let guide = cx.theme().foreground.opacity(0.12);

    index.rows[range]
        .iter()
        .map(|row| match row {
            TreeRow::File {
                path,
                depth,
                added,
                removed,
                changed,
            } => file_row(
                path, *depth, *added, *removed, *changed,
                Some(path.as_str()) == selected,
                active_bg, hov_bg, guide, cx,
            )
            .into_any_element(),
            TreeRow::Dir {
                path,
                depth,
                open,
                changed,
            } => dir_row(path, *depth, *open, *changed, hov_bg, guide, cx).into_any_element(),
        })
        .collect()
}

/// A directory row: the name, and a `● n` badge when the diff found
/// changes anywhere under it (a rolled-up `+/−` across a whole subtree
/// says very little — the count does). A clean directory carries no
/// badge: how many files are inside it is not a status.
fn dir_row(
    path: &str,
    depth: usize,
    open: bool,
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
            0 => format!("{path}/"),
            n => format!("{path}/ {n} changed"),
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
            // Open by default: presence in the set = expanded.
            if !this.diff_tree_open.remove(&toggle) {
                this.diff_tree_open.insert(toggle.clone());
            }
            this.rebuild_tree_index();
            this.persist(cx);
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
        .when(changed > 0, |el| {
            el.child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .text_sm()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().yellow)
                    .child("●")
                    .child(changed.to_string()),
            )
        })
}

/// A file row: the name in the tree's own text color, carrying `+a/−b`
/// when the diff found the file and bare when it did not — whether a file
/// changed is what the figures say, not what the name's weight says.
#[allow(clippy::too_many_arguments)]
fn file_row(
    path: &str,
    depth: usize,
    added: usize,
    removed: usize,
    changed: bool,
    active: bool,
    active_bg: Hsla,
    hov_bg: Hsla,
    guide: Hsla,
    cx: &mut Context<AppView>,
) -> impl IntoElement {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let name_copy = name.clone();
    let path_copy = path.to_owned();
    let path_owned = path.to_owned();
    div()
        .id(SharedString::from(format!("diff-file-{path}")))
        .role(Role::TreeItem)
        .aria_selected(active)
        .aria_label(SharedString::from(if changed {
            format!("{path}{}", figures(added, removed, ' '))
        } else {
            format!("{path}")
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
            this.select_path(path_owned.clone());
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
            diff_file_icon(path)
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
                .text_color(cx.theme().foreground.opacity(0.9))
                .child(name),
        )
        // A changed file with no line figures (binary, or an empty new
        // file) shows none: `+0 −0` reads as a status without being one.
        .when(changed && (added > 0 || removed > 0), |el| {
            el.child(plus_minus(added, removed, cx))
        })
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

/// Right-aligned tabular `+N −N` figures in a fixed track: the counts
/// align vertically across rows, GitHub-style.
///
/// A zero side is left out entirely (`+8` rather than `+8 −0`, nothing at
/// all for a binary change): a figure of zero is not information, and a
/// row of `+0 −0` reads as though something happened.
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
        .when(added > 0, |el| {
            el.child(
                div()
                    .min_w(px(scaled(34.)))
                    .text_right()
                    .text_color(cx.theme().green)
                    .child(format!("+{added}")),
            )
        })
        .when(removed > 0, |el| {
            el.child(
                div()
                    .min_w(px(scaled(34.)))
                    .text_right()
                    .text_color(cx.theme().red)
                    .child(format!("−{removed}")),
            )
        })
}

/// The same figures as text, for a row's accessibility label (and the
/// tile's tooltip).
pub(crate) fn figures(added: usize, removed: usize, sep: char) -> String {
    let mut out = String::new();
    if added > 0 {
        out.push_str(&format!("+{added}"));
    }
    if removed > 0 {
        if !out.is_empty() {
            out.push(sep);
        }
        out.push_str(&format!("−{removed}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{build_index, seed_open, TreeIndex, TreeRow};
    use crate::diff::tree::{Changes, Child};
    use crate::diff::DiffFile;
    use std::collections::HashMap;
    use std::collections::HashSet;

    fn changed(path: &str, added: usize, removed: usize) -> DiffFile {
        DiffFile {
            path: path.to_owned(),
            added,
            removed,
            ..Default::default()
        }
    }

    /// A fake working tree: `dir` → its children, one entry per test
    /// directory. Every call is counted, which is how the laziness is
    /// pinned — the whole point of the rewrite.
    struct FakeFs {
        dirs: HashMap<&'static str, Vec<Child>>,
        listed: Vec<String>,
    }

    impl FakeFs {
        fn new() -> Self {
            Self {
                dirs: HashMap::new(),
                listed: Vec::new(),
            }
        }

        fn dir(mut self, dir: &'static str, children: Vec<Child>) -> Self {
            self.dirs.insert(dir, children);
            self
        }

        fn index(&mut self, open: &HashSet<String>, changes: &Changes<'_>) -> TreeIndex {
            let dirs = std::mem::take(&mut self.dirs);
            let mut listed = Vec::new();
            let index = build_index(open, |dir| {
                listed.push(dir.to_owned());
                // Like `list_dir`: the listing happens first, the diff's
                // figures are folded into it by full path.
                dirs.get(dir)
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|child| match child {
                        Child::File { name, .. } => {
                            let path = if dir.is_empty() {
                                name.clone()
                            } else {
                                format!("{dir}/{name}")
                            };
                            let file = changes.get(&path);
                            Child::File {
                                name,
                                added: file.map_or(0, |f| f.added),
                                removed: file.map_or(0, |f| f.removed),
                                changed: file.is_some(),
                            }
                        }
                        dir => dir,
                    })
                    .collect()
            });
            self.dirs = dirs;
            self.listed = listed;
            index
        }
    }

    fn file(name: &str) -> Child {
        Child::File {
            name: name.to_owned(),
            added: 0,
            removed: 0,
            changed: false,
        }
    }

    fn dir(name: &str, changed: usize) -> Child {
        Child::Dir {
            name: name.to_owned(),
            changed,
        }
    }

    fn rows(index: &TreeIndex) -> Vec<String> {
        index
            .rows
            .iter()
            .map(|row| match row {
                // Rows name full paths; the layer prints the last
                // component under its indent, which is what this mirrors.
                TreeRow::File { path, depth, .. } => {
                    format!("{}{}", "  ".repeat(*depth), path.rsplit('/').next().unwrap())
                }
                TreeRow::Dir {
                    path, depth, open, ..
                } => format!(
                    "{}{}/{}",
                    "  ".repeat(*depth),
                    path.rsplit('/').next().unwrap(),
                    if *open { "" } else { " (closed)" }
                ),
            })
            .collect()
    }

    fn tree() -> FakeFs {
        FakeFs::new()
            .dir(
                "",
                vec![
                    dir("docs", 0),
                    dir("src", 1),
                    file("README.md"),
                    file("changed.rs"),
                ],
            )
            .dir("docs", vec![dir("deep", 0), file("a.md")])
            .dir("docs/deep", vec![file("b.md")])
            .dir("src", vec![dir("ui", 0), file("changed.rs")])
            .dir("src/ui", vec![file("mod.rs")])
    }

    /// The default expansion plus the rows it produces: the changes and
    /// their ancestors are open, everything else is one click away —
    /// and the directories that are not open are **never listed**.
    #[test]
    fn the_tree_opens_on_its_changes_and_lists_nothing_else() {
        let diff = vec![changed("src/changed.rs", 2, 1)];
        let changes = Changes::of(&diff);
        let mut open = HashSet::new();
        seed_open(&mut open, &changes);
        assert_eq!(open, HashSet::from(["src".to_owned()]));

        let mut fs = tree();
        let index = fs.index(&open, &changes);
        assert_eq!(
            rows(&index),
            [
                "docs/ (closed)",
                "src/",
                "  ui/ (closed)",
                "  changed.rs",
                "README.md",
                "changed.rs",
            ]
        );
        // The root and `src/` — and *nothing* under `docs/` or `src/ui/`.
        assert_eq!(fs.listed, ["", "src"]);

        // The row carries the diff's figures, and the dir badge counts
        // the changes under it.
        let TreeRow::File {
            path,
            added,
            removed,
            changed,
            ..
        } = &index.rows[3]
        else {
            panic!("a file row");
        };
        assert_eq!(path, "src/changed.rs");
        assert_eq!((*added, *removed, *changed), (2, 1, true));
        let TreeRow::Dir { path, changed, .. } = &index.rows[1] else {
            panic!("a dir row");
        };
        assert_eq!((path.as_str(), *changed), ("src", 1));
        let TreeRow::Dir { path, changed, .. } = &index.rows[0] else {
            panic!("a dir row");
        };
        assert_eq!((path.as_str(), *changed), ("docs", 0), "a clean directory");
    }

    /// A clean repository opens folded: one listing, the root, and every
    /// directory a click away.
    #[test]
    fn a_clean_tree_lists_only_the_root() {
        let changes = Changes::of(&[]);
        let mut open = HashSet::new();
        seed_open(&mut open, &changes);
        let mut fs = tree();
        let index = fs.index(&open, &changes);
        assert_eq!(
            rows(&index),
            [
                "docs/ (closed)",
                "src/ (closed)",
                "README.md",
                "changed.rs"
            ]
        );
        assert_eq!(fs.listed, [""]);
    }

    /// Expanding is what lists a directory — and the deeper rows come
    /// with it, marked open so the row can carry the right chevron.
    #[test]
    fn expanding_lists_one_more_directory() {
        let changes = Changes::of(&[]);
        let mut open = HashSet::from(["docs".to_owned()]);
        let mut fs = tree();
        let index = fs.index(&open, &changes);
        assert_eq!(
            rows(&index),
            [
                "docs/",
                "  deep/ (closed)",
                "  a.md",
                "src/ (closed)",
                "README.md",
                "changed.rs"
            ]
        );
        assert_eq!(fs.listed, ["", "docs"]);

        open.insert("docs/deep".to_owned());
        let index = fs.index(&open, &changes);
        assert_eq!(
            rows(&index),
            [
                "docs/",
                "  deep/",
                "    b.md",
                "  a.md",
                "src/ (closed)",
                "README.md",
                "changed.rs"
            ]
        );
        assert_eq!(fs.listed, ["", "docs", "docs/deep"]);
    }

    /// The seeding rule descends from the change to its directory, and
    /// leaves a sibling alone.
    #[test]
    fn seeding_opens_every_ancestor_of_a_change() {
        let diff = vec![
            changed("src/ui/mod.rs", 1, 1),
            changed("docs/deep/b.md", 1, 0),
            changed("top.rs", 1, 0),
        ];
        let changes = Changes::of(&diff);
        let mut open = HashSet::new();
        seed_open(&mut open, &changes);
        assert_eq!(
            open,
            HashSet::from([
                "src".to_owned(),
                "src/ui".to_owned(),
                "docs".to_owned(),
                "docs/deep".to_owned()
            ])
        );
    }

    /// The layer end to end, against a real repository: `rebuild_tree_index`
    /// lists the root plus the directories a change opened, and nothing
    /// else — 40 files in a directory nobody opened are never read — while
    /// the rows it does have are a real scroller (the scroll region rule a
    /// padded or unbounded host silently breaks).
    #[test]
    fn the_layer_lists_only_open_directories() {
        use crate::app::AppView;
        use crate::config::{Config, State};
        use crate::diff::{DiffFile, DiffHunk, DiffLine, GitDiff, Snapshot};
        use gpui_kit::{Entity, TestAppContext, gpui};

        fn file(ix: usize, dir: usize) -> DiffFile {
            DiffFile {
                path: format!("many/d{dir:02}/f{ix:04}.txt"),
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

        // A real working tree on disk: the listing is the filesystem's,
        // which is the whole point of the rewrite.
        let root = std::env::temp_dir().join(format!("ddu-tree-layer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("mkdir");
        let repo = git2::Repository::init(&root).expect("init");
        for dir in 0..30 {
            std::fs::create_dir_all(root.join(format!("many/d{dir:02}"))).expect("mkdir");
            for ix in 0..40 {
                std::fs::write(
                    root.join(format!("many/d{dir:02}/f{ix:04}.txt")),
                    "content\n",
                )
                .expect("write");
            }
        }
        let cwd = root.clone();
        let changed = vec![file(7, 3)];

        gpui::run_test_once(
            0,
            Box::new(move |dispatcher| {
                let mut cx0 = TestAppContext::build(dispatcher, Some("diff_tree_virtual"));
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
                        v.projects[0].path = cwd.clone();
                        v.projects[0].sessions[0].cwd = cwd.clone();
                        v.snapshot = Some(Snapshot {
                            diff: GitDiff {
                                branch: None,
                                files: changed.clone(),
                            },
                        });
                        v.show_diff_tree = true;
                        v.tree_seeded = false;
                        v.diff_tree_open.clear();
                        v.rebuild_tree_index();
                        cx.notify();
                    });
                });
                vcx.update(|window, cx| {
                    let _ = window.draw(cx);
                });
                let (rows, dirs, files, max) = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    let index = v.tree_index.as_ref().expect("rows");
                    let dirs = index
                        .rows
                        .iter()
                        .filter(|r| matches!(r, TreeRow::Dir { .. }))
                        .count();
                    let files = index
                        .rows
                        .iter()
                        .filter(|r| matches!(r, TreeRow::File { .. }))
                        .count();
                    (
                        index.rows.len(),
                        dirs,
                        files,
                        f32::from(v.diff_tree_scroll.base_handle().max_offset().y),
                    )
                });
                // `many/`, its 30 subdirectories, and the one opened
                // `d03/`'s 40 files — the other 1 160 files on disk were
                // never read.
                assert_eq!(dirs, 31, "the root and `many/` only");
                assert_eq!(files, 40, "`many/d03/` and nothing else");
                assert_eq!(rows, 71);
                assert!(
                    max > 0.,
                    "71 rows in a 220px layer must scroll (max offset {max})"
                );
                let open = vcx.update(|_, cx| view.read(cx).diff_tree_open.clone());
                assert_eq!(
                    open,
                    HashSet::from(["many".to_owned(), "many/d03".to_owned()]),
                    "the seeding rule opened exactly the change's path"
                );

                // Opening one more directory lists exactly that one.
                vcx.update(|_, cx| {
                    view.update(cx, |v, cx| {
                        v.diff_tree_open.insert("many/d04".to_owned());
                        v.rebuild_tree_index();
                        cx.notify();
                    });
                });
                let files = vcx.update(|_, cx| {
                    let v = view.read(cx);
                    v.tree_index
                        .as_ref()
                        .expect("rows")
                        .rows
                        .iter()
                        .filter(|r| matches!(r, TreeRow::File { .. }))
                        .count()
                });
                assert_eq!(files, 80, "d03 and d04, and nothing deeper");

                cx.update(|cx| {
                    cx.background_executor().forbid_parking();
                    cx.quit();
                });
                cx.run_until_parked();
            }),
        );
        drop(repo);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `figures` is what a row's label (and the pane's header) prints:
    /// one side, both, or nothing.
    #[test]
    fn figures_leave_out_a_zero_side() {
        use super::figures;
        assert_eq!(figures(8, 0, ' '), "+8");
        assert_eq!(figures(0, 4, ' '), "−4");
        assert_eq!(figures(8, 4, ' '), "+8 −4");
        assert_eq!(figures(0, 0, ' '), "");
    }
}
