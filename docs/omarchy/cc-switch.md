# cc-switch: Claude Code through the local proxy

`claude` (Claude Code) talks the Anthropic Messages protocol: it POSTs to
`$ANTHROPIC_BASE_URL/v1/messages`. The Command Code provider
(`https://api.commandcode.ai/provider`) splits its models across two
endpoints:

- `/v1/messages` — Anthropic-shape, **Claude models only**, and every
  `claude-*` id is gated by plan (`403 MODEL_NOT_IN_PLAN` on this account).
- `/v1/chat/completions` — OpenAI-shape, the open models (`deepseek/*`,
  `moonshotai/*`, `zai-org/*`, …). Sending an open model to `/v1/messages`
  answers `400 invalid_request_error: Model "…" is not supported on this
  endpoint`.

So a provider whose models are open models **cannot** be pointed at directly
from Claude Code. cc-switch's local proxy is the translator: it listens on
`127.0.0.1:15721`, accepts Anthropic-shape `/v1/messages`, converts to
`/v1/chat/completions` upstream, and converts the reply back. The provider row
already declares this (`meta.apiFormat = "openai_chat"`), which is what makes
the conversion happen.

## Enable the route

```sh
cc-switch proxy enable -a claude     # route on: starts the worker, writes the live config
cc-switch proxy show -a claude       # worker pid, port, active routes
cc-switch provider current -a claude # provider + model mapping
```

`proxy enable` rewrites `~/.claude/settings.json` into takeover form — the real
upstream URL and key stay in the cc-switch provider row:

```json
"ANTHROPIC_BASE_URL": "http://127.0.0.1:15721",
"ANTHROPIC_AUTH_TOKEN": "PROXY_MANAGED",
"ANTHROPIC_DEFAULT_SONNET_MODEL": "claude-sonnet-4-6",
"ANTHROPIC_DEFAULT_SONNET_MODEL_NAME": "deepseek/deepseek-v4.1-flash"
```

Claude Code asks for a `claude-*` alias, the proxy maps it to the upstream
open model. `provider switch <id> -a claude` keeps the localhost wiring (the
switch lands in the proxy runtime, not the live config), so switching providers
or models under takeover is safe.

### What each variable does

Measured against v2.1.273 with a scratch `CLAUDE_CONFIG_DIR` and a local
request dump:

- `ANTHROPIC_DEFAULT_<ALIAS>_MODEL` — the model **id on the wire**. Set
  `ANTHROPIC_DEFAULT_OPUS_MODEL=WIRE-ID` and the request body carries
  `"model": "WIRE-ID"`.
- `ANTHROPIC_DEFAULT_<ALIAS>_MODEL_NAME` / `_MODEL_DESCRIPTION` — **display
  only**: the label and description of that alias's row in `/model` (the picker
  lists `LABEL · DESCRIPTION` instead of the stock `Custom <alias> model`). They
  never reach the request. The banner and `/status` show the id from `_MODEL`,
  not this.
- `ANTHROPIC_MODEL` outranks the alias defaults: with both set, the wire id is
  `ANTHROPIC_MODEL`; the alias pairs apply when it is unset.
- `settings.json` `env` outranks the shell environment — exporting
  `ANTHROPIC_BASE_URL` while takeover is on does *not* reroute `claude`.
- `[1m]` appended to a model name (or `CLAUDE_CODE_MAX_CONTEXT_TOKENS`) is how
  the assumed context window is raised.

So in this setup the id (`claude-sonnet-4-6`) is what Claude Code asks for and
what cc-switch maps upstream, while the name
(`deepseek/deepseek-v4.1-flash`) is only what the picker row says — chosen so it
is obvious which upstream model a row actually runs.

## Why it breaks after every reboot

The supervisor **reverts** the live config on shutdown (settings.json goes back
to the direct upstream URL) and does **not** re-ensure the route on start, so
after a logout/reboot `claude` hits the gateway directly and dies with
`API Error: 400 Invalid input at messages.1.role` (and the
`[claude-code:unrecognized_model]` notice on stderr). Two systemd user units
close that gap — the second unit just re-applies the route once the supervisor
is up (`cc-switch proxy enable` is idempotent):

- `~/.config/systemd/user/cc-switch-daemon.service` —
  `ExecStart=/usr/bin/cc-switch daemon start`, `Restart=always`, so the
  supervisor (and therefore the worker) survives crashes.
- `~/.config/systemd/user/cc-switch-claude-proxy.service` — oneshot,
  `After=cc-switch-daemon.service`, `ExecStart=/usr/bin/cc-switch proxy enable -a claude`.

```sh
systemctl --user enable --now cc-switch-daemon cc-switch-claude-proxy
systemctl --user restart cc-switch-daemon cc-switch-claude-proxy
```

## Check it

```sh
cc-switch daemon status                 # worker running, takeovers: claude=true
cc-switch provider speedtest -a claude
claude -p 'reply with exactly: ok'      # end-to-end, through the proxy
```

Logs: `~/.local/state/cc-switch/cc-switchd.log`, plus per-request rows in
`~/.cc-switch/cc-switch.db:proxy_request_logs` (`status_code`, `model`,
`first_token_ms`).

`cc-switch proxy disable -a claude` (or stopping the daemon) reverts the live
config to the direct upstream — which is a broken configuration for this
provider, not a clean fallback.

## Known rough edge

Claude Code's model catalog doesn't know `deepseek/deepseek-v4.1-flash`
(used as `CLAUDE_CODE_SUBAGENT_MODEL`), so it assumes a 200k context window and
auto-compacts there, while the gateway advertises 1M. Setting
`CLAUDE_CODE_MAX_CONTEXT_TOKENS=1000000` silences the notice and lifts the
assumed window.
