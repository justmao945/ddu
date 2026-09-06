//! Session domain model: projects, agent sessions and the command
//! presets used to spawn them (DESIGN.md §5, §9).

use gpui_kit::Entity;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::terminal::{PtySpawn, TermSession};

/// Lifecycle of one agent session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    Running,
    Done(i32),
    Error(String),
}

impl AgentStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, AgentStatus::Running)
    }
}

/// Backend command preset for spawning one agent.
#[derive(Debug, Clone)]
pub struct AgentCmd {
    pub program: String,
    pub args: Vec<String>,
}

impl AgentCmd {
    /// Human-readable command line for the session list.
    pub fn label(&self) -> String {
        if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        }
    }

    /// Basename of the program (`/bin/zsh` → `zsh`); the sidebar title.
    pub fn basename(&self) -> String {
        self.program
            .rsplit('/')
            .next()
            .unwrap_or(&self.program)
            .to_string()
    }

    /// Build the PTY spawn spec with `cwd` as the working directory.
    pub fn spec(&self, cwd: &Path) -> PtySpawn {
        PtySpawn {
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: cwd.into(),
        }
    }
}

/// One agent session inside a project.
#[derive(Debug, Clone)]
pub struct AgentSession {
    /// Stable id, source of element ids.
    pub id: String,
    /// Row title; agents override it live via the PTY's OSC title.
    pub title: String,
    pub status: AgentStatus,
    pub cmd: AgentCmd,
    /// Which launcher created this (`terminal`/builtin/custom name).
    pub kind: String,
    /// When the current process was spawned (reset on restart).
    pub started: Instant,
    /// When the process exited, for the final duration reading.
    pub ended: Option<Instant>,
    /// Live terminal; `None` while the process failed to spawn.
    pub term: Option<Entity<TermSession>>,
}

impl AgentSession {
    /// Shell sessions are plain terminals; everything else is an agent.
    pub fn is_agent(&self) -> bool {
        self.kind != "terminal"
    }

    /// Time the current (or last) run has taken, in `m`/`h`/`d` units.
    pub fn elapsed_label(&self) -> String {
        let end = self.ended.unwrap_or_else(Instant::now);
        let secs = end.saturating_duration_since(self.started).as_secs();
        if secs < 60 {
            "<1m".into()
        } else if secs < 3600 {
            format!("{}m", secs / 60)
        } else if secs < 86_400 {
            format!("{}h", secs / 3600)
        } else {
            format!("{}d", secs / 86_400)
        }
    }
}

/// One project in the left pane.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub sessions: Vec<AgentSession>,
}

/// The workspace the app starts with: the directory it was launched
/// from (per DESIGN §10 the full project list becomes persistent state
/// in a later milestone).
pub fn initial_projects() -> Vec<Project> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let name = cwd
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_string_lossy().into_owned());
    vec![Project {
        name,
        path: cwd,
        sessions: vec![],
    }]
}
