//! Git diff model for the right pane: the types shared with the view,
//! the pane's row stream, plus the [`git`] query layer
//! (docs/DESIGN.md §7).
//!
//! Scope: project-level HEAD→workdir diff (staged + unstaged +
//! untracked). Per-session worktree scoping comes later.

pub mod git;
pub mod tree;
pub mod view;

/// Hard cap on search matches per file; keeps the highlight set and the
/// counter cheap even for queries like `e` in a maxed-out file.
pub const SEARCH_MAX_MATCHES: usize = 500;

/// Display-cell estimate of a line (non-ASCII counts double) — the cheap
/// ranking the pane's width measurement and a whole-file view's
/// candidate list both use.
pub fn cells(text: &str) -> usize {
    text.chars()
        .fold(0usize, |n, c| n + if c.is_ascii() { 1 } else { 2 })
}

/// Occurrences of `needle` (already lowercased) in `hay`, without
/// allocating for the common all-ASCII line.
fn count_matches(hay: &str, needle: &str) -> usize {
    let n = needle.len();
    if n == 0 || hay.len() < n {
        return 0;
    }
    if hay.is_ascii() && needle.is_ascii() {
        let (hb, nb) = (hay.as_bytes(), needle.as_bytes());
        return (0..=hb.len() - n)
            .filter(|&i| hb[i..i + n].eq_ignore_ascii_case(nb))
            .count();
    }
    hay.to_lowercase().match_indices(needle).count()
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
    /// Captured for the model (and asserted in `git.rs` tests); the UI
    /// does not display it yet.
    pub branch: Option<String>,
    pub files: Vec<DiffFile>,
}

/// One poll's view of the project: the diff and the **full** working-tree
/// listing, read together so the tree's badges and the pane's rows can
/// never disagree about which files changed (`docs/FILE_TREE.md` §4.1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub diff: GitDiff,
    pub tree: tree::FileTree,
}

/// One row of a pane row stream.
pub enum PaneRow<'a> {
    /// An `@@ …` hunk band.
    Header(&'a str),
    /// A numbered line (`' '` context, `'+'` added, `'-'` removed).
    Line(&'a DiffLine),
    /// The single-line note a stream appends last.
    Note,
}

/// Which note [`RowStream`] appends as its final row, if any.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NoteKind {
    /// No hunks at all: binary, empty, or a metadata-only change.
    NoTextChanges,
    /// The diff's line budget capped this file (Diff mode).
    DiffCapped,
    /// The file itself is empty (File mode).
    EmptyFile,
    /// The view's tints stop at the diff cap, or are missing entirely
    /// because the file no longer matches the poll (File mode).
    TintsCapped,
}

/// Random-access row stream for the right pane: a file's diff hunks
/// (Diff mode) or the merged whole-file view (File mode, [`view`]).
///
/// Both modes share one contract: a row's index **is** its item index in
/// the pane's virtual list *and* its index in the find bar's match list —
/// what `scroll_to_item`, the highlight set and the drag selection ride
/// on. The pane renders only the visible slice of either.
pub struct RowStream<'a> {
    src: RowSrc<'a>,
    /// Row index of each hunk header, plus the total: `row` is then a
    /// binary search rather than a walk, so a deep scroll never re-walks
    /// the rows above it.
    hunk_rows: Vec<usize>,
    note: Option<NoteKind>,
}

enum RowSrc<'a> {
    Diff(&'a DiffFile),
    View(&'a crate::diff::view::TextFileView),
}

impl<'a> RowStream<'a> {
    /// The diff's hunks, plus a cap note when the budget truncated them.
    pub fn diff(file: &'a DiffFile) -> Self {
        let mut hunk_rows = Vec::with_capacity(file.hunks.len() + 1);
        let mut next = 0;
        for hunk in &file.hunks {
            hunk_rows.push(next);
            next += hunk.lines.len() + 1;
        }
        hunk_rows.push(next);
        let note = if file.hunks.is_empty() {
            Some(NoteKind::NoTextChanges)
        } else if file.truncated {
            Some(NoteKind::DiffCapped)
        } else {
            None
        };
        Self {
            src: RowSrc::Diff(file),
            hunk_rows,
            note,
        }
    }

    /// A whole-file view: its rows, plus a note when the tints stop
    /// short or the file has no lines at all.
    pub fn view(v: &'a crate::diff::view::TextFileView) -> Self {
        let note = if v.rows.is_empty() {
            Some(NoteKind::EmptyFile)
        } else if v.tints_capped {
            Some(NoteKind::TintsCapped)
        } else {
            None
        };
        Self {
            src: RowSrc::View(v),
            hunk_rows: Vec::new(),
            note,
        }
    }

    /// Rows the note (if any) would come after.
    pub fn rows(&self) -> usize {
        match &self.src {
            RowSrc::Diff(file) => file.hunks.iter().map(|h| h.lines.len() + 1).sum(),
            RowSrc::View(v) => v.rows.len(),
        }
    }

    /// Row count the pane's virtual list sees, note included.
    pub fn len(&self) -> usize {
        self.rows() + usize::from(self.note.is_some())
    }

    pub fn note(&self) -> Option<NoteKind> {
        self.note
    }

    /// The note's row index, when the stream appends one.
    pub fn note_ix(&self) -> Option<usize> {
        self.note.map(|_| self.rows())
    }

    pub fn row(&self, ix: usize) -> Option<PaneRow<'a>> {
        if ix >= self.rows() {
            return self.note.map(|_| PaneRow::Note);
        }
        match &self.src {
            RowSrc::Diff(file) => {
                let hunk = self.hunk_rows.partition_point(|start| *start <= ix) - 1;
                let offset = ix - self.hunk_rows[hunk];
                let hunk = &file.hunks[hunk];
                if offset == 0 {
                    Some(PaneRow::Header(&hunk.header))
                } else {
                    Some(PaneRow::Line(&hunk.lines[offset - 1]))
                }
            }
            RowSrc::View(v) => match &v.rows[ix] {
                crate::diff::view::ViewRow::Header(header) => Some(PaneRow::Header(header)),
                crate::diff::view::ViewRow::Line(line) => Some(PaneRow::Line(line)),
            },
        }
    }

    /// Indices of every line whose text contains `query`,
    /// case-insensitively. A line with several occurrences contributes
    /// its index once per occurrence, so the result is ordered but not
    /// deduplicated — the counter counts occurrences while the highlight
    /// set dedupes by row. Empty query → no matches.
    pub fn match_indices(&self, query: &str) -> Vec<usize> {
        let needle = query.trim().to_lowercase();
        let mut hits = Vec::new();
        if needle.is_empty() {
            return hits;
        }
        for ix in 0..self.rows() {
            let Some(PaneRow::Line(line)) = self.row(ix) else {
                continue;
            };
            for _ in 0..count_matches(&line.text, &needle) {
                hits.push(ix);
                if hits.len() >= SEARCH_MAX_MATCHES {
                    return hits;
                }
            }
        }
        hits
    }

    /// Highest line number in the stream (the pane's gutter width).
    pub fn max_line_no(&self) -> u32 {
        match &self.src {
            RowSrc::Diff(file) => file
                .hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter_map(|l| l.old_no.max(l.new_no))
                .max()
                .unwrap_or(0),
            RowSrc::View(v) => v.max_line_no,
        }
    }

    /// Longest-line candidates `(text, is_header)` for the pane's width
    /// measurement. The view answers from the handful it ranked when it
    /// was built; a diff walks its rows (it is capped, so that is cheap).
    pub fn width_candidates(&self) -> Vec<(&'a str, bool)> {
        match &self.src {
            RowSrc::Diff(file) => file
                .hunks
                .iter()
                .flat_map(|hunk| {
                    std::iter::once((hunk.header.as_str(), true))
                        .chain(hunk.lines.iter().map(|l| (l.text.as_str(), false)))
                })
                .collect(),
            RowSrc::View(v) => v
                .width_hints
                .iter()
                .filter_map(|ix| v.rows.get(*ix))
                .map(|row| match row {
                    crate::diff::view::ViewRow::Header(h) => (h.as_str(), true),
                    crate::diff::view::ViewRow::Line(l) => (l.text.as_str(), false),
                })
                .collect(),
        }
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
        assert!(RowStream::diff(&f).match_indices("").is_empty());
        assert!(RowStream::diff(&f).match_indices("   ").is_empty());
    }

    #[test]
    fn match_rows_is_case_insensitive() {
        let f = file(vec![vec![line('+', "Hello WORLD")]]);
        assert_eq!(RowStream::diff(&f).match_indices("hello"), vec![1]);
        assert_eq!(RowStream::diff(&f).match_indices("world"), vec![1]);
        // Non-ASCII text takes the allocating path and still matches.
        let f = file(vec![vec![line('+', "Grüße WELT")]]);
        assert_eq!(RowStream::diff(&f).match_indices("grüße"), vec![1]);
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
        assert_eq!(
            RowStream::diff(&f).match_indices("find me"),
            vec![2, 5, 5]
        );
    }

    #[test]
    fn match_rows_caps_at_max() {
        let f = file(vec![(0..SEARCH_MAX_MATCHES + 64)
            .map(|_| line(' ', "needle"))
            .collect()]);
        assert_eq!(
            RowStream::diff(&f).match_indices("needle").len(),
            SEARCH_MAX_MATCHES
        );
    }

    /// The stream's index contract: `row(ix)` is the row the pane renders
    /// at item index `ix`, headers included, and the note (when the
    /// stream appends one) is the last row.
    #[test]
    fn diff_stream_rows_are_indexed_in_render_order() {
        let f = file(vec![
            vec![line(' ', "a"), line('+', "b")],
            vec![line('-', "c")],
        ]);
        let stream = RowStream::diff(&f);
        // Two hunks, each a header plus its lines.
        assert_eq!(stream.rows(), (1 + 2) + (1 + 1));
        assert_eq!(stream.len(), stream.rows());
        assert!(matches!(stream.row(0), Some(PaneRow::Header("@@ test @@"))));
        assert!(matches!(stream.row(1), Some(PaneRow::Line(l)) if l.text == "a"));
        assert!(matches!(stream.row(2), Some(PaneRow::Line(l)) if l.text == "b"));
        assert!(matches!(stream.row(3), Some(PaneRow::Header(_))));
        assert!(matches!(stream.row(4), Some(PaneRow::Line(l)) if l.text == "c"));
        assert!(stream.row(5).is_none());

        let mut capped = f.clone();
        capped.truncated = true;
        let stream = RowStream::diff(&capped);
        assert_eq!(stream.note(), Some(NoteKind::DiffCapped));
        assert_eq!(stream.note_ix(), Some(stream.rows()));
        assert_eq!(stream.len(), stream.rows() + 1);
        assert!(matches!(stream.row(stream.rows()), Some(PaneRow::Note)));

        let empty = file(vec![]);
        let stream = RowStream::diff(&empty);
        assert_eq!(stream.note(), Some(NoteKind::NoTextChanges));
        assert_eq!(stream.len(), 1);
    }

    /// A whole-file view shares that contract: every match index the
    /// search returns points at the row that holds the text.
    #[test]
    fn view_stream_matches_are_row_indices() {
        let dir = std::env::temp_dir().join(format!("ddu-rowstream-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f.txt"), "alpha\nbeta\ngamma\n").unwrap();
        let crate::diff::view::FileView::Text(view) =
            crate::diff::view::FileView::build(&dir, "f.txt", None)
        else {
            panic!("text view");
        };
        let stream = RowStream::view(&view);
        assert_eq!(stream.rows(), 3);
        assert!(stream.note().is_some() || !view.tints_capped);
        let hits = stream.match_indices("beta");
        assert_eq!(hits, vec![1]);
        assert!(matches!(stream.row(1), Some(PaneRow::Line(l)) if l.text == "beta"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
