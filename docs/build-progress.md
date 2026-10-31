# Lexsus — Built and Remaining Work

**Updated:** 17 October 2026. **Scope:** the full [Lexsus 2.0 PRD](../lexsus.md), including later phases.

This inventory is based on the source review in this session and the [full report](lexsus-full-report-2026-10-17.md). It describes the local working tree, including uncommitted changes. It is an area-level checklist, not a requirement-by-requirement acceptance audit. No builds, tests, installations, security audit, or real-provider demonstrations were run for this inventory.

**Status meanings:**

- **Implemented:** code or configuration exists for the named component; release acceptance is not implied.
- **Partial:** useful implementation exists but the full requirement is unfinished.
- **Remaining:** no working implementation was established in the review.
- **Unverified:** sufficient current evidence is missing; this does not automatically mean absent.

Checked items below mean implementation exists. They do not mean the entire PRD requirement passed its acceptance criteria.

## Overall position

**Desktop foundation update (17 October 2026):** implementation hardening is in place: Lexsus packaging, pnpm hooks, minimum window dimensions, WebView CSP, app-wide render recovery, startup retry, accessible OAuth controls, bridge compile repairs, and a three-platform unsigned package workflow. See [desktop foundation](desktop-foundation.md) for build evidence and the installation/runtime acceptance checklist. Cross-platform release readiness is not yet established.

Lexsus has substantial desktop, storage, local execution, and connector foundations. The main remaining product work is safe capture, evidence-linked memory, a reviewable handoff, and demonstrated continuation in a supported agent.

The earlier conversational estimate of **35–45% implemented** was a rough planning judgment. It was not calculated from audited requirements and should not be used as a release metric. There is no defensible measured completion percentage yet.

## 1. Components implemented in the current foundation

### Desktop and project controls

- [x] Tauri 2 desktop application with Rust backend.
- [x] React/TypeScript/Vite frontend.
- [x] Single active dashboard with connector, tunnel, and activity sections.
- [x] Project-folder selection and recent-project list.
- [x] Filesystem watcher integration.
- [x] Onboarding and status bar.
- [x] Approval, question, session-grant, and failover banners.
- [x] Tool usage, file activity, command records, audit, and recent-activity display.

Evidence: [app shell](../src/App.tsx), [dashboard](../src/views/DashboardView.tsx), [frontend guide](../src/README.md), [watcher](../src-tauri/src/watcher.rs).

The old dedicated Git, memory, handoff, trace, and terminal views have been removed from the current frontend. Their former desktop workflows are not counted as completed current UI.

### Capture, storage, memory, and handoff

- [x] Claude Code JSONL transcript reader.
- [x] SQLite session archive and memory storage.
- [x] Heuristic extraction of objectives, decisions, constraints, failed attempts, and changed files.
- [x] Explicit tools for recording objectives, decisions, constraints, attempts, and task lists.
- [x] Session and fact retrieval.
- [x] Handoff construction and `get_handoff` retrieval.
- [x] `continue_work` MCP prompt implementation.
- [x] Interruption/idle detection and in-app continuation offers.

Evidence: [transcripts](../src-tauri/src/transcript.rs), [archive](../src-tauri/src/archive.rs), [facts](../src-tauri/src/facts.rs), [database](../src-tauri/src/db.rs), [integration](../src-tauri/src/lib.rs), [failover](../src-tauri/src/failover.rs).

This is a basic continuity implementation. It does not establish reviewed, versioned, provenance-complete handoffs or successful automatic switching.

### Connector and security mechanisms

- [x] Native MCP server using the local Rust execution layer.
- [x] Loopback binding by default.
- [x] Connector start, stop, restart, port, and allowed-host controls.
- [x] Read-only versus write-enabled tool exposure.
- [x] Bearer authentication and response-signing implementation.
- [x] OAuth routes, token handling, and client management.
- [x] Tunnel detection and lifecycle controls.
- [x] Approval classes and sensitive-path policy handling.
- [x] Scoped session-grant mechanism, kill switch, and audit records.

Evidence: [MCP](../src-tauri/src/mcp.rs), [authentication](../src-tauri/src/auth.rs), [OAuth](../src-tauri/src/oauth.rs), [tunnels](../src-tauri/src/tunnel.rs), [tool policy](../src-tauri/src/bridge/mod.rs).

These mechanisms still require current verification and hardening. Ordinary edits do not necessarily prompt under the current policy; worktrees do not sandbox arbitrary shell commands.

### Local tool inventory

The registry contains **58 tool names**. The groups below list that surface. Delegation is explicitly unavailable and is not counted as working functionality.

| Group | Registered tools | Implementation qualification |
|---|---|---|
| Reading | `read_file`, `read_many_files`, `list_directory`, `notebook_read`, `read_media` | Implemented; limits and permissions need acceptance evidence |
| Editing | `write_file`, `edit_file`, `multi_edit`, `apply_patch`, `delete_file`, `move_file`, `copy_file`, `create_directory`, `notebook_edit` | Implemented; policy hardening remains |
| Commands | `run_command`, `run_command_background`, `command_output`, `kill_command` | Implemented; process and permission guarantees need verification |
| Search | `grep`, `glob` | Implemented |
| Git | `git_status`, `git_diff`, `git_log`, `git_add`, `git_unstage`, `git_commit`, `git_branches`, `git_checkout`, `git_create_branch`, `git_show`, `git_commit_diff` | Implemented in backend; no current dedicated Git screen |
| Memory | `todo_write`, `todo_read`, `set_objective`, `remember_decision`, `remember_constraint`, `remember_attempt`, `get_facts`, `list_sessions`, `request_handoff`, `get_handoff` | Basic implementation; richer PRD contracts remain |
| Web | `web_fetch`, `web_search` | Implemented; fetch has an SSRF guard; external reliability unverified |
| Code intelligence | `lsp_diagnostics`, `lsp_definition`, `lsp_references`, `lsp_symbols` | Implementation depends on available/configured language servers |
| Agent interaction | `ask_user`, `propose_plan`, `monitor`, `notify`, `report_findings` | Implemented; `propose_plan` policy inconsistency remains recorded |
| Delegation | `delegate_task` | Returns `AGENT_NOT_AVAILABLE`; runtime remaining |
| Isolation and delivery | `enter_worktree`, `exit_worktree`, `publish_artifact` | Implemented; worktrees are change isolation only |
| Discovery | `list_tools`, `describe_tool` | Implemented |

Evidence: [canonical registry](../src-tauri/src/bridge/specs.rs), [schemas](../src-tauri/src/bridge/schema.rs), [executor](../src-tauri/src/bridge/mod.rs). The proposed `lexsus_*` Continuity Profile in the PRD is separate from these existing tools.

### Engineering and persistent project knowledge

- [x] Frontend lint, typecheck, and build scripts.
- [x] CI configuration for frontend checks, Rust fmt/Clippy/tests, and compression-service health.
- [x] Existing Rust test suites.
- [x] Product PRD and engineering handbook.
- [x] Root `AGENTS.md` with required startup reading order.
- [x] `CLAUDE.md` and `GEMINI.md` pointers to shared instructions.
- [x] `SESSION_CONTEXT.md` for persistent progress and handoffs.
- [x] Source-based full project report.

Evidence: [package scripts](../package.json), [CI](../.github/workflows/ci.yml), [agent rules](../AGENTS.md), [engineering handbook](../ENGINEERING_STANDARDS.md).

Current passing results are **unverified**. Historical status records contain conflicting counts.

## 2. Full-plan coverage and remaining work

| Area / PRD reference | Status | Work still required to satisfy the intended outcome |
|---|---|---|
| Product strategy and scope, §§1–6 | Written; hypotheses unvalidated | Establish demand and show improvement over manual Markdown handoff |
| Supported hosts and platforms, §7 | Partial / unverified | Pin exact host versions; validate Linux first and macOS/Windows separately |
| User journeys and desktop UX, §§8–9 | Partial | Visible primary continuation action; capture, review, export, connection recovery, and revoke journeys |
| Project scope, FR-001–007 | Partial | Stable IDs, final-resource checks, project-switch invalidation, stale/missing-root behavior and tests |
| Capture, FR-008–017 | Partial | Explicit source selection, parser bounds, source offsets, secret filtering, truthful partial imports, manual input |
| Memory, FR-018–027 | Partial | Provenance, confidence categories, corrections, supersession, contradictions and stale-fact handling |
| Handoffs, FR-028–041 | Partial | Versioned reviewed snapshots, JSON/Markdown parity, redaction, stable evidence references, size budgets, freshness checks |
| MCP/adapters, FR-042–060 | Partial | Minimal Continuity Profile, scoped schemas, host capability records, actual transport and client acceptance evidence |
| Remote strategy, INT-07–18 | Foundation only | Opt-in reviewed snapshot sharing, hosted gateway, ownership controls, expiry/revoke, tenant isolation and real-host tests |
| Security, SEC-001–026 | Partial | Consistent policy matrix, execution-time authorization coverage, grant lifecycle, OAuth review, path/process adversarial tests |
| Privacy lifecycle, §17 | Partial | Clear retention settings, deletion/export, credential handling verification and local encryption plan |
| Architecture, §18 | Existing core; proposed changes remain | Cohesive domain boundaries, separate local/remote profiles and offline guarantees; avoid blanket rewrites |
| Storage contracts, §19 | Basic persistence | Versioned domain contracts, concurrency control, project isolation, migrations and recovery evidence |
| Error semantics, §20 | Partial | Consistent typed errors, state-change disclosure, cancellation, safe retry and partial-state recovery |
| Nonfunctional qualities, §21 | Unverified against targets | Measure resource bounds, responsiveness, recovery, accessibility and supported-platform behavior |
| Quality assurance, §22 | Tests/configuration exist | Fresh baseline, relevant negative tests, transport tests and pinned-host end-to-end demonstrations |
| Pilot evaluation, §23 | Plan written | Recruit 5–10 developers; run comparable tasks against their normal handoff baseline |
| Analytics, §24 | Activity statistics only | Consent-aware content-free outcome events, defined denominators and verified continuation measurement |
| Commercial model, §25 | Hypotheses written | Demand interviews, actual payments, repeat use, costs and supported operational commitments |
| Delivery gates, §26 | Incomplete | Produce evidence for G0–G6 in sequence |
| Risk and launch, §§27–28 | Written plans | Resolve blocking findings; collect release, security, support and rollback evidence |
| Open decisions, §29 | Open | Choose initial second host, snapshot storage, essential evidence, approval defaults and justified remote scope |
| Compression | Stub only | Optional bounded implementation with deterministic fallback after core handoff validation |
| Delegation, orchestration and team expansion | Remaining / later scope | Separate justification and design; not dependencies for the first continuity release |

## 3. Delivery-gate checklist

### G0 — Current engineering baseline

- [ ] Record checkout revision, dirty state, toolchains, and tested host versions.
- [ ] Run and record frontend and Rust checks; resolve or triage actual failures.
- [ ] Reconcile conflicting status documents and obsolete setup instructions.

### G1 — Safe capture

- [ ] Select source session explicitly and associate it with one project.
- [ ] Enforce import budgets, source provenance, and secret exclusions.
- [ ] Demonstrate correct partial-import and permission-denial behavior.
- [ ] Verify scope enforcement and project-switch behavior.

### G2 — Reviewable handoff

- [ ] Produce evidence-linked facts with visible uncertainty and corrections.
- [ ] Provide a handoff review/redaction/confirmation UI.
- [ ] Export versioned JSON and Markdown with matching semantics.
- [ ] Detect stale repository state and show missing evidence accurately.

### G3 — Proven local continuation

- [ ] Expose and test the minimal continuity tool profile.
- [ ] Record a real interrupted-task continuation in the first supported host.
- [ ] Repeat with a named second host and version.
- [ ] Demonstrate denial, cancellation, and recovery; verify a useful next action.

### G4 — Developer pilot

- [ ] Recruit 5–10 target developers.
- [ ] Compare Lexsus with their usual manual handoff process.
- [ ] Record continuation outcomes, clarification effort, time, errors, and trust.
- [ ] Assess repeat use and decide whether the observed benefit justifies expansion.

### G5 — Secure remote beta

- [ ] Share only reviewed, redacted snapshots through explicit opt-in.
- [ ] Implement ownership, scoped access, expiry, revocation, and tenant isolation.
- [ ] Validate actual remote hosts and authentication flows.
- [ ] Complete privacy/security review and operational recovery procedures.

### G6 — Paid beta

- [ ] Offer transparent limits and tested onboarding.
- [ ] Obtain actual paid-use evidence and measure delivery/support costs.
- [ ] Establish repeat usage and operationally credible support commitments.

No gate is marked complete solely because some supporting components exist.

## 4. Maintenance rules

Update this inventory when an implementation milestone or acceptance result changes. For each newly completed requirement, record its PRD ID, source location, exact validation method, checkout/date, and any remaining limitations. Keep product requirements in `lexsus.md`, durable engineering rules in `ENGINEERING_STANDARDS.md`, and temporary progress in `SESSION_CONTEXT.md`.

**Next product milestone:** establish G0, then demonstrate one complete capture → reviewed handoff → real local continuation journey. This checklist records the plan; it does not authorize starting unrelated implementation work.
