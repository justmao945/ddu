//! The sidebar's file tree: the working tree listed **lazily**, one
//! directory at a time (`docs/FILE_TREE.md` §4.1).
//!
//! [`Changes`] answers "is this one file changed" and "how many changed
//! files are under this directory" off the poll's own diff (path-sorted,
//! so both are a binary search), and [`list_dir`] reads a single
//! directory from the filesystem when the UI expands it. A 40k-file
//! repository therefore draws its few hundred visible rows without ever
//! touching the other 39k — the eager `FileTree` this replaced rebuilt
//! every node, rollup and row list on every 3 s poll (measured: 18 ms of
//! a 47 ms poll at 40k files, and ~5 MB of strings held for the frame).
//!
//! The one whole-tree read is [`walk_files`], and it belongs to the quick
//! open: it runs on the background executor once per palette open
//! (measured: this repository's 44k files in ~30 ms), never on the poll
//! and never per frame.

use std::path::Path;

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
/// **Nothing is filtered but `.git`.** The tree is the working tree as
/// the filesystem has it: what `.gitignore` hides (`target/`, `.env`, a
/// build output) is listed like anything else, in the tree's own text
/// color with no figures — git's view of a file is the *badge*, not a
/// gate on the listing. A submodule directory lists like any other (it is
/// one), and a symlink is classified by what it points at (`entry_kind`).
pub fn list_dir(root: &Path, dir: &str, changes: &Changes<'_>) -> Vec<Child> {
    let abs = if dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(dir)
    };
    let Ok(entries) = std::fs::read_dir(&abs) else {
        return Vec::new();
    };
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
        let Some(kind) = entry_kind(&entry) else {
            continue;
        };
        if kind.is_dir() {
            dirs.push(Child::Dir {
                name,
                changed: changes.count_under(&rel),
            });
        } else {
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

/// Every file under `root`, workdir-relative and sorted — the quick
/// open's universe.
///
/// A real walk, not the index: the palette is for "open the file I have
/// in mind", which reaches what git does not track and what the ignore
/// rules hide (`target/`, `node_modules/`, `.env`), at any depth. `.git`
/// is the one name skipped — its objects are not files a user opens — a
/// symlink that resolves to a file is listed and one that resolves to a
/// directory is not (it has no content to show, and the tree is where a
/// directory is browsed), a non-UTF-8 path is left out rather than
/// lossily compared.
pub fn walk_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    // Depth-first with an explicit stack: a workdir is deep enough that a
    // recursive walk risks the default 8 MB thread stack, and a
    // background executor's threads are what run this.
    let mut todo = vec![(root.to_path_buf(), String::new())];
    while let Some((abs, rel)) = todo.pop() {
        let Ok(entries) = std::fs::read_dir(&abs) else {
            continue;
        };
        for entry in entries.flatten() {
            let Some(kind) = entry_kind(&entry) else {
                continue;
            };
            // A directory a link points at is not descended: its target is
            // in the list by its own path, and a cycle does not terminate.
            if kind == Kind::LinkDir {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name == ".git" {
                continue;
            }
            let child = if rel.is_empty() {
                name
            } else {
                format!("{rel}/{name}")
            };
            if kind.is_file() {
                out.push(child);
            } else {
                todo.push((entry.path(), child));
            }
        }
    }
    out.sort_unstable();
    out
}

/// What a directory entry is, for both listings. A symlink is classified
/// by what it points at: `DirEntry::file_type` reports the link itself,
/// so a link to a file would otherwise be neither a directory nor a file
/// and would vanish from a listing that asked it. Whether the link *was*
/// one survives into `LinkDir`/`LinkFile`, because the tree browses what
/// a linked directory holds while [`walk_files`] refuses to descend one.
/// A broken link is neither, and is dropped.
fn entry_kind(entry: &std::fs::DirEntry) -> Option<Kind> {
    let file_type = entry.file_type().ok()?;
    let link = file_type.is_symlink();
    let file_type = if link {
        std::fs::metadata(entry.path()).ok()?.file_type()
    } else {
        file_type
    };
    if file_type.is_dir() {
        Some(if link { Kind::LinkDir } else { Kind::Dir })
    } else if file_type.is_file() {
        Some(if link { Kind::LinkFile } else { Kind::File })
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Dir,
    File,
    LinkDir,
    LinkFile,
}

impl Kind {
    fn is_dir(self) -> bool {
        matches!(self, Kind::Dir | Kind::LinkDir)
    }

    fn is_file(self) -> bool {
        matches!(self, Kind::File | Kind::LinkFile)
    }
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

/// How many query prefixes one typing branch keeps. Each level holds the
/// paths that matched it, so the depth is also the memo's memory bound —
/// deep enough that a real query's expensive levels (the first character
/// or two, which nearly the whole tree matches) sit behind the cheap ones,
/// shallow enough that the levels are nothing beside the path list.
const MEMO_DEPTH: usize = 8;

/// The quick open's paths, and the memo that makes ranking them cheap.
///
/// Every tier of the ranking is a **substring or subsequence** test, so a
/// needle that extends a prefix can only match a subset of what that
/// prefix matched, and can only rank a path **the same or worse**: dropping
/// a needle's tail only loosens a test, so what matched a prefix's *name*
/// still matches, and what needed the whole *path* may not. A keystroke is
/// therefore scored against the previous keystroke's survivors, and skips
/// the tiers those survivors had already failed — a path that only ever
/// matched as a subsequence of the path is tested for that one thing.
/// Measured on this repository's 44k paths: one typed query costs ~13 ms
/// across its keystrokes instead of ~63 ms rebuilding each time, its last
/// keystrokes are microseconds (a `listing.r` looks at 57 paths), and the
/// worst single keystroke is ~4 ms.
///
/// The ranking is the same either way — the memo only decides how much of
/// the list a keystroke looks at. Backspace re-uses the deepest memoized
/// prefix the query still extends (its ranks are *better* then, so nothing
/// is skipped); a query that extends nothing memoized (a paste, a cleared
/// field, a different word) is a full pass.
pub struct PathSearch {
    paths: Vec<String>,
    /// `(needle, what it matched)`, shortest first and each one a prefix
    /// of the next: the branch of queries typed into this bar so far.
    memo: Vec<(String, Vec<Matched>)>,
}

/// One survivor of a memoized query: the path, and the tier it matched at
/// **for that query**.
#[derive(Clone, Copy)]
struct Matched {
    rank: u8,
    ix: u32,
}

impl PathSearch {
    pub fn new(paths: Vec<String>) -> Self {
        Self { paths, memo: Vec::new() }
    }

    /// Point the search at a new list — the walk landing, a session
    /// switch, the bar closing. Every memoized level belongs to the old
    /// paths and is dropped with them: their indices would name other
    /// files.
    pub fn set_paths(&mut self, paths: Vec<String>) {
        self.paths = paths;
        self.memo.clear();
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    /// The path at `ix` — what a hit renders and what Enter opens.
    pub fn path(&self, ix: usize) -> Option<&str> {
        self.paths.get(ix).map(String::as_str)
    }

    /// The ranked hits for `query`: indices into the paths, best first, at
    /// most `limit` of them.
    ///
    /// The query is matched case-insensitively as a **subsequence**
    /// (`difpan` finds `src/ui/diff_panel.rs`), ranked by where it lands: a
    /// file whose *name* matches beats one that only matches somewhere in
    /// its directories, a prefix beats a hit in the middle, a substring
    /// beats scattered characters. Ties go to the shorter path, then to
    /// path order, so the same query always ranks the same way.
    pub fn rank(&mut self, query: &str, limit: usize) -> Vec<usize> {
        let needle = query.trim().to_ascii_lowercase();
        if needle.is_empty() {
            self.memo.clear();
            return Vec::new();
        }
        // Where the scan starts: the survivors of the deepest memoized
        // prefix of this query, or `None` for the whole list.
        let start = self.memo.iter().rposition(|(m, _)| needle.starts_with(m.as_str()));
        let scan = start.map_or(self.paths.len(), |level| self.memo[level].1.len());
        // Both are at most as long as the candidate set, and a one-letter
        // query on a build-output tree fills them: reserve instead of
        // doubling a Vec into its final size, per keystroke.
        let (mut hits, mut survivors) = (Vec::with_capacity(scan), Vec::with_capacity(scan));
        match start {
            Some(level) => {
                // A level's needle is a prefix of this one: either the same
                // query again (its ranks *are* this query's) or a shorter
                // one whose verdicts stand as the floor to start from.
                let exact = self.memo[level].0.len() == needle.len();
                for &Matched { rank, ix } in &self.memo[level].1 {
                    if exact {
                        hits.push((rank, self.paths[ix as usize].len(), ix));
                        survivors.push(Matched { rank, ix });
                    } else {
                        score_from(&self.paths, ix, &needle, rank, &mut hits, &mut survivors);
                    }
                }
            }
            None => {
                for ix in 0..self.paths.len() as u32 {
                    score_from(&self.paths, ix, &needle, 0, &mut hits, &mut survivors);
                }
            }
        }
        // Keep the branch: the levels this query still extends, then the
        // query itself. The oldest level is the one a longer query is
        // least likely to come back to.
        self.memo.truncate(start.map_or(0, |level| level + 1));
        if self.memo.len() == MEMO_DEPTH {
            self.memo.remove(0);
        }
        self.memo.push((needle, survivors));
        // The palette shows at most `limit` hits, and a one-letter query on
        // a build-output tree matches nearly every path: partition to the
        // best `limit` (linear) and sort only those, rather than sorting
        // tens of thousands of rows to throw them away. The ordering is
        // total — the index breaks a tie — so the kept slice is exactly the
        // sort's prefix.
        if hits.len() > limit {
            hits.select_nth_unstable(limit);
            hits.truncate(limit);
        }
        hits.sort_unstable();
        hits.into_iter().map(|(_, _, ix)| ix as usize).collect()
    }

    /// The reuse points the last query left, shortest first — what the
    /// next keystroke can start from. Read by the tests that pin the memo
    /// to the typing it is supposed to pay for — nothing in the app asks,
    /// so it is not built into the app.
    #[cfg(test)]
    pub(crate) fn memo_levels(&self) -> usize {
        self.memo.len()
    }
}

/// Score one path against `needle`, pushing it onto the survivors and its
/// rank onto the hits — testing only the tiers at or below `from`, the
/// worst rank the path could have had for a needle this one extends.
fn score_from(
    paths: &[String],
    ix: u32,
    needle: &str,
    from: u8,
    hits: &mut Vec<(u8, usize, u32)>,
    survivors: &mut Vec<Matched>,
) {
    let path = &paths[ix as usize];
    let name = path.rsplit('/').next().unwrap_or(path);
    let rank = if starts_with_fold(name, needle) {
        0
    } else if from <= 1 && contains_fold(name, needle) {
        1
    } else if from <= 2 && is_subsequence(name, needle) {
        2
    } else if from <= 3 && contains_fold(path, needle) {
        3
    } else if is_subsequence(path, needle) {
        4
    } else {
        return;
    };
    // The tier a path matched a shorter needle at is the last one it can
    // match a longer one at: every tier is a substring or a subsequence
    // test — on the name or on the path — and dropping a needle's tail
    // only loosens it.
    debug_assert!(rank >= from, "a longer needle ranked a path better: {path}");
    hits.push((rank, path.len(), ix));
    survivors.push(Matched { rank, ix });
}

/// `needle` (already folded) is the start of `hay`, ASCII-case-insensitively
/// — the ranking's first tier, without the folded copy of the name a
/// `to_ascii_lowercase()` would allocate for every path on every keystroke.
fn starts_with_fold(hay: &str, needle: &str) -> bool {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    h.len() >= n.len() && h[..n.len()].eq_ignore_ascii_case(n)
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

    /// The listing is the filesystem's: what `.gitignore` hides is listed
    /// like anything else (with no figures — the diff is what marks a
    /// change, not the listing's gate), `.git` never is, a directory's
    /// badge counts what changed under it, and a symlink is what it points
    /// at.
    #[test]
    fn list_dir_reads_the_directory_as_the_filesystem_has_it() {
        let root = std::env::temp_dir().join(format!("ddu-tree-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/ui")).expect("mkdir");
        std::fs::create_dir_all(root.join("target/debug")).expect("mkdir");
        std::fs::create_dir_all(root.join(".git/objects")).expect("mkdir");
        std::fs::write(root.join("README.md"), "hi\n").expect("write");
        std::fs::write(root.join("untracked.rs"), "x\n").expect("write");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("src/ui/mod.rs"), "\n").expect("write");
        std::fs::write(root.join("target/debug/junk"), "x\n").expect("write");
        std::fs::write(root.join(".git/objects/pack"), "x\n").expect("write");
        // Ignored by git's rules — which the listing does not read.
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").expect("write");
        std::fs::write(root.join("ignored.log"), "x\n").expect("write");
        std::os::unix::fs::symlink(root.join("README.md"), root.join("link.rs")).expect("symlink");
        std::os::unix::fs::symlink(root.join("src"), root.join("vendored")).expect("symlink");
        // A link nothing answers: neither a file nor a directory.
        std::os::unix::fs::symlink(root.join("gone"), root.join("broken")).expect("symlink");

        let files = vec![changed("src/main.rs", 2, 1), changed("untracked.rs", 1, 0)];
        let changes = Changes::of(&files);

        let names: Vec<String> = list_dir(&root, "", &changes)
            .iter()
            .map(|c| {
                let kind = match c {
                    Child::File { .. } => "f",
                    Child::Dir { .. } => "d",
                };
                format!("{kind}:{}", c.name())
            })
            .collect();
        // Directories first, then files (each name-sorted). `target/` is
        // here despite the rules, `ignored.log` beside it, `.git` and the
        // broken link are not.
        assert_eq!(
            names,
            [
                "d:src",
                "d:target",
                "d:vendored",
                "f:.gitignore",
                "f:ignored.log",
                "f:link.rs",
                "f:README.md",
                "f:untracked.rs",
            ],
            "listing of the working tree root"
        );

        let root_children = list_dir(&root, "", &changes);
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
        let target = root_children
            .iter()
            .find(|c| c.name() == "target")
            .expect("target/");
        assert_eq!(
            *target,
            Child::Dir {
                name: "target".to_owned(),
                changed: 0,
            },
            "an ignored directory is listed, with nothing changed under it"
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
        let ignored = root_children
            .iter()
            .find(|c| c.name() == "ignored.log")
            .expect("ignored.log");
        assert_eq!(
            *ignored,
            Child::File {
                name: "ignored.log".to_owned(),
                added: 0,
                removed: 0,
                changed: false,
            }
        );

        // The nested listing is its own call, and only its own files.
        let inner: Vec<String> = list_dir(&root, "src", &changes)
            .iter()
            .map(|c| c.name().to_owned())
            .collect();
        assert_eq!(inner, ["ui", "main.rs"]);

        let _ = std::fs::remove_dir_all(&root);
    }

    /// The quick open's universe is the filesystem's, not git's: an
    /// ignored path and any depth of directory are in, `.git` is out
    /// wherever it sits, and a link is a file only when it resolves to one
    /// (a linked directory is not descended — its target is listed by its
    /// own path).
    #[test]
    fn walk_files_reaches_what_git_hides() {
        let root = std::env::temp_dir().join(format!("ddu-walk-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src/deep/deeper")).expect("mkdir");
        std::fs::create_dir_all(root.join("src/deep/.git")).expect("mkdir");
        std::fs::create_dir_all(root.join("target/debug/deps")).expect("mkdir");
        std::fs::create_dir_all(root.join(".git/objects/ab")).expect("mkdir");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write");
        std::fs::write(root.join("src/deep/deeper/deep.rs"), "\n").expect("write");
        std::fs::write(root.join("src/deep/.git/config"), "x\n").expect("write");
        std::fs::write(root.join("target/debug/deps/lib.rlib"), "x\n").expect("write");
        std::fs::write(root.join("ignored.log"), "x\n").expect("write");
        std::fs::write(root.join(".gitignore"), "target/\n*.log\n").expect("write");
        std::fs::write(root.join(".git/objects/ab/1234"), "x\n").expect("write");
        std::os::unix::fs::symlink(root.join("src/main.rs"), root.join("link.rs")).expect("symlink");
        std::os::unix::fs::symlink(root.join("src"), root.join("vendored")).expect("symlink");

        assert_eq!(
            walk_files(&root),
            [
                ".gitignore",
                "ignored.log",
                "link.rs",
                "src/deep/deeper/deep.rs",
                "src/main.rs",
                "target/debug/deps/lib.rlib",
            ],
            "sorted, relative, every depth — and nothing under `.git`"
        );

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

    /// One quick-open corpus, shared by the tests that pin the ranking:
    /// every tier, a case pair, a directory-only match and a name that
    /// only a subsequence reaches.
    fn corpus() -> Vec<String> {
        [
            "docs/FILE_TREE.md",
            "docs/diff/notes.md",
            "src/ui/diff_panel.rs",
            "src/xdiff.rs",
            "src/diff/mod.rs",
            "src/app/diff.rs",
            "src/Diff/Notes.rs",
            "target/release/deps/libdiff_notes.rlib",
        ]
        .iter()
        .map(|p| p.to_string())
        .collect()
    }

    /// The quick-open ranking, tier by tier: a file whose *name* starts
    /// with the query first (shortest path first), then a name hit inside
    /// a word, then a subsequence, then a path that only matches in its
    /// directories — and never one that does not match at all.
    #[test]
    fn search_ranks_the_file_name_first() {
        let paths = corpus();
        let mut search = PathSearch::new(paths.clone());
        let named = |search: &mut PathSearch, query: &str| -> Vec<String> {
            search
                .rank(query, 10)
                .into_iter()
                .map(|ix| search.path(ix).expect("a ranked index is a path").to_owned())
                .collect::<Vec<_>>()
        };

        assert_eq!(
            named(&mut search, "diff"),
            [
                // A name that *starts* with the query, shortest path first.
                "src/app/diff.rs",
                "src/ui/diff_panel.rs",
                // Then a name that only carries it (and a shorter path
                // does not jump the tier).
                "src/xdiff.rs",
                "target/release/deps/libdiff_notes.rlib",
                // Last, the paths whose *name* says nothing: `mod.rs`,
                // `Notes.rs` and `notes.md` match only through their
                // directory, by length.
                "src/diff/mod.rs",
                "src/Diff/Notes.rs",
                "docs/diff/notes.md",
            ],
            "ranking for `diff`"
        );
        // A subsequence reaches into the name: `difpan`.
        assert_eq!(named(&mut search, "difpan"), ["src/ui/diff_panel.rs"]);
        // Case folds both ways.
        assert_eq!(named(&mut search, "DIFF_PANEL"), ["src/ui/diff_panel.rs"]);
        assert_eq!(named(&mut search, "file_tree"), ["docs/FILE_TREE.md"]);
        // Nothing for an empty query, and nothing for a miss.
        assert!(named(&mut search, "   ").is_empty());
        assert!(named(&mut search, "zzz").is_empty());
        // The cap is the cap.
        assert_eq!(search.rank("s", 2).len(), 2);
        // And a capped answer is exactly the uncapped ranking's prefix —
        // the best `limit` are partitioned to, not sorted apart from.
        for q in ["s", "d", "diff", "mod", "src", "zzz"] {
            let capped = search.rank(q, 2);
            let all = search.rank(q, 100);
            assert_eq!(capped, all[..capped.len()], "the cap keeps the ranking's head ({q})");
        }
    }

    /// The memo is an optimization, not a query language: typing a query
    /// one character at a time, backspacing over it, pasting a whole word
    /// at once and switching to a fresh word must all rank exactly as a
    /// search that starts from the whole list every time. The reference is
    /// a fresh index per query — the memo is the only difference.
    #[test]
    fn the_prefix_memo_ranks_like_a_search_from_scratch() {
        let paths = corpus();
        let reference = |query: &str| PathSearch::new(paths.clone()).rank(query, 200);

        // Typed forward, one keystroke at a time.
        let mut search = PathSearch::new(paths.clone());
        let mut typed = String::new();
        for ch in "libdiff_notes".chars() {
            typed.push(ch);
            assert_eq!(search.rank(&typed, 200), reference(&typed), "typing `{typed}`");
        }
        assert!(search.memo_levels() > 1, "typing is what the memo is for");

        // Backspaced over the same branch.
        for len in (1..typed.len()).rev() {
            let shorter = &typed[..len];
            assert_eq!(search.rank(shorter, 200), reference(shorter), "back to `{shorter}`");
        }

        // A different word, and a query that extends nothing memoized.
        for query in ["notes", "xdiff", "rs", "d", "zzz", "Diff", "  diff  "] {
            assert_eq!(search.rank(query, 200), reference(query), "query `{query}`");
        }

        // The same query twice: a level's own verdicts are the answer, and
        // they are still that query's verdicts.
        assert_eq!(search.rank("xdiff", 200), reference("xdiff"));
        assert_eq!(search.rank("xdiff", 200), reference("xdiff"));

        // A cleared field memoizes nothing, and the next query is a full
        // pass again.
        assert!(search.rank("", 200).is_empty());
        assert_eq!(search.memo_levels(), 0, "an empty query memoizes nothing");
        assert_eq!(search.rank("diff", 200), reference("diff"));

        // A new list drops the levels that named the old one.
        let mut search = PathSearch::new(paths.clone());
        search.rank("lib", 200);
        search.set_paths(vec!["other.rs".to_owned()]);
        assert_eq!(search.memo_levels(), 0);
        assert_eq!(search.rank("other", 200), [0], "ranked against the new list");
    }
}
