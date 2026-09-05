//! Session domain model: projects and agent sessions.
//!
//! M1 holds static mock data. M2+ wires real PTY processes behind
//! [`AgentSession`] (see DESIGN.md §5, §6).

/// Lifecycle of one agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    Done(i32),
    Killed,
    Error(String),
}

impl AgentStatus {
    pub fn label(&self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Done(_) => "done",
            AgentStatus::Killed => "killed",
            AgentStatus::Error(_) => "error",
        }
    }
    pub fn is_running(&self) -> bool {
        matches!(self, AgentStatus::Running)
    }

}

/// One agent session inside a project.
#[derive(Debug, Clone)]
pub struct AgentSession {
    pub id: String,
    pub title: String,
    pub status: AgentStatus,
    /// Backend command snapshot (M2: used to spawn the PTY).
    pub cmd: String,
    /// Placeholder transcript lines (M2: replaced by the live grid).
    pub transcript: Vec<String>,
    /// Placeholder diff files for the right pane (M3: replaced by git2).
    pub diff_files: Vec<DiffFile>,
}

/// One file entry in the mock diff panel.
#[derive(Debug, Clone)]
pub struct DiffFile {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub hunks: Vec<DiffHunk>,
}

/// One hunk of a mock diff.
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

/// One line of a mock diff hunk.
#[derive(Debug, Clone)]
pub struct DiffLine {
    /// ' ', '+', or '-'.
    pub kind: char,
    pub old_no: Option<usize>,
    pub new_no: Option<usize>,
    pub text: String,
}

/// One project in the left pane.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub sessions: Vec<AgentSession>,
}

impl Project {
    pub fn running_count(&self) -> usize {
        self.sessions
            .iter()
            .filter(|s| s.status.is_running())
            .count()
    }
}

/// Static mock workspace for M1.
pub fn mock_projects() -> Vec<Project> {
    vec![
        Project {
            id: "ddu".to_string(),
            name: "ddu".to_string(),
            path: "~/Code/ddu".to_string(),
            sessions: vec![
                AgentSession {
                    id: "ddu-1".to_string(),
                    title: "implement terminal".to_string(),
                    status: AgentStatus::Running,
                    cmd: "claude".to_string(),
                    transcript: vec![
                        "$ claude".to_string(),
                        "Welcome to Claude Code!".to_string(),
                        "cwd: /Users/just/Code/ddu".to_string(),
                        "> implement the terminal view".to_string(),
                        "I'll start by reading DESIGN.md and the existing scaffold.".to_string(),
                        "Read src/main.rs, src/app.rs, DESIGN.md".to_string(),
                        "Added src/terminal.rs — PTY wrapper: spawn, resize, reader thread"
                            .to_string(),
                        "Added src/terminal_view.rs — grid renderer + focus handling"
                            .to_string(),
                        "cargo check passes, 0 warnings".to_string(),
                        "> also handle resize".to_string(),
                        "Terminal::resize now syncs the PTY size with the panel.".to_string(),
                        "Done. Diff ready for review — 2 files changed.".to_string(),
                    ],
                    diff_files: vec![
                        DiffFile {
                            path: "src/terminal.rs".to_string(),
                            added: 128,
                            removed: 12,
                            hunks: vec![DiffHunk {
                                header: "@@ -1,3 +1,4 @@".to_string(),
                                lines: vec![
                                    DiffLine {
                                        kind: ' ',
                                        old_no: Some(1),
                                        new_no: Some(1),
                                        text: "use gpui_kit::*;".to_string(),
                                    },
                                    DiffLine {
                                        kind: '-',
                                        old_no: Some(2),
                                        new_no: None,
                                        text: "use portable_pty::PtySize;".to_string(),
                                    },
                                    DiffLine {
                                        kind: '+',
                                        old_no: None,
                                        new_no: Some(2),
                                        text: "use portable_pty::{PtySize, MasterPty};".to_string(),
                                    },
                                    DiffLine {
                                        kind: '+',
                                        old_no: None,
                                        new_no: Some(3),
                                        text: "use alacritty_terminal::Term;".to_string(),
                                    },
                                    DiffLine {
                                        kind: ' ',
                                        old_no: Some(3),
                                        new_no: Some(4),
                                        text: "".to_string(),
                                    },
                                ],
                            }],
                        },
                        DiffFile {
                            path: "src/app.rs".to_string(),
                            added: 24,
                            removed: 3,
                            hunks: vec![DiffHunk {
                                header: "@@ -40,6 +40,9 @@".to_string(),
                                lines: vec![
                                    DiffLine {
                                        kind: ' ',
                                        old_no: Some(40),
                                        new_no: Some(40),
                                        text: "fn render_sidebar(&self) -> impl IntoElement {"
                                            .to_string(),
                                    },
                                    DiffLine {
                                        kind: '+',
                                        old_no: None,
                                        new_no: Some(41),
                                        text: "    // TODO: wire real sessions".to_string(),
                                    },
                                ],
                            }],
                        },
                    ],
                },
                AgentSession {
                    id: "ddu-4".to_string(),
                    title: "polish layout".to_string(),
                    status: AgentStatus::Done(0),
                    cmd: "claude".to_string(),
                    transcript: vec![
                        "$ claude".to_string(),
                        "Session complete in 18s.".to_string(),
                    ],
                    diff_files: vec![DiffFile {
                        path: "src/app.rs".to_string(),
                        added: 3,
                        removed: 0,
                        hunks: vec![DiffHunk {
                            header: "@@ -10,3 +10,6 @@".to_string(),
                            lines: vec![
                                DiffLine {
                                    kind: ' ',
                                    old_no: Some(10),
                                    new_no: Some(10),
                                    text: "fn render_center(&self) -> impl IntoElement {"
                                        .to_string(),
                                },
                                DiffLine {
                                    kind: '+',
                                    old_no: None,
                                    new_no: Some(11),
                                    text: "    // zed-style polish".to_string(),
                                },
                            ],
                        }],
                    }],
                },
                AgentSession {
                    id: "ddu-2".to_string(),
                    title: "design review".to_string(),
                    status: AgentStatus::Done(0),
                    cmd: "codex".to_string(),
                    transcript: vec![
                        "$ codex".to_string(),
                        "Session complete in 42s.".to_string(),
                    ],
                    diff_files: vec![],
                },
                AgentSession {
                    id: "ddu-3".to_string(),
                    title: "fix resizable".to_string(),
                    status: AgentStatus::Error("pty spawn failed".to_string()),
                    cmd: "claude".to_string(),
                    transcript: vec![
                        "$ claude".to_string(),
                        "Error: pty spawn failed".to_string(),
                    ],
                    diff_files: vec![],
                },
            ],
        },
        Project {
            id: "blog".to_string(),
            name: "blog".to_string(),
            path: "~/Code/blog".to_string(),
            sessions: vec![AgentSession {
                id: "blog-1".to_string(),
                title: "write post".to_string(),
                status: AgentStatus::Running,
                cmd: "claude".to_string(),
                transcript: vec![
                    "$ claude".to_string(),
                    "> draft the release post".to_string(),
                ],
                diff_files: vec![DiffFile {
                    path: "posts/release.md".to_string(),
                    added: 86,
                    removed: 0,
                    hunks: vec![DiffHunk {
                        header: "@@ -0,0 +1,5 @@".to_string(),
                        lines: vec![DiffLine {
                            kind: '+',
                            old_no: None,
                            new_no: Some(1),
                            text: "# Release notes".to_string(),
                        }],
                    }],
                }],
            }],
        },
    ]
}
