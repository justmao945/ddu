# Verifying a change

> The long form of the verification rules in `AGENTS.md`. UI changes are
> proven against the real window on the platform you are on; there is no
> substitute for that on either platform.

## Tests

`cargo test` — includes the terminal regression tests (`plain_text_lands`,
`zsh_prompt_bytes_land` — raw zsh prompt escape bytes must render) and the git
diff tests (`head_diff_sees_edits_and_untracked`).

### A PTY in a test is non-deterministic IO

A spawned child's output — and the pump channel closing when its threads exit —
reaches the session's forwarder task *from the `ddu-pty-read` / `ddu-pty-wait`
thread*, and gpui's test scheduler counts executor activity on any thread but
its own test's as failure. One executor serves the whole test process, so the
report lands on whichever test happens to be running when the wake arrives:

```
Detected activity on thread Some("ddu-pty-read") … but test scheduler is
running on Some("<some other test>"). Your test is not deterministic.
assertion `left == right` failed: local task dropped by a thread that didn't
spawn it. Task spawned at src/terminal/session.rs:…
```

then the runnable is dropped on the pump's thread, a destructor panics during
cleanup, and the whole test binary dies with SIGABRT (exit 134). It is a
race — measured on this machine before the fix: 2 of 18 runs aborted, and one
of four in a later sample — which is why a single green run proves nothing
here.

Two rules keep it out, both in the code rather than in a convention:

- **a test that spawns a session ends it** with
  `harness::shutdown(&session, cx)` — kill the child, cancel the forwarder
  task, *join* both pump threads — so no pump thread of a finished test can
  wake anything. `cat` (the harness's child, not a shell) stays silent on
  purpose, so nothing else wakes the forwarder mid-test;
- **`TermSession::spawn` opts the test out of that thread check** by calling
  the executor's per-test `allow_parking()` — upstream's escape hatch for "a
  mix of deterministic and non-deterministic async behavior, such as when
  interacting with I/O in an otherwise deterministic test". Being per-test it
  weakens no other test's checks, which is why the `#[cfg(test)]` call sits
  there instead of at each call site.

The forwarder task and its trailing flush timer are **held, not detached**
(`TermSession::pump_task`, and the `flush_task` local in `session.rs`): a
detached task outlives the session that spawned it, and its waker stays live
for the pumps to fire. Keeping the handle cancels the task with the session.

One trap found the hard way — the teardown *order*. Cancelling the forwarder
is what makes the pumps' wakes inert, but the *waiter* then had nobody to
drain it: capacity is one message, a pending `Wakeup` holds the exit code at
the door, and `send_blocking` parked the thread forever (three tests hung;
diagnosed by printing which pump a bounded join gave up on). The waiter now
retries with a deadline (`EXIT_SEND_PATIENCE`) and drops the code if the
channel never opens, and `PumpThreads::join` is bounded besides — a test must
never hang on a thread that is only hygiene.

Prove it by repetition: `cargo test` 20× in a row and count SIGABRTs (before
the fix: 2–3 of 20; after: 20/20 pass, and `terminal::` 30/30 with no join
timing out).

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
