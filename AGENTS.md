# AGENTS.md — ddu (Day Day Up)

Multi-agent workspace: left = projects + agent sessions, center = PTY terminal
(runs `terminal` / `claude` / `codex` CLIs), right = live git diff. Rust +
`gpui-kit = "0.6"` (re-exports `gpui-pre 0.3.3` + `gpui-component 0.6`).
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

- `src/main.rs` — entry, window creation, `AppAssets` (brand SVGs layered over
  the gpui-kit icon set).
- `src/app/` — `mod.rs` `AppView` (all shared state + actions: sessions, panel
  toggles, shortcuts, notifications), `sessions.rs` (spawn/kill/restore),
  `diff.rs` (poll, selection, both search bars — the pane's find and the
  tree's quick open — and the per-file scroll positions), `persist.rs`
  (state.json round-trip),
  `panels.rs`, `workspace.rs` (projects, folder picker).
- `src/session.rs` — domain model (`Project`, `AgentSession`, `AgentStatus`),
  launch presets, `spec`/`resume_spec`, `initial_projects()` (= cwd).
- `src/config.rs` — `settings.json` + `state.json` under
  `~/Library/Application Support/ddu/` (macOS) or `$XDG_CONFIG_HOME/ddu/`
  (Linux); `DDU_STATE_PATH` / `DDU_SETTINGS_PATH` override. Loads check errors;
  a corrupt file is backed up as `<name>.corrupt-<ts>` and defaults are used.
- `src/diff/` — `git.rs` git2 working-tree diff (5000 lines/file cap, polled
  with a seq guard against stale results); `mod.rs` data model (the pane's
  `RowStream`, and `marks()` — the overview's change runs); `tree.rs` the lazy
  listing (case-insensitive name order) and the quick open's path ranking;
  `view.rs` the whole-file surface: file lines + byte offsets + the parsed
  `SyntaxHighlighter`, built on the background pass.
- `src/terminal/` — `mod.rs` portable-pty pump + subscriber-channel wakeups,
  `grid.rs` alacritty grid, `element.rs` custom paint element, `attention.rs`
  (`BEL`/`OSC 9`/`OSC 777` → desktop notification), `boxart.rs`.
- `src/ui/code_text.rs` — `SelectableText` with caller-supplied `TextRun`s
  (gpui-base's lays out the runs it built from its own text, so a highlighted
  row cannot ride it); one selection participant per row, same contract.
- `src/ui/` — panels (`session_panel`, `terminal`, `diff_panel`, `diff_tree`,
  `status_bar`, `title_bar`), `palette.rs` (the quick open's floating overlay —
  scrim + card over the workspace, never inside a panel), `settings/`
  (standalone window), `markdown.rs`
  (the image plugin a rendered document goes through: gpui's own text view sends
  every `![]()`/`<img>` to the *http* client, which cannot read a path, so the
  block holding an image is rendered here through `img(Resource::Path)`), and
  `mod.rs` with the shared metrics + `scaled()`.
- `contrib/usage/` — UsageTray (MIT; not a cargo member, the Rust build never
  touches it).
- `.omp/agents/AGENTS.md` — the global agent-rules payload deployed to
  `~/.agents/AGENTS.md`; it is not ddu's own rules (these are).

## Build / run

- Linux: `scripts/linux.sh run` (build `--release` + run, workspace = `$DDU_DIR`
  or this repo) and `scripts/linux.sh install`. Details: `docs/RUNNING.md`.
- macOS: `scripts/dev.sh` / `scripts/install.sh` — the only launch paths.
- `cargo test`.

## Rules

- **GPL-free throughout.** Zed's `terminal*`/`mappings` are GPL-3.0: understand
  and rewrite, never paste. Only crates.io artifacts (MIT/Apache) are allowed.
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
- **Scroll regions**: the `.vertical_scrollbar(...)` host must be an un-padded
  `v_flex` ancestor, never the tracked element (`docs/UI.md`).
- **Cached panels**: a panel's subtree replays until notified, so keep the
  three-halves notify discipline (`subscribe_term` / OSC-title /
  `notify_panels`) and give every `root_style()` a size (`docs/UI.md`).
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
  monospace heuristic (the settings picker lists with it too).
- **Highlighting is per row, off the file** (`diff/view.rs` + `ui/code_text.rs`):
  `ViewRow::offset` is the row's byte offset in the file (`None` for a spliced
  deletion, which is never colored), and `RowStream::row_styles` clips a
  whole-file style range down to that row. The parse rides `FileView::build`'s
  background thread; a render never parses. Grammars come from the
  `tree-sitter-*` features in `Cargo.toml` (all MIT — the GPL-free rule), one
  per language, and only the **File** surface highlights: Diff mode's hunks
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
- **The tree lists the working tree lazily** (`diff/tree.rs`): only the root and
  the directories the user has expanded are read (`list_dir`), merged with the
  poll's diff, so a 40k-file repository draws a few hundred rows. Clean files are
  listed (directories first, in the tree's own text color — the figures mark a
  change, not the name's weight), so anything that assumed "a row means a
  changed file" — the selection, the pane's rows, the find bar — goes through
  `AppView::selection` (a path) instead. A file nobody changed renders as the
  file itself (`AppView::surface` folds Diff into File), and a zero figure is
  never printed (`+8`, not `+8 −0`).
- **A restore of the pane's scroll position waits for its own rows**: rows
  mount in stages (the poll's hunks, then the whole-file view), and the virtual
  list clamps an offset to whatever is mounted — applied early, a deep position
  is lost for good. `AppView::pending_scroll` defers it; see `docs/UI.md`.
- **`h_flex()` centers on the cross axis**: `flex_row` + `items_center`, so a
  row that must hand a child the full height states `items_stretch()`
  (`docs/UI.md`).
- **Never `use super::*` in a `src/app/*` test module**: the parent globs
  gpui-kit, and globbing *it* brings gpui's `#[test]` attribute into scope,
  which expands into itself ("recursion limit reached while expanding
  `#[test]`"). Name the imports the test needs.
- **Repaint**: never poll-render — `PumpMsg` events drive it; `subscribe_term`
  is the single subscription point and repaints only the visible session; the
  adaptive `stream_interval` steps must sit above a real frame's paint cost
  (`docs/UI.md`).
- **Keyboard**: app shortcuts are `secondary-` chords (copy/paste/find are
  `⌃⇧` on Linux); a chord a binding claims never reaches the PTY — `⌃P` (the
  quick open) trades readline's previous-history for a file search, pinned by
  `app::tests::the_quick_open_owns_its_chord_in_every_context`. A search bar's
  arrows belong to its input, so the bars step with ⌘G/⌘⇧G — except the
  palette, whose single-line field makes gpui-base's `up`/`down` binding land on
  a handler that was never registered, letting the palette's own binding take
  them (`docs/UI.md`). Any terminal action shown in a menu must wire
  `PopupMenuItem::action` (`docs/UI.md`).
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
