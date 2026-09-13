//! Persistence, split in two files with one responsibility each:
//!
//! * `settings.json` — [`Config`]: one-to-one user settings (default
//!   launcher, shell, terminal font, theme).
//! * `state.json` — [`State`]: runtime workspace snapshot (project
//!   list with expand states, panel visibility).
//!
//! Both are plain data with `Default`. Loads report problems instead of
//! failing the launch: a corrupt file is backed up beside itself and
//! defaults are used; saves return the error for the caller to surface.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::session::AgentCmd;

/// `~/Library/Application Support/ddu/` on macOS, `$XDG_CONFIG_HOME/ddu`
/// (or `~/.config/ddu`) elsewhere. Env override `DDU_STATE_PATH` still
/// names the state file directly (launcher/tests); `DDU_SETTINGS_PATH`
/// does the same for settings.
fn data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        return PathBuf::from(home).join("Library/Application Support/ddu");
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("ddu");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/ddu")
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShellConfig {
    pub program: String,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            program: default_shell(),
        }
    }
}

/// Login shell for new Terminal sessions: `$SHELL` when it names a
/// real file, otherwise the first installed fallback. A hardcoded
/// `/bin/zsh` fallback breaks machines without zsh (typical Linux):
/// every restore dies with a spawn ENOENT.
fn default_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .into_iter()
        .chain(
            [
                "/bin/bash",
                "/usr/bin/bash",
                "/bin/sh",
                "/bin/zsh",
            ]
            .map(str::to_string),
        )
        .find(|p| Path::new(p).is_file())
        .unwrap_or_else(|| "/bin/sh".into())
}

/// What the sidebar `+` button creates by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewSessionDefault {
    /// `"terminal"` or a builtin name (`claude`/`codex`/`omp`).
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
    /// Terminal font family; `None` = system default mono face.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font: Option<String>,
    /// Terminal font size (px); `None` = the stock 13px mono size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_font_size: Option<f32>,
    #[serde(default)]
    pub dark_theme: bool,
    /// Desktop notification when an agent signals "your turn" (the
    /// `BEL`/`OSC 9`/`OSC 777` markers of its TUI). `None` = on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify_on_attention: Option<bool>,
    /// Terminal scrollback cap (lines); the grid drops history beyond
    /// it. `None` = the stock 3000-line history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_scrollback: Option<usize>,
}

/// Stock UI base size (px) — the root the panels' `text_sm`/`text_xs`
/// steps are relative to.
pub const UI_FONT_SIZE_DEFAULT: f32 = 14.;

/// Stock mono size the terminal falls back to when unset.
pub const TERMINAL_FONT_SIZE_DEFAULT: f32 = 13.;

/// Stock mono face per platform, mirroring gpui-component's Theme
/// default. The theme's `mono_font_family` is always ddu-managed (the
/// configured terminal face or this), so "System default" needs the
/// same value the registry would have used.
pub const PLATFORM_MONO_FAMILY: &str = if cfg!(target_os = "macos") {
    "Menlo"
} else if cfg!(target_os = "windows") {
    "Consolas"
} else {
    "DejaVu Sans Mono"
};

/// Terminal scrollback cap (lines) the terminal falls back to when unset.
pub const TERMINAL_SCROLLBACK_DEFAULT: usize = 3000;

/// Sane band for the desktop's text scale: outside it the stored value
/// is a broken dconf entry rather than a preference.
const TEXT_SCALE_MIN: f32 = 0.5;
const TEXT_SCALE_MAX: f32 = 3.;

/// Cached result of [`detect_text_scale`] — startup asks dconf once.
static TEXT_SCALE: LazyLock<f32> = LazyLock::new(detect_text_scale);

/// The desktop's text scale, i.e. how much bigger than stock the user
/// asked for UI text to be. GTK apps multiply their font sizes by it
/// and omarchy's `display text size` writes it, so ddu's *defaults*
/// follow it too — otherwise the app sits at stock 12px text next to a
/// desktop that renders everything at 16px. Linux only: 1.0 (no
/// scaling) elsewhere and whenever the preference can't be read.
///
/// An explicit `terminal_font_size` in settings.json stays absolute:
/// a size the user picked while looking at the running app is not a
/// request to scale it again.
pub fn desktop_text_scale() -> f32 {
    *TEXT_SCALE
}

/// Effective UI base size (px): the stock base scaled by the desktop's
/// text scale.
pub fn ui_font_size() -> f32 {
    UI_FONT_SIZE_DEFAULT * desktop_text_scale()
}

/// `DDU_TEXT_SCALE` wins (verification/tests), then dconf, then stock.
fn detect_text_scale() -> f32 {
    let override_ = std::env::var("DDU_TEXT_SCALE").ok();
    let gsettings = cfg!(target_os = "linux")
        .then(|| {
            std::process::Command::new("gsettings")
                .args([
                    "get",
                    "org.gnome.desktop.interface",
                    "text-scaling-factor",
                ])
                .output()
                .ok()
                .filter(|out| out.status.success())
                .and_then(|out| String::from_utf8(out.stdout).ok())
        })
        .flatten();
    resolve_text_scale(override_.as_deref(), gsettings.as_deref())
}

/// Precedence and clamping, split out so both are pinned without a
/// dconf round-trip.
fn resolve_text_scale(override_: Option<&str>, gsettings: Option<&str>) -> f32 {
    override_
        .and_then(parse_text_scale)
        .or_else(|| gsettings.and_then(parse_text_scale))
        .unwrap_or(1.)
}

/// `gsettings get` prints the value as a plain number (`1.3332999999999999`).
fn parse_text_scale(raw: &str) -> Option<f32> {
    let scale: f32 = raw.trim().parse().ok()?;
    scale
        .is_finite()
        .then(|| scale.clamp(TEXT_SCALE_MIN, TEXT_SCALE_MAX))
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
    /// Launcher kind (`terminal`/`claude`/`codex`/`omp`).
    pub kind: String,
    /// Row title (the live OSC title for agents, program name for shells).
    pub title: String,
    /// Agent session id for `--resume`; `None` for shell rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<String>,
    /// Whether the row was running when the workspace was last saved:
    /// `Some(true)` comes back running next launch (as
    /// `--resume <resume>` for an agent), `Some(false)` stays `Done` and
    /// waits for a click. `None` — files written before this flag
    /// existed — falls back to the old rule in
    /// [`crate::app::AppView::restore_sessions`]; every save writes an
    /// explicit value from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<bool>,
    /// The file this session had selected in the diff tree (path;
    /// re-pinned to an index on load).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_file: Option<String>,
    /// Directories collapsed in this session's diff tree.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed_dirs: Vec<String>,
    /// Sidebar splitter height for this session's diff tree (px);
    /// clamped to the layer's min/max on restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_height: Option<f32>,
}

fn default_true() -> bool {
    true
}

/// Root of `state.json`: runtime workspace snapshot, not configuration.
impl gpui_kit::Global for State {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// `None` = never persisted (first run → seed the cwd project);
    /// `Some` is authoritative — `Some([])` means the user removed
    /// every project and the workspace stays empty across restarts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<ProjectConfig>>,
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
    /// Last main-window placement (mode + frame), restored on launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPlacement>,
}

/// How the main window was last shown. On macOS a zoomed (green-button
/// "maximized") window reports as `Windowed` with the zoomed frame, so
/// `Maximized` mainly round-trips cross-platform state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowMode {
    Windowed,
    Maximized,
    Fullscreen,
}

/// Persisted main-window frame (logical px): the variant plus the
/// bounds to open with. For `Maximized`/`Fullscreen` the bounds are the
/// restore (un-maximized) frame, matching gpui's `WindowBounds`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub mode: WindowMode,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
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
        items
    }

    /// Resolve a kind key to a spawnable command. A configured
    /// program that names no existing file fails here with a plain
    /// message instead of a cryptic PTY spawn ENOENT at restore time.
    pub fn cmd_for(&self, kind: &str) -> anyhow::Result<AgentCmd> {
        let (program, args) = if kind == "terminal" {
            (self.shell.program.as_str(), "")
        } else if BUILTIN_AGENTS.iter().any(|(_, p)| *p == kind) {
            (kind, "")
        } else {
            anyhow::bail!("Unknown session type: {kind}");
        };
        anyhow::ensure!(
            !program.trim().is_empty(),
            "Set a program for {kind} in Settings."
        );
        let program = program.trim();
        if program.contains('/') && !Path::new(program).is_file() {
            anyhow::bail!("{program} is not installed (picked in Settings → Program).");
        }
        let args = shlex::split(args).ok_or_else(|| {
            anyhow::anyhow!("Unclosed quote in arguments for {kind}. Check Settings.")
        })?;
        Ok(AgentCmd {
            program: program.into(),
            args,
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
                .unwrap_or_else(|| other.to_string()),
        }
    }

    /// Effective terminal font size (px): the configured value, or the
    /// stock mono size (scaled by the desktop's text scale) when unset.
    pub fn terminal_font_size(&self) -> f32 {
        self.terminal_font_size
            .unwrap_or(TERMINAL_FONT_SIZE_DEFAULT * desktop_text_scale())
    }

    /// Effective mono face: the configured terminal font, else the
    /// platform stock mono. Drives the theme's `mono_font_family`,
    /// which the terminal, diff pane and diff tree all read — without
    /// it they render in the registry's fallback face regardless of
    /// the configured terminal font.
    pub fn mono_family(&self) -> &str {
        self.terminal_font
            .as_deref()
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .unwrap_or(PLATFORM_MONO_FAMILY)
    }

    /// Configured terminal scrollback cap (lines), falling back to the
    /// stock history when unset.
    pub fn terminal_scrollback(&self) -> usize {
        self.terminal_scrollback.unwrap_or(TERMINAL_SCROLLBACK_DEFAULT)
    }

    /// Whether an agent's "your turn" marker raises a desktop
    /// notification — on unless switched off in Settings.
    pub fn notify_on_attention(&self) -> bool {
        self.notify_on_attention != Some(false)
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

/// Warnings from the startup `load_all`, handed to the first window so
/// corrupt-file/backup failures surface in a dialog — bundled launches
/// lose stderr (see `report_error`).
pub struct LoadWarnings(pub Vec<String>);

impl gpui_kit::Global for LoadWarnings {}

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
    fn older_settings_load_with_light_theme() {
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert!(!cfg.dark_theme);
        assert_eq!(cfg.new_session.kind, "terminal");
        // A settings file from before the font-size key still gets the
        // stock mono size (scaled by the desktop's text scale), never a
        // 0px fallback.
        assert_eq!(cfg.terminal_font_size, None);
        assert_eq!(
            cfg.terminal_font_size(),
            TERMINAL_FONT_SIZE_DEFAULT * desktop_text_scale()
        );
        // Same for the scrollback cap: an old file without the key
        // keeps the stock 3000-line history.
        assert_eq!(cfg.terminal_scrollback(), TERMINAL_SCROLLBACK_DEFAULT);
    }


    #[test]
    fn desktop_text_scale_prefers_the_override_and_clamps() {
        // The env override wins, a plain dconf value is used as-is, and
        // a broken value (or none at all) falls back to stock 1.0.
        assert_eq!(resolve_text_scale(Some("1.3333"), Some("2.0")), 1.3333);
        assert_eq!(resolve_text_scale(None, Some("0.8\n")), 0.8);
        assert_eq!(resolve_text_scale(None, None), 1.);
        assert_eq!(resolve_text_scale(Some("nan"), Some("garbage")), 1.);
        assert_eq!(resolve_text_scale(Some("99"), None), TEXT_SCALE_MAX);
        assert_eq!(resolve_text_scale(None, Some("0.01")), TEXT_SCALE_MIN);
    }

    #[test]
    fn removed_last_project_stays_removed() {
        // Never persisted (first run) → None → the cwd project is
        // seeded. An explicit empty list is the user's choice: it must
        // round-trip as Some([]) and never re-seed.
        let fresh: State = serde_json::from_str("{}").unwrap();
        assert_eq!(fresh.projects, None);
        let emptied: State = serde_json::from_str(r#"{"projects": []}"#).unwrap();
        assert_eq!(emptied.projects, Some(vec![]));
        let saved = serde_json::to_string(&emptied).unwrap();
        let reloaded: State = serde_json::from_str(&saved).unwrap();
        assert_eq!(reloaded.projects, Some(vec![]));
    }

    #[test]
    fn window_placement_and_tree_height_round_trip() {
        // The window frame and each session's splitter height must
        // survive state.json; an old file without either still loads.
        let mut state = State::default();
        state.window = Some(WindowPlacement {
            mode: WindowMode::Maximized,
            x: 12.,
            y: 24.,
            w: 960.,
            h: 680.,
        });
        state.projects = Some(vec![ProjectConfig {
            name: "p".into(),
            path: "/tmp/p".into(),
            expanded: true,
            sessions: vec![SavedSession {
                kind: "omp".into(),
                title: "omp".into(),
                resume: Some("id-1".into()),
                live: Some(true),
                selected_file: None,
                closed_dirs: vec![],
                tree_height: Some(260.),
            }],
        }]);
        let saved = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.window, state.window);
        assert_eq!(
            back.projects.as_ref().unwrap()[0].sessions[0].tree_height,
            Some(260.)
        );
        // Files from before this feature carry neither key.
        let old: State = serde_json::from_str(r#"{"projects": []}"#).unwrap();
        assert_eq!(old.window, None);
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

    #[test]
    fn terminal_shell_prefers_an_installed_program() {
        // The Linux failure: `$SHELL` unset and no zsh on disk must
        // not default to a missing `/bin/zsh` (every restore dies
        // with spawn ENOENT); a bogus persisted program is rejected
        // at `cmd_for` with a plain message instead.
        let fallback = super::default_shell();
        assert!(
            std::path::Path::new(&fallback).is_file(),
            "default shell must exist, got {fallback}"
        );
        let cfg = Config {
            shell: ShellConfig {
                program: "/definitely/not/a/shell".into(),
            },
            ..Config::default()
        };
        assert!(cfg.cmd_for("terminal").is_err());
    }
}
