# Verifying a change

> The long form of the verification rules in `AGENTS.md`. UI changes are
> proven against the real window on the platform you are on; there is no
> substitute for that on either platform.

## Tests

`cargo test` — includes the terminal regression tests (`plain_text_lands`,
`zsh_prompt_bytes_land` — raw zsh prompt escape bytes must render) and the git
diff tests (`head_diff_sees_edits_and_untracked`).

## macOS — the real window, through `computer`

`bash scripts/dev.sh`, then screenshot / `win.ax()` / `win.click|press|type`
(`screencapture -x -l <windowid>` still works for pixels). Screen Recording,
Accessibility and input belong to the *app* (`dev.just.ddu`, not omp or the
terminal, and a stale grant is why either fails; `SIGNING.md`).

Accessibility is cached per process, so a new grant only applies after the app
is quit and reopened — and the FIRST query after a launch returns a bare tree
(registering the client is what makes gpui build one; the next frame carries
it), so query twice.

What the AX tree exposes is what the app opts into: session rows, diff-tree
rows (`Role::TreeItem` + name + selected/expanded), the terminal grid's visible
text, the find bars (labelled inputs and `1/5` counters) and every button ddu
owns — icon-only ones carry an `accessibility_label`. gpui-component's own
chrome (the settings page nav, its list/tree widgets) exposes nothing, so drive
that by coordinates and keep actions reversible: a click in the terminal pane
types into a live agent.

## Linux — observable state (and pixels when available)

There is no AX tree, and on a Wayland session without a screenshot portal no
pixels either. Verify by observable state instead:

- `hyprctl clients -j` — the window mapped, its pid, size, `class` (must be
  `ddu`, see `RUNNING.md`), and one entry per window (main + settings);
- `ps -eo pid,ppid,cmd` — the PTY child of the ddu pid is the restored/default
  session's shell;
- the state file (`DDU_STATE_PATH`) — what the app persisted, including the
  `live` flags;
- `hyprctl binds -j` — no compositor chord collides with the app's
  accelerators (omarchy/Hyprland grab `SUPER`/`CTRL+ALT` chords, never bare
  `CTRL` or `CTRL+SHIFT`).

A fresh state spawns the default launcher by itself, so the shell child proves
the PTY path without touching the UI.

Pixels and synthesized input do work here when needed:

- `grim -g "<x>,<y> <w>x<h>" out.png` captures a window (geometry from
  `hyprctl clients -j`).
- On Hyprland 0.56 `hyprctl dispatch` wraps its arguments in Lua — bare tokens
  fail (`attempt to call a nil value`). Drive input with `wtype` instead, and
  send a chord with explicit press/release: `wtype -M ctrl -P comma -p comma -m
  ctrl` opens Settings (`⌃,`); the `-k` form did not deliver the chord.
- Launch a supervised instance with `hub start` (never a detached `&`/`nohup`),
  passing `DDU_DIR` / `DDU_STATE_PATH` / `DDU_SETTINGS_PATH` in its env, and
  stop it with `hub stop`.

## Scratch workspace, never the working tree

Make a dirty scratch repo (`git init`, commit, edit) and point the launch at it:

```sh
DDU_DIR=/tmp/scratch DDU_STATE_PATH=/tmp/v.json DDU_SETTINGS_PATH=/tmp/vc.json \
  bash scripts/linux.sh run      # or scripts/dev.sh on macOS
```

A fresh state also starts with every pane closed — the cleanest base for a run.

## Proving liveness

Prove liveness by state change: edit a tracked file → the right diff panel must
show it within ~3 s; compare screenshot hashes across the change.

The diff find bar ("Find in diff") needs a selected file first: ⌘T drops the
file-tree layer in under the sessions, ⌘R opens the changes pane, then click a
row in the tree and ⌘F. The layer toggles make the whole sequence replayable
from a fresh state.
