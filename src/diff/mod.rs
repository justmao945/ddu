//! Git diff model for the right pane: types shared with the view plus
//! the [`git`] query layer (docs/DESIGN.md §7).
//!
//! Scope: project-level HEAD→workdir diff (staged + unstaged +
//! untracked). Per-session worktree scoping comes later.

pub mod git;

/// Hard cap on search matches per file; keeps the highlight set and the
/// counter cheap even for queries like `e` in a maxed-out file.
pub const SEARCH_MAX_MATCHES: usize = 500;

/// Child-row indices (per [`DiffFile::rows`]) of every diff line whose
/// text contains `query`, case-insensitively. A line with several
/// occurrences contributes its index once per occurrence, so the result
/// is ordered but not deduplicated — the counter counts occurrences
/// while the highlight set dedupes by row. Empty query → no matches.
pub fn match_rows(file: &DiffFile, query: &str) -> Vec<usize> {
    let needle = query.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let needle = needle.to_lowercase();
    let mut rows = Vec::new();
    'rows: for (ix, row) in file.rows().enumerate() {
        let DiffRow::Line(line) = row else { continue };
        let haystack = line.text.to_lowercase();
        for _ in haystack.match_indices(&needle) {
            rows.push(ix);
            if rows.len() >= SEARCH_MAX_MATCHES {
                break 'rows;
            }
        }
    }
    rows
}

/// Initial cap on collected diff lines per file (docs/DESIGN.md §11); the
/// stat counts keep going so `+a/-b` stays truthful. A truncated file's
/// budget grows (×4, up to [`EXPAND_MAX_LINES`]) when the pane's scroll
/// reaches the cap note — the virtualized list keeps large diffs cheap.
pub const MAX_LINES_PER_FILE: usize = 5000;

/// Hard ceiling for the scroll-driven budget growth: past this the
/// truncation note stays and no more lines load.
pub const EXPAND_MAX_LINES: usize = 200_000;

/// One changed file with its hunks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiffFile {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
    /// Total lines collected (for the truncation notice).
    pub lines_total: usize,
    pub truncated: bool,
}

/// One hunk of a file diff.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// One line of a diff hunk: `+` added, `-` removed, ` ` context.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffLine {
    pub kind: char,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    pub text: String,
}

/// Full working-tree diff of one project.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GitDiff {
    /// Current branch shorthand, `None` on detached HEAD / non-repo.
    pub branch: Option<String>,
    pub files: Vec<DiffFile>,
}

impl GitDiff {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// One row of the diff pane's row stream: the `@@` header band or a
/// diff line. The pane renders rows in exactly this order (see
/// `ui::diff_panel`), so the index [`DiffFile::rows`] yields for a row
/// IS the row's child index in the pane's scroll container.
pub enum DiffRow<'a> {
    Header(&'a str),
    Line(&'a DiffLine),
}

impl DiffFile {
    /// Rows in render order: per hunk, its header then its lines. The
    /// empty-file and truncation notices the pane appends render after
    /// all hunk rows, so they never shift the indices this yields.
    pub fn rows(&self) -> impl Iterator<Item = DiffRow<'_>> + '_ {
        self.hunks.iter().flat_map(|h| {
            let header = std::iter::once(DiffRow::Header(&h.header));
            let lines = h.lines.iter().map(DiffRow::Line);
            header.chain(lines)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: char, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: None,
            new_no: None,
            text: text.to_string(),
        }
    }

    fn file(hunks: Vec<Vec<DiffLine>>) -> DiffFile {
        DiffFile {
            hunks: hunks
                .into_iter()
                .map(|lines| DiffHunk {
                    header: "@@ test @@".into(),
                    lines,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn match_rows_empty_query_matches_nothing() {
        let f = file(vec![vec![line(' ', "hello world")]]);
        assert!(match_rows(&f, "").is_empty());
        assert!(match_rows(&f, "   ").is_empty());
    }

    #[test]
    fn match_rows_is_case_insensitive() {
        let f = file(vec![vec![line('+', "Hello WORLD")]]);
        assert_eq!(match_rows(&f, "hello"), vec![1]);
        assert_eq!(match_rows(&f, "world"), vec![1]);
    }

    #[test]
    fn match_rows_indices_track_rows_walk() {
        // Two hunks: row 0 = header, rows 1..=3 = lines, row 4 = next
        // header, rows 5..=6 = lines. Hunk headers never match.
        let f = file(vec![
            vec![
                line(' ', "fn main() {"),
                line('+', "    println!(\"find me\");"),
                line(' ', "}"),
            ],
            vec![line('-', "find me twice find me"), line(' ', "tail")],
        ]);
        // Line rows per the rows() walk: 1,2,3 then 5,6.
        assert_eq!(match_rows(&f, "find me"), vec![2, 5, 5]);
    }

    #[test]
    fn match_rows_caps_at_max() {
        let f = file(vec![(0..SEARCH_MAX_MATCHES + 64)
            .map(|_| line(' ', "needle"))
            .collect()]);
        assert_eq!(match_rows(&f, "needle").len(), SEARCH_MAX_MATCHES);
    }
}
