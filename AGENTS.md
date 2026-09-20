# AGENTS.md — ddu (Day Day Up)

Multi-agent workspace: left = projects + agent sessions, center = PTY terminal
(runs `terminal` / `claude` / `codex` CLIs), right = live git diff. Rust +
`gpui-kit = "0.6"` (re-exports `gpui-pre 0.3.3` + `gpui-component 0.6`),
pinned to the upstream commit that fixes the rendered document's double wrap
until a release carries it (`docs/RUNNING.md`, `docs/UI.md`).
Targets macOS and Linux (X11/Wayland). Apache-2.0, GPL-free throughout
(`contrib/usage/` is MIT). This file is the short form: the rules to obey while
editing, and where the long form lives.

## Docs

- `docs/DESIGN.md` — architecture: source tree (§3), layout (§4), state model
  (§5), terminal (§6), diff (§7), config/persistence/shortcuts (§10),
  performance summary (§11), milestones (§12).
- `docs/RUNNING.md` — build/run/install per platform, macOS LaunchServices
  rules, shell default, window identity.
- `docs/VERIFICATION.md` — how to prove a change on macOS and on Linux.
- `docs/UI.md` — GPUI/panel invariants and the measurements behind them.
- `docs/SIGNING.md` — codesign identity and TCC grants (macOS).
- `docs/FILE_TREE.md` — the file tree + view panel design, **landed** except
  per-session worktrees; `docs/AGENT_CORE.md` — design, **not implemented**.
- `docs/omarchy/` — Omarchy desktop tweak notes (host config, not this app).

## Source map

One Cargo workspace, four crates, layered bottom-up: a crate names the ones
below it and never the ones above (`ddu-diff` is a leaf beside the chain; the
binary is a target of `ddu-app`, not a layer of its own).

- `crates/ddu-terminal` — the terminal stack. `lib.rs` is the root: the
  `TermSession` entity + `TermEvent`, and the public surface the pane uses
  (`PtySpawn`, `TermMatch`, `MouseTracking`, `STREAM_FRAME_MIN`). Each concern
  is its own module: `session.rs` PTY lifecycle + IO, `stream.rs` repaint
  pacing, `search.rs` ⌘F over the grid, `mouse.rs` reporting to the child,
  `selection.rs`, `scrollbar.rs` geometry + drags, `ime.rs`, and the test-only
  `harness.rs`; `pty.rs` process + master handles, `grid.rs` alacritty grid +
  pump threads, `element.rs` custom paint element, `palette.rs` the ANSI ramp +
  theme defaults, `input.rs` keystroke → escapes, `boxart.rs`. Owns no app
  config: the scrollback cap arrives as a `TermSession::spawn` argument.
- `crates/ddu-diff` — `lib.rs` the data model (the pane's `RowStream`, and
  `marks()` — the overview's change runs) plus the re-exported `git2` it
  speaks; `git.rs` git2 working-tree diff (5000 lines/file cap, polled with a
  seq guard against stale results); `listing.rs` the lazy working-tree listing
  (case-insensitive name order) and the quick open's path ranking;
  `file_view.rs` the whole-file surface: file lines + byte offsets + the parsed
  `SyntaxHighlighter`, built on the background pass; `read.rs` the one read
  policy both text surfaces obey (binary, size caps, gone-from-disk). The one
  crate that turns on the `tree-sitter-*` `gpui-kit` features.
- `crates/ddu-core` — `lib.rs` is the root; `session.rs` the domain model
  (`Project`, `AgentSession`, `AgentStatus`), launch presets,
  `spec`/`resume_spec`, `initial_projects()` (= cwd); `config.rs`
  `settings.json` + `state.json` under
  `~/Library/Application Support/ddu/` (macOS) or `$XDG_CONFIG_HOME/ddu/`
  (Linux); `DDU_STATE_PATH` / `DDU_SETTINGS_PATH` override. Loads check errors;
  a corrupt file is backed up as `<name>.corrupt-<ts>` and defaults are used.
- `crates/ddu-app` — the shell, one crate that ships itself: `lib.rs` mounts
  `app` + `ui`, and `main.rs` is the `ddu` binary (`[[bin]] name = "ddu"` —
  the artifact, the desktop entry and the window's `app_id` all key off
  `ddu`, not off the crate name).
  - `main.rs` — entry, window creation, `AppAssets` (brand SVGs layered over
    the gpui-kit icon set), the panic log, and the icon-asset test.
  - `app/` — `mod.rs` `AppView` (the shared state + the actions macro + the
    app's own accessors), `keys.rs` (the binding table + accel labels + the
    `Command` table the settings window rebinds), `render.rs`
    (the Render impl: shell layout, action handlers, splitter healing),
    `pane.rs` (the right pane's surface/mode, its file-view + preview caches and
    the per-file scroll memory),
    `search.rs` (both bars — the pane's find and the tree's quick open),
    `diff.rs` (poll, snapshot apply, selection re-pin, tree index),
    `sessions.rs` (spawn/kill/restore/resume), `shutdown.rs` (the confirmed
    close/⌘Q path), `persist.rs` (state.json round-trip), `panels.rs` (dock
    toggles + the panel-cache contract tests),
    `workspace.rs` (projects, folder picker).
  - `ui/` — panels (`session_panel`, `terminal_panel` (the center pane),
    `diff_panel/` (the changes pane: `mod.rs` frame + `rows.rs` + `overview.rs`
    + `body.rs` + `find_bar.rs`), `file_tree/` (`mod.rs` rows + `index.rs` the
    tree model), `status_bar`, `title_bar`), `palette.rs` (the quick open's
    floating overlay — scrim + card over the workspace, never inside a panel),
    `settings/` (standalone window: `mod.rs` the shipped pages +
    `update_config`, `keys.rs` the shortcut recorder), `markdown.rs`
    (the image plugin a rendered document goes through: gpui's own text view
    sends every `![]()`/`<img>` to the *http* client, which cannot read a path,
    so the block holding an image is rendered here through
    `img(Resource::Path)`), `code_text.rs` — `SelectableText` with
    caller-supplied `TextRun`s (gpui-base's lays out the runs it built from its
    own text, so a highlighted row cannot ride it); one selection participant
    per row, same contract — and `mod.rs` with the shared metrics + `scaled()`.
- `contrib/usage/` — UsageTray (MIT; not a cargo member, the Rust build never
  touches it).
- `.omp/agents/AGENTS.md` — the global agent-rules payload deployed to
  `~/.agents/AGENTS.md`; it is not ddu's own rules (these are).

## Build / run

- Linux: `scripts/linux.sh run` (build `--release` + run, workspace = `$DDU_DIR`
  or this repo; the build is CPU-capped through `scripts/capped.sh`) and
  `scripts/linux.sh install`. Details: `docs/RUNNING.md`.
- macOS: `scripts/dev.sh` / `scripts/install.sh` — the only launch paths.
- **On Linux, every `cargo` build, test and check goes through
  `scripts/capped.sh`** — `scripts/capped.sh cargo test -p ddu-app`,
  `scripts/capped.sh cargo check`, anything else CPU-heavy. A bare `cargo`
  cannot be limited from the outside: `[build] jobs` counts rustc *processes*
  and misses the LLVM thread pool inside one crate, so a single build pins
  every core (measured 7.62 of 8 bare, 4.01 capped). The wrapper runs the
  command in a systemd user scope at half the machine's cores
  (`DDU_BUILD_CPU_QUOTA=400%` / `off`) **and** serializes it through an
  `flock`: one wrapped command at a time per user, so N agents in N terminals
  cannot add up to N quotas — the rest wait their turn and are told they are
  waiting. Scope and lock both end with the command (the app `run` execs is
  outside them); a host with no systemd user session (macOS, a container)
  runs uncapped.
- `cargo test` — the whole workspace; `-p ddu-app` (or `ddu-core`, `ddu-diff`,
  `ddu-terminal`) restricts it to one crate's suite, which is also the quickest
  way to compile just that layer. `-p ddu-app` runs the library's tests and the
  binary's. On Linux wrap it as above:
  `scripts/capped.sh cargo test -p ddu-app`.

## Rules

- **GPL-free throughout.** Zed's `terminal*`/`mappings` are GPL-3.0: understand
  and rewrite, never paste. Only crates.io artifacts (MIT/Apache) are allowed.
- **The workspace is layered** (`ddu-terminal`/`ddu-diff` → `ddu-core` →
  `ddu-app`, whose `main.rs` is the `ddu` binary): a crate names the ones below
  it and never the ones above, so a setting read goes *up* the stack as an
  argument — the terminal takes its scrollback cap as a `TermSession::spawn`
  parameter instead of reading `Config` — and a capability lives with its owner:
  only `ddu-diff` turns on the `tree-sitter-*` `gpui-kit` features, and the
  terminal's test-only surface (`inject_bytes`, `kill_and_join`, the spawn-time
  `allow_parking`) sits behind its `test-support` feature, which `ddu-app`'s
  dev-dependencies enable for the tests that drive a PTY from another crate.
- **Never hardcode a shell.** `config::default_shell()` resolves it; a missing
  hardcoded path kills every restored session (`docs/RUNNING.md`).
- **macOS**: never launch the binary as a background child or via `cargo run` —
  the window freezes after one frame; never `pgrep`/`pkill` the app (the caller
  is usually its descendant); `launch.sh` must keep `exec` as its last line;
  never vendor-patch `gpui-pre-macos` (`docs/RUNNING.md`).
- **Window identity**: both windows open with `config::window_app_id()` —
  on Linux `ddu`, which must equal the desktop entry's basename, or the window
  is anonymous (`docs/RUNNING.md`).
- **One GPUI surface**: `use gpui_kit::*;` plus specific component modules.
  Never invent gpui APIs — follow the gpui-kit/gpui-component sources.
- **An icon must exist in the asset bundle**: `AppAssets` layers this repo's
  `assets/icons/*.svg` over `gpui_kit::assets::Assets`, which carries the icon
  set's *default* list only, so a path neither source provides renders as an
  empty gap — silently, with no error anywhere (the Keys page shipped
  `icons/keyboard.svg` that way). A new glyph is vendored into `assets/icons/`
  (Lucide, ISC, with the provenance comment) plus a case in `AppAssets::load`;
  a stock name comes from `gpui_kit::component::IconName` (generated from the
  default list), never from the full catalog's. Pinned by
  `icon_assets::every_app_icon_exists` in `main.rs`.
- **Scroll regions**: the `.vertical_scrollbar(...)` host must be an un-padded
  `v_flex` ancestor, never the tracked element (`docs/UI.md`).
- **Cached panels**: a panel's subtree replays until notified, so keep the
  three-halves notify discipline (`subscribe_term` / OSC-title /
  `notify_panels`) and give every `root_style()` a size (`docs/UI.md`). The
  changes pane is the exception — it is mounted *uncached* because gpui's
  window selection needs its participants re-registered every frame, and a
  cached subtree registers nothing (a live selection in the pane was swept a
  frame later). Never cache a panel whose text can be selected.
- **Splitters**: keep the flex slots unpinned and correct widths through
  `resize_panel` + `suppress_resize_records`, never an unconditional
  render-time fixup (`docs/UI.md`).
- **Long lists are virtual**: the diff pane and the tree declare row sizes and
  build only the visible slice; their rows come from a prebuilt index or a
  cached view, never from work redone per frame (`docs/UI.md`).
- **An idle poll is inert**: `apply_snapshot` compares and reports whether
  anything moved; only then repaint, invalidate the file-view/preview caches
  (`diff_gen`) or rebuild the tree index. A poll that repaints regardless is a
  flash every 3 s over content that did not change.
- **Splitter widths heal per container size**, never per render: the correction
  exists for a resize that landed while every slot was still pinned, and a drag
  never changes the container — a per-render correction reverts the drag
  (`docs/UI.md`, pinned by `a_dragged_splitter_is_not_reverted_by_a_render`).
- **The mono face must name an installed family**: gpui matches a family by
  exact name against the loaded faces — no fontconfig substitution — and a
  family it cannot find falls back to the *UI* face, which is how the stock
  `DejaVu Sans Mono` rendered the terminal and every code row in a proportional
  font. `ui::resolve_mono_family` (called from `apply_mono_typography`, i.e.
  after every `Theme::change`) corrects it, and `ui::is_mono_family` is the one
  monospace heuristic (the settings picker lists with it too). That heuristic
  rejects gpui's *virtual* names — `.ZedMono`/`.ZedSans` name Lilex and IBM
  Plex Sans, which the machine need not have — because `all_font_names()`
  carries them and `.ZedMono` is otherwise the first list entry saying "mono"
  (`docs/UI.md`).
- **Highlighting is per row, off the file** (`diff/file_view.rs` + `ui/code_text.rs`):
  `ViewRow::offset` is the row's byte offset in the file (`None` for a spliced
  deletion, which is never colored), and `RowStream::row_styles` clips a
  whole-file style range down to that row. The parse rides `FileView::build`'s
  background thread; a render never parses. Grammars come from the
  `tree-sitter-*` features on `ddu-diff`'s `gpui-kit` dependency
  (`crates/ddu-diff/Cargo.toml`, all MIT — the GPL-free rule), one per
  language, and only the **File** surface highlights: Diff mode's hunks
  are fragments with no offsets into the file.
- **The overview strip** lives in the pane's scrollbar column (its own rect,
  two halves: added left, removed right), is built from `RowStream::marks()`,
  and positions in relative lengths, never pixels (`docs/UI.md`). A clean file
  has no marks, so it draws no strip.
- **A file nobody changed has no view switch**: the pane's mode button and
  `set_view_mode(Diff)` both need a diff, because `surface()` folds a Diff
  request on an unchanged file back into the file itself.
- **The pane's default surface is File** (`ViewMode::default`): the whole file
  with the diff tinted in place, and no `@@` bands there — the merged stream is
  the file in order.
- **The tree lists the working tree lazily** (`diff/listing.rs`): only the root and
  the directories the user has expanded are read (`list_dir`), merged with the
  poll's diff, so a 40k-file repository draws a few hundred rows. Clean files are
  listed (directories first, in the tree's own text color — the figures mark a
  change, not the name's weight), so anything that assumed "a row means a
  changed file" — the selection, the pane's rows, the find bar — goes through
  `AppView::selection` (a path) instead. A file nobody changed renders as the
  file itself (`AppView::surface` folds Diff into File), and a zero figure is
  never printed (`+8`, not `+8 −0`).
- **Nothing is filtered but `.git`** (`diff/listing.rs::list_dir`): the tree is
  the filesystem's, so `.gitignore` is not read at all — `target/`, `.env`, a
  build output are listed like anything else, in the tree's own text color with
  no figures (git's view of a file is its *badge*, not a gate on the listing).
  A symlink is classified by what it points at (`entry_kind`): `DirEntry`'s own
  kind reports the link, which would otherwise vanish from the listing.
- **The quick open's universe is a walk, and its ranking is a memo over it**
  (`diff/listing.rs`): `walk_files` — not the git index — is what a palette
  query searches (background executor, once per open, `.git` the only skip;
  `docs/UI.md`), and `PathSearch` re-ranks a keystroke against the survivors of
  the typed prefix instead of the whole list. That memo and its tier pruning
  rest on every tier being **monotone in the needle** (a longer needle can only
  rank a path the same or worse): a new, non-monotone tier (a fuzzy score) must
  stay off the pruned path, or the fast path is wrong, not slow.
- **A restore of the pane's scroll position waits for its own rows**: rows
  mount in stages (the poll's hunks, then the whole-file view), and the virtual
  list clamps an offset to whatever is mounted — applied early, a deep position
  is lost for good. `AppView::pending_scroll` defers it; see `docs/UI.md`.
- **The file tree is the project's, the open file is the session's**: one
  repository and one working tree per project means one tree, so its expansion
  (`Project::tree_open`), layer height, own scroll (`tree_scroll`) and
  default-expansion flag live on the
  `Project` (and in `ProjectConfig`), never on a session — a session records
  only the file it has open (`AgentSession::diff_selected`) and the pane's mode.
  A switch therefore leaves the tree's rows, expansion and own scroll exactly
  where they were (`adopt_session_diff` resets the outgoing row's state, not the
  working tree's), puts the incoming row's file back on the spot when the
  project's diff is still loaded (`select_path`), and only a cold tree lets the
  first poll seed it. The pane's position is read out **before** the switch —
  `remember_scroll` at the top of every path that moves the current session, and
  in `remove_session_row` before the row goes — or the outgoing file keeps only
  the place it had at the last file switch.
- **Two surfaces keep their place elsewhere**: a rendered Markdown document's
  scroll lives in the `Entity<TextViewState>` the pane draws it from
  (`AppView::documents`, one per `(working tree, path)`) — gpui drops a text
  view's state as soon as it is not rendered, so a document would otherwise
  start at the top every time the file comes back; and the file tree's place is
  the project's (`Project::tree_scroll`), read off the live handle by every
  path that moves `current_project` (`remember_tree_scroll`, beside
  `remember_scroll`) and applied through `pending_tree_scroll` when the incoming
  project's rows exist (`rebuild_tree_index` — a switch drops the snapshot, and
  with it the index). Neither is a `file_positions` entry; see `docs/UI.md`.
- **`h_flex()` centers on the cross axis**: `flex_row` + `items_center`, so a
  row that must hand a child the full height states `items_stretch()`
  (`docs/UI.md`).
- **Never `use super::*` in a `crates/ddu-app/src/app/*` test module**: the parent globs
  gpui-kit, and globbing *it* brings gpui's `#[test]` attribute into scope,
  which expands into itself ("recursion limit reached while expanding
  `#[test]`"). Name the imports the test needs.
- **Repaint**: never poll-render — `PumpMsg` events drive it; `subscribe_term`
  is the single subscription point, and a wakeup repaints the pane only for the
  visible session while an OSC-title change notifies the sidebar row from a
  *background* session too (its spinner is on screen even when its grid is not;
  a row left to the duration tick reads as jerky). The adaptive
  `stream_interval` steps must sit above a real frame's paint cost. The one
  wall-clock repaint — the sidebar's `m`/`h`/`d` duration reading — sleeps to
  that reading's own turn-over (`AgentSession::label_change_in`, capped by
  `LABEL_TICK_CAP`) and notifies the *sidebar alone*: a per-second notify on the
  app sends the fan-out through every cached panel, redrawing the terminal grid
  and the changes pane for a string that did not move (`docs/UI.md`).
- **Keyboard**: app shortcuts are `secondary-` chords (copy/paste/find are
  `⌃⇧` on Linux); a chord a binding claims never reaches the PTY — `⌃P` (the
  quick open) trades readline's previous-history for a file search, pinned by
  `app::tests::the_quick_open_owns_its_chord_in_every_context`. A search bar's
  arrows belong to its input, so the bars step with ⌘G/⌘⇧G — except the
  palette, whose single-line field makes gpui-base's `up`/`down` binding land on
  a handler that was never registered, letting the palette's own binding take
  them (`docs/UI.md`). Any action shown in a menu must wire
  `PopupMenuItem::action` — that is what makes the item show the chord that
  runs it, so an item wired only to `on_click` reads as a command the keyboard
  cannot reach — and it must be that chord's *winning* binding, not a fallback
  in its chain (a predicate-less binding ties a named context at the focused
  element and wins the later-binding tiebreak). The tree's and the pane's copy
  commands follow it (⌥⌘C / `⌃⌥C` path, ⌥⇧⌘C / `⌃⌥⇧C` contents), and a tree
  right-click selects its row first, so the menu and the chord always act on
  the same file
  (`docs/UI.md`).
- **Shortcuts are one table, and overrides are appended**: every rebindable
  shortcut is a `Command` in `crates/ddu-app/src/app/keys.rs` (id = the `settings.json` `keys`
  key, the settings row, the builtin chords); the Keys page records a chord into
  that map, and `keys::apply_overrides` adds the new chord plus an `Unbind` on
  every chord it retires. Never `clear_key_bindings` — gpui-component's own
  bindings live in the same keymap — and never re-bind the builtin table on a
  later window (it would land *after* the override layer and out-rank it);
  `keys::install` is once per process. Hints print the current chord through
  `accel_hint(id, cx)`, so a rebind moves the tooltips and menu labels with it.
- **Modal dialogs**: pickers go through `rfd::AsyncFileDialog` deferred with
  `window.spawn`; confirm dialogs use `ui::dialog_footer(...)`, never a
  hand-rolled `DialogFooter` pair (`docs/UI.md`).
- **Box drawing** stays vector-drawn (except the three diagonals) so crossings
  match borders (`docs/UI.md`); sidebar hover goes through
  `session_panel::toggle_hover`, never an assignment in `on_hover`.

## Verification

- Prove UI changes against the real surface: the `computer` device on macOS,
  observable state (`hyprctl clients -j`, the PTY child, `DDU_STATE_PATH`) on
  Linux — `docs/VERIFICATION.md`.
- Verify against a scratch repo via `DDU_DIR` / `DDU_STATE_PATH` /
  `DDU_SETTINGS_PATH`, never the working tree.
- Prove liveness by state change (edit a tracked file → the diff panel shows it
  within ~3 s).
- **A test that spawns a PTY owns its pumps**: end it with
  `harness::shutdown` (kill the child, cancel the forwarder, join both
  threads); `TermSession::spawn` additionally calls the executor's per-test
  `allow_parking()` — both live behind `ddu-terminal`'s `test-support` feature,
  which `ddu-app`'s dev-dependencies enable, so a PTY test in a *dependent*
  crate still gets them. A pump thread's wake reaches a `!Send` task from its own
  thread, which gpui's test scheduler reports as non-determinism in whichever
  test is running then — and one executor serves the whole test process, so
  the binary aborts (SIGABRT) rather than failing one test. Measured ~10% of
  runs before the fix; prove it with a 20× loop, never a single green run
  (`docs/VERIFICATION.md`).
