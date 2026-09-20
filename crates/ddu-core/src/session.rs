//! Session domain model: projects, agent sessions and the command
//! presets used to spawn them (docs/DESIGN.md §5, §9).

use gpui_kit::Entity;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ddu_terminal::{PtySpawn, TermSession};

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

    /// A session that has been running for `secs`, as of `now`.
    fn running_for(now: Instant, secs: u64) -> AgentSession {
        AgentSession {
            id: "s".into(),
            title: "bash".into(),
            status: AgentStatus::Running,
            cmd: AgentCmd {
                program: "bash".into(),
                args: vec![],
            },
            resume_id: None,
            was_live: false,
            kind: "terminal".into(),
            started: now - Duration::from_secs(secs),
            ended: None,
            term: None,
            cwd: PathBuf::new(),
            diff_selected: None,
            view_mode: None,
        }
    }

    /// The label a run of `secs` reads (frozen at that reading, so the
    /// assertion does not race the wall clock).
    fn label_of(secs: u64) -> String {
        let now = Instant::now();
        let mut s = running_for(now, 0);
        s.ended = Some(now + Duration::from_secs(secs));
        s.elapsed_label()
    }

    /// The boundary the tick sleeps to has to be the instant the label
    /// reads differently — short of it is a repaint for a reading that
    /// did not move, past it a row that lies about its run.
    #[test]
    fn a_label_boundary_is_the_labels_next_reading() {
        for secs in [
            0, 1, 59, 60, 119, 120, 121, 3_599, 3_600, 7_199, 7_200, 86_399, 86_400, 86_401,
        ] {
            let now = Instant::now();
            let wait = running_for(now, secs)
                .label_change_in(now)
                .expect("a running run has a boundary")
                .as_secs();
            let at = secs + wait;
            assert_ne!(
                label_of(secs),
                label_of(at),
                "a run at {secs}s reads the same {wait}s later"
            );
            assert_eq!(
                label_of(secs),
                label_of(at - 1),
                "and it already moved before that boundary"
            );
        }

        // A finished run keeps its last reading; nothing to schedule.
        let now = Instant::now();
        let mut done = running_for(now, 5);
        done.ended = Some(now);
        assert_eq!(done.label_change_in(now), None);
    }

    #[test]
    fn resume_spec_uses_agent_proper_flag() {
        let base = AgentCmd {
            program: "claude".into(),
            args: vec!["--foo".into()],
        };
        let spec = base.resume_spec(Path::new("/t"), "abc-123");
        assert_eq!(spec.program, "claude");
        assert_eq!(spec.args, vec!["--foo", "--resume", "abc-123"]);
        assert_eq!(spec.cwd, PathBuf::from("/t"));
        // The preset itself stays resume-free: Restart spawns `spec`.
        assert_eq!(base.args, vec!["--foo"]);
        assert_eq!(base.spec(Path::new("/t")).args, vec!["--foo"]);

        let codex = AgentCmd {
            program: "codex".into(),
            args: vec![],
        };
        let spec = codex.resume_spec(Path::new("/t"), "u-1");
        assert_eq!(spec.program, "codex");
        assert_eq!(spec.args, vec!["resume", "u-1"]);
    }
}

/// Backend command preset for spawning one agent. Never carries a
/// resume id: a row's conversation id lives on the session
/// ([`AgentSession::resume_id`]), so Restart always starts the preset
/// from scratch while Resume asks for [`AgentCmd::resume_spec`].
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

    /// Spawn spec that resumes agent session `id`: codex takes a bare
    /// `resume <id>` subcommand, claude and omp the `--resume <id>`
    /// flag. Codex accepts `--resume` only for `codex exec`, so the
    /// form follows the program name.
    pub fn resume_spec(&self, cwd: &Path, id: &str) -> PtySpawn {
        let mut spec = self.spec(cwd);
        if self.program.ends_with("codex") {
            spec.args.push("resume".into());
        } else {
            spec.args.push("--resume".into());
        }
        spec.args.push(id.into());
        spec
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
    /// The row's agent conversation id — captured from the run's output
    /// (startup banner / exit footer) or restored from state.json.
    /// Resume replays it (`--resume <id>`); Restart ignores it.
    pub resume_id: Option<String>,
    /// The PTY was still alive when the app began shutting down, so the
    /// next launch must bring this row back running. Set by the shell
    /// (`AppView`'s shutdown); `status.is_running()` covers
    /// every other moment (see `persist`).
    pub was_live: bool,
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
    /// Session-scoped diff state: the file this session's pane shows.
    /// The file *tree* is not here — it belongs to the project (one
    /// repository, one working tree), so only the selected path is a
    /// row's own.
    pub diff_selected: Option<String>,
    /// The row's persisted right-pane mode (`diff` / `file`),
    /// written on save and adopted on switch.
    pub view_mode: Option<String>,
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

    /// How long until [`Self::elapsed_label`] reads differently — what the
    /// ui tick sleeps, so a label that moves once a minute is not
    /// repainted once a second. `None` once the run has ended: its label
    /// is frozen and nothing has to be scheduled for it.
    ///
    /// Exact, and never zero: the label turns over at the unit boundary
    /// its own thresholds use (`1m` at the 2-minute mark, since a run
    /// under a minute already reads `1m`), and the tick that lands on it
    /// shows the same second the old per-second tick did.
    pub fn label_change_in(&self, now: Instant) -> Option<Duration> {
        if self.ended.is_some() {
            return None;
        }
        let secs = now.saturating_duration_since(self.started).as_secs();
        let next = if secs < 60 {
            // `1m` spans both sides of the 60 s mark: the first reading
            // that differs is the two-minute one.
            120
        } else if secs < 3600 {
            (secs / 60 + 1) * 60
        } else if secs < 86_400 {
            (secs / 3600 + 1) * 3600
        } else {
            (secs / 86_400 + 1) * 86_400
        };
        Some(Duration::from_secs(next - secs))
    }
}

/// One project in the left pane: one working tree, one repository, and
/// therefore one **file tree** — every session in it shares the tree's
/// expansion and layer height, and only the selected file is a session's
/// own.
#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub sessions: Vec<AgentSession>,
    /// Directories expanded in the file tree (the layer is lazy:
    /// everything else is listed on demand).
    pub tree_open: std::collections::HashSet<String>,
    /// The sidebar's diff-tree layer height (px).
    pub tree_height: Option<f32>,
    /// Where the file tree was left scrolled (the layer's own offset,
    /// negative y, px). The tree is the project's, so its place is too:
    /// another session of the same project reads the same rows, and a
    /// switch back to it comes back to the same line.
    pub tree_scroll: Option<f32>,
    /// Whether the default-expansion rule has run for this project: it
    /// opens the changes' ancestors once, and never overrides a
    /// directory the user has collapsed since.
    pub tree_seeded: bool,
}

impl Project {
    /// A project whose tree has nothing remembered about it yet.
    pub fn new(name: String, path: PathBuf) -> Self {
        Self {
            name,
            path,
            sessions: Vec::new(),
            tree_open: Default::default(),
            tree_height: None,
            tree_scroll: None,
            tree_seeded: false,
        }
    }
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
    vec![Project::new(name, cwd)]
}
