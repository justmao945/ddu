# File Tree + View Panel (design, not yet implemented)

> Upgrade plan for M5/§8 of `DESIGN.md`: turn the changed-files diff tree into a
> **complete file tree** with the diff folded into it, and turn the diff pane into
> a **view panel** that shows a whole file with its diff merged in, plus a
> **Markdown preview** mode.
> Framework: `gpui-kit = "0.6"` only. License: Apache-2.0, **GPL-free throughout**.

## 1. Goal

* Sidebar lower layer lists **every file in the working tree** — tracked (index)
  and untracked, `.gitignore` respected — as a real directory tree.
* Changed files carry their `+a/−b` figures and tint **inside that same tree**;
  unchanged files are listed, muted, and selectable. One tree, no separate
  "changes" list.
* Right pane becomes a view panel with three modes for the selected file:
  1. **Diff** — today's hunks-only view (kept verbatim),
  2. **File** — the whole file, with added/removed lines tinted in place and the
     deletions spliced back in (a unified, context-complete view),
  3. **Preview** — rendered Markdown (`*.md` / `*.markdown` / `*.mdx`).
* Selection, per-session memory, search, horizontal scrolling and the 3 s
  freshness guarantee all keep working exactly as today.

## 2. Non-Goals

* **No editing.** The view panel is read-only (`DESIGN.md` §2: "read-only file
  tree + diff; editing happens back in the terminal").
* No per-session worktrees / branch scoping (§7 stays project-level HEAD→workdir).
* No syntax highlighting in v1 (see §8.4 — opt-in follow-up, cargo-feature gated).
* No multi-file "review strip"; one selected file at a time.
* Non-git directories keep today's behavior (no tree, "not a repository" note).

## 3. As-is (what the upgrade has to preserve)

| Layer | Where | Shape |
| --- | --- | --- |
| Model | `src/diff/mod.rs` | `GitDiff { branch, files }`, `DiffFile { path, added, removed, hunks, lines_total, truncated }`, `DiffLine { kind: ' ' \| '+' \| '-', old_no, new_no, text }`, `DiffFile::rows()` (= `Header`/`Line` walk), `match_rows`, `MAX_LINES_PER_FILE = 5000`, `SEARCH_MAX_MATCHES = 500` |
| Query | `src/diff/git.rs::head_diff` | `Repository::discover` → `diff_tree_to_workdir_with_index(head_tree, opts)` with `include_untracked(true).recurse_untracked_dirs(true).show_untracked_content(true)`; per-file line cap |
| Flow | `src/app/diff.rs` | 3 s poll (`DIFF_POLL_SECS`, `src/app/mod.rs`), `reload_diff` + `diff_seq` stale guard, `apply_diff` re-pins the selection **by path** through `diff_seed_path`, `DiffSearch` (⌘F) match list = child-row indices |
| Tree UI | `src/ui/diff_tree.rs` | `TreeNode { files, dirs }` built in `render` from `&[DiffFile]`; `tree_stats` rollup; `flatten` → `TreeRow::{File,Dir}`; `guides(depth)` stripes; `dir_row`/`file_row`; collapse set = "present in `diff_tree_closed` means collapsed" (all-open default); `plus_minus`; `TREE_{DEFAULT,MIN,MAX}_H` |
| Pane UI | `src/ui/diff_panel.rs` | `panel_view!` → cached child view; `header` (path + `+a/−b`), `body` (scroll container whose **direct children are the rows**, `into_row(el, content_w)`), `diff_line` (two 36 px number gutters + 14 px sign column, green/red 0.12 tints, yellow search tints 0.30/0.13, `SelectableText` per line), `measure_content_width` (top 16 lines shaped exactly), `hunk_header`, `find_bar` |
| State | `src/app/mod.rs` | `diff`, `diff_error`, `diff_file: Option<usize>`, `diff_seed_path`, `diff_tree_closed: HashSet<String>`, `diff_tree_scroll`, `diff_hunks_scroll`, `sidebar_split_state`, `show_diff_tree`, `diff_tree_height_seed`, `diff_search`, `diff_pane: Entity<...PanelView>` |
| Mount | `src/app/mod.rs:1078`, `src/ui/session_panel.rs:83-129` | `diff_pane.cached(diff_panel::root_style())`; the tree layer is the sidebar's lower splitter slot, `.visible(show_diff_tree)` |
| Persist | `src/config.rs`, `src/app/persist.rs` | per **session**: `selected_file: Option<String>`, `closed_dirs: Vec<String>`, `tree_height: Option<f32>`; `live_tree_height` reads splitter slot 1 |
| Cache contract | `src/ui/mod.rs` `panel_view!`, `AppView::notify_panels` | stream frames notify the terminal pane alone; every other `cx.notify()` fans out to all panels — pinned by `panel_cache_tests` |

Two facts drive the whole design:

* `DiffFile::rows()` order **is** the pane's child order — `match_rows` indices,
  `scroll_to_item`, and the drag-selection `document_order` all ride on it
  (`src/diff/mod.rs`, `src/ui/diff_panel.rs::file_rows` comment). Any new row
  stream must keep that identity.
* The tree is built **inside `render`** today. Fine for a handful of changed
  files, fatal for a 100k-file repository — the build has to move to the
  background snapshot (§4.1, §7).

## 4. Data layer

### 4.1 One snapshot per poll

Replace `head_diff(path) -> GitDiff` with:

```rust
// src/diff/git.rs
pub struct Snapshot {
    pub branch: Option<String>,
    pub diff: GitDiff,              // unchanged shape: hunks + per-file stats
    pub tree: FileTree,             // NEW: the full working-tree listing
}

pub fn snapshot(path: &Path) -> anyhow::Result<Snapshot>;
```

`FileTree` is a **prebuilt** structure (paths split into dirs once, in the
background thread), not a `Vec<String>` re-split on every render:

```rust
// src/diff/tree.rs (new)
pub struct TreeEntry { pub path: String, pub kind: EntryKind, pub added: usize, pub removed: usize }
pub enum EntryKind { Clean, Added, Modified, Deleted, Renamed, Untracked }
pub struct TreeNode { pub entries: Vec<(usize, TreeEntry)>, pub dirs: Vec<(String, TreeNode)> }
pub struct FileTree { pub root: TreeNode, pub entries: Vec<TreeEntry>, pub changed: usize, pub added: usize, pub removed: usize }
```

Sources, in one `git2` pass (no subprocess, no extra workdir walk):

* **Tracked**: `repo.index()?.iter()` (`git2-0.19.0/src/index.rs:375`) — the
  complete index, byte-sorted, includes files deleted in the workdir. Skip
  `mode == 0o160000` (submodule gitlinks) in v1; note it in the empty-state docs.
* **Untracked**: the deltas the diff already computes (`include_untracked(true)`
  + `recurse_untracked_dirs(true)`) whose path is not in the index — this is
  `git ls-files --cached --others --exclude-standard` without the subprocess.
* **Ignored** (`target/`, `node_modules/`, …) never enter: neither call enables
  `include_ignored` (`git2-0.19.0/src/status.rs:129`).
* Status per path: presence in the diff ⇒ `Added`/`Modified`/`Deleted`/`Renamed`
  (git2's `Delta` status), index-only + clean ⇒ `Clean`, diff-only ⇒ `Untracked`.
  `added`/`removed` come from `DiffFile` (0 for clean files).

Cost: one index read (in-memory) plus the status work the poll already does;
the tree build is O(n) string splitting on a background thread. Acceptance on a
100k-file repository: **< 50 ms** for `snapshot`, never on the UI thread.

### 4.2 View rows (`src/diff/view.rs`, new)

The pane's row stream becomes mode-independent, so search, scrolling and the
child-index contract survive all three modes:

```rust
pub enum ViewKind { Context, Added, Removed }
pub struct ViewLine { pub old_no: Option<u32>, pub new_no: Option<u32>, pub kind: ViewKind, pub text: String }
pub enum ViewRow<'a> { Header(&'a str), Line(&'a ViewLine) }
pub enum FileView {
    Text { rows: Vec<ViewLine>, truncated: bool },
    Binary,                    // NUL byte in the first 8 KiB
    TooLarge,                  // > MAX_VIEW_LINES or > MAX_VIEW_BYTES
    Missing,                   // deleted file: workdir has no content
}
pub fn build_view(path: &Path, diff: Option<&DiffFile>) -> FileView;
```

Merge algorithm for a changed file (`diff` present, workdir content readable):

1. Read the file; bail out to `Binary` / `TooLarge` / `Missing` as above.
2. Split into lines on `\n` (strip a trailing `\r` for display).
3. Walk `DiffFile::hunks` with an absolute `next_new` cursor (1-based):
   * before the first line of a hunk, emit `next_new ..= anchor-1` as `Context`
     (`anchor` = the hunk's first line with `kind != '-'`);
   * `' '` → `Context` with the line's own `old_no`/`new_no`; `'+'` → `Added`
     (`old_no: None`); `'-'` → push onto a pending-deletion buffer;
   * flush the pending deletions **immediately before** the next non-deleted row,
     or at end-of-file when the hunk ends deleted (this is the one anchor that
     needs a test: a trailing deletion belongs after the last context line);
4. after the last hunk, emit the remaining workdir lines as `Context`, then any
   still-pending deletions.
5. Invariant, asserted in tests: every workdir line number appears exactly once,
   and `rows.len() == file_lines + deleted_lines`.

Other shapes: an untracked/fully-added file falls out of the same walk (one
`+` block); a clean file is all `Context`; a deleted file returns `Missing` and
the pane renders Diff mode instead (the removed lines live in the hunks).

Caps: `MAX_VIEW_LINES = MAX_LINES_PER_FILE` (5 000, same notice), `MAX_VIEW_BYTES
= 1 MiB`, binary sniff on the first 8 KiB. The view is built **on demand** and
cached in `AppView` against `(path, snapshot generation)` — never per poll.

## 5. UI

### 5.1 Sidebar layer (file tree)

`render` stops calling `build_tree`; it flattens the prebuilt `FileTree` from the
snapshot (render-time cost = visible rows only).

* **Default collapse rule.** The current set means "present = collapsed" and
  defaults to all-open. For a full tree the default inverts: on the first
  snapshot of a session, every directory that has **no changed descendant** is
  seeded into `diff_tree_closed`; directories containing changes start open,
  and their ancestors stay open. New directories that appear later join the
  closed set when they arrive clean. Explicit user toggles always win afterwards
  (`toggle` semantics unchanged: presence in the set = collapsed).
  Net effect: the visible row count starts at "changed subtrees + root level"
  and only grows where the user opens folders — no virtualization needed, and no
  behavior change for the changed-only mental model the app has today.
* **Rows.** `file_row` keeps the edge-to-edge band, `guides(depth)` stripes, the
  type icon (`ui::diff_file_icon`) and the copy-name/copy-path context menu.
  Changed rows keep today's `+a/−b`; clean rows render the same row geometry with
  a muted name and no figures. `dir_row` shows a `● n` changed-descendant badge
  instead of the `+a/−b` rollup, which is meaningless across a whole subtree.
* **Filter.** The layer's summary strip becomes `All 1 204 · Changed 7`
  (two-state toggle, ⌘⇧F). "Changed" is exactly today's tree. Persisted per
  session; "All" is the default once this ships.
* **Refresh.** A changed file appearing/disappearing only changes badges and
  tints — no collapse state is disturbed, so the tree does not jump under the
  user when an agent saves a file.

### 5.2 View panel

* Header: `path` + `+a/−b` + a three-way mode toggle
  (`component::button::toggle::{Toggle, ToggleGroup}` — `button/toggle.rs:34` /
  `:221` — `Size::XSmall`, ids `("view-mode", mode)`),
  plus the find bar overlay unchanged.
* **Diff** mode = `file_rows` exactly as today.
* **File** mode = the same row machinery over `FileView::Text` rows: same
  gutters, same `LINE_CHROME`, same `SelectableText` per line and
  `document_order`, `' '` rows untinted, `+`/`-` rows tinted 0.12. `Missing` /
  `Binary` / `TooLarge` fall back to Diff mode with the existing notice style
  (`empty()` / `hunk_header` band).
* **Preview** mode = `TextView::markdown(id, text)`
  (`gpui-component-0.6.0/src/text/compat.rs:42`) inside the pane's scroll
  container, `.selectable(true).scrollable(true)`, styled through `TextViewStyle`.
  Parsing is `markdown` 1.0 / mdast (a `gpui-component` dependency), so the GFM
  constructs (tables, strikethrough, task lists) come from the parser's defaults;
  `MarkdownExtensions` (`gpui-base-0.6.0/src/text/markdown_ext.rs:177`) is only
  for MDX and custom block nodes and is not needed here. Offered only
  for `md`/`markdown`/`mdx`; selecting another file while Preview is active
  silently renders File mode (the mode survives, so coming back to a `.md` file
  returns to the preview). The find bar is **hidden** in Preview (no match API on
  a rendered document) — ⌘F in Preview reopens in File mode instead.
* Keyboard: ⌘⇧M cycles Diff → File → Preview (bound in `key_bindings()`,
  `src/app/mod.rs`); ⇧⌘F toggles the tree filter. Both get a routing assertion in
  the existing key-binding test.
* Cached-panel discipline is preserved: mode changes mutate `AppView` and
  `cx.notify()` (fan-out through `notify_panels`), so the Markdown parse/screen
  layout runs only when the pane is notified — the stream path still touches the
  terminal pane alone (`panel_cache_tests`).

### 5.3 Empty states

| Condition | Note |
| --- | --- |
| no session | "No active session — select one in the project tree." (unchanged) |
| not a repository | "Not a git repository — no file tree." |
| loading / error | `diff_error` else "Loading files…" |
| no selection | "Select a file in the tree." (unchanged) |
| no changes at all | tree still renders; strip reads `All n · Changed 0` |

## 6. State & persistence

New `AppView` fields:

```rust
pub(crate) snapshot: Option<crate::diff::Snapshot>,   // replaces `diff: Option<GitDiff>`
pub(crate) file_view: Option<(String, u64, crate::diff::view::FileView)>, // path, seq, view
pub(crate) view_mode: ViewMode,                       // Diff | File | Preview
pub(crate) tree_filter: TreeFilter,                   // All | Changed
pub(crate) tree_seeded: bool,                         // default-collapse rule ran for this session
```

`SavedSession` gains `view_mode: Option<String>` and `tree_filter: Option<String>`
(`#[serde(default, skip_serializing_if = "Option::is_none")]`, so old
`state.json` files load unchanged). `selected_file`, `closed_dirs`,
`tree_height` keep their fields but now describe the file tree; `selected_file`
may name a **clean** file, so `apply_diff`'s re-pin must search the full tree
(today it searches `diff.files` only — a stale selection would be dropped).
`diff_tree_closed` keeps holding collapsed dirs (now including clean ones), and
is persisted per session as today (`persist()` reads it into the session slot).

## 7. Performance

* One `git2` pass per 3 s poll; `snapshot` (diff + index union + tree build) runs
  on `cx.background_spawn`, guarded by `diff_seq` as today.
* Render cost is O(visible rows): the tree is flattened from the prebuilt
  `FileTree` (no path splitting, no `tree_stats` recursion — rollups are computed
  once in the snapshot).
* `FileView` is built lazily, once per (path, snapshot seq), on demand.
* The default-collapse rule bounds the initial visible rows to the changed
  subtrees; a user who opens a 20k-file folder pays for those rows only.
  Escape hatch if that still bites: `component::list::{List, ListDelegate,
  ListState}` (`list/delegate.rs:8`) virtualizes uniform-height rows — our rows
  are all `ROW_PX` tall, so it is a drop-in, but it changes the band/scrollbar
  look; only take it behind a measurement (tree render > ~8 ms p50, the same bar
  the terminal pacing uses).
* Markdown: one parse per pane notify; keep the element id stable
  (`("md-preview", path)`) so gpui's element-state cache is reused, and fall back
  to `TextViewState` (`gpui-base-0.6.0/src/text/state.rs:132`) if profiling shows
  re-parsing per frame.

## 8. Implementation checklist

| Phase | Files | Work |
| --- | --- | --- |
| P1 data | `src/diff/git.rs`, `src/diff/tree.rs` (new), `src/diff/mod.rs` | `snapshot()` = diff + index union + prebuilt `FileTree`; `Snapshot`/`TreeEntry`/`EntryKind`; hermetic tests (§9) |
| P2 tree UI | `src/ui/diff_tree.rs`, `src/app/mod.rs`, `src/app/diff.rs` | flatten the prebuilt tree; badges; filter strip ⇧⌘F; default-collapse seeding; selection re-pin against the full tree |
| P3 view | `src/diff/view.rs` (new), `src/ui/diff_panel.rs`, `src/app/diff.rs` | `build_view`, mode toggle + ⌘⇧M, File mode rows, `match_rows` over the merged rows, find bar scoping, `file_view` cache |
| P4 preview | `src/ui/diff_panel.rs` | `TextView::markdown` path, `Preview` mode + extension gate, empty-state fallbacks |
| P5 cleanup | `AGENTS.md`, `docs/DESIGN.md`, `src/diff/mod.rs` | source map + §7/§8 rewritten; delete what the cutover obsoletes (`build_tree`/`tree_stats` in the UI layer, any Diff-mode-only helper, stale comments about "changed files only") |

### 8.4 Syntax highlighting (explicitly deferred)

Available without new dependencies at the data level: `gpui-kit` forwards
`gpui-component`'s grammar features (`gpui-kit-0.6.0/Cargo.toml`), and
`gpui-component::highlighter::SyntaxHighlighter` (`highlighter/highlighter.rs`)
maps a rope + language to style ranges (`SyntaxHighlighter::new("markdown")`,
`.update(...)`, `.styles(range, theme)`). Enabling it means picking
`tree-sitter-*` features (a dozen grammar crates, C build steps, plus a license
audit — `DESIGN.md` §13 mandates the check) and turning style ranges into
`TextRun`s per row. Deferred: v1 ships a correct, fast, uncolored file view.

## 9. Verification

* `cargo test`, extending the existing hermetic style (`src/diff/git.rs` tests use
  a temp repo built with `RepositoryInitOptions`):
  * `snapshot_lists_tracked_untracked_and_skips_ignored` — tracked file, untracked
    file, `.gitignore`d file and directory; exactly the first two are listed,
    with correct statuses.
  * `view_merges_hunks_into_whole_file` — modified file: line numbers, added and
    removed placement, the trailing-deletion anchor, insertion-only hunks, and
    the `rows.len()` invariant.
  * `view_falls_back_for_deleted_binary_and_huge_files`.
  * `view_rows_match_pane_child_indices` — the `rows()`/child-index identity,
    mirroring the existing `match_rows` tests.
  * `panel_cache_tests` extension: a mode switch notifies the diff pane and not
    the sidebar/terminal.
* Visual, following the repo's existing `DDU_VERIFY_*` hooks (`AGENTS.md`):
  `DDU_VERIFY_FILETREE=1` opens the layer with a fixed query state and dumps
  `{rows, changed, collapsed}` to `/tmp/ddu-filetree-verify.json`; the pane
  variant dumps `{mode, rows, first_row_kind, content_w}` — the layout questions
  (row heights, gutters, tint bands) are answered from the dump, not by eye.
* Live acceptance: edit a tracked file in a real session → the tree's badge and
  the pane's rows update within ~3 s; open a `README.md` → Preview renders
  headings, lists, a fenced block and a table.

## 10. Risks

| Risk | Mitigation |
| --- | --- |
| Huge repositories (100k+ files) | Prebuilt tree off the UI thread, default-collapse clean dirs, measured `< 50 ms` snapshot budget, `List` virtualization as the escape hatch (§7) |
| Row-index contract breaks (search jumps to the wrong row, drag selection copies garbage) | The merged row stream reuses `rows()`'s definitional identity; a test pins indices against rendered child order (as `match_rows` already does) |
| Deletion anchoring subtleties in the merge (EOF deletions, multiple delete runs) | Explicit algorithm step + tests for each shape |
| Markdown re-parse per frame | Stable element id + gpui element-state cache, `TextViewState` as the documented fallback; the pane is a cached child view, so parsing only happens on notify |
| Markdown images (README screenshots) | `TextView` renders inline images; verify local relative paths during P4, otherwise document the gap rather than faking it |
| Selection/persistence drift (`selected_file` was diff-only) | Re-pin against the full tree in `apply_diff`; `tree_filter`/`view_mode` default when absent |
| Grammar features pulled in for highlighting | Deferred entirely (§8.4); no `tree-sitter-*` feature enabled in this plan |

## 11. Decisions taken (open to challenge)

1. **Cutover, not coexistence**: the tree replaces `diff_tree`; there is no
   second "changed only" pane — the filter covers that use.
2. Clean directories start collapsed; changed subtrees start open.
3. File mode is the merged, whole-file view; Diff mode stays byte-identical to
   today so nothing regresses while the new mode beds in.
4. Preview is Markdown-only and disables the find bar in that mode.
5. No syntax highlighting, no virtualization, no `notify`-crate file watching in
   v1: the 3 s poll already meets the freshness bar.
