//! Persistence, split in two files with one responsibility each:
//!
//! * `settings.json` — [`Config`]: one-to-one user settings (default
//!   launcher, shell, agent args, custom agents, font, theme).
//! * `state.json` — [`State`]: runtime workspace snapshot (project
//!   list with expand states, panel visibility).
//!
//! Both are plain data with `Default`. Loads report problems instead of
//! failing the launch: a corrupt file is backed up beside itself and
//! defaults are used; saves return the error for the caller to surface.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::session::AgentCmd;

/// `~/Library/Application Support/ddu/` on macOS, `~/.config/ddu/`
/// elsewhere. Env override `DDU_STATE_PATH` still names the state file
/// directly (launcher/tests); `DDU_SETTINGS_PATH` does the same for
/// settings.
fn data_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    if cfg!(target_os = "macos") {
        PathBuf::from(home).join("Library/Application Support/ddu")
    } else {
        PathBuf::from(home).join(".config/ddu")
    }
}

pub fn settings_path() -> PathBuf {
    std::env::var_os("DDU_SETTINGS_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir().join("settings.json"))
}

pub fn state_path() -> PathBuf {
    std::env::var_os("DDU_STATE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir().join("state.json"))
}

// ── settings.json ─────────────────────────────────────────────────────

/// One user-defined agent launcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPreset {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellConfig {
    pub program: String,
    #[serde(default)]
    pub args: String,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            program: std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
            args: String::new(),
        }
    }
}

/// What the sidebar `+` button creates by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSessionDefault {
    /// `"terminal"`, a builtin name (`claude`/`codex`/`omp`) or a custom
    /// agent name.
    pub kind: String,
}

impl Default for NewSessionDefault {
    fn default() -> Self {
        Self {
            kind: "terminal".into(),
        }
    }
}

/// Root of `settings.json`: user configuration, nothing else.
impl gpui_kit::Global for Config {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub new_session: NewSessionDefault,
    #[serde(default)]
    pub shell: ShellConfig,
    /// Extra args for builtin agents, keyed by program name.
    #[serde(default)]
    pub agent_args: BTreeMap<String, String>,
    #[serde(default)]
    pub custom_agents: Vec<AgentPreset>,
    /// Terminal font family; `None` = system default mono face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font: Option<String>,
    #[serde(default)]
    pub dark_theme: bool,
}

// ── state.json ────────────────────────────────────────────────────────

/// Persisted project entry: the full session list survives a restart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default = "default_true")]
    pub expanded: bool,
    /// The project's session rows, in sidebar order. Agents carry a
    /// resume id so the conversation can be picked up; shell rows
    /// respawn fresh.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SavedSession>,
}

/// One restored session row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedSession {
    /// Launcher kind (`terminal`/`claude`/`codex`/`omp`/custom name).
    pub kind: String,
    /// Row title (the live OSC title for agents, program name for shells).
    pub title: String,
    /// Agent session id for `--resume`; `None` for shell rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// The file this session had selected in the diff tree (path;
    /// re-pinned to an index on load).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_file: Option<String>,
    /// Directories collapsed in this session's diff tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed_dirs: Vec<String>,
}

fn default_true() -> bool {
    true
}

/// Root of `state.json`: runtime workspace snapshot, not configuration.
impl gpui_kit::Global for State {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub hidden_sessions: bool,
    /// Sidebar width (px), clamped by the panel min/max on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar_width: Option<f32>,
    /// Diff panel width (px), clamped on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff_width: Option<f32>,
    /// Last active project (index into `projects`).
    #[serde(default)]
    pub current_project: usize,
    /// Last active session (index into the current project's rows).
    /// Restored after restart; clamped to the rows that actually exist.
    #[serde(default)]
    pub current_session: usize,
    /// Last diff-panel visibility (⌘R; a tree click opens it too).
    #[serde(default)]
    pub show_diff: bool,
    /// Last diff file-tree layer visibility in the sidebar (⌘T).
    #[serde(default)]
    pub show_diff_tree: bool,
    /// Per-project right-pane state: the diff selection, tree collapse
    /// state and tree/content split height are project-local concerns —
    /// they mean nothing once the working tree changes or the project
    /// changes.
    #[serde(default)]
    pub project_state: BTreeMap<String, ProjectState>,
}

/// Per-project runtime state for the diff pane and related viewers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectState {
    /// Height of the tree pane above the content pane (px).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_height: Option<f32>,
}

// ── load / save ───────────────────────────────────────────────────────

/// Load settings + state plus any warnings worth showing the user.
pub fn load_all() -> (Config, State, Vec<String>) {
    let mut warnings = Vec::new();

    let cfg_path = settings_path();
    let config = match read_json(&cfg_path) {
        Ok(c) => c,
        Err(e) => {
            warnings.push(e);
            Config::default()
        }
    };
    let state = match read_json(&state_path()) {
        Ok(s) => s,
        Err(e) => {
            warnings.push(e);
            State::default()
        }
    };
    (config, state, warnings)
}

impl Config {
    pub fn save(&self) -> Result<(), String> {
        write_json(&settings_path(), self)
    }

    /// Menu entries for the `...` button and the settings page:
    /// `(label, kind-key, builtin?)`.
    pub fn agent_menu(&self) -> Vec<String> {
        let mut items = vec!["terminal".to_string()];
        items.extend(BUILTIN_AGENTS.iter().map(|(_, p)| p.to_string()));
        items.extend(self.custom_agents.iter().map(|a| a.name.clone()));
        items
    }

    /// Resolve a kind key to a spawnable command.
    pub fn cmd_for(&self, kind: &str) -> anyhow::Result<AgentCmd> {
        let (program, args) = if kind == "terminal" {
            (self.shell.program.as_str(), self.shell.args.as_str())
        } else if BUILTIN_AGENTS.iter().any(|(_, p)| *p == kind) {
            (
                kind,
                self.agent_args.get(kind).map(String::as_str).unwrap_or(""),
            )
        } else {
            let agent = self
                .custom_agents
                .iter()
                .find(|a| a.name == kind)
                .ok_or_else(|| anyhow::anyhow!("Unknown session type: {kind}"))?;
            (agent.program.as_str(), agent.args.as_str())
        };
        anyhow::ensure!(
            !program.trim().is_empty(),
            "Set a program for {kind} in Settings."
        );
        let args = shlex::split(args).ok_or_else(|| {
            anyhow::anyhow!("Unclosed quote in arguments for {kind}. Check Settings.")
        })?;
        Ok(AgentCmd {
            program: program.trim().into(),
            args,
            resume: None,
        })
    }

    /// Human label for a kind key.
    pub fn label_for(&self, kind: &str) -> String {
        match kind {
            "terminal" => "Terminal".into(),
            other => BUILTIN_AGENTS
                .iter()
                .find(|(_, p)| *p == other)
                .map(|(l, _)| l.to_string())
                .or_else(|| {
                    self.custom_agents
                        .iter()
                        .find(|a| a.name == other)
                        .map(|a| a.name.clone())
                })
                .unwrap_or_else(|| other.to_string()),
        }
    }
}

impl State {
    pub fn save(&self) -> Result<(), String> {
        write_json(&state_path(), self)
    }
}

/// Surface a persistence error on stderr. There may be no window at
/// save time (e.g. while re-opening from the dock), so a GUI toast is
/// not a reliable channel; stderr is the durable one.
pub fn report_error(err: String, _cx: &mut gpui_kit::App) {
    eprintln!("[ddu] {err}");
}

/// Env-gated debug trace (`DDU_DEBUG=1` → /tmp/ddu-debug.log): the
/// resume-capture chain spans async events and a quit, where stderr
/// from a bundled launch is lost. Cheap file append, off by default.
pub fn debug_log(msg: &str) {
    if std::env::var_os("DDU_DEBUG").is_none() {
        return;
    }
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/ddu-debug.log")
    {
        let _ = writeln!(f, "{msg}");
    }
}

/// Read and parse a JSON value. A missing file is a normal first run and
/// yields `Default`; an unreadable or unparseable file is backed up
/// (timestamped `*.corrupt-<secs>` beside it) so nothing is silently
/// lost, and the error is returned.
fn read_json<T>(path: &Path) -> Result<T, String>
where
    T: DeserializeOwned + Default,
{
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(format!("Could not read {}: {e}", path.display())),
    };
    match serde_json::from_str::<T>(&text) {
        Ok(value) => Ok(value),
        Err(e) => {
            // `path.with_extension` would replace the real extension
            // (settings.json → settings.corrupt-…json), so append to
            // the full file name instead: settings.json.corrupt-….
            let mut file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            file_name.push_str(&format!(".corrupt-{}", unix_secs()));
            let backup = path.with_file_name(file_name);
            match std::fs::rename(path, &backup) {
                Ok(()) => Err(format!(
                    "{}: {e} — invalid file backed up to {}; defaults loaded",
                    path.display(),
                    backup.display()
                )),
                Err(re) => Err(format!(
                    "{}: {e} — invalid file (and backing it up failed: {re}); defaults loaded",
                    path.display()
                )),
            }
        }
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Could not serialize {}: {e}", path.display()))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
    }
    // Atomic: write a sibling temp, then rename over the target, so a
    // crash mid-write can never leave a truncated file behind.
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, json)
        .map_err(|e| format!("Could not write {}: {e}", temporary.display()))?;
    std::fs::rename(&temporary, path).map_err(|e| {
        let _ = std::fs::remove_file(&temporary);
        format!("Could not save {}: {e}", path.display())
    })
}

fn unix_secs() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u128)
        .unwrap_or(0)
}

// ── builtins ──────────────────────────────────────────────────────────

/// The three builtin agent launchers, in menu order.
pub const BUILTIN_AGENTS: &[(&str, &str)] = &[
    ("Claude", "claude"),
    ("Codex", "codex"),
    ("Oh My Pi", "omp"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_arguments_keep_spaces_and_empty_values() {
        let mut cfg = Config::default();
        cfg.shell.args = "--name 'two words' \"\" path\\ with\\ spaces".into();
        assert_eq!(
            cfg.cmd_for("terminal").unwrap().args,
            ["--name", "two words", "", "path with spaces"]
        );
        cfg.shell.args = "'unfinished".into();
        assert!(cfg.cmd_for("terminal").is_err());
    }

    #[test]
    fn older_settings_load_with_light_theme() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.dark_theme);
        assert_eq!(cfg.new_session.kind, "terminal");
    }

    #[test]
    fn corrupt_file_is_backed_up_and_defaulted() {
        let dir = std::env::temp_dir().join(format!("ddu-cfg-{}", unix_secs()));
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        let result = read_json::<Config>(&path);
        assert!(result.is_err());
        // Original renamed aside, defaults returned, backup carries the content.
        assert!(!path.exists());
        let backup_name = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("settings.json.corrupt-"));
        assert!(backup_name.is_some(), "expected a .corrupt backup sibling");
        assert_eq!(
            std::fs::read_to_string(dir.join(backup_name.unwrap())).unwrap(),
            "{ not json"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_are_clean_defaults() {
        let dir = std::env::temp_dir().join(format!("ddu-cfg-miss-{}", unix_secs()));
        let state_path = dir.join("state.json");
        let settings_path = dir.join("settings.json");
        let _ = std::fs::create_dir_all(&dir);
        unsafe {
            std::env::set_var("DDU_STATE_PATH", state_path.to_str().unwrap());
            std::env::set_var("DDU_SETTINGS_PATH", settings_path.to_str().unwrap());
        }
        let (config, state, warnings) = load_all();
        unsafe {
            std::env::remove_var("DDU_STATE_PATH");
            std::env::remove_var("DDU_SETTINGS_PATH");
        }
        assert_eq!(config, Config::default());
        assert_eq!(state, State::default());
        assert!(warnings.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
