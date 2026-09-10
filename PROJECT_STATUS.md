# Lexsus — Project Status & Roadmap

> **"Your AI can change. Your work doesn't."**
> A local-first desktop app that turns any **MCP-capable web AI** — Claude.ai first, any MCP host in general — into a **real coding agent on your machine**, so when your local agent (Claude Code) hits a limit, crashes, or you just want to switch, you hand off real project state instead of re-explaining it.

**Snapshot** · 2026-09-10 · branch `developing` → `main` · MIT license

| | |
|---|---|
| 🧩 What exists | 15 of 58 planned tools built · 5-view workbench · **native MCP connector (single transport)** · Rust core (~8.5k LOC) + React/TS (~5.0k LOC) |
| ✅ Code healthy | `cargo fmt --all --check` clean · `cargo clippy --lib --all-targets -- -D warnings` exit 0 · `pnpm typecheck` clean · `pnpm lint` 0 errors (6 pre-existing warnings) · `pnpm build` succeeded |
| ⚠️ Unverified | **`cargo test --lib` was not run** after this change (see §6). Last green: **62 passed / 0 failed** on 2026-09-09, before the extension removal |
| 🚪 Still to prove | **M1 live gate** (a real web-AI session end-to-end through the connector) and the **exit gate** (5–10 developers) — the actual startup validation is not yet done |

---

## 1. What this product is

Lexsus solves one problem: *AI-assisted work dies when the agent dies.* When your local Claude Code session hits a usage/context limit, crashes, or you simply prefer a different model, the working state (what was attempted, what changed, what failed, what's next) is trapped inside that session.

Lexsus is the **neutral bridge**:
1. **Captures** the real state of your work — from git, the filesystem watcher, Claude Code's own transcripts, and the web AI's tool activity (not from chat memory).
2. **Structures** it into facts, not chat: objective, decisions, failed attempts, constraints, changed files, progress.
3. **Delivers** a handoff to a web AI and **gives that web AI real tools** (`read_file`, `write_file`, `run_command`…) executed locally by the Rust core with approval gates.

It is **not** a chat-history copier, a limit bypass, or browser automation/DOM scraping.

---

## 2. Architecture at a glance — one connector, four layers

```
 MCP-capable web AI  ⇄  MCP (Streamable HTTP, loopback)  ⇄  [Rust core ⇄ SQLite]  ⇄  your files/git/terminal
                          http://127.0.0.1:45147/mcp
```

> **Major change, 2026-09-10:** the Chrome extension transport was **removed entirely** — `src-tauri/src/ws.rs`, `extension/`, `scripts/check-spec-sync.mjs`, the `tungstenite`/`getrandom` deps, 4 Tauri commands, 5 `AppState` fields, and all frontend pairing UI. The loopback WebSocket (`ws://127.0.0.1:45241`), 6-digit pairing, DOM scraping and composer injection are gone. The native MCP server is now the **only** transport. Full write-up: [`ongoing/2026-09-10-mcp-native-architecture.md`](ongoing/2026-09-10-mcp-native-architecture.md).

**The connector** (`src-tauri/src/mcp.rs`) is an `rmcp` 3.2 MCP server speaking Streamable HTTP, bound to loopback only, running on its own OS thread with its own tokio runtime. Two controls define its posture:

- **Read-only first:** `mcp_allow_write` (default **off**) hides the write/command tools from `tools/list` entirely. Seeded by `LEXSUS_MCP_ALLOW_WRITE`, flipped live from the desktop via `mcp_set_allow_write`. `mcp_status` reports `{listening, endpoint, allow_write, workspace}`.
- **DNS-rebinding allowlist:** loopback is always accepted; anything else is opted in per-environment via `LEXSUS_MCP_ALLOWED_HOSTS` — needed only when exposing the connector through a tunnel for a cloud-hosted provider.

**Bridge A — local agent capture:** the developer runs Claude Code in their *own* terminal. Lexsus does **not** wrap or mirror it; it reads the real signals it can see itself (git, fs watcher, `~/.claude/projects/*.jsonl` transcripts, web-AI tool activity).

**Bridge B — web-AI coding-agent bridge:** the connector exposes the tool surface to the web AI over its native tool channel, executes calls locally, and returns results.

| Layer | Purpose | Status |
|---|---|---|
| **1. Session Archive** | Raw, lossless record of executed tool calls + file/git/terminal events | ✅ **Built** (`archive.rs`, DB migration `0005`) |
| **2. Structured Project Memory** | Facts, not chat: objective / decisions / failed attempts / constraints / changed files / progress | ✅ **Built** (`facts.rs`, `db.rs` memory API) |
| **3. Context Compression** | LLM-summarize state to handoff size | 🔶 **Stub only** — FastAPI service exists, `/compress` returns 501 by design |
| **4. Handoff Engine** | Format the handoff, establish the bridge | ✅ **Built** (`bridge.rs`, `transcript.rs`, `mcp.rs`) — but delivery is now **pull-based**; see §7 |

---

## 3. What is BUILT today

### 3.1 The Rust core (`src-tauri/src/`, ~8.5k LOC)

| Module | LOC | What it does |
|---|---|---|
| `bridge.rs` | 3,489 | The **tool engine**: 15-tool `SPECS` registry, permission model, sensitive-path rules, `resolve_path` containment, session grants, chunked reads, edit/patch/apply logic, plus the shared `parse_tool_call` coercion and `tool_input_schema` JSON Schemas. Largest module. |
| `lib.rs` | 1,060 | Tauri command wiring: git commands, watcher, trace recording, handoff builder, grants, failover, terminal events, connector spawn/status. |
| `db.rs` | 853 | Embedded SQLite + hand-rolled versioned migrations (0001–0005+): sessions, session_events, facts tables, trace_steps, audit_log, settings. |
| `transcript.rs` | 619 | Claude Code JSONL transcript reader (epoch parsing, munge-path matching, timeline events). |
| `mcp.rs` | 376 | **The connector**: rmcp Streamable HTTP server, gated `tools/list`, `tools/call` → policy engine, result cap, allowed-hosts handling. |
| `pty.rs` | 353 | `portable-pty` one-shot command execution with streaming, timeout→kill, output cap, 500 ms quiet-drain after exit. |
| `failover.rs` | 323 | Interruption state machines: local `inactive→working→stalled→interrupted` (vetoed by file changes); web direction is now **inactivity-only** (no socket left to watch). |
| `process.rs` | 322 | Process-group registry for `run_command`; owner-scoped kill; SIGKILL escalation when the group outlives the leader; protected PIDs. |
| `git.rs` | 321 | `git2` full workflow: status, diff, stage/unstage, branches+checkout (refuses dirty tree), log, commit_diff. |
| `archive.rs` | 269 | Mirrors Claude Code transcripts into the session archive, idempotent (source-path + mtime dedupe). |
| `facts.rs` | 259 | Sentence-level heuristics → decisions/failed attempts/constraints; deterministic progress heuristic; output caps. |
| `shell.rs` | 144 | Shell detection/abstraction (Sh/Bash/Zsh/Cmd/PowerShell). |
| `watcher.rs` | 64 | `notify` recursive file watcher (single replaceable watch, no thread-per-start leak). |

### 3.2 The web-AI tool surface — **15 of 58 tools built**

Every tool is registered **once** in `SPECS` (`src-tauri/src/bridge.rs`) with a matching JSON Schema in `tool_input_schema()` and matching coercion in `parse_tool_call()`. The drift guard is now the Rust unit tests in `bridge.rs`/`mcp.rs` — the old `scripts/check-spec-sync.mjs` and its JS registry are gone with the extension.

| Group | Tool | Approval | Auto-insert |
|---|---|---|---|
| Reading | `read_file` (chunked/paged) | SensitivePathOnly | yes |
| Reading | `list_directory` | Auto | yes |
| Reading | `read_many_files` (≤20 paths, 20KB budget) | Auto | yes |
| Editing | `write_file` | Always | no |
| Editing | `edit_file` (string replace) | SensitivePathOnly | no |
| Editing | `multi_edit` (batched, atomic) | SensitivePathOnly | no |
| Editing | `apply_patch` (unified diff + drift search) | Always | no |
| Editing | `delete_file` | **Destructive** | no |
| Editing | `move_file` (both paths sensitive-checked) | **Destructive** | no |
| Editing | `copy_file` (both paths checked) | Always | no |
| Editing | `create_directory` | Auto | no |
| Commands | `run_command` (streams live, 120 s / 1 MB) | Always | no |
| Git | `git_status` | Auto | yes |
| Meta | `list_tools` / `describe_tool` | Auto | yes |

**Two independent controls, don't conflate them:** the **read-only gate** (visibility — is the whole write/command surface advertised at all?) and the **approval class** (once visible, does *this* call ask?). `Auto` runs silently · `SensitivePathOnly` asks only when a sensitive path (`.env*`, keys, `.git/config`, certs…) fires · `Always` asks every time · `Destructive` asks every time **and the card shows the resolved absolute path**. Phase 6 **session grants** landed with Phase 1: scoped by tool class + canonical path prefix, **source-scoped** (a connector grant never covers a desktop call), **never** grantable for Destructive or sensitive paths, kill-switch revokes all + pauses.

### 3.3 The desktop control center (React + TS + shadcn/ui + Tailwind + HeroUI + xterm)

A workbench shell: left **icon rail** + persistent **read-only command terminal** (xterm.js), active view on the right, global approval/grant/failover banners on top, status-bar heartbeat below.

| View | What it does |
|---|---|
| **Live activity trace** (`TraceView`) | Real-time step tree of every web-AI action, cross-checked against the fs watcher (`✓ saved` only when disk confirms), headroom collapsing, expand-on-demand |
| **Git** (`GitView`) | Full workflow: status, diff, stage/unstage, branches + checkout, history, **commit from the app** |
| **Handoff** (`HandoffView`) | Editable objective, honest progress/files/errors from real trace stats, **"Continue in your web AI" → builds the handoff and copies it to the clipboard** (the connector cannot push into a chat) |
| **Project memory** (`MemoryView`) | Scan & extract facts; objective, decisions, attempts, constraints, changed files, progress; archived-session browser |
| **Web-AI connector** (`BridgeView`) | **MCP endpoint + bound workspace + the live "Allow writes & commands" switch**, tool sandbox against real paths, audit trail |
| Banners / chrome | `ApprovalBanner` (Allow/Deny + grant checkbox), `GrantsBar` (kill switch), `FailoverBanner` (now in-app delivery), `TerminalPane`, `Titlebar`, `Statusbar` (`connector · ro` / `rw` / `offline`), `ProjectDialog` ("Project & connector"), `WorkbenchRail`, `ViewShell` |

### 3.4 Capture / memory / failover (the product thesis)

- **F2 + F3 are complete:** every Claude Code transcript for the monitored project is mirrored into SQLite (archive) and distilled into facts (objective, decisions, failed attempts, constraints, changed files, heuristic progress) that ride into every handoff.
- **Handoffs carry the *why*:** decisions / "do not retry" failed attempts / constraints are explicit blocks in the prompt text.
- **Automatic failover is code-complete** (`failover.rs` + `FailoverBanner`): it detects a stalled local agent or an idle web AI and **offers** continuation in-app. It no longer auto-delivers, because MCP has no push channel.

---

## 4. Roadmap — two different "phases" (don't mix them up)

The repo tracks **two parallel roadmaps**, and both call things "Phase n":

### 4.A Product roadmap (`docs/full-plan.md`) — the *project's* stages

> `full-plan.md` is a **historical record** now; its extension-era design is preserved as written, with a pointer note at the top.

| Phase | Theme | Status |
|---|---|---|
| **0** | Scaffold (Tauri shell, Rust core, SQLite, CI, compression skeleton) | ✅ Done |
| **1** | **MVP — prove web-AI-as-coding-agent** | 🔶 Code-complete & machine-verified; **live M1 gate + exit gate remain** |
| **2** | More web AIs + compression (Layer 3 service) | 🔶 Partial — multi-provider reach now comes from MCP itself; **compression service still a stub** |
| **3** | Automatic failover | 🔶 Code-complete; delivery downgraded from auto-push to in-app offer (see §7) |
| **4** | Orchestration (route work across local + multiple web AIs) | ⬜ Not started |
| **5** | Team & Enterprise (shared state, RBAC, audit, self-host) | ⬜ Not started |

### 4.B Tool-surface roadmap (`docs/tool-roadmap.md`) — the *tools* the web AI sees

A 58-tool plan in phases 0–10. **15 of 58 built.** Phases 8–10 and a **Claude Code parity map** were added on 2026-09-10.

| Phase | Theme | Built | → total | Status |
|---|---|---|---|---|
| (orig) | original MVP tools | 5 | 5 | ✅ done |
| 0 | registry + progressive disclosure | +2 | 7 | ✅ done |
| 1 | files & editing (8 tools) | +8 | **15** | ✅ done |
| 2 | **search (`grep`, `glob`)** — both must filter `is_sensitive_path()` | 0 | 17 | ⏳ not started |
| 3 | git (10 tools) | 0 | 27 | ⏳ not started — *the "cheapest" phase: `git.rs` already has every function* |
| 4 | project memory (10 — incl. the **`get_handoff` pull tool**) | 0 | 37 | 🔶 partial — 6 of 10 have built backing |
| 5 | background commands (3) | 0 | 40 | ⏳ not started |
| 6 | approval policy engine (0 tools) | — | 40 | 🔶 partial — session grants **+ the read-only gate** landed; persistence/config/expiry remain |
| 7 | web & long tail (`web_fetch` [SSRF guard], `web_search`, `notebook_*`, `delegate_task`) | 0 | 45 | ⏳ not started |
| 8 | **code intelligence (LSP)** — diagnostics/definition/references/symbols | 0 | 49 | ⏳ not started — **highest-value gap** |
| 9 | **the agent loop** (`ask_user`, `propose_plan`, `monitor`, `notify`) | 0 | 53 | ⏳ not started — the phase that makes autonomy *supervised* |
| 10 | **isolation & delivery** (`enter/exit_worktree`, `read_media`, `publish_artifact`, `report_findings`) | 0 | 58 | ⏳ not started |

> Of the ~43 tools still to build, roughly **18 are wiring over code that already exists** — all of Phase 3, 6 of 10 in Phase 4, plus thin file ops.
>
> **MCP surface beyond tools (unused):** the connector currently exposes only `tools/*`. **Resources** (workspace files, live git status) and a **`continue_work` prompt** (the handoff, delivered natively) are both natural fits — the prompt is arguably a better answer to the pull-based handoff gap than a tool is.

---

## 5. Feature-level scorecard (F-numbers from `docs/full-plan.md`)

| Feature | Status |
|---|---|
| F1 PTY session capture | 🔶 **Superseded** — the app doesn't host Claude Code; state comes from `transcript.rs` |
| F2 Session archive · F3 Fact extraction · F4 Grounding signals | ✅ Built |
| F5 `read_file` · F6 `write_file` · F7 `run_command` · F9 tool-result relay | ✅ Built |
| F8 extra read/repo ops (`list_directory`, `git_status`, `git_diff`) | 🔶 Partial — `list_directory` + `git_status` built; `git_diff` & friends planned in tool Phase 3 |
| F10 Snapshot compression (Layer 3) | ⬜ Not built (`/compress` is a 501 stub) |
| F11 Handoff prompt · F12 Handoff card | ✅ Built |
| F13–F15 Live activity trace | ✅ Built |
| F16–F18 Outcome-space UX | 🔶 Built in `TraceView`; live UX not yet validated |
| F19 Git panel · F20 Commit from app · F21 Single read-only command terminal | ✅ Built |
| F22 Unified chronological timeline | 🔶 Partial (trace + terminal + git correlate, but no single merged view) |
| F23 Manual interruption → handoff card | ✅ Built |
| F24 Continue on a web AI | 🔶 **Changed** — handoff is built and copied to the clipboard; the web AI attaches through the MCP connector. The old "continue with ChatGPT/Gemini/Grok" push targets are gone with the extension |
| F25 Automatic failover | 🔶 Code-complete; delivery is now an **in-app offer**, not an auto-push |
| F26 Explicit per-tool permissions · F28 Secrets protection · F30 Audit logs · F31 Command approval | ✅ Built |
| F27 Sandboxing | 🔶 Partial — path containment + PTY isolation; no full OS sandbox; `run_command` itself isn't path-gated (why Phase-1 edit tools exist) |
| F29 Encryption / encrypted local storage | ⬜ Not started (SQLite is local-first but plaintext) |
| F32–F35 Team / enterprise | ⬜ Not started (product Phase 5) |

---

## 6. Verification state — **as actually run** (2026-09-10)

| Check | Result | Notes |
|---|---|---|
| `cargo check --lib` | ✅ clean | |
| `cargo fmt --all --check` | ✅ **clean** | was **red** on 2026-09-09; the `ws.rs` fmt-diff site is gone |
| `cargo clippy --lib --all-targets -- -D warnings` | ✅ **exit 0, no warnings** | was **red** on 2026-09-09 (`result_large_err` at `ws.rs:354`); that file no longer exists |
| `pnpm typecheck` (`tsc --noEmit`) | ✅ clean | |
| `pnpm lint` | ⚠️ 0 errors, 6 pre-existing warnings | |
| `pnpm build` | ✅ succeeded | |
| **`cargo test --lib`** | ⚠️ **NOT RUN** | Deliberate: the user's standing instruction for this pass was *do not run tests*. Last green **62 passed / 0 failed** on 2026-09-09, *before* this change. The change deleted `ws.rs` (and its tests) and altered `mcp.rs` config, so the suite's composition changed even though the policy engine did not. **Running it is the top follow-up.** |
| `node scripts/check-spec-sync.mjs` | ➖ **n/a** | The script and the `extension/tool-spec.js` registry it guarded were both deleted; the invariant moved to the `bridge.rs`/`mcp.rs` unit tests. |

**CI (`.github/workflows/ci.yml`):** frontend lint/typecheck/build · `cargo fmt --all --check` · `cargo clippy -- -D warnings` · compression `/health`. A second workflow runs a **Claude PR review** on every PR via `agentrouter.org` (deepseek-v4-flash), gated on the `THIRD_PARTY_API_KEY` secret — its review prompt was updated on 2026-09-10 to describe the new single-registry + read-only-gate invariants instead of the deleted extension registries.

> **CI gap:** `cargo test` is **not** in CI, and this change deliberately did **not** add it — pushing an unverified gate to `main` would be worse than the existing gap. Add it once the suite is confirmed green.

---

## 7. Open issues & known gaps (the honest list)

1. **The extension removal cost one real capability: push delivery.** MCP cannot push a message into a chat, so (a) the Handoff view copies to the clipboard instead of injecting, and (b) failover now *offers* continuation in-app instead of auto-delivering. The fix is a **`get_handoff` pull tool** (tool-roadmap Phase 4) or, better, a **`continue_work` MCP prompt** — neither is built. The one structural obstacle: the bridge executor's `(tool, source, root)` signature has no `AppState`, which `build_handoff_impl()` needs.
2. **`cargo test --lib` is unverified after this change** — the top follow-up, and the reason `cargo test` still isn't a CI gate.
3. **Live validation is the real gap.** Everything above is machine-verified; nothing has been proven end-to-end with a real web-AI session through the connector. Startup validation = **M1 live gate**, then the **exit gate with 5–10 real developers** (metric: *successful continuation rate*).
4. **Provider connector coverage is now the platform risk** (it replaced DOM scraping as risk #1). Gemini has no broad consumer MCP; ChatGPT/Codex varies; Claude and Grok depend on plan and rollout. Mitigated by the connector being provider-native (no selectors to break) and read-only-first — but Lexsus now reaches fewer providers out of the box than the extension did.
5. **Compression service is a stub** (Layer 3, `/compress` 501) — the handoff today is uncompressed structured facts.
6. **Partial rebrand — deliberately untouched this pass.** Repo/README/UI say **Lexsus**; the crate name, package name, tauri identifier (`com.aicb.bridge`) and `productName: "ai-continuity-bridge"` still say AI Continuity Bridge. Cosmetic but visible.
7. **Package-manager drift:** repo is on pnpm (`pnpm-lock.yaml`, docs say pnpm) yet `package-lock.json` still exists and CI uses `npm ci` / `npm run`. Clean one direction or the other. (Pre-existing; not touched here.)
8. **Phase 6 remainder:** persisted grant policies, per-tool configuration, grant expiry (grants are in-memory and die with the app).
9. **F29 encryption not started; F22 unified timeline only partial.**
10. **No automated test coverage for the connector's live path** — `mcp.rs`'s tests cover surface gating and parsing, but nothing exercises a real MCP client against the loopback server in CI.
11. **The per-call cancel path is now vestigial (found during this pass).** `process::set_execution_owner` is only ever called from tests, so `execution_owner()` is always `None`, every `ProcessEntry.owner` is `None`, and the `cancel_request` Tauri command (still registered in `invoke_handler!`) can match no process. Its owner used to be the WebSocket request id that `ws.rs` set — the transport that populated it is gone. **No UI calls `cancel_request`, so nothing is broken for a user today**, but the capability (stop a running `run_command`) is unreachable. Either re-wire the owner to the MCP request id or delete the machinery; do not leave it in the half state.

---

## 8. Startup / validation context

- There is a startup-validation doc: `requirements/AI_Continuity_Bridge_Startup_Validation.pdf`.
- **MVP success criterion:** *a real interrupted coding task is continued by a web AI that genuinely reads, writes, and runs commands on the local project — without the developer re-explaining.* Validated with 5–10 developers.
- **Primary metric:** successful continuation rate. Secondary: weekly retained devs, handoffs/user/week, tool usage, user-reported trust.
- **Risk posture:** the highest risks are now provider connector availability (mitigated by using the provider's native tool channel rather than DOM scraping, plus a read-only-first default) and command-execution safety (mitigated by per-tool approval, grants, destructive-path cards, audit log, sensitive-path filtering).
- **Near-term next actions, in order:** (1) **run `cargo test --lib`** and record the result; (2) run the **M1 live gate** through the MCP connector (runbook: `docs/connector-native-proof-runbook.md`); (3) build **`get_handoff`** / the `continue_work` prompt to restore pull-based continuity; (4) **Phase 2 `grep`/`glob`** and **Phase 8 LSP diagnostics** — the two highest-value tool gaps; (5) real **Layer 3 compression**; then the 5–10-dev validation.

---

## 9. Repo map & reading order

```
├── src/                React/TS control center (views/ + components/)
├── src-tauri/          Rust core (bridge, mcp, db, git, pty, process, watcher, facts, archive, transcript, failover, shell)
│   └── src/mcp.rs      the connector — rmcp Streamable HTTP on 127.0.0.1:45147/mcp
├── compression-service/  Python FastAPI Layer-3 (stub)
├── docs/               full-plan · tool-roadmap · protocol-v2 · architecture · tech-stack · ui-design · connector-native-proof-runbook
├── requirements/       product-requirements · mvp-scope · trade-offs · startup-validation PDF
├── ongoing/            work logs: phase-0 · phase-1-mvp · facts-and-archive · windows runbook · 2026-09-10-mcp-native-architecture
└── .github/workflows/  ci.yml (lint/typecheck/build, fmt/clippy, service health) · claude-review.yml
```

Reading order for someone new: `docs/architecture.md` → `docs/tool-roadmap.md` → `docs/protocol-v2.md` → `ongoing/2026-09-10-mcp-native-architecture.md` → this file.

---

*Status compiled 2026-09-10 on `developing` from a working tree carrying the extension-removal change, plus the verification runs in §6. Feature claims are cross-checked against code (`bridge.rs` SPECS, `mcp.rs` surface lists); where the docs and code disagreed, the code won. The one thing this document cannot vouch for is the Rust test suite — it was not run.*
