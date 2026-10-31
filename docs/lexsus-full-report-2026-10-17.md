# Lexsus — Full Project Report

**Prepared:** 17 October 2026. **Basis:** the current local source code, requirements, architecture documents, workflow configuration, and development records. This report includes uncommitted work visible in the checkout. It is a source review; no application launch, test run, provider compatibility check, or developer study was performed for this report. Historical test results are identified as historical.

**1. What Lexsus is**

Lexsus is a desktop application for continuing AI-assisted coding work across interrupted sessions and different AI tools. It preserves project context and gives a connected AI access to tools on the developer's computer.

Its central promise is: **“Your AI can change. Your work doesn't.”**

For example, a developer starts a task in Claude Code. The session reaches a usage limit halfway through a change. Lexsus reads available session context, collects project facts and changed files, and prepares a handoff. A compatible AI connects through Lexsus and continues by reading files, editing code, and running commands against the same project.

The intended benefit is less time spent reconstructing context: what the task is, which decisions were made, what failed, and what should happen next.

**2. Why we are building it**

AI coding work accumulates valuable context inside individual sessions. An interruption can separate the next AI from the reasoning behind the existing code. Files show the current result, but often omit rejected approaches, constraints, and unfinished steps.

Lexsus addresses four connected problems:

- **Lost continuity:** a new session needs the objective, decisions, failed attempts, and remaining work.
- **Manual transfer:** developers spend time copying explanations, code, and command output between tools.
- **Limited local access:** a web conversation needs a controlled connection to the repository to perform coding work.
- **Trust:** developers need to see what the AI accesses and control sensitive actions.

The initial audience is developers already using AI coding tools who experience interrupted tasks or switch between models. The repository defines an initial validation group of 5–10 real developers. Demand, retention, and willingness to pay remain questions for that validation.

**3. How the system works**

The architecture has four layers:

| Layer | Responsibility | Current position |
|---|---|---|
| Session archive | Store session events and captured context in SQLite | Implemented |
| Structured project memory | Maintain objectives, decisions, constraints, failed attempts, and changed files | Implemented using explicit memory tools and heuristic extraction |
| Context compression | Produce a smaller, useful summary for a fresh context window | Service skeleton exists; `/compress` returns HTTP 501 |
| Handoff and local tools | Expose continuation context and execute calls from an MCP client | Implemented in the Rust core; real provider continuation still needs documented validation |

The desktop interface uses React, TypeScript, Vite, and Tauri 2. Rust handles the connector, file operations, processes, Git, storage, watching, authentication, and other tools. Python/FastAPI is reserved for the optional compression service.

The local agent runs in the developer's own terminal. Lexsus's transcript reader specifically understands Claude Code JSONL files under its projects directory. This supports continuity from Claude Code; equivalent capture from every other local agent should not be assumed.

MCP is the connection between the AI host and Lexsus. The server binds to loopback by default. A remote AI service needs an explicitly exposed route, currently supported through tunnel management. Local MCP hosts can connect directly.

**4. Features present in the source**

The current registry contains **58 named tools**. This measures the registered surface, not 58 independently validated capabilities. In particular, `delegate_task` returns `AGENT_NOT_AVAILABLE` because there is no delegation runtime in this build.

| Capability | What is available | Qualification |
|---|---|---|
| File access | Chunked reads, multiple-file reads, directory listing, media reads | Sensitive paths have additional policy handling |
| Editing | Write, exact-string edits, multiple edits, patches, copy, move, delete, directory creation | Write visibility and approval policy are separate controls |
| Search | Text search and path globbing | Implemented locally |
| Commands | Foreground execution, background execution, output retrieval, termination | Powerful access running with the local process's permissions |
| Git | Status, diff, history, staging, unstaging, commits, branches, checkout, revision inspection | Backend tools remain available despite removal of the dedicated Git screen |
| Project memory | Objectives, decisions, constraints, attempts, tasks, facts, session listing | Extracted facts can be incomplete or misclassified |
| Handoff | Request and retrieve handoff context; `continue_work` MCP prompt | Continuation is pulled by the client; provider presentation needs validation |
| Web | Fetch and search | Fetch includes an SSRF guard; network-dependent behavior still needs operational validation |
| Notebooks | Read and edit notebook cells | Registered and implemented |
| Code intelligence | Diagnostics, definitions, references, symbols through language servers | Depends on available language servers and their configuration |
| Human interaction | Questions, plans, notifications, monitoring, findings | The desktop carries approval and question interactions |
| Isolation and delivery | Git worktrees and artifact publication | Worktrees isolate changes; they do not provide a full OS sandbox |
| Delegation | A tool name and explicit unavailable response | No functioning sub-agent runtime |

The application also includes connector start/stop/restart controls, configurable ports and allowed hosts, tunnel controls, OAuth client management, project selection, onboarding, activity statistics, file and command records, audit information, approval banners, session grants, and a kill switch.

**5. What the desktop experience is today**

The current application is a **single dashboard**. `App.tsx` renders `DashboardView` with global approval, question, grant, and failover banners, a project dialog, onboarding, and a status bar.

The dashboard combines connector and tunnel controls with collapsible sections for OAuth clients, tools, activity, files, commands, audit records, and recent events.

Older documentation describes a workbench with separate Git, memory, handoff, trace, and connector views plus a persistent terminal. Those components have been deleted in the current working tree. Their Rust and MCP capabilities largely remain, but the former desktop workflows cannot be presented as current UI features.

This creates a product question: can a first-time user find and complete the main handoff journey through the simplified dashboard? The implementation review does not establish that. A clear, visible path from “my agent stopped” to “continue this task” deserves direct user testing.

**6. How development is going**

Lexsus has moved beyond a scaffold. It has substantial implementations for local execution, project memory, authentication, process management, Git, and connector control. Development records also show a move from browser-extension integration to native MCP, followed by OAuth work and dashboard consolidation.

My assessment is **an advanced prototype / early MVP with significant engineering breadth and incomplete product validation**. A percentage-complete estimate would be misleading because the key uncertainty is whether developers can successfully continue real interrupted tasks.

The current checkout contains many staged and unstaged changes, including the dashboard simplification and a split of the bridge module into `mod.rs`, `specs.rs`, and `schema.rs`. This report describes that working state; it does not establish the state of a published release.

CI configuration now includes frontend lint, type checking, and builds; Rust formatting, Clippy, and tests; and compression-service health checking. This is useful coverage, but configuration alone does not prove the checks currently pass.

The historical status file contradicts itself: its opening claims 133 passing Rust tests with no failures, while a later September entry reports 174 passing and six failing. It also retains older statements that Rust tests are absent from CI, although the current workflow includes them. Current health needs a fresh recorded run before making a clean-build claim.

**7. Security and control: what is implemented and what remains**

Implemented controls include loopback binding, host restrictions, bearer authentication, OAuth support, a read-only default surface, sensitive-path handling, approval classes, scoped session grants, an audit trail, and a kill switch. Response signing is also implemented.

The precise approval behavior matters. The README broadly claims every mutation asks for approval. The code assigns tools different classes: `Always`, `Destructive`, `SensitivePathOnly`, and `Auto`. For example, ordinary `edit_file` and `multi_edit` calls use `SensitivePathOnly`, so an ordinary non-sensitive edit does not necessarily prompt once the write surface is enabled. The report should describe the actual policy, and the product wording should match it.

Additional limits are explicit in the repository: SQLite storage is plaintext; commands lack a full OS sandbox; grant persistence, expiry, and configuration remain incomplete. A workspace boundary for file tools should not be interpreted as containment of arbitrary shell behavior.

The `propose_plan` tool is on the read-only surface while its registry class requires approval. This is a policy inconsistency recorded in the project. It needs an explicit decision about what “read-only” promises.

OAuth support exists in code, including client management, authorization routes, PKCE-related handling, and token validation. Its presence should be followed by a dedicated review and real connector tests before treating remote access as production-ready. This report is not a security audit.

**8. What is missing or unfinished**

| Gap | Why it matters | Priority |
|---|---|---|
| Documented live continuation through a real provider | Tests of individual components cannot prove the complete user journey | Highest |
| Validation with 5–10 developers | Establishes whether handoffs save effort and produce successful continuation | Highest |
| Current, consistent setup and status documentation | Users cannot reliably follow instructions for deleted screens or outdated authentication behavior | Highest |
| Fresh quality baseline | Conflicting historical results prevent a reliable health assessment | Highest |
| Clear handoff entry point in the current dashboard | The main product value needs an understandable desktop journey | High |
| Accurate, explainable approval policy | Users need to understand which actions will ask and which can run automatically | High |
| Stronger memory quality and provenance | Sentence markers can miss nuance, stale decisions, and the actual next action | High |
| Cross-platform release evidence | macOS, Windows, and Linux support is an objective; equivalent behavior is not established by this review | High |
| Production remote-access and authentication validation | Tunnel-based setup introduces onboarding and operational dependencies | High |
| Context compression | Useful for larger histories; explicitly outside the initial MVP scope | Medium |
| Persistent and configurable grant policies | Needed for predictable longer-term permission management | Medium |
| Local encryption and retention controls | Relevant to stored proprietary code, transcripts, and sensitive context | Medium, depending on target users |
| Working delegation runtime | Required before advertising actual delegated execution | Later |
| Orchestration and team capabilities | Routing, shared state, RBAC, and enterprise deployment remain future scope | Later |

The source also supports failover detection and in-app continuation offers. This should not be marketed as fully automatic switching: a new AI still needs to receive or retrieve the handoff and participate through its host's connector capabilities.

**9. What we should do next**

1. **Establish the current baseline.** Record the exact checkout and fresh frontend/Rust check results. Resolve failures and reconcile status claims.
2. **Repair the setup documentation.** Align instructions with the dashboard, OAuth implementation, current tool registry path, and actual permission behavior. Remove obsolete instructions about six read-only tools, deleted screens, and an unbuilt `get_handoff` tool.
3. **Prove one provider journey.** Start with the intended first MCP host. Capture evidence of discovery, reads, approved writes, commands, denial recovery, interruption, and continuation using retrieved project context.
4. **Make handoff quality reviewable.** Let the developer inspect the objective, constraints, failed attempts, changed files, and next step. Preserve source references and distinguish heuristics from explicit decisions.
5. **Run the 5–10 developer pilot.** Give users real interrupted tasks and observe completion, confusion, context loss, permission friction, and trust.
6. **Harden the problems that the pilot exposes.** Improve onboarding, memory, connection reliability, and controls before expanding into more providers, orchestration, or team administration.

**10. How to measure success**

The requirements identify **successful continuation rate** as the primary metric: the share of real interrupted tasks that another AI can continue with local tool access and without substantial re-explanation.

Define that metric before the pilot. Record whether the new AI understood the task, obtained the relevant context, performed a useful next action, and avoided repeating known failed approaches. Record failures as well as successes.

Supporting measures should include time to first useful action, setup completion rate, clarification effort after handoff, tool-call failures, approval timeouts, repeat use, and developer-reported trust. Compare against the developer's normal manual handoff process. These measurements would test the claimed benefit; the repository does not currently establish the results.

**11. Product direction and business questions**

The clearest product direction is continuity of coding work with controlled local execution. That gives future features a practical filter: does this improve successful continuation, reduce setup effort, preserve reasoning, or increase trust?

The repository uses an MIT license. This review found no basis for claiming revenue, paying customers, retention, a validated pricing model, or product-market fit. Those need direct evidence. Pricing and commercial packaging should follow learning about who repeatedly needs the product and which benefits they value enough to pay for.

The main technical dependency is the host's ability to connect and use MCP correctly. The main product dependency is the quality of the transferred context. The main execution risk is expanding the feature surface before proving the core continuation workflow.

**12. Evidence and confidence**

The strongest source-backed conclusions are that Lexsus has a substantial Rust tool backend, a dashboard frontend, Claude Code transcript ingestion, structured memory, handoff retrieval, authentication mechanisms, and a compression stub. The important unresolved claims are real provider reliability, useful continuation without re-explanation, current build health, cross-platform readiness, and repeat developer demand.

Primary repository references:

- [Product requirements](../requirements/product-requirements.md) and [MVP scope](../requirements/mvp-scope.md): purpose, boundaries, and validation criteria.
- [README](../README.md) and [project status](../PROJECT_STATUS.md): positioning and historical progress, with the inconsistencies discussed above.
- [App shell](../src/App.tsx), [dashboard](../src/views/DashboardView.tsx), and [frontend guide](../src/README.md): current desktop experience.
- [Tool registry](../src-tauri/src/bridge/specs.rs), [executor and policy](../src-tauri/src/bridge/mod.rs), and [MCP server](../src-tauri/src/mcp.rs): registered tools, behavior, and exposure rules.
- [Transcript reader](../src-tauri/src/transcript.rs), [fact extraction](../src-tauri/src/facts.rs), and [core integration](../src-tauri/src/lib.rs): capture and handoff behavior.
- [OAuth implementation](../src-tauri/src/oauth.rs), [Tauri configuration](../src-tauri/tauri.conf.json), [compression service](../compression-service/main.py), and [CI workflow](../.github/workflows/ci.yml): authentication, packaging, compression status, and configured checks.

**Recommended immediate milestone:** demonstrate one complete interrupted-task continuation through the current desktop and a real MCP host, with recorded evidence, then repeat it with the initial developer group.
