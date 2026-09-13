# Agent Core — Multi-Agent Chat Architecture

> Status: **design, not implemented.** Nothing in this document exists in the
> tree yet.
>
> This is a second architecture alongside the PTY-backed sessions described in
> `DESIGN.md`. It is intended to run **inside the ddu process**, next to (not
> instead of) the terminal. `DESIGN.md` §9 describes the current
> spawn-a-CLI-in-a-PTY backend; this document describes what replaces it for
> native agents.

## 1. Why

Measured 2026-09-10, omp 18.1.16:

| Observation | Value |
| --- | --- |
| `omp` binary | ~135 MB Bun single-file executable + Rust native add-ons |
| process model | **one process per session** — ddu spawns `omp` as a PTY child per session (verified: both session PIDs had `ddu.bin` as parent) |
| active session, physical footprint | 318 MB / 281 MB |
| low-load session, physical footprint | 73 MB / 75 MB |
| what `ps` RSS reports | 738 MB / 516 MB — **overstated** |

RSS counts shared clean pages (the 135 MB binary, the ~1.5 GB dyld shared
cache's resident portion). Those are shared across processes by the OS and are
not duplicated per session. The `footprint` numbers above are the real
per-process cost.

What the 318 MB actually is: ~174 MB app-tagged dirty allocations + 82 MB
`MALLOC_SMALL` + 17 MB JS JIT + 7 MB JS VM gigacage. There were **no** external
LSP child processes and **no** embedded Chrome. The cost is the Bun/JSC runtime
and its heap — not session content. (A 200k-token context is well under 1 MB of
text.)

Two conclusions:

1. **The per-session increment is ~300 MB of private memory, and it is the
   runtime, not the work.** Ten sessions ≈ 3 GB. A Rust core where one process
   hosts every session, sharing one model registry, one HTTP client pool, one
   file-scan cache, one tree-sitter registry, removes most of that floor. The
   win is **sharing**, not Rust being faster.
2. **Process-per-session also forces a bad interaction model.** A subagent
   becomes a spawned process you cannot see into, addressed only as
   "prompt in, wait, string out". Replacing the runtime is a good moment to
   replace the model too — with conversations between equal peers.

## 2. Goals / Non-Goals

Goals:

- **One process.** N agents and N conversations are tasks, not processes.
- **Flat peers.** Agents are not owned by anyone. No parent/child, no tree, no
  spawn depth.
- **Conversations are the unit of work** — reusable, listable, scannable.
- Agents can message agents; agents can message the human. **The human is a
  peer**, with an inbox.
- Everything an agent does is **observable in the conversation where it
  happened** — including the tools it ran and the artifacts it produced.

Non-goals (v1):

- No agent hierarchy, no roster tree, no lineage-based context inheritance.
- No PTY scraping for native agents. The PTY backend stays for shells and for
  external CLIs (`claude`, `codex`) — see `DESIGN.md` §6.
- No LSP / browser / MCP / computer-use in the core. These are precisely the
  memory and complexity drivers measured in §1; add one only when a concrete
  need forces it.

## 3. Core model

```text
Agent         id, name, role, model, system prompt, tools, memory, status
              — a persistent identity; belongs to no one; may sit in many
                conversations at once

Conversation  id, title, members: Set<MemberId>, log: [Entry],
              origin: Option<ConversationId>

Entry         id, conversation, from, mentions: [MemberId], kind, payload, ts

Member        Human | Agent
```

Entry kinds:

| kind | payload | notes |
| --- | --- | --- |
| `chat` | text (+ attachments) | the natural-language messages |
| `tool_call` | tool name + args | authored by the calling agent |
| `tool_result` | output (+ artifacts) | authored by the tool executor |
| `artifact` | diff / file / blob ref | attachable to any entry |
| `approval_request` | what needs approving | awaits an `approval_response` |
| `system` | status / model / membership changes | not rendered as chat |
| `summary` | compaction / handoff text | replaces a range in projections |

Invariants:

1. **The log is the only truth; every view is a projection.** Append-only.
   Nothing is edited — corrections are new entries.
2. **`origin` is provenance, not containment.** It records which conversation a
   new one was started from, for debugging and cost rollup. Runtime logic never
   traverses it. There is no tree.
3. **An agent's LLM context is a projection, never the raw log** (§5).
4. **A member is woken by addressing, not by membership** (§4.1).

## 4. The four rules

These four are the whole design. Everything else is mechanism.

### 4.1 Wake = address

Only a member that is **addressed** is woken. A message with `mentions: [bob]`
wakes Bob; the other members see it the next time they run, but they are not
started by it. In a 1:1 conversation the other party is addressed implicitly.

Without this rule, one message wakes every member, every member answers, every
answer wakes everyone again. That is not a hypothetical — it is the default
failure mode of a group chat full of agents, and it burns tokens quadratically.

### 4.2 Isolation = a new conversation

There is no private parent→child channel to hide work in. Isolation is
**explicit**: to do something that should not pollute the current context, open
a new conversation.

This is the primitive that replaces "subagent with its own context". It is
user-visible, listable, and reusable — which is the point.

### 4.3 Reuse = handoff

Reusing a conversation carries its whole history: coherent, cheap on tokens
(nothing to re-explain), and the peer still remembers. Opening a new one starts
from a blank context: clean, but the peer needs a **brief**.

A brief is a `summary` entry placed at the head of the new conversation — not a
copy of the old log. Copying the old log defeats the purpose of opening a new
one.

So "reuse or reopen?" is not a UI detail. **It is the entire context-management
story of this architecture.** See §6 for who decides.

### 4.4 One turn per agent

An agent may sit in several conversations, but it processes **one turn at a
time**; other wakeups queue in its inbox.

Rationale: a single identity speaking concurrently in two threads holds one
context that belongs to neither. It will contradict itself, and its output in
each thread will be shaped by the other. Serializing turns is what makes "one
agent, many conversations" coherent.

## 5. Context

### 5.1 Projection

```text
ConversationView(agent, conversation) -> [ChatMessage]
```

Projection rules:

- The agent sees the `chat` and `summary` entries of conversations it is a
  member of.
- The agent sees **its own** `tool_call` / `tool_result` entries — it needs them
  to reason about what it just did.
- **Another agent's tool traffic is hidden by default.** Foreign tool logs are
  noise, they blow the context window, and they invite the model to imitate
  another agent's turns. If the result matters, it is the other agent's job to
  express it as a `chat` message.
- The human sees everything; the projection rules above are for LLM consumers.

Projection is per `(agent, conversation)` pair. **An agent in two conversations
holds two independent contexts.** This is intentional (§4.2) and it has a
consequence: an agent that worked in thread B while sitting in thread A does not
automatically know about B. Carrying facts across its own conversations is
either an explicit act (§6.4) or belongs in **agent memory**.

### 5.2 Agent memory vs conversation memory

These are different and both are required:

| | scope | lives in | survives |
| --- | --- | --- | --- |
| conversation log | one conversation, shared by its members | `Conversation.log` | forever, append-only |
| agent memory | one agent, across all its conversations | per-agent store | forever |

Without agent memory, a peer is a blank slate in every new conversation and
"colleague" is a lie. Without per-conversation logs, every conversation is the
same blob and §4.2 is impossible.

### 5.3 Compaction

A `summary` entry replaces a range of entries in projections. Same shape as
omp's compaction / branch-summary entries: the full log is retained, the
projection shows the summary instead of the range. Compaction is triggered by
projected token accounting, not by log size.

## 6. `ask` — how peers start work

```
ask(agent_id, message, conversation?) -> Receipt
```

- `conversation` given → the message is posted into that existing conversation.
- `conversation` omitted → a new 1:1 conversation is opened, `origin` = the
  caller's current conversation.
- **It returns immediately** with a receipt — `{ delivered: true, conversation }`
  — **not the answer.**

### 6.1 Why it is not blocking

Blocking would deadlock. The peer is allowed to talk back mid-task:

```text
Alice: ask(bob, "review the diff module")   [waits for return value]
Bob:   "are you on git2 or gix?"            [asks Alice a question]
Alice: ...never runs, it is blocked waiting for Bob...
```

Both sides wait on the other's function return. Deadlock. Since bidirectional
messaging is a goal, `ask` cannot block.

Non-blocking is not a compromise — it is how every agent here works already. An
agent has no call stack; it has an inbox and is woken by messages. `ask` is the
same shape as an async background job, except the job is a peer conversation.

A blocking convenience wrapper is possible, but the runtime must still pump the
peer's inbound messages while "waiting", or it reintroduces the deadlock.
Do not ship one in v1.

### 6.2 Walkthrough

```text
C1  members {you, Alice}                       origin: —

  you   ──► Alice   "look at the diff module"
  Alice ──► you     "I'll ask Bob"
  Alice  ask(bob, "review the diff module", conversation = None)
            └─ runtime opens C2, members {Alice, Bob}, origin = C1,
               and appends the brief as its first entry
  Alice (turn ends; she holds a receipt, not an answer)

C2  members {Alice, Bob}                       origin: C1

  Bob   ──► Alice   "git2 or gix?"        [only Alice is addressed → she wakes]
  Alice ──► Bob     "git2"
  Bob   ──► Alice   "git.rs:88 unwraps on detached HEAD"
  Alice (woken in C2; this is her context, not C1's)

  Alice send(C1, "Bob's verdict: ...")    [explicit report-back, §6.4]

C1

  you see the verdict in C1
```

### 6.3 Reuse vs new

| | reuse (`conversation` given) | new (`conversation` omitted) |
| --- | --- | --- |
| context | continues; coherent; nothing to re-explain | blank; requires a brief |
| token cost | grows with the existing history | fixed cost of the brief |
| risk | contaminated by unrelated history | peer lacks background |
| answer lands in | that conversation | the new conversation |

Who decides: the **caller** chooses — it is a tool argument. The **human** can
override from the UI ("continue here" vs "new chat"). Both are legitimate; the
agent chooses by default because it knows whether the background is relevant.

### 6.4 Reporting back

A turn is bound to one conversation: the agent thinks in C2's context and its
output lands in C2. To put the result in C1, the agent calls

```
send(conversation_id, message)
```

Caveat: during that turn the agent does **not** hold C1's context, so a
cross-post must be a self-contained report, not a reply that assumes the
reader's history. Cross-posted entries are marked as such in the UI so the human
can see that the author was not "present" in that thread while writing.

## 7. Frame protocol

Everything the runtime emits to any front end is one of a small set of frames.
Define this **first**, as serde types, and route the in-process GUI through it
too — not through direct calls. Then a socket transport, a web guest, or a
collab-style remote costs nothing but a serializer, and every conversation is
replayable.

| frame | payload | purpose |
| --- | --- | --- |
| `entry` | a durable `Entry` | the log, appended |
| `event` | streaming deltas, tool progress | live, not durable |
| `state` | status, model, context %, cost | snapshots |
| `agents` | roster: status, role, model, in-flight work | roster view |
| `ui_request` / `ui_response` | select / confirm / input | approvals and prompts |
| `notice` | errors, lifecycle | toasts, banners |

This mirrors the frame taxonomy omp settled on for its collab protocol
(durable `entry`, live `event`, `state`, subagent `bus`, `agents`,
`ui-request`/`ui-response`). That taxonomy is sound; reuse the shape rather than
inventing one.

### 7.1 Align with AG-UI before freezing these types

The ecosystem has already standardized the agent↔user leg: **AG-UI**
(<https://docs.ag-ui.com/introduction>) is an open, event-based protocol for
exactly this stream — streaming chat, tool-call events, shared state carried as
event-sourced diffs, human-in-the-loop interrupts, sub-agent composition,
steering, and custom events. Most of the table above is a re-derivation of it.

Treat these frames as an **in-process rendering of AG-UI**, not a parallel
invention, and keep only the conversation/membership concepts AG-UI does not
model as our own extension. Buzz states the principle bluntly and it applies
here: *"Protocol-native… no custom wire formats."* See §12.5.

## 8. Tools, approvals, artifacts

- A tool call **is** an entry (§3). Tool execution is an actor: agents post
  `tool_call`, the executor posts `tool_result`.
- **Approvals are entries.** A write/bash call that needs a human becomes an
  `approval_request`; the answer is a new entry. This maps 1:1 to a UI dialog
  and leaves an audit trail with no extra plumbing.
- **Artifacts are entries.** Diffs, files, and blobs attach to the entry that
  produced them — which is what makes "show me what happened in this chat"
  free (§9) instead of a separate instrumentation problem.
- Everything an agent does is attributable: each entry carries its author, so
  cost and tokens roll up per agent and per conversation.

## 9. Persistence and replay

- Append-only **JSONL per conversation**, under
  `~/Library/Application Support/ddu/`, plus an in-memory index.
- The UI is a view over the log; nothing is stored twice.
- Resume = reopen the log and bring its agents back up (idle or parked).
- **Replay** = feed a persisted log to a mock provider and re-run the model
  loop. Because the log is the only truth, this is a first-class testing and
  debugging tool, not an afterthought.
- The workspace snapshot (`state.json` today) gains: conversation list, agent
  roster, membership.

## 10. ddu integration

### 10.1 Concept mapping

| today (`DESIGN.md`) | after |
| --- | --- |
| left: projects + sessions | left: projects + **conversations**, plus an **agent roster** group |
| center: PTY terminal | center: **conversation transcript** for chat sessions; PTY terminal stays for shell / `claude` / `codex` |
| right: git diff | right: **inspector** — entry/artifact detail, selected agent's context & memory (diff still reachable) |
| — | **attention surface**: unread badges, "an agent is waiting on you" |

The attention surface is new and required: in this model an agent can message
*you*. That only exists because peers are equal.

### 10.2 Building blocks that already exist

`gpui-component 0.6` already ships the hard parts of the transcript UI:
`bubble`, `message`, `message_scroller`, `marker`, `text::TextView::markdown`
(markdown + code-block highlighting), `highlighter` (tree-sitter), `virtual_list`,
`list`, `sidebar`, `tab`. **No markdown renderer needs to be written.**

### 10.3 Code touch points

- `src/session.rs` — `AgentSession` / `AgentCmd` / `AgentStatus`. Target: a
  `Session` enum with today's `Pty` arm and a new `Chat` arm bound to a
  `Conversation`. `AgentCmd` presets remain the spawn recipe for the `Pty` arm.
- `src/terminal/` — `PtySpawn` / `TermSession`, unchanged, now only one arm.
- `src/app/mod.rs` — `AppView`, actions, global shortcuts (`⌘T`, `⌘N`, `⌘O`, …);
  `src/app/persist.rs` — the `state.json` snapshot this design extends.
- `src/ui/` — new `conversation_panel`, `transcript`, `roster`, `composer`,
  `inspector`; existing `session_panel`, `terminal`, `diff_panel` evolve.

## 11. Risks

- **Blast radius.** Today one session per process: an OOM kills one session.
  One process: a single runaway agent takes down every conversation. Mitigate
  with bounded inboxes, per-agent token/cost budgets, and cancellable turns.
  This is the price of the architecture; budget for it from the start.
- **Infinite chatter.** Addressed wake (§4.1) plus a per-conversation turn
  budget, a per-agent budget, and stop propagation. Not optional.
- **Context explosion.** Foreign tool traffic hidden (§5.1); children get
  briefs, not logs (§4.3).
- **Agent proliferation.** Creating a peer, or inviting a third party into a
  conversation, is a powerful capability. Rate-limit it or require human
  approval, or agents will spawn colleagues indefinitely.
- **Projection correctness.** `ConversationView` is the single most
  consequential function in the system: projection bugs surface as incoherent
  agents, not as errors. It needs tests from day one.
- **Token cost.** Anthropic's own guidance for its agent-teams feature: teams
  "use significantly more tokens than a single session", and for sequential
  work, same-file edits, or heavily dependent work a single session or a
  subagent is the better tool. Multi-agent chat is expensive **by
  construction** — every peer turn is a model call with its own context. The
  addressing and concurrency rules (§4.1, §4.4) and the budgets above are the
  precondition for this shape working at all, not a tuning knob.

## 12. Prior art

Surveyed 2026-09-11. The model is converging — most of its pieces shipped within
the last year — but the full combination is unoccupied (§12.7).

### 12.1 Closest overall: Buzz (Block)

[`block/buzz`](https://github.com/block/buzz) — Rust, Apache-2.0, ~32k stars.
Desktop app (Tauri + React) plus a Nostr relay: *"a workspace where humans and
agents build together, on a relay you own."*

| This design | Buzz |
| --- | --- |
| agent and human are peers | "humans and AI agents are first-class equals" — agents get their own keypairs, permissions, audit trail |
| the conversation is the unit | channels / threads / DMs |
| an agent can initiate to the human | DMs |
| the log is the only truth | every message, reaction, workflow step, review, and git event is one signed event in one log |
| wake = address (§4.1) | `buzz-acp` bridges relay **@mentions** → agents over ACP/JSON-RPC |
| one process, many sessions | `buzz-agent`: "up to 8 concurrent sessions", each with its own MCP servers, history, and context |

**Divergence:** Buzz is relay/server-shaped (Nostr, Postgres, Redis,
multi-tenant) and its agents run in external ACP harnesses rather than
in-process. Identity is cryptographic, not a local roster.

### 12.2 Same substrate, opposite topology: AgentTeams

[`agentscope-ai/AgentTeams`](https://github.com/agentscope-ai/AgentTeams) — Go,
Apache-2.0, ~5.6k stars. Agents collaborate in **Matrix rooms** (bundled Tuwunel
server, Element client), human-in-the-loop by default, everything auditable.

It validates *room-as-conversation*, and is the inverse of this design on the two
points that matter most: **Manager–Workers hierarchy** (there is a tree) and
**one container per agent** (K8s-native; 2 CPU / 4 GB minimum). Borrow the room
ergonomics, not the topology or the deployment.

### 12.3 Same messaging, hierarchical: Claude Code Agent Teams

[`code.claude.com/docs/en/agent-teams`](https://code.claude.com/docs/en/agent-teams)
— experimental, Claude Code v2.1.178+.

- **Mailbox**: one JSON inbox per agent at
  `~/.claude/teams/{team}/inboxes/{agent}.json`.
- Agents message each other directly (`SendMessage`); the human can open any
  teammate's transcript and message it **without going through the lead**.
- A shared **task list** (pending / in progress / completed, with dependencies)
  coordinates work.

Two takeaways. The mailbox-per-agent file is a proven, dead-simple
implementation of the addressing layer (§4.1) — worth copying outright. And the
hierarchy is deliberate on their side: the unit is a **task**, not a reusable
conversation, and each teammate is a separate Claude session in its own process.

### 12.4 Same memory split: Letta

Letta already separates an agent's persistent memory from its message threads:
the Conversations API gives one agent several isolated threads that all **share
the agent's memory blocks**, and talking is itself a tool
(`send_message_to_agent_async` / `..._sync`). That is exactly §5.2, shipped.
Note the direction of travel: Letta is deprecating memory blocks in favor of
shared memory repositories / MemFS, and its docs flag the concurrency hazard —
appends are safe, whole-block rewrites are last-writer-wins.

### 12.5 Protocols — do not invent a fourth

The ecosystem has settled on three edges:

| Edge | Protocol | Origin |
| --- | --- | --- |
| agent ↔ tools & data | **MCP** | Anthropic |
| agent ↔ agent | **A2A** | Google |
| agent ↔ user (event stream) | **AG-UI** | CopilotKit, with LangChain and CrewAI |

AG-UI is the one that overlaps §7. Its integration list already covers
LangChain, CrewAI, Microsoft Agent Framework, Google ADK, AWS
Strands/AgentCore, Mastra, Pydantic AI, Agno, LlamaIndex, and the Claude Agent
SDK. **ACP** is the other candidate for the agent↔host leg — it is what
`buzz-agent` and Zed speak. Define our frames as an implementation of one of
these instead of a fourth thing to maintain.

### 12.6 Adjacent, not the same thing

- **Cowork-class desktop workspaces** — Claude Cowork (Anthropic, 2026-01),
  OpenAI's Codex app / Workspace Agents, and open alternatives (Open Cowork,
  OpenWork, Paseo, CoWork OS, Nimbalyst). Local-first, multi-model, sandboxed —
  but **a single agent executing tasks**: no agent-to-agent conversation, no
  peer roster.
- **Parallel-agent orchestrators** — Vibe Kanban (Rust, Apache-2.0, ~28k stars,
  **sunsetting**), Crystal, Conductor, Claude Squad, the GitHub Copilot desktop
  app, VS Code multi-agent. They manage N agent CLIs and surface diffs, but each
  agent stays a CLI process and agents do not talk to each other. Closest to
  ddu's *current* PTY architecture, not to this one.
- **Frameworks** — AutoGen, Agency Swarm, KaibanJS, CAMEL, LangGraph.
  Conversation-shaped multi-agent libraries, but no human-as-peer workspace and
  no local runtime.

### 12.7 The gap this design occupies

| Element | Who already has it |
| --- | --- |
| human + agent peers in shared rooms | Buzz |
| wake by @mention | Buzz, Claude Code Agent Teams |
| everything is an append-only event log | Buzz |
| single process, many sessions, in Rust | `buzz-agent` |
| conversation memory vs agent memory split | Letta |
| event protocol for the front end | AG-UI / ACP |
| **local-first, single process, desktop** | **—** |
| **flat — no hierarchy anywhere** | **—** (the two closest multi-agent systems are both hierarchical) |
| **conversation list as the primary surface, bound to a repo / worktree / diff** | **—** |

Multi-agent chat is no longer a novel idea. The differentiator is the three
qualifiers: **local-first, in-process, coding workspace.**

## 13. Crate layout and host API

```text
ddu-agent/            # core crate; no gpui, no terminal, no UI deps
  proto/     Entry, Frame, Envelope — the wire format (serde)
  store/     ConversationStore: append-only log, index, per-conversation JSONL
  runtime/   Runtime: agent registry, conversations, routing, wake policy,
             budgets, cancellation
  context/   ConversationView projection, token accounting, compaction
  agent/     Agent actor: inbox -> projection -> provider -> entries
  provider/  streaming provider clients
  tool/      tool registry, executor actor, approvals
```

The core must not depend on the GUI. If it does, it can never be tested
headless, and the TUI/remote front end becomes impossible. The boundary is the
frame protocol.

```rust
runtime.spawn_agent(spec) -> AgentId
runtime.open_conversation(members, title, origin) -> ConversationId
runtime.post(conversation, entry)
runtime.send(agent, message, conversation: Option<ConversationId>) -> Receipt
runtime.subscribe() -> impl Stream<Item = Frame>
runtime.approve(entry, decision)
```

## 14. Milestones

- **A1 — headless slice.** `proto` + `store` + one agent + one conversation,
  no UI. Acceptance: a one-shot prompt produces a persisted log.
- **A2 — peers.** `ask`, addressing/wake, the four rules, budgets. Acceptance:
  the §6.2 walkthrough runs end to end against a mock provider, and the
  persisted log replays to the same result.
- **A3 — ddu surface.** Conversation list + transcript + composer + streaming
  for a single agent. First visible payoff.
- **A4 — team.** Agent roster, multi-member conversations, attention/inbox.
- **A5 — durability.** Resume, compaction, agent memory, cost rollup.

The A2 acceptance test — *human asks Alice, Alice asks Bob, Bob answers, Alice
reports back* — is the acceptance test for the entire architecture. If that
replays from the log, the model works.
