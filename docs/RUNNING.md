# Running ddu

> The long form of the "Running" rules in `AGENTS.md`: build/install commands,
> the macOS launch constraints, and why they exist. Signing and TCC grants are
> in `SIGNING.md`.

## The gpui stack is pinned to one commit

the workspace `Cargo.toml` carries a `[patch.crates-io]` entry pinning `gpui-kit` — and with
it the four crates that then resolve from that checkout (`gpui-base`,
`gpui-component`, `gpui-kit-assets`, `gpui-component-macros`) — to `bfd72443`
on `longbridge/gpui-kit`: the commit that stops a rendered document's inline
code from wrapping twice (`docs/UI.md`, "The rendered document's text").
crates.io's newest release, 0.6.1, predates it, so the first build fetches the
rev from GitHub and needs the network; `Cargo.lock` records the rev, so every
build after that is offline and byte-reproducible. When a release past 0.6.1
lands, delete the section and `cargo update -p gpui-kit`.

## Linux (X11 / Wayland)

- **Dev**: `scripts/linux.sh run` — builds `--release`, then runs the binary
  from `$DDU_DIR` (default: this repo). `initial_projects()` seeds from the
  process cwd, so the launch directory *is* the workspace. `DDU_STATE_PATH` /
  `DDU_SETTINGS_PATH` isolate a test workspace.
- **Install**: `scripts/linux.sh install` — copies the binary to
  `~/.local/bin/ddu` (prefix: `DDU_INSTALL_DIR`), installs `assets/icon.svg`
  into the hicolor theme and writes `~/.local/share/applications/ddu.desktop`
  with `Path=` set to the launch directory (a desktop launch would otherwise
  start in `$HOME`).
- **The build is CPU-capped**: both paths build inside a systemd user scope
  carrying a `CPUQuota`, because a release build otherwise pins every core for
  the whole compile and the desktop stops answering. The cap is a cgroup
  property, so it covers the per-crate LLVM thread pools that `cargo -j` (a
  count of rustc *processes*) cannot, and it ends with the build — the app
  `run` execs afterwards is outside it. Default: every core but two, which
  scales with the machine and measured 5.96 of 8 cores against 7.62 uncapped
  on a rebuilt crate. `DDU_BUILD_CPU_QUOTA` overrides it with a systemd
  percentage (`400%`, the suffix is required) or `off`; a host with no
  systemd user session falls back to an uncapped build and says so.
- **Window identity**: every `WindowOptions` opens with
  `config::window_app_id()` — on Linux the Wayland `app_id` / X11 `WM_CLASS`
  `ddu`, which must equal the desktop entry's basename (`ddu.desktop`) for the
  menu launch to find its window: icon, taskbar grouping and `class:ddu`
  window rules all key off it. gpui leaves the identity unset on its own, so
  the window otherwise arrives as an anonymous client (empty `hyprctl clients`
  class). `None` on macOS, where the bundle carries the identity. Check a
  launch with `hyprctl clients -j | jq '.[] | select(.pid==<ddu pid>) | .class'`.
- **No bundle, no LaunchServices, no signing** — a plain binary whose window
  the compositor maps normally. Nothing here to keep in sync with the macOS
  bundle flow: the two scripts write different paths and never collide.
- **State**: `$XDG_CONFIG_HOME/ddu/` (default `~/.config/ddu/`).

## Shell default

`config::default_shell()` takes `$SHELL` only when it names a real file, then
the first installed of `/bin/bash`, `/usr/bin/bash`, `/bin/sh`, `/bin/zsh`.
Never hardcode a shell: a minimal environment (a desktop entry, a session
without `SHELL` exported) otherwise spawns a missing `/bin/zsh` and every
restored session dies with a spawn ENOENT.

## macOS 26 — LaunchServices only

- **Dev**: `scripts/dev.sh` — builds, bundles `target/ddu-dev.app` as "Day Day
  Up Dev" (`dev.just.ddu.dev`), isolates state under
  `~/Library/Application Support/ddu-dev/`, then `open`s it.
- **Install**: `scripts/install.sh [--open]` — builds and bundles straight
  into `/Applications` as "Day Day Up" (`dev.just.ddu`; `DDU_INSTALL_DIR`
  overrides). Shared builder: `scripts/make-bundle.sh`. The distinct bundle
  ids let dev and installed run side by side without LaunchServices activating
  the wrong one; `open` never mixes their state either.
- **Replacing a running copy**: both scripts go through `scripts/lib.sh`
  (`app_pids` / `stop_app`, shared by `dev.sh` and `install.sh`). NEVER
  `pgrep`/`pkill` here — they hide the caller and all its ancestors, and the
  app being replaced is usually an ancestor (these scripts are run from a ddu
  terminal session), so the kill silently no-ops and `open` just re-activates
  the stale instance. `stop_app` refuses (status 2) in that ancestor case: the
  caller is running inside the app, so killing it would take the shell down
  mid-script. The generated `launch.sh` must keep `exec` as its LAST line —
  anything appended after it never runs.
- **Signing**: `make-bundle.sh` signs with a local self-signed code-signing
  identity (`Day Day Up Local Signing`; `scripts/make-signing-identity.sh`
  provisions it, `DDU_SIGN_IDENTITY` overrides). It signs `ddu.bin` first with
  the bundle id as its identifier, then the bundle without `--deep`. Do not
  swap the identity or regenerate the certificate casually: macOS stores the
  app's grants (Screen Recording, Accessibility) against the
  *designated requirement*, so only a stable certificate keeps them across
  rebuilds — `SIGNING.md`.

**Never** start the binary directly as a background child (`nohup`, `hub exec`,
raw spawn) — on macOS 26 an unactivated process: (a) never gets
`NSWindowOcclusionStateVisible` (it reports a private on-screen bit `0x2000`
instead), (b) never receives `windowDidChangeOcclusionState`, (c) cannot
self-activate. gpui's display link is the sole frame driver and its start guard
requires the Visible bit, so a background-launched window freezes after its
first frame and no click/activation path recovers it deterministically.
`cargo run` from a terminal hits the same wall — use the script.

The bundle launcher cds to `DDU_DIR` (default `~/Code/ddu`) because
LaunchServices starts apps with cwd `/`; `initial_projects()` uses cwd.

**Do not vendor-patch gpui-pre-macos.** The registry crate (0.3.3, latest) is
kept pristine; the launch convention above is the chosen fix. (A
`schedule_frame` bypass and an occlusion-guard patch were tried and reverted.)
