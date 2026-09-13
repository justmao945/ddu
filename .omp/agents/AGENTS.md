# Global Agent Rules

China network: foreign hosts (~40KB/s or blocked). Probe before build/download; use domestic mirrors.

## Background Tasks

- NEVER use `sleep` to wait for anything — jobs, processes, files, network. There is always an event-driven primitive; use it:
  - jobs/subagents: auto-deliver via callback; while pending, do other work. `hub wait` only when fully blocked
  - processes: `hub wait … for=ready|exit`, `pidwait`, or the tool's own readiness/exit signal
  - network: `--max-time` / `timeout` / `AbortSignal` deadlines — never sleep-then-recheck
  - browser/UI: `waitForSelector` / `waitForUrl`-style event waits
- Hung job → cancel once, never re-check.

## Network & Builds

- Probe first, assume foreign fails: `curl -sS -o /dev/null -w "%{http_code} %{speed_download}B/s" --max-time 10 <url>`
- Mirrors:
  - rustup: `RUSTUP_DIST_SERVER=https://rsproxy.cn RUSTUP_UPDATE_ROOT=https://rsproxy.cn/rustup`
  - crates.io: already replaced via `~/.cargo/config.toml` (rsproxy-sparse)
  - npm/bun: `registry.npmmirror.com`
  - PyPI: `pypi.tuna.tsinghua.edu.cn/simple` (`UV_DEFAULT_INDEX` for uv)
  - brew bottles: `mirrors.tuna.tsinghua.edu.cn/homebrew-bottles`; brew API not mirrored → prefer uv/pip wheels
- All network ops need explicit timeouts. 0% CPU + foreign-CDN connection = mirror problem: swap source and retry, never touch build flags.

## Image Reading

- I have native vision: look at images myself whenever possible — user-supplied images, screenshots, `:img`, video frames. NEVER delegate these.
- Delegate to a vision model via `?q=` ONLY when the image itself has no downstream value and just one conclusion is needed (saves context). Fine-grained observation, multi-round interaction, or anything UI-related: look with my own eyes.
