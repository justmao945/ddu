//! Session domain model: projects, agent sessions and the command
//! presets used to spawn them (DESIGN.md §5, §9).

use std::path::{Path, PathBuf};
use gpui_kit::{App, Entity};


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

    pub fn label(&self) -> String {
        match self {
            AgentStatus::Running => "running".into(),
            AgentStatus::Done(0) => "done".into(),
            AgentStatus::Done(code) => format!("exit {code}"),
            AgentStatus::Error(err) => format!("error: {err}"),
        }
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

    /// Build the PTY spawn spec with `cwd` as the working directory.
    pub fn spec(&self, cwd: &Path) -> PtySpawn {
        PtySpawn { program: self.program.clone(), args: self.args.clone(), cwd: cwd.into() }
    }

}

/// One agent session inside a project.
#[derive(Debug, Clone)]
pub struct AgentSession {
    /// Stable id, source of element ids.
    pub id: String,
    pub title: String,
    pub status: AgentStatus,
    pub cmd: AgentCmd,
    /// Which launcher created this (`terminal`/builtin/custom name).
    pub kind: String,
    /// Live terminal; `None` while the process failed to spawn.
    pub term: Option<Entity<TermSession>>,
}

impl AgentSession {
    /// Sidebar sub-label: the launcher kind, resolved for display.
    pub fn kind_label(&self, cx: &App) -> String {
        cx.global::<crate::config::Config>().label_for(&self.kind)
    }

    /// Raw kind key without an `App` handle (search filtering).
    pub fn kind_label_inner(&self) -> String {
        self.kind.clone()
    }
}


/// One project in the left pane.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub sessions: Vec<AgentSession>,
}

impl Project {
    pub fn running_count(&self) -> usize {
        self.sessions.iter().filter(|s| s.status.is_running()).count()
    }
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
    vec![Project { name, path: cwd, sessions: vec![] }]
}
