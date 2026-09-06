//! Git diff model for the right pane: types shared with the view plus
//! the [`git`] query layer (DESIGN.md §7).
//!
//! Scope: project-level HEAD→workdir diff (staged + unstaged +
//! untracked). Per-session worktree scoping comes later.

pub mod git;

/// Hard cap on collected diff lines per file (DESIGN §11); the stat
/// counts keep going so `+a/-b` stays truthful.
pub const MAX_LINES_PER_FILE: usize = 5000;

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
