# UI — GPUI invariants and the measurements behind them

> The long form of the UI rules in `AGENTS.md`. Every item here is a bug that
> was hit and fixed; the numbers are measured, not estimates. Architecture and
> layout live in `DESIGN.md` (§4 tree, §5 state, §11 performance summary).

## Import surface

`use gpui_kit::*;` plus specific component modules — one GPUI surface, never a
second one. Never invent gpui APIs: follow the `gpui-kit 0.6` /
`gpui-component 0.6` / `gpui-pre 0.3.3` sources.

## Metrics, selection, accent

`src/ui/mod.rs::scaled(base)` multiplies every shell geometry value (row
heights, panel widths, tree-layer heights, indents) by
`config::desktop_text_scale()`, so layout tracks the same GTK text scale the
fonts already follow. Selection is `foreground.opacity(0.12)`; the accent
color is reserved for activity.

## Scroll regions

The `.vertical_scrollbar(...)` host must be an **un-padded ancestor**, never
the tracked element itself: the overlay is an `absolute inset_0` *child* of
whatever hosts it, so hosting it on a padded `track_scroll` element counts
that padding as content and leaves phantom scroll range (a scrollbar over a
list that fits). That host must also be a flex container (`v_flex`), or the
`flex_1` scroller inside never gets a bounded height and long content is
clipped instead of scrolling.

## Splitter widths

`shell_state` / `panes_state` (`src/app/mod.rs`) are gpui-base
`ResizableState`s with three quirks that all have to be handled together.

**The sized panel is `flex_none`.** An unsized flex sibling takes a flex
*base* of the whole container (the panel element itself is `size_full`), so a
pane left growable shrinks against that sibling instead of holding the width
it was given — the laid-out width then disagrees with the record the drag's
arithmetic reads.

**The record is re-measured, not nudged.** gpui-base pins a slot at its
*first* measured bounds, and a slot measured while the group had another shape
— the center alone, before the diff pane existed, where it measures the whole
container — keeps that number for good. A drag reads the pair as its starting
widths and hands the changed space to the sibling, so a pair that is stale (or
no longer sums to the container) puts the divider wherever that arithmetic
lands, not at the pointer. `heal_splitter_widths` therefore drops each
splitter's state (`clear`) when its container *or* its slot count changes; the
next layout re-pins both slots against the current shape, with the sized panel
re-asserting its recorded width as its initial size.

**The trigger must be something a drag cannot produce.** A drag redistributes
within one container and never changes the container, which is exactly what
keeps this correction out of a live drag's way: correcting on every render
would revert whatever the pointer just did (the divider that refuses to be
dragged), and correcting against a stale pair caps the divider short of the
pointer (the one that stops following it). Both are pinned by
`a_dragged_splitter_is_not_reverted_by_a_render` and
`the_diff_divider_tracks_the_pointer`, the second driven by real pointer
events on the handle.

Keep the flex slots unpinned besides (`reset_panel` on the region and the
center pane): once every slot is pinned, `adjust_to_container_size`
proportionally *rewrites* every recorded width on each window resize. And when
the window is too narrow to honor a width, the clamped layout is correct —
retrying there would emit `Resized` every frame.

Related: a render is not a repaint. `AppView::apply_snapshot` returns whether
the poll actually moved anything, and the 3 s tick only notifies when it did —
an idle tick that repaints anyway is a visible flash whenever a pane's content
is rebuilt from a cache keyed on the changed state.

## Virtualized lists

Both long lists (the terminal's diff pane rows and the sidebar's file tree) are
`v_virtual_list`s: the item sizes are declared up front and only the visible
slice is built per frame — a 3 000-row tree builds ~8 rows, measured. The tree's
list is short by construction — it holds the directories the user opened, listed
on demand (`file_tree::build_index` over `diff::listing::list_dir`), not every file
in the repository — so its per-frame size table stays small too. Two
consequences:

* The host of a virtual list must give it a **bounded height** (a flex container
  whose `flex_1` slot is definite — see "Scroll regions" above); a plain block
  host leaves the list at content height, and the layer then clips instead of
  scrolling.
* Anything a row needs must be cheap and precomputed: the tree's rows come from
  `AppView::tree_index` (one `build_index` per diff and per expansion toggle —
  each one listing only the open directories), the whole-file view from the
  File-mode cache (one background `FileView::build` per `(path, diff
  generation)`). Render only ever reads them: `file_tree::render` does no IO, and
  neither does `list_dir` — that runs from `rebuild_tree_index`, off the render
  path.

`v_virtual_list` needs the full `Rc<Vec<Size<Pixels>>>` every frame, so a list's
sizes are an O(rows) allocation per frame by design; what must not be O(rows) is
element building, path splitting, stat rollups or file IO.

### The overview strip beside the rows

The changes pane draws an overview in the **scrollbar's own column**
(`diff_panel::scroll_overview`): one mark per run of changed rows, from
`RowStream::marks()` — nothing else. Its rect is the scrollbar thumb's resting
rect, from gpui-base's own numbers (`THUMB_WIDTH` 6px, `THUMB_INSET` 4px in from
the right edge, **unscaled** — so the strip is unscaled too, or it drifts out of
that column as the desktop text scale moves), and it is mounted *before*
`.scrollbar(...)` so the thumb paints over it. It deliberately draws **no
viewport band**: the scrollbar in that same column says where the viewport is,
and a second, coarser one is noise. For the same reason the strip is drawn only
when the stream has marks at all.

Two columns, not one: additions take the left half, removals the right
(`OVERVIEW_COLUMN` = 3px each). A replacement is a deleted line with its added
counterpart a row below, and stacked in a single column the second mark covered
the first — every replacement read as one colour. Side by side, the pair reads as
the change it is.

Its positions are **relative lengths** — a share of the content and of the
stream's own rows — never pixels: the element's height is whatever the layout
hands it, so a position derived from a height measured a frame earlier (the
scroll handle's `bounds()`) drifts on resize, and pinning the element's height
to a stale number trades that drift for a wrong one.

A mark is never shorter than `scaled(5.)`: in a long file a one-line run is a
fraction of a pixel tall, and a mark nobody can see (or click) is not a mark.
The floor is also why the two columns matter — at that height a replacement's
two runs would otherwise overlap instead of sitting beside each other.

The row beside it is why the pane's body is an `h_flex` with
`items_stretch()`: `h_flex()` is `flex_row` + `items_center`, so the rows would
otherwise lay out at their content height in the middle of the pane — the
virtual list was painting a centred sliver until the cross-axis alignment was
stated.

### Positions: `ScrollHandle` state is as real as AppView's

A file's place is a `(working tree, path, mode)` entry in
`AppView::file_positions`, written when the file is left and read when it is
returned to — the same entry serves a file switch and a **session** switch
(the tree is the project's, so another session's visit to the same file is the
same reading of it). Three rules make it hold:

* **A restore waits for the rows it belongs to.** Rows mount in stages — a
  changed file shows the poll's hunks while its whole-file view is built off
  the thread — and the virtual list *clamps* an offset to the content mounted
  at that moment. Apply a deep position early and the clamp is permanent: the
  file reopens at the fallback's height, not at its own row. The restore is
  therefore deferred (`AppView::pending_scroll`) and applied when the build
  lands or the poll settles (`stream_is_final`).
* **A pending restore suppresses the write-back.** While a restore waits, the
  handle's offset still describes the *previous* file, so `remember_scroll`
  leaves the stored entry alone — otherwise the position just asked for would
  be overwritten by the one being left.

* **The write happens before the state it reads is torn down.** The offset lives
  on the pane's shared scroll handle, so `remember_scroll` has to run while the
  outgoing file is still the pane's — which is why every path that moves the
  current session (`select_session`, `spawn_session_of`, `remove_session_row`,
  `add_project_path`, `remove_project`) reads it at the top, before the indices
  move and `adopt_session_diff` resets the pane. A switch that skipped it left the
  outgoing file's place only as good as the last file switch, which is the
  "where was I?" it exists to answer.

Two surfaces keep their place somewhere else, and for the same reason — their
scroll is not the pane's row list:

* **A rendered document.** gpui's text view keeps its scroll in the state the
  element is drawn from, and gpui drops that state the moment the element is
  not rendered — so a `.md` file that is switched away from and back would
  start at the top. The pane therefore holds one `Entity<TextViewState>` per
  `(working tree, path)` (`AppView::documents`) and draws the document from it
  (`TextView::new(&state)`), which is what makes the passage it was left at
  come back with the file; the document's key is never written to
  `file_positions` (`remember_scroll`/`restore_scroll` skip the Preview
  surface), so a position recorded there can only ever describe rows.
* **The file tree.** The tree is the project's, so its place is the project's:
  `Project::tree_scroll` (persisted with the expansion and the height). Every
  path that moves `current_project` reads the live handle first
  (`remember_tree_scroll`, beside `remember_scroll`), and the incoming
  project's place goes back through `AppView::pending_tree_scroll` — a project
  switch drops the poll's snapshot and with it the index, so there are no rows
  to put an offset on until the incoming project's poll lands
  (`rebuild_tree_index`). A save never writes the live offset back while that
  restore is still pending, or the switch's own zeroing would overwrite the
  place being returned to. A switch *inside* a project touches none of this:
  the rows are the same rows, and the handle already holds the place.

## Cached panels

The sidebar, the terminal pane and the title bar's breadcrumb are cached child
views (`panel_view!` in `src/ui/mod.rs`): they mount as
`Entity::cached(panel::root_style())`, so gpui replays a panel's whole subtree
— render, layout, paint, hitboxes, mouse listeners, key contexts, focus —
until that view is notified. The breadcrumb is one because it mirrors the
session title, which agent CLIs spin, and it must not drag the panels into
that repaint.

**The changes pane is not cached**, and must not be: it is the app's only
selectable surface, and gpui's window selection keeps a participant only while
it re-registers — `SelectableText`/`TextView` do that from their own paint, so
a replayed (cached) subtree registers nothing and `finish_frame`'s sweep drops
the participant, taking the selection with it. The symptom was a selection in
the pane blinking off one frame after the drag that made it; for the rendered
document it was worse, because clearing that participant notifies the text
view, so every stream frame rebuilt the pane and painted the whole window
twice (measured 5.5% → 3% CPU, 2× → 1× root renders per stream frame). The
pane re-renders on every frame the window draws — which is what the library
assumes of a selectable surface — and nothing else does.

Three halves to keep in sync, all pinned by tests in `src/app/panels.rs`
(`panel_cache_tests`):

* a stream wakeup notifies `terminal_pane` alone (`subscribe_term`), which is
  what keeps the *cached* panels cached on stream frames — measured with the
  changes pane open: per-frame draw cost −35%, taffy layout −65%, sidebar
  render −92%; the changes pane re-renders per frame regardless (see above),
  so the win is the sidebar's and the breadcrumb's, plus the double frame the
  document surface used to cost;
* a stream frame that *changes the OSC title* (the spinner glyph in the
  sidebar row and the breadcrumb) additionally notifies those two — and
  nothing else, or an agent's spinner tick would rebuild the cached panels 20
  times a second; a *background* row's title change notifies the sidebar alone
  (its row is on screen, its grid is not);
* every other `cx.notify()` on `AppView` fans out through
  `AppView::notify_panels` (an app-level `observe_self`), or a panel whose
  state changed would keep its stale frame.

A panel's cached style must be its own layout box — **including a size**: a
cached box is laid out as a leaf from that style alone (there is nothing to
measure), so one that leaves its cross size to its content collapses to zero
and its replayed content lands wherever the parent centers that empty box
(which is how the title bar's breadcrumb ended up against the bar's bottom
border). Every panel states a size (`size_full` / `h_full`), and each panel
states `root_style()` once so the mount and the panel's root element agree.

## Repaint scope and pacing

Wakeups never poll-render: the reader thread pushes `PumpMsg` events through a
subscriber channel.

`AppView::subscribe_term` is the single subscription point for every spawn
path (new/restart/restore). It repaints the center pane only for the session it
renders (`is_visible_term`): a background row keeps parsing — selecting it
must show current output — but its grid changes nothing on screen, and
several streaming agents would otherwise each add a full-window redraw per
frame (measured 47 fps vs 1 fps with two background streams). Its OSC title is
the exception, and it is on screen either way: the sidebar row shows it, so a
title change notifies the sidebar from a background row too
(`AppView::note_row_title`) — a row left to the duration tick stepped its
spinner there and read as jerky the moment the user switched to a quiet
session. Nothing else follows a background row, and an *unchanged* title
notifies nobody. Exit, the diff poll and interaction notify on their own.

The sidebar's duration reading is the one thing that moves with no input at
all, and it reads in `m`/`h`/`d` units (`AgentSession::elapsed_label`): the tick
sleeps to that reading's next turn-over (`AgentSession::label_change_in`, capped
at a minute by `LABEL_TICK_CAP`, which is what notices a row that appeared since
the last wake) and notifies the **sidebar alone**. It used to notify the app once
a second, and the fan-out above carried that into every cached panel: the
terminal grid and the changes pane were re-rendered 60 times for a string that
had not moved (3 600 times for a two-hour run). Measured on a visible 1 261×1 381
window of this repository, nothing running: **0.58% → 0.08%** of a core, and the
per-second signature — a ~10 ms repaint every second, sixty times the label's own
resolution — is gone from the timeline. The launch seconds cost ~0.2 s in total,
so an idle window that has been up for minutes says nothing about the frames the
user's own scrolling and typing spent.

Stream repaint pacing is adaptive: the pump spaces output-driven repaints by
`stream_interval(paint_ms)` — 50 ms (20 fps) while the terminal element's own
paint is cheap, then 66 / 100 ms once a frame's paint passes 4 / 9 ms
(`STREAM_FRAME_STEPS`, EWMA fed by `TermSession::note_paint_cost`). Frame rate
is the one lever that scales the whole-window redraw (gpui repaints every
primitive each frame); keystrokes, scrolling and selection never pass through
the throttle, so interactive latency is unchanged.

The cost steps have to sit *above* what a real repaint costs, not below: a
full-screen TUI redraw on a 1400×900 retina window measures p50 1.8 ms /
p90 3.6 ms, so the original 2.5 ms first step pinned every agent turn at
15 fps — under the floor, and the stutter was plainly visible. 20 fps in the
pane is fine to watch; the *session list* stutter was a different bug (the
cached row missing the title notify, above).

## Keyboard

Every app shortcut is a `secondary-` chord — `⌘` on macOS, `⌃` on Linux
(`src/app/keys.rs::key_bindings`; the full list is `DESIGN.md` §10). Global
shortcuts live in `AppView::new`; terminal-scoped `⌘C`/`⌘V` (`TermCopy` /
`TermPaste`) double as the right-click menu's shortcut hints via
`PopupMenuItem::action` — and any action shown in a menu, terminal or not,
must wire its action the same way. An item wired only to `on_click` renders
with no chord at all, which reads as a command the keyboard cannot reach
(the tree's and the pane's copy commands were exactly that: they now carry
`CopyFilePath` / `CopyFileContents`, bound in the `⌃⌥` space, and a tree
right-click selects its row so the item and the chord name the same file).
The item's action must also be the chord's **winner**, not merely one of its
bindings: the menu asks for the highest-precedence binding for that action and
renders nothing when the chord's top binding is a different action. The
terminal's Copy item printed no chord at all while its Paste item printed `⌘V`
for exactly that reason — the pane's `input::Copy` sat on the same chord
predicate-less, which ties a named context at the focused element and wins the
later-binding tiebreak; bound `!Terminal` the two are mutually exclusive and
`TermCopy` is the winner inside the terminal
(`app::keys::tests::copy_routes_by_focus`).

Copy/paste/find are the exception to `secondary-`: a terminal shares those
keys with the shell, so on Linux they live in the `⌃⇧` space (`Ctrl+C` must
stay SIGINT). A chord a binding claims never reaches the PTY — gpui's
bubble-phase action dispatch stops propagation before the terminal's key
listener runs — so the Linux `⌃R`, `⌃N`, `⌃O`, `⌃T`, `⌃B`, `⌃W` chords are
ddu's, not readline's, and `⌃P` (`FILE_SEARCH_ACCEL`, the quick open) joins them
knowingly: readline's previous-history for a search over every file. Anything
unbound still reaches the shell. Pin the split with
`app::tests::shell_control_keys_stay_with_the_shell`, and the quick open's chord
with `app::tests::the_quick_open_owns_its_chord_in_every_context`.

All three search bars take their keys the same way: the bar carries
`track_focus(input)` plus a `key_context` (`DiffSearch` / `TerminalSearch` /
`FileSearch`), and the actions are handled on the bar (Enter, Escape — dispatched
by the input itself) or on `AppView` (⌘G/⌘⇧G, so they keep working if focus
drifts mid-search).

The plain arrows belong to the input, which is why the bars step with ⌘G/⌘⇧G —
with one exception the palette exploits. gpui-base binds `up`/`down` for any
input in the deeper `Input` context, but it *registers handlers* for them only
when the field is multi-line: in the palette's single-line field the action is
dispatched into nothing, the chain moves on to the next binding, and the
palette's own — one context shallower — steps the cursor. Pinned by
`app::tests::the_palette_steps_with_the_arrows_the_field_gives_up` (the app's
half; the library's keymap is not visible from a unit test) and exercised end to
end — type, `down`, `up`, Enter — in
`app::diff::tests::quick_open_searches_the_working_tree_and_opens_the_hit`.

## The Keys page's recorder

The settings window's Keys page rebinds a shortcut by recording the next
keystroke, and that cannot go through `on_key_down`: gpui resolves the keymap
*before* key listeners, so pressing `⌘N` while recording would spawn a session
instead of being recorded. The recorder is an app-level keystroke interceptor —
`App::intercept_keystrokes`, which runs before action dispatch and can stop it
(`App::stop_propagation`; `ui/settings/keys.rs::record`) — armed while a row is
recording and gated on `window.focused(cx) == recorder handle`, so the keys of
another window, or of a settings field the user clicked into, are not chords.
The recorder element is focused when the row is clicked and **replaces** the
chord button while it holds that focus (the button that was clicked cannot then
fight it for focus on the same press), and a render-time check in
`SettingsWindow::render` disarms a capture whose recorder lost focus — clicking
anywhere else is a cancel, Escape is another. A refused chord keeps recording
with the reason on the row's second line, so the next press is the next
attempt.

The rules live in `app::keys::captured`, not in the page: a chord must carry
`⌃`, `⌥` or `⌘`/Super (a bare key would be swallowed from every text field and
every PTY), and it must not be one another command or a reserved chord already
holds (`chord_taken_by`, which skips the command being rebound and the reserved
chords it already shares — close-session's `⌘W` is the settings window's too).
The page only prints the answer.

Applying one is **appended, never a rebuild**: gpui has no way to take a binding
back out, but an `Unbind` added *after* a chord hides the earlier bindings for
that action at that chord, so an override layer is the new chord plus an
`Unbind` on every chord it replaces — the builtin ones and the ones the
*previous* layer bound (`app::keys::apply_overrides`, which remembers them).
An `Unbind` is permanent for the bindings *under* it, which is what makes a
reset the harder half: putting a command back on a chord a layer retired is only
possible by binding that chord *again*, later in the list — so a command a layer
has touched is re-stated on the chord it runs on now, whether or not it is
overridden. (Without that, one rebind followed by Reset left the builtin chord
dead for the rest of the session; two rebinds had hidden it.) Pinned by
`ui::settings::keys::tests::a_reset_puts_the_builtin_chord_back`.
`app::keys::install` binds the builtin table exactly once per process
(`AppView::new` can run again when a window is re-created, and a re-added
builtin lands after the layer and out-ranks it — silently resurrecting a
retired chord), then applies the overrides; `update_config` re-applies on every
settings edit, a no-op while the `keys` map is unchanged. Pinned by
`app::keys::tests::a_rebind_replaces_the_chord_it_moved_off` and
`…::a_second_rebind_retires_the_first_layer`, and end to end (record, press,
persist, dispatch) by
`ui::settings::keys::tests::a_recorded_chord_is_stored_and_takes_the_command_over`.

A row keeps the anatomy of `super::item` (title and control on one line, the
text beneath) with one difference: the second line can carry the refusal reason
or the broken-override reason, which is why it is not built through `item()`. It
also passes `on_reset` (`is_dirty` = the command has a `keys` entry), which is
what makes the page's "Reset All" appear while anything is customized, and a
per-row reset sits beside the chord — always rendered, as an empty box when
there is nothing to reset, so the column stays aligned.

## The quick open's palette

⌘P opens a **floating palette** over the workspace (`ui/palette.rs`), not a
search inside the sidebar: the hits are a list of paths, the tree is a tree, and
putting the first inside the second put a sidebar-sized box around an answer
that wants a palette's width. Nothing is revealed and nothing is hidden — the
panels stay exactly as the user left them, and committing a hit selects the file
exactly as a click in the tree does (ancestors opened, pane taking it).

It is two layers: a scrim (`absolute inset_0`, click-away closes, and no scroll
handler of its own so a wheel over it never reaches the panes) and the card
hanging from the top. The card's geometry is a share of the window — `TOP_SHARE`
down, `WIDTH_SHARE` wide, clamped between `MIN_WIDTH` and `MAX_WIDTH` — with the
scaled px bounds following the desktop text scale; the row heights are the rows'
own `scaled()`.

Its list height is **stated, not measured**: one row per hit up to
`VISIBLE_ROWS`, then scrolling, with the cursor what scrolls it
(`scroll_to_item(.., Nearest)` in `file_search_step`). A virtual list measures
nothing, so a host that leaves the height to its content lays out at zero — the
card would render as a field with no hits under it. The card is mounted as the
last child of `#app-root` and before `Root::render_dialog_layer`, so it paints
over every panel and under any dialog opened from it.

**Its universe is a walk of the working tree, not git's view of it**
(`diff/listing.rs::walk_files`): an ignored path (`target/`, `node_modules/`,
`.env`) and any depth of directory are searchable, because "open the file I have
in mind" is not the question `git ls-files` answers. The walk runs on the
**background executor** once per palette open — measured: this repository's 44k
files in ~30 ms, ~4.5 MB of paths, freed when the bar closes — so the bar
answers its first frame from the paths git already knows (the index plus the
poll's diff) and re-ranks under the query as it reads then. A walk that lands
under a user who has already stepped keeps the cursor on its own path; a walk
for a bar that has closed or been reopened since is dropped; and the list goes
with the bar, so a build output tree's tens of thousands of paths are never a
session-long cost.

**Ranking is an index over that list, and it is what keeps a keystroke cheap**
(`PathSearch`). Every tier of the ranking — name prefix, name substring, name
subsequence, path substring, path subsequence — is a substring or subsequence
test, so a needle that extends a prefix can only match a subset of what that
prefix matched, **and can only rank a path the same or worse**: dropping a
needle's tail only loosens a test. So a keystroke is scored against the previous
keystroke's survivors, and skips the tiers those survivors had already failed —
a path that only ever matched as a subsequence of the path is tested for that
one thing, not five. The memo keeps up to `MEMO_DEPTH` prefixes of the branch
being typed (`(needle, survivors)` with each survivor's tier), so backspace
lands on a level instead of the whole list, a re-typed query is a lookup, and
anything that extends nothing memoized (a paste, a cleared field, another word)
falls back to a full pass — **the ranking is identical either way; only the
scan's size changes**, pinned by
`diff::listing::tests::the_prefix_memo_ranks_like_a_search_from_scratch` against
a from-scratch search.

Measured on this repository's 44k paths, one typed query:

| | per keystroke |
| --- | --- |
| no memo (a full pass each) | 63 ms total (`listing.rs`), 73 ms (`diff_panel`), peak 8.4 ms |
| with the memo | **13 ms total**, peak 4.7 ms, last keystrokes 5–60 µs (1 path left of 43,874) |

The remaining costs are honest and bounded: the **first** keystroke of a branch
is a full pass (~2 ms here, ~14 ms at 300k paths), a **pasted** query is one
full pass at its length (~7 ms here), and the memo itself is ≤ 8 levels ×
survivors × 8 B (≈ 1 MB for a real query here, ~2.8 MB worst case, freed with
the list). A repository with a few hundred thousand paths on disk is where the
first keystroke would show (~14 ms, under a frame but not free) — the answer
there is to rank on the background executor with a debounce, as the terminal's
find bar already does, never a smaller universe.

## Terminal glyphs

Box-drawing chars are all vector-drawn except the three diagonals
(`src/terminal/boxart.rs`, pinned by
`the_whole_box_drawing_block_is_vector`): a char left to the font glyph renders
at the font's own weight and bounding box, so anything missed — `┼` was —
disagrees with the vector strokes it meets and a table's crossings come out
heavier than its borders.

## Sidebar hover

The hover slots (`hovered_session` / `hovered_project`) update through
`session_panel::toggle_hover`, never by assigning in the `on_hover` callback:
mouse listeners bubble in reverse paint order, so the row being left reports
its leave AFTER the row being entered reports hover — an unconditional clear
drops the fresh entry and the row's action buttons never appear while moving
down the list.

## Modal dialogs

File/folder pickers must use `rfd::AsyncFileDialog` deferred through
`window.spawn`. The sync picker runs a nested modal runloop inside gpui's click
dispatch; system events fired during the modal (e.g. keyboard-layout change)
re-enter gpui effects while `App` is borrowed → `RefCell already borrowed`
crash.

Confirm dialogs: never hand-roll `DialogFooter` button pairs — use
`ui::dialog_footer(label, id, on_confirm)` (`src/ui/mod.rs`: Cancel-outline +
danger-small shared recipe). Set `.on_ok(...)` alongside the footer when Enter
should confirm. One-off informational dialogs (no footer) are fine inline.

## The rendered document's text

**A virtual font name is not a family.** `all_font_names()` mixes gpui's own
aliases — `.SystemUIFont`, `.ZedMono`, `.ZedSans` — in with the platform's
real faces, and a name the machine does not have is no font at all: gpui
matches a family by exact name against the faces it loaded (no fontconfig
substitution) and silently falls back to the *UI* face. The mono fallback
scans that same list, so on a desktop without the stock `DejaVu Sans Mono` it
picked `.ZedMono` — alphabetically the first alias, and the only name in the
list saying "mono" — which names *Lilex*, an installed family almost nowhere:
the terminal, the diff rows and every inline-code span rendered
**proportional** while the rest of the panels looked normal. `is_mono_family`
now rejects a name that starts with `.` (which is also how that alias left
the settings picker), pinned by
`ui::tests::the_mono_heuristic_rejects_gpui_virtual_names`.

**Inline code can wrap twice — upstream, pending a release.** `InlineFlow`
shapes, wraps and positions each fragment, then hands that fragment's text to
a nested text element whose available width is *exactly* the fragment's
measured width (the code padding is added to size the box and taken straight
back off for the text), and the bounds are device-pixel snapped besides — so
one lost f32 bit in that round-trip (measured: 86.274216px of text, 86.27421px
of width) leaves the nested element a hair narrower than the text it must
hold. It then wraps its *last* break opportunity onto a second line and paints
it at the fragment's own x: the tail lands on the line below (overlapping the
next block, or clipped away) while the line the flow measured stays one line
short. Which spans break is float luck — the same
document breaks different spans when the font changes, and a macOS window
usually breaks none — so it reads as "some text overlaps on Linux only", and
the wrapped line can be *prose*, not just code (a list item's last fragment
wraps the same way). Fixed upstream in longbridge/gpui-kit#3046
(`bfd72443`, `crates/base/src/text/inline_flow.rs`: `whitespace_nowrap()` on
the fragment container, landed 2026-09-11), and the crate is **pinned to that
commit** in `Cargo.toml` — 0.6.1, the newest release, still carries the bug —
which is what the pane renders with (`docs/RUNNING.md`). Verify a future
release's fix the same way the pin was checked: render a document with inline
code inside a wrapped paragraph (this repository's own `AGENTS.md` does) and
look for a span's tail sitting on the line below its box.

**A heading base is a bare px, so headings do not follow the desktop scale.**
`TextViewStyle::heading_base_font_size` is `Pixels`, not `Rems` — gpui-base
ships 14 — and every level is a multiple of it (`h1` `rems(2.)`, `h2`
`rems(1.5)`, `h3` `rems(1.25)`, `h4` `rems(1.125)`, `h5`/`h6` `rems(1.)`,
`base/src/text/node.rs`). A document's prose rides `config::ui_font_size()`
(the theme's font size), so on any desktop whose text scale is not 1.0 the
headings stop tracking the text they head: measured at factor 1.33 — body
18.6px, `h4` 15.75px, `h5`/`h6` 14px, i.e. the levels a document uses for its
sub-sub-sections rendered *smaller* than their own paragraphs.
`ui::document_text_style()` is the component style with the base at the UI
size, and both text-view call sites use it (`diff_panel/body.rs`, plus the
image plugin's prose runs in `markdown.rs` — a run rendered beside an image
must not drift from the document around it). Verify at
`DDU_TEXT_SCALE=1.33`: `h1` is 2× the body, `h4` 1.125×, `h5`/`h6` level with
it (they used to sit below it).
