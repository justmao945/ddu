//! The working tree as the sidebar lists it: **every** tracked and
//! untracked file (`.gitignore` respected), with the poll's diff folded
//! in as per-file stats — `docs/FILE_TREE.md` §4.1.
//!
//! Built once per poll from the in-memory index plus the diff deltas the
//! poll already computes: no second workdir walk, no subprocess. The
//! nodes hold *indices* into [`FileTree::entries`], never copies of a
//! path, so the UI can flatten rows without re-splitting anything.

use std::cmp::Ordering;

use super::DiffFile;

/// One file of the working tree. `changed` marks the ones the poll's
/// diff found — they carry the stats; a clean file has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub changed: bool,
}

/// One directory level: the files directly in it (indices into
/// [`FileTree::entries`], path-sorted), its subdirectories, and the
/// rollups its row and the ancestors' badges read.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DirNode {
    pub files: Vec<usize>,
    /// Subdirectory name → its subtree. The name is the last path
    /// component; the node's full path is implied by the walk.
    pub dirs: Vec<(String, DirNode)>,
    /// Files anywhere under this subtree.
    pub files_total: usize,
    /// How many of them the diff found.
    pub changed_total: usize,
    pub added: usize,
    pub removed: usize,
}

/// The whole working tree.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct FileTree {
    /// Every file, path-sorted — the tree's rows index into this.
    pub entries: Vec<TreeEntry>,
    pub root: DirNode,
}

impl FileTree {
    pub fn files(&self) -> usize {
        self.entries.len()
    }

    pub fn changed(&self) -> usize {
        self.root.changed_total
    }

    pub fn added(&self) -> usize {
        self.root.added
    }

    pub fn removed(&self) -> usize {
        self.root.removed
    }

    /// The entry for a repo-relative path, if the tree lists it.
    pub fn get(&self, path: &str) -> Option<&TreeEntry> {
        let ix = self
            .entries
            .binary_search_by(|e| e.path.as_str().cmp(path))
            .ok()?;
        self.entries.get(ix)
    }
}

/// Build the tree from the index's paths (sorted, as git stores them)
/// and the poll's diff. The union is a merge of two sorted runs: a path
/// in both is a changed file, a diff-only path is untracked, an
/// index-only path is clean. A file deleted in the workdir is still in
/// the index, which is exactly right — it belongs in the tree, carrying
/// its removed lines.
pub fn build(index: Vec<String>, diff: &[DiffFile]) -> FileTree {
    let mut found: Vec<&DiffFile> = diff.iter().collect();
    found.sort_by(|a, b| a.path.cmp(&b.path));

    // The merge below needs both runs sorted. git's index is, by
    // construction — but an unsorted run would not fail loudly, it would
    // silently duplicate entries (one per `Equal` missed), so the
    // contract is checked here rather than trusted: one pass of string
    // compares, and a sort only if a caller ever breaks it.
    let mut clean: Vec<String> = index;
    if !clean.windows(2).all(|pair| pair[0] <= pair[1]) {
        clean.sort_unstable();
    }

    let mut entries = Vec::with_capacity(clean.len() + found.len());
    let mut clean = clean.into_iter().peekable();
    let mut changed = found.into_iter().peekable();
    loop {
        match (clean.peek(), changed.peek()) {
            (Some(path), Some(file)) => match path.as_str().cmp(file.path.as_str()) {
                Ordering::Less => entries.push(clean_entry(clean.next().expect("peeked"))),
                Ordering::Equal => {
                    let path = clean.next().expect("peeked");
                    let file = changed.next().expect("peeked");
                    entries.push(changed_entry(path, file));
                }
                Ordering::Greater => {
                    let file = changed.next().expect("peeked");
                    entries.push(changed_entry(file.path.clone(), file));
                }
            },
            (Some(_), None) => entries.push(clean_entry(clean.next().expect("peeked"))),
            (None, Some(_)) => {
                let file = changed.next().expect("peeked");
                entries.push(changed_entry(file.path.clone(), file));
            }
            (None, None) => break,
        }
    }

    let mut root = DirNode::default();
    // Sorted entries mean a directory's files are contiguous and each
    // level's children arrive in order, so one pass builds the tree:
    // walk the path components, take the child named after each (dirs at
    // one level have unique names), and drop the file index in its level.
    for (ix, entry) in entries.iter().enumerate() {
        let mut node = &mut root;
        let mut parts = entry.path.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                break;
            }
            let pos = match node.dirs.iter().position(|(name, _)| name == part) {
                Some(pos) => pos,
                None => {
                    node.dirs.push((part.to_owned(), DirNode::default()));
                    node.dirs.len() - 1
                }
            };
            node = &mut node.dirs[pos].1;
        }
        node.files.push(ix);
    }
    rollup(&mut root, &entries);
    FileTree { entries, root }
}

fn clean_entry(path: String) -> TreeEntry {
    TreeEntry {
        path,
        added: 0,
        removed: 0,
        changed: false,
    }
}

fn changed_entry(path: String, file: &DiffFile) -> TreeEntry {
    TreeEntry {
        path,
        added: file.added,
        removed: file.removed,
        changed: true,
    }
}

/// Post-order pass: a directory's totals are its own files plus every
/// subtree's. Depth is the path depth, so this cannot recurse far.
fn rollup(node: &mut DirNode, entries: &[TreeEntry]) {
    node.files_total = node.files.len();
    node.changed_total = node.files.iter().filter(|ix| entries[**ix].changed).count();
    node.added = node.files.iter().map(|ix| entries[*ix].added).sum();
    node.removed = node.files.iter().map(|ix| entries[*ix].removed).sum();
    for (_, sub) in &mut node.dirs {
        rollup(sub, entries);
        node.files_total += sub.files_total;
        node.changed_total += sub.changed_total;
        node.added += sub.added;
        node.removed += sub.removed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed(path: &str, added: usize, removed: usize) -> DiffFile {
        DiffFile {
            path: path.to_owned(),
            added,
            removed,
            ..Default::default()
        }
    }

    fn paths(tree: &FileTree) -> Vec<&str> {
        tree.entries.iter().map(|e| e.path.as_str()).collect()
    }

    /// The union: tracked, untracked and deleted all land in the tree,
    /// each with the right kind, and the ignored never do (the caller
    /// never hands them over — the index and the diff both skip them).
    #[test]
    fn index_and_diff_merge_into_one_sorted_tree() {
        let index = vec![
            "README.md".to_owned(),
            "docs/a.md".to_owned(),
            "docs/gone.md".to_owned(),
            "src/main.rs".to_owned(),
        ];
        let diff = vec![
            changed("README.md", 3, 1),
            changed("docs/gone.md", 0, 7),
            changed("new.txt", 5, 0),
        ];
        let tree = build(index, &diff);

        assert_eq!(
            paths(&tree),
            [
                "README.md",
                "docs/a.md",
                "docs/gone.md",
                "new.txt",
                "src/main.rs"
            ]
        );
        assert_eq!(tree.files(), 5);
        assert_eq!(tree.changed(), 3);
        assert_eq!((tree.added(), tree.removed()), (8, 8));

        // Clean file: listed, muted, no figures.
        let readme = tree.get("README.md").expect("README.md");
        assert!(readme.changed);
        assert_eq!((readme.added, readme.removed), (3, 1));
        let clean = tree.get("docs/a.md").expect("docs/a.md");
        assert!(!clean.changed);
        assert_eq!((clean.added, clean.removed), (0, 0));
        // Untracked: in the tree, changed, outside the index.
        assert!(tree.get("new.txt").expect("new.txt").changed);
        assert!(tree.get("nope.txt").is_none());

        // Rollups: `docs/` sums its own files, the root sums everything.
        let (name, docs) = &tree.root.dirs[0];
        assert_eq!(name, "docs");
        assert_eq!(docs.files_total, 2);
        assert_eq!(docs.changed_total, 1);
        assert_eq!(docs.removed, 7);
        assert_eq!(tree.root.files_total, 5);
        assert_eq!(tree.root.files.len(), 2, "root files: README.md, new.txt");

        // Subdirectories keep the sorted order, and nesting is by level.
        let (name, src) = &tree.root.dirs[1];
        assert_eq!(name, "src");
        assert_eq!(src.files_total, 1);
        assert_eq!(src.files.len(), 1);
        assert!(src.dirs.is_empty());
    }

    /// A clean tree is not an empty tree: that is the whole point of the
    /// listing — no diff, every file still there.
    #[test]
    fn a_clean_tree_lists_every_file_with_no_changes() {
        let index = vec![
            "a.txt".to_owned(),
            "deep/nest/one.rs".to_owned(),
            "deep/nest/two.rs".to_owned(),
        ];
        let tree = build(index, &[]);

        assert_eq!(paths(&tree), ["a.txt", "deep/nest/one.rs", "deep/nest/two.rs"]);
        assert_eq!((tree.files(), tree.changed()), (3, 0));
        assert_eq!((tree.added(), tree.removed()), (0, 0));
        assert_eq!(tree.root.changed_total, 0);
        let (name, deep) = &tree.root.dirs[0];
        assert_eq!(name, "deep");
        assert!(deep.files.is_empty(), "deep/ holds no files directly");
        assert_eq!(deep.dirs.len(), 1);
        assert_eq!(deep.dirs[0].0, "nest");
        assert_eq!(deep.dirs[0].1.files_total, 2);
    }

    /// Two sibling directories whose names share a prefix must not be
    /// confused with each other, and `get` must find deep paths. The
    /// path order is git's (byte order): `-` sorts before `/`, so
    /// `src-gen/…` comes before `src/…`.
    #[test]
    fn sibling_directories_stay_distinct() {
        let index = vec![
            "src-gen/b.rs".to_owned(),
            "src-gen/c.rs".to_owned(),
            "src/a.rs".to_owned(),
        ];
        let tree = build(index, &[]);
        assert_eq!(tree.root.dirs.len(), 2);
        assert_eq!(tree.root.dirs[0].0, "src-gen");
        assert_eq!(tree.root.dirs[1].0, "src");
        assert_eq!(tree.root.dirs[0].1.files_total, 2, "src-gen/ holds two");
        assert_eq!(tree.root.dirs[1].1.files_total, 1, "src/ holds one");
        assert!(tree.get("src-gen/c.rs").is_some());
        assert!(tree.get("src-gen/missing.rs").is_none());
    }
}
