# AGENTS.md — ddu (Day Day Up)

Multi-agent workspace: left = projects + agent sessions, center = PTY terminal
(runs `terminal` / `claude` / `codex` CLIs), right = live git diff. Rust +
`gpui-kit = "0.6"` (re-exports `gpui-pre 0.3.3` + `gpui-component 0.6`).
License: Apache-2.0, GPL-free throughout. Design doc: `DESIGN.md`.

## Source map

- `src/main.rs` — entry; window creation (`gpui_kit::application`).
- `src/app.rs` — `AppView`: all shared state + actions (sessions, diff polling,
  notifications, folder picker). Keyboard actions declared here.
- `src/session.rs` — domain model (`Project`, `AgentSession`, `AgentStatus`),
  launch presets, `initial_projects()` (= cwd).
- `src/config.rs` — `~/.config/ddu/config.toml` (agent menu, overrides) and
  `state.json` (persisted sessions).
- `src/diff/` — `git.rs` git2 working-tree diff (cap 5000 lines/file), polled
  with a seq guard against stale results; `mod.rs` data model.
- `src/terminal/` — `mod.rs` portable-pty pump + subscriber-channel wakeup
  events; `grid.rs` alacritty_terminal grid + parser; `element.rs` custom
  paint element (char-cell metrics, TextRuns, SGR colors).
- `src/ui/` — panels: `session_panel`, `terminal`, `diff_panel`,
  `status_bar`, `title_bar`, `settings_dialog`. Shared metrics/mappings in
  `src/ui/mod.rs` (`PANEL_HEADER_PX` 32, `ROW_PX` 26, selection =
  `foreground.opacity(0.12)`, accent reserved for activity).

## Running — LaunchServices only (macOS 26)

`scripts/ddu-app.sh` copies the release binary into `target/ddu.app` and
`open`s it. Always launch this way.

**Never** start the binary directly as a background child (`nohup`, `hub exec`,
raw spawn) — on macOS 26 an unactivated process: (a) never gets
`NSWindowOcclusionStateVisible` (it reports a private on-screen bit `0x2000`
instead), (b) never receives `windowDidChangeOcclusionState`, (c) cannot
self-activate. gpui's display link is the sole frame driver and its start
guard requires the Visible bit, so a background-launched window freezes after
its first frame and no click/activation path recovers it deterministically.
`cargo run` from a terminal hits the same wall — use the script.

The bundle launcher cds to `DDU_DIR` (default `~/Code/ddu`) because
LaunchServices starts apps with cwd `/`; `initial_projects()` uses cwd.

**Do not vendor-patch gpui-pre-macos.** The registry crate (0.3.3, latest) is
kept pristine; the launch convention above is the chosen fix. (A
`schedule_frame` bypass and an occlusion-guard patch were tried and reverted.)

## Modal dialogs

File/folder pickers must use `rfd::AsyncFileDialog` deferred through
`window.spawn`. The sync picker runs a nested modal runloop inside gpui's
click dispatch; system events fired during the modal (e.g. keyboard-layout
change) re-enter gpui effects while `App` is borrowed → `RefCell already
borrowed` crash.

## Verification

- `cargo test` — includes terminal regression tests (`plain_text_lands`,
  `zsh_prompt_bytes_land` — raw zsh prompt escape bytes must render) and git
  diff tests (`head_diff_sees_edits_and_untracked`).
- Visual: `screencapture -x -l <windowid>` (screen-recording permission is
  granted here; synthetic clicks are NOT — no accessibility). Prove liveness
  by state change: edit a tracked file → right diff panel must show it within
  ~3 s; compare screenshot hashes across the change.

## Conventions

- Single GPUI import surface: `use gpui_kit::*;` plus specific component
  modules. Never invent gpui APIs; follow gpui-kit/gpui-component sources.
- Theme via `cx.theme()`; mono font for terminal/diff/stat text; wrap list
  rows in `ROW_PX` single lines (ellipsis on the title only).
- Async view updates: `cx.weak_entity()` + `view.update_in(cx, ...)`
  inside `window.spawn` (see `add_project`).
- Terminal wakeups flow as `PumpMsg` events through a subscriber channel;
  never poll-render.
