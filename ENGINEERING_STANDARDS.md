# ENGINEERING_STANDARDS.md — Lexsus functional design, clean code and verification handbook

> **Repository reading contract:** Read this handbook at the start of a fresh coding session alongside [`AGENTS.md`](AGENTS.md), [`lexsus.md`](lexsus.md), and [`SESSION_CONTEXT.md`](SESSION_CONTEXT.md). Apply relevant rules to the requested change. Existing violations are migration work, not grounds for unrelated rewrites. Keep task progress in session context and durable engineering guidance here.

> **Status:** Proposed repository engineering standard; adapt to the checked-out code before enforcing as CI policy.
>
> **Applies to:** Tauri/Rust services, React/TypeScript desktop UI, MCP protocol adapters, local persistence, and optional Python services.
>
> **Companion:** [`AGENTS.md`](AGENTS.md) is the concise operational entry point for coding agents. This document is the detailed reference for humans and agents.
>
> **Source note:** The functional-programming ideas are paraphrased from *Functional Programming in Scala* (Paul Chiusano and Rúnar Bjarnason, Manning, 2015), with particular attention to chapters 1, 3–4, 6–8, 10–13, and 15. The Rust/TypeScript designs and Lexsus-specific rules here are **our engineering adaptations**, not excerpts from the book.

---

## 1. Why this standard exists

Lexsus is a security-sensitive local software agent and continuity engine. It reads developer materials, archives AI-assisted work, offers MCP tools and may execute commands on behalf of another client. A function that is merely convenient to write can become dangerous when its inputs come from untrusted transcripts or a remote agent.

**Quality is observable behavior under ordinary and adversarial conditions**: deterministic context reconstruction, comprehensible errors, safe permission boundaries, correct recovery, small reviewable patches and evidence that the next agent can continue a task.

The goal is **not** to maximize functional jargon, enforce an arbitrary number of lines per function, introduce an unnecessary framework, or rewrite working Rust as Scala. The goal is to preserve the reasoning benefits of functional design while using the existing technologies effectively.

### 1.1 Classification of rules

| Label | Meaning | Example |
|---|---|---|
| MUST | Required for safety/correctness or review | Authorization checked before executing a tool |
| SHOULD | Default unless a documented reason exists | Domain transformations written as pure functions |
| MAY | Optional technique when complexity justifies it | Property-based testing library for large generators |
| REPORTED | Described by a historical source review | Registered tools, current dashboard, compression stub |
| VERIFIED | Observed in the currently running checkout | Passing command with captured output |
| PROPOSED | Design proposal not proven in the product | New handoff versioning contract |

**Never** convert `REPORTED` or `PROPOSED` into `VERIFIED` merely by writing a document.

### 1.2 Engineering constraints specific to Lexsus

- A source review reported Tauri 2, React/TypeScript/Vite, Rust, SQLite, Claude Code JSONL ingestion, a local MCP server and optional FastAPI compression.
- The review did not run tests, connect actual providers, examine a release build or perform a security audit.
- It reported 58 registered tool names; registration is not validation.
- It reported that `delegate_task` explicitly fails and `/compress` returns HTTP 501.
- It reported a single active desktop dashboard and deleted older workbench screens.
- It reported plain SQLite storage, no full OS sandbox for commands, and some incomplete grant policies.
- Therefore implementation guidance here is **targeted to these modules but must be checked against the current checkout**.

---

## 2. Lessons from *Functional Programming in Scala*

The book's central discipline is to represent computations as transformations of values, reducing uncontrolled effects. It treats purity and referential transparency as tools for modular reasoning and derives structures for errors, state, parallelism, testing, I/O and streaming.

| Book topic | Relevant chapter | Lexsus engineering interpretation |
|---|---:|---|
| Pure functions / referential transparency | 1 | Same transcript + same policy + same revision => same extracted candidate facts |
| Functional data structures | 3 | Build new handoff state instead of mutating shared global session objects |
| `Option` / `Either` error values | 4 | Explicit `Result` / discriminated errors instead of swallowing parser failures |
| Stateful programs modeled functionally | 6 | Use `(state, event) -> (new_state, effects)` for handoff and authorization transitions |
| Pure parallelism / laws | 7 | Make independent reads composable without hiding order-sensitive effects |
| Property-based testing | 8 | Test invariants across generated paths, transcripts, command names and records |
| Parser combinators | 9 | Separate syntax parsing, semantic validation and useful error locations |
| Monoids | 10 | Combine evidence collections with explicitly defined associative rules |
| Monads and applicability | 11–12 | Compose possibly failing steps without prematurely performing I/O |
| External effects / I/O | 13 | Keep filesystem, database, network and process effects at the boundary |
| Local mutation | 14 | Encapsulate necessary mutation behind predictable pure interfaces |
| Incremental I/O | 15 | Process large transcripts in bounded chunks and close resources on all paths |

### 2.1 What the book does **not** prove

- It does not establish which programming language or effect library Lexsus must use.
- It does not validate a particular MCP tool or security architecture.
- It does not claim immutability removes the need for cancellation, locks, transactions or authorization.
- It does not replace end-to-end tests with equational reasoning alone.
- It does not establish that a concise one-line expression is more readable than a named sequence of transformations.

### 2.2 Preferred mental model

```text
Untrusted external data
       |
       v
[ Decode / parse / validate ]      <- effects only to obtain input
       |
       v
[ Pure domain transformation ]     <- explicit arguments and return values
       |
       v
[ Decide intended effects ]        <- no command has run yet
       |
       v
[ Authorize with live policy ]     <- execution-time scope, user grant
       |
       v
[ Perform scoped I/O ]             <- filesystem / Git / SQLite / MCP
       |
       v
[ Audit result and render state ]   <- preserve cause, outcome, next action
```

---

## 3. Write better functions: a repeatable design procedure

Before implementing any nontrivial function, answer these questions in its design note or test names.

1. **Purpose:** What single observable decision or transformation does this function own?
2. **Inputs:** Which required values must be supplied rather than fetched implicitly?
3. **Output:** What meaningful type is returned on success?
4. **Failure:** What exactly can go wrong and how is it represented?
5. **Invariant:** What condition must remain true for every valid invocation?
6. **Effects:** Does the function read state, mutate files, call network services or spawn processes?
7. **Determinism:** Can inputs be replayed to reproduce output?
8. **Resource bound:** What is the worst-case memory, CPU, I/O or time cost?
9. **Security:** What must be authorized and checked at the final effect boundary?
10. **Test:** What smallest test exposes a bad implementation?

### 3.1 The function-quality rubric

A good function generally has:

- A name communicating its decision (`select_latest_confirmed_fact`, not `processData`).
- A signature that forces callers to supply all domain dependencies.
- Inputs as narrow domain types, not a map of arbitrary strings.
- One stable level of abstraction: do not mix parsing with subprocess execution.
- No hidden calls to the clock, environment or global database when pure behavior is intended.
- Explicit errors, not special success-looking values such as `""` or `[]` after failure.
- No accidental logging of secrets or full customer context.
- Tests for the success path, empty path, invalid path and a boundary case.

### 3.2 Don't worship a line-count limit

A five-line wrapper can hide three network effects; a thirty-line pure state transition may be easier to understand. Extract a function when the named abstraction clarifies intent or isolates a genuine responsibility. Do not split functions solely to satisfy a mechanical length rule.

### 3.3 Minimize the number of states the program can inhabit

Bad data models permit contradictory states: `is_complete=true` alongside `missing_context=true`, or `approved=true` with `approved_by=None`. Better models encode a closed set of states and required fields with enums/unions.

### 3.4 Separate query from command

- **Query:** Determine information or compute a proposal; must not mutate external resources.
- **Command:** Attempt an authorized state-changing effect; return an auditable outcome.
- One MCP tool must not be described as read-only if its implementation creates persistent records, invokes approvals with side effects, or mutates project files.

---

## 4. Referential transparency applied to session memory

A pure function always produces equivalent output from equivalent input and causes no externally observable effect. This is useful for handoff reconstruction because users need to trust why the state looks a certain way.

### 4.1 TypeScript — avoid hidden state

**Bad:**

```ts
let currentSession: Session | undefined;

function summarizeCurrentSession(): string {
  // Meaning changes with an unrelated global assignment.
  return currentSession?.messages.map((m) => m.text).join("\n") ?? "";
}
```

**Better:**

```ts
type Message = Readonly<{ kind: "user" | "assistant"; text: string }>;
type Session = Readonly<{ messages: readonly Message[] }>;

function visibleMessages(session: Session): readonly string[] {
  return session.messages
    .filter((message) => message.kind === "user")
    .map((message) => message.text.trim())
    .filter((text) => text.length > 0);
}
```

- The improved function is deterministic given the supplied session.
- It does **not** make the extracted statements true; provenance and confidence are separate obligations.
- The reader/parser supplying `Session` may still perform I/O; that effect belongs in an adapter.

### 4.2 Rust — compute facts before persisting them

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateFact {
    pub text: String,
    pub source_line: usize,
}

pub fn extract_marked_facts(lines: &[String]) -> Vec<CandidateFact> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            line.strip_prefix("DECISION: ").map(|text| CandidateFact {
                text: text.to_owned(),
                source_line: index + 1,
            })
        })
        .collect()
}
```

This deliberately extracts **candidates**, not verified decisions. Calling a text marker an authoritative user decision would be a product bug.

### 4.3 Handling time, randomness and process state

- Inject timestamps as arguments to pure logic; record which clock produced them.
- Inject ID generators at the outer boundary; do not call random global state from core transformations.
- Inject `git HEAD`, file hashes or working-tree snapshots before state evaluation.
- When a runtime snapshot changes during extraction, either retry consistently or mark the handoff as non-atomic.

---

## 5. Data modeling: replace primitive obsession with domain types

The following domain concepts should not be exchanged as arbitrary strings:

- `ProjectId`, `SessionId`, `HandoffId`, `GrantId`.
- `WorkspaceRelativePath` distinct from an absolute canonical filesystem path.
- `SourceEvidenceRef` distinct from user-facing free text.
- `GitRevision` distinct from an unverified branch name.
- `PermissionClass` and `ActionScope` distinct from a UI label.
- `ConfirmedFact` distinct from `ExtractedCandidate`.

### 5.1 Rust newtypes

```rust
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn parse(raw: &str) -> Result<Self, &'static str> {
        let valid = !raw.is_empty()
            && raw.len() <= 80
            && raw.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if valid { Ok(Self(raw.to_owned())) } else { Err("invalid project id") }
    }

    pub fn as_str(&self) -> &str { &self.0 }
}
```

- Keep constructors responsible for enforcing invariants.
- Avoid `pub` fields on validated newtypes if callers could bypass checks.
- A newtype should clarify an actual semantic boundary, not create dozens of one-use wrappers.

### 5.2 TypeScript discriminated unions

```ts
type Evidence = Readonly<{
  source: string;
  locator: string;
  capturedAt: string;
}>;

type MemoryFact =
  | Readonly<{ kind: "candidate"; text: string; evidence: Evidence }>
  | Readonly<{ kind: "confirmed"; text: string; evidence: Evidence; reviewer: string }>
  | Readonly<{ kind: "rejected"; text: string; evidence: Evidence; reason: string }>;

function mayEnterVerifiedSummary(fact: MemoryFact): boolean {
  return fact.kind === "confirmed";
}
```

A runtime validator must still validate JSON arriving over IPC/MCP; TypeScript static types do not validate network bytes.

### 5.3 Invariants to encode

- A confirmed fact must identify the confirming actor or event.
- Evidence references must resolve or be clearly marked unresolved.
- A handoff cannot claim a successful test without an observed test result.
- A permission grant must have an owner, action scope and expiry or explicit session lifetime.
- A denied action must never be represented as a successful no-op.
- Storage schema versions must be monotonically tracked and migration-safe.

---

## 6. Errors are values, not surprising control flow

The book's `Option` and `Either` chapter motivates representing absence and failure explicitly. In Lexsus, error types are also part of user trust: a blocked operation needs an understandable reason and a recoverable next action.

### 6.1 Distinguish absent, invalid, denied and unavailable

| Case | Meaning | Correct result |
|---|---|---|
| Optional transcript is missing | Data genuinely absent | `None` / `undefined` where expected |
| Transcript is malformed | Data cannot be safely parsed | Typed parse error |
| Path is out of scope | Authorization failure | Denied error, no side effect |
| Remote client disconnected | Transport failed | Retryable transport error if safe |
| Compression not implemented | Capability unavailable | Explicit unsupported/unavailable result |
| Handoff has no evidence | Product-quality limitation | Reviewable incomplete state, not fabricated context |

### 6.2 Rust `Result` with context

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum HandoffError {
    EmptyObjective,
    MissingEvidence,
    RevisionConflict,
}

#[derive(Debug, PartialEq, Eq)]
pub struct HandoffDraft {
    pub objective: String,
    pub evidence_count: usize,
}

pub fn draft_handoff(objective: &str, evidence_count: usize)
    -> Result<HandoffDraft, HandoffError>
{
    let objective = objective.trim();
    if objective.is_empty() { return Err(HandoffError::EmptyObjective); }
    if evidence_count == 0 { return Err(HandoffError::MissingEvidence); }
    Ok(HandoffDraft { objective: objective.to_owned(), evidence_count })
}
```

- `?` is appropriate at effectful boundaries when propagating an error without losing useful context.
- `unwrap`/`expect` are not appropriate for parsing untrusted transcripts or remote MCP input.
- Converting every failure to one generic string makes client recovery worse.

### 6.3 TypeScript result shape

```ts
type Result<T, E> =
  | Readonly<{ ok: true; value: T }>
  | Readonly<{ ok: false; error: E }>;

type HandoffInputError =
  | Readonly<{ kind: "empty-objective" }>
  | Readonly<{ kind: "missing-evidence" }>;

function validateDraft(objective: string, evidenceCount: number):
  Result<{ objective: string; evidenceCount: number }, HandoffInputError> {
  const normalized = objective.trim();
  if (normalized.length === 0) return { ok: false, error: { kind: "empty-objective" } };
  if (evidenceCount <= 0) return { ok: false, error: { kind: "missing-evidence" } };
  return { ok: true, value: { objective: normalized, evidenceCount } };
}
```

- Do not wrap all normal synchronous TypeScript errors in a homegrown monad if discriminated unions are sufficient.
- Never confuse a **denied** action with a **canceled** action; users need different recovery guidance.

---

## 7. Immutable state transitions and explicit effects

Model important workflows as state machines: `transition(state, event) -> transition_result`. This adapts the book's functional state material to real desktop operations.

### 7.1 Handoff lifecycle

```text
NotCaptured
    | capture requested
    v
Capturing --failure--> CaptureFailed
    | capture completed
    v
Drafted --edit/review--> Drafted
    | user confirms
    v
ApprovedForSharing
    | destination consumes handoff
    v
Delivered --agent action verified--> Continued
    | transport failure
    v
DeliveryFailed (retry with same handoff ID)
```

- The product must record what `Continued` means; merely fetching a handoff does not meet that state.
- A handoff can be superseded by a newer snapshot; an old handoff remains auditable, not silently mutated.
- A revoked grant can invalidate future execution without deleting historical audit facts.

### 7.2 Pure transition example

```ts
type HandoffState =
  | { kind: "draft"; revision: string }
  | { kind: "approved"; revision: string; approvedBy: string };

type HandoffEvent =
  | { kind: "approve"; actor: string; currentRevision: string }
  | { kind: "invalidate"; currentRevision: string };

type TransitionError = "revision-conflict" | "already-approved";

type Transition = { ok: true; state: HandoffState } |
  { ok: false; error: TransitionError };

function transition(state: HandoffState, event: HandoffEvent): Transition {
  if (state.revision !== event.currentRevision) {
    return { ok: false, error: "revision-conflict" };
  }
  if (event.kind === "invalidate") {
    return { ok: true, state: { kind: "draft", revision: event.currentRevision } };
  }
  if (state.kind === "approved") {
    return { ok: false, error: "already-approved" };
  }
  return { ok: true, state: {
    kind: "approved", revision: state.revision, approvedBy: event.actor,
  }};
}
```

An actual approval **must also be authenticated and authorized at the boundary**. The pure transition checks domain consistency, not identity or permission by itself.

### 7.3 Effects as a plan

- Step A: validate request and state without I/O.
- Step B: compute proposed effect and required scope.
- Step C: obtain approval or evaluate a valid existing grant.
- Step D: acquire current project state and recheck preconditions to prevent time-of-check/time-of-use errors.
- Step E: perform the effect once; use an idempotency key when retries are possible.
- Step F: record action outcome, not merely intent, then update UI.

---

## 8. Pure core, imperative shell: practical layering

**Domain core**

- Validates data and invariants.
- Resolves evidence ordering and conflicts.
- Computes projected handoff sections.
- Determines whether an action requires approval.
- Produces effect requests without executing them.

**Application orchestration**

- Coordinates reads, grants, state snapshots, transactions and cancellation.
- Converts domain effects into calls to adapters.
- Ensures output and audit events correspond to what actually happened.

**Adapters and gateways**

- Read transcript JSONL with strict resource limits.
- Interact with SQLite, Git, operating-system processes and local files.
- Expose MCP request/response schemas and Tauri commands.
- Handle network connectivity, TLS, OAuth, timeouts and retries.

**Presentation**

- Displays state and evidence.
- Accepts human decisions and shows permission scope.
- Never bypasses backend validation or determines effective authorization.

### 8.1 Anti-pattern: React component as business controller

Avoid mixing transcript scanning, permission evaluation, MCP call construction and UI rendering in `DashboardView`. Put deterministic derivation in a pure module; put Tauri interaction in a typed boundary; render a view model.

### 8.2 Anti-pattern: MCP endpoint as catch-all executor

Avoid a single handler that parses arbitrary JSON, infers permissions, modifies files, stores memory and renders user messages. Break apart decoding, authorization, command selection, execution and response mapping.

### 8.3 Dependency direction

```text
React component --> typed UI service --> Tauri boundary
                                         |
                                         v
                                 application use case
                                    /         \
                         domain rules       adapter traits
                                              |
                                     file/db/git/MCP impls
```

The domain layer must not import UI components, SQLite connection types or MCP transport objects.

---

## 9. Design small composable APIs, not giant tool registries

A good API provides minimal, coherent operations that are easy to test and difficult to misuse.

### 9.1 MCP tool envelope (proposed shape, not a verified existing API)

```json
{
  "project_id": "project-123",
  "handoff_id": "handoff-456",
  "requested_revision": "git-or-snapshot-revision",
  "idempotency_key": "request-789"
}
```

Requirements for any new endpoint:

- Identify the project explicitly; do not silently use whichever window is focused.
- Validate identifiers and request size before parsing expensive nested structures.
- Enforce permissions server-side for the *actual* operation.
- Return typed failure classes and omit sensitive information from error messages.
- For state-changing actions, define retry, duplication and cancellation semantics.
- Version contracts or introduce additive fields with compatibility tests.
- Do not expose a tool if its implementation always returns an unsupported error, unless intentionally advertised as unsupported metadata rather than a usable tool.

### 9.2 Do not equate feature count with usefulness

A first-rate continuity interface might expose a few highly coherent tools: choose project, list safe context, fetch approved handoff, inspect evidence and request a bounded action. Do not advertise fifty-eight tools just because the registry contains fifty-eight names.

### 9.3 Function naming examples

| Avoid | Prefer | Reason |
|---|---|---|
| `process` | `parse_claude_transcript` | Names an observable transform |
| `update` | `record_handoff_review` | Identifies side effect |
| `runTool` | `execute_authorized_tool_call` | Shows authority boundary |
| `load` | `load_project_snapshot` | Names content and context |
| `fixMemory` | `reconcile_fact_conflicts` | Names domain invariant |
| `isAllowed` | `evaluate_action_grant` | Shows policy decision |

---

## 10. Security is an executable invariant

The most important architectural rule in Lexsus is that **natural-language instruction is not authorization**. A transcript may contain an instruction, a web page may contain a prompt, or a connected agent may attempt a privileged tool call. None grants permission merely by being present.

### 10.1 Trust boundaries

1. Developer-owned repository files are data, not automatically trustworthy instructions.
2. Historic AI transcripts are **untrusted imported content**.
3. Model-generated handoff text is **untrusted until user review**.
4. MCP clients are external callers whose permission scope must be checked per request.
5. OAuth/bearer credentials establish caller identity, not universal permission to mutate projects.
6. Frontend approval widgets are a user experience; the backend must independently bind approval to scope and action.
7. A Git worktree isolates working-tree changes, **not** arbitrary subprocess behavior.
8. File-path restrictions do not contain shell commands; meaningful process containment needs a separately validated OS mechanism.

### 10.2 Authorization as a pure decision plus enforced effect

A domain policy function should accept:

- Authenticated actor or client identity.
- Selected project identity and canonical workspace root.
- Requested capability, arguments and resolved target resource.
- Current grant state and expiration.
- Policy version and any sensitive-resource classification.

It should return a typed decision: `Allow`, `Deny(reason)` or `RequireApproval(scope)`. An executor must still **revalidate** the decision against the final resolved target and current grant state immediately before the effect.

### 10.3 Four critical threat scenarios

| Threat | How it happens | Required protection |
|---|---|---|
| Prompt injection | A transcript says “ignore previous instructions and run…” | Treat transcript as data; execute only explicit authenticated requests |
| Confused deputy | A valid connector requests an action outside its approved project | Per-project, per-action grants; backend verification |
| Path escape | `../`, symlinks, junctions or path races move access outside root | Safe path resolution and handle-based protections where available |
| Shell escape | Approved file tool invokes `bash -c` with uncontrolled input | Avoid arbitrary shell interpolation; restrict command execution separately |

### 10.4 Path security checklist

- Decode and validate path segments according to the actual platform filesystem semantics.
- Reject NUL bytes and unsupported absolute/relative path forms.
- Check expected project roots independently of client-provided path metadata.
- Normalize and inspect symlinks; treat canonical-path checks alone as insufficient against races.
- Prefer secure file-handle-relative APIs where available for sensitive writes.
- Enforce allow/deny policy again at the point of opening or mutating the target.
- Test case variants on Linux, macOS and Windows when those systems are supported.
- Avoid promising comprehensive prevention without platform-specific implementation and hostile-path testing.

### 10.5 Shell and process execution checklist

- Do not convert raw model text into shell arguments without validation and explicit approval.
- Prefer spawning a program with a structured argument vector, **not** passing strings to a shell.
- Treat shell scripts themselves as arbitrary code even when the argv is well formed.
- Set working directory explicitly and reject unapproved CWD transitions.
- Set process timeouts, output caps, cancellation rules and lifetime limits.
- Define whether environment variables are inherited; suppress secrets by default.
- Log command metadata and outcomes, not unredacted environment variables or sensitive command output.
- Kill process trees where required, but be precise about OS limits.

### 10.6 Remote connector checklist

- Bind to loopback unless the user explicitly enables a suitable authenticated remote path.
- Treat tunnels as a new public attack surface, not a routine port toggle.
- Verify authorization code and PKCE behavior against actual host implementations.
- Test token expiry, reuse, revocation, audience, scope and concurrent sessions.
- Ensure UI displays whether exposure is active, who can connect, and how to stop it.
- Revoke permissions on project change or user request according to documented semantics.

---

## 11. Handoff memory: provenance before compression

A useful handoff is not a polished essay; it is a **reconstructable record** that lets a second agent identify the next safe action. The extraction process must distinguish:

- **Observed:** Git status, file hash, test output or tool result actually captured.
- **Reported:** Statement in an imported transcript, possibly incorrect.
- **Inferred:** Candidate produced by a heuristic or model.
- **Confirmed:** Reviewed and explicitly accepted by the user or an authorized actor.
- **Unknown:** Missing, contradictory or stale evidence that must not be filled with guesses.

### 11.1 Minimal reviewable handoff schema (illustrative)

```json
{
  "schema_version": 1,
  "project_id": "project-123",
  "handoff_id": "handoff-456",
  "snapshot": { "git_head": "optional-sha", "working_tree_dirty": true },
  "objective": { "text": "Fix the permission path", "status": "reported" },
  "constraints": [],
  "confirmed_decisions": [],
  "failed_attempts": [],
  "changed_files": [],
  "test_results": [],
  "open_questions": [],
  "next_action": { "text": "Run the denied-call test", "status": "proposed" },
  "evidence": []
}
```

The exact schema must be agreed with consumers before implementation. `git_head` alone cannot identify uncommitted file content.

### 11.2 Handoff invariants

- Every strong claim must carry resolvable evidence or visible human confirmation.
- Do not turn a model summary into a confirmed source merely because it is fluent.
- Tests are `passed` only if a recorded run proves the outcome for the given checkout.
- Commands are `executed` only if the execution adapter reports completion.
- A failed attempt includes **what was tried**, **observed outcome**, and **what it rules out**.
- The next action includes enough context to execute and verify, not only an aspirational sentence.
- Sensitive content is redacted before any intended cross-client delivery unless explicitly authorized.
- The user can review what will be shared and to which destination.

### 11.3 Conflict handling is not overwriting

If two transcript segments contain incompatible decisions, return both with timestamps and evidence. A deterministic reconciliation rule can prioritize explicit later confirmation, but should preserve the superseded state for review. Never silently delete dissenting evidence from history.

### 11.4 Compression should be optional

Until an actual compression engine is implemented and validated, use bounded, deterministic selection by:

1. User-confirmed objective and constraints.
2. Current working-tree status and changed files.
3. Last relevant failing tests.
4. Confirmed decisions and rejected approaches.
5. Unresolved blockers and next action.

A handoff that omits an important constraint for brevity is worse than a longer handoff with a clear structure. Measure both factual retention and downstream task performance.

---

## 12. Persistence, transactions and concurrent writers

Pure functions cannot make database race conditions disappear. The imperative shell must preserve invariants when multiple tools, windows or provider clients act on the same project.

### 12.1 Atomic state updates

- Read the current version within a suitable transaction.
- Validate expected version and actor scope.
- Apply related changes atomically when supported.
- Increment the version and append an audit record that refers to the action outcome.
- Return a revision-conflict error when stale state would otherwise be overwritten.
- Do not send success to the MCP caller before the durable operation is complete.

### 12.2 Optimistic concurrency

A revision check should compare the persisted version, not only the UI's local copy. A stale client must reload before retrying a conflicting write. Define what happens if two agents attempt to approve the same handoff concurrently.

### 12.3 Idempotency

- Use a caller-provided or generated idempotency key for operations that might be retried across lost connections.
- Scope the key to the authenticated caller and operation semantics.
- Persist the result long enough to answer duplicate attempts safely.
- Do not blindly retry non-idempotent shell or Git commands.
- Distinguish repeated request delivery from repeated authorization.

### 12.4 Migration design

- Every migration is reviewed for data preservation and downgrade consequences.
- Take backup/recovery expectations seriously for plaintext local archives.
- Run migrations on fixture databases containing realistic old schemas.
- Do not silently rewrite the entire archive on every startup.
- Document whether old clients can read records written by new clients.

### 12.5 Retention

- Define retention separately for transcripts, handoffs, logs and audit events.
- Provide deletion semantics that explain what remains in backups or external providers.
- Never promise encryption-at-rest until key management, migration and threat model exist.
- Protect private workspaces from inadvertent cloud synchronization.

---

## 13. Streaming and incremental parsing

Chapter 15 of the book motivates incremental transformations rather than loading a whole input when each record can be processed separately. This matters for large Claude Code JSONL archives.

### 13.1 Required parser behavior

- Read incrementally with a maximum line size and total-byte budget.
- Limit records, nesting depth and retained textual payloads.
- Validate JSON syntax and schema separately.
- Record source offsets or line numbers for recoverable provenance.
- Decide explicitly whether malformed lines stop the import or generate partial-import warnings.
- Ensure cancellation closes file handles and stops background work.
- Avoid silently dropping parse errors if doing so could change the meaning of a handoff.

### 13.2 Functional pipeline

```text
read bounded chunk (effect)
  -> decode JSON (Result)
  -> validate event type (Result)
  -> classify event (pure)
  -> select relevant evidence (pure)
  -> accumulate bounded state (pure transition)
  -> persist selected events (effect)
```

### 13.3 Resource bounds

State worst-case resource use in the requirement or test: maximum accepted transcript size, per-message limit, retained evidence count, and cancellation response. Don't claim O(1) memory if the accumulator holds all extracted messages.

---

## 14. Concurrency, cancellation and structured lifetimes

The book's parallelism chapter shows why a computation's API semantics matter. In a real desktop application, cancellation and owned lifetimes are equally important.

- Independent pure transformations may run concurrently if outputs are merged deterministically.
- Side effects with ordering dependencies must be sequenced explicitly.
- Give processes and background tasks a parent lifetime or cancellation token.
- Avoid orphaned children after a user presses “stop,” closes the project or revokes permissions.
- Beware of holding a mutex while waiting on an external process or network response.
- Keep critical sections short and never await arbitrary untrusted work under a lock.
- Define cancellation points and whether partial writes are rolled back.
- Use bounded queues; backpressure is preferable to unbounded event accumulation.
- Identify the exact consistency guarantee that the UI can display during an in-flight update.

### 14.1 Deterministic merge property

If evidence items from independent readers may arrive in different orders, sorting should use stable identifiers such as `(source_id, event_index)` or an explicitly documented chronological ordering. Arrival order from futures is not a durable history ordering.

### 14.2 Timeouts are not just UX

When a remote client hangs after requesting a privileged action, stale approvals, blocked process trees and ambiguous execution outcomes become security and correctness issues. Set and test timeout, cancellation and result-reconciliation semantics.

---

## 15. Test design: from examples to properties

A test suite should prove design invariants, not merely execute happy-path code. Chapter 8 of *Functional Programming in Scala* develops property-based testing and shrinking; adopt those principles using the language's current test ecosystem.

### 15.1 Layers

| Layer | Example | Typical mock needs |
|---|---|---|
| Pure unit | evidence deduplication, state transitions, scope decisions | None |
| Parser | valid, truncated, enormous, malformed JSONL | Fixtures |
| Adapter | SQLite transaction, process timeout, canonical paths | Temporary files/db |
| Protocol | MCP schema, auth denials, host compatibility | Test client |
| UI | source display, scope review, revoked-grant state | Stubbed backend |
| End to end | Claude Code interruption → reviewed handoff → real destination next action | Real supported providers |
| Security | traversal, TOCTOU attempts, token expiry and prompt injection | Isolated test environment |

### 15.2 Properties worth asserting

- **Determinism:** `derive(x) == derive(x)` for unchanged inputs.
- **Idempotence:** `normalize(normalize(x)) == normalize(x)`.
- **Stable serialization:** `decode(encode(x)) == x` for valid model values.
- **No privilege escalation:** narrowing a grant never increases allowed actions.
- **Denial safety:** denied calls leave protected state unchanged.
- **Deduplication:** merging evidence with itself does not invent new facts.
- **Conflict preservation:** adding contradictory evidence never erases historical provenance.
- **Monotonic audit:** a recorded effect outcome is not quietly deleted by an unrelated action.
- **Permission revocation:** no subsequent action succeeds using the revoked grant.
- **Recovery:** interrupting a transfer produces a meaningful incomplete state, not a fabricated success.

### 15.3 Concrete Rust property-like unit test

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracting_facts_is_repeatable() {
        let lines = vec![
            "DECISION: do not bypass authorization".to_owned(),
            "irrelevant line".to_owned(),
        ];
        let first = extract_marked_facts(&lines);
        let second = extract_marked_facts(&lines);
        assert_eq!(first, second);
        assert_eq!(first[0].source_line, 1);
    }

    #[test]
    fn draft_requires_evidence() {
        assert_eq!(
            draft_handoff("continue work", 0),
            Err(HandoffError::MissingEvidence)
        );
    }
}
```

These tests refer to the self-contained illustrative functions in sections 4 and 6; they do **not** claim current Lexsus source exports the same names.

### 15.4 Property-based and fuzz testing

- Generate valid and invalid JSONL records, Unicode sequences, long lines and record orderings.
- Generate relative-path segments including ambiguous normalization and platform separators.
- Generate state/event sequences and assert no illegal state is constructed.
- Generate grant/action combinations and verify deny-by-default behavior.
- Shrink failures to the smallest reproducer; store the seed in the regression suite.
- Fuzz parsers and native boundary code in isolated, bounded environments.
- Do not turn fuzzing on untrusted files into uncontrolled network or process execution.

### 15.5 Testing a pure function vs integration

For `render_handoff`:

- Unit: verify section ordering and redaction marker behavior.
- Property: output is deterministic when evidence ordering is canonical.
- Snapshot: verify stable readability for a fixture, but review semantics as well as text.
- Integration: verify the produced document is reachable through a real MCP client.
- E2E: verify the receiving agent performs the next appropriate action.

A snapshot test proves formatting, not correctness of inferred facts.

### 15.6 Test names should read as contracts

Prefer `denied_write_does_not_modify_workspace` over `test_write2`, and `handoff_marks_failed_test_as_failed` over `test_summary`.

---

## 16. Practical TypeScript / React rules

- Prefer `strict` TypeScript configuration where the existing project can adopt it responsibly.
- Represent asynchronous UI states as unions (`idle` / `loading` / `ready` / `error`), not unrelated booleans.
- Keep transport DTOs distinct from validated domain values.
- Derive UI text from state instead of caching redundant copies in separate React state.
- Avoid `any` for MCP/IPC messages; use `unknown` at ingress and validate.
- Keep `useEffect` for synchronization with external systems, not pure state derivation.
- Keep event handlers thin; compose a domain action rather than embedding large business decisions in JSX.
- Never infer backend permission from whether a button is enabled.
- Avoid accidental retries of mutating calls caused by lifecycle rerenders.

### 16.1 UI asynchronous state model

```ts
type LoadState<T> =
  | { kind: "idle" }
  | { kind: "loading"; requestId: string }
  | { kind: "ready"; value: T }
  | { kind: "error"; message: string; retryable: boolean };

function summaryLabel<T>(state: LoadState<T>): string {
  switch (state.kind) {
    case "idle": return "Not loaded";
    case "loading": return "Loading";
    case "ready": return "Ready";
    case "error": return state.retryable ? "Try again" : "Action required";
  }
}
```

Use an exhaustive compile-time check when a union evolves. Runtime input validation still belongs at the IPC boundary.

### 16.2 Accessibility for consent

A grant dialog should say **which client**, **which project**, **which operation**, **which resources**, **how long**, and **how to revoke**. Focus management, keyboard activation and readable error feedback are part of correctness.

---

## 17. Practical Rust rules

- Use `Result<T, E>` for expected failures; recoverable errors must not panic.
- Use `Option<T>` only when absence is an expected, non-error state.
- Prefer explicit ownership and borrowing over clones of very large transcript structures.
- Design typed enums for permission and handoff states.
- Keep `unsafe` out of new code unless there is a documented necessity and focused review.
- Avoid performing blocking process work on an async runtime thread.
- Keep mutexes, filesystem handles and child-process ownership scoped.
- Avoid reusing a mutable `Arc<Mutex<State>>` as a universal domain API; expose narrow operations.
- Use crate or module interfaces that reveal capabilities without exposing internal handles.
- Preserve error sources/context at adapters without leaking sensitive payloads to UI.
- Prefer structured argv when spawning a program; avoid shell interpolation of user text.

### 17.1 Separate validation from effect

```rust
#[derive(Debug, PartialEq, Eq)]
pub struct RequestedRead { pub relative_path: String }

pub fn validate_relative_read(raw: &str) -> Result<RequestedRead, &'static str> {
    use std::path::{Component, Path};

    let path = Path::new(raw);
    if raw.is_empty() || path.is_absolute() {
        return Err("path must be relative");
    }
    if path.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err("path has unsupported components");
    }
    Ok(RequestedRead { relative_path: raw.to_owned() })
}
```

**Security qualification:** this example only validates a syntactic shape. It is **not sufficient** to prevent symlink escapes, races, Windows aliasing or command-based escape. Real secure access must resolve and enforce the target at the I/O boundary.

---

## 18. Optional Python service rules

The source review identifies Python/FastAPI as an optional compression service with a 501 endpoint. Treat compression as unavailable until tested. If Python code is introduced:

- Keep core handoff usable when the service is stopped or missing.
- Use typed request and response models with bounded input sizes.
- Keep text selection and scoring in testable pure functions.
- Place model/network calls in adapters with explicit timeouts and cost budgets.
- Never claim a model compression result is guaranteed to preserve facts.
- Preserve original evidence references through any abstraction or summarization.
- Return an explicit unavailable result rather than manufacturing a compressed handoff.
- Avoid requiring another cloud subscription for the basic local continuation journey.

---

## 19. Error messages and observability

Every reported error should answer:

1. What operation failed?
2. What broad cause category is known?
3. Was anything changed?
4. Can it be retried safely?
5. What user action would resolve it?

### 19.1 Error shape (proposed)

```ts
type UserFacingFailure = Readonly<{
  code: "DENIED" | "INVALID_INPUT" | "STALE_STATE" | "UNAVAILABLE" | "IO_FAILURE";
  summary: string;
  changedState: "no" | "yes" | "unknown";
  retryable: boolean;
  correlationId: string;
}>;
```

Never turn an unknown side-effect outcome into `changedState: "no"`. A timed-out command may have completed after the transport stopped waiting.

### 19.2 Safe logs

Allow: operation name, opaque IDs, duration, auth decision category, path classification, redacted failure code, correlation IDs, result size and coarse workflow states.

Do not include by default: full prompts, transcripts, customer source files, OAuth tokens, shell environment, private Git URLs with credentials, copied clipboard contents or raw response bodies.

### 19.3 Useful metrics

- Handoff completed with review.
- Validated next action after handoff.
- Time to first useful action.
- Handoff rejection or correction rate.
- Permission-denied call rate and reason categories.
- Failed connector discovery and connection attempts.
- Timeouts, user cancellations and recovered retries.
- Storage migration and corruption failures.

Define numerator and denominator for every reported success percentage; collecting events is not equivalent to validating product benefit.

---

## 20. Clean architecture without abstraction inflation

Apply abstractions **after** identifying concrete duplicated business meaning, not merely because a design pattern sounds advanced.

### 20.1 Prefer cohesive modules

A `handoff` module should own handoff types, validity and transformations. Persistence implementation should be a separate adapter. The UI should consume a view model. A giant `utils.rs` or `helpers.ts` tends to hide domain boundaries.

### 20.2 Prefer small interfaces

A component that only needs `get_handoff` should not depend on a 58-tool administration client. Narrow interfaces improve testability and capability isolation.

### 20.3 Avoid premature generic frameworks

- Do not build a generic agent runtime to execute one handoff operation.
- Do not implement full monad/typeclass machinery in Rust/TS to imitate Scala's pedagogy.
- Do not create an event bus for a single synchronous state change.
- Do not introduce microservices or a new database merely to separate a simple module.
- Do not replace a boring explicit `match` with elaborate dynamic dispatch without clear benefit.

### 20.4 Comments and documentation

Good comments explain **why a surprising constraint exists** and cite the associated invariant or security finding. Avoid comments that only repeat the next line of code. Public functions and effectful adapters should document preconditions, error classes, side effects, resource limits and cancellation.

### 20.5 Naming and vocabulary

Use `source`, `evidence`, `candidate`, `confirmed`, `handoff`, `grant`, `scope`, `revision`, `effect`, `approval` consistently. Avoid interchangeable names such as `memory`, `context`, `session`, `fact` when they refer to different lifecycle objects.

---

## 21. How to refactor messy code safely

Refactoring is not a license to silently change product semantics, especially around authorization.

### Recipe A — mixed parsing and database mutation

**Before:** `importSession()` reads JSONL, mutates singleton state, inserts into SQLite, chooses “latest truth”, updates UI, and catches all errors as `null`.

**After:**

1. Introduce an explicit `ImportedEvent` data type with schema validation.
2. Separate `read_events` adapter from `classify_event` pure function.
3. Separate `derive_candidates(events)` from user confirmation.
4. Define transaction boundaries and idempotency for `persist_import`.
5. Return typed errors with evidence offsets.
6. Add golden fixtures and malformed-input regression tests.
7. Replace the old entry point incrementally while preserving behavior.

### Recipe B — giant Tauri command handler

1. Keep the exposed Tauri command signature compatible initially.
2. Move request decoding into typed input validation.
3. Move domain rules to pure core functions.
4. Move I/O to an application service with narrow adapter dependencies.
5. Add independent permission tests before any behavior changes.
6. Remove dead helper paths only after verifying callers.
7. Split compatibility cleanup into a subsequent, separately reviewable patch if needed.

### Recipe C — duplicated permission checks

1. Inventory *all* dispatch paths, including direct MCP, local UI and background tasks.
2. Write one formal policy matrix that includes resource and operation scope.
3. Implement a pure policy decision function.
4. Enforce it in the centralized executor and test alternate entry paths.
5. Ensure no bypass via helper functions or internal methods.
6. Record audit outcomes consistently without leaking sensitive values.
7. Test token expiry, grant revocation and changing resources between check and use.

### Recipe D — an opaque session summary

1. Extract facts as candidates with evidence references.
2. Preserve confirmed decisions separately from heuristics.
3. Detect contradictory facts and stale revisions.
4. Render a human-readable review UI.
5. Store approved snapshot and source hashes.
6. Compare continuation against the baseline manual `HANDOFF.md` process.

### Refactor safety rule

When a change mixes structural refactoring with functional change, reviewers may be unable to tell which behavior changed. Prefer small self-contained patches with tests; use an explicit staged migration when a large edit is unavoidable.

---

## 22. Review rules for AI-generated patches

A code review is not a style contest. Review the **risk and invariants first**.

### 22.1 Severity classes

- **Blocker:** unauthorized effect, secret leakage, misleading success, data corruption, uncontrolled remote exposure or lost state.
- **High:** broken protocol contract, unbounded parser/process, missing cancellation, race, migration risk or absent failure recovery.
- **Medium:** confusing domain coupling, duplicated rule, inconsistent vocabulary, weak tests or needless dependency.
- **Low:** naming, formatting or minor comment cleanup not affecting behavior.

### 22.2 Required questions

- Is the observed user problem clearly identified?
- Is the change the smallest coherent step toward it?
- Does it introduce a source of truth that competes with another one?
- Are side effects isolated and permission-checked at execution?
- Does the error surface explain what happened?
- Are tested cases the same cases the user cares about?
- Does a host-compatible end-to-end test exist, or is a limitation declared?
- Could the change leak code, secrets or transcripts via logs?
- Is the result independent of an unimplemented or speculative platform capability?

### 22.3 Feedback style

Review comments should name a file/location, reproduce or reason through failure, identify violated invariant and suggest the smallest correction. Avoid generic “improve readability” comments without a precise example.

---

## 23. Verification gates and CI policy

These are **proposed checks**: the current source review does not prove they run successfully in the latest checkout.

| Gate | Condition | Evidence |
|---|---|---|
| Formatting | Rust and frontend files conform to repository formatters | Actual command output |
| Static analysis | TypeScript strictness and Clippy rules appropriate to project pass | Recorded run |
| Unit tests | Pure domain logic and parsing tests pass | Recorded run |
| Integration | Storage, MCP, auth and execution adapters pass relevant negative tests | Test artifacts |
| Security | No bypass through direct or nested executor paths | Denial tests + review |
| Product | User completes capture → review → real continuation in selected client | E2E observation |
| Data | Handoff source evidence is retrievable and stale cases are visible | Fixture and UI test |
| Docs | README, agent instructions, permissions and onboarding match current UI/code | Review notes |

### 23.1 Command discovery

Check the actual repository scripts and toolchains before attempting commands. Examples (not verified here):

```bash
# Check root scripts rather than assuming npm is the chosen package manager.
cat package.json
# From Rust workspace / crate, if present:
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
# Only if scripts exist:
npm run lint
npm run build
```

Always report what ran and which checks were blocked. Never alter tests or security checks merely to obtain a green badge.

### 23.2 Test evidence template

```text
Checkout / revision: <actual hash or dirty-working-tree description>
Component and scenario: <precise target>
Command or method: <exact executed operation>
Expected result: <observable behavior>
Actual result: <observed behavior>
Status: PASS | FAIL | NOT RUN | BLOCKED
Artifacts: <logs/screenshot/fixture references if available>
Remaining risk: <explicit uncertainty>
```

---

## 24. Performance and complexity decisions

- Choose algorithms based on measured workload, not aesthetics.
- Write worst-case complexity for operations over large transcript histories.
- Keep parsing and search bounded by size, depth and elapsed time.
- Prefer indexing repeated lookups rather than scanning full archives on every UI update.
- Avoid premature cloning of entire session histories.
- Cache only stable derived values with a correct invalidation key.
- Never cache authorization in a way that survives revocation or resource changes.
- Profile in a representative environment before promising latency budgets.
- Protect the UI thread from expensive disk I/O, language-server calls and subprocess waits.
- Report performance distributions, not only best-case timings.

### 24.1 A safe optimization order

1. Correctness and security.
2. Reproducible baseline measurement.
3. Bounded memory and responsive cancellation.
4. A narrow change to the actual bottleneck.
5. Regression tests and comparison with baseline.

---

## 25. Definition of done by feature class

### 25.1 Domain function

- Explicit input/output/error contract.
- Deterministic behavior where possible.
- No uncontrolled external effect.
- Boundary, malformed and relevant property tests.
- Understandable naming and domain invariants.

### 25.2 MCP tool

- Input/output schema checked with real wire-format tests.
- Per-call authorization at the execution boundary.
- Defined retry and cancellation behavior.
- Safe error codes, no sensitive output leakage.
- Evidence of host-specific compatibility or explicit unverified status.

### 25.3 Handoff workflow

- Source facts classified with provenance.
- User can inspect and correct handoff.
- Provider consumption demonstrated where claimed.
- A useful follow-on action verified.
- Success measured against a manual baseline, not mere tool invocation.

### 25.4 Security-sensitive operation

- Documented actor/project/resource/action scope.
- Expiry/revocation and denial behavior tested.
- Race and resource-resolution concerns reviewed.
- Audit record reflects actual outcome.
- No reliance on prompt text as authorization.

### 25.5 User-interface capability

- The action and its scope are understandable.
- Keyboard and assistive-technology affordances are considered.
- Loading, error, denial and cancellation states are visible.
- Backend remains the authority for permissions.
- Recovery action is supported when failure is common.

---

## 26. Anti-pattern reference: what not to merge

| Anti-pattern | Why harmful | Better approach |
|---|---|---|
| `catch { return [] }` for imported transcript failures | Fabricates normal absence | Typed parse/partial-import error |
| `unwrap()` on untrusted MCP input | Panic becomes remote denial of service | Validate and return error |
| Mutable global `CURRENT_PROJECT` | Wrong-project side effects | Explicit project identity per call |
| Model output labeled `confirmed` | Hides uncertainty | `candidate` with source evidence |
| Direct shell interpolation | Command injection | argv and separate scoped command authorization |
| UI-only permission check | Backend bypass | Execution-time backend policy |
| Worktree claimed as OS sandbox | False security boundary | Accurate scope and verified containment |
| Huge `process_everything` function | Mixed responsibilities | Small parsing / domain / adapter boundary |
| New framework for two use cases | Complexity without value | Explicit functions and types |
| “All tests pass” without run | Misleading evidence | Exact command outcomes |
| Auto-running all MCP tools after connection | Privilege escalation | Limited authenticated grants |
| Returning HTTP 200 for unsupported action | False success semantics | Typed `UNAVAILABLE` |
| Full transcript in telemetry | Privacy and security risk | Metadata and redacted event fields |
| Silent cross-client conversation scraping | Unreliable / unauthorized | Supported API or user-controlled export |
| Long monolithic `AGENTS.md` | Context waste and staleness | Short repository map + this handbook |

---

## 27. Working examples: a full change from requirement to patch

**Scenario:** A developer clicks “Create handoff” while the selected project has uncommitted changes.

**User-visible requirement:** The handoff must disclose the dirty working tree and must not claim that `git HEAD` fully identifies current code state.

**Proposed function boundaries:**

1. `read_project_snapshot(project_id)` — effectful Git/file adapter; includes status and relevant hashes.
2. `validate_snapshot(snapshot)` — pure; flags missing or inconsistent data.
3. `derive_handoff_candidates(events, snapshot)` — pure; uses source evidence.
4. `render_review_model(candidates)` — pure; shows user what will be shared.
5. `confirm_handoff(review_input, snapshot_revision)` — authorized application action; stale revision fails.
6. `persist_handoff(handoff)` — transactional effect with version and audit.
7. `get_approved_handoff(client, handoff_id)` — read scoped by grant.

**Failure cases:**

- No project selected: explain how to select one; do not default silently.
- Git unavailable: provide accurate partial state and an explicit limitation.
- Files change during capture: mark snapshot non-atomic or retry with a bounded limit.
- Approval is denied: preserve draft but share nothing.
- MCP disconnects after fetch: delivery status is not automatically equivalent to continuation.
- Next agent repeats a known failed attempt: record pilot failure rather than falsely counting success.

**Tests:**

- Pure snapshot validation tests with dirty and clean cases.
- Fixture with an actual uncommitted diff and known hash references.
- Stale revision test that denies approval.
- Permission denial test proving no handoff content is exposed.
- Real-client continuation test when integration is available.

**Observability:** record project ID as an opaque identifier, handoff ID, snapshot class, user approval, destination class, duration and result. Do not log code or transcript bodies.

---

## 28. How to update this handbook responsibly

- Add a rule only when it addresses a repeated engineering mistake, a measured failure or a stable architectural decision.
- Include a code example or test when the rule could be misinterpreted.
- If a rule depends on current repository layout, confirm the path in the checked-out source.
- Prefer mechanical enforcement in formatter, compiler, tests and CI over prose-only prohibitions.
- Avoid adding a “MUST” without identifying how a reviewer or automated test can verify it.
- Review guidance after major product or security changes; delete outdated requirements.
- Keep `AGENTS.md` concise; this document owns deeper examples and tradeoffs.

---

## 29. References and provenance

### 29.1 User-provided technical basis

1. Paul Chiusano and Rúnar Bjarnason, *Functional Programming in Scala*, Manning, 2015. Uploaded PDF, 322 pages. Especially printed chapters 1, 3–4, 6–8, 10–13 and 15. Concepts are paraphrased and adapted, not reproduced from code listings.
2. *Lexsus — Full Project Report*, dated 17 October 2026 (user-provided local source-review narrative). It distinguishes the historical source-code inventory from end-to-end verification.
3. *Lexsus 2.0 — Extreme Product Requirements Document*, dated 17 October 2026 (planning artifact). It records proposed product requirements, exclusions and validation gates; proposed behavior is not asserted as shipped behavior.

### 29.2 Public engineering and agent-instruction research

4. AGENTS.md open convention — https://agents.md/ — why root instructions help coding agents.
5. OpenAI, *Harness engineering: leveraging Codex in an agent-first world* (11 February 2026) — https://openai.com/index/harness-engineering/ — maintain a short instruction map and deeper repository knowledge.
6. OpenAI, *Custom Code Review rules for Codex* (20 July 2026) — https://developers.openai.com/blog/custom-code-review-rules-for-codex — actionable repository-specific review rules, not style noise.
7. Anthropic, *How Claude remembers your project* — https://code.claude.com/docs/en/claude-md — working with `CLAUDE.md` and supported `AGENTS.md` instructions.
8. Google Engineering Practices, *Small CLs* — https://google.github.io/eng-practices/review/developer/small-cls.html — small coherent changes and related tests.
9. Google Engineering Practices, *The Standard of Code Review* — https://google.github.io/eng-practices/review/reviewer/standard.html — incremental code health.
10. TypeScript Handbook, *Unions and Intersection Types* — https://www.typescriptlang.org/docs/handbook/unions-and-intersections.html — discriminated union modeling and exhaustiveness.

### 29.3 Scope and credibility statement

This handbook is a **proposed implementation discipline**, not proof that Lexsus currently follows every rule, that every code sample belongs to its repository, or that external provider integration is production-ready. Verify against the actual repository and live provider tests before shipping.
