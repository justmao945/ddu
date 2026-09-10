//! Session domain model: projects, agent sessions and the command
//! presets used to spawn them (docs/DESIGN.md §5, §9).

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

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn resume_spec_uses_agent_proper_flag() {
        let base = AgentCmd {
            program: "claude".into(),
            args: vec!["--foo".into()],
            resume: None,
        };
        let spec = base
            .clone()
            .with_resume("abc-123".into())
            .spec(Path::new("/t"));
        assert_eq!(spec.program, "claude");
        assert_eq!(spec.args, vec!["--foo", "--resume", "abc-123"]);
        assert_eq!(spec.cwd, PathBuf::from("/t"));

        let codex = AgentCmd {
            program: "codex".into(),
            args: vec![],
            resume: None,
        };
        let spec = codex.with_resume("u-1".into()).spec(Path::new("/t"));
        assert_eq!(spec.program, "codex");
        assert_eq!(spec.args, vec!["resume", "u-1"]);
    }
}

/// Backend command preset for spawning one agent.
#[derive(Debug, Clone)]
pub struct AgentCmd {
    pub program: String,
    pub args: Vec<String>,
    /// Session id to resume (`--resume <id>` / `codex resume <id>`).
    pub resume: Option<String>,
}

impl AgentCmd {
    /// Copy with a resume id attached; `spec` then appends the right
    /// flag form (`--resume` for claude/omp, bare `resume <id>` for
    /// codex's subcommand style).
    pub fn with_resume(mut self, id: String) -> Self {
        self.resume = Some(id);
        self
    }

    /// Human-readable command line for the session list.
    pub fn label(&self) -> String {
        let base = if self.args.is_empty() {
            self.program.clone()
        } else {
            format!("{} {}", self.program, self.args.join(" "))
        };
        match &self.resume {
            Some(id) => format!("{base} (resume {id})"),
            None => base,
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
        let mut args = self.args.clone();
        if let Some(id) = &self.resume {
            // codex takes a bare `resume <id>` subcommand; claude and
            // omp use `--resume <id>`. Codex still accepts `--resume`
            // only for `codex exec`, so branch on the program name.
            if self.program.ends_with("codex") {
                args.push("resume".into());
                args.push(id.clone());
            } else {
                args.push("--resume".into());
                args.push(id.clone());
            }
        }
        PtySpawn {
            program: self.program.clone(),
            args,
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
    /// The working tree this session runs in (project root today; a
    /// per-session worktree later). The diff poll targets this.
    pub cwd: PathBuf,
    /// Session-scoped diff tree state: with per-session worktrees each
    /// session's changes, selection and collapsed dirs are its own.
    pub diff_selected: Option<String>,
    pub diff_closed: std::collections::HashSet<String>,
    /// This session's sidebar splitter height (diff tree layer, px) —
    /// per-session layout, restored when the session is selected.
    pub diff_tree_height: Option<f32>,
}

impl AgentSession {
    /// Shell sessions are plain terminals; everything else is an agent.
    pub fn is_agent(&self) -> bool {
        self.kind != "terminal"
    }

    /// Time the current (or last) run has taken, in `m`/`h`/`d` units.
    /// Anything under a minute shows as `1m` (the label is a coarse
    /// human read-out, not a timer).
    pub fn elapsed_label(&self) -> String {
        let end = self.ended.unwrap_or_else(Instant::now);
        let secs = end.saturating_duration_since(self.started).as_secs();
        if secs < 60 {
            "1m".into()
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
/// from (per docs/DESIGN.md §10 the full project list becomes persistent state
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
