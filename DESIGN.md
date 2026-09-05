# Day Day Up (ddu) Design Document

> Multi-agent management app: left = projects + child tabs, center = agent terminal, right = git diff.
> UI framework: `gpui-kit = "0.6"` (single source of GPUI via `use gpui_kit::*`). License: Apache-2.0, **GPL-free throughout**.

## 1. Goal

* Manage multiple projects at once, each with multiple agent sessions.
* Left pane switches projects/sessions, center pane interacts with agents (running `claude` / `codex` CLIs, etc.), right pane shows the current project's git diff.
* MVP bar: three draggable panes, sessions can be opened/killed/restarted, terminal streams readable output and accepts input, diff refreshes on change.

## 2. Non-Goals (Out of MVP)

* Full xterm semantics: alt-screen fullscreen (vim/less), vi-mode, search, link click, persistence, remote — all deferred.
* Draggable, serializable dock panels (`DockArea`): MVP uses `h_resizable` with a fixed three-pane layout. Simpler and predictable.
* Built-in editor/file editing: read-only file tree + diff; editing happens back in the terminal.
* Multi-window, collab, JS plugins (`gpui-shell`): out of scope.

## 3. Architecture

Start as a single crate, split by capability into modules (per the gpui-kit Coding Guides; split into workspace crates once it grows):

```text
ddu/
  Cargo.toml
  DESIGN.md
  src/
    main.rs        # app shell: init + open_window + Root, composition only
    app.rs         # AppView: three-pane assembly + global state ownership
    session.rs     # Project / AgentSession model + spawn/kill/restart
    terminal.rs    # ddu-terminal: PTY + grid + TerminalView (only complex module)
    diff.rs        # GitDiff model + right-pane view
    explorer.rs    # left-pane project tree (later; MVP can use nested SidebarMenuItem)
    config.rs      # config + persistence + agent command definitions
```

Dependency direction: `app -> {session, terminal, diff, explorer}`; features never point at each other, communication via `cx.emit / subscribe` or small shared types. Don't touch `gpui-base` (unless building new behavior); styling lives in the app layer only.

```toml
[dependencies]
gpui-kit = "0.6"                    # the only GPUI source; never pull gpui directly
alacritty_terminal = "0.24"         # crates.io build, Apache; never the zed fork git rev
portable-pty = "0.9"                # MIT, PTY allocation
vte = { version = "0.15", features = ["ansi"] }  # MIT, only if needed (alacritty_terminal already parses)
git2 = "0.19"                       # MIT, diff data layer
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
```

Note: pin the exact `alacritty_terminal` 0.2x minor version against docs.rs at implementation time. Direct GitHub access from China is extremely slow, so **no git dependencies** — crates.io plus a domestic mirror only.

## 4. UI Layout

```text
Root
└─ h_resizable("main")
   ├─ left 260px: Sidebar (projects) + per-project session list
   ├─ center flex: TabBar (current project's sessions) + TerminalView + bottom Input
   └─ right 380px collapsible: Diff panel (file list + hunks)
```

* `Root`: must be the first-level child of every window (`Root::new(view, window, cx)`), otherwise Dialog/Sheet/Notification/focus-trap all break.
* Left pane: `Sidebar + SidebarGroup + SidebarMenuItem`, projects nest `children([...sessions])`; selection via `active()`; right-click `context_menu()` for kill/restart/close; badge shows running count.
* Center pane: `TabBar::new("sessions").selected_index(...)` + controlled `on_click` switching; content renders the `Entity<SessionState>` matching `session.id`.
* Right pane: `resizable_panel().visible(self.show_diff)` for collapse; header holds branch + stat + refresh + toggle.
* `ElementId` must use domain ids (e.g. `("session-tab", session.id)`), **never list indexes**, or add/remove will mix up tab state. Never generate random ids inside `render`.
* Focus: terminal owns a `FocusHandle`, toggled with the bottom Input focus; reserve `Tab` shortcuts for panel switching without conflicting with Root's Tab navigation.

## 5. State Model

```rust
struct Project { id: String, name: String, path: PathBuf, sessions: Vec<AgentSession> }
struct AgentSession {
    id: String,            // stable id, source of ElementId
    title: String,
    status: AgentStatus,   // Running | Done(i32) | Killed | Error(String)
    cmd: AgentCmd,         // backend command snapshot
    state: Entity<SessionState>,  // terminal state owner
}
struct SessionState {
    pty: Option<PtyHandle>,       // child process + writer
    grid_version: u64,            // dirty flag for coalesced notify
    // grid itself (Term or home-grown line buffer, see §6)
    scrollback: Vec<TermLine>,    // simplified line buffer is the cheapest MVP
    exit_status: Option<i32>,
}
```

* Ownership: domain state lives in `AppView` (projects + current pointers), PTY handles live in `SessionState`, rendering is read-only.
* Controlled values: `TabBar.selected_index` and `Sidebar.active` are owned by the owner view; callbacks mutate the owner + a single `cx.notify()`. No `notify` inside `render` (infinite loop).
* Background pump: PTY reader thread via `cx.spawn / background_spawn` collects bytes → updates grid → throttled `notify` (~60fps coalesced). Keep the `Task` handle for kill.

## 6. Terminal ddu-terminal (Only Hard Part, GPL Red Line)

### 6.1 Compliance red line (hard constraint)

`zed/crates/terminal*`, `terminal_view*`, and `mappings/` are entirely GPL-3.0: **no verbatim copying**, copying infects your app. Escape sequences are facts — read, understand, rewrite. Only upstream crates.io artifacts are allowed (MIT/Apache): `alacritty_terminal`, `vte`, `portable-pty`.

### 6.2 Plan: alacritty for the grid, GPUI for rendering

* `portable-pty`: `openpty + CommandBuilder(cmd, cwd, env) + resize`, spawns the agent CLI; reader thread pumps bytes.
* `alacritty_terminal::Term`: grid data model + ANSI state machine (`Term::new(config, cols, lines, listener)`, bytes fed via advance). Write a minimal `TermConfig` of our own (scrollback lines, cursor shape); do not copy zed's `TerminalSettings`.
* Key/color mappings: study zed's `mappings/` then **rewrite** a minimal table (Enter/Ctrl-C/arrows/function keys → escape sequences; SGR 16/256 colors → near `cx.theme()` colors), covering only the common agent-CLI set.
* Rendering: hand-write a simplified `Element` (following gpui-kit's own `Input/Editor` style; do NOT port zed's 113KB `terminal_element.rs`): take visible rows → paint `TextRun`s with SGR fg/bg/bold, highlight rows with tinted divs. MVP covers main screen + streaming + scroll + text selection + basic keyboard/mouse reporting.
* Input: GPUI key events → escape sequences → PTY master write; the bottom `Input` is a second input path (sends a full line + `\n`); both paths write only to the PTY, never to each other's state.
* Resize: panel size change → `Term::resize(cols, lines)` + `pty.resize`, converting via char-cell metrics (font_size × cell), debounced.
* Exit: child exit → `status = Done(code)` + `push_notification`, scrollback kept viewable, restart button offered.

### 6.3 Why not a full terminal

Agent CLIs (claude/codex) daily need only: streaming output, ANSI colors, line-wrap scrolling, one-line input, Ctrl-C interrupt. Dropping alt-screen/vim/mouse-tracking shrinks the work from "port half of Zed" to "200 lines of PTY + simplified rendering", and stays GPL-clean.

## 7. Right-Pane git diff

* Data: `git2::Repository::open(project.path)` → `statuses` (file list) + `diff_index_to_workdir + diff_head_to_index` (hunks). Watch `.git` with `notify` + manual refresh + 3s fallback poll (poll-only is fine for MVP).
* View: file list via `List` (stat: `+a/-b`); hunk area paints its own rows: line numbers + green `+` / red `-` / gray context rows, `@@` header + filename. Large diffs render only visible rows (`VirtualList`).
* Scope: project-level HEAD diff by default; per-session scope (branch/worktree) comes later — with multiple sessions in one repo, share the project diff first and label the branch in the title.

## 8. Left Pane and File Tree

* MVP: left pane = project tree, no full file tree. Nested `SidebarMenuItem` sessions are enough.
* Later `explorer.rs`: `tree::{Tree, TreeState, TreeItem}` delegate backed by `walkdir` + `notify`, lazy expansion, honoring `.gitignore` (via the `ignore` crate). Coexists with the session tree: each project node splits into `Sessions` / `Files` groups.

## 9. Agent Backend Abstraction

```rust
struct AgentCmd { program: String, args: Vec<String>, cwd: PathBuf, env: Vec<(String, String)> }
// config.toml presets: claude = { program: "claude", args: [] }, codex = {...}, custom = {...}
```

* Session start = spawn from snapshot, cwd = project path (later worktree: one branch + one directory per session).
* kill = close writer + kill child + cancel pump Task; restart = re-spawn same cmd, either clearing or keeping scrollback (pick one; default keep + separator line).
* Output parsing: MVP treats output as a byte stream, no structured agent-event parsing; "task status extraction" (reading plan/tool-call lines) comes later.

## 10. Config / Persistence / Shortcuts / Theme / Notifications

* Config `~/.config/ddu/config.toml`: agent commands, font, scrollback, follow-system theme.
* Persistence `~/.config/ddu/state.json`: project list + current selection; PTY contents are NOT persisted (sessions restart as empty shells needing manual restart).
* Shortcuts: `cmd-1/2/3` focus the three panes, `cmd-t` new session, `cmd-w` close session (with `AlertDialog` kill confirmation), `cmd-b` collapse right pane. Via `actions! + bind_keys`.
* Theme: everything via `cx.theme()` tokens, no hardcoded colors; terminal SGR colors map onto the theme palette.
* Notifications: agent exit/error via `push_notification`; kill via `open_alert_dialog` (title names the object, confirm button names the outcome, e.g. Remove "xxx").

## 11. Performance and Correctness

* Coalesce/throttle terminal notifies; read only the visible grid window; truncate large diffs (single file >5000 lines shows head/tail N lines + notice).
* `render` stays declarative: read state, compose elements; parsing/mutation go in named methods. One `cx.notify()` per mutation.
* Pin `gpui-kit 0.6` (i.e. `gpui-pre 0.3.1`); upgrade only by following gpui-kit.

## 12. Milestones

* M1 Static three panes: resizable + Sidebar mock data + TabBar switching + right-pane mock diff. Acceptance: window drags, all three panes switch correctly.
* M2 Real PTY: `ddu-terminal` runs `bash` echo + `claude` launch + clean resize. Acceptance: `cargo run` is interactive on M2.
* M3 Real diff: wire up git2 + refresh on change. Acceptance: editing a file updates the right pane within 3s.
* M4 Session management: open/kill/restart/notifications/confirmations/persistence. Acceptance: killing a session never crashes, restart recovers.
* M5 File tree + worktree + polish: explorer, per-session worktrees, shortcuts, theme.

## 13. Risks

* GPL contamination: understand and rewrite, never paste; add `cargo deny` license checks in CI.
* Slow GitHub access: no git dependencies; cargo via mirror (npmmirror/tuna rsproxy per global rules).
* GPUI API drift: follow `gpui-kit` docs only, never invent APIs; model rendering on component's own patterns.
* Terminal Element complexity: land simplified rendering for M2 first; alt-screen only when actually needed.
