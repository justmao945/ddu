# AGENTS.md — ddu (Day Day Up)

Multi-agent workspace: left = projects + agent sessions, center = PTY terminal
(runs `terminal` / `claude` / `codex` CLIs), right = live git diff. Rust +
`gpui-kit = "0.6"` (re-exports `gpui-pre 0.3.3` + `gpui-component 0.6`).
Targets macOS and Linux (X11/Wayland).
License: Apache-2.0, GPL-free throughout (`contrib/usage/` is MIT — see
`contrib/usage/LICENSE`). Design docs: `docs/DESIGN.md` (the app),
`docs/AGENT_CORE.md` (in-process agent core — design, not yet implemented).

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
  `~/Library/Application Support/ddu/` (macOS) or
  `$XDG_CONFIG_HOME/ddu/` / `~/.config/ddu/` (Linux): `settings.json`
  (user settings, one-to-one with the Settings window) and `state.json`
  (runtime workspace snapshot: projects, the active project/session,
  panel widths, per-project diff state, last agent resume hint, and
  per-row `live` — the rows still running at the last save, which the
  next launch respawns).
  Loads/saves check errors; corrupt files are backed up with
  `.corrupt-<ts>` and defaults are used.
- `src/diff/` — `git.rs` git2 working-tree diff (cap 5000 lines/file), polled
  with a seq guard against stale results; `mod.rs` data model.
- `src/terminal/` — `mod.rs` portable-pty pump + subscriber-channel wakeup
  events; `grid.rs` alacritty_terminal grid + parser; `element.rs` custom
  paint element (char-cell metrics, TextRuns, SGR colors); `attention.rs`
  streams `BEL`/`OSC 9`/`OSC 777` markers out of the raw bytes (the signal
  behind the agent-finished desktop notification).
- `src/ui/` — panels: `session_panel`, `terminal`, `diff_panel`,
  `status_bar`, `title_bar`, `settings_window` (standalone native window,
  singleton via a global slot). Shared metrics/mappings in
  `src/ui/mod.rs` (`scaled(base)` — all shell geometry (row heights,
  panel widths, tree-layer heights, indents) is a base px value at
  text-scale factor 1.0 multiplied by
  `config::desktop_text_scale()`, so layout tracks the GTK text scale
  the fonts already follow; selection = `foreground.opacity(0.12)`,
  accent reserved for activity).
- `contrib/usage/` — UsageTray, a self-contained MIT-licensed subproject: a macOS
  menu-bar tray (Swift) for subscription plan usage plus an Omarchy bar-widget
  port (`omarchy/`). Not a cargo member; the Rust build never touches it.
- `docs/omarchy/` — Omarchy desktop tweak notes (text size, fcitx5). The text-size
  note documents the very switch `config::desktop_text_scale()` reads.
- `.omp/agents/AGENTS.md` — the global agent-rules payload, deployed to
  `~/.agents/AGENTS.md`; it is not ddu's own rules (these are).

## Running

### Linux (X11 / Wayland)

- Dev: `scripts/linux.sh run` — builds `--release` and runs the binary from
  `$DDU_DIR` (default: this repo). `initial_projects()` seeds from the
  process cwd, so the launch directory *is* the workspace. `DDU_STATE_PATH`
  / `DDU_SETTINGS_PATH` isolate a test workspace.
- Install: `scripts/linux.sh install` — copies the binary to
  `~/.local/bin/ddu` (prefix: `DDU_INSTALL_DIR`), installs `assets/icon.svg`
  into the hicolor theme and writes `~/.local/share/applications/ddu.desktop`
  with `Path=` set to the launch directory (a desktop launch would otherwise
  start in `$HOME`).
- Window identity: every `WindowOptions` opens with `config::window_app_id()` —
  on Linux the Wayland `app_id` / X11 `WM_CLASS` `ddu`, which must equal the
  desktop entry's basename (`ddu.desktop`) for the menu launch to find its
  window: icon, taskbar grouping and `class:ddu` window rules all key off it.
  gpui leaves the identity unset on its own, so the window otherwise arrives as
  an anonymous client (empty `hyprctl clients` class). `None` on macOS, where
  the bundle carries the identity instead. Verify a launch with
  `hyprctl clients -j | jq '.[] | select(.pid==<ddu pid>) | .class'`.
- No bundle, no LaunchServices, no signing — a plain binary whose window the
  compositor maps normally. There is nothing here to keep in sync with the
  macOS bundle flow, so the two never collide (the scripts write different
  paths).
- State: `$XDG_CONFIG_HOME/ddu/` (default `~/.config/ddu/`).
- Keyboard: every app shortcut is a `secondary-` chord — `⌘` on macOS, `⌃`
  on Linux (`src/app/mod.rs::key_bindings`). Copy/paste/find are the
  exception: a terminal shares those keys with the shell, so on Linux they
  live in the `⌃⇧` space (`Ctrl+C` must stay SIGINT). A chord a binding
  claims never reaches the PTY — gpui's bubble-phase action dispatch stops
  propagation before the terminal's key listener runs — so the Linux `⌃R`,
  `⌃N`, `⌃O`, `⌃T`, `⌃B`, `⌃W` chords are ddu's, not readline's. Anything
  unbound still reaches the shell. Pin the split with
  `app::tests::shell_control_keys_stay_with_the_shell`.
- Shell default: `config::default_shell()` takes `$SHELL` only when it names
  a real file, then the first installed of `/bin/bash`, `/usr/bin/bash`,
  `/bin/sh`, `/bin/zsh`. Never hardcode a shell — a minimal environment (a
  desktop entry, a session without `SHELL` exported) otherwise spawns a
  missing `/bin/zsh` and every restored session dies with a spawn ENOENT.

### macOS 26 — LaunchServices only

- Dev: `scripts/dev.sh` — builds, bundles `target/ddu-dev.app` as "Day Day
  Up Dev" (`dev.just.ddu.dev`), isolates state under
  `~/Library/Application Support/ddu-dev/`, then `open`s it.
- Install: `scripts/install.sh [--open]` — builds and bundles straight
  into `/Applications` as "Day Day Up" (`dev.just.ddu`; `DDU_INSTALL_DIR`
  overrides). Shared builder: `scripts/make-bundle.sh`. The distinct
  bundle ids let dev and installed run side by side without LaunchServices
  activating the wrong one; `open` never mixes their state either.
- Replacing a running copy: both scripts go through `scripts/lib.sh`
  (`app_pids` / `stop_app`, shared by `dev.sh` and `install.sh`). NEVER
  `pgrep`/`pkill` here — they hide the caller and all its ancestors, and
  the app being replaced is usually an ancestor (these scripts are run
  from a ddu terminal session), so the kill silently no-ops and `open`
  just re-activates the stale instance. `stop_app` refuses (status 2) in
  that ancestor case: the caller is running inside the app, so killing it
  would take the shell down mid-script. The generated `launch.sh` must
  keep `exec` as its LAST line — anything appended after it never runs.
- `make-bundle.sh` signs with a local self-signed code-signing identity
  (`Day Day Up Local Signing`; `scripts/make-signing-identity.sh`
  provisions it, `DDU_SIGN_IDENTITY` overrides). It still signs `ddu.bin`
  first with the bundle id as its identifier, then the bundle without
  `--deep`. Do not swap the identity or regenerate the certificate
  casually: macOS stores the app's grants (desktop notifications, Screen
  Recording) against the *designated requirement*, so only a stable
  certificate keeps them across rebuilds — `docs/SIGNING.md`.

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
- UI verification runs against the real window through the `computer` device
  (`bash scripts/dev.sh`, then screenshot / `win.ax()` / `win.click|press|type`;
  `screencapture -x -l <windowid>` still works for pixels). Screen Recording,
  Accessibility and input belong to the *app* (`dev.just.ddu`, not omp or the
  terminal, and a stale grant is why either fails; `docs/SIGNING.md`).
  Accessibility is additionally cached per process, so a new grant only applies
  after the app is quit and reopened — and the FIRST query after a launch
  returns a bare tree (registering the client is what makes gpui build one; the
  next frame carries it), so query twice. What the AX tree exposes is what the
  app opts into: session rows, diff-tree rows (`Role::TreeItem` + name +
  selected/expanded), the terminal grid's visible text, the find bars (labelled
  inputs and `1/5` counters) and every button ddu owns — icon-only ones carry an
  `accessibility_label`. gpui-component's own chrome (the settings page nav, its
  list/tree widgets) exposes nothing, so drive that by coordinates and keep
  actions reversible: a click in the terminal pane types into a live agent.
- On Linux there is no AX tree and, on a Wayland session without a
  screenshot portal, no pixels either — verify by observable state instead:
  `hyprctl clients -j` (the window mapped, its pid and size), `ps -eo pid,ppid,cmd`
  (the PTY child of the ddu pid is the restored/default session's shell), the
  state file (`DDU_STATE_PATH`) for what the app persisted, and
  `hyprctl binds -j` that no compositor chord collides with the app's
  accelerators (omarchy/Hyprland grab `SUPER`/`CTRL+ALT` chords, never bare
  `CTRL` or `CTRL+SHIFT`). A fresh state spawns the default launcher by itself,
  so the shell child proves the PTY path without touching the UI.
- The diff find bar ("Find in diff") needs a selected file first: ⌘T drops the
  file-tree layer in under the sessions, ⌘R opens the changes pane, then click a
  row in the tree and ⌘F. The layer toggles make the whole sequence replayable
  from a fresh state.
- Verify against a scratch repo, not the working tree: make one dirty (`git
  init`, commit, edit) and point the dev launch at it (`DDU_DIR=/tmp/scratch
  DDU_STATE_PATH=/tmp/v.json DDU_SETTINGS_PATH=/tmp/vc.json bash scripts/dev.sh`).
  Fresh state also starts with every pane closed, the cleanest base for a run.
- Prove liveness by state change: edit a tracked file → the right diff panel
  must show it within ~3 s; compare screenshot hashes across the change.

## Conventions

- Single GPUI import surface: `use gpui_kit::*;` plus specific component
  modules. Never invent gpui APIs; follow gpui-kit/gpui-component sources.
- Scroll regions: the `.vertical_scrollbar(...)` host must be an un-padded
  ancestor, never the tracked element itself — the overlay is an
  `absolute inset_0` CHILD of whatever hosts it, so hosting it on a
  padded `track_scroll` element counts that padding as content and leaves
  phantom scroll range (a scrollbar over a list that fits). That host
  must also be a flex container (`v_flex`), or the `flex_1` scroller
  inside never gets a bounded height and long content is clipped instead
  of scrolling.
- Terminal wakeups flow as `PumpMsg` events through a subscriber channel;
  never poll-render.
- Session repaints go through `AppView::subscribe_term`, the single
  subscription point for every spawn path (new/restart/restore). It
  repaints only for the session the center pane renders (`is_visible_term`):
  a background row keeps parsing — selecting it must show current output —
  but its output changes nothing on screen, and several streaming agents
  would otherwise each add a full-window redraw per frame (measured 47 fps
  vs 1 fps with two background streams). Exit/attention/diff poll/
  interaction notify on their own; background rows pick up their OSC
  title/status on the next repaint.
- The three shell panels are **cached child views** (`panel_view!` in
  `src/ui/mod.rs`): sidebar, changes pane and terminal pane mount as
  `Entity::cached(panel::root_style())`, so gpui replays a panel's whole
  subtree — render, layout, paint, hitboxes, mouse listeners, key
  contexts, focus — until that view is notified. The title bar's
  breadcrumb is one too (`ui/title_bar.rs`): it mirrors the session
  title, which agent CLIs spin, and it must not drag the panels into
  that repaint. Three halves to keep in sync, all pinned by tests in
  `src/app/mod.rs`:
  * a stream wakeup notifies `terminal_pane` alone (`subscribe_term`),
    which is what keeps the panels cached on stream frames — measured
    with the changes pane open: per-frame draw cost −35%, taffy layout
    −65%, sidebar render −92%;
  * a stream frame that *changes the OSC title* (the spinner glyph in
    the sidebar row and the breadcrumb) additionally notifies those two
    — and nothing else, or an agent's spinner tick would rebuild the
    changes pane 20 times a second;
  * every other `cx.notify()` on `AppView` fans out through
    `AppView::notify_panels` (an app-level `observe_self`), or a panel
    whose state changed would keep its stale frame.
  A panel's cached style must be its own layout box — *including a
  size*: a cached box is laid out as a leaf from that style alone
  (there is nothing to measure), so one that leaves its cross size to
  its content collapses to zero and its replayed content lands wherever
  the parent centers that empty box (which is how the title bar's
  breadcrumb ended up against the bar's bottom border). Every panel
  states a size (`size_full` / `h_full`), and each panel states
  `root_style()` once so the mount and the panel's root element agree.
- Splitter widths (`shell_state` / `panes_state` in `src/app/mod.rs`):
  gpui-base pins every slot at its first measured bounds
  (`update_panel_size`), and once all slots are pinned
  `adjust_to_container_size` proportionally REWRITES every recorded
  width on each window resize. Render therefore keeps the flex slots
  unpinned (`reset_panel` on the region and the center pane — the
  adjust then bails) and re-asserts the recorded sidebar/diff widths
  when a resize already drifted them (`resize_panel`, flagged via
  `suppress_resize_records` so the drag-persist subscription ignores
  the synthetic event). Never let a render-time correction fire
  unconditionally: when the window is too narrow to honor a width, the
  clamped layout is correct and retrying would emit Resized every
  frame.
- Stream repaint pacing is adaptive: the pump spaces output-driven
  repaints by `stream_interval(paint_ms)` — 50 ms (20 fps) while the
  terminal element's own paint is cheap, then 66 / 100 ms once a frame's
  paint passes 4 / 9 ms (`STREAM_FRAME_STEPS`, EWMA fed by
  `TermSession::note_paint_cost`). Frame rate is the one lever that
  scales the whole-window redraw (gpui repaints every primitive each
  frame); keystrokes, scrolling and selection never pass through the
  throttle, so interactive latency is unchanged. The cost steps have to
  sit *above* what a real repaint costs, not below: a full-screen TUI
  redraw on a 1400×900 retina window measures p50 1.8 ms / p90 3.6 ms,
  so the original 2.5 ms first step pinned every agent turn at 15 fps —
  under the floor, and the stutter was plainly visible. 20 fps in the
  pane is fine to watch; the *session list* stutter was a different bug
  (the cached row missing the title notify, see above).
- Box-drawing chars are all vector-drawn except the three diagonals
  (`src/terminal/boxart.rs`, pinned by
  `the_whole_box_drawing_block_is_vector`): a char left to the font
  glyph renders at the font's own weight and bounding box, so anything
  missed — `┼` was — disagrees with the vector strokes it meets and a
  table's crossings come out heavier than its borders.
- Sidebar hover slots (`hovered_session`/`hovered_project`) update through
  `session_panel::toggle_hover`, never by assigning in the `on_hover`
  callback: mouse listeners bubble in reverse paint order, so the row
  being left reports its leave AFTER the row being entered reports hover —
  an unconditional clear drops the fresh entry and the row's action
  buttons never appear while moving down the list.
- Global shortcuts live in `AppView::new` (`src/app/mod.rs`): ⌘N new
  session (guarded against an empty workspace), ⌘O add project (shares
  `add_project`'s folder-picker flow), ⌘T/⌘B/⌘R toggle the diff file tree /
  sidebar / changes pane, ⌘W close session, ⌘, settings.
  Terminal-scoped ⌘C/⌘V (`TermCopy`/`TermPaste`) double as the
  right-click menu's shortcut hints via `PopupMenuItem::action` — any new
  terminal action shown in a menu must wire its action the same way.
