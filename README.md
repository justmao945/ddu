# Day Day Up

**ddu** is a native macOS workspace for AI coding agents: run **claude**, **codex**, or a plain shell session in any project — several at once — and watch the working tree's git diff update live in the next pane. One window, three panes, everything in view.

> Inspired by [Zed](https://zed.dev) — for the feel of a fast, native UI, not its code. Built on [gpui](https://github.com/zed-industries/zed) and [gpui-kit](https://github.com/longbridge/gpui-kit).

<p align="center">
  <img src="assets/screenshots/main.png" alt="ddu — projects and sessions on the left, agent terminal in the center, live git diff on the right" width="960">
</p>

## What you get

- **Three-pane workspace** — projects and agent sessions on the left, a real terminal in the center, the project's live git diff on the right. Panes are resizable; the side panels collapse when you don't need them.
- **Real terminals, not a text box** — every session runs in a PTY with streaming ANSI color, scrollback, and live resize, so agent CLIs behave exactly as they do in your shell.
- **Live git diff** — working tree vs. HEAD: modified, staged, and untracked files with per-file `+`/`−` stats and the current branch, refreshed automatically as your agents edit.
- **Session lifecycle** — spawn, kill, and restart sessions; get notified when an agent exits or fails. ddu keeps each session's conversation id, so an agent chat can be resumed later (`claude --resume` / `codex resume`).
- **Settings window** — theme, shell, and agent command presets, in a dedicated window (`⌘,`).
- **Persistent workspace** — projects, panel layout, and per-project diff state survive relaunches (`~/Library/Application Support/ddu/`).

## Keyboard shortcuts

| Keys | Action |
| --- | --- |
| `⌘N` | New agent session |
| `⌘O` | Add project (folder picker) |
| `⌘1`…`⌘9` | Select the Nth session in the current project |
| `⌘T` | Toggle the diff file tree |
| `⌘B` | Toggle the sessions sidebar |
| `⌘R` | Toggle the diff panel |
| `⌘W` | Close session |
| `⌘,` | Open settings |
| `⌘C` / `⌘V` | Copy / paste (in terminal) |

## Getting started

Requirements: macOS 13+ and a Rust toolchain. Agent sessions need the `claude` or `codex` CLI on your `PATH`; the plain terminal preset works out of the box.

```sh
git clone https://github.com/justmao945/ddu.git
cd ddu
cargo build --release
scripts/ddu-app.sh
```

`scripts/ddu-app.sh` packages the release binary into `target/ddu.app` and opens it through LaunchServices — always launch the app this way. Set `DDU_DIR` to open a different workspace root (defaults to `~/Code/ddu`).

## Under the hood

[gpui](https://github.com/zed-industries/zed) via [gpui-kit](https://github.com/longbridge/gpui-kit) for UI, [alacritty_terminal](https://crates.io/crates/alacritty_terminal) for the terminal grid and ANSI parsing, [portable-pty](https://crates.io/crates/portable-pty) for PTYs, and [git2](https://crates.io/crates/git2) for diffs. Apache-2.0 throughout, GPL-free.

## License

Apache-2.0
