# File Tree + View Panel (design)

> Upgrade plan for M5/§8 of `DESIGN.md`: turn the changed-files diff tree into a
> **complete file tree** with the diff folded into it, and turn the diff pane into
> a **view panel** that shows a whole file with its diff merged in (Markdown
> files rendered).
>
> **Landed (2026-09-13, `DESIGN.md` §7/§12):** §4.1's **index union** — the
> sidebar lists the whole working tree (tracked + untracked, `.gitignore`
> respected), with the default-collapse rule
> (`src/diff/view.rs`); the pane's **whole-file** surface with the `⌘⇧M` /
> far-right icon-button switch (per session, persisted); and the virtualization
> §7 asked for — the pane renders a mode-independent `RowStream` through
> `v_virtual_list`, and the tree renders a per-poll `TreeIndex` the same way.
> A Markdown file renders in File mode through gpui-base's own per-block list;
> a source that cannot be read falls back to rows with the same band. > **Landed (2026-09-13, later the same day):** the tree is **lazy** — §4.1's
> index union is gone again, in its place a per-directory `list_dir` the layer
> calls only for the directories it shows. §5.1's strip never survives that
> rewrite — one merged tree, directories before files, every name in the tree's
> own text color (the `+a/−b` figures mark a change, not the name's weight), no
> filter, no counts: `+0 −0` and a file total are noise, and a lazy listing
> cannot count what it has not read. The pane's header carries its figures the
> same way (`+8`, not `+8 −0`) and drops the word "unchanged" for a file with
> none; **one** number gutter numbers the file as it is now (a deleted line
> carries no number) where §5.2 drew two; a file nobody changed renders as the
> file itself rather than an empty hunks pane; and **images** — an image file,
> and `![](…)`/`<img>` inside a rendered document — draw through
> `src/ui/markdown.rs` instead of going to the http client that cannot read a
> path.
> **Not landed:** §8's
> deferred items (syntax highlighting, worktrees). Caps came out at
> `MAX_VIEW_BYTES` 8 MiB / `MAX_VIEW_LINES` 200 000 (not the 1 MiB / 5 000
> guessed here): the pane virtualizes, so a large file costs one build pass.
> Framework: `gpui-kit = "0.6"` only. License: Apache-2.0, **GPL-free throughout**.

## 1. Goal

* Sidebar lower layer lists **every file in the working tree** — tracked (index)
  and untracked, `.gitignore` respected — as a real directory tree.
* Changed files carry their `+a/−b` figures and tint **inside that same tree**;
  unchanged files are listed in the tree's own text color and selectable (the
  figures, not the name's weight, mark a change). One tree, no separate
  "changes" list.
* Right pane becomes a view panel with two modes for the selected file:
  1. **Diff** — today's hunks-only view (kept verbatim),
  2. **File** — the whole file, with added/removed lines tinted in place and the
     deletions spliced back in (a unified, context-complete view). A Markdown
     file (`*.md` / `*.markdown` / `*.mdx`) renders as its document here.
* Selection, per-session memory, search, horizontal scrolling and the 3 s
  freshness guarantee all keep working exactly as today.

## 2. Non-Goals

* **No editing.** The view panel is read-only (`DESIGN.md` §2: "read-only file
  tree + diff; editing happens back in the terminal").
* No per-session worktrees / branch scoping (§7 stays project-level HEAD→workdir).
* No syntax highlighting in v1 (see §8.4 — opt-in follow-up, cargo-feature gated).
* No multi-file "review strip"; one selected file at a time.
* Non-git directories keep today's behavior — no tree, and today the raw
  `Repository::discover` error text lands in `diff_error`; §5.3 gives it a
  curated note instead.

## 3. As-is (what the upgrade has to preserve)

| Layer | Where | Shape |
| --- | --- | --- |
| Model | `src/diff/mod.rs` | `GitDiff { branch, files }`, `DiffFile { path, added, removed, hunks, lines_total, truncated }`, `DiffLine { kind: ' ' \| '+' \| '-', old_no, new_no, text }`, `DiffFile::rows()` (= `Header`/`Line` walk), `match_rows`, `MAX_LINES_PER_FILE = 5000`, `SEARCH_MAX_MATCHES = 500` |
| Query | `src/diff/git.rs::head_diff` | `Repository::discover` → `diff_tree_to_workdir_with_index(head_tree, opts)` with `include_untracked(true).recurse_untracked_dirs(true).show_untracked_content(true)`; per-file line cap |
| Flow | `src/app/diff.rs` | 3 s poll (`DIFF_POLL_SECS`, `src/app/mod.rs`), `reload_diff` + `diff_seq` stale guard, `apply_diff` re-pins the selection **by path** through `diff_seed_path`, `DiffSearch` (⌘F) match list = child-row indices |
| Tree UI | `src/ui/diff_tree.rs` | `TreeNode { files, dirs }` built in `render` from `&[DiffFile]`; `tree_stats` rollup; `flatten` → `TreeRow::{File,Dir}`; `guides(depth)` stripes; `dir_row`/`file_row`; collapse set = "present in `diff_tree_closed` means collapsed" (all-open default); `plus_minus`; `tree_{default,min,max}_h()` (scale-aware) |
| Pane UI | `src/ui/diff_panel.rs` | `panel_view!` → cached child view; `header` (path + `+a/−b`), `body` (scroll container whose **direct children are the rows**, `row_box(el, w, h)`), `diff_line` (one number gutter measured per file by `gutter_width()`, numbering the file as it is now + a sign column (`LINE_CHROME_EXTRAS`), green/red 0.12 tints, yellow search tints 0.30/0.13, `SelectableText` per line), `measure_content_width` (top 16 lines shaped exactly), `hunk_header`, `find_bar` |
| State | `src/app/mod.rs` | `diff`, `diff_error`, `diff_file: Option<usize>`, `diff_seed_path`, `diff_tree_closed: HashSet<String>`, `diff_tree_scroll`, `diff_hunks_scroll`, `sidebar_split_state`, `show_diff_tree`, `diff_tree_height_seed`, `diff_search`, `diff_pane: Entity<...PanelView>` |
| Mount | `src/app/mod.rs:1041`, `src/ui/session_panel.rs:118-130` | `diff_pane.cached(diff_panel::root_style())`; the tree layer is the sidebar's lower splitter slot, `.visible(show_diff_tree)` |
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

> **Superseded by the lazy listing.** `Snapshot` carries the diff and nothing
> else; the poll does not read the working tree at all. What the tree needs from
> the diff — "is this path changed", "how many changed files are under this
> directory" — is answered off the diff's own records, path-sorted
> (`diff/tree.rs::Changes`), so a directory's children can be listed from the
> filesystem on demand (`list_dir`) and merged with the changes as they are
> read. Measured on a 40k-file repository: the poll drops from 70.7 ms to
> 57 ms (the tree's share, 17.8 ms, plus the snapshot comparison it forced,
> ~2 ms), and the ~5 MB of paths the eager `FileTree` held are gone. The sketch
> below is kept as the design that led there.

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
5. Invariant, asserted in tests: every workdir line number appears exactly once
   and `rows.len() == hunks + file_lines + deleted_lines` (the stream keeps
   `/\*\* @@ … @@ \*\/` headers as rows, so they count too).

Other shapes: an untracked/fully-added file falls out of the same walk (one
`+` block); a clean file is all `Context`; a deleted file returns `Missing` and
the pane renders Diff mode instead (the removed lines live in the hunks).

Caps: `MAX_VIEW_LINES = MAX_LINES_PER_FILE` (5 000, same notice), `MAX_VIEW_BYTES
= 1 MiB`, binary sniff on the first 8 KiB. The view is built **on demand** and
cached in `AppView` against `(path, snapshot generation)` — never per poll.

## 5. UI

### 5.1 Sidebar layer (file tree)

`render` reads a `TreeIndex` the app rebuilds off the render path; the index is
the root plus the **expanded** directories, each listed on demand
(`build_index(open, list)`, `list` = `tree::list_dir`). Nothing else is read:
the layer's cost is the visible tree, not the repository.

* **Default expansion.** The set means "present = expanded" and starts empty.
  On the first snapshot of a session, `seed_open` opens every directory on the
  way to a changed file; everything else is one click away. A clean repository
  therefore opens folded — one root listing. User toggles win afterwards.
* **Rows.** `file_row` keeps the edge-to-edge band, `guides(depth)` stripes, the
  type icon (`ui::diff_file_icon`) and the copy-name/copy-path context menu.
  Changed rows carry `+/−` figures and a bright name; clean rows the same
  geometry in the tree's own text color and no figures. **A zero side is not printed**
  (`+8`, never `+8 −0`; a binary or empty change prints nothing at all), and
  `dir_row` shows a `● n` changed-descendant badge — never a file count, which
  the lazy listing cannot know and the user does not need.
* **No filter.** One tree with the diff merged in: the `All / Changed` toggles
  are gone (with the counts they carried), and so is their chord. "Changed only"
  is what the default expansion already gives.
* **Refresh.** A changed file appearing/disappearing only changes badges and
  tints — no expansion state is disturbed, so the tree does not jump under the
  user when an agent saves a file.

### 5.2 View panel

* Header: `path`, then the change figures (a zero side left out), then **one
  icon button at the far right**, wearing the surface it switches to
  (document = whole file, two-versions = hunks), tooltip + `⌘⇧M`, plus the find
  bar overlay unchanged. Labeled toggle groups were the wrong trade at the
  pane's width: three labels ellipsized the file's own name.
* **Diff** mode = `file_rows` exactly as today — and for a file nobody
  changed, `AppView::surface` folds Diff into File, so a clean `.rs` shows its
  source and a clean `README.md` its rendered document instead of empty hunks.
* **File** mode = the same row machinery over `FileView::Text` rows: same
  gutter, same `LINE_CHROME_EXTRAS`, same `SelectableText` per line and
  `document_order`, `' '` rows untinted, `+`/`-` rows tinted 0.12. `Missing` /
  `Binary` / `TooLarge` fall back to Diff mode with the existing notice style
  (`empty()` / `hunk_header` band).
* **Images.** `src/ui/markdown.rs` takes over the *block* holding an image (a
  paragraph, or a raw HTML block with `<img>`) and renders it here: the prose
  through the same Markdown view, the picture through `img(PathBuf)`
  (`Resource::Path`), resolved against the document's own directory. Without it
  every image went to `ImageSource::Resource(Resource::Uri)` — the app's
  **http** client, which cannot read a file — so a README's screenshots rendered
  as nothing. An image *file* selected in the tree is drawn the same way
  (`FileView::Image`, fitted with `Contain`) instead of banding "binary". A
  block plugin is the only hook this text view offers (inline custom nodes are
  unsupported), which is why a paragraph is the unit and its formatting is
  re-rendered as a nested view rather than edited in place.
* The rendered document = `TextView::markdown(id, text)`, in File mode
  (`gpui-component-0.6.0/src/text/compat.rs:42`) inside the pane's scroll
  container, `.selectable(true).scrollable(true)`, styled through `TextViewStyle`.
  Parsing is `markdown` 1.0 / mdast (a `gpui-component` dependency), so the GFM
  constructs (tables, strikethrough, task lists) come from the parser's defaults;
  `MarkdownExtensions` (`gpui-base-0.6.0/src/text/markdown_ext.rs:177`) is only
  for MDX and custom block nodes and is not needed here. Offered only
  for `md`/`markdown`/`mdx`. The find bar has no match API over a rendered
  document, so ⌘F over one switches to Diff, whose rows it can match.
* Keyboard: ⌘⇧M toggles Diff ⇄ File (bound in `key_bindings()`,
  `src/app/mod.rs`), and the header's far-right icon button is the same toggle;
  ⇧⌘F toggles the tree filter. Both get a routing assertion in the existing
  key-binding test.
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
| no changes at all | tree still renders (root listing, folded); the strip's figures disappear rather than reading `+0 −0` |
| image selected | drawn fitted to the pane (`Contain`), no band |

## 6. State & persistence

New `AppView` fields:

```rust
pub(crate) snapshot: Option<crate::diff::Snapshot>,   // replaces `diff: Option<GitDiff>`
pub(crate) file_view: Option<(String, u64, crate::diff::view::FileView)>, // path, seq, view
pub(crate) view_mode: ViewMode,                       // Diff | File
pub(crate) diff_tree_open: HashSet<String>,           // expanded dirs (the layer is lazy)
pub(crate) repo: Option<(PathBuf, Rc<Repository>)>,   // for the tree's listings
pub(crate) tree_seeded: bool,                         // default-expansion rule ran for this session
```

`SavedSession` carries `view_mode: Option<String>` and `open_dirs: Vec<String>`
(`#[serde(default, skip_serializing_if = …)]`, so old `state.json` files load
unchanged — the pre-lazy `closed_dirs` and `tree_filter` keys are simply
ignored). `selected_file` may name a **clean** file: `apply_snapshot` keeps the
selection when the path is in the diff or still on disk, since the tree no
longer holds a list to look it up in.

## 7. Performance

* One `git2` walk per 3 s poll; `snapshot` (the diff, and nothing else) runs on
  `cx.background_spawn`, guarded by `diff_seq` as today. Measured at 40k files:
  57 ms, of which ~29 ms is libgit2's workdir scan.
* Render cost is O(visible rows): the index lists the expanded directories only
  (measured: 31 directory listings and 71 rows for a repository whose 1 200
  files sit behind unopened folders), and the per-directory queries are binary
  searches over the diff's own records.
* `FileView` is built lazily, once per (path, snapshot seq), on demand — and
  only for the **surface** the pane is showing (a clean file's Diff mode is the
  file's own view; an image needs a build for its kind alone).
* The default-collapse rule bounds the initial visible rows to the changed
  subtrees; a user who opens a 20k-file folder pays for those rows only.
  Escape hatch if that still bites: `component::list::{List, ListDelegate,
  ListState}` (`list/list.rs:706` / `:70`, trait `list/delegate.rs:10`)
  virtualizes uniform-height rows — ours are all `row_px()` tall, so it is a
  drop-in, but it changes the band/scrollbar look; only take it behind a
  measurement (tree render > ~8 ms p50, the same bar the terminal pacing uses).
* Markdown: one parse per pane notify; keep the element id stable
  (`("md-preview", path)`) so gpui's element-state cache is reused, and fall back
  to `TextViewState` (`gpui-base-0.6.0/src/text/state.rs:86`) if profiling shows
  re-parsing per frame.

## 8. Implementation checklist

| Phase | Files | Work |
| --- | --- | --- |
| P1 data | `src/diff/git.rs`, `src/diff/tree.rs` (new), `src/diff/mod.rs` | `snapshot()` = diff + index union + prebuilt `FileTree`; `Snapshot`/`TreeEntry`/`EntryKind`; hermetic tests (§9) |
| P2 tree UI | `src/ui/diff_tree.rs`, `src/app/mod.rs`, `src/app/diff.rs` | flatten the prebuilt tree; badges; filter strip ⇧⌘F; default-collapse seeding; selection re-pin against the full tree |
| P3 view | `src/diff/view.rs` (new), `src/ui/diff_panel.rs`, `src/app/diff.rs` | `build_view`, the mode switch + ⌘⇧M, whole-file rows, `match_rows` over the merged rows, find bar scoping, `file_view` cache |
| P4 preview | `src/ui/diff_panel.rs` | `TextView::markdown` path, the Markdown gate on File mode, empty-state fallbacks |
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
  * `view_rows_match_pane_child_indices` — the row-index/search-index identity
    over the merged stream (`deep_rows_are_indexed_and_searchable` covers it at
    20k rows today).
  * `panel_cache_tests` extension: a mode switch notifies the diff pane and not
    the sidebar/terminal.
* Visual, through the `computer` device (macOS; `VERIFICATION.md`): drive the
  real window — open the layer, set the query, screenshot, read element bounds —
  and expose the tree's rows to AX the way the session rows already are
  (`div.role(..)` plus `aria_label`/`aria_selected`), so the layout questions
  (row heights, gutters, tint bands) are answered from the tree and pixels, not
  by eye.
* On Linux there is no AX tree: answer the same questions from observable state
  (`hyprctl clients -j` for geometry, `grim -g "<x>,<y> <w>x<h>"` for pixels,
  `DDU_STATE_PATH` for what persisted) — `VERIFICATION.md`.
* Live acceptance: edit a tracked file in a real session → the tree's badge and
  the pane's rows update within ~3 s; open a `README.md` → File mode renders
  headings, lists, a fenced block and a table.

## 10. Risks

| Risk | Mitigation |
| --- | --- |
| Huge repositories (100k+ files) | Prebuilt tree off the UI thread, default-collapse clean dirs, measured `< 50 ms` snapshot budget, `List` virtualization as the escape hatch (§7) |
| Row-index contract breaks (search jumps to the wrong row, drag selection copies garbage) | The merged row stream reuses `rows()`'s definitional identity; a test pins indices against rendered child order (as `match_rows` already does) |
| Deletion anchoring subtleties in the merge (EOF deletions, multiple delete runs) | Explicit algorithm step + tests for each shape |
| Markdown re-parse per frame | Stable element id + gpui element-state cache, `TextViewState` as the documented fallback; the pane is a cached child view, so parsing only happens on notify |
| Markdown images (README screenshots) | Landed as a block plugin (`src/ui/markdown.rs`): local paths resolve to `Resource::Path`, remote URLs keep gpui's loader, a missing local file shows its alt text |
| Selection/persistence drift (`selected_file` was diff-only) | Re-pin against the full tree in `apply_diff`; `tree_filter`/`view_mode` default when absent |
| Grammar features pulled in for highlighting | Deferred entirely (§8.4); no `tree-sitter-*` feature enabled in this plan |

## 11. Decisions taken (open to challenge)

1. **Cutover, not coexistence**: the tree replaces `diff_tree`; there is no
   second "changed only" pane — the default expansion covers that use, and the
   `All / Changed` filter was dropped rather than kept beside a lazy listing
   that cannot count what it has not read.
2. Clean directories start collapsed; changed subtrees start open.
3. File mode is the merged, whole-file view; Diff mode stays byte-identical to
   today so nothing regresses while the new mode beds in.
4. Markdown rendering is not a mode: File mode renders those files, and the
   find bar leaves File mode for Diff rather than matching a rendered
   document.
5. No syntax highlighting, no virtualization, no `notify`-crate file watching in
   v1: the 3 s poll already meets the freshness bar.
