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

Panel widths come from two sources that must not fight: the drag (live, several
updates a second) and the persisted record (which catches up a tick later,
through the `ResizablePanelEvent` subscription). Anything that re-asserts a
recorded width therefore has to be keyed to something a drag cannot produce —
here, **the container size**: the drift it heals (a resize that landed while
every slot was still pinned) always changes the container, and a drag never
does. Correcting on every render instead reverts whatever the pointer just did,
which reads as a divider that refuses to be dragged.

Related: a render is not a repaint. `AppView::apply_snapshot` returns whether
the poll actually moved anything, and the 3 s tick only notifies when it did —
an idle tick that repaints anyway is a visible flash whenever a pane's content
is rebuilt from a cache keyed on the changed state.

## Virtualized lists

Both long lists (the terminal's diff pane rows and the sidebar's file tree) are
`v_virtual_list`s: the item sizes are declared up front and only the visible
slice is built per frame — a 3 000-row tree builds ~8 rows, measured. Two
consequences:

* The host of a virtual list must give it a **bounded height** (a flex container
  whose `flex_1` slot is definite — see "Scroll regions" above); a plain block
  host leaves the list at content height, and the layer then clips instead of
  scrolling.
* Anything a row needs must be cheap and precomputed: the tree's rows come from
  `AppView::tree_index` (one `build_index` per diff and per collapse toggle),
  the whole-file view from the File-mode cache (one background `FileView::build`
  per `(path, diff generation)`). Render only ever reads them.

`v_virtual_list` needs the full `Rc<Vec<Size<Pixels>>>` every frame, so a list's
sizes are an O(rows) allocation per frame by design; what must not be O(rows) is
element building, path splitting, stat rollups or file IO.

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

## Splitter widths

`shell_state` / `panes_state` (`src/app/mod.rs`): gpui-base pins every slot at
its first measured bounds (`update_panel_size`), and once all slots are pinned
`adjust_to_container_size` proportionally *rewrites* every recorded width on
each window resize. Render therefore keeps the flex slots unpinned
(`reset_panel` on the region and the center pane — the adjust then bails) and
re-asserts the recorded sidebar/diff widths when a resize already drifted them
(`resize_panel`, flagged via `suppress_resize_records` so the drag-persist
subscription ignores the synthetic event).

Never let a render-time correction fire unconditionally: when the window is
too narrow to honor a width, the clamped layout is correct and retrying would
emit Resized every frame.

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
ddu's, not readline's. Anything unbound still reaches the shell. Pin the split
with `app::tests::shell_control_keys_stay_with_the_shell`.

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
