//! Whole-file view for the right pane's **File** mode: the file's own
//! lines with the diff's changes tinted in place and the deletions
//! spliced back in — a unified, context-complete reading of one file
//! (`docs/FILE_TREE.md` §4.2). No hunk-header bands: the rows are the
//! file in order, so a `@@` line would name a document that has no
//! hunks (Diff mode keeps them — that surface *is* hunks). A file whose
//! syntax the pane knows is highlighted (`language_of`), parsed once
//! here with the rows.
//!
//! The rows are materialized once per `(path, diff generation)` by the
//! pane and never rebuilt per frame; the pane virtualizes them, so a
//! 200k-line file costs one build pass and one visible slice per frame.
//!
//! Row shape: every workdir line exactly once with the deletions
//! spliced in — `rows.len() == file_lines + deleted_lines`, asserted in
//! the tests.
//!
//! Two honesty rules keep the merge safe against the 3 s poll window:
//! a context line is only trusted when the file on disk still agrees
//! with the diff that numbered it (otherwise the whole view degrades to
//! untinted context), and a capped diff only tints the prefix it knows
//! (`tints_capped`), so the rows are never spliced at the wrong place.

use std::path::Path;

use gpui_kit::base::input::Rope;
use gpui_kit::component::highlighter::SyntaxHighlighter;

use super::{cells, DiffFile, DiffLine};

/// Refuse to build a view for a file larger than this (the pane falls
/// back to the diff's hunks with a note).
pub const MAX_VIEW_BYTES: u64 = 8 * 1024 * 1024;
/// Second guard: row count, which also bounds the pane's size table.
pub const MAX_VIEW_LINES: usize = 200_000;
/// A NUL byte within this prefix means binary.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// Longest-line candidates remembered for the pane's width measurement
/// (one `usize` per hint, not a copy of the text).
const WIDTH_HINTS: usize = 32;

/// Why a file could not be shown as text. One policy, two surfaces: the
/// merged whole-file view and the rendered document's Markdown source
/// refuse for the
/// same reasons and band the same note.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unreadable {
    /// NUL byte in the first [`BINARY_SNIFF_BYTES`].
    Binary,
    /// Over [`MAX_VIEW_BYTES`], over [`MAX_VIEW_LINES`], or not a file.
    TooLarge,
    /// Not on disk (a deleted file: its content lives in the diff).
    Missing,
}

impl Unreadable {
    /// The one-line band the pane puts above the rows it falls back to.
    pub fn note(self) -> &'static str {
        match self {
            Unreadable::Binary => "Binary file — showing the diff.",
            Unreadable::TooLarge => "Too large to view — showing the diff.",
            Unreadable::Missing => "No longer on disk — showing the diff.",
        }
    }
}

/// The rendered document's built source, with the `(path, diff
/// generation)` it belongs
/// to (same keying as `file_view_key`/`file_view`).
pub struct PreviewBuild {
    pub key: (String, u64),
    pub source: Result<std::rc::Rc<str>, Unreadable>,
}

/// The File-mode view of one selected file.
pub enum FileView {
    Text(TextFileView),
    /// An image, decoded and drawn by the pane ([`is_image`]).
    Image,
    /// NUL byte in the first [`BINARY_SNIFF_BYTES`].
    Binary,
    /// Over [`MAX_VIEW_BYTES`] or [`MAX_VIEW_LINES`].
    TooLarge,
    /// Not on disk (a deleted file: its content lives in the diff).
    Missing,
}

impl FileView {
    /// The refusal behind this view, for the pane's note.
    pub fn refusal(&self) -> Option<Unreadable> {
        match self {
            FileView::Text(_) | FileView::Image => None,
            FileView::Binary => Some(Unreadable::Binary),
            FileView::TooLarge => Some(Unreadable::TooLarge),
            FileView::Missing => Some(Unreadable::Missing),
        }
    }
}

/// Whether a path is an image the pane draws instead of reading: the
/// formats gpui's own image loader decodes (SVG included). Anything else
/// binary goes through the reading policy like any other file.
pub fn is_image(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".ico", ".svg",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// One row of the whole-file stream: a line of the file, or a line the
/// diff deleted, spliced above the line that replaced it. The line
/// reuses [`DiffLine`] — same numbers, same `' ' | '+' | '-'` meaning —
/// so the pane renders File mode with the Diff mode row code.
pub struct ViewRow {
    pub line: DiffLine,
    /// Byte offset of `line.text` in the file, when the row is *in* the
    /// file (context and `+` rows). `None` for a deletion: its text is
    /// no longer there to highlight.
    pub offset: Option<u32>,
}

/// A whole file, ready to render.
pub struct TextFileView {
    pub rows: Vec<ViewRow>,
    /// Highest line number any row carries (the pane's single gutter:
    /// the line this row has in the file as it is now).
    pub max_line_no: u32,
    /// The diff that tinted this view was capped, or was too stale to
    /// trust: rows past the cap (or all of them) carry no tint.
    pub tints_capped: bool,
    /// Row indices of the longest lines, by display-cell estimate.
    pub width_hints: Vec<usize>,
    /// The file's parsed syntax, when its extension names a grammar the
    /// build carries: rows highlight off it. Parsed here, once, on the
    /// same background pass as the rows — a render never parses.
    pub highlighter: Option<SyntaxHighlighter>,
}

/// Files past this size are listed but not parsed: highlighting is a
/// reading aid, and a multi-megabyte source would hold the background
/// build (and so the pane) for a parse nobody asked to wait for.
const MAX_HIGHLIGHT_BYTES: usize = 1 << 20;

impl FileView {
    /// Read `path` (relative to the project root) and merge `diff` into
    /// it. `diff` is the poll's file record — `None` (or one that no
    /// longer lines up with the file) still yields a complete view, just
    /// without tints.
    pub fn build(root: &Path, path: &str, diff: Option<&DiffFile>) -> FileView {
        // An image is content, not a file to read: it has no lines to
        // number and no tint to merge — the pane draws it (the size caps
        // below are about *text*, and a 4 MB PNG has three lines' worth
        // of nothing to show).
        if is_image(path) {
            return match std::fs::metadata(root.join(path)) {
                Ok(meta) if meta.is_file() => FileView::Image,
                _ => FileView::Missing,
            };
        }
        let text = match read_text(root, path) {
            Ok(text) => text,
            Err(Unreadable::Binary) => return FileView::Binary,
            Err(Unreadable::TooLarge) => return FileView::TooLarge,
            Err(Unreadable::Missing) => return FileView::Missing,
        };
        let (lines, starts) = split_lines(&text);
        if lines.len() > MAX_VIEW_LINES {
            return FileView::TooLarge;
        }

        let (rows, tints_capped) = match diff {
            Some(diff) => match merge(&lines, &starts, diff) {
                Some(rows) => (rows, diff.truncated),
                // The file moved under the poll: show it untinted rather
                // than splice the changes into the wrong lines.
                None => (context_only(&lines, &starts), true),
            },
            // No diff record at all: the file is *clean* (the tree lists
            // every file, so this is the normal case for an unchanged
            // one). Nothing is missing — claiming the tints were capped
            // would band "Loading the file's changes…" over a file that
            // has none, and the pane's scroll-driven budget grower would
            // chase a note it can never clear.
            None => (context_only(&lines, &starts), false),
        };
        FileView::Text(TextFileView::new(rows, &lines, tints_capped, path, &text))
    }
}

impl TextFileView {
    fn new(
        rows: Vec<ViewRow>,
        lines: &[&str],
        tints_capped: bool,
        path: &str,
        text: &str,
    ) -> Self {
        let max_line_no = lines.len().max(1) as u32;
        // Rank by display cells once, here: the pane re-measures a
        // handful of candidates per frame, not every row.
        let mut hints: Vec<(usize, usize)> = Vec::with_capacity(WIDTH_HINTS + 1);
        for (ix, row) in rows.iter().enumerate() {
            let cells = cells(row.line.text.as_str());
            if hints.len() < WIDTH_HINTS {
                hints.push((cells, ix));
                hints.sort_unstable();
            } else if let Some(first) = hints.first_mut() {
                if cells > first.0 {
                    *first = (cells, ix);
                    hints.sort_unstable();
                }
            }
        }
        let mut width_hints: Vec<usize> = hints.into_iter().map(|(_, ix)| ix).collect();
        width_hints.sort_unstable();
        let highlighter = (text.len() <= MAX_HIGHLIGHT_BYTES)
            .then(|| language_of(path))
            .flatten()
            .map(|language| {
                let mut highlighter = SyntaxHighlighter::new(language);
                // A full parse, not an incremental edit: the view is
                // built once per (path, diff generation), off the UI
                // thread, and a half-parsed tree highlights nothing.
                highlighter.update(None, &Rope::from_str(text), None);
                highlighter
            });
        Self {
            rows,
            max_line_no,
            tints_capped,
            width_hints,
            highlighter,
        }
    }
}

/// The grammar a path's extension names, if the pane can highlight it.
/// Names are the ones `LanguageRegistry` registers (gpui-component's
/// `Language::from_name` also accepts them), so this table only decides
/// *which* grammar a file gets — never how it is parsed.
fn language_of(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "c" => "c",
        // Headers are the C++ grammar's territory: it parses C headers
        // too, where the C grammar chokes on a class or a namespace.
        "h" | "hh" | "hpp" | "hxx" | "cc" | "cpp" | "cxx" | "c++" => "cpp",
        "java" => "java",
        "html" | "htm" => "html",
        "css" | "scss" => "css",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" | "pyw" => "python",
        "swift" => "swift",
        "sh" | "bash" | "zsh" => "bash",
        "json" | "jsonc" => "json",
        _ => return None,
    })
}



/// A Markdown file's source for its rendered document (off the UI
/// thread), or why
/// it cannot be shown — the same refusals File mode bands.
pub fn read_source(root: &Path, path: &str) -> Result<String, Unreadable> {
    read_text(root, path)
}

/// Read `path` for display, or say why it cannot be shown. The one place
/// the size, binary and on-disk policy lives: the merged view and the
/// Markdown source both come through here, so the two modes can never
/// disagree about what is viewable.
pub fn read_text(root: &Path, path: &str) -> Result<String, Unreadable> {
    let Ok(meta) = std::fs::metadata(root.join(path)) else {
        return Err(Unreadable::Missing);
    };
    if !meta.is_file() || meta.len() > MAX_VIEW_BYTES {
        return Err(Unreadable::TooLarge);
    }
    let Ok(bytes) = std::fs::read(root.join(path)) else {
        return Err(Unreadable::Missing);
    };
    if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        return Err(Unreadable::Binary);
    }
    // Lossy: a stray invalid byte must not cost the whole view.
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// File lines for display, and the byte each starts at in the file — the
/// offset a row hands the highlighter. `\n` splits, a trailing `\r` is
/// dropped from the *text* (so CRLF files do not render a stray glyph)
/// while staying in the offsets, and a trailing newline does not invent an
/// empty last line.
fn split_lines(text: &str) -> (Vec<&str>, Vec<u32>) {
    let mut lines: Vec<(&str, u32)> = Vec::new();
    let mut at = 0u32;
    for raw in text.split('\n') {
        lines.push((raw.strip_suffix('\r').unwrap_or(raw), at));
        at += raw.len() as u32 + 1;
    }
    if lines.last().is_some_and(|(line, _)| line.is_empty()) {
        lines.pop();
    }
    lines.into_iter().unzip()
}

/// Every line as untinted context, numbered 1..=n.
fn context_only(lines: &[&str], starts: &[u32]) -> Vec<ViewRow> {
    lines
        .iter()
        .enumerate()
        .map(|(ix, text)| context(text, ix as u32 + 1, ix as u32 + 1, starts[ix]))
        .collect()
}

/// Splice the diff's hunks into the file's lines. `None` means the two
/// disagree — a context line's number or text is not what the diff
/// claims — which the caller answers with untinted context.
///
/// Deletions are buffered and flushed immediately before the next
/// surviving line, so a `-` run reads directly above the line that
/// replaced it; a run that ends the file (or the hunk) flushes at the
/// end.
fn merge(lines: &[&str], starts: &[u32], diff: &DiffFile) -> Option<Vec<ViewRow>> {
    let mut rows: Vec<ViewRow> = Vec::with_capacity(lines.len() + diff.removed);
    // 0-based workdir cursor and its 1-based old-file twin; both resync
    // from every hunk line the diff numbers.
    let mut next_new = 0usize;
    let mut next_old = 1u32;
    let mut pending: Vec<ViewRow> = Vec::new();

    for hunk in &diff.hunks {
        let anchor = hunk
            .lines
            .iter()
            .find(|l| l.kind != '-')
            .and_then(|l| l.new_no)
            .unwrap_or(0);
        // The untouched lines between the previous hunk and this one.
        if anchor > 0 {
            while next_new < anchor as usize - 1 {
                let text = lines.get(next_new)?;
                rows.push(context(text, next_old, next_new as u32 + 1, starts[next_new]));
                next_new += 1;
                next_old += 1;
            }
        }
        for line in &hunk.lines {
            match line.kind {
                '-' => {
                    next_old = line.old_no.unwrap_or(next_old).max(next_old);
                    // A deletion is not in the file: no offset, so no
                    // highlight — its text is the diff's, not the file's.
                    pending.push(ViewRow {
                        line: line.clone(),
                        offset: None,
                    });
                }
                kind => {
                    let text = lines.get(next_new)?;
                    if line.text != *text {
                        return None;
                    }
                    if let Some(new_no) = line.new_no {
                        if new_no as usize != next_new + 1 {
                            return None;
                        }
                    }
                    rows.append(&mut pending);
                    rows.push(ViewRow {
                        line: DiffLine {
                            kind,
                            old_no: if kind == '+' {
                                None
                            } else {
                                line.old_no.or(Some(next_old))
                            },
                            new_no: Some(next_new as u32 + 1),
                            text: (*text).to_string(),
                        },
                        offset: Some(starts[next_new]),
                    });
                    if kind == ' ' {
                        next_old = line.old_no.unwrap_or(next_old) + 1;
                    }
                    next_new += 1;
                }
            }
        }
    }

    // Everything the last hunk did not cover, then a deletion run that
    // ended the file.
    while next_new < lines.len() {
        rows.push(context(
            lines[next_new],
            next_old,
            next_new as u32 + 1,
            starts[next_new],
        ));
        next_new += 1;
        next_old += 1;
    }
    rows.append(&mut pending);
    Some(rows)
}

fn context(text: &str, old_no: u32, new_no: u32, offset: u32) -> ViewRow {
    ViewRow {
        line: DiffLine {
            kind: ' ',
            old_no: Some(old_no),
            new_no: Some(new_no),
            text: text.to_string(),
        },
        offset: Some(offset),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffHunk, DiffLine};

    fn line(kind: char, old: Option<u32>, new: Option<u32>, text: &str) -> DiffLine {
        DiffLine {
            kind,
            old_no: old,
            new_no: new,
            text: text.to_string(),
        }
    }

    /// `merge` with the byte offsets of a fresh file: the tests build
    /// rows from `&[&str]` and never care where the bytes are.
    fn merged(lines: &[&str], diff: &DiffFile) -> Option<Vec<ViewRow>> {
        let starts: Vec<u32> = split_lines(&lines.join("\n")).1;
        merge(lines, &starts, diff)
    }

    fn diff_file(hunks: Vec<Vec<DiffLine>>) -> DiffFile {
        let removed = hunks
            .iter()
            .flatten()
            .filter(|l| l.kind == '-')
            .count();
        DiffFile {
            path: "f.txt".into(),
            removed,
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

    /// A hunk replacing line 2 of a 4-line file: the merged stream is
    /// the file in order, with the deletion above its replacement and
    /// the hunk header band on top.
    #[test]
    fn merge_places_deletion_above_its_replacement() {
        // The file already holds the changed content — that is what a
        // diff means — so line 2 reads `TWO` while the hunk still
        // carries the removed `two`.
        let lines = ["one", "TWO", "three", "four"];
        let diff = diff_file(vec![vec![
            line(' ', Some(1), Some(1), "one"),
            line('-', Some(2), None, "two"),
            line('+', None, Some(2), "TWO"),
            line(' ', Some(3), Some(3), "three"),
        ]]);
        let rows = merged(&lines, &diff).expect("merge");
        let texts: Vec<&str> = rows
            .iter()
            .map(|r| r.line.text.as_str())
            .collect();
        assert_eq!(texts, ["one", "two", "TWO", "three", "four"]);
        let kinds: Vec<char> = rows
            .iter()
            .filter_map(|r| Some(r.line.kind))
            .collect();
        assert_eq!(kinds, [' ', '-', '+', ' ', ' ']);
        // Untouched lines past the hunk keep climbing both counters.
        let last = &rows.last().unwrap().line;
        assert_eq!((last.old_no, last.new_no), (Some(4), Some(4)));
    }

    /// A deletion run that ends the file has no following line to
    /// anchor on: it must still land in the stream, at the end.
    #[test]
    fn merge_anchors_a_trailing_deletion_at_eof() {
        let lines = ["one"];
        let diff = diff_file(vec![vec![
            line(' ', Some(1), Some(1), "one"),
            line('-', Some(2), None, "gone"),
        ]]);
        let rows = merged(&lines, &diff).expect("merge");
        let last = rows.last().expect("rows");
        let l = &last.line;
        assert_eq!(l.kind, '-');
        assert_eq!(l.text, "gone");
        assert_eq!(l.new_no, None);
    }

    /// Insertion-only hunk: the added lines sit between the anchors and
    /// every workdir line still appears exactly once.
    #[test]
    fn merge_handles_insertion_only_hunks() {
        let lines = ["new", "a", "b"];
        let diff = diff_file(vec![vec![
            line('+', None, Some(1), "new"),
            line(' ', Some(1), Some(2), "a"),
            line(' ', Some(2), Some(3), "b"),
        ]]);
        let rows = merged(&lines, &diff).expect("merge");
        let texts: Vec<&str> = rows.iter().map(|r| r.line.text.as_str()).collect();
        assert_eq!(texts, ["new", "a", "b"]);
    }

    /// The invariant the whole design rests on: every workdir line
    /// appears exactly once, and the row count is file lines plus the
    /// deletions the diff carries.
    #[test]
    fn merge_keeps_every_line_once() {
        // HEAD: one, two, three, four, five, six. Workdir: one,
        // TWO-THREE, four, five — so the second hunk's `five` is old 5
        // / new 4 and the trailing deletion ends the file.
        let lines = ["one", "TWO-THREE", "four", "five"];
        let diff = diff_file(vec![
            vec![
                line(' ', Some(1), Some(1), "one"),
                line('-', Some(2), None, "two"),
                line('-', Some(3), None, "three"),
                line('+', None, Some(2), "TWO-THREE"),
            ],
            vec![
                line(' ', Some(5), Some(4), "five"),
                line('-', Some(6), None, "six"),
            ],
        ]);
        let rows = merged(&lines, &diff).expect("merge");
        let numbered: Vec<u32> = rows.iter().filter_map(|r| r.line.new_no).collect();
        assert_eq!(numbered, vec![1, 2, 3, 4]);
        let deleted = rows
            .iter()
            .filter(|r| r.line.kind == '-')
            .count();
        assert_eq!(deleted, 3);
        let view = TextFileView::new(rows, &lines, false, "f.txt", "");
        // The invariant: every workdir line appears once and every
        // deletion is spliced in — a hunk header would be a row with no
        // line behind it, and this surface has none.
        assert_eq!(view.rows.len(), lines.len() + deleted);
        assert!(view.rows.iter().all(|r| r.offset.is_none() || r.line.kind != '-'));
        assert_eq!(view.max_line_no, lines.len() as u32);
    }

    /// A stale diff (the file changed under the poll) must not splice:
    /// `merge` refuses and the caller renders untinted context.
    #[test]
    fn merge_refuses_a_diff_that_no_longer_matches_the_file() {
        // The diff numbers line 2 as `two`; the workdir says otherwise,
        // so nothing about this file's changes can be trusted.
        let lines = ["one", "CHANGED", "three"];
        let diff = diff_file(vec![vec![
            line(' ', Some(1), Some(1), "one"),
            line(' ', Some(2), Some(2), "two"),
        ]]);
        assert!(merged(&lines, &diff).is_none());
    }

    /// A capped diff still tints its prefix (its hunks are complete up
    /// to the cap and spatially correct) — the view flags it instead of
    /// discarding the tints.
    #[test]
    fn capped_diff_keeps_prefix_tints() {
        let lines = ["one", "two", "three", "four"];
        let mut diff = diff_file(vec![vec![
            line('+', None, Some(1), "one"),
            line(' ', Some(1), Some(2), "two"),
        ]]);
        diff.truncated = true;
        let rows = merged(&lines, &diff).expect("merge");
        let view = TextFileView::new(rows, &lines, diff.truncated, "f.txt", "");
        assert!(view.tints_capped);
        // Every workdir line, in order, and no header band.
        assert_eq!(view.rows.len(), lines.len());
        assert_eq!(view.max_line_no, lines.len() as u32);
    }

    /// Every row that is in the file knows where it is: `offset` is the
    /// byte its own text starts at, which is what the pane hands the
    /// highlighter. A spliced deletion has no offset — it is not in the
    /// file to point at.
    #[test]
    fn rows_carry_their_byte_offset_in_the_file() {
        let dir = std::env::temp_dir().join(format!("ddu-offsets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // CRLF on purpose: a stripped `\r` must not shift a later row.
        // The file already holds the changed content — that is what a
        // diff means: line 2 reads `BETA` while the hunk carries `beta`.
        std::fs::write(dir.join("f.txt"), "alpha\r\nBETA\r\ngamma\ndelta\n").unwrap();
        let diff = diff_file(vec![vec![
            line(' ', Some(1), Some(1), "alpha"),
            line('-', Some(2), None, "beta"),
            line('+', None, Some(2), "BETA"),
            line(' ', Some(3), Some(3), "gamma"),
        ]]);
        let FileView::Text(view) = FileView::build(&dir, "f.txt", Some(&diff)) else {
            panic!("text view");
        };
        let text = std::fs::read_to_string(dir.join("f.txt")).unwrap();
        let mut deletions = 0;
        for row in &view.rows {
            let Some(at) = row.offset else {
                assert_eq!(row.line.kind, '-', "only a deletion lacks an offset");
                deletions += 1;
                continue;
            };
            let at = at as usize;
            assert_eq!(
                &text[at..at + row.line.text.len()],
                row.line.text,
                "row offset points at the row's own text"
            );
        }
        assert_eq!(deletions, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file whose extension names a grammar is parsed with its rows, and
    /// a row's styles come back **relative to that row** — the pane colors
    /// one line without knowing where the file it came from starts. A
    /// grammar-less file (or a row that is not in the file) yields none.
    #[test]
    fn a_known_language_highlights_one_row_at_a_time() {
        use crate::diff::RowStream;
        use gpui_kit::component::highlighter::HighlightTheme;

        let dir = std::env::temp_dir().join(format!("ddu-highlight-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {\n    let x = 1;\n}\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "fn main() {\n    let x = 1;\n}\n").unwrap();

        let FileView::Text(view) = FileView::build(&dir, "main.rs", None) else {
            panic!("text view");
        };
        assert!(view.highlighter.is_some(), "a .rs file parses");
        let stream = RowStream::view(&view);
        let theme = HighlightTheme::default_light();
        let theme = theme.as_ref();
        let body = &view.rows[1].line.text;
        let styles = stream.row_styles(1, theme);
        assert!(!styles.is_empty(), "the row carries styles");
        for (range, _) in &styles {
            assert!(
                range.end <= body.len(),
                "a style stays inside its own row: {range:?} of {body:?}"
            );
        }
        // Line 0 is `fn main() {`: its styles are relative to *it*, so the
        // same keyword sits at a different offset in each row.
        assert!(styles.iter().any(|(range, _)| range.start < body.len()));
        assert!(!stream.row_styles(0, theme).is_empty(), "the first row too");

        let FileView::Text(plain) = FileView::build(&dir, "notes.txt", None) else {
            panic!("text view");
        };
        assert!(plain.highlighter.is_none(), "no grammar, no parse");
        assert!(RowStream::view(&plain).row_styles(0, theme).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_lines_drops_the_trailing_newline_and_cr() {
        assert_eq!(split_lines("a\nb\n").0, vec!["a", "b"]);
        assert_eq!(split_lines("a\r\nb").0, vec!["a", "b"]);
        // The offsets keep the bytes the text dropped: `b` starts at 3.
        assert_eq!(split_lines("a\r\nb").1, vec![0, 3]);
        assert_eq!(split_lines("").0, Vec::<&str>::new());
        assert_eq!(split_lines("\n").0, vec![""]);
        // An empty file has one empty line, not a phantom one.
        assert_eq!(split_lines("").1, Vec::<u32>::new());
    }

    /// The row contract at depth: a deep row is reachable by index and
    /// the search's index points back at the row that holds the text —
    /// what the pane's virtual list and `scroll_to_item` ride on, at a
    /// scale (20k rows) where a re-walk per access would show.
    #[test]
    fn deep_rows_are_indexed_and_searchable() {
        let dir = std::env::temp_dir().join(format!("ddu-view-deep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut text = String::new();
        for i in 0..20_000 {
            if i == 19_999 {
                text.push_str("LINE 19999 CHANGED\n");
            } else {
                text.push_str(&format!("line {i:05}\n"));
            }
        }
        std::fs::write(dir.join("deep.txt"), &text).unwrap();
        let diff = DiffFile {
            path: "deep.txt".into(),
            added: 1,
            removed: 1,
            hunks: vec![DiffHunk {
                header: "@@ -19999,2 +19999,2 @@".into(),
                lines: vec![
                    line(' ', Some(19999), Some(19999), "line 19998"),
                    line('-', Some(20000), None, "line 19999"),
                    line('+', None, Some(20000), "LINE 19999 CHANGED"),
                ],
            }],
            lines_total: 3,
            truncated: false,
        };
        let FileView::Text(view) = FileView::build(&dir, "deep.txt", Some(&diff)) else {
            panic!("text view");
        };
        let stream = crate::diff::RowStream::view(&view);
        assert_eq!(view.max_line_no, 20_000);
        assert_eq!(stream.rows(), 20_000 + 1);
        // Deep random access: the last row is the file's last line.
        let last = stream.row(stream.rows() - 1).expect("row");
        assert!(matches!(last, crate::diff::PaneRow::Line(l) if l.text == "LINE 19999 CHANGED"));
        // Search lands on a deep index, and that index is the row.
        let hits = stream.match_indices("LINE 19999 CHANGED");
        assert_eq!(hits.len(), 1);
        assert!(hits[0] > 19_000, "deep hit, not a prefix walk: {}", hits[0]);
        assert!(
            matches!(stream.row(hits[0]), Some(crate::diff::PaneRow::Line(l)) if l.kind == '+')
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The three fallbacks and the read path, against real files: a
    /// deleted file, a binary one, one over the cap, and a normal one
    /// merged with its diff.
    #[test]
    fn build_reads_merges_and_falls_back() {
        let dir = std::env::temp_dir().join(format!("ddu-view-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Missing: nothing on disk for this path.
        assert!(matches!(
            FileView::build(&dir, "gone.txt", None),
            FileView::Missing
        ));

        // Binary: a NUL in the sniff window.
        std::fs::write(dir.join("bin.dat"), b"text\0more").unwrap();
        assert!(matches!(
            FileView::build(&dir, "bin.dat", None),
            FileView::Binary
        ));

        // TooLarge: past the byte cap without reading the content.
        std::fs::write(
            dir.join("huge.txt"),
            vec![b'x'; (MAX_VIEW_BYTES + 1) as usize],
        )
        .unwrap();
        assert!(matches!(
            FileView::build(&dir, "huge.txt", None),
            FileView::TooLarge
        ));

        // Text: the file's own lines, tinted where the diff says so.
        std::fs::write(dir.join("f.txt"), "one\nTWO\nthree\n").unwrap();
        let diff = diff_file(vec![vec![
            line(' ', Some(1), Some(1), "one"),
            line('-', Some(2), None, "two"),
            line('+', None, Some(2), "TWO"),
            line(' ', Some(3), Some(3), "three"),
        ]]);
        let FileView::Text(view) = FileView::build(&dir, "f.txt", Some(&diff)) else {
            panic!("text view");
        };
        assert_eq!(view.max_line_no, 3);
        assert!(!view.tints_capped);
        assert!(!view.width_hints.is_empty());
        let texts: Vec<&str> = view.rows.iter().map(|r| r.line.text.as_str()).collect();
        assert_eq!(texts, ["one", "two", "TWO", "three"]);
        let kinds: Vec<char> = view
            .rows
            .iter()
            .filter_map(|r| Some(r.line.kind))
            .collect();
        assert_eq!(kinds, [' ', '-', '+', ' ']);

        // A clean file: complete, untinted, and — unlike a stale diff —
        // with nothing to say about tints (the pane has no note to band).
        let FileView::Text(clean) = FileView::build(&dir, "f.txt", None) else {
            panic!("text view");
        };
        assert!(!clean.tints_capped, "a clean file has no tints to cap");
        assert!(crate::diff::RowStream::view(&clean).note().is_none());

        // A diff that no longer matches the file: complete, untinted.
        std::fs::write(dir.join("f.txt"), "one\nEDITED\nthree\n").unwrap();
        let FileView::Text(view) = FileView::build(&dir, "f.txt", Some(&diff)) else {
            panic!("text view");
        };
        assert!(view.tints_capped);
        assert_eq!(view.rows.len(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The rendered document reads through the same policy as File mode: it refuses a
    /// file the merged view refuses, for the same reason, so the pane
    /// bands one note whichever mode asked.
    #[test]
    fn read_source_refuses_what_the_view_refuses() {
        let dir = std::env::temp_dir().join(format!("ddu-view-src-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(read_source(&dir, "gone.md"), Err(Unreadable::Missing));

        // Binary is a refusal here too: a NUL byte is not Markdown.
        std::fs::write(dir.join("bin.md"), b"# t\0x").unwrap();
        assert_eq!(read_source(&dir, "bin.md"), Err(Unreadable::Binary));

        // A directory is not a file.
        std::fs::create_dir_all(dir.join("sub.md")).unwrap();
        assert_eq!(read_source(&dir, "sub.md"), Err(Unreadable::TooLarge));

        // The shared policy, asserted across both surfaces: whatever
        // the merged view refuses, the source refuses identically.
        for path in ["gone.md", "bin.md", "sub.md"] {
            assert_eq!(
                FileView::build(&dir, path, None).refusal(),
                read_source(&dir, path).err(),
                "{path} must refuse the same way in both modes"
            );
        }

        std::fs::write(dir.join("ok.md"), "# Title\n\ntext\n").unwrap();
        assert_eq!(
            read_source(&dir, "ok.md").expect("readable"),
            "# Title\n\ntext\n"
        );
        assert!(matches!(
            FileView::build(&dir, "ok.md", None),
            FileView::Text(_)
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
