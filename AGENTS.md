# AGENTS.md — ddu (Day Day Up)

Multi-agent workspace: left = projects + agent sessions, center = PTY terminal
(runs `terminal` / `claude` / `codex` CLIs), right = live git diff. Rust +
`gpui-kit = "0.6"` (re-exports `gpui-pre 0.3.3` + `gpui-component 0.6`).
License: Apache-2.0, GPL-free throughout. Design doc: `DESIGN.md`.

## Source map

- `src/main.rs` — entry; window creation (`gpui_kit::application`).
- `assets/icons/` + `src/main.rs` `AppAssets` — brand SVGs (claude/openai
  from simple-icons, CC0; `omp.svg` hand-drawn π) layered over the
  gpui-kit icon set; monochrome, tinted via `text_color`.
- `src/app.rs` — `AppView`: all shared state + actions (sessions, diff
  polling, notifications, folder picker). Keyboard actions declared here.
- `src/session.rs` — domain model (`Project`, `AgentSession`, `AgentStatus`),
  launch presets, `initial_projects()` (= cwd).
- `src/config.rs` — persistence split in two JSON files under
  `~/Library/Application Support/ddu/`: `settings.json` (user
  settings, one-to-one with the Settings window) and `state.json`
  (runtime workspace snapshot: projects, panel widths, per-project
  diff state, last agent resume hint). Loads/saves check errors;
  corrupt files are backed up with `.corrupt-<ts>` and defaults are
  used.
- `src/diff/` — `git.rs` git2 working-tree diff (cap 5000 lines/file), polled
  with a seq guard against stale results; `mod.rs` data model.
- `src/terminal/` — `mod.rs` portable-pty pump + subscriber-channel wakeup
  events; `grid.rs` alacritty_terminal grid + parser; `element.rs` custom
  paint element (char-cell metrics, TextRuns, SGR colors).
- `src/ui/` — panels: `session_panel`, `terminal`, `diff_panel`,
  `status_bar`, `title_bar`, `settings_window` (standalone native window,
  singleton via a global slot). Shared metrics/mappings in
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

Confirm dialogs: never hand-roll `DialogFooter` button pairs — use
`ui::dialog_footer(label, id, on_confirm)` (`src/ui/mod.rs`:
Cancel-outline + danger-small shared recipe). Set `.on_ok(...)` alongside
the footer when Enter should confirm. One-off informational dialogs
(no footer) are fine inline.
## Verification

- `cargo test` — includes terminal regression tests (`plain_text_lands`,
  `zsh_prompt_bytes_land` — raw zsh prompt escape bytes must render) and git
  diff tests (`head_diff_sees_edits_and_untracked`).
- Visual: `screencapture -x -l <windowid>` (screen-recording permission is
  granted here; synthetic clicks are NOT — no accessibility). Prove liveness
  by state change: edit a tracked file → right diff panel must show it within
  ~3 s; compare screenshot hashes across the change.
- Settings window: `DDU_VERIFY_SETTINGS=<page_ix> bash scripts/ddu-app.sh`
  bakes the flag into the bundle launcher; the app then auto-opens the
  Settings window on that page (0-based) for screenshots. Relaunch without
  the env var to regenerate a clean launcher.

## Conventions

- Single GPUI import surface: `use gpui_kit::*;` plus specific component
  modules. Never invent gpui APIs; follow gpui-kit/gpui-component sources.
- Terminal wakeups flow as `PumpMsg` events through a subscriber channel;
  never poll-render.
- Global shortcuts live in `AppView::new` (`src/app.rs`): ⌘T/⌘N new
  session (same action, guarded against an empty workspace), ⌘O add
  project (shares `add_project`'s folder-picker flow), ⌘,/⌘B/⌘R/⌘W.
  Terminal-scoped ⌘C/⌘V (`TermCopy`/`TermPaste`) double as the
  right-click menu's shortcut hints via `PopupMenuItem::action` — any new
  terminal action shown in a menu must wire its action the same way.
