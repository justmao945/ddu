//! The sidebar's file tree: the working tree listed **lazily**, one
//! directory at a time (`docs/FILE_TREE.md` §4.1).
//!
//! Nothing here reads the whole repository. [`Changes`] answers "is this
//! one file changed" and "how many changed files are under this
//! directory" off the poll's own diff (path-sorted, so both are a binary
//! search), and [`list_dir`] reads a single directory from the
//! filesystem when the UI expands it. A 40k-file repository therefore
//! draws its few hundred visible rows without ever touching the other
//! 39k — the eager `FileTree` this replaced rebuilt every node, rollup
//! and row list on every 3 s poll (measured: 18 ms of a 47 ms poll at
//! 40k files, and ~5 MB of strings held for the frame).

use std::path::Path;

use git2::Repository;

use super::DiffFile;

/// The poll's changed files, queried by path and by directory.
///
/// Sorted indices rather than a sorted copy: the diff's records carry
/// hunks, and copying them (`TreeEntry` held a path plus stats per file)
/// was most of the memory the eager tree cost.
pub struct Changes<'a> {
    files: &'a [DiffFile],
    /// Indices into `files`, sorted by path.
    order: Vec<u32>,
}

impl<'a> Changes<'a> {
    pub fn of(files: &'a [DiffFile]) -> Self {
        // git2 hands the diff over in its own order; the per-directory
        // queries below need path order, and sorting indices keeps the
        // diff itself untouched.
        let mut order: Vec<u32> = (0..files.len() as u32).collect();
        order.sort_unstable_by(|a, b| files[*a as usize].path.cmp(&files[*b as usize].path));
        Self { files, order }
    }

    fn path(&self, ix: u32) -> &'a str {
        self.files[ix as usize].path.as_str()
    }

    /// The half-open range of `order` holding the paths under `dir`
    /// (`""` = the whole tree). Path order puts a directory's files
    /// together with their `dir/` prefix, so one binary search finds the
    /// start and the run after it is the directory.
    fn range(&self, dir: &str) -> (usize, usize) {
        if dir.is_empty() {
            return (0, self.order.len());
        }
        let prefix = format!("{dir}/");
        let start = self
            .order
            .partition_point(|ix| self.path(*ix) < prefix.as_str());
        let end = start
            + self.order[start..]
                .iter()
                .take_while(|ix| self.path(**ix).starts_with(&prefix))
                .count();
        (start, end)
    }

    /// The diff's record for a path, if the poll found it changed. An
    /// exact lookup — a path that merely *prefixes* other paths (`src`
    /// against `src/main.rs`) is not a file.
    pub fn get(&self, path: &str) -> Option<&'a DiffFile> {
        let ix = self
            .order
            .partition_point(|ix| self.path(*ix) < path);
        let ix = *self.order.get(ix)?;
        (self.path(ix) == path).then(|| &self.files[ix as usize])
    }

    /// How many changed files are under `dir` — the directory row's
    /// badge. Only directories are asked; a file's own path has no
    /// directory range and answers 0.
    pub fn count_under(&self, dir: &str) -> usize {
        let (start, end) = self.range(dir);
        end - start
    }

    /// Every changed path, and every directory on the way to one — what
    /// the tree opens on (`seed_open`).
    pub fn paths(&self) -> impl Iterator<Item = &'a str> + '_ {
        self.order.iter().map(|ix| self.path(*ix))
    }

}

/// One entry of a directory's listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Child {
    File {
        name: String,
        added: usize,
        removed: usize,
        /// The poll found this file changed — the row tints and carries
        /// its figures (a binary or empty change has none).
        changed: bool,
    },
    Dir {
        name: String,
        /// Changed files anywhere under it.
        changed: usize,
    },
}

impl Child {
    pub fn name(&self) -> &str {
        match self {
            Child::File { name, .. } | Child::Dir { name, .. } => name,
        }
    }
}

/// One directory's direct children, read from the filesystem — the only
/// IO the tree does, and only for a directory the UI is showing.
///
/// `.gitignore` is honored through libgit2 (`status_should_ignore`), so
/// the listing agrees with the diff's untracked scan instead of growing
/// a second ignore implementation. A **tracked** file is listed whatever
/// the rules say, which is what git itself does; a submodule (a gitlink
/// in the index) is skipped, as the eager tree skipped it.
pub fn list_dir(repo: &Repository, dir: &str, changes: &Changes<'_>) -> Vec<Child> {
    let Some(root) = repo.workdir() else {
        return Vec::new();
    };
    let abs = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let Ok(entries) = std::fs::read_dir(&abs) else {
        return Vec::new();
    };
    // One index handle for the whole directory: `get_path` is a binary
    // search into it, and only the *untracked* entries need an ignore
    // check at all.
    let index = repo.index().ok();
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == ".git" {
            continue;
        }
        let rel = if dir.is_empty() {
            name.clone()
        } else {
            format!("{dir}/{name}")
        };
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if meta.is_dir() {
            let gitlink = index
                .as_ref()
                .and_then(|ix| ix.get_path(Path::new(&rel), 0))
                .is_some_and(|e| e.mode == 0o160000);
            if gitlink {
                continue;
            }
            if repo.status_should_ignore(Path::new(&rel)).unwrap_or(false) {
                continue;
            }
            dirs.push(Child::Dir {
                name,
                changed: changes.count_under(&rel),
            });
        } else if meta.is_file() {
            let tracked = index
                .as_ref()
                .and_then(|ix| ix.get_path(Path::new(&rel), 0))
                .is_some();
            if !tracked && repo.status_should_ignore(Path::new(&rel)).unwrap_or(false) {
                continue;
            }
            let file = changes.get(&rel);
            files.push(Child::File {
                name,
                added: file.map_or(0, |f| f.added),
                removed: file.map_or(0, |f| f.removed),
                changed: file.is_some(),
            });
        }
    }
    // Directories first, then files, each name-sorted: a tree reads top
    // down through its folders before it lists what sits beside them.
    dirs.sort_unstable_by(|a, b| by_name(a.name(), b.name()));
    files.sort_unstable_by(|a, b| by_name(a.name(), b.name()));
    dirs.extend(files);
    dirs
}

/// Names compare case-insensitively — `README.md` belongs with `readme.md`
/// and after `assets/`, not in a block of its own above every lowercase
/// name — with the exact bytes as the tie-break, so the order stays total
/// and stable for names that differ only in case. Byte-wise folding keeps
/// it allocation-free: the listing is re-sorted on every expansion, and a
/// `to_lowercase()` per comparison would allocate twice per pair.
fn by_name(a: &str, b: &str) -> std::cmp::Ordering {
    fn fold(s: &str) -> impl Iterator<Item = u8> + '_ {
        s.bytes().map(|c| c.to_ascii_lowercase())
    }
    fold(a).cmp(fold(b)).then_with(|| a.cmp(b))
}

/// Ranked matches for the quick-open query: indices into `paths`, best
/// first, at most `limit` of them.
///
/// The query is matched case-insensitively as a **subsequence**
/// (`difpan` finds `src/ui/diff_panel.rs`), ranked by where it lands: a
/// file whose *name* matches beats one that only matches somewhere in its
/// directories, a prefix beats a hit in the middle, a substring beats
/// scattered characters. Ties go to the shorter path, then to path order,
/// so the same query always ranks the same way.
pub fn search(paths: &[String], query: &str, limit: usize) -> Vec<usize> {
    let needle = query.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, usize, usize)> = paths
        .iter()
        .enumerate()
        .filter_map(|(ix, path)| {
            let name = path.rsplit('/').next().unwrap_or(path);
            let lower = name.to_ascii_lowercase();
            let rank = if lower.starts_with(&needle) {
                0
            } else if contains_fold(name, &needle) {
                1
            } else if is_subsequence(name, &needle) {
                2
            } else if contains_fold(path, &needle) {
                3
            } else if is_subsequence(path, &needle) {
                4
            } else {
                return None;
            };
            Some((rank, path.len(), ix))
        })
        .collect();
    hits.sort_unstable();
    hits.truncate(limit);
    hits.into_iter().map(|(_, _, ix)| ix).collect()
}

/// `needle` (already folded) occurs in `hay`, ASCII-case-insensitively.
/// Byte windows rather than a lowered copy of both sides: a directory
/// listing is scanned per keystroke, and `to_ascii_lowercase` on a
/// hundred paths allocates a hundred strings to answer a boolean.
fn contains_fold(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    n.len() <= h.len() && h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
}

/// `needle`'s characters appear in `hay` in order, ASCII-case-insensitively.
fn is_subsequence(hay: &str, needle: &str) -> bool {
    let mut chars = hay.chars();
    needle
        .chars()
        .all(|n| chars.any(|h| h.eq_ignore_ascii_case(&n)))
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

    /// Query by path and by directory off one path-sorted index: exact
    /// lookups, prefix counts, and the sibling-prefix trap (`src` must
    /// not answer with `src-gen/b.rs`, nor `src/a.rs` with `src`).
    #[test]
    fn changes_answer_by_path_and_by_directory() {
        // Deliberately unsorted: `Changes` sorts its own indices.
        let files = vec![
            changed("src/main.rs", 3, 1),
            changed("README.md", 1, 0),
            changed("src-gen/b.rs", 9, 9),
            changed("src/ui/mod.rs", 0, 4),
        ];
        let changes = Changes::of(&files);

        assert_eq!(changes.get("README.md").map(|f| (f.added, f.removed)), Some((1, 0)));
        assert_eq!(changes.get("src/main.rs").map(|f| f.added), Some(3));
        assert!(changes.get("src").is_none(), "a directory is not a file");
        assert!(changes.get("src/none.rs").is_none());
        assert!(changes.get("src-gen").is_none());

        assert_eq!(changes.count_under(""), 4);
        assert_eq!(changes.count_under("src"), 2);
        assert_eq!(changes.count_under("src/ui"), 1);
        assert_eq!(changes.count_under("src-gen"), 1);
        assert_eq!(changes.count_under("docs"), 0);

        let paths: Vec<&str> = changes.paths().collect();
        assert_eq!(paths, ["README.md", "src-gen/b.rs", "src/main.rs", "src/ui/mod.rs"]);
    }

    /// The listing is the filesystem's, with git's own ignore rules:
    /// tracked files show however the rules read, ignored ones do not,
    /// and a directory's badge counts what changed under it.
    #[test]
    fn list_dir_reads_one_directory_through_the_ignore_rules() {
        let root = std::env::temp_dir().join(format!("ddu-tree-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/ui")).expect("mkdir");
        std::fs::create_dir_all(root.join("target/debug")).expect("mkdir");
        std::fs::write(root.join("README.md"), "hi\n").expect("write");
        std::fs::write(root.join("untracked.rs"), "x\n").expect("write");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("src/ui/mod.rs"), "\n").expect("write");
        std::fs::write(root.join("target/debug/junk"), "x\n").expect("write");
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").expect("write");
        std::fs::write(root.join("ignored.log"), "x\n").expect("write");

        let repo = Repository::init(&root).expect("init");
        let mut index = repo.index().expect("index");
        // README.md and src/ are tracked; `target/` holds only ignored
        // files and `untracked.rs` is not in the index at all.
        index.add_path(Path::new("README.md")).expect("add");
        index.add_path(Path::new("src/main.rs")).expect("add");
        index.add_path(Path::new("src/ui/mod.rs")).expect("add");
        index.write().expect("write index");

        let files = vec![changed("src/main.rs", 2, 1), changed("untracked.rs", 1, 0)];
        let changes = Changes::of(&files);

        let names: Vec<String> = list_dir(&repo, "", &changes)
            .iter()
            .map(|c| {
                let kind = match c {
                    Child::File { .. } => "f",
                    Child::Dir { .. } => "d",
                };
                format!("{kind}:{}", c.name())
            })
            .collect();
        // Directories first, then files (each name-sorted);
        // `.gitignore` and the ignored file are gone, `target/`
        // (ignored, no tracked content) is gone with them.
        assert_eq!(
            names,
            ["d:src", "f:.gitignore", "f:README.md", "f:untracked.rs"],
            "listing of the repository root"
        );

        let root_children = list_dir(&repo, "", &changes);
        let src = root_children
            .iter()
            .find(|c| c.name() == "src")
            .expect("src/");
        assert_eq!(
            *src,
            Child::Dir {
                name: "src".to_owned(),
                changed: 1
            }
        );
        let untracked = root_children
            .iter()
            .find(|c| c.name() == "untracked.rs")
            .expect("untracked.rs");
        assert_eq!(
            *untracked,
            Child::File {
                name: "untracked.rs".to_owned(),
                added: 1,
                removed: 0,
                changed: true,
            }
        );
        let readme = root_children
            .iter()
            .find(|c| c.name() == "README.md")
            .expect("README.md");
        assert_eq!(
            *readme,
            Child::File {
                name: "README.md".to_owned(),
                added: 0,
                removed: 0,
                changed: false,
            }
        );

        // The nested listing is its own call, and only its own files.
        let inner: Vec<String> = list_dir(&repo, "src", &changes)
            .iter()
            .map(|c| c.name().to_owned())
            .collect();
        assert_eq!(inner, ["ui", "main.rs"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// Sorting ignores case: an uppercase name is not a block of its own
    /// above every lowercase one, and a case-only pair keeps a total
    /// order (uppercase first, byte-wise) rather than the listing's.
    #[test]
    fn names_sort_case_insensitively() {
        let mut names = ["Zed.md", "api.rs", "README.md", "main.rs", "readme.md"];
        names.sort_unstable_by(|a, b| by_name(a, b));
        assert_eq!(
            names,
            ["api.rs", "main.rs", "README.md", "readme.md", "Zed.md"]
        );
    }

    /// The quick-open ranking, tier by tier: a file whose *name* starts
    /// with the query first (shortest path first), then a name hit inside
    /// a word, then a subsequence, then a path that only matches in its
    /// directories — and never one that does not match at all.
    #[test]
    fn search_ranks_the_file_name_first() {
        let paths: Vec<String> = [
            "docs/FILE_TREE.md",
            "docs/diff/notes.md",
            "src/ui/diff_panel.rs",
            "src/xdiff.rs",
            "src/diff/mod.rs",
            "src/app/diff.rs",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect();
        let named = |query: &str| -> Vec<&str> {
            search(&paths, query, 10)
                .into_iter()
                .map(|ix| paths[ix].as_str())
                .collect()
        };

        assert_eq!(
            named("diff"),
            [
                // A name that *starts* with the query, shortest path first.
                "src/app/diff.rs",
                "src/ui/diff_panel.rs",
                // Then a name that only carries it (and a shorter path
                // does not jump the tier).
                "src/xdiff.rs",
                // Last, the paths whose *name* says nothing: `mod.rs` and
                // `notes.md` match only through their directory.
                "src/diff/mod.rs",
                "docs/diff/notes.md",
            ],
            "ranking for `diff`"
        );
        // A subsequence reaches into the name: `difpan`.
        assert_eq!(named("difpan"), ["src/ui/diff_panel.rs"]);
        // Case folds both ways.
        assert_eq!(named("DIFF_PANEL"), ["src/ui/diff_panel.rs"]);
        assert_eq!(named("file_tree"), ["docs/FILE_TREE.md"]);
        // Nothing for an empty query, and nothing for a miss.
        assert!(named("   ").is_empty());
        assert!(named("zzz").is_empty());
        // The cap is the cap.
        assert_eq!(search(&paths, "s", 2).len(), 2);
    }
}
