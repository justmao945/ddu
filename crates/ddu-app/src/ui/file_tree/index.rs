//! The tree's rows as data: what the virtual list renders, flattened from
//! the directories the user has opened (`build_index`), and the default
//! expansion that opens a session on its changes (`seed_open`).

use std::collections::HashSet;

use ddu_diff::listing::{Child, Changes};

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
