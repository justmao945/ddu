# China mirrors on this host

Host-level note: the mirrors this machine's package managers and toolchains pull
through, not anything ddu's source does. `mise` is the front door here — every
agent CLI on `PATH` is a mise tool — so most of it is one global config file.

| What pulls | Where the mirror is set |
|---|---|
| GitHub release assets (`claude`, `codex`, `opencode`, `pi`, `gh`, `uv`, `github:can1357/oh-my-pi`) | `~/.config/mise/config.toml` → `[settings.url_replacements]` |
| Node runtime tarball | same file → `node.mirror_url` |
| pip / uv / rustup downloads | same file → `[env]` |
| npm packages (`npm:` tools, mise's embedded `aube`, node's own npm) | `~/.npmrc` |
| crates.io (ddu's own build) | `~/.cargo/config.toml` |
| Arch packages | `/etc/pacman.d/mirrorlist` + a `pre-refresh-pacman` hook |

## mise

`~/.config/mise/config.toml` — settings are global, and TOML forbids a parent
table after its subtables, so `[settings]` goes **above** the existing
`[settings.upgrade]` block:

```toml
[settings]
# node tarball + SHASUMS256.txt + .sig
node.mirror_url = "https://npmmirror.com/mirrors/node/"

[settings.url_replacements]
# every tool in [tools] resolves to a GitHub-release backend, so its artifact
# is a github.com/<owner>/<repo>/releases/download/... URL
"https://github.com/" = "https://ghfast.top/https://github.com/"

[env]
UV_DEFAULT_INDEX = "https://pypi.tuna.tsinghua.edu.cn/simple"
UV_PYTHON_INSTALL_MIRROR = "https://ghfast.top/https://github.com/astral-sh/python-build-standalone/releases/download"
PIP_INDEX_URL = "https://pypi.tuna.tsinghua.edu.cn/simple"
RUSTUP_DIST_SERVER = "https://rsproxy.cn"
RUSTUP_UPDATE_ROOT = "https://rsproxy.cn/rustup"
```

### Why `url_replacements`, and what its scope really is

`mise registry` says what each installed name resolves to, and all of them are
release backends (`aqua:`/`github:`) or `npm:` — never a registry that has a
"mirror URL" setting of its own:

```
claude    aqua:anthropics/claude-code   http:claude
codex     aqua:openai/codex             npm:@openai/codex
opencode  aqua:anomalyco/opencode
pi        aqua:earendil-works/pi  github:earendil-works/pi  npm:@earendil-works/pi-coding-agent
gh        aqua:cli/cli                  asdf:bartlomiejdanek/asdf-github-cli
uv        aqua:astral-sh/uv             asdf:asdf-community/asdf-uv  pypi:uv
```

`url_replacements` is the one knob that rewrites them all at once. A plain key
is a **substring match on the full URL** — `https://github.com/` (scheme plus
trailing slash) deliberately misses `api.github.com` and
`github.com.example.org`; `'regex:^https://github\.com/([^/]+)/([^/]+)/releases/download/(.+)'`
narrows it to release assets if the broad rule ever bites. mise logs the
rewrite at `-vv`, which is the cheapest proof a rule is live.

Scope caveat: replacements apply to **mise's own HTTP client only**. Git
clones (asdf plugins, `codeload`), node's npm, `gh api`, and the agent CLIs'
own self-updaters route independently — those need their own config (npm:
below; the rest: a proxy or none).

### `[env]` rather than per-tool config

`UV_PYTHON_INSTALL_MIRROR` is the only supported way to move uv's Python
downloads (the `--mirror` flag's env twin), and `PIP_INDEX_URL` /
`RUSTUP_DIST_SERVER` cover the pip and rustup clients mise launches. Putting
them in `[env]` means one file, applied whenever mise is activated — check with
`mise env`. They are global: a future need to hit upstream PyPI means
overriding the variable for that command.

Deliberately **not** set: `pypi.registry_url`. It is the version-resolution
endpoint (`https://pypi.org/pypi/{}/json` by default), and TUNA's copy of that
path 404s for at least one installed tool while `pypi.org` answers 200 —
turning it on breaks `pipx:` version lookups for a few kilobytes of metadata.
The download bytes are what matter, and those go through the uv/pip index vars.

## npm

```
registry=https://registry.npmmirror.com/
```

One file covers three clients: mise's own npm metadata client, the embedded
`aube` installer, and node's npm (also on `PATH` via the mise node). Full
coverage was proven by pointing the registry at a server that 404s — metadata
lookup (`mise ls-remote npm:prettier`) and installation
(`mise install npm:prettier`) both fail with `package not found`, i.e. neither
bypasses `~/.npmrc`.

## crates.io

`~/.cargo/config.toml`:

```toml
[source.crates-io]
replace-with = "rsproxy-sparse"

[source.rsproxy-sparse]
registry = "sparse+https://rsproxy.cn/index/"
```

rsproxy mirrors both the index and the `.crate` downloads; TUNA and USTC also
serve a sparse index, but TUNA's `config.json` still points `dl` at
`static.crates.io`. `cargo fetch` names the source it used —
`Downloaded foo v1.2.3 (registry 'rsproxy-sparse')` — and the sparse index
lands in `~/.cargo/registry/index/rsproxy.cn-<hash>/`.

## Arch / Omarchy

`/etc/pacman.d/mirrorlist` ships as a single line
(`https://stable-mirror.omarchy.org/$repo/os/$arch`), and the `[omarchy]` repo
points at `pkgs.omarchy.org` — neither has a China mirror, and the channel
(`stable`/`rc`/`edge`) decides which one you get.

The Arch mirrors themselves can be swapped (`aliyun` / `tuna` / `ustc` /
`sjtug` all verified serving `core.db` 200):

```
sudo install -m644 ~/.config/omarchy/mirrors/arch-cn.mirrorlist /etc/pacman.d/mirrorlist
sudo pacman -Syy
```

The catch: `omarchy-refresh-pacman` (and therefore
`omarchy version channel set <channel>`) copies
`$OMARCHY_PATH/default/pacman/mirrorlist-<channel>` over that file. Order in
that script is *copy mirrorlist → run the `pre-refresh-pacman` hook →
`pacman -Syyuu`*, so a hook is the supported place to re-apply the domestic
mirrors and survive every channel switch and `omarchy refresh pacman`:

```
omarchy hook install pre-refresh-pacman ~/.config/omarchy/mirrors/apply-arch-cn-mirrors.sh
```

The hook (`sudo install -m644` of `arch-cn.mirrorlist`, same directory) rides
the sudo timestamp `omarchy-refresh-pacman` already paid for. `omarchy update`
itself does not refresh the mirrorlist, so it never fights the hook. One
migration does read that file (`migrations/1788112314.sh` fixes up an
`rc-mirror.omarchy.org` mirror paired with an edge `[omarchy]` repo); its first
`grep` simply misses a domestic mirrorlist, so it no-ops rather than mis-fixes.

## What was verified

| Check | Expected |
|---|---|
| `mise install gh@latest --force -vv` | `Replaced URL using string replacement 'https://github.com/' … -> https://ghfast.top/…`, then `verify GitHub artifact attestations` |
| `mise install node@26.8.1 --force -vv` | downloads `npmmirror.com/mirrors/node/v26.8.1/…tar.gz` **and** `SHASUMS256.txt` (+`.sig`) |
| `cargo fetch --locked` in this repo | every line `(registry 'rsproxy-sparse')` |
| `npm view prettier dist.tarball` | `https://registry.npmmirror.com/prettier/-/prettier-3.9.8.tgz` |
| `mise settings ls` / `mise env` | the settings and the five exports above |
| `curl -I mirrors.ustc.edu.cn/archlinux/core/os/x86_64/core.db` | 200 |

## Traps

- **A mirror changes where bytes come from, not which bytes.** The checksum and
  GitHub attestation are still validated against upstream: pointing a rule at a
  local server that returns random bytes fails the install with
  `Checksum mismatch … Expected: sha256:b1c2…, Actual: sha256:a668…` and nothing
  lands in `installs/`. The failure came through the `aqua:` path, whose
  lockfile pins the upstream URL and digest — which is why the check still
  fires when the request goes somewhere else.
- **`ghfast.top` is a third party.** It sees which artifact you asked for and
  the bytes it hands back. Per mise's URL-replacement rules a *host-changing*
  rewrite drops the credentials scoped to the original host, so a GitHub token
  is not forwarded there (and a private asset would fail rather than leak) —
  netrc for the mirror host is the documented way to authenticate one. Put
  private assets on a self-hosted mirror/Artifactory rule, or use `HTTPS_PROXY`
  (mise honors the standard proxy variables) instead.
- **GitHub blocked outright** is a different problem from GitHub being slow:
  `mise bootstrap --github-relay …` (`github_relay.*` /
  `MISE_GITHUB_RELAY_SOCKET`) borrows read-only GitHub access over a machine
  that has it, and is the mechanism built for that case.
- **Version lists** do not come from the mirrors: mise asks
  `mise-versions.jdx.dev` (a shared cache in front of GitHub's API). Reachable
  here; `use_versions_host=false` falls back to GitHub directly.
- **Rollback**: drop the three blocks added to `config.toml` (keep
  `[settings.upgrade]` last), `rm ~/.npmrc`, `rm ~/.cargo/config.toml` (both did
  not exist before), and
  `rm ~/.config/omarchy/hooks/pre-refresh-pacman.d/apply-arch-cn-mirrors.sh`.
