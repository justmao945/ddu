# AGENTS.md — ddu (Day Day Up)

Multi-agent workspace: left = projects + agent sessions, center = PTY terminal
(runs `terminal` / `claude` / `codex` CLIs), right = live git diff. Rust +
`gpui-kit = "0.6"` (re-exports `gpui-pre 0.3.3` + `gpui-component 0.6`).
License: Apache-2.0, GPL-free throughout. Design docs: `docs/DESIGN.md` (the app),
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
  `~/Library/Application Support/ddu/`: `settings.json` (user
  settings, one-to-one with the Settings window) and `state.json`
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
  `src/ui/mod.rs` (`PANEL_HEADER_PX` 32, `ROW_PX` 26, selection =
  `foreground.opacity(0.12)`, accent reserved for activity).

## Running — LaunchServices only (macOS 26)

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
- `make-bundle.sh` signs with a **local self-signed code-signing
  identity** (`Day Day Up Local Signing`, keychain
  `~/Library/Keychains/ddu-signing.keychain-db`), and only falls back to
  ad-hoc on a machine that has no such identity. It signs `ddu.bin`
  first, with the bundle id as its code identifier, then the bundle
  without `--deep`.
  The identity is not cosmetic: macOS keys bundle-scoped services
  (desktop notifications) and TCC grants (Screen Recording) off the
  running process's *code identity* — the process is the exec'd
  `ddu.bin`, so an unsigned build calls `show_system_notification` and
  macOS drops it (`UNErrorDomain Code=1`, usernotificationsd "not
  allowed"), and `--deep` would re-sign the binary with a derived id and
  undo the match. And grants are stored against the *designated
  requirement*: ad-hoc's requirement is a bare `cdhash`, which every
  re-sign changes, so each install silently invalidated the existing
  系统设置 → 屏幕录制 entry — the toggle stayed on while every request
  failed (`Failed to match existing code requirement for subject
  dev.just.ddu`, which is also why `screencapture` broke after an
  install). The certificate makes the requirement
  `identifier "dev.just.ddu" and certificate root = H"c6c9…"`, which no
  build changes.
- One-time setup of that identity (done on this machine; here for the
  next clean install). Generate the certificate — `-legacy` matters,
  Security cannot verify OpenSSL 3's default PBES2 MAC:
  ```
  openssl req -x509 -newkey rsa:2048 -sha256 -days 3650 -nodes \
    -keyout key.pem -out cert.pem -subj "/CN=Day Day Up Local Signing" \
    -addext basicConstraints=critical,CA:FALSE \
    -addext keyUsage=critical,digitalSignature \
    -addext extendedKeyUsage=critical,codeSigning
  openssl pkcs12 -export -legacy -out identity.p12 -inkey key.pem \
    -in cert.pem -passout pass:ddu
  ```
  put it in its own keychain (a key imported into the *login* keychain
  raises an authorization dialog on every build, even with `-A`, and
  setting its partition list there needs the login password):
  ```
  KC=~/Library/Keychains/ddu-signing.keychain-db
  PW=$(openssl rand -hex 16)
  security create-keychain -p "$PW" "$KC"
  security set-keychain-settings -lut 21600 "$KC"
  security import identity.p12 -k "$KC" -P ddu -T /usr/bin/codesign -A
  security set-key-partition-list -S apple-tool:,apple: -s -k "$PW" "$KC"
  ```
  trust it (`find-identity -v` lists nothing without this step — an
  untrusted self-signed cert shows as `CSSMERR_TP_NOT_TRUSTED`),
  ```
  security add-trusted-cert -r trustRoot -p codeSign \
    -k ~/Library/Keychains/login.keychain-db cert.pem
  ```
  and list the keychain — **codesign resolves identities through the
  search list**, `--keychain` alone answers "no identity found":
  ```
  security list-keychains -d user -s \
    ~/Library/Keychains/login.keychain-db "$KC"
  ```
  The keychain's password lives in `~/.config/ddu/signing.keychain-pw`
  (0600) and `make-bundle.sh` unlocks it per build (it locks on sleep);
  key material is backed up in `~/.config/ddu/signing/`.
  `DDU_SIGN_IDENTITY` overrides the whole arrangement (e.g. a real
  Developer ID, with its own keychain unlocked by the caller).

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
  granted here; synthetic clicks are NOT — no accessibility). When the
  Screen Recording is keyed to the requester's *code identity*, and the
  requester here is the app, not the agent CLI (ddu spawns omp in a PTY,
  so TCC attributes the request to `dev.just.ddu` — granting Terminal or
  omp does nothing). The stable signing identity above is what keeps
  that grant across rebuilds; if a request is refused, `log show
  --predicate 'subsystem == "com.apple.TCC"'` names the subject and says
  whether the stored requirement failed to match (a leftover entry from
  an ad-hoc-signed build does exactly that): remove the entry, re-add
  /Applications/ddu.app and relaunch the app. When the capture is
  refused (`screencapture -x` fails with "could not create
  image from display"), get pixels from the app itself instead: a
  temporary `DDU_VERIFY_SHOT=<path>` hook that calls
  `window.render_to_image()` on the main thread a few ticks after launch
  and quits. That needs `features = ["test-support"]` on the *main*
  `gpui-kit` dependency plus `gpui-pre-macos = { version = "0.3.3",
  features = ["test-support"] }` (gpui-pre's `test-support` does not
  forward to the macOS crate, and the feature is what compiles
  `render_to_image`). `image`'s encoders are off — write `img.as_raw()`
  and convert with PIL. Layout questions are then answered by measuring
  ink rows in the dump, not by eyeballing. Prove liveness
  by state change: edit a tracked file → right diff panel must show it within
  ~3 s; compare screenshot hashes across the change.
- Settings window: `DDU_VERIFY_SETTINGS=<page_ix> bash scripts/dev.sh`
  bakes the flag into the bundle launcher; the app then auto-opens the
  Settings window on that page (0-based) for screenshots. Relaunch without
  the env var to regenerate a clean launcher.
- Diff search: `DDU_VERIFY_SEARCH=<query> bash scripts/dev.sh` waits for
  the first diff poll, selects the first changed file, opens the find bar
  (⌘F) with the query and dumps the observed state (match count, scroll
  offset, scroll-container child count) to `/tmp/ddu-search-verify.json`.
  The hook polls readiness on a background timer — a self-rearming
  `defer_in` pumps a frame per defer at display-link rate, starving the
  main runloop (frozen app, ~100% CPU). Dev hooks that need to re-check
  state must never re-arm per frame.
- Terminal search: `DDU_VERIFY_TERMSEARCH=<query> bash scripts/dev.sh`
  plants a marker row + filler straight into the session's grid (never
  the PTY — an agent CLI would read a write as a prompt), opens the
  find bar, queries, reveals and dumps `{open, matches, current,
  display_offset}` to `/tmp/ddu-term-search-verify.json`. Same
  background-timer discipline as above.

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
- Global shortcuts live in `AppView::new` (`src/app.rs`): ⌘T/⌘N new
  session (same action, guarded against an empty workspace), ⌘O add
  project (shares `add_project`'s folder-picker flow), ⌘,/⌘B/⌘R/⌘W.
  Terminal-scoped ⌘C/⌘V (`TermCopy`/`TermPaste`) double as the
  right-click menu's shortcut hints via `PopupMenuItem::action` — any new
  terminal action shown in a menu must wire its action the same way.
