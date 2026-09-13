# Day Day Up (ddu) Design Document

> Multi-agent management app: left = projects + sessions, center = agent terminal, right = git diff.
> UI framework: `gpui-kit = "0.6"` (single source of GPUI via `use gpui_kit::*`). License: Apache-2.0, **GPL-free throughout**.
>
> Status: §2–§11 describe the code as it ships today, and anything designed but
> unbuilt is marked as such and lives in its own document (`AGENT_CORE.md`,
> `FILE_TREE.md`). §12 is the milestone ledger.

## 1. Goal

* Manage multiple projects at once, each with multiple agent sessions.
* Left pane switches projects/sessions, center pane interacts with agents (running `claude` / `codex` CLIs, etc.), right pane shows the current project's git diff.
* MVP bar: three draggable panes, sessions can be opened/killed/restarted, terminal streams readable output and accepts input, diff refreshes on change.

## 2. Non-Goals (Out of MVP)

* Beyond what agent CLIs need: vi-mode, clickable links (`OSC 8`), PTY *content* persistence (the workspace snapshot is persisted; live scrollback is not), remote/multi-window, collab, JS plugins.
* Search is no longer deferred — the terminal and the diff pane each grew a find bar. Alt-screen is not a feature this app implements either way: alacritty owns the alternate grid and the element paints `RenderableContent`, so vim/less work without app code.
* Draggable, serializable dock panels (`DockArea`): MVP uses `h_resizable` with a fixed three-pane layout. Simpler and predictable.
* Built-in editor/file editing: read-only file tree + diff; editing happens back in the terminal.
* Multi-window, collab, JS plugins (`gpui-shell`): out of scope.

## 3. Architecture

Start as a single crate, split by capability into modules (per the gpui-kit Coding Guides; split into workspace crates once it grows):

```text
ddu/
  Cargo.toml
  docs/
    DESIGN.md         # this document
    AGENT_CORE.md     # in-process agent core (design, not implemented)
    FILE_TREE.md      # full file tree + view panel (design, not implemented)
    SIGNING.md        # macOS code identity and the TCC grants it keys
    omarchy/          # desktop tweak notes (host config, not this app)
  src/
    main.rs           # app shell: init + open_window + Root + menus, composition only
    app/              # AppView: three-pane assembly + global state ownership
      mod.rs          # state, actions, key bindings, AppView::render
      sessions.rs     # spawn / kill / restart / select + PTY subscriptions
      diff.rs         # diff poll, selection, find bar
      persist.rs      # state.json read/write, restore-on-launch
      panels.rs       # panel toggles + remembered last-dragged widths
      workspace.rs    # projects, folder picker
    session.rs        # Project / AgentSession / AgentCmd model + resume recipes
    terminal/         # ddu-terminal (only complex module; own directory)
      mod.rs          # TermSession entity: spawn/kill/resize/write + events
      pty.rs          # portable-pty wrapper: spawn, writer, resize, killer
      grid.rs         # alacritty Term behind FairMutex + pump threads
      input.rs        # keystroke → escape-sequence encoding (+ tests)
      element.rs      # custom GPUI Element painting the grid
      attention.rs    # BEL / OSC 9 / OSC 777 hand-back markers
      boxart.rs       # vector box-drawing glyphs
    diff/             # git diff model
      mod.rs          # DiffFile / DiffHunk / DiffLine / GitDiff types
      git.rs          # git2 HEAD→workdir query (+ tests)
    ui/               # surface regions, thin composition over the models
      mod.rs          # scaled(), dialog_footer(), panel_view!, shared metrics
      terminal.rs     # center pane: focus, keys, scroll, exit banner
      session_panel.rs / diff_panel.rs / diff_tree.rs / status_bar.rs / title_bar.rs
      settings/       # settings window: mod.rs (pages) + theme/shell/notify
  scripts/            # dev.sh / install.sh / make-bundle.sh + make-signing-identity.sh
                      # (macOS bundle + signing), linux.sh, lib.sh
  contrib/usage/      # UsageTray (MIT; not a cargo member, no Rust build)
```

Dependency direction: `session`, `terminal` and `diff` are leaves — they never name the app. `app` owns every piece of state; each `ui/` panel is a cached child view that renders `&mut AppView`, so `app` and `ui` reference each other by design, while cross-module traffic inside `app` is `cx.emit / subscribe` (terminal wakeups, resize events) or small shared types. Don't touch `gpui-base` (unless building new behavior).

```toml
[dependencies]
gpui-kit = "0.6"                    # the only GPUI source; never pull gpui directly
alacritty_terminal = "0.24"         # crates.io build, Apache; never the zed fork git rev
portable-pty = "0.9"                # MIT, PTY allocation
git2 = "0.19"                       # MIT, diff data layer
rustix = { version = "0.38", features = ["std", "event"] }  # poll(2) timeout on the PTY reader
anyhow = "1"                        # spawn/IO error context
async-channel = "2"                 # pump → UI wakeup channel (bounded(1) = coalescing)
parking_lot = "0.12"                # unwrappable locks (TermMeta etc.)
rfd = "0.15"                        # folder picker, deferred through `window.spawn`
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"                  # settings.json / state.json
shlex = "1.3"                       # splitting a custom launcher's argument string
```

Note: `alacritty_terminal` is pinned at the `0.24` minor line — bump it only after checking docs.rs. Direct GitHub access from China is extremely slow, so **no git dependencies** — crates.io plus a domestic mirror only.

## 4. UI Layout

```text
Root (gpui-kit; first level of the window)
└─ AppView::render — v_flex
   ├─ TitleBar                 breadcrumb = the active session's OSC title
   ├─ h_resizable("shell")     [ sidebar | region ]
   │    ├─ sidebar  resizable_panel(200px base, 150–300 range, flex_none)
   │    │    session_panel (project tree + session rows)
   │    │    + sidebar status strip
   │    └─ region   v_flex
   │         ├─ h_resizable("panes")   [ terminal | changes ]
   │         │    terminal_pane (cached) | diff_pane (cached, only when shown)
   │         └─ center status strip
   ├─ dialog layer (gpui-kit `Root::render_dialog_layer`)
   └─ shutdown overlay (while live agents are stopped and ids saved)
```

Inside `session_panel` the project tree sits above the diff file tree layer in
its own `v_resizable("sidebar-split")`.

* `Root`: must be the first-level child of every window (`Root::new(view, window, cx)`), otherwise Dialog/Sheet/Notification/focus-trap all break.
* Window identity: both windows (main + settings) open with `config::window_app_id()` — on Linux the Wayland `app_id` / X11 `WM_CLASS` `ddu`, matching the installed `ddu.desktop` basename and its `StartupWMClass`, so the menu entry's icon, taskbar grouping and `class:ddu` rules apply. Unset on macOS, where the bundle carries it, and unset by default in gpui (an anonymous window otherwise).
* Left pane: project rows (`+` quick-add, `...` menu with every launcher and the project ops) with that project's sessions as two-line rows — kind badge + live title, meta line with run duration and a spinner while an agent works; the lower splitter slot is the diff file tree layer (§7).
* Center pane: no tab bar — the session list selects and `⌘1…⌘9` picks the Nth. The pane renders the selected session's `Entity<TermSession>` by id, or the empty state.
* Right pane: a `resizable_panel()` added only while the changes pane is shown (⌘R; 340 px base, 200 px floor, 60 % of the viewport as the drag cap). Its header shows the selected file's path + `+a/−b`; the copy actions (path / contents) live in the body's context menu; the tree layer's summary strip above carries `N files changed` + totals.
* `ElementId` must use domain ids (e.g. `("project-row", project)`, `("diff-line", id)`), **never list indexes**, or add/remove will mix up row state. Never generate random ids inside `render`.
* Panels are cached child views (`panel_view!`): a panel's subtree — render, layout, paint, hitboxes, listeners, key contexts, focus — is replayed until that panel is notified, so every panel states its own root style *including a size*.
* Focus: the terminal owns a `FocusHandle`; clicking a pane focuses the root handle so the cursor goes hollow. `Tab`/`Shift-Tab` are bound in the `Terminal` context (they reach the PTY instead of Root's focus cycling), and the settings window binds Escape/⌘W in its own context.

## 5. State Model

```rust
struct Project { name: String, path: PathBuf, sessions: Vec<AgentSession> }

struct AgentSession {
    id: String,                    // stable id, source of element ids
    title: String,                 // live: agents override it via the OSC title
    status: AgentStatus,           // Running | Done(i32) | Error(String)
    cmd: AgentCmd,                 // launcher preset snapshot
    resume_id: Option<String>,     // conversation id, captured from the run's output
    was_live: bool,                // still running at shutdown → respawn next launch
    kind: String,                  // "terminal" | "claude" | "codex" | "omp"
    started: Instant, ended: Option<Instant>,
    term: Option<Entity<TermSession>>,   // None while the spawn failed
    cwd: PathBuf,                  // project root today; a worktree later
    diff_selected: Option<String>, diff_closed: HashSet<String>,
    diff_tree_height: Option<f32>,
}

struct AgentCmd { program: String, args: Vec<String> }   // + label/basename/spec/resume_spec
```

* There is no separate session-state struct: the grid, PTY handle, selection and
  find-bar state all live inside `TermSession` (§6), which owns the pump.
* Ownership: domain state lives in `AppView` (projects, current project/session
  pointers, panel visibility and widths, diff snapshot, find-bar state); the
  `Entity<TermSession>` per row owns its process. Rendering is read-only.
* Controlled values: held by the owner view; callbacks mutate the owner and end
  with a single `cx.notify()`. No `notify` inside `render` (infinite loop).
* Background pump: the reader thread pulls PTY bytes → feeds the grid → posts a
  `PumpMsg` on a `bounded(1)` channel; `AppView` subscribes and repaints on a
  paced interval. Never poll-render.

## 6. Terminal ddu-terminal (Only Hard Part, GPL Red Line)

### 6.1 Compliance red line (hard constraint)

`zed/crates/terminal*`, `terminal_view*`, and `mappings/` are entirely GPL-3.0: **no verbatim copying**, copying infects your app. Escape sequences are facts — read, understand, rewrite. Only upstream crates.io artifacts are allowed (MIT/Apache): `alacritty_terminal`, `vte`, `portable-pty`.

### 6.2 alacritty for the grid, GPUI for rendering

* `portable-pty`: `openpty + CommandBuilder(cmd, cwd, env) + resize`, spawns the agent CLI; a reader thread pumps bytes (a `poll(2)` wait with a timeout, so the reader wakes on a deadline as well as on input).
* `alacritty_terminal::Term`: grid data model + ANSI state machine, behind a `FairMutex`; bytes are fed in by the pump. The config is alacritty's own `Config { scrolling_history, ..Default }` — no zed `TerminalSettings` copy.
* Key/color mappings: zed's `mappings/` studied, then rewritten minimal (Enter/Ctrl-C/arrows/function keys → escape sequences; SGR 16/256 colors → the theme palette), covering the common agent-CLI set.
* Rendering: a hand-written `Element` (`terminal/element.rs`, following gpui-kit's own `Input/Editor` style; zed's 113 KB `terminal_element.rs` was not ported): cell metrics, `TextRun`s with SGR fg/bg/bold, tinted rows, plus the pieces added since — text selection (copy), scrollback with a hover-revealed scrollbar, SGR mouse + wheel reporting, IME preedit overlay and candidate-popup anchoring, the find bar's match washes, and vector box-drawing glyphs (`boxart.rs`).
* Input: GPUI key events → escape sequences (`input.rs`) → PTY master write. An IME commit writes its text to that same PTY writer (`replace_text_in_range`); the preedit is only an overlay until it commits.
* Resize: panel size change → `Term::resize(cols, lines)` + `pty.resize`, converting via char-cell metrics (font_size × cell), debounced so a window drag reflows ~7×/s instead of per pixel.
* Exit: child exit → `AgentStatus::Done(code)` (a non-zero code colors the row red) or `Error` on an abnormal exit; the last ~64 KiB of output are scanned for the agent's conversation id, which lands in `resume_id` and feeds Resume; the scrollback stays viewable and the row offers Restart.

### 6.3 Scope boundary

Agent CLIs (claude/codex/omp) daily need streaming output, ANSI colors, line-wrap scrolling, an input line and Ctrl-C — but the terminal that grew here is a real one: selection, scrollback, mouse reporting and IME all ship (§6.2). What stays out is what an agent CLI never asks for — vi-mode, clickable links, and PTY *content* persistence (kept in memory only; the workspace snapshot is what survives a relaunch). That keeps the module at "one PTY + one element + a few tables" rather than "port half of Zed", and GPL-clean.

## 7. Right-Pane git diff + file tree layer

* Data: `Repository::discover(project.path)` → `diff_tree_to_workdir_with_index(head, opts)` with `include_untracked(true).recurse_untracked_dirs(true).show_untracked_content(true)` — staged, unstaged and untracked in one pass, no subprocess. Refreshed by the 3 s poll (every session switch reloads immediately); there is **no** `notify`-crate `.git` watcher and no manual refresh control.
* Truncation: a file's collected lines are capped at `MAX_LINES_PER_FILE` (5 000) with the stat counts still counted in full; the pane renders a cap note and reaching it grows that file's budget ×4 up to `EXPAND_MAX_LINES` (200 000).
* View: the file list is the sidebar's diff tree layer (`ui/diff_tree.rs`) — changed paths split into directories, `+a/−b` per file and rolled up per directory, per-session collapse state; the hunk area (`ui/diff_panel.rs`) paints its own rows: two number gutters + a sign column, green `+` / red `-` / untinted context rows, `@@` header bands, `SelectableText` per line and a find bar (⌘F) whose match indices are the pane's child indices.
* Scope: project-level HEAD→workdir diff by default; per-session scope (branch/worktree) comes later — with multiple sessions in one repo they share the project diff. The branch is captured on `GitDiff` but has no display yet.
* The full working-tree listing and the pane's File/Preview modes are designed in `FILE_TREE.md` (not implemented).

## 8. Left Pane and File Tree

* The left pane is the project tree (projects → their sessions) plus, as its
  lower splitter slot, the **diff file tree layer** (§7): today it lists the
  changed files as a directory tree, toggled with ⌘T and resizable per session.
* A full working-tree file tree (`walkdir`/`ignore`-backed, lazily expanded,
  `.gitignore` respected) is designed in `FILE_TREE.md`, which replaces that
  layer's changed-only build rather than adding a second pane beside it. The
  earlier `explorer.rs` sketch in this document is superseded by that plan.

## 9. Agent Backend Abstraction

```rust
struct AgentCmd { program: String, args: Vec<String> }
// Resolved by `config::cmd_for(kind)`: `terminal` = the configured login
// shell, `claude` / `codex` / `omp` = `config::BUILTIN_AGENTS` (program
// name with no arguments). Anything else is rejected — there are no
// custom launchers yet.
```

* Session start = spawn from the preset, cwd = the session's `cwd` (the project
  path today; a per-session worktree later: one branch + one directory each).
* The command line is built in `src/session.rs`: `spec(cwd)` for a fresh start,
  `resume_spec(cwd, id)` for Resume — codex takes the bare `resume <id>`
  subcommand, the others `--resume <id>`. Restart always uses `spec`, ignoring
  the row's `resume_id`.
* kill = an escalating graceful stop (`escalate_close`): Esc, then 2× Ctrl-C 400 ms apart, then 2× Ctrl-D, then a hard kill at ~5 s — whatever ignored all of that still fires the Exit event. The pump exits with the reader. Restart re-spawns the preset into the same row.
* Output parsing: the byte stream is not interpreted as agent events. The one
  exception is the resume id, extracted from the tail of the output (see §6.2)
  and from the persisted `resume_id` on restore.
* **Native agents (designed, not implemented):** `AGENT_CORE.md` specifies a second backend that runs **in one process** — conversations between equal peers instead of PTY-spawned CLIs. It supersedes this section for native agents; the PTY path here stays for shells and external CLIs (`claude`, `codex`).

## 10. Config / Persistence / Shortcuts / Theme / Notifications

* Settings `settings.json` — one-to-one with the Settings window: default new-session
  launcher, login shell, terminal font family/size, scrollback cap, light/dark
  theme, the attention-notification toggle. No `config.toml` ever existed.
* State `state.json` — the runtime workspace snapshot: projects with their session
  rows (including `resume_id` and the per-row `live` flag), the active
  project/session, panel visibility and widths, per-session diff selection /
  collapsed directories / tree height, and window placement. PTY *contents* are
  never persisted; a row that was still running is respawned on the next launch,
  agents resumed from their id.
* Both files live under `~/Library/Application Support/ddu/` (macOS) or
  `$XDG_CONFIG_HOME/ddu/` / `~/.config/ddu/` (Linux); `DDU_STATE_PATH` /
  `DDU_SETTINGS_PATH` name them directly. Load checks each file: a corrupt one is
  backed up as `<name>.corrupt-<ts>` and defaults are used. Writes go to a sibling
  temp file and are renamed over the target.
* Shortcuts, all `secondary-` (⌘ on macOS, ⌃ elsewhere): ⌘N new session, ⌘O add
  project, ⌘1…⌘9 select the Nth session, ⌘T toggle the diff file tree, ⌘B toggle
  the sidebar, ⌘R toggle the changes pane, ⌘W close session, ⌘, settings, ⌘Q quit,
  ⌘+/⌘− terminal font zoom. Copy/paste/find are `secondary-` on macOS and
  `⌃⇧C` / `⌃⇧V` / `⌃⇧F` on Linux, so the terminal keeps `Ctrl+C` for SIGINT. A
  chord the app binds is never forwarded to the shell — on Linux `⌃N/O/T/B/R/W`
  and friends are ddu's, not readline's — while everything unbound still reaches
  the shell (pinned by `app::tests::shell_control_keys_stay_with_the_shell`).
* Theme: everything via `cx.theme()` tokens, no hardcoded colors; terminal SGR colors map onto the theme palette.
* Confirmations: every destructive prompt (close session, quit with live agents)
  is an `open_alert_dialog` whose footer comes from `ui::dialog_footer(...)`, so
  the Cancel-outline + danger-button recipe is shared and the button names the
  outcome. A row's exit/error is reported in the row itself
  (`AgentStatus::Done` / `Error` + the final duration), not as an in-app toast.
* Desktop notifications (macOS): an interactive agent CLI never "finishes" at the
  process level, so the signal is the agent's own hand-back marker in the PTY
  stream — `BEL`, `OSC 9 ; <text>` (iTerm2/WezTerm/Ghostty) or
  `OSC 777 ; notify ; <title> ; <body>` (urxvt). `terminal/attention.rs` scans
  the raw bytes beside the alacritty parser and emits `TermEvent::Attention`;
  `AppView` raises `show_system_notification` (gpui → `UNUserNotificationCenter`,
  one tag per session row so a newer marker replaces the older toast) unless the
  window is active *and* that session is the one on screen. ConEmu progress
  (`OSC 9 ; 4 ; …`) is filtered — it means "busy", not "yours". Setting:
  Settings → General → Notifications. macOS only hands a notification to a
  process whose code identity is the app's: `make-bundle.sh` signs the
  exec'd `ddu.bin` with the bundle id and the local certificate, then seals
  the bundle, so a differently-signed build calls
  `show_system_notification` and the OS drops it (`UNErrorDomain Code=1`).
  `docs/SIGNING.md`: the requirement must stay stable or every grant
  (notifications, Screen Recording, Accessibility) dies with the next
  install.

## 11. Performance and Correctness

* Repaint pacing is adaptive: the pump spaces output-driven repaints by a
  `stream_interval(paint_ms)` (50 ms → 66 → 100 ms as a frame's own paint cost
  grows, EWMA-fed). Keystrokes, scrolling and selection bypass the throttle, so
  interactive latency is unchanged.
* Panels are cached child views: a stream frame notifies the terminal pane alone,
  an OSC-title change adds the sidebar row and the title-bar breadcrumb, and every
  other `cx.notify()` fans out through `AppView::notify_panels`. This is what keeps
  a streaming agent from rebuilding the changes pane 20×/s.
* The terminal element paints only the visible window; large diffs are capped per
  file (§7) and the diff pane renders only the rows in view.
* `render` stays declarative: read state, compose elements; parsing/mutation go in
  named methods. One `cx.notify()` per mutation.
* Pin `gpui-kit 0.6` (i.e. `gpui-pre 0.3.3`); upgrade only by following gpui-kit.

## 12. Milestones

* ~~M1 Static three panes~~ ✅ shipped (c41296a): resizable + mock panes, settings + theme switching.
* ~~M2 Real PTY~~ ✅ shipped: `terminal/` runs real agents (`claude`/`codex`/`omp`, shell fallback), streaming, colors, scrollback, resize, keystroke encoding. Acceptance met: the app is interactive; headless roundtrip tests cover spawn → parse → grid → exit.
* ~~M3 Real diff~~ ✅ shipped: `diff/` queries HEAD→workdir (staged + unstaged + untracked) via git2; 3 s poll; stat counts with directory roll-ups; per-file line cap with a growing budget (§7).
* ~~M4 Session management~~ ✅ shipped: open / kill / restart / exit status / confirmations / **persistence** — `state.json` restores projects, layout and per-session state, and rows that were still running come back running, agents resumed from their captured id.
* M5 File tree + worktree + polish — open:
  * the full working-tree file tree and the pane's file/preview modes: designed in `FILE_TREE.md`, not implemented;
  * per-session worktrees (one branch + one directory per session) — sessions share the project diff today;
  * syntax highlighting in the file view — deferred there too (`FILE_TREE.md` §8.4).
  Shortcuts, theme switching, the settings window and the sidebar's diff tree layer are shipped.
* Desktop notifications ✅ shipped: `terminal/attention.rs` decodes the agent's own hand-back markers (`BEL` / `OSC 9` / `OSC 777`) off the PTY stream, `AppView` posts one `show_system_notification` per row unless that terminal is the one on screen; Settings → General toggles it.

## 13. Risks

* GPL contamination: understand and rewrite, never paste; `cargo deny` license checks are still a to-do (the repo has no CI workflow yet).
* Slow GitHub access: no git dependencies; cargo via mirror (npmmirror/tuna rsproxy per global rules).
* GPUI API drift: follow `gpui-kit` docs only, never invent APIs; model rendering on component's own patterns.
* Terminal Element complexity: the element stays a hand-written painter over alacritty's grid — new terminal features are added only when an agent CLI actually needs them (§6.3).
