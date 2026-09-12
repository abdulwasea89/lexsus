# Tool Roadmap — 58 Tools Across Phases 0–10

> The tool surface the web AI sees, phase by phase: what is **built**, what is
> **planned**, and the invariants every new tool must uphold.
>
<<<<<<< HEAD
<<<<<<< HEAD
> Status date: 2026-09-09. **27 of 44 built.**
>
> **Completed:** Phase 0 ✅, Phase 1 ✅, Phase 2 ✅, Phase 3 ✅. **Not started:**
> Phases 5, 7. **Partial:** Phase 4 (0 tools shipped, 6 of 9 have built
> backing) and Phase 6 (session-grants slice landed; persisted
> policy/config/expiry remain).
=======
> Status date: 2026-09-10. **15 of 58 built.**
=======
> Status date: 2026-09-10. **15 of 58 built.** Phase 1's *round trip* was
> reworked on 2026-09-10 (text + `structuredContent`, per-tool `outputSchema`,
> typed error codes, paging by offset) — the tools and their arguments are
> unchanged; what crosses the connector boundary is not. See invariant 9.
>>>>>>> 6cbf3487c9d86e9f38a231a2773b5a6ad6fc339a
>
> **Completed:** Phase 0 ✅, Phase 1 ✅. **Not started:** Phases 2, 3, 5, 7, 8, 9, 10.
> **Partial:** Phase 4 (0 tools shipped, 6 of 10 have built backing) and
> Phase 6 (session-grants slice landed; persisted policy/config/expiry remain).
>>>>>>> 15b86747d260d728cb108d11ca09c9706c5c764b

This is the capability roadmap for the coding-agent bridge. It is deliberately
separate from `full-plan.md` §13, whose "Phase 0"–"Phase 7" describe the
project's *scaffolding* history (Tauri shell, connector, MVP). Here,
"Phase 0"–"Phase 10" refer to tool-surface expansion only.

> **What changed on 2026-09-10.** The Chrome extension and the loopback
> WebSocket transport were removed; the connector is now a native MCP server
> (`mcp.rs`). Phases 0–7 below are unchanged in substance — a tool's API never
> depended on the transport. What changed is where the surface is *registered*
> (see the next section) and, at the end, **Phases 8–10 plus the Claude Code
> parity map**, which close the remaining gaps against a modern agent tool
> surface. Full write-up: [`ongoing/2026-09-10-mcp-native-architecture.md`](../ongoing/2026-09-10-mcp-native-architecture.md).

## Where the tool surface lives

Every tool must be registered in **two places that must never drift**, and its
arguments must survive one parser:

| Place | File | Role |
|---|---|---|
| Rust registry | `src-tauri/src/bridge.rs` — `SPECS` | name, aliases, approval class, timeout, group, auto-insert — **the single source of truth** |
| MCP schema | `src-tauri/src/bridge.rs` — `tool_input_schema()` | the JSON Schema served to the connector's `tools/list` |
| Wire parser | `src-tauri/src/bridge.rs` — `parse_tool_call()` | lenient argument coercion for model output (aliases, number/bool coercion, single-string→array) |

The schema and the parser must agree with each other and with `SPECS` — a tool
that advertises a field the parser drops, or vice versa, is a half-registered
tool that looks fine in `tools/list` and fails on the first call.

**The drift guard is now Rust unit tests**, not a script: `bridge.rs` and
`mcp.rs` carry `surface_partitions_all_spec_tools`,
`every_exposed_tool_has_descriptor_and_schema`,
`write_tools_hidden_until_enabled`, plus the round-trip guards added
2026-09-10 — `every_exposed_tool_declares_output_schema`,
`structured_output_matches_its_declared_schema`,
`error_code_survives_the_mcp_boundary`, and
`no_tool_output_reads_as_call_syntax`. (The old
`scripts/check-spec-sync.mjs` existed solely to keep `SPECS` aligned with
`extension/tool-spec.js`; both the script and the extension are gone, and the
invariant moved to the test that actually enforces it.)

## The read-only gate — a second axis, above the approval classes

Since the connector landed, a tool's visibility has **two** independent
controls, and it is worth keeping them straight:

| Axis | Where it lives | What it does |
|---|---|---|
| **Visibility** | `mcp_allow_write` (`Arc<AtomicBool>`, default **off**) | When off, `tools/list` does not advertise the write/run tools at all. The AI cannot call what it cannot see. Flipped live from the desktop (`mcp_set_allow_write`) or seeded via `LEXSUS_MCP_ALLOW_WRITE=1`. |
| **Approval** | the tool's class in `SPECS` | Once a tool *is* visible, decides whether a given call executes silently, asks, or asks-with-a-card. |

The gate is the coarse control (whole classes of capability off), the class is
the fine control (this particular call, this particular path). A tool that
bypasses the gate — for instance by being reachable through an always-visible
tool like `run_command` — is a bug in the gate, not a feature.

## Approval classes

Every tool carries exactly one, and it decides what the user sees:

| Class | Meaning | Tools today |
|---|---|---|
| `Auto` | executes, result auto-inserted into the chat | `read_file`, `list_directory`, `git_status`, `list_tools`, `describe_tool`, `create_directory`, `read_many_files` |
| `SensitivePathOnly` | asks only when `is_sensitive_path()` fires | `read_file`, `edit_file`, `multi_edit` |
| `Always` | asks every time | `write_file`, `run_command`, `apply_patch`, `copy_file` |
| `Destructive` | asks every time **and the card must show what disappears** | `delete_file`, `move_file` |

The session-grants slice of Phase 6 has landed: a grant ("auto-approve edits
under `src/` for this session") auto-approves matching `Always`/`SensitivePathOnly`
calls, **never** a `Destructive` one and never a sensitive path, and the
GrantsBar's kill switch revokes every grant and pauses the bridge. Grants are
in-memory only — they die with the app. They are also **source-scoped**, so a
grant made for a connector session never covers a desktop call.

## Phases at a glance

| Phase | Theme | Built | Added | Total | Status |
|---|---|---|---|---|---|---|
| — | original MVP tools | 5 | — | 5 | ✅ done |
| 0 | registry + progressive disclosure | 5 | 2 | 7 | ✅ done |
| 1 | files & editing | 8 (+1 early) | 8 | 15 | ✅ done |
<<<<<<< HEAD
| 2 | search | 0 | 2 | 17 | ✅ done |
| 3 | git | 0 | 10 | 27 | ✅ done |
| 4 | project memory | 0 | 9 | 36 | 🔶 partial |
| 5 | background commands | 0 | 3 | 39 | ⏳ not started |
| 6 | approval policy engine | — | 0 | 39 | 🔶 partial |
| 7 | web & long tail | 0 | 5 | 44 | ⏳ not started |

Of the 17 not yet built, **~11 are wiring over code that already exists** —
most of Phase 4 (the SQLite tables and extraction are built) and the thin
file ops.
=======
| 2 | search | 0 | 2 | 17 | ⏳ not started |
| 3 | git | 0 | 10 | 27 | ⏳ not started |
| 4 | project memory | 0 | 10 | 37 | 🔶 partial |
| 5 | background commands | 0 | 3 | 40 | ⏳ not started |
| 6 | approval policy engine | — | 0 | 40 | 🔶 partial |
| 7 | web & long tail | 0 | 5 | 45 | ⏳ not started |
| 8 | **code intelligence (LSP)** | 0 | 4 | 49 | ⏳ not started |
| 9 | **the agent loop** | 0 | 4 | 53 | ⏳ not started |
| 10 | **isolation & delivery** | 0 | 5 | 58 | ⏳ not started |

Of the 43 not yet built, **~18 are wiring over code that already exists** — all
of Phase 3 (git.rs has every function), 6 of 10 in Phase 4 (the SQLite tables and
extraction are built), and the thin file ops.
>>>>>>> 15b86747d260d728cb108d11ca09c9706c5c764b

**Suggested order of attack.** Phase 2 (`grep`/`glob`) and Phase 8 (LSP
diagnostics) are the two highest-value gaps: without search an agent guesses
filenames, and without diagnostics it cannot tell that its own edit broke the
build. Phase 3 is the cheapest (pure wiring). Phase 9 is the phase that most
changes what the product *is* — see its section.

---

## Phase 0 — Registry + Progressive Disclosure ✅ DONE

**Goal:** one source of truth for the tool surface, plus a way for a chat to
discover it without a handoff.

| Tool | Approval | Status |
|---|---|---|
| `list_tools` | Auto | ✅ built |
| `describe_tool` | Auto | ✅ built |

Also landed in this phase:

- Single-source `SPECS` registry, now guarded by Rust unit tests.
- Manifest and prompt output are **inert by construction** — a call must begin
  its line, so an AI echoing the manifest fires nothing.
- Preview/render caps (the composer cap is gone with the extension; the
  **result cap** of 140,000 chars (`RESULT_CHAR_CAP`) now lives in `mcp.rs`).
- Chunked `read_file` (see Phase 1).

**Exit criterion — met:** the 5 original tools round-tripping in a live chat.

---

## Phase 1 — Files & Editing (8 tools) ✅ DONE

**Goal:** precise edits instead of whole-file rewrites. Today the only way the
AI can change code is `write_file` on an entire file, overwriting the chunked-read
protocol — every edit is a full-file round trip.

`rm`/`mv`/`cp`/`mkdir` are already reachable through `run_command`, so the
case for dedicated tools is not capability but **sandboxing** (`run_command`
is not path-checked; `delete_file "../../x"` is rejected by `resolve_path()`),
**structured approval** (a card that shows what disappears, not a shell
string), and **portability** (`rm` vs `del` depends on the detected shell).

| Tool | Approval | Status |
|---|---|---|
| `edit_file` | SensitivePathOnly | ✅ built — str-replace edit, the single highest-value tool in this plan |
| `multi_edit` | SensitivePathOnly | ✅ built — batched `edit_file` ops, applied atomically |
| `apply_patch` | Always | ✅ built — unified diff with context-line drift search; `PatchDoesNotApply` |
| `delete_file` | **Destructive** | ✅ built — card shows the resolved absolute path; refuses directories |
| `move_file` | **Destructive** | ✅ built — **both** paths through `is_sensitive_path()` |
| `copy_file` | Always | ✅ built — same both-paths rule |
| `create_directory` | Auto | ✅ built — `create_dir_all` |
| `read_many_files` | Auto | ✅ built — ≤20 paths, 20KB batch budget, sensitive paths skipped with a notice |

New error codes: `StringNotFound`, `AmbiguousMatch`, `PatchDoesNotApply`,
`BridgePaused`.

**⚠️ Gating — resolved:** the first `Destructive` tools shipped together with
the session-grants slice of Phase 6 (grants scoped by tool class + path
prefix, destructive never grantable, GrantsBar kill switch), so the approval
card does not become reflexive clicking. What remains of Phase 6 is polish:
persisted grant policies and per-tool configuration.

---

## Phase 2 — Search (2 tools) ✅ DONE

**Goal:** the AI can find things instead of guessing filenames.

| Tool | Approval | Notes |
|---|---|---|
| `grep` | Auto | `regex` crate, content search |
| `glob` | Auto | `glob` crate, filename patterns |

**Invariant — non-negotiable:** every result from both tools is filtered
through `is_sensitive_path()`. Without it, search becomes a secret-exfiltration
path that bypasses the `read_file` gate entirely: `grep ".env"` must return
nothing for `.env` itself even though the file matches.

Both walk the project tree with `walkdir`, prune `.git`/vendored/build
directories, skip binary and over-large files, filter every hit (and glob
result) through `is_sensitive_path()`, and cap files/hits/output bytes so an
`Auto` search can't hang or flood the chat.

<<<<<<< HEAD
## Phase 3 — Git (10 tools) — the cheapest phase ✅ DONE
=======
**Parity note (Claude Code's `Grep`/`Glob`):** those tools take a `path` scope,
an optional `glob` filter, and a result-count cap, and return *files-with-matches*
by default rather than every matching line. Copy that shape — the cap matters
more than the search itself, because an uncapped search is a context bomb.

---

## Phase 3 — Git (10 tools) — the cheapest phase ⏳ NOT STARTED
>>>>>>> 15b86747d260d728cb108d11ca09c9706c5c764b

**Goal:** expose the git workflow the app already has to the AI using it.
Almost pure wiring: `git.rs` has every function, and most already have Tauri
commands in `lib.rs` for the UI panel. Only `git_create_branch` and
`git_show`-class tools need new logic.

| Tool | Approval | Backing |
|---|---|---|
<<<<<<< HEAD
| `git_diff` | Auto | `git.rs::diff_workdir` |
| `git_log` | Auto | `git.rs::log` |
| `git_add` | Always | `git.rs::stage`/`stage_all` |
| `git_unstage` | Always | `git.rs::unstage` |
| `git_commit` | Always | `git.rs::commit` — refuses an empty/staged-nothing commit |
| `git_branches` | Auto | `git.rs::branches` |
| `git_create_branch` | Always | `git.rs::create_branch` |
| `git_checkout` | **Destructive** | `git.rs::checkout` — refuses on a dirty tree |
| `git_commit_diff` | Auto | `git.rs::commit_diff` |
| `git_show` | Auto | `git.rs::show` — commit message + diff by oid |
=======
| `git_diff` | Auto | `git.rs::diff_workdir`, `lib.rs` |
| `git_log` | Auto | `git.rs::log`, `lib.rs` |
| `git_add` | Always | `git.rs::stage`/`stage_all` |
| `git_unstage` | Always | `git.rs::unstage` |
| `git_commit` | Always | `git.rs::commit` |
| `git_branches` | Auto | `git.rs::branches`, `lib.rs` |
| `git_create_branch` | Always | ~20 new lines in `git.rs` |
| `git_checkout` | **Destructive** | `git.rs::checkout`, `lib.rs` — **must refuse on a dirty tree** |
| `git_commit_diff` | Auto | `git.rs::commit_diff`, `lib.rs` |
| `git_show` | Auto | new: render a commit (message + diff) by oid |
>>>>>>> 15b86747d260d728cb108d11ca09c9706c5c764b

---

## Phase 4 — Project Memory (10 tools) 🔶 PARTIAL

**Goal:** this tier is the product thesis, not just capability — the AI that
inherits the work also inherits the *why*.

| Tool | Backing |
|---|---|
| `todo_write` / `todo_read` | new `0006_todos` migration |
| `set_objective` | facts tables (F3) already built |
| `remember_decision` | built |
| `remember_constraint` | built |
| `remember_attempt` | built |
| `get_facts` | extraction already built |
| `list_sessions` | built |
| `request_handoff` | `build_handoff_impl()` in `lib.rs` — thin wiring again now that the extension's request path is gone |
| **`get_handoff`** | **new, and the next tool to build** — read-only pull of the current handoff, so a connector-only session gets continuity without a clipboard |

`get_handoff` is the direct replacement for the delivery the extension used to
perform. MCP cannot push into a chat, so instead of *injecting* the handoff,
the connector lets the AI **pull** it: `get_handoff` returns the same payload
`build_handoff_impl()` already produces (objective, decisions, failed attempts,
constraints, changed files, progress). It is `Auto` and read-only.

> Backing note: the bridge executor signature `(tool, source, root)` has no
> `AppState`, and `build_handoff_impl()` needs one. Adding `get_handoff` means
> either widening that seam or resolving the handoff inside `mcp.rs` before it
> reaches the executor. Decide that when you build it — it is the only
> structural work in this phase.

Feeds `build_handoff_impl()`: todos become part of the next handoff prompt.

---

## Phase 5 — Background Commands (3 tools) ⏳ NOT STARTED

**Goal:** long-running commands that don't hold the tool channel open. A
connector call that blocks for 120 s on a build is a call the chat has to
stall on; these tools let the AI start work, do something else, and come back.

| Tool | Notes |
|---|---|
| `run_command_background` | needs `spawn_command_background()` in `pty.rs` |
| `command_output` | reads a bounded ring buffer per command |
| `kill_command` | Always |

The terminal's streaming path was kept rAF-coalesced precisely so these can
stream into it.

**Parity note (Claude Code's `Bash` with `run_in_background`, `TaskOutput`):**
the pattern that makes this work in practice is an **id + a bounded tail** —
`run_command_background` returns an id immediately, `command_output` takes that
id and returns only the new output since the last poll. A version that returns
the whole buffer re-sends everything on every poll and burns the context window.

---

## Phase 6 — Approval Policy Engine (0 tools) 🔶 PARTIAL

No new tools — this is what contains the ~15 gated ones from Phases 1, 3 and 5:

- **Session grants scoped by tool class and path prefix — landed with Phase 1**
  ("auto-approve writes under `src/` for this session"). In-memory only;
  source-scoped (a connector-created grant never covers desktop calls).
- **Destructive never auto-approves.** Not overridable by a grant. Landed.
- **A visible kill switch**: the GrantsBar's "Revoke all & pause" button
  revokes every grant and pauses the bridge. Landed.
- **The read-only gate** (`mcp_allow_write`) landed with the connector — the
  coarsest policy control in the system, and the one that makes "connect first,
  trust later" a safe default.

Remaining for a later pass: persisted grant policies, per-tool configuration,
and grant expiry.

---

## Phase 7 — Web & Long Tail (5 tools) ⏳ NOT STARTED

| Tool | Approval | Notes |
|---|---|---|
| `web_fetch` | Auto | **must block `169.254.169.254` and non-loopback private ranges** — SSRF against cloud metadata and the LAN |
| `web_search` | Auto | needs an external API; the only tool whose result isn't local-first |
| `notebook_read` | Auto | Jupyter `.ipynb` as structured JSON |
| `notebook_edit` | Always | cell-level edits; Claude Code's `NotebookEdit` takes a `cell_id`, not a line number — copy that |
| `delegate_task` | Always | spawn a sub-task and collect its result |

---

## Phase 8 — Code Intelligence (4 tools) ⏳ NOT STARTED — **highest-value gap**

**Goal:** the AI can ask the code what it means, and — more importantly — find
out that it just broke something.

Right now an edit is a blind write: the AI changes a function signature, and
the only way it learns that three call sites no longer compile is if *you* run
a build and tell it. Every real agent loop closes that gap with a language
server, and it is the single biggest capability difference between Lexsus's
current surface and a production coding agent. (`run_command` + a test suite
covers this partially, and badly: it is slow, it is coarse, and it requires the
project to have a working build command.)

Backing: an LSP client in the Rust core — one spawned language server per
project, lazily started on first use, torn down with the workspace. Scope tools
to the bound workspace and filter every result through `is_sensitive_path()`
(symbols in a sensitive file are that file's contents).

| Tool | Approval | What it answers |
|---|---|---|
| `lsp_diagnostics` | Auto | "what's broken in this file right now?" — errors/warnings after every edit. The one to build first |
| `lsp_definition` | Auto | "where is this defined?" |
| `lsp_references` | Auto | "what else calls this?" — the rename-safety question |
| `lsp_symbols` | Auto | "what's in this file / this workspace?" — cheaper and far more precise than `grep` for structure |

**Design constraints:**
- **Best-effort, never blocking.** A project with no language server must
  degrade to "no diagnostics available", not to a failed tool call. Diagnostics
  are a bonus signal, not a dependency.
- **Cap and dedupe.** A cold build produces hundreds of diagnostics; return a
  bounded, severity-sorted, deduped list with a count of what was withheld —
  the same truncation-marker discipline as `read_file`.
- **`lsp_diagnostics` is the enforcement point for "did the AI break it?"**
  The natural next step after this phase is an *automatic* post-edit
  diagnostics attachment on `edit_file`/`apply_patch` results, which is what
  Claude Code does — the AI sees the consequences of its edit without asking.

---

## Phase 9 — The Agent Loop (4 tools) ⏳ NOT STARTED — **changes what the product is**

**Goal:** give the AI the three primitives that make a long autonomous run
*supervised* rather than merely permitted.

Today Lexsus's only channel from the AI to the human is a yes/no approval on a
single call. That is enough to stop a bad edit; it is not enough to run a
twenty-step task. The AI cannot ask "which of these two approaches do you
want?", cannot say "here is my plan, approve it before I touch anything", and
cannot say "the build finished while you were away". Every serious agent
harness has these, and Lexsus's banner infrastructure makes them nearly free —
they are the same event-plus-blocking-wait shape approval already uses.

| Tool | Approval | What it does |
|---|---|---|
| `ask_user` | Auto (it *is* a question) | Structured question to the desktop — a prompt with 2–4 labelled options plus free text. Blocks like an approval, returns the chosen answer. This is how the AI resolves ambiguity instead of guessing |
| `propose_plan` | Always | The AI submits a plan and **waits for approval before acting**. On Allow, the plan is pinned into the trace and the AI proceeds; on Deny, it gets the refusal and your comment. This is the natural companion to the read-only gate: read freely, plan, then be trusted to write |
| `monitor` | Auto | Watch a path or a running background command and return when it changes or matches a pattern — the push-side companion to Phase 5's pull-side `command_output`. This is what lets an AI wait for a build instead of polling it |
| `notify` | Auto | Raise a desktop notification when a long task finishes or needs attention. The AI is not always being watched, and today a finished task is silent |

**Why this phase matters more than it looks.** Phases 2, 3 and 8 make the AI
*capable*. Phase 9 is what makes it *trustworthy enough to leave alone*, which
is the actual product promise — you go do something else and the work continues
correctly. `propose_plan` in particular is the feature that lets a cautious
user hand over more autonomy than they otherwise would, because the first
interaction is a plan review rather than a write approval.

---

## Phase 10 — Isolation & Delivery (5 tools) ⏳ NOT STARTED

**Goal:** let the AI work without risking the user's working tree, and hand
back results as artifacts rather than walls of text.

| Tool | Approval | What it does |
|---|---|---|
| `enter_worktree` | Always | Create a throwaway git worktree and point subsequent tools at it — the AI experiments on a branch copy, your working tree is untouched. Cheap: `git.rs`/`git2` already has everything, and Claude Code's `EnterWorktree` is the same idea |
| `exit_worktree` | Always | Leave and clean up (keep or discard), refusing if there are uncommitted changes unless explicitly told to discard |
| `read_media` | Auto | Return an image (and a PDF's pages) as an MCP **image content block** rather than pretending it is text — a screenshot or a design mock is data the AI can genuinely consume, and today `read_file` would corrupt it |
| `publish_artifact` | Always | Hand a file to the user as a first-class artifact (a report, a screenshot, a built diff) instead of pasting its contents into the chat and spending the context window on it |
| `report_findings` | Auto | A structured review result — file, line, severity, one-line claim, evidence — rendered as a findings list in the desktop. Constrains a code review to something you can act on, instead of a prose essay |

**On `delegate_task` (Phase 7) and multi-agent work.** Claude Code's surface
goes further than a single sub-task — named agents, `ListAgents`, `SendMessage`
between them, and workflow orchestration over many. That is genuinely out of
scope here: it multiplies the approval surface, and Lexsus's value is that
*you* see and approve what runs. `delegate_task` stays the single, bounded
delegation primitive; revisit fan-out only after the 5–10-developer validation.

---

## The MCP surface beyond tools

A tool surface is only half of what an MCP server can expose, and Lexsus is
currently using just that half. Two additions are worth planning here rather
than in a phase, because they are not tools:

**Resources** (`resources/list`, `resources/read`). Read-only, addressable
content the AI can fetch without a tool call and that the host can present in
its own UI. The obvious map for Lexsus: the bound workspace's files, a live
`git status`, and the last sessions. The payoff is on the *host* side —
Claude.ai and Claude Desktop render resources natively, and a resource can be
attached to a conversation by the user, which no tool can achieve. It also
gives a cheap read path that does not consume the approval machinery.

**Prompts** (`prompts/list`, `prompts/get`). Parameterised prompt templates
the user invokes from the host's UI. The handoff is precisely this shape: a
`continue_work` prompt that takes an optional focus and returns the same text
the Handoff view builds. That turns "copy the handoff to your clipboard and
paste it" into "pick *Continue work in Lexsus* from the connector's menu" —
the natural fixing of the one rough edge the extension's removal left behind.

Both flow through the same policy engine as any tool call; resources in
particular must respect `is_sensitive_path()` and the read-only gate, since a
resource listing is a file listing by another name.

---

## Claude Code parity map

How Lexsus's surface compares to the tool set a modern coding agent ships with.
The right-hand column is the point of this table: for each Claude Code tool,
whether Lexsus has an equivalent, and if not, **why**.

| Claude Code tool | Lexsus | Where |
|---|---|---|
| `Read` (chunked text) | ✅ `read_file` | built |
| `Read` (images, PDFs) | ⏳ `read_media` | Phase 10 |
| `Write` | ✅ `write_file` | built |
| `Edit` / `MultiEdit` | ✅ `edit_file` / `multi_edit` | built |
| `Glob` / `Grep` | ⏳ `glob` / `grep` | Phase 2 |
| `NotebookEdit` | ⏳ `notebook_read` / `notebook_edit` | Phase 7 |
| `Bash` | ✅ `run_command` | built |
| `Bash` (background) / `TaskOutput` | ⏳ background trio | Phase 5 |
| `PowerShell` | ✅ covered | `shell.rs` already abstracts Sh/Bash/Zsh/Cmd/PowerShell — no separate tool needed |
| `LSP` | ⏳ 4 tools | **Phase 8** |
| `TodoWrite` / `Task*` | ⏳ `todo_write` / `todo_read` | Phase 4 |
| `EnterPlanMode` / `ExitPlanMode` | ⏳ `propose_plan` | **Phase 9** |
| `AskUserQuestion` | ⏳ `ask_user` | **Phase 9** |
| `EnterWorktree` / `ExitWorktree` | ⏳ both | Phase 10 |
| `WebFetch` / `WebSearch` | ⏳ both | Phase 7 |
| `Agent` (subagents) | ⏳ `delegate_task` | Phase 7 (fan-out out of scope) |
| `ListAgents` / `SendMessage` | ⬜ deliberate non-goal | multiplies the approval surface; see Phase 10 |
| `Workflow` | ⬜ deliberate non-goal | same |
| `Skill` | ⬜ deferred | revisit after validation — the handoff-as-MCP-**prompt** is the same idea, smaller |
| `Artifact` / `SendUserFile` | ⏳ `publish_artifact` | Phase 10 |
| `ReportFindings` | ⏳ `report_findings` | Phase 10 |
| `Monitor` | ⏳ `monitor` | Phase 9 |
| `PushNotification` | ⏳ `notify` | Phase 9 |
| `ListMcpResourcesTool` / `ReadMcpResourceTool` | ⏳ resources surface | "beyond tools", above |
| `ToolSearch` / `WaitForMcpServers` | ✅ not needed | Lexsus exposes 15 tools today; `list_tools`/`describe_tool` already give progressive disclosure, and deferred-tool loading only pays off at a much larger surface |
| `Cron*` / `ScheduleWakeup` / `RemoteTrigger` / `EndConversation` / `ShareOnboardingGuide` / `SendFeedback` | ⬜ out of scope | harness/product mechanics, not coding capability |

---

## Invariants every new tool must uphold

These are the standing rules the built tools established. A PR that breaks
any of them is wrong regardless of what it adds:

1. **Register in one registry plus one schema.** Add the row to `SPECS`
   (`bridge.rs`), add its JSON Schema to `tool_input_schema()`, and teach
   `parse_tool_call()` any non-trivial coercion. The `bridge.rs`/`mcp.rs` unit
   tests must stay green — they are the drift guard.
2. **Respect the read-only gate.** Anything that writes, deletes, moves, or
   runs a command must be hidden from `tools/list` while `mcp_allow_write` is
   off. `write_tools_hidden_until_enabled` is the test that enforces this; if a
   new tool trips it, the test is right and the tool is wrong.
3. **Tool output must not parse as tool calls.** If the AI echoes a result,
   nothing fires. The anchored parser and the manifest's no-call-syntax rule
   exist because this class of bug froze the host page. This applies to
   **results**, not just the manifest — `read_file`'s chunking footer used to
   spell out `read_file("big.txt", 401)`, which the manifest-only test could
   not see. `no_tool_output_reads_as_call_syntax` drives every tool and checks
   every result; page with data (an offset) rather than with prose that
   imitates a call.
4. **Cap what you return.** `RESULT_CHAR_CAP` (140,000 chars) is applied by the
   connector; page like `read_file` does if a single result can be large, and
   include the truncation marker so the model knows it was cut. The marker is
   deliberately tool-neutral — it cannot know what it is truncating, so it
   must not advise calling a specific tool.
5. **Sensitive-path filtering applies to lists, not just reads.** `grep`,
   `glob`, `list_directory`, LSP symbols, and MCP resource listings included.
6. **Destructive tools show what disappears** and refuse unsafe states
   (`git_checkout` + dirty tree, `exit_worktree` + uncommitted changes).
7. **Both paths checked on path-pair operations** (`move_file`, `copy_file`,
   `enter_worktree`).
8. **Never block longer than the connector timeout.** Gated calls block on the
   desktop for at most `APPROVAL_WAIT_SECS` (120 s). A tool that needs longer
   belongs in Phase 5's background trio, not in a blocking call.
9. **Structured output is a promise, not a bonus.** If a tool returns
   `structuredContent`, add its row to `output_schema()` (`bridge.rs`) in the
   same commit — MCP says the payload must conform to the advertised
   `outputSchema`, and `structured_output_matches_its_declared_schema`
   enforces it both ways (no undeclared key, no missing required key). Report
   a **typed** `ErrorCode` from the core rather than a bare string, so a
   failure reaches the model as something it can branch on instead of prose.
   Put the facts a caller would otherwise have to scrape — a count, a line, a
   next offset — in the structured half, and keep the text readable for the
   human reading the trace.

## Verification per phase

Offline (run all of these before handing anything over):

```bash
cd src-tauri && cargo fmt --all --check \
  && cargo clippy --lib --all-targets -- -D warnings \
  && cargo test --lib
cd .. && pnpm typecheck && pnpm lint && pnpm build
```

Live (the real gate — offline checks cannot prove the tool doesn't misfire or
that the model can actually use it): connect an MCP host to
`http://127.0.0.1:45147/mcp` and confirm the tool appears in `tools/list` with
the right schema; call it and confirm the result; if it is gated, confirm it is
**absent** from `tools/list` while `mcp_allow_write` is off, then flip the
switch and confirm it appears and asks for approval; call it on a sensitive path
and confirm the refusal.
