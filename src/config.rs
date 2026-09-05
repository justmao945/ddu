//! Persistent app configuration (`state.json`): projects, new-session
//! default, terminal shell, agent arg overrides and custom agents.
//!
//! Everything is plain data with `Default` so a missing/corrupt file
//! falls back to sane built-ins instead of failing the launch.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::session::AgentCmd;

/// Where the state file lives: `~/Library/Application Support/ddu/` on
/// macOS, `~/.config/ddu/` elsewhere.
pub fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    if cfg!(target_os = "macos") {
        PathBuf::from(home)
            .join("Library/Application Support/ddu/state.json")
    } else {
        PathBuf::from(home).join(".config/ddu/state.json")
    }
}

/// One user-defined agent launcher.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentPreset {
    pub name: String,
    pub program: String,
    #[serde(default)]
    pub args: String,
}

impl AgentPreset {
    fn cmd(&self) -> AgentCmd {
        AgentCmd {
            program: self.program.clone(),
            args: self.args.split_whitespace().map(str::to_string).collect(),
        }
    }
}

/// Root of `state.json`.
impl gpui_kit::Global for Config {}

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
        Self { kind: "terminal".into() }
    }
}

/// Persisted project entry (sessions themselves are ephemeral).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub path: PathBuf,
    #[serde(default = "default_true")]
    pub expanded: bool,
}

fn default_true() -> bool {
    true
}

/// Root of `state.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
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
}

/// The three builtin agent launchers, in menu order.
pub const BUILTIN_AGENTS: &[(&str, &str)] = &[("Claude", "claude"), ("Codex", "codex"), ("Oh My Pi", "omp")];

impl Config {
    pub fn load() -> Self {
        let path = state_path();
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = state_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, json);
        }
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
    pub fn cmd_for(&self, kind: &str) -> Option<AgentCmd> {
        match kind {
            "terminal" => Some(AgentCmd {
                program: self.shell.program.clone(),
                args: self.shell.args.split_whitespace().map(str::to_string).collect(),
            }),
            builtin if BUILTIN_AGENTS.iter().any(|(_, p)| *p == builtin) => {
                let args = self
                    .agent_args
                    .get(builtin)
                    .map(|a| a.split_whitespace().map(str::to_string).collect())
                    .unwrap_or_default();
                Some(AgentCmd { program: builtin.into(), args })
            }
            custom => self
                .custom_agents
                .iter()
                .find(|a| a.name == custom)
                .map(|a| a.cmd()),
        }
    }

    /// Human label for a kind key.
    pub fn label_for(&self, kind: &str) -> String {
        match kind {
            "terminal" => "Terminal".into(),
            other => BUILTIN_AGENTS
                .iter()
                .find(|(_, p)| *p == other)
                .map(|(l, _)| l.to_string())
                .or_else(|| self.custom_agents.iter().find(|a| a.name == other).map(|a| a.name.clone()))
                .unwrap_or_else(|| other.to_string()),
        }
    }
}
