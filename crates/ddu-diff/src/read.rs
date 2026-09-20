//! One read policy for every text surface: the merged whole-file view and
//! the rendered document's Markdown source come through here, so the two can
//! never disagree about what is viewable. Binary (a NUL in the first 8 KiB),
//! oversized (bytes or lines) and gone-from-disk are refused with the reason
//! the pane bands; everything else comes back as the file's own text.

use std::path::Path;

/// Refuse to build a view for a file larger than this (the pane falls
/// back to the diff's hunks with a note).
pub const MAX_VIEW_BYTES: u64 = 8 * 1024 * 1024;
/// Second guard: row count, which also bounds the pane's size table.
pub const MAX_VIEW_LINES: usize = 200_000;
/// A NUL byte within this prefix means binary.
pub const BINARY_SNIFF_BYTES: usize = 8 * 1024;
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
