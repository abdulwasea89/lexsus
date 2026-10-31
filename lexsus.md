# LEXSUS 2.0 — EXTREME PRODUCT REQUIREMENTS DOCUMENT

> **Repository reading contract:** Coding agents read this PRD with [`AGENTS.md`](AGENTS.md), [`ENGINEERING_STANDARDS.md`](ENGINEERING_STANDARDS.md), and [`SESSION_CONTEXT.md`](SESSION_CONTEXT.md) at session startup. This file owns intended product requirements; current implementation must be checked in source. Record progress in session context rather than rewriting requirements as if they have shipped.

> **Product promise:** Your AI can change. Your work doesn't.
>
> **Product category:** Developer infrastructure / cross-agent work continuity.
>
> **Status:** Proposed requirements, **not** a claim that the described release exists.
>
> **Date:** 17 October 2026 · **Version:** PRD 1.0 · **Owner:** Zhilo Labs / Lexsus product team.
>
> **Evidence baseline:** User-provided *Lexsus — Full Project Report* (17 October 2026), which reviews a local source checkout but did not execute the application or validate provider compatibility.
## 0. How this PRD is written and how engineers should use it

- **Principle 0.1 — Write the problem before the solution:** A PRD must explain who suffers, in what workflow, why an existing workaround fails, and what measured result would justify building software [R1][R2].
- **Principle 0.2 — Separate facts from hypotheses:** Mark observations from the uploaded report as `REPORTED`, independent platform documentation as `DOCUMENTED`, planned behavior as `REQUIRED`, and untested demand as `HYPOTHESIS`.
- **Principle 0.3 — Make every requirement testable:** Each numbered `FR`, `SEC`, `NFR`, and `INT` entry specifies observable behavior, constraints, or a verifiable acceptance condition; engineering may split entries into tickets.
- **Principle 0.4 — Specify boundaries explicitly:** The PRD defines exclusions, permissions, failure modes, supported environments, dependency risks, and stopping rules rather than promising arbitrary agent autonomy.
- **Principle 0.5 — Track ownership and decisions:** Product owns outcomes and scope; engineering owns implementation design; security owns threat-model approval; design owns journey clarity; QA owns independent verification.
- **Principle 0.6 — Avoid false precision:** Targets below are **proposed release gates or experiments**, not measured customer outcomes; revise them after baseline measurement.
- **Principle 0.7 — Define priorities:** `P0` blocks a usable first release; `P1` supports a beta but can follow a validated core; `P2` is conditional expansion, not a launch dependency.
- **Principle 0.8 — Change control:** Significant scope changes need a dated decision, reason, affected user journey, security effect, owners, revised acceptance test, and explicit P0/P1/P2 trade-off.
- **Principle 0.9 — Do not conflate a PRD with a design spec:** Data shapes and protocols are illustrative product contracts; detailed schemas, key management, and threat mitigations require engineering design reviews.
- **Principle 0.10 — Definition of done:** A feature is done only after code, automated coverage, end-to-end demonstration, documentation, error handling, telemetry, accessibility checks, and its acceptance criteria pass.
## 1. Executive decision and product strategy

- **Decision D-01:** Keep the existing desktop Lexsus as the trusted local authority for code, files, Git, permissions, and audit events; do **not** replace it with a thin cloud plugin.
- **Decision D-02:** Recenter the experience on a narrow outcome: **capture work state → review evidence → generate handoff → continue in a supported agent → verify useful progress**.
- **Decision D-03:** Use MCP as a reusable integration layer, while acknowledging that clients differ in tool invocation, UI, authentication, and available conversation context [R3][R4][R5].
- **Decision D-04:** Release **local continuation** before **web-hosted connector capabilities**; implement cloud remote access only when security and provider-specific end-to-end tests pass.
- **Decision D-05:** The first commercial claim is **less re-explanation during real interrupted coding tasks**, not automatic model switching, token-limit circumvention, or universal transcript retrieval.
- **Decision D-06:** Treat plugin packaging as **distribution**, not the core moat; the durable product is provenance-rich task state and dependable execution handoff.
- **Decision D-07:** Preserve the source code's substantial capabilities where verified, but hide or disable unfinished tools and prioritize an intuitive handoff workflow over showing 58 tools.
- **Executive hypothesis:** Developers who already use Claude Code and at least one other agent will adopt Lexsus if it reconstructs next-action context more reliably than an ordinary manually written handoff file.
- **Release unit:** One project, one source session, one destination client, one reviewed handoff, one measurable next action; broader orchestration is outside first-release scope.
## 2. Source-grounded current product versus proposed release

| Dimension | Reported current Lexsus (`REPORTED`) | Lexsus 2.0 requirement (`REQUIRED`) | Evidence gap |
|---|---|---|---|
| Core shell | Tauri 2 + React/TypeScript/Vite + Rust | Retain desktop app and Rust local authority | Fresh build and installation not run |
| Transcript capture | Claude Code JSONL reader | Verified explicit project/source selection and bounded extraction | Other agents' private transcripts unproven |
| Session store | SQLite archive | Schema versioning, retention controls, encryption plan | Existing archive migration not tested |
| Project memory | Explicit tools + heuristic extraction | Provenance, confidence classes, contradiction handling | Recall accuracy unmeasured |
| Compression | Python service skeleton; `/compress` returns HTTP 501 | Deterministic bounded handoff first; compression optional later | No working compression demonstrated |
| MCP surface | 58 registered names; some incomplete | Small stable continuation-focused tool profile | Tool inventory is not acceptance evidence |
| Delegation | `delegate_task` returns `AGENT_NOT_AVAILABLE` | Do not advertise or expose as working | No agent runtime exists |
| Local execution | Files, Git, commands, LSP, notebooks reported | Narrow default permissions, approvals and tests | No full OS sandbox |
| Remote access | Loopback default; tunnels and OAuth code reported | Authenticated, tightly scoped, opt-in remote handoff | External-host compatibility not yet tested |
| Desktop UI | Single dashboard, banners, onboarding, controls | Visible primary handoff CTA and review screen | First-run usability unmeasured |
| Reliability | CI configured; historical test counts conflict | Reproducible green baseline and recorded E2E evidence | Current checks not run |
| Licensing | Repository reported under MIT | Maintain clear licensing/third-party notices | Commercial differentiation unresolved |

- **Baseline caveat B-01:** This PRD relies on the uploaded 159-line report, not direct access to all repository files; any implementation claim remains provisional until inspection of the specific checkout.
- **Baseline caveat B-02:** The report describes both staged and unstaged work; a release branch may differ materially, so create a release-tagged inventory before implementation.
- **Baseline caveat B-03:** The old multi-screen workbench is documented as deleted; do not design first-run flows as if those views are still available.
- **Baseline caveat B-04:** The report identifies a `propose_plan` read-only/policy mismatch; it is a blocking policy-design issue rather than a documentation typo.
## 3. The problem, its economic consequences, and validation uncertainty

- **Problem P-01:** A developer's objective, rejected approaches, constraints, failed tests, and next steps often live in an AI session rather than in version-controlled project artifacts.
- **Problem P-02:** When the session ends, the next agent can inspect files but may not understand the reasoning behind the state, leading to redundant exploration and unsafe repetitions.
- **Problem P-03:** A developer can manually write `HANDOFF.md`, but consistency, provenance, freshness, and the amount of effort vary significantly by person and task.
- **Problem P-04:** A web AI tool's conversation is not the same security boundary as a local repository; an integration must not assume it can read files or issue commands without an explicit grant.
- **Problem P-05:** Tool and connector setup can be harder than the interruption itself; poor onboarding can erase all productivity benefits.
- **Evidence E-01 (`REPORTED`):** The source report documents the architectural intention and implemented modules, but **does not** demonstrate successful user continuation or willingness to pay.
- **Evidence E-02 (`DOCUMENTED`):** OpenAI plugins can package skills with MCP tools; public web workflows use remote MCP endpoints and must satisfy authentication and plugin rules [R3][R4][R6].
- **Evidence E-03 (`DOCUMENTED`):** Claude Code supports local stdio and remote HTTP MCP; Claude custom remote connectors connect from Anthropic infrastructure, not directly to local loopback [R5][R7].
- **Hypothesis H-01:** A correct, reviewable handoff reduces the elapsed time from switching agents to the first useful code change compared with a developer's normal method.
- **Hypothesis H-02:** Users prefer persistent project state that cites specific files/commands over opaque auto-generated prose that cannot be verified.
- **Hypothesis H-03:** Professional developers may pay for reliability, privacy controls, and lower rework; payment is unproven until real transactions occur.
- **Hypothesis H-04:** The value persists after providers improve their own session and compaction tooling; if not, Lexsus must be repositioned around *cross-provider verified state*.
- **Null hypothesis H-00:** An ordinary tracked Markdown handoff plus Git status delivers equivalent outcomes with less setup; the MVP must explicitly test against this alternative.
## 4. Customer segments and product positioning

- **Primary ICP:** Independent or small-team software engineers, actively coding with Claude Code and another AI coding host, switching on live projects at least weekly.
- **Secondary ICP:** Small engineering teams with long-lived repositories, multiple contributors, and internal policy constraints; defer team administration until individual value is demonstrated.
- **Excluded launch ICP:** Nontechnical consumers, fully regulated organizations needing formal compliance certification, and organizations seeking unsupervised privileged endpoint management.
- **Persona U-01 — Solo builder:** Has a work-in-progress feature, limited patience for manual context packaging, and values zero-cost setup more than elaborate administration.
- **Persona U-02 — Consultant:** Switches among client repositories, needs strict project boundaries and provable nonleakage, and is sensitive to accidental retention.
- **Persona U-03 — Tech lead:** Wants predictable continuation, auditability, and visibility into what the next agent will do before it acts.
- **Persona U-04 — Skeptical security reviewer:** Wants minimal scopes, network clarity, explicit revoke/delete mechanisms, and evidence that prompts cannot override local authorization.
- **Job JTBD-01:** “When my current AI session stops mid-task, help another supported AI understand the task and take the next safe step without me explaining everything again.”
- **Job JTBD-02:** “When a model suggests an edit, show what project context informed it and what local permissions it can actually use.”
- **Job JTBD-03:** “When I change machines or clients, let me transfer only the selected handoff instead of sharing my whole private project history.”
- **Positioning statement:** “Lexsus preserves reviewable coding-task state and exposes scoped project tools to supported AI agents so developers can continue without losing the work behind their code.”
## 5. Goals, north-star metric, and decision gates

- **G-01 / P0:** The target user can perform one successful Claude Code → supported destination handoff through an understandable desktop flow, without manually recreating the full task history.
- **G-02 / P0:** Handoff data lists an objective, repository identity, changes, constraints, relevant evidence, and next action or explicitly marks them unknown.
- **G-03 / P0:** A model cannot read outside a user-approved project root through a Lexsus file tool; privileged or destructive actions remain denied without the specified authorization.
- **G-04 / P0:** The initial supported local destination can retrieve handoff data through MCP in a repeatable test environment.
- **G-05 / P0:** Developers can correct wrong memories, inspect source references, and revoke access without editing internal database tables.
- **G-06 / P1:** ChatGPT/Claude web users can retrieve a **user-approved, narrowly scoped** handoff through a remote connector when their plan and host support it.
- **G-07 / P1:** Lexsus reports setup errors, offline connections, permission denials, and unsupported clients instead of silently pretending continuation happened.
- **North star NS-01:** *Verified continuation rate* = qualifying interrupted tasks where a destination agent understands the objective, retrieves allowed context, and performs a useful next step without substantial re-explanation / all qualifying pilot tasks.
- **Pilot quality threshold (proposed):** At least 8 of 10 deliberately interrupted evaluation tasks meet the rubric; the target does not constitute current performance.
- **Pilot efficiency threshold (proposed):** Median time-to-first-useful-action decreases by at least 25% versus each participant's usual handoff approach; revise after baseline measurement.
- **Safety release gate:** Zero observed unauthorized cross-project file reads, privilege escalations, or destructive actions in the release-gate adversarial suite; any finding blocks launch until fixed.
- **Business decision gate:** At least three unrelated developers or teams agree to paid pilots or complete real payments; otherwise continue research without assuming product-market fit.
- **Retention validation:** Observe repeat use across multiple distinct interruptions and voluntary product use without coaching; one successful demo is insufficient.
## 6. Non-goals and technically unsupported promises

- **NG-01:** Do not claim to scrape, capture, or reconstruct the complete private history of ChatGPT, Claude web, or other proprietary clients without supported, authorized APIs/exports.
- **NG-02:** Do not promise automatic bypass of provider usage limits, plan controls, billing limits, model rate limits, or anti-abuse mechanisms.
- **NG-03:** Do not promise automatic movement of a live hidden reasoning state or exact model-internal context between vendors; only user-visible and supported exported task state may transfer.
- **NG-04:** Do not promise that any AI host will discover, call, or obey a Lexsus tool; client behavior varies and must be tested separately.
- **NG-05:** Do not advertise a fully autonomous delegated agent system while `delegate_task` has no working runtime.
- **NG-06:** Do not advertise general-purpose OS sandboxing; worktrees and path restrictions do not contain arbitrary subprocesses.
- **NG-07:** Do not build a custom universal model router, inference hosting platform, agent marketplace, team ERP, or browser scraping extension for launch.
- **NG-08:** Do not make cloud sync of full codebases, complete conversation logs, or all process output a default or hidden prerequisite.
- **NG-09:** Do not submit a local-only plugin for general public distribution unless the applicable OpenAI eligibility and review requirements are satisfied [R8].
- **NG-10:** Do not implement arbitrary cloud-initiated shell access in the MVP; this would dramatically enlarge the remote attack surface.
- **NG-11:** Do not call a handoff “verified” merely because an LLM claims to understand it; actual file/test/next-step evidence must support the outcome.
- **NG-12:** Do not guarantee compatibility with future clients, operating systems, or provider revisions without regression testing.
## 7. Supported-platform matrix and integration constraints

| Environment | MVP | Transport | Permitted function | Release condition |
|---|---|---|---|---|
| Lexsus desktop: Linux | P0 reference platform | Native Tauri/Rust | Capture, review, Git, local file tools | Fresh installation and end-to-end tests |
| Lexsus desktop: macOS | P1 | Native Tauri/Rust | Same after platform hardening | Signed build and permission tests |
| Lexsus desktop: Windows | P1 | Native Tauri/Rust | Same after path/process hardening | Installer, permissions, and E2E tests |
| Claude Code | P0 | Local MCP stdio or supported local endpoint | Read handoff; optional locally scoped tools | Documented real-client journey |
| Codex local supported host | P0 candidate | Host-supported MCP config | Read handoff and continue code work | Validate exact host/version first |
| ChatGPT plugin / Work | P1 | Authenticated remote HTTPS MCP | Read approved handoff, check state | Host eligibility, connector review, remote tests |
| Claude web custom connector | P1 | Authenticated remote HTTPS MCP | Read approved handoff, check state | Cloud-origin connectivity and OAuth tests |
| Claude Desktop | P1 | Local MCP if configured or remote connector | Read handoff | Validate separately from claude.ai |
| Other MCP clients | P2 | Protocol-specific | Best-effort read only | Named version and contract tests |

- **INT-01:** Client capability negotiation must choose the smallest supported feature set; do not infer remote write permissions from successful read discovery.
- **INT-02:** Every adapter must declare `host_name`, `host_version`, `transport`, `supports_tools`, `supports_prompts`, `supports_resources`, and tested behavior; unknown fields remain conservative.
- **INT-03:** For remote connectors, Lexsus will expose a **stable HTTPS MCP service** that authenticates users; local loopback addresses alone are not reachable from web-hosted providers [R4][R7].
- **INT-04:** The first remote implementation provides approved snapshot retrieval; any later live relay to a local agent needs a separate threat model and release gate.
- **INT-05:** A plugin skill is an instructional workflow, not an authorization mechanism; every sensitive operation must be authorized by Lexsus server code [R3][R6].
- **INT-06:** Publication to any plugin directory is a separate commercial/operational milestone, not proof that a development-side connector works for every user.
## 8. End-to-end user journeys and failure states

- **Journey J-01 — First installation:** User downloads a trusted desktop release, selects one repository, reviews the privacy statement, and sees an inactive/active connector status without enabling remote access.
- **J-01 success:** A first-time user can tell which folder is selected, what the agent can read, what is stored, and where to disable the connection before initiating capture.
- **Journey J-02 — Capture:** User selects an available Claude Code session; Lexsus reports source, capture timestamp, detected objective, and any unreadable/truncated transcript regions.
- **J-02 failure:** If the transcript is absent, unreadable, corrupt, or permissions are denied, show a truthful partial-state notice and offer a manual task-summary path.
- **Journey J-03 — Review:** User compares proposed objective, constraints, failed attempts, completed steps, changed files, and next action against citations; edits are visible and versioned.
- **J-03 success:** User confirms a minimal handoff or marks unknown sections; Lexsus never fabricates missing information to fill a required field.
- **Journey J-04 — Local continuation:** User selects an installed supported agent; Lexsus displays the specific MCP configuration and connection test before launching or instructing use.
- **J-04 success:** Destination retrieves the handoff and Git/project state, then proposes its next action; user can allow or deny any requested file modification.
- **Journey J-05 — Remote read-only continuation:** User explicitly selects a handoff to share, signs into the Lexsus connector, and grants read scope to that handoff without exposing the entire disk.
- **J-05 success:** ChatGPT/Claude connector retrieves only the approved package; if offline or revoked, it returns a typed error with remediation rather than stale private data.
- **Journey J-06 — Resume after interruption:** User revisits a previously generated handoff; Lexsus checks repository HEAD and snapshot age and flags any evidence that may now be stale.
- **J-06 success:** A destination agent can distinguish historical decisions from the current working tree and is instructed to recheck state before writes.
- **Journey J-07 — Revoke/delete:** User disconnects a host or deletes a handoff; subsequent requests cannot read it, and local/remote retention rules are applied consistently.
- **J-07 success:** A server-side authorization test confirms revocation even if a cached client still displays an old link.
- **Journey J-08 — Unsafe request:** An agent requests a command or file outside the grant; the engine denies, records a redacted audit event, and explains the limiting policy.
- **J-08 success:** Neither tool descriptions nor model persuasion can promote read permission to write or extend filesystem roots.
## 9. Desktop information architecture and UX specification

- **UX-01 / P0:** Primary navigation uses `Projects`, `Capture`, `Handoffs`, `Connections`, and `Security`; advanced tool controls remain in a clearly separated section.
- **UX-02 / P0:** The home screen shows one dominant action, **Continue a coding task**, followed by current project, last capture, last handoff, and connection health.
- **UX-03 / P0:** The project selector displays canonical path and repository identity, warns before switching a project with pending approvals, and prevents cross-project bleed.
- **UX-04 / P0:** A source-picker displays detected sessions with origin, last modified time, associated project, approximate event count, and extraction limitations.
- **UX-05 / P0:** The handoff editor contains sections for `Objective`, `Current State`, `Decisions`, `Constraints`, `Known Failures`, `Changes`, `Tests`, and `Next Step`.
- **UX-06 / P0:** Every automatically inferred statement carries a visual `inferred` label; explicit user facts and repository-observed facts receive distinct labels.
- **UX-07 / P0:** Source references open a bounded local preview of the relevant event, file, command, or commit without silently opening external URLs.
- **UX-08 / P0:** A status strip distinguishes `Connector running`, `Agent connected`, `Handoff available`, and `Remote sharing enabled`; none implies all the others.
- **UX-09 / P0:** Permissions appear in plain language: `Read current project`, `Edit current project`, `Run approved command`, `Allow remote handoff read`, with exact scope and expiry.
- **UX-10 / P0:** Destructive approval dialogs name the tool, repository path, arguments summary, likely effects, and one-time/session decision; default action is deny.
- **UX-11 / P0:** A one-click `Copy handoff` and `Save Markdown` fallback must remain usable when a provider connector is unavailable.
- **UX-12 / P1:** A guided connector setup shows provider-specific instructions, test result, token/authorization status, and safe cleanup for failed setup.
- **UX-13 / P1:** Accessibility requires keyboard navigation, semantic focus order, readable status without color alone, and screen-reader text for permission decisions.
- **UX-14 / P1:** All trust and security UI states need screenshots and usability evaluation with developers unfamiliar with the product.
## 10. Functional requirements: project selection and workspace scope

- **FR-001 / P0:** Lexsus MUST request explicit user selection of an existing project directory; no home-folder-wide discovery or automatic all-repository indexing by default.
- **FR-002 / P0:** The canonical project root MUST be stored with a stable project identifier, filesystem root, repository identity when present, and creation timestamp.
- **FR-003 / P0:** Symlinks, `..` traversal, case normalization, platform path separators, and mount-point escapes MUST be checked using canonical filesystem paths and policy-aware reads.
- **FR-004 / P0:** Workspace boundaries MUST apply independently to read, search, write, patch, copy, move, and delete tools; a safe search result cannot authorize unsafe follow-up access.
- **FR-005 / P0:** The daemon MUST deny operations if the selected root disappears or resolves unexpectedly, and prompt the user to reselect rather than silently choosing another folder.
- **FR-006 / P0:** Switching projects MUST invalidate ephemeral tool grants, clear the active handoff selection, and update every visible scope label before enabling requests.
- **FR-007 / P1:** Multi-project listing MAY be implemented, but concurrent active workspaces MUST have separate project-scoped credentials, storage keys, and audit filters.
## 11. Functional requirements: transcript ingestion and evidence capture

- **FR-008 / P0:** The initial importer MUST support **Claude Code JSONL** only for versions and event types explicitly verified in fixtures; other sources remain manual or experimental adapters.
- **FR-009 / P0:** The importer MUST require user action and project association; it MUST NOT invisibly enumerate or upload every local session across unrelated projects.
- **FR-010 / P0:** Read operations MUST be bounded by maximum file size, event count, parsing budget, and wall-clock timeout; oversized inputs yield a partial import report.
- **FR-011 / P0:** Parsing MUST preserve source offsets or stable event identifiers so generated facts can point back to the exact supporting material.
- **FR-012 / P0:** Secret scanning and user-configurable exclusions MUST run before content is placed into a handoff or external sharing channel.
- **FR-013 / P0:** The extractor MUST distinguish a user's instruction, an agent suggestion, tool output, observed Git state, and verified test result; no suggestion becomes a confirmed outcome automatically.
- **FR-014 / P0:** Filesystem status MUST be captured separately from transcripts, including current branch, HEAD identifier, dirty-file list, and safe summarized diff statistics.
- **FR-015 / P0:** A failed or incomplete importer MUST produce a structured warning; it MUST NOT silently claim that all important context was captured.
- **FR-016 / P1:** Differential imports SHOULD de-duplicate event IDs and capture only new evidence; changing importer logic MUST NOT rewrite confirmed user facts without review.
- **FR-017 / P1:** A manual-input source adapter MUST allow a short user-authored objective, constraints, and next step without requiring Claude Code at all.

## 12. Functional requirements: structured project memory and provenance

- **FR-018 / P0:** Facts MUST have `id`, `project_id`, `type`, `statement`, `source_refs`, `origin`, `confidence_class`, `created_at`, `updated_at`, and `status`.
- **FR-019 / P0:** Supported fact types MUST include `objective`, `decision`, `constraint`, `attempt`, `failure`, `change`, `test`, `open_question`, `next_action`, and `environment`.
- **FR-020 / P0:** Confidence is categorical (`observed`, `user_confirmed`, `inferred`, `unverified`), not a fabricated numerical probability masquerading as calibration.
- **FR-021 / P0:** An explicit user correction MUST supersede an inferred fact in the active handoff while retaining a visible version-history record.
- **FR-022 / P0:** Contradictory facts MUST be flagged with source references and cannot be silently merged into a single definitive statement.
- **FR-023 / P0:** A failed approach MUST retain why it failed and under which repository state; obsolete failures MUST not permanently forbid reconsideration after code changes.
- **FR-024 / P0:** Each decision SHOULD include rationale, alternatives considered, and consequences when supported by actual evidence; missing fields remain `unknown`.
- **FR-025 / P0:** Tool output is evidence, not authority; instructions appearing inside source code, logs, comments, or transcripts MUST NOT modify Lexsus's policy or tool permission rules.
- **FR-026 / P1:** A memory review UI SHOULD enable filtering by type, source, age, stale status, and project; it must not require a separate outdated workbench screen.
- **FR-027 / P1:** Superseded and deleted facts MUST be excluded from new handoffs while preserving only the minimum audit metadata consistent with retention settings.

## 13. Functional requirements: handoff generation, review, and validity

- **FR-028 / P0:** Creating a handoff MUST assemble a versioned snapshot from the chosen session, reviewed facts, Git state, optional selected files, and user-entered next action.
- **FR-029 / P0:** A handoff MUST contain `handoff_id`, `schema_version`, `project_id`, `source`, `created_at`, `repository_state`, `sections`, `evidence_refs`, and `warnings`.
- **FR-030 / P0:** The system MUST create stable references for changes and test outcomes; it MAY omit raw code or output when a concise bounded reference is sufficient.
- **FR-031 / P0:** Any section without reliable evidence MUST display `unknown`, `not captured`, or `requires verification` instead of invented narrative.
- **FR-032 / P0:** Handoff output MUST have both a machine-readable JSON form and a Markdown export with identical semantics and visible caveats.
- **FR-033 / P0:** The reviewer MUST be able to edit any prose field, remove sensitive details, and inspect changes before the package becomes shareable.
- **FR-034 / P0:** Every snapshot MUST include creation time and repository HEAD; if the working tree changes later, the viewer MUST show a stale-state banner with an option to refresh.
- **FR-035 / P0:** The composer MUST cap handoff size by a configurable token/byte budget; it MUST prioritize objective, constraints, relevant diff, failures, and next action over exhaustive transcript history.
- **FR-036 / P0:** The generator MUST include explicit, prominent warnings when tests were not run, the source session is partial, or no verified next action was determined.
- **FR-037 / P0:** Exported files MUST be written only to a user-chosen path or an opt-in project-local `.lexsus/` directory, with documentation about `.gitignore` and accidental commits.
- **FR-038 / P1:** A deterministic selector SHOULD identify evidence most relevant to the active task using file paths, recency, task references, and fact types, with a reproducible fallback.
- **FR-039 / P1:** LLM-assisted compression MAY be offered after deterministic handoffs pass evaluation; failure must fall back to transparent extractive summaries, not a fabricated success.
- **FR-040 / P1:** Re-running generation against unchanged evidence MUST preserve stable IDs and avoid producing materially contradictory summaries without an explicit reason.
- **FR-041 / P1:** A handoff diff SHOULD show what changed since the previous snapshot: new decisions, resolved blockers, new tests, changed branch/HEAD, and removed facts.

### 13.1 Example handoff Markdown contract

```markdown
# Lexsus Handoff — <project> — <handoff-id>
Source: Claude Code / session identifier
Created: ISO timestamp | Repository HEAD: <sha> | Dirty tree: yes/no
Objective: <user-confirmed objective or unknown>
Status: <verified completed steps and remaining work>
Decisions: <rationale with source IDs>
Constraints: <facts with source IDs>
Failed attempts: <attempt, error, when, source ID>
Changed files: <paths and diff state; no secrets>
Tests: <command, result, timestamp or not-run>
Next safe action: <specific user-reviewed suggestion>
Warnings: <stale, truncated, unverified, unavailable>
```

### 13.2 Handoff verification rubric

- **HV-01:** Reviewer can identify the user objective without reading the entire original session.
- **HV-02:** Reviewer can point to at least one supporting source for each nontrivial claimed decision or test result.
- **HV-03:** Agent can identify the current repository commit and recognize a dirty working tree before editing.
- **HV-04:** Agent does not repeat a known failed approach without acknowledging new evidence that makes it worth retrying.
- **HV-05:** Agent requests clarification rather than inventing an unknown design constraint or claiming unrun tests passed.
- **HV-06:** Evaluator records whether continuation was useful based on observable artifact/test progress, not agent self-reported confidence.

## 14. Functional requirements: MCP tools, prompts, and client adapters

- **FR-042 / P0:** Define a small read-only **Continuity Profile** separate from the existing broad 58-tool registry; default exposure includes only essential metadata and reviewed handoff access.
- **FR-043 / P0:** Publish `lexsus_list_projects` with stable IDs and sanitized names; scope results to the authenticated or locally selected user's accessible projects.
- **FR-044 / P0:** Publish `lexsus_list_handoffs(project_id)` with bounded pagination, age, source, and status; do not expose raw transcript bodies through list responses.
- **FR-045 / P0:** Publish `lexsus_get_handoff(handoff_id)` that returns a reviewed structured package with warnings, repository snapshot, provenance references, and a deterministic size cap.
- **FR-046 / P0:** Publish `lexsus_get_project_state(project_id)` that returns current branch, HEAD, modified-file summary, and limitations for an approved local project; remote tools return snapshot state only unless live sync is separately authorized.
- **FR-047 / P0:** Publish `lexsus_get_evidence(ref_id)` with bounded source excerpt, origin classification, and access controls consistent with the parent handoff.
- **FR-048 / P0:** Publish `lexsus_get_connection_status()` to distinguish local connected, remote snapshot available, expired, revoked, and unsupported rather than reporting a generic success flag.
- **FR-049 / P0:** All tools MUST validate arguments against explicit JSON schemas with length limits, enumeration constraints, and opaque stable IDs; malformed requests return typed errors.
- **FR-050 / P0:** Tool results MUST include `schema_version`, `request_id`, `data`, `warnings`, and `staleness` when applicable, and SHOULD use `structuredContent` where clients support it [R4].
- **FR-051 / P0:** MCP annotations MUST match actual behavior: read-only tools cannot mutate state, write actions must not be labeled read-only, and annotations never replace server enforcement [R6].
- **FR-052 / P0:** The continuation prompt or skill MUST instruct the destination agent to retrieve the handoff, inspect warnings, verify repo state, explain the next step, and seek permission for modifications.
- **FR-053 / P0:** If the client does not support MCP prompts or skills, the same procedure MUST be available as ordinary human-readable instructions and copyable Markdown.
- **FR-054 / P0:** Existing file/Git/command tools MUST remain behind separate local profiles, with an explicit feature gate and readable permission descriptions.
- **FR-055 / P0:** Do not expose `delegate_task` as a working capability while it returns unavailable; tool discovery MUST omit it or clearly mark it unsupported.
- **FR-056 / P1:** The OpenAI plugin package SHOULD contain a focused continuity skill and MCP configuration; its manifest MUST be validated against the currently supported published format [R3][R9].
- **FR-057 / P1:** Claude Code integration SHOULD support a project-scoped or user-scoped local MCP install path with tested removal instructions and no hardcoded user-specific paths [R5].
- **FR-058 / P1:** Web remote connector MUST offer **read-only handoff retrieval first**; unsupported local editing or terminal execution requests get an explicit refusal and an instruction to open the local app.
- **FR-059 / P1:** The capability adapter MUST check authenticated identity, tenant/project ownership, and actual connector support on **every** tool call; metadata from the model is never trusted as identity.
- **FR-060 / P2:** A richer remote action relay MAY be researched after security review but MUST be a separate PRD with its own release gates, not a hidden extension of the initial connector.

### 14.1 Representative MCP request/result

```json
{
  "tool": "lexsus_get_handoff",
  "arguments": {"handoff_id": "ho_01JEXAMPLE"}
}
```

```json
{
  "schema_version": "1.0",
  "request_id": "req_01JEXAMPLE",
  "data": {
    "handoff_id": "ho_01JEXAMPLE",
    "objective": "Complete the currently reviewed task",
    "next_action": "Inspect the failing test before modifying code",
    "evidence_refs": ["ev_001", "ev_002"]
  },
  "warnings": ["Repository state was captured 20 minutes ago"],
  "staleness": "snapshot_only"
}
```

- **Contract note:** The sample identifiers and content are illustrative; the real schema and permission rules MUST be published and version-tested before rollout.

## 15. Remote ChatGPT and Claude connector strategy

- **INT-07:** Build a hosted, stable HTTPS MCP gateway for approved snapshot retrieval; it MUST support platform-compatible OAuth flow, explicit resource protection, and per-request authorization [R4][R6].
- **INT-08:** The desktop user must explicitly choose `Share this handoff remotely`; no default background upload of transcripts, file contents, or repository trees.
- **INT-09:** MVP remote sync sends a reviewed, redacted snapshot and essential metadata only; the hosted service MUST NOT fetch unapproved local files on behalf of a remote model.
- **INT-10:** A remotely shared handoff must have a resource owner, scope, optional expiration, creation time, revision, and delete/revoke operation.
- **INT-11:** A remote connector is considered functional only if an actual eligible ChatGPT or Claude account completes OAuth linking, tool discovery, data retrieval, and revoke verification.
- **INT-12:** The endpoint MUST not assume web ChatGPT and Claude can use local stdio processes; cloud-origin requests require a publicly reachable, authenticated server [R4][R7].
- **INT-13:** When the remote snapshot is too stale for safe action, the connector MUST instruct the model to verify state locally; it must not fabricate a live Git status.
- **INT-14:** Hosted data must be encrypted in transit and at rest using operationally managed keys, with separated per-user permissions and documented retention periods.
- **INT-15:** Cloud billing/availability and user plan restrictions MUST be disclosed before presenting `Connect ChatGPT` or `Connect Claude web` as universally available.
- **INT-16:** Public plugin publication MUST satisfy the host's current distribution requirements, privacy policy, data minimization rules, and review processes [R8][R10].
- **INT-17:** Any future local live-bridge using outbound connections MUST require desktop opt-in, scoped ephemeral sessions, visible connected-client identity, local approval, and disconnection when the desktop daemon exits.
- **INT-18:** The first remote product MUST work without a custom embedded UI; optional UI can follow after tool contract and security correctness are proven [R3][R4].

## 16. Security architecture, threat model, and permission policy

- **Security premise:** The sensitive assets are proprietary code, local documents, transcript history, Git state, credentials, network endpoints, command execution, and customer/project separation.
- **Threat T-01:** An untrusted transcript or source file contains prompt injection urging an agent to disclose secrets or change security policy.
- **Threat T-02:** A remote connector endpoint is stolen or misused to read another user's handoff or repository snapshot.
- **Threat T-03:** A path parameter escapes the approved repository via symlink, traversal, Windows path aliasing, or time-of-check/time-of-use behavior.
- **Threat T-04:** A seemingly safe shell command invokes a child process, reads environment secrets, or modifies outside the workspace.
- **Threat T-05:** Local or cloud logs unintentionally record tokens, secrets, private prompts, or complete source code.
- **Threat T-06:** A stale approval or shared session grant persists after project switch, logout, restart, or explicit revocation.
- **Threat T-07:** A malicious MCP host sends forged project identifiers, user IDs, annotations, or approval flags to elevate privileges.
- **Threat T-08:** A user commits generated handoff exports containing secrets to a public repository.
- **SEC-001 / P0:** Deny by default: new installation enables read-only local metadata and no external sharing; write and command tool profiles require explicit enablement.
- **SEC-002 / P0:** Enforce permission in Rust or trusted backend on every operation, using authenticated identity, project scope, action class, and a valid grant; prompts cannot authorize writes.
- **SEC-003 / P0:** Define explicit policy classes: `read`, `edit`, `destructive`, `command`, `network`, `sensitive_path`; each has a documented approval and revocation rule.
- **SEC-004 / P0:** Resolve the existing `Always`/`Destructive`/`SensitivePathOnly`/`Auto` ambiguity by mapping every exported tool to documented behavior; test both default and write-enabled modes.
- **SEC-005 / P0:** The current `propose_plan` mismatch MUST be resolved: if a read-only mode can call it, that call may propose text but must not trigger privileged side effects or approval contradictions.
- **SEC-006 / P0:** Grant data MUST include principal/client, project, allowed tools/actions, created/expiry times, revoked status, and session identifier; grants cannot broaden themselves.
- **SEC-007 / P0:** Revocation MUST be enforced immediately in the policy store for subsequent requests; stale UI state or unexpired access tokens must not keep access open.
- **SEC-008 / P0:** File operations MUST canonicalize both existing and destination paths with race-aware checks; deny symlink traversal outside the approved root.
- **SEC-009 / P0:** Sensitive path rules MUST cover `.env`, private keys, SSH folders, credential stores, browser profiles, and configurable enterprise exclusions, including case variants.
- **SEC-010 / P0:** Shell commands MUST be disabled by default, locally mediated, visibly confirmed, and auditable; the UI MUST say clearly that permission prompts are **not** an OS sandbox.
- **SEC-011 / P0:** Commands allowed under an approved policy MUST run with least privilege, restricted working directory, timeout, output cap, and restricted environment inheritance where practical.
- **SEC-012 / P0:** Explicitly ban shell execution via the remote cloud connector in the MVP, even if a legacy tool exists in the local Rust registry.
- **SEC-013 / P0:** Network-facing MCP services MUST authenticate every private read, perform object-level authorization, reject cross-tenant IDs, and rate-limit enumeration.
- **SEC-014 / P0:** OAuth flows MUST validate issuer, audience, signature, expiration, `state`, PKCE where applicable, redirect rules, and minimal resource scopes; perform a dedicated implementation review.
- **SEC-015 / P0:** Never write raw bearer tokens, refresh tokens, API keys, session cookies, or full command stdout to diagnostic logs; audit records use redacted metadata and correlation IDs.
- **SEC-016 / P0:** Untrusted transcript instructions MUST be treated strictly as data; local policy and system configuration must not be modified by content extracted from them.
- **SEC-017 / P0:** A `Kill switch` MUST immediately disable daemon tool execution and remote sharing requests, display what was revoked, and preserve an audit record without storing secrets.
- **SEC-018 / P0:** Desktop connector SHOULD bind to loopback by default and reject unexpected `Host` headers; exposing a tunnel requires an explicit warning, authentication, and scoped lifetime.
- **SEC-019 / P0:** Audit events MUST record tool ID, actor/client ID, project, decision, timestamp, safe arguments summary, and request ID; audit access is limited to the owner.
- **SEC-020 / P0:** The application MUST provide export, deletion, and retention controls for captured transcripts, handoffs, and temporary share tokens with clear storage-location explanations.
- **SEC-021 / P1:** Local archive encryption SHOULD use an OS keystore-backed key strategy where supported; existing plaintext SQLite data requires an explicit and tested migration plan.
- **SEC-022 / P1:** Remote snapshots SHOULD have short default retention and automatic expiry; deletion must remove active access immediately and queue durable storage cleanup.
- **SEC-023 / P1:** A security test suite MUST include prompt-injection fixtures, crafted symlinks, IDOR attempts, malformed OAuth flows, race conditions, grant reuse, and denied-command cases.
- **SEC-024 / P1:** Dependency/SBOM scanning, signed installers, vulnerability reporting, and release key management SHOULD be established before broad public distribution.
- **SEC-025 / P1:** The hosted gateway MUST publish a clear privacy policy and data-flow diagram, including processing locations, sub-processors if any, retention, breach contact, and deletion behavior.
- **SEC-026 / P2:** Enterprise SSO, RBAC, audit exports, and compliance programs remain follow-on requirements tied to paid organizational demand, not marketing claims for the MVP.

## 17. Privacy model and data lifecycle

- **Data class C-01:** Project metadata: name, local path, Git identifiers, status; keep locally unless explicitly included in a reviewed remote handoff.
- **Data class C-02:** Transcript events: potentially sensitive conversations and tool output; default local-only with user-selected capture and explicit deletion controls.
- **Data class C-03:** Handoff facts: reviewed project-specific information; eligible for export, but only user-approved versions may be shared remotely.
- **Data class C-04:** Authentication material: tokens and connector secrets; store only through appropriate secure credential storage and never in exported handoff JSON.
- **Data class C-05:** Analytics: non-content events and aggregate outcomes; no raw source code, prompts, transcripts, or command output in telemetry.
- **Retention R-01:** Default local retention and remote retention must be shown in Settings before public launch; do not silently claim a specific existing behavior without code verification.
- **Retention R-02:** A deleted project invalidates its handoffs, memory facts, active grants, remote share links, and future background captures in the same operation plan.
- **Retention R-03:** Backups and third-party infrastructure retention, if used, must be disclosed separately from live-row deletion; confirm operational behavior before promising purge deadlines.
- **Export R-04:** Provide user-readable Markdown and machine-readable JSON for handoffs; consider a full local data export only after a migration-ready schema exists.
- **No-training R-05:** Lexsus must not train its own models on customer code or transcripts unless a distinct, explicit, informed opt-in and separate legal review occur.

## 18. Technical architecture: components and trust boundaries

```text
SOURCE: Claude Code JSONL (user-selected local files)
    │
    ▼
Local Capture Adapter ──> Evidence Store (SQLite; local by default)
    │                                   │
    ▼                                   ▼
Fact Extraction ────────────────> Reviewed Project Memory
    │                                   │
    └──────────────────┬────────────────┘
                       ▼
                 Handoff Builder
                       │
                 Review / Redact UI
                     /     \
                    ▼       ▼
           Local MCP     Approved Snapshot Export
         (Claude/Codex)          │
                                 ▼
                         HTTPS MCP Gateway
                       (OAuth, ownership, TTL)
                          /             \
                         ▼               ▼
                    ChatGPT plugin   Claude connector
```

- **ARCH-01:** Keep the existing Tauri app as desktop UI and Rust as trusted local execution and policy layer; reuse tested modules rather than reimplementing the entire bridge.
- **ARCH-02:** Introduce `capture_adapter`, `evidence_store`, `memory_service`, `handoff_service`, `policy_engine`, `mcp_facade`, and `sync_client` as conceptual boundaries; exact Rust module layout is an ADR decision.
- **ARCH-03:** Store large raw transcript bodies separately from compact indexed fact metadata so ordinary MCP responses never accidentally return a complete history.
- **ARCH-04:** Persist schema versions and migrations with forward/backward compatibility policies; always back up or export before a destructive migration.
- **ARCH-05:** An internal event bus MAY emit `capture.completed`, `handoff.approved`, `share.revoked`, and `grant.denied` for UI updates; do not make a distributed queue mandatory for local MVP.
- **ARCH-06:** Treat local MCP and remote MCP as separate **security front doors** over deliberately shared domain logic, not as two ports exposing the same unrestricted tool registry.
- **ARCH-07:** Keep remote gateway stateless where practical, with authorization lookup and narrowly scoped encrypted snapshot storage; do not centralize full workspace mirrors.
- **ARCH-08:** Offline local continuation must continue to work even when cloud services or tunnels are unavailable, provided the chosen local MCP host is installed.
- **ARCH-09:** Version all externally visible resource schemas; breaking changes require explicit compatibility testing and migration instructions.
- **ARCH-10:** Observability must not violate privacy: use aggregate counters and redacted request traces; disable content capture by default.

## 19. Storage and domain data contracts

- **Entity Project:** `project_id`, `display_name`, `canonical_root`, `repo_remote_fingerprint?`, `active_branch?`, `created_at`, `updated_at`, `storage_policy`.
- **Entity SessionSource:** `source_id`, `project_id`, `provider`, `format`, `path_or_import_ref`, `captured_range`, `capture_status`, `warnings`, `hash`.
- **Entity Evidence:** `evidence_id`, `project_id`, `source_id`, `source_offset`, `type`, `excerpt_or_hash`, `sensitivity`, `timestamp`, `retention_state`.
- **Entity MemoryFact:** `fact_id`, `project_id`, `fact_type`, `statement`, `origin`, `confidence_class`, `evidence_ids`, `status`, `supersedes`, `timestamps`.
- **Entity RepoSnapshot:** `snapshot_id`, `project_id`, `head_sha`, `branch`, `dirty_paths`, `diff_summary`, `captured_at`, `workspace_fingerprint`.
- **Entity Handoff:** `handoff_id`, `project_id`, `source_ids`, `fact_versions`, `repo_snapshot_id`, `sections_json`, `review_state`, `warnings`, `created_at`.
- **Entity RemoteShare:** `share_id`, `owner_id`, `handoff_id`, `revision`, `allowed_scopes`, `created_at`, `expires_at`, `revoked_at`, `remote_storage_id`.
- **Entity ToolGrant:** `grant_id`, `principal_id`, `client_id`, `project_id`, `action_classes`, `issued_at`, `expires_at`, `revoked_at`, `policy_version`.
- **Entity AuditEvent:** `event_id`, `request_id`, `timestamp`, `actor`, `project_id`, `operation`, `decision`, `redacted_reason`, `resource_id?`.
- **DB-01:** Primary and foreign keys must isolate project data, including handoffs assembled from multiple imported sources; user-scoped lookups must never return another tenant's records.
- **DB-02:** Use optimistic versioning for edits to confirmed memory facts and handoffs; concurrent updates must surface conflicts instead of overwriting silently.
- **DB-03:** Source content hashes provide integrity checks and de-duplication, not proof of truth or authority.
- **DB-04:** On import errors or schema migration errors, preserve recoverable existing data and return actionable diagnostics.
- **DB-05:** Store only the minimum source excerpt needed for provenance in remote shares; a cloud evidence reference may point to unavailable local evidence and must say so.

## 20. Error semantics and recoverability

| Error code | Trigger | Safe client response |
|---|---|---|
| `PROJECT_NOT_SELECTED` | No authorized project is active | Show project picker; no default filesystem scan |
| `SOURCE_UNSUPPORTED` | No verified transcript parser | Offer manual handoff import |
| `SOURCE_PARTIAL` | Truncated or incomplete events | Warn and allow reviewed partial snapshot |
| `HANDOFF_NOT_REVIEWED` | Attempt to share a draft | Require review approval |
| `HANDOFF_STALE` | Git HEAD or snapshot outdated | Offer refresh; never pretend current state |
| `GRANT_REQUIRED` | Privileged action requested | Deny, show local grant workflow |
| `GRANT_REVOKED` | Reuse after revoke | Deny and force reauthorization |
| `RESOURCE_FORBIDDEN` | Cross-project/tenant object | Return safe denial without revealing existence |
| `REMOTE_UNAVAILABLE` | Cloud or local sync offline | Use local export or retry later |
| `HOST_UNSUPPORTED` | Connector lacks required capability | Explain available fallback |
| `RATE_LIMITED` | Gateway request budget exceeded | Safe retry guidance without sensitive detail |
| `POLICY_CONFLICT` | Tool registry/class contradicts mode | Fail closed and create internal diagnostic |

- **ERR-01:** Every error MUST have a stable code, human-readable remediation, safe diagnostic request ID, and retryable/non-retryable classification.
- **ERR-02:** A permission denial MUST NOT be retried with expanded scopes automatically or wrapped as another tool name.
- **ERR-03:** Partial success must be visible: if extraction succeeds but Git status fails, the handoff explicitly lacks verified Git state.
- **ERR-04:** The desktop must present a recovery path without directing users to paste credentials or private complete transcripts into support tickets.

## 21. Nonfunctional requirements and measurable service qualities

- **NFR-001 / P0 — Startup:** On the reference Linux machine, ordinary desktop launch SHOULD reach an interactive status screen within five seconds when local storage is healthy; benchmark and publish hardware assumptions.
- **NFR-002 / P0 — Capture responsiveness:** For a representative 10 MB transcript fixture, importing MUST provide progress feedback and must not block the UI event loop; record p50/p95 duration rather than claiming universal speed.
- **NFR-003 / P0 — Tool reliability:** Local read-only MCP calls SHOULD have at least 99% success under a reproducible, healthy test environment; exclude deliberate denials and invalid requests from transport-error calculations.
- **NFR-004 / P0 — Latency:** `get_handoff` p95 target is less than two seconds for an already-built local snapshot under reference load; remote p95 target is less than five seconds under documented network conditions.
- **NFR-005 / P0 — Bounded payloads:** All MCP output, transcript import, file preview, and diagnostic operations MUST enforce configured memory, size, and duration limits and return typed partial/error results.
- **NFR-006 / P0 — Data integrity:** Abrupt daemon termination during capture must not corrupt previously committed handoffs; use transactions, integrity checks, and restart recovery tests.
- **NFR-007 / P0 — Idempotency:** Repeating safe snapshot reads is side-effect-free, and retrying approved share-create operations must not silently create duplicate public exposure.
- **NFR-008 / P0 — Security baseline:** No known critical/high exploitable issues in the supported shipped path; threat-model findings must have documented mitigation and verified regression tests.
- **NFR-009 / P0 — Compatibility:** Pin tested Rust, Tauri, frontend, MCP SDK, and host versions in a public compatibility matrix; never promise every release of every AI provider.
- **NFR-010 / P1 — Availability:** Hosted MCP gateway SHOULD target 99.5% monthly availability during paid beta, with transparent incidents and an offline local fallback; this is a target, not a current SLA.
- **NFR-011 / P1 — Accessibility:** Keyboard-only users must complete capture/review/export; contrast and semantic control testing is part of the UI release checklist.
- **NFR-012 / P1 — Updates:** Installer upgrades must preserve user-approved projects and handoffs or provide a validated rollback/export path before schema migration.
- **NFR-013 / P1 — Privacy observability:** Operational metrics must exclude raw conversation history, source code, credentials, and arbitrary file content by design.
- **NFR-014 / P1 — Supportability:** A user must be able to generate a redacted diagnostic bundle listing app version, connector state, error codes, and recent non-content events.
- **NFR-015 / P1 — Scalability:** The remote read-only gateway must isolate tenants and remain within a measured cost and latency budget under a documented load test, not extrapolated from single-user demonstrations.

## 22. Quality assurance: test pyramid and end-to-end acceptance

- **QA-001:** Unit-test transcript parsers against valid, malformed, truncated, unknown-version, and adversarial fixture files; assert no unsupported event becomes a confirmed fact.
- **QA-002:** Unit-test handoff selection against empty sessions, contradictory decisions, many-file diffs, failed tests, and missing next steps.
- **QA-003:** Property-test ID parsing, path normalization, JSON schema validation, pagination bounds, and data-shape migrations.
- **QA-004:** Integration-test SQLite transactions, migration/rollback behavior, source content de-duplication, and concurrent edits.
- **QA-005:** Integration-test each Continuity Profile MCP tool through the actual transport and authentication path, not only through a Rust function call.
- **QA-006:** Verify that project A's principals cannot enumerate, read, modify, or infer project B's private content through IDs, errors, search, or timing-sensitive metadata.
- **QA-007:** Test all privileged tool classes under read-only mode, write-enabled mode, revoked grant, expired grant, denied prompt, and interrupted approval.
- **QA-008:** Test filesystem escape attempts including relative traversal, symlinks, junctions, alternate separators, and project-switch race conditions.
- **QA-009:** Use real supported host builds for end-to-end Claude Code and Codex continuation; simulated MCP Inspector passes are necessary but not sufficient.
- **QA-010:** For ChatGPT, validate plugin installation, login, tool selection, approved snapshot fetch, missing permission, token expiry, and revoked share on an eligible account.
- **QA-011:** For Claude remote connector, validate cloud-reachable endpoint, OAuth, tool invocation, error handling, and revoked share independently of Claude Code tests.
- **QA-012:** Perform manual usability sessions with first-time developers observing confusion, incorrect assumptions about permissions, and time to accomplish each journey.
- **QA-013:** Run documented frontend lint/type checks/build, Rust fmt/Clippy/tests, and connector health checks against a pinned checkout; publish a consistent pass/fail matrix.
- **QA-014:** Test crash recovery at each capture/review/share step; no interrupted save may silently transform an unreviewed draft into a public share.
- **QA-015:** Regression fixtures must include malicious instructions embedded in comments, transcripts, README files, tool outputs, and filenames; the policy engine must disregard them.
- **QA-016:** For any failed test, record reproduction steps, expected versus actual result, affected version, severity, owner, and verification of the fix.

## 23. Continuation evaluation protocol and pilot experiment

- **EVAL-01:** Recruit five to ten developers from the intended ICP rather than friends who only watch a guided demonstration; capture experience level and tools used.
- **EVAL-02:** Prepare comparable real or realistic interrupted tasks involving tests, partial patches, unresolved design decisions, and deliberately failed approaches.
- **EVAL-03:** Each participant completes a normal handoff baseline (`HANDOFF.md`, copy/paste, or their usual tool) and a Lexsus-assisted handoff in counterbalanced task order.
- **EVAL-04:** The evaluator records task context, interruption point, destination host, known ground truth, handoff creation time, and tool/permission failures.
- **EVAL-05:** A useful first action is an independently reviewable code change, relevant successful diagnostic, correct test reproduction, or meaningful verified plan tied to the task.
- **EVAL-06:** “No substantial re-explanation” means at most one short factual clarification that does not restate the task architecture; log all clarifications verbatim only with consent.
- **EVAL-07:** Score `objective understood`, `constraints respected`, `failed attempts avoided`, `Git state recognized`, `useful action achieved`, and `unauthorized action avoided` independently.
- **EVAL-08:** Measure median and p75 time-to-first-useful-action and compare paired differences; report raw sample size and uncertainty, not statistical significance claims from tiny samples.
- **EVAL-09:** Survey trust and perceived effort only after observing behavior; ask what the user would do without Lexsus and whether they would use it tomorrow.
- **EVAL-10:** Predefine a stopping rule: if handoffs repeatedly omit crucial context or don't outperform manual Markdown, stop expanding integrations and fix the core.

## 24. Product analytics: instrumentation without collecting customer code

- **Metric A-01 — Setup completion:** Users who connect one supported host and retrieve a test handoff / users who start installation.
- **Metric A-02 — Activation:** Users completing one reviewed handoff plus one successful destination retrieval / installed users; report separately for local and remote.
- **Metric A-03 — Verified continuation:** Qualifying tasks passing the independent continuation rubric / all qualified evaluated interruptions.
- **Metric A-04 — Time saved:** Per-user baseline time to useful action minus Lexsus time; include distribution, task difficulty, and errors instead of reporting average only.
- **Metric A-05 — Handoff usefulness:** Share of handoffs with a verified objective, next step, repo state, and at least one valid evidence reference.
- **Metric A-06 — Correctness cost:** Number of stale-context mistakes, repeated known failures, false confirmed facts, and harmful suggested actions per task.
- **Metric A-07 — Trust friction:** Permission denials by category, approval wait time, unexpected tool-call attempts, and revoke failures; never treat fewer prompts alone as safer.
- **Metric A-08 — Reliability:** Transport/host/tool success rates, timeouts, partial imports, offline states, and provider version regressions.
- **Metric A-09 — Retention:** Developers returning for a second distinct coding interruption in 7 and 30 days; distinguish real repeat usage from repeated retries of one failed attempt.
- **Metric A-10 — Monetization:** Qualified pricing conversations, paid conversion, trial-to-paid, refunds, churn, support cost, hosting cost, and observed gross margin.
- **Event policy:** Send non-content names such as `handoff.created`, `handoff.reviewed`, `mcp.fetch.succeeded`, `permission.denied`, and `session.continuation.verified`, with consent and minimal metadata.
- **Anti-gaming rule:** Product analytics must not classify self-reported “done” text as successful code continuation when no actual task-level evidence exists.

## 25. Commercial thesis and operating model — assumptions, not market proof

- **Business hypothesis:** Individual developers initially adopt through a free local product; sustained value comes from reliable cross-client workflows, authenticated remote handoff convenience, and support for multi-project professionals.
- **Free experiment:** Local capture, review, Markdown export, and one verified local MCP continuation should remain available to allow comparison with manual workflows.
- **Paid experiment A:** Offer hosted read-only connector convenience with transparent retention and quota limits; ask users to choose whether it saves meaningful setup time.
- **Paid experiment B:** Offer professional support, expanded history, optional secure sync, or team administration only when buyers explicitly request and value them.
- **Pricing experiment:** Test willingness to pay through honest landing-page interviews and real opt-in transactions; do not manufacture a market-based price from hypothetical conversion math.
- **Acquisition channels:** Technical demos of genuine interrupted tasks, open-source documentation, GitHub issues/discussions, carefully disclosed Reddit participation, developer meetups, and integration tutorials.
- **Proof asset:** Publish an uncut or reproducible side-by-side comparison showing baseline handoff versus Lexsus on the same class of task, including failure cases.
- **Distribution risk:** Provider-native session handoff features and free Markdown workflows may capture most of the value; differentiation must be measured, not asserted.
- **Cost model:** Measure monthly gateway hosting, storage per approved share, bandwidth, monitoring, support time, auth costs, and payment processing before proposing margins.
- **Gross margin formula:** `(recognized recurring revenue - attributable delivery/support infrastructure costs) / recognized recurring revenue`; exclude one-time funding as revenue.
- **CAC definition:** Fully loaded acquisition spend divided by newly acquired paying customers for a defined period, with founder sales labor treated transparently.
- **LTV caution:** Do not calculate meaningful lifetime value until there is a measured retention cohort; early churn assumptions are too fragile.
- **Decision checkpoint:** If the majority of pilot users say a manual `HANDOFF.md` is simpler and achieve similar results, narrow or rethink the product instead of expanding the feature catalogue.

## 26. Delivery sequence, dependencies, and release gates

| Gate | Deliverable | Required proof | Can proceed when |
|---|---|---|---|
| G0 — Baseline | Pin checkout, module inventory, test logs, supported versions | Frontend/Rust checks run; historical contradictions resolved | Critical failures triaged |
| G1 — Safe capture | Source picker, parser, bounded archive, project controls | Fixture imports + scope/path tests | Partial-data warnings are truthful |
| G2 — Handoff | Provenance-linked memory, review UI, JSON/Markdown export | Review acceptance + snapshot/staleness tests | Handoff is independently inspectable |
| G3 — Local continuation | Minimal MCP profile, Claude Code and one second host | Actual interrupted-task demos on pinned hosts | No hidden grants or unsupported claims |
| G4 — Pilot | Real user tests versus manual baseline | Outcome, trust, friction, retention notes | Measurable improvement and demand signals |
| G5 — Remote beta | OAuth gateway, opt-in redacted snapshots, ChatGPT/Claude tests | Tenant isolation, revoke, host E2E, privacy review | Security review approves launch |
| G6 — Paid beta | Transparent plan and onboarding | Real payments + unit-cost tracking | Repeat use and support burden acceptable |

- **Dependency DP-01:** Do not add remote connectors before reviewed snapshot contracts and object-level authorization work locally.
- **Dependency DP-02:** Do not add AI compression before evidence provenance, hallucination checks, and deterministic fallback behavior are stable.
- **Dependency DP-03:** Do not promote Windows/macOS as fully supported solely because Tauri can compile; require installation, filesystem, subprocess, auth, and host-specific E2E evidence.
- **Dependency DP-04:** Do not open unrestricted local write profiles until the permission-class mismatch and denial/revoke tests are fixed.
- **Dependency DP-05:** Do not spend heavily on public plugin listing, pricing pages, or broad distribution before a reproducible continuation demonstration and initial user evidence.
- **Resourcing note:** A small team can deliver this sequence by reusing the existing core; attempting all providers, remote execution, team collaboration, and autonomous orchestration simultaneously is not a credible MVP scope.

## 27. Risks, dependencies, and mitigations

| Risk | Likelihood / impact (qualitative) | Leading indicator | Required mitigation |
|---|---|---|---|
| Provider API or plugin changes | High / high | Auth/discovery tests fail on host update | Pinned matrix, adapter contract tests, clear fallbacks |
| Manual Markdown solves enough | High / high | No observed time reduction | Controlled baseline, stop rule, narrow differentiation |
| Memory invents decisions | Medium / high | Unsupported facts in sampled handoffs | Source references, editor review, unknown labels |
| Remote data leak | Lower frequency / critical impact | Cross-tenant access attempt or unsafe scope | Strong authz, test suite, privacy/security review |
| Command execution misuse | Medium / critical impact | Unapproved subprocess access | Local-only gated commands; no MVP cloud shell |
| Complex onboarding | High / medium | High install-to-activation drop-off | Guided journey, copy fallback, tested host versions |
| Dependency on one transcript format | High / medium | Claude Code format changes | Parser fixtures, version detection, manual import |
| Hosting economics uncertain | Medium / medium | Share size/traffic exceed budget | Small snapshots, quotas, cost accounting |
| Too many inactive tools | High / medium | Users cannot locate handoff workflow | Minimal Continuity Profile, primary CTA |
| Security claims exceed reality | Medium / high | Marketing says 'sandbox' or 'always approves' | Policy-to-copy audit, red-team review |

## 28. Launch checklist and go/no-go authority

- **L-01:** Product owner confirms ICP, headline promise, and evidence classification without claiming validated PMF.
- **L-02:** Engineering signs off the exact git commit, reproducible builds, supported-host versions, and all P0 acceptance checks.
- **L-03:** Security reviewer signs off privacy boundary, auth flows, authorization, local command warnings, retention, and unresolved risk inventory.
- **L-04:** QA supplies end-to-end video/log evidence for capture → review → destination fetch → useful action, including a failing-case demonstration.
- **L-05:** Docs use the current single dashboard, not deleted workbench screenshots or nonexistent API/tool names.
- **L-06:** Support pages describe offline operation, errors, how to revoke, how to remove connectors, how to export, and what is never collected.
- **L-07:** Every marketing claim has a proof source: repository observation, host documentation, measured pilot result, or explicitly labeled future intent.
- **L-08:** A privacy/security incident owner and rollback mechanism are identified before any public remote service launch.
- **L-09:** If any critical unauthorized data access is reproducible, the release is **no-go** regardless of pilot satisfaction or feature completeness.

## 29. Open questions and written decision register

- **Q-01:** Which exact Codex client and version is the first verified destination: CLI, desktop, or another host? Answer through tested compatibility, not branding.
- **Q-02:** Should local snapshots live wholly in SQLite, or should handoff artifacts also be written into a private project-local directory by default? Decide after repository/privacy research.
- **Q-03:** Which transcript elements are essential for continuity, and which unnecessarily increase risk, latency, or token use? Answer through ablation tests.
- **Q-04:** Which edits should default to explicit approval in read/write mode? Resolve tool-class contradictions before expanding write access.
- **Q-05:** Does remote read-only handoff retrieval create meaningful extra value over copy/paste or local MCP, and for which user segment?
- **Q-06:** What retention periods and pricing limits can the team operationally guarantee, given the selected infrastructure and user expectations?
- **Q-07:** Which provider-specific connector review and plan restrictions apply to the first actual beta cohort at launch date?
- **Q-08:** What measured minimum win over manual handoff is enough to justify the ongoing security and hosting complexity?

## 30. References and evidence provenance

- **[S0] Primary source (user supplied):** *Lexsus — Full Project Report*, prepared 17 October 2026, supplied in this conversation as `Pasted text(2).txt`; descriptions of current implementation, gaps, and status are derived from this report, not a new code audit.
- **[R1] Atlassian:** [What is a Product Requirements Document?](https://www.atlassian.com/agile/product-management/requirements/) — PRD objectives, assumptions, scope, user needs, and shared understanding.
- **[R2] Product School:** [The Only PRD Template You Need](https://productschool.com/blog/product-strategy/product-template-requirements-document-prd) — standard requirement-document organization and release alignment.
- **[R3] OpenAI:** [Plugin architecture](https://developers.openai.com/plugins/concepts/plugins) — skills, MCP servers, host-specific behavior, and optional UI.
- **[R4] OpenAI:** [Build an MCP server](https://developers.openai.com/plugins/build/mcp-server) — tool schemas, structured results, authentication, host testing, and deployment.
- **[R5] Anthropic:** [Claude Code MCP reference](https://code.claude.com/docs/en/mcp) — supported local/remote transports, scopes, installation, and host behavior.
- **[R6] OpenAI:** [Plugin security and privacy](https://developers.openai.com/plugins/guides/security-privacy) — least privilege, privacy, server-side authorization, tool annotations, and prompt-injection defenses.
- **[R7] Anthropic:** [Custom connectors using remote MCP](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp) — cloud-origin network access, connector setup, and permissions.
- **[R8] OpenAI:** [Desktop-only plugins](https://developers.openai.com/plugins/deploy/desktop-only) — current distribution caveat for local MCP submissions.
- **[R9] OpenAI:** [Package your plugin](https://developers.openai.com/plugins/build/plugins) — portable plugin manifests, skills, and MCP configuration.
- **[R10] OpenAI:** [Plugin guidelines](https://developers.openai.com/plugins/plugin-guidelines) — publishing rules, narrow data access, privacy policy, and restricted-data requirements.
- **Source usage:** Documentation was consulted on 17 October 2026; vendor platform rules may change. This PRD's feasibility conclusions follow documented APIs and intentionally do not depend on undocumented chat scraping.

---

**Final product decision:** Build **Lexsus Continuity Core** first, demonstrate source-linked handoff on one interrupted coding task, validate against manual Markdown with real developers, and add ChatGPT/Claude remote read-only integrations only when security and user demand justify them. A plugin is a useful distribution surface; verifiable continuity is the product.
