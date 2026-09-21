# Agent Chief — the orchestrator layer (design)

> Status: **design, not implemented.** Nothing here exists in the tree.
>
> Extends `AGENT_CORE.md` (the peer runtime: §4 wake = address, §5 projection,
> §6 `ask`, §7 frames, §13 crate layout). The chief is **not a second runtime**:
> it is one *role* with a closed tool set, one durable object (the task board),
> and one piece of ordinary code (the dispatcher), on top of the peer model.
> `AGENT_CORE.md` §2/§12.7 claim flatness — §12 below is the reconciliation,
> and it is the load-bearing part of this document.

## 1. What it is

One agent per project (or per goal) — the **chief** — whose job is a human
lead's job: it holds the plan, hands out work, keeps the workers busy, reviews
what comes back, and talks to the human — as the only agent that does, and as
an interlocutor rather than a status feed (§8). It writes no code, reads no
source, runs no command, opens no file. Everything under that line is a task on
its board, worked by a peer agent.

Three separations carry the whole idea:

1. **The chief has no work tools** — enforced by the runtime, not by a prompt (§3.1).
2. **Work detail never crosses to the chief** — the worker→chief interface is a
   capped report plus artifact *references* (§3.2), never a transcript.
3. **The chief's context is state, not history** — a code-rendered projection of
   the board, bounded by *open* tasks (§7).

The payoff of the third: **the chief's window does not grow with the amount of
work that passes through it.** 200 finished tasks cost it what 5 finished tasks
cost, because a finished task is one ledger line, not a conversation.

The human edge is not an afterthought to that model either: answering,
recommending, initiating and being told off are the other half of the job, and
§8 is as load-bearing as §6.

## 2. Goals / Non-goals

Goals:

- Delegate everything; a chief turn that would touch the repo is a design bug,
  not a judgement call.
- Keep every worker slot busy — or state in one line why it cannot be (§6.5).
- Accept human work at any rate without blocking, dropping, or serializing it
  into one giant context (§7.5).
- Survive a restart with the board intact and no task stranded mid-claim (§6.3).
- One surface to watch: what is running, who is doing it, what it cost, what is
  stuck, and what the chief actually saw.
- Be a real assistant at the human edge: answer, recommend, initiate, and answer
  again when asked — never dump (§8).

Non-goals (v1):

- No chief-of-chiefs, no agent hierarchy. The hierarchy is over **tasks**,
  never over agents or contexts (§12).
- No second memory system: the board is an append-only log like
  `Conversation.log`, and the notes store is one bounded file (§7.4).
- No LLM in the mechanical loop: routing a task to a free slot is a queue
  operation, not a completion (§6).
- No OS notification of ddu's own in v1: the attention surface is in-app (§8.3).
- No new viewer: artifacts open in the panes that already exist (§9).
- No PTY worker on the board. An external CLI has no report channel, so it
  stays a session (the line `DESIGN.md` §9 already draws) until it can emit one.

## 3. The two rules

### 3.1 The chief has no work tools

```rust
enum Role { Chief, Worker }
fn tools_for(role: Role) -> &'static [ToolId]   // closed sets, no overlap
```

- **Chief**: `task_create` / `task_update` / `task_link` / `task_assign` /
  `task_block` / `task_close`, `ask`, `send`, `note_read` / `note_write`,
  `board_query`.
- **Worker**: the work surface — `read` / `write` / `edit` / `bash` / …

A chief turn that calls a tool outside its set gets `role_violation` **from the
tool executor**, appended as an error entry and surfaced as a `notice` frame. A
prompt that says "do not work" is a hope; a closed tool set is a fact. The check
is one `match` in the executor actor (`AGENT_CORE.md` §8) — the tool executor is
already the actor that owns every call. Roo Code ships this and states the
reason (the Orchestrator mode has "**No direct tool access**", because reading
files "causes the context to become filled with file reads … prevents context
poisoning"), and `hermes-agent` makes it a profile role — but in both it is a
recommendation; here the runtime refuses the call (§15.1).

Two consequences, both wanted:

- **The chief cannot read the repo.** When it needs a fact to decompose ("which
  module owns the diff poll?"), that is a `scout` task — one worker turn on a
  small question, and the answer comes back as a report. Repo content stays out
  of the chief's context for the same reason §3.2 exists.
- **The chief cannot verify by inspection.** Acceptance criteria come from the
  human's message or the goal statement, and are checked by another worker (§5),
  not by the chief reading a diff.

### 3.2 Work detail never crosses to the chief

The worker→chief interface is one `report` entry:

```rust
struct Report {
    task: TaskId,
    state: ReportState,          // Done | Failed | Blocked | Question
    line: String,                // ≤ REPORT_LINE_MAX (200 chars) — the whole summary
    artifacts: Vec<ArtifactRef>, // Diff | File | Log | Blob
    asks: Vec<Question>,         // ≤ 3, each one line
    cost: Cost,
}
```

- A worker **cannot** inline a diff, a file, or a command log in a report: the
  tool refuses oversize text and the worker must publish an artifact and point
  at it. The cap is enforced, not advisory.
- The chief reads a report line and artifact *refs*. It reads artifact
  **contents** only by asking a worker to summarize them (a task) — or by the
  human clicking, which is a pane action, not the chief's context.
- Workers have no channel to the human *as speakers* (§8): their only outbound
  edges are `report` and `ask` to the chief. (The human may still write to the
  board directly — §8.2a. The two rules are different things.)

## 4. Core model

```text
Task       id, goal            one line, imperative — what the board shows
           brief               the full spec: a document, NOT rendered in the digest
           accept              acceptance criteria; a task without one cannot be Ready (I3)
           evidence            how it is checked: Check(cmd) | Contract(ci) | Worker (§5)
           deps                TaskIds that must be Done first
           phase               §5
           claim               Option<Claim> — who holds it, until when (§6.3)
           claims_paths        the workdir globs this task may write (§6.4)
           attempts, budget    retry count + token/time budget
           artifacts           what it produced
           idem                an idempotency key: a replayed intake cannot create it twice
           lane                a group id (one plan → one lane): the UI column and the rollup

TaskEvent  id, task, ts, kind, payload     append-only — the task's whole history

Board      the projection: lanes × tasks, ready queue, slot pool, ledgers
Slot       { worker: AgentId, conversation: ConversationId, task: Option<TaskId>, since }
```

| phase | meaning | moved by |
| --- | --- | --- |
| `Backlog` | captured, not runnable (no `accept`, unmet deps, or awaits a human decision) | chief |
| `Ready` | runnable now | chief; dispatcher when deps close |
| `Running` | claimed by a slot | dispatcher |
| `Verifying` | a worker reported Done; the evidence its `accept` names is being checked (§5) | dispatcher |
| `Blocked` | needs a decision or a missing input | worker → chief → human |
| `Done` / `Failed` / `Cancelled` | terminal; a retry is a new **attempt** on the same task id | chief |

Invariants:

- **I1** The board is append-only: phase is a projection of `TaskEvent`s;
  corrections are new events. (Same primitive as `Conversation.log`.)
- **I2** Every transition is an event with an author — human, chief, dispatcher,
  or worker.
- **I3** A task in `Ready` has a non-empty `accept` and an `evidence` kind.
- **I4** A terminal task leaves the digest and enters the ledger in the same
  transaction that records the transition (§7.3).
- **I5** The chief's tool set is closed (§3.1).
- **I6** No worker is a human channel: a worker cannot open a conversation with
  the human, cannot answer an approval, and cannot consent (§8). The human may
  still write to the board directly (§8.2a) — the two rules are different things.
- **I7** A report is size-capped; oversize becomes an artifact plus a line (§3.2).
- **I8** No LLM in the mechanical loop (§6).
- **I9** The chief answers the human in the human's language, from state,
  without pasting a transcript, a diff, or raw worker output (§8.1).
- **I10** The chief initiates only on the five shapes of §8.3; a per-task
  completion is not one of them (§8.5).
- **I11** One writer at a time per path: two tasks whose `claims_paths` overlap
  never run concurrently (§6.4).
- **I_free** `free_slots > 0 ⇒ ready.is_empty() ∨ dispatch_in_flight` (§6).

## 5. Lifecycle

The transitions worth naming (the rest are the table in §4):

- `Running → Verifying` needs a report with `state = Done` *and* evidence: an
  artifact, or an explicit `no-artifact` mark for a read-only task. A bare
  "done" is a `Failed` report.
- **Evidence first, a verifier second.** `accept` names how it is checked:
  - `Check(cmd)` — a command whose exit status is the verdict (a test run, a
    build, a lint). Machine-checked: no model, no argument.
  - `Contract(ci)` — an external check contract: the required checks of the
    target repo/PR, read at completion time.
  - `Worker` — a *different* worker checks it against `accept`, with the brief
    plus the artifact refs. **The verifier never sees the author's transcript**,
    and that is a rule rather than a courtesy: Cognition measured the
    generator-verifier loop working *best* with no shared context ("an average
    of 2 bugs per PR, of which roughly 58% are severe") and blames *context rot*
    in the author's long context for the difference. The chief adjudicates
    pass/fail; it never runs the check itself (§3.1).
  `hermes-agent` ships all three (a `review` card the dispatcher claims with a
  bundled review skill, plus a PR completion contract that re-reads branch
  protection and exact-head checks and refuses "zero-run acceptance"); the
  independent verifier is the fallback for what cannot be machine-checked.
- **A brief states the goal, `accept`, and the boundaries — never the steps.** A
  chief that cannot read the repo is exactly the manager Cognition found "overly
  prescriptive, which backfires when the manager lacks deep codebase context";
  the worker owns the how and may push back on a brief that prescribes it. A
  brief containing a step list is the smell that the chief is trying to work
  (§13).
- Pass → `Done`. Fail → a new attempt: the findings go back as a **steer** into
  the same worker conversation (the worker still remembers, so nothing is
  re-explained — `AGENT_CORE.md` §4.3), or the task goes `Blocked` after
  `max_attempts`.
- **Proportionality**: policy `verify = writes-only | always` decides whether a
  `Worker`-arm check is created at all. A scout report needs none; anything that
  writes code, config, or an outward message gets one — or, better, a
  `Check`/`Contract` that costs no model call (§5).
- **Stall**: no event and no artifact growth for `stall_secs` → the dispatcher
  emits `stalled`; the chief steers, parks (frees the slot, keeps the
  conversation) or cancels.
- **Amendment mid-flight**: a human entry on a task is forwarded as a steer to
  the live conversation, or held for the next attempt — never dropped.
- **A blocked task frees its slot.** A worker waiting on the chief must not
  idle a warm slot: the task goes `Blocked` with `wait: human|chief`, and the
  dispatcher refills. The answer re-enters the same conversation. This is the
  single most important mechanical rule for §6's utilization.

## 6. The dispatcher (why the workers are never idle)

The chief does not do this — it is deterministic code in `ddu-agent/sched/`. A
model call per dispatch is slow, costly, and nondeterministic; the difference
between a queue and an LLM router is the entire utilization story, so **no
completion ever rides in the scheduling path** (I8).

Wake conditions — each one a board event, no polling:

1. A task enters `Ready` → fill a free slot.
2. A slot frees (`Done` / `Failed` / `Blocked` / `Cancelled`) → fill it.
3. A slot opens (worker spawned, conversation reused) → fill it.
4. `ready.len() < LOOKAHEAD` (default `2 × slots`) → wake the **chief** with a
   `board_digest` frame. This is the one automatic wake the chief has, and its
   work is management — decompose, re-plan — never execution.
5. `T_review` on the `Blocked` set → wake the chief (a blocked task must not be
   forgotten).
6. Claim lease expiry → reclaim (§6.3).
7. `backlog` non-empty while `ready` is empty → wake the chief: the plan's
   dependencies are the bottleneck, and only the chief can re-cut them.

A tick still sweeps the board (default 60 s, §6.3), because a dispatcher with no
timer cannot notice the one thing events never report: a worker that died
without saying anything.

`openai/symphony` states the same boundary as an invariant — "the orchestrator
is the only component that mutates scheduling state" — and Magentic
orchestration is the counter-example worth avoiding: its manager is an LLM that
picks the next speaker every round, at "several US dollars and tens of minutes
per task".

**6.1 Slot growth.** `slots < max_slots ∧ ready.len() > slots ∧ no conflict` →
the dispatcher asks the chief to grow. The chief owns roster policy; the runtime
owns mechanics. Growth is a `worker_create`, and it is human-approvable
(`AGENT_CORE.md` §11: agent proliferation is a real risk). Magentic goes the
other way — a **fixed roster**, because "unused specialists distract it, and
missing expertise has no fallback" — which is the honest trade: a fixed roster
has no growth cost and no growth; a grown roster must be capped and approved.

**6.2 Lookahead is the liveness knob.** Keeping the ready queue deeper than the
slot pool is a standing instruction in the chief's policies (`§7.4` notes), not
a runtime guess: it is what makes a completion instantaneous rather than a
decomposition round-trip.

**6.3 Claims are leased.**

```rust
struct Claim { worker: AgentId, conversation: ConversationId, until: Instant }
```

`until = now + lease_secs`, renewed by the worker's turn heartbeat. A worker
that crashes, is parked, or burns its budget leaves a claim that expires; the
dispatcher returns the task to `Ready` with `attempt++` and logs the reclaim.
Without a lease one dead worker strands a task forever and the board lies.

Reference values from the field: `hermes-agent` uses a 15-minute claim TTL
**extended while the worker's PID is alive** (a slow model in one tool-free LLM
call is not a dead worker), a 1-hour no-heartbeat backstop, a 4-hour stale
reclaim, a 60 s dispatcher tick, and a `stranded_in_ready` warning at 30
minutes; `openai/symphony` uses no lease at all, only a 5-minute
`stall_timeout_ms`, a 1 s continuation retry and `min(10 s × 2^(n−1), 5 min)`
failure backoff. We take **both halves** — a lease *and* a tick — because either
alone has a hole: a lease with no tick never fires, and a tick with no lease
cannot tell a slow worker from a dead one. (The field's own honest note: leases
are rarer than boards; watchdogs plus reconciliation are the common pattern.
§15.3.)

**6.4 Path claims (the merge-hell guard).** A task declares `claims_paths`
(globs). The dispatcher refuses to run two tasks whose claim sets intersect
(set intersection — cheap, and it belongs in the dispatcher for the same reason
the dispatcher exists). Overlaps are resolved by the chief: merge the tasks,
order them with a `dep`, or split them by file. Concurrent same-file edits are
the classic way a fleet of agents produces an unusable diff.

This is the design's least-precedented choice, and §15.4 says so: the field
answers same-file concurrency with *advice* — Copilot `/fleet` documents "If two
agents write to the same file, the last one to finish wins—silently" and answers
with prompt discipline; Symphony answers by never letting two issues share a
directory; `swarm-protocol` computes conflicts and calls them "advisory, not
enforced"; `hermes-agent` uses a `hotspot:` comment convention. We enforce where
the others advise, which is exactly the kind of thing that must be honest about
being unproven.

Isolation is the other half of the same problem, and the field's standard is a
workspace per task: Symphony gives each issue its own workspace directory (plus
three path invariants and a `workspace-write` sandbox default), `hermes-agent`
offers `scratch` (deleted on completion), `dir:<abs>` and `worktree`. v1 here
follows `DESIGN.md`'s open worktree item — one working tree per project with
`claims_paths` as the guard — and the per-lane worktree is the K5 upgrade, which
relaxes `I11` from "must not overlap" to "must not overlap inside one worktree".

**6.5 Honest limits — state them, don't spin.**

- Sequential work has no available parallelism. With `ready = 1` and one slot,
  extra slots buy nothing, and a slot with no *non-conflicting* task stays
  **idle** rather than inventing work. "Never idle" holds where the plan has
  parallel branches; the design's obligation is a deep queue, not fabricated
  tasks.
- The queue's depth comes from the plan. A 40-task plan keeps N slots busy for
  most of its life; a 3-task goal does not. A chief asked to "keep the workers
  busy" on a 3-task goal must answer *"this goal is 3 tasks"* — that reply is
  the job, not a failure of it. The field says the same at the level of the
  whole architecture: Anthropic's post, "most coding tasks involve fewer truly
  parallelizable tasks than research"; Claude Code's team docs, "For sequential
  tasks, same-file edits, or work with many dependencies, a single session or
  subagents are more effective."

## 7. The chief's context (why it never needs a long window)

```text
ChiefView(chief) =                      # the ORDER is load-bearing — §7.7
    system(role + policies + notes)     stable prefix, capped (NOTES_MAX, §7.4)
  + human_thread_tail                   last K = 10 messages; older → a summary entry
  + ledger_tail                         the last few terminal tasks, one line each
  + board_digest                        ≈25–30 tokens per OPEN task, capped (§7.2)
  + pending_intake                      one line per item, batched (§7.5)
  + reports_since_last_turn             ≤ REPORT_LINE_MAX each
```

**7.1 The digest is rendered by code, never written by a model.**

```text
open 7   ready 2   running 3/4   blocked 1   done today 31   spend 412k
lanes  parser ✓ 4/6 d · ui 2/2 r · docs 1 b
t11 Running  w3  3m   "regenerate the icon set"        deps:—     paths:assets/icons/**
t12 Ready    —   —    "icon: add keyboard.svg"          deps:t11   paths:assets/icons/keyboard.svg
t14 Blocked  w2 12m   "which shell is the default?"     wait:human  asked:9m
done  t1…t10 → ledger (board_query(state=done, since=…))
```

Target: ≤ 30 tokens per open-task line (asserted in K2), 40 open tasks ≈ 1.2k
tokens. Terminal tasks print as a ledger *tail*, never as rows.

The digest is also **recitation**, which is why it goes near the *end* of the
view rather than the start: Manus's fourth principle is that its agent "tends to
create a `todo.md` file — and update it step-by-step … it's a deliberate
mechanism to manipulate attention" that "pushes the global plan into the model's
recent attention span, avoiding 'lost-in-the-middle' issues". The digest is the
chief's `todo.md`, regenerated by code instead of rewritten by the model.

**7.2 Growth is a function of work-in-progress.** The digest holds open tasks;
the ledger holds everything else, queryable on demand (`board_query`) and never
injected wholesale. The chief's context is therefore **O(open tasks)** — bounded
by construction rather than by a compaction heuristic firing at the right
moment. This is the "special context logic": the natural compaction unit is the
**task**, because a task's boundaries are already durable events (`I1`).

**7.3 Eviction is part of the transition.** Recording `Done` and dropping the
task from the digest is one operation (`I4`). There is no "when the window fills
up, summarize" pass, because the window cannot fill up.

**7.4 Notes are the chief's memory, not its context.** `ChiefNotes` per project:
one file, hard cap `NOTES_MAX` (target ≈ 4k chars) with an explicit *prune*
action, injected whole. Content: the current goal, standing policies ("never
push"; "file writes inside the worktree are pre-approved"), open questions for
the human, decisions and *why*. Rules: decisions and pointers only — no file
contents, no diffs, no transcripts; a note that needs a transcript names a task
id. This is what makes "tell the human once" work, and it is the one thing
besides the board that must survive a restart.

**7.5 Intake folding (unbounded acceptance without unbounded turns).** Two
stages, cheap one first:

1. **Triage**, per message, *without the chief's context*: a rule pass over the
   six classes of §8.2 (reply-into-a-task-thread ⇒ amendment; a leading marker ⇒
   task; a question ⇒ an answer from the digest) and, only when still ambiguous,
   a small-model call on that message alone. Every intake entry carries an
   `idem` key, so a retried automation cannot create the same task twice.
2. **Fold**, per turn: the chief's next turn consumes **all** pending intake at
   once. 50 messages that arrived while it was mid-turn are one chief turn with
   50 lines.

So intake costs O(1) chief turns regardless of rate, and nothing is dropped —
the inbox is durable and `Backlog` is unbounded (only `Running` is bounded by
slots). *"The chief can accept unlimited work"* means **accepting is an append,
not a slot**.

**7.6 Turn discipline.** One turn at a time (`AGENT_CORE.md` §4.4): reports that
arrive mid-turn queue. A turn ends when digest, intake, and reports are
consumed. There is no `await` in the chief's tool set — blocking would deadlock
against a worker that must ask mid-task (`AGENT_CORE.md` §6.1). Anthropic's
production post names the same wall from the lead's side — "our lead agents
execute subagents synchronously … the lead agent can't steer subagents,
subagents can't coordinate, and the entire system can be blocked while waiting
for a single subagent" — and non-blocking `ask` plus `steer` is how this design
avoids it.

**7.7 The view obeys the KV-cache, not just the token count.** Manus's first
principle: "the KV-cache hit rate is the single most important metric for a
production-stage AI agent" — with Claude Sonnet, cached input costs $0.30/MTok
against $3/MTok uncached, a 10× difference, so a per-turn prompt that
invalidates its own prefix pays it every single turn. Consequences for
`ChiefView`:

- **The stable prefix comes first** (role, policies, notes) and the volatile
  material last (digest, intake, reports). Anything that changes every turn must
  not sit above anything that does not — which is why the view order above is
  not cosmetic.
- **No timestamps** in the prefix (Manus names a second-precision timestamp in
  the system prompt as the anti-pattern) and **deterministic serialization**: the
  digest renderer must be byte-stable for the same board state, so rows sort by
  task id and never by a hash-map order.
- **A note write invalidates the whole cache**, because notes live in the
  prefix. Notes are therefore written in batches — at most one write per chief
  turn — and editing them is a deliberate act, not a log.
- The tool schema is **static**, the same principle from the other side: Manus's
  second rule is "Mask, don't remove" — "unless absolutely necessary, avoid
  dynamically adding or removing tools mid-iteration", because tool definitions
  sit near the front of the context. `I5`'s closed tool set is cache-friendly by
  construction.

## 8. The human edge (the chief's other half)

The chief is the only agent that talks to the human, and that is **half the
job** — not a reporting channel bolted onto a scheduler. A human lead who
handed out work and then went silent would be a bad lead; a chief that
dispatched perfectly and said nothing is the same failure. Everything below is
as load-bearing as §6.

### 8.1 The human is a conversational peer, not a form

- The human's thread is an ordinary conversation (`AGENT_CORE.md` §3): the human
  is a peer with an inbox (`AGENT_CORE.md` §10.1), the chief answers *in it*,
  and every line is a durable entry — auditable, replayable, and visible beside
  the work it refers to.
- `human_thread_tail` (§7) is in the chief's view, so a follow-up lands in the
  conversation the human is actually having rather than in a stateless ticket
  system.
- **Language, register, and standing preferences are notes** (§7.4): the human's
  language (which need not be the language of the briefs the workers get), how
  much detail, whether they want to see diffs before anything lands. Written
  once, honored from then on without re-explaining — this is the working
  relationship's memory, and it is the only place a preference may live.
- **Style contract**: an answer is *written*, not dumped. No raw worker output,
  no transcript paste, no digest blob unless it was asked for (§3.2). The
  structured shapes in §8.3 exist so the human can scan — not so the chief can
  avoid saying something.

### 8.2 Inbound — what a human message can be, and what each one costs

| it is | recognized by | what happens | workers woken |
| --- | --- | --- | --- |
| a new goal | the default | decomposed into a *plan preview* (§8.4), then tasks | when it starts |
| an amendment | reply into a task's thread, or "instead of X, do Y" | a `steer` into that task's live conversation, or held for its next attempt (§5) | none (a steer) |
| a status question — "how is it going?" | triage | **answered from the digest: zero worker turns** (§7.1) | 0 |
| a fact about the work | triage: states something, asks nothing | an entry on the affected task | 0 |
| a change of standing policy — "always ask before you push" | triage | written to notes (§7.4), applied from then on | 0 |
| an approval / rejection | a reply to an `approval_request` | routed to the asking worker, recorded | 0 |

Recognition is §7.5's triage (rules first, a small model only when ambiguous,
**never with the chief's context**), so the cheap paths stay cheap: only a new
goal and an amendment touch the work.

**A question that needs facts is answered asynchronously, like a colleague.**
The chief posts one entry immediately — *"checking, back in a moment"* — opens a
`scout` task, and posts the answer when the report lands. It never blocks on the
answer (§7.6; `AGENT_CORE.md` §6.1), and it never leaves the human hanging
without saying so. That promise is tracked: a scout whose report is missing past
`await_secs` is a `Blocked` task like any other, so **the human's question is on
the board** and cannot be forgotten.

### 8.2a The human is not a client of the chief

The chief is the only *agent* that speaks to the human. The human is **not**
limited to speaking through the chief: they hold a write path to the board
itself — comment on a task, unblock it, cancel it, answer an
`approval_request`, set a lane's priority — and each of those is a normal
`TaskEvent` with `author = human`. The chief is *notified* of such an entry (it
appears in the next view as a change it did not cause), never asked for
permission to accept it.

This corrects a first draft of this section, which made the human a client of
the chief. The field is unanimous the other way: `hermes-agent` gives the human
the same CLI, slash command and dashboard the agents use — "every handoff is a
row any profile (or human) can see and edit", with the human's own command
explicitly exempted from the mid-turn guard; Claude Code lets the human open any
teammate's transcript and message it directly; Copilot exposes a machine-readable
`/tasks` surface the orchestrator does not mediate. `AGENT_CORE.md` §10.1's
attention surface asks for the same thing. A single human channel is a bottleneck
unless the human can also reach around it.

### 8.3 Outbound — the five shapes, and the only way the chief initiates

| shape | when | carries |
| --- | --- | --- |
| `progress` | the human asked, or a cadence tick (§8.5) | a digest render (§7.1) plus one line of judgement |
| `decision_needed` | a `Blocked` task, an approval, or an ambiguity policy says not to guess | the question, the options **with the chief's recommendation**, the cost and reversibility of each, and exactly what is paused meanwhile |
| `handoff` | a lane or goal is `Done` | what was done, artifact refs, what was verified and by whom, spend |
| `heads_up` | a stall, a budget warning, a plan that turned out wrong | the fact, the chief's proposed correction, and what it already did about it |
| `answer` | any human question, including "why did you do that?" | the answer, with artifact refs and board evidence |

- **The chief recommends; it does not forward.** "The worker wants to run a
  destructive command" is a failure of the job; "t1 wants to delete the stale
  build dir — reversible, ~2 min, and I'd say yes; t4 and t7 pause meanwhile" is
  the job.
- Initiating is expected — that is what makes it an assistant rather than a
  console — but only through these five shapes (§8.5).
- **Attention surface** (`AGENT_CORE.md` §10.1): an unread badge on the chief's
  row and on a lane, plus a "the chief is waiting on you" strip (§9).
- **Channels beyond the app are adapters over the same five shapes.** v1's
  channel is the in-app thread. A mail / chat / webhook channel is a transport
  that renders the same entries outward and feeds replies back as inbound
  (§8.2), and it stays behind the plan gate and the approval policy *because* it
  is outward-facing and irreversible. The split of §3.1/§3.2 holds across every
  channel: the chief speaks, workers never do.
- **No OS notification in v1.** ddu deliberately owns none (`DESIGN.md` §10: the
  sniffed bell was removed because it could not tell "needs you" from "printed a
  bell"). A structured `decision_needed` is *not* a sniff — ddu knows it is a
  question — so a native notification for that one class is defensible later;
  but it would be the first notification this app owns, so it is the human's
  call (a setting), never a design assumption.

### 8.4 A new goal goes through a plan gate

1. Triage classifies it as a goal (§8.2).
2. The chief decomposes it into a **lane**: a one-line goal, its tasks with
   goals / `deps` / `accept`, the slot count it wants, and a spend estimate.
3. Policy (`plan_gate`) decides which side it starts on:
   - **reversible, inside the worktree, within budget** → start, and post the
     plan as the first `progress`: approve-by-default, vetoable mid-flight;
   - **irreversible, outward-facing, or over `plan_gate_tokens`** → post the
     plan and wait for a yes, as a `decision_needed`.
   The gate is a per-project setting, so the human sets their own taste once
   instead of answering per goal. Magentic ships the same knob
   (`RequirePlanSignoff`), and Microsoft's guidance is explicit about the reason
   — a human gate before "file deletion, external API writes, or any action with
   no rollback" — because the Magentic-One paper's own agents attempted account
   lockouts, unauthorized password resets, accepting ToS without review, and
   recruiting humans via social media.
4. Either way the plan is on the board and in the ledger, so *"what did you
   decide, and why"* is always answerable (§8.3's `answer`) — the human can
   audit the chief's judgement, not just its results.

### 8.5 Interruption policy (how often it may open its mouth)

- **Reactive**: every human message gets an answer, in the human's language.
  Always. A late answer is an apology plus the answer, never silence.
- **Proactive**: only the five shapes of §8.3. Never a per-task completion,
  never a re-statement of a digest the panel already shows.
- **Coalescing**: proactive entries closer than `quiet_secs` fold into one
  (§7.5's fold, applied outward). A chief that emits twenty lines for one bad
  afternoon is a chief the human mutes, and a muted chief is worse than none.
- **Cadence**: `progress_every` (off by default) posts a `progress` on an
  interval, and only while a lane is running and something moved — the panel is
  the always-available version of the same string, so the cadence is for a human
  who is not looking at the window.
- **The human may mute a class**, in notes: "don't report tasks; wake me only
  when you need a decision".

## 9. UI (the window)

| surface | where | content | reuses |
| --- | --- | --- | --- |
| **Board** | new panel in the left pane beside the session list (`⌘⇧B`), toggled like the file tree | columns = phases (`Backlog / Ready / Running / Verifying / Blocked / Done`); a card per task: id, goal, assignee, elapsed, attempts, cost, artifact count | `v_virtual_list`, the file-tree row recipe, `ui::scaled` |
| **Worker lanes** | the board panel's footer (or the sidebar's session group) | one row per slot: worker, current task, state, spinner while a turn is live, last-activity age, spend | the existing two-line `session_panel` row — it already *is* this shape |
| **Chief inspector** | the right pane's second tab, beside the diff | digest, notes, context %, spend per task, the digest's own token count | the existing pane frame + `ui/code_text.rs` |

- Clicking a worker row mounts that worker's **conversation transcript** in the
  center pane (`AGENT_CORE.md` §10.3): streamed output, tool calls, artifacts,
  live. That is "subagent work in a window" without a second viewer.
- Clicking an artifact opens it in the **existing** panes — a diff artifact is a
  path plus a generation, a file artifact the file view — so the board adds no
  viewer of its own.
- The chief's thread is the center pane's default, and the composer writes to
  the chief and nothing else. A worker's transcript is read-only there; steering
  a worker goes through the chief (an amendment entry, §5). The human's *own*
  board actions are the exception the panel must expose (§8.2a): a card is
  commentable, cancellable and unblockable in place, and those writes author as
  the human.
- The pane carries the **attention surface** (`AGENT_CORE.md` §10.1): an unread
  count on the chief's row and on any lane that wrote while the human was away,
  and a "the chief is waiting on you" strip while an unanswered
  `decision_needed` or a `Blocked(wait: human)` task names the human — the
  in-app stand-in for the OS notification this app does not own (§8.3). A strip
  and a badge, never a modal: the human may keep reading.
- Panel rules (`docs/UI.md`): lanes are virtual; the transcript mounts
  **uncached** (window selection participants); a board change notifies the
  board panel alone; a worker's stream frames repaint its lane row, not the
  board — the same single-subscription discipline `subscribe_term` enforces for
  the sidebar today.
- The inspector exists to make §7 **visible**: the human watches the digest size
  hold flat while the `Done` column grows. If that number ever climbs with the
  ledger, the design is broken, and the inspector is where it shows.

## 10. Frames (`AGENT_CORE.md` §7 gains five)

| frame | payload | purpose |
| --- | --- | --- |
| `task` | `TaskEvent` | durable: one task's transition/history |
| `board` | the projection: lanes, counts, ready depth, slot pool | UI state snapshot |
| `slot` | slot ↔ worker ↔ task, live status | the worker lanes |
| `attention` | unread counts per thread, who is waiting on whom | the human edge (§8.3) |
| `digest` | the rendered digest, byte-counted | the chief's own wake payload; the audit of what it saw |

`digest` is in the protocol for the same reason `entry` is: what the chief saw
must be replayable, and the audit is how "the window stayed small" is proved
rather than asserted.

## 11. ddu integration

```text
ddu-agent/                    # AGENT_CORE.md §13's new crate, no gpui, no terminal
  board/   Task, TaskEvent, TaskLog (JSONL per project), Phase, the Board
           projection, ledger queries, the digest renderer
  sched/   Dispatcher: ready queue, slot pool, claim leases, path-claim conflict
           check, stall detection, the wake conditions of §6 — no model, no gpui
  chief/   Role + closed tool sets, ChiefView projection, ChiefNotes, intake
           triage (§8.2), the outbound shapes (§8.3), the plan gate (§8.4)
```

- `Role` rides on the agent spec (`runtime.spawn_agent(spec)`); `tools_for(role)`
  is enforced in the tool executor actor — the existing chokepoint.
- **Persistence**: `tasks.jsonl` + `notes.md` per project in the app support dir,
  beside the conversation logs. `state.json` gains only the board's UI bits
  (panel visibility, selected lane): the board itself is its own log (`I1`).
- **Settings** (`settings.json`, one-to-one with the Settings window):
  `slots` (3), `max_slots`, `lookahead_factor` (2), `lease_secs`, `stall_secs`,
  `max_attempts`, `verify` (`writes-only | always`), `chief_model`,
  `report_line_max`, `notes_max`; for the human edge — `plan_gate`
  (`auto | ask`), `plan_gate_tokens`, `await_secs` (a promise's deadline),
  `quiet_secs` (coalescing), `progress_every` (off), `notify` (off, §8.3).
- **Touch points**: `crates/ddu-core/src/session.rs` — a `Session::Board` arm
  beside today's `Pty` and `AGENT_CORE`'s `Chat`; `crates/ddu-app/src/app/` — a
  `board.rs` beside `diff.rs` (event-driven, poll-free); new
  `crates/ddu-app/src/ui/board_panel/`; `terminal_panel` untouched.
- The board toggle and the task actions that deserve chords become `Command`s in
  `crates/ddu-app/src/app/keys.rs` (the board toggle `secondary-shift-b`), so the
  Keys page rebinds them like every other shortcut and the panel's tooltips read
  the current chord through `accel_hint`.
- A worker slot's backend is an implementation detail of the slot: the board's
  interface to it is `(conversation, report)`. v1 slots are native agents; a PTY
  CLI joins the board only when it can emit a report (§2).

## 12. Reconciliation with `AGENT_CORE.md`'s flat model

`AGENT_CORE.md` §2/§12.7 claim flatness — no hierarchy anywhere — while §12.3
there observes that the managers in the wild are hierarchical. A chief does not
contradict that claim once the axis is named:

- **Agents stay peers.** No agent owns another; no spawn tree, no lineage, no
  inherited context. A worker is whichever peer occupies a slot right now, and
  slot membership is ephemeral.
- **The hierarchy is over tasks.** `deps`, `claims_paths`, `assignee`, `lane`
  are edges in a *durable task graph*, not edges between agents.
- **The chief owns a board, not agents and not contexts.** Its power is the
  right to *write* the board; its limits are the closed tool set (§3.1) and the
  digest projection (§7).
- The thing a manager pattern usually smuggles in — "the manager's context is
  silently the subagent's brief" — stays out: a brief is an explicit `summary`
  entry at the head of the worker's conversation (`AGENT_CORE.md` §4.3),
  listable and reviewable, and it is the *only* context a worker inherits.
- The four rules of `AGENT_CORE.md` §4 are untouched: wake is still address,
  isolation is still a new conversation, reuse is still a handoff, one agent
  still runs one turn at a time. **The board consumes those primitives; it does
  not add a second set.**

Revised differentiator for `AGENT_CORE.md` §12.7: local-first, single process,
**flat agents — hierarchical tasks**, bound to a repo / worktree / diff.

## 13. Failure modes

| failure | guard |
| --- | --- |
| the chief starts doing the work | closed tool set; `role_violation` entry + `notice` (`I5`) |
| the chief's context creeps up on transcripts | capped reports; code-rendered digest; the inspector prints the number (§9) |
| workers idle while work is ready | `I_free` + lookahead + the completion wake (§6) |
| a dead worker strands a task | claim leases + reclaim (§6.3) |
| two workers rewrite the same file | `claims_paths` conflict check (§6.4) |
| over-decomposition (N workers on one task's work) | one task = one deliverable in a lane; verification catches a split that does not add up |
| a task graph that spawns tasks forever | per-task budget + a depth cap on the task graph + a per-turn rate limit on `task_create` |
| the chief becomes the bottleneck | intake folding (§7.5); triage without the chief's context; the dispatcher doing the mechanical work (§6) |
| the chief forwards a blocker instead of deciding | the entry shape is checked before it is written: no options, no recommendation and no paused-set, no `decision_needed` (§8.3) |
| the chief goes quiet while it waits on a scout | the "checking" entry is a tracked promise with `await_secs`, and a missed deadline is a `Blocked` task (§8.2) |
| the chief floods the human | five shapes only, coalescing inside `quiet_secs`, and a mute class in notes (§8.5) |
| the chief answers in the wrong voice or language | language, register and detail level are notes, applied without re-explaining (§8.1) |
| a worker's report hijacks the chief | reports are data: length-capped, fence-rendered, never instructions; the tool set is small and approvals still reach the human |
| the chief over-prescribes the how | `task_create` refuses a `brief` carrying a step list (goal + `accept` + boundaries only), and a worker may push back — Cognition's manager-Devin finding is that an over-prescriptive manager "backfires when the manager lacks deep codebase context" (§5) |
| the human becomes the bottleneck | the human holds a direct write path to the board (§8.2a); a status question costs zero worker turns (§8.2); the five shapes cap what is asked of them (§8.5). The field's warning is explicit — "dispatch volume can outrun review capacity" |
| a restart loses the plan | `tasks.jsonl` is the truth; reopen → the dispatcher re-derives the ready queue and reclaims expired claims |

## 14. Milestones

- **K1 — headless board + dispatcher.** `board` + `sched`, N slots, a scripted
  task tree against a mock worker. Acceptance: the log replays; `I_free` holds
  over a 30-task plan; a killed worker's task is reclaimed within one tick
  (§6.3) and re-run; a replayed intake with the same `idem` key creates one task.
- **K2 — the chief role.** Closed tool sets + `ChiefView` + notes + intake fold.
  Acceptance: a work-tool call is refused; a 200-task board renders a digest
  ≤ 2k tokens, and the digest does not grow with the ledger; two consecutive
  turns over an unchanged board share a byte-identical context prefix (§7.7).
- **K3 — live slice.** One chief, three worker slots, a real goal. Acceptance:
  every slot busy while `ready ≥ slots`; human messages never wait on a slot; a
  report → verify → `Done` cycle with artifacts, where a `Check(cmd)` acceptance
  completes with **zero model calls** in the verification path; a status question
  costs **zero worker turns**; a question that needs facts gets an immediate
  "checking" entry and a follow-up when the scout reports.
- **K4 — the surface.** Board + worker lanes + chief inspector + click-through
  to a transcript and to an artifact in the existing panes, plus the attention
  surface. Acceptance: a `decision_needed` renders with its recommendation and
  the paused set; a `Blocked(wait: human)` task shows the strip; nothing fires
  an OS notification.
- **K5 — durability + policy.** Restart mid-flight, standing approvals, verify
  policy, cost rollup, per-slot budgets, and the per-lane worktree that relaxes
  `I11` from "must not overlap" to "must not overlap inside one worktree" (§6.4).

The K2 acceptance pair — *a chief cannot work, and its window does not grow with
the ledger* — is the acceptance test for this design. The rest is arrangement.

## 15. External survey (2026-09-21)

> Researched 2026-09-21 from primary sources — docs, specs, repos, papers —
> unlike `AGENT_CORE.md` §12's 2026-09-11 pass, which predates most of what
> follows. Two purposes: cite the field honestly, and record what it **changed**
> in this document (§15.3). It is not a bibliography.

### 15.1 The field at a glance

| system | board | dispatcher | orchestrator's tools | isolation | verification | human |
| --- | --- | --- | --- | --- | --- | --- |
| **Hermes Kanban** (Nous Research) | SQLite `kanban.db` per board; `triage\|todo\|ready\|running\|blocked\|review\|done\|archived`, `task_links` deps, comments as the protocol | code loop, 60 s tick: reclaim → promote → atomic claim → spawn | "an orchestrator is a Hermes profile whose toolset includes `kanban` but excludes `terminal` / `file` / `code` / `web`" — a *recommendation*, not a hard contract | `scratch` (deleted on completion) / `dir:<abs>` / `worktree`; **no path-conflict check** | a `review` card the dispatcher claims with a bundled `sdlc-review` skill; PR "completion contracts" re-read required GitHub checks | a peer: the human drives the same board by CLI, slash command and dashboard |
| **openai/symphony** | an external issue tracker *is* the board (Linear / GitHub / Jira / Asana / GitLab); no orchestrator DB by design | code; "the orchestrator is the only component that mutates scheduling state"; `agent.max_concurrent_agents` 10 | none — it is not an agent at all | per-issue workspace + three path invariants + `workspace-write` sandbox default | none in the spec (delegated to `WORKFLOW.md` policy) | tracker handoff states (`Human Review`); a blocked/approval run stays claimed and is surfaced, never left stalled |
| **Magentic** (AutoGen → MS Agent Framework) | a *task ledger* (facts, guesses, plan) + a *progress ledger* (satisfied / looping / progressing / next speaker) | an LLM manager picks the next speaker every round | n/a — the manager is an agent over a fixed roster | specialists isolated (Docker recommended) | "insufficient-verification-steps" is a documented top failure mode | `RequirePlanSignoff` |
| **Roo Code** "Orchestrator" | none (an in-context task tree) | LLM, one `new_task` per subtask | **"No direct tool access"** | context only | none | approve each subtask's creation and completion |
| **Claude Code agent teams** | session task list: 3 states, deps, file-locked claims | the lead, **or** self-claim | the lead is an ordinary session | own context window per teammate | `TaskCompleted` hooks may refuse; permission prompts the teammate cannot approve | talk to any teammate directly |
| **Beads / Gas Town** | a git+Dolt-backed dependency graph (`bd ready`, `bd update <id> --claim`) | Gas Town: capacity-controlled polecat dispatch (`scheduler.max_polecats`) over ephemeral "sling context" beads | — | worktrees | shipped-work gate (a pushed commit) | `gt escalate` severities |
| **Copilot CLI `/fleet`** | SQL `todos` + `todo_deps`, `pending → in_progress → done` / `blocked` | the main agent (LLM); workers also self-claim a row | the main agent | one shared filesystem — "If two agents write to the same file, the last one to finish wins—silently" | the orchestrator verifies | `/tasks` panel (machine-readable, not mediated) |
| **Temporal / Restate / DBOS** | the engine's journal + a task queue | worker-pull; no orchestrator | — | — | — | — |
| **`omp`** (the harness this repo was developed with, `AGENT_CORE.md` §1) | a `todo` the *working* agent keeps | none | the worker *is* the orchestrator (a prompt, not a role) | none | none | the chat |

### 15.2 The four arguments that shaped the field

- **Orchestrator-worker, measured** — Anthropic, *How we built our multi-agent
  research system* (2025-06-13). Lead agent + parallel subagents beat single-agent
  Opus 4 by **90.2%** on their internal research eval; the cost is real and
  stated: "agents typically use about 4× more tokens than chat interactions, and
  multi-agent systems use about 15× more tokens than chats", and "most coding
  tasks involve fewer truly parallelizable tasks than research". Three findings
  this design borrows: the lead **drops its plan into a memory** because a
  >200k-token context gets truncated (§7.2/§7.4); subagent output goes **to a
  filesystem** and only "lightweight references" come back (§3.2); and
  synchronous delegation is a named bottleneck — "the lead agent can't steer
  subagents, subagents can't coordinate, and the entire system can be blocked
  while waiting for a single subagent" (§7.6).
- **Context engineering, measured** — Manus, *Context Engineering for AI Agents*
  (2025-07-18): KV-cache hit rate as the first metric ($0.30/MTok cached vs
  $3/MTok uncached = 10×), "mask, don't remove" for tools, the filesystem as
  context with **restorable** compression, recitation via a rewritten `todo.md`,
  and keeping errors in the context as evidence. §7.7 and §7.1 are its direct
  consequences.
- **The reversal** — Cognition, *Don't Build Multi-Agents* (2025-06-12) →
  *Multi-Agents: What's Actually Working* (2026-04-22). The class that works:
  "setups where multiple agents contribute intelligence to a task while **writes
  stay single-threaded**", and the practical shape is "map-reduce-and-manage",
  with "unstructured swarm[s] … mostly a distraction". Two findings this design
  treats as rules: the generator-verifier loop works best when the two agents
  **share no context** ("Devin Review catches an average of 2 bugs per PR, of
  which roughly 58% are severe", attributed to *context rot* in the author's long
  context) — §5; and a manager without codebase context "defaults to being overly
  prescriptive, which backfires" — §5's brief rule.
- **Two ledgers and a stall counter** — Magentic-One (arXiv 2411.04468) as
  shipped in Microsoft Agent Framework: `max_round_count` 10, `max_stall_count`
  3, `max_reset_count` 2, `RequirePlanSignoff`. The separation of *what we are
  trying to do* (task ledger) from *what we did* (progress ledger) is this
  design's §4 board + §7 digest, and the stall counter is §6's stall detection —
  except that here the counter trips the **dispatcher**, not a manager LLM.

### 15.3 What the survey changed here

1. **§7.7 is new — the chief's view obeys the KV-cache, not just the token
   count** (stable prefix, volatile tail, no timestamps, deterministic digest
   rendering, batched note writes, static tool schema). This was simply missing.
2. **§4/§5 — evidence before verification.** A task's `accept` now names *how* it
   is checked (`Check(cmd)` / `Contract(ci)` / `Worker`), replacing "a verifier
   worker checks it" as the only arm. `hermes-agent` ships all three; a
   machine-checked acceptance is cheaper and unfalsifiable by argument.
3. **§5 — the verifier shares no context, as a rule** (Cognition's finding, with
   its numbers) and **a brief never prescribes steps** (their manager-Devin
   finding). Both were implicit before; both are now `I7`-class rules.
4. **§8.2a is new — the human is not a client of the chief.** The board accepts
   human writes directly (author = human), and the chief is notified rather than
   consulted. The first draft made every human action flow through the chief,
   which the whole field contradicts and which turns the chief into the
   bottleneck the Replicas guide warns about ("dispatch volume can outrun review
   capacity").
5. **§6.4 keeps its path-claim check, with the honest framing**: the field
   answers same-file concurrency with *advice* (Copilot's prompt discipline,
   Symphony's one-directory-per-issue, `swarm-protocol`'s "advisory, not
   enforced", `hermes-agent`'s `hotspot:` comments). We enforce; that is a
   genuine delta and the only one in the survey with no direct precedent.
6. **§6.3 takes both halves of the field's liveness machinery** — a lease *and* a
   periodic tick — with published reference values: Hermes' 15-minute claim TTL
   extended while the PID lives, 1-hour heartbeat backstop, 4-hour stale reclaim,
   60 s tick, 30-minute `stranded_in_ready` warning; Symphony's 5-minute
   `stall_timeout_ms`, 1 s continuation retry, `min(10 s × 2^(n−1), 5 min)`
   backoff. (The field's own honest note: leases are *rarer* than boards — the
   common pattern is watchdogs plus reconciliation.)
7. **§6.1 records the roster trade**: Magentic fixes its roster ("unused
   specialists distract it"), Hermes and we allow growth; ours stays
   human-approved and capped.
8. **§6/§5 — the workspace-per-task standard** (Symphony's per-issue workspace,
   Hermes' `scratch` / `dir` / `worktree`) is named as the K5 upgrade to `I11`,
   not an innovation of ours.
9. **§7.5 — intake entries carry an `idem` key** (Hermes' idempotency keys) so a
   replayed automation cannot create a task twice.

### 15.4 What has no precedent — and the two places the field says we are wrong

- **The bounded, code-rendered board digest has no precedent.** No surveyed
  system renders a board into an *LLM* orchestrator's context as a bounded
  artifact. The field's three answers, all of which we are rejecting, are: do not
  give the orchestrator read tools at all (Roo Code, explicitly "to prevent
  context poisoning" — a partial substitute for bounding its *inputs*), do not
  use an LLM orchestrator (Symphony's JSON status surface is for humans), or
  accept unbounded session context (Copilot `/fleet`, Codex). Citing those as
  precedent for §7 would be dishonest; they are the alternatives §7 exists to
  beat, and the K2 assertion (digest ≤ 2k tokens at a 200-task board, flat as the
  ledger grows) is the only thing that can settle it.
- **A durable append-only *log* is a divergence too.** Symphony deliberately has
  no database ("tracker/filesystem-driven restart recovery without requiring a
  persistent database"); Copilot's board is session-scoped. `I1` is a bet that a
  local-first app should own its own log (§12's differentiator) — and that bet is
  also what makes `I4`'s eviction exact.
- **The single human channel is unusual** — and §8.2a is the correction: the
  chief is the only *agent* that speaks to the human, not the only *path* to the
  board.
- **One counter-argument kept in view**: `openai/symphony` is, architecturally,
  the closest thing to §6 in the survey (a code orchestrator, a claim set, a
  stall watchdog, a human handoff state, per-issue isolation) and it needs **no
  LLM orchestrator at all**. The honest reason to keep one here is §8: an
  assistant that answers the human, decomposes a goal, and adjudicates a blocker
  is a language-model job, and the dispatcher is kept strictly mechanical so that
  judgement never enters the mechanical path (`I8`).
