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
on demand (`diff_tree::build_index` over `diff::tree::list_dir`), not every file
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
  generation)`). Render only ever reads them: `diff_tree::render` does no IO, and
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
returned to. Two rules make it hold:

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

## Cached panels

The three shell panels are cached child views (`panel_view!` in
`src/ui/mod.rs`): sidebar, changes pane and terminal pane mount as
`Entity::cached(panel::root_style())`, so gpui replays a panel's whole subtree
— render, layout, paint, hitboxes, mouse listeners, key contexts, focus —
until that view is notified. The title bar's breadcrumb is one too
(`ui/title_bar.rs`): it mirrors the session title, which agent CLIs spin, and
it must not drag the panels into that repaint.

Three halves to keep in sync, all pinned by tests in `src/app/mod.rs`:

* a stream wakeup notifies `terminal_pane` alone (`subscribe_term`), which is
  what keeps the panels cached on stream frames — measured with the changes
  pane open: per-frame draw cost −35%, taffy layout −65%, sidebar render −92%;
* a stream frame that *changes the OSC title* (the spinner glyph in the
  sidebar row and the breadcrumb) additionally notifies those two — and
  nothing else, or an agent's spinner tick would rebuild the changes pane 20
  times a second;
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
path (new/restart/restore). It repaints only for the session the center pane
renders (`is_visible_term`): a background row keeps parsing — selecting it
must show current output — but its output changes nothing on screen, and
several streaming agents would otherwise each add a full-window redraw per
frame (measured 47 fps vs 1 fps with two background streams). Exit, attention,
the diff poll and interaction notify on their own; a background row picks up
its OSC title/status on the next repaint.

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
(`src/app/mod.rs::key_bindings`; the full list is `DESIGN.md` §10). Global
shortcuts live in `AppView::new`; terminal-scoped `⌘C`/`⌘V` (`TermCopy` /
`TermPaste`) double as the right-click menu's shortcut hints via
`PopupMenuItem::action` — any new terminal action shown in a menu must wire its
action the same way.

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
