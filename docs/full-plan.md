# AI Continuity Bridge — Full Project Plan

> **Historical record.** This document captures the plan as it stood when the
> project was scaffolded. The browser-extension / loopback-WebSocket connection
> path it describes was later removed in favour of a native MCP connector (see
> `docs/architecture.md` and `docs/protocol-v2.md`). The phase history below is
> preserved as-is; read it for provenance, not for current architecture.

> **"Your AI can change. Your work doesn't."**

> **Current model (single-terminal):** the app hosts exactly one terminal — the read-only live view of the web AI's `run_command` executions. The developer runs their local agent (Claude Code) in their **own** terminal; the app does not embed or mirror it. Sections below that describe an embedded interactive terminal / Claude Code pane / PTY session capture refer to the earlier design and are superseded by this.

A local-first desktop application that connects your local coding work to **web AIs** (ChatGPT, Claude.ai, Gemini). When your local coding agent — like Claude Code — hits its usage limit, crashes, or you simply want to switch, the bridge captures the real state of your work and hands the task to a **web AI**, turning it into a real coding agent that can **read your files, write files, and run terminal commands** on your local machine.

---

## Table of Contents

1. [Vision & Positioning](#1-vision--positioning)
2. [Problem & Core Insight](#2-problem--core-insight)
3. [Product Overview](#3-product-overview)
4. [Architecture](#4-architecture)
5. [Tech Stack](#5-tech-stack)
6. [Full Feature List](#6-full-feature-list)
7. [End-User Experience](#7-end-user-experience)
8. [Web AI Integration](#8-web-ai-integration)
9. [Security Model](#9-security-model)
10. [Business Model & Market](#10-business-model--market)
11. [Risk Register](#11-risk-register)
12. [MVP Scope & Success Criteria](#12-mvp-scope--success-criteria)
13. [Phased Development Roadmap](#13-phased-development-roadmap)
14. [Milestones & Definition of Done](#14-milestones--definition-of-done)
15. [Validation Metrics](#15-validation-metrics)
16. [FAQ & Objections](#16-faq--objections)

---

## 1. Vision & Positioning

### Core Principle
The AI should be replaceable. The user's work should not be.

### What We Build
A desktop app that sits between your local coding work and the **web AIs** you use in your browser (ChatGPT, Claude.ai, Gemini). It captures the real state of your project while you work locally, and when your local agent is interrupted, it **connects your PC to a web AI** and turns that web AI into a **working coding agent** on your machine — with real read, write, and terminal access.

### Strong Positioning
> "Hit your limit in Claude Code? Continue on ChatGPT — as a real coding agent on your machine, without losing project state."

### Weak Positioning (What We Avoid)
> "Copy your chat history between websites." — passive, no real agent capability, and easy to copy.

### The Distinct Identity
The product is **not another coding agent**. It is the **neutral bridge** that:
- Lets any web AI operate on your local machine (read/write/run).
- Preserves continuity when your local agent is interrupted.
- Shows you exactly what the web AI is doing — which file it read, which it wrote, which command it ran.

### Why the Desktop App Is the Control Center
The desktop app is not just a relay — it is the **single place where you see and control everything** about your work:

- **Live activity trace** — every command run, every file read/written, every test result, in one clear timeline.
- **Full git workflow** — status, staged changes, diffs, branches, and history all visible; **you can commit directly from the app**.
- **Interactive terminal pane** — see live command output and type into it directly.
- **Real project state** — grounded in your actual filesystem, git, and terminal, not an AI's self-report.
- **Everything local** — nothing leaves your machine; all state lives in local SQLite.

The web AI works through the bridge; **you watch and control it from the desktop control center.**

### Tagline
> **Your AI can change. Your work doesn't.**

---

## 2. Problem & Core Insight

### The Problem
You work on a real coding task using **Claude Code** locally on your laptop. It reads your repo, edits files, runs commands, and makes real progress. Then, mid-task, it **stops**:

- You hit your **usage limit** (out of tokens).
- It runs out of **context**.
- It **crashes** or hangs.
- The provider has an **outage**.
- You simply want to **switch** to a different AI.

Now the working context — what was being attempted, what was completed, why decisions were made, which files changed, what failed, what remains — is **locked inside that local session**.

To continue, you would normally have to either:
1. Re-explain the entire project to another tool, or
2. Paste chunks of context into a web AI, manually feed it your files, then manually copy its edits back into your project.

The web AIs (ChatGPT, Claude.ai, Gemini) are extremely capable, but they **cannot touch your local machine**. They cannot read your actual files, write changes, or run your terminal. So they can't truly "take over" a coding task. The valuable part of the work — the state, the progress, the context — is stranded.

> **Problem statement:** AI-assisted work can be interrupted because its context and execution are tied too closely to one agent, and web AIs cannot act on your local machine.

### The Core Insight
The **local project and task state** should be the source of truth — not any single AI's memory. A web AI should be able to act as a **real coding agent** on your machine, with read, write, and terminal access, while you keep using the familiar web chat interface you already trust.

> **Pattern:** `YOU + Claude Code (local)` → **bridge** → `WEB AI (ChatGPT / Claude.ai / Gemini) acting as a coding agent`

The web AI becomes an **interchangeable worker**. The user cares about the task, not which AI performs it at any moment.

### Why Local-First Matters
The local project is more trustworthy than an AI's self-reported state. The bridge can inspect files, Git diffs, terminal output, build status, and tests. This lets it transfer **actual work state** rather than copying a conversation — and it lets a web AI operate on **real** files and commands, not a simulated environment.

---

## 3. Product Overview

### What It Is
A local-first desktop app that:
1. **Watches and captures** the real state of your coding work (files, git, terminal) while you use Claude Code locally.
2. When your local session is **interrupted** (limit, crash, context, or choice), builds a **handoff** from real project state.
3. **Connects your local PC to a web AI** (ChatGPT, Claude.ai, Gemini) and turns it into a coding agent that can **read files, write files, and run terminal commands** on your machine.
4. **Shows the web AI's activity live** — which file it read, which it wrote, which command it ran — like a coding agent's activity trace, grounded to real operations on disk.

### What It Is Not
- Not a "limit bypass" tool.
- Not a chat-history copier.
- Not browser automation that scrapes web pages.
- Not a local coding agent itself.

### Triggers (the entry points)
- Usage limit reached on the local agent.
- Context limit / long-running task.
- Crash or provider outage.
- Wanting to switch models / providers.
- Privacy needs (keep some work on a specific model).
- Cost optimization.

Limits are the **entry point**, not the foundation. The deeper value is turning web AIs into working coding agents and preserving continuity.

---

## 4. Architecture

The system has two bridges: one captures local work, the other lets a web AI act on the local machine.

### The Four Layers (State Capture → Delivery)

| Layer | Purpose | What it stores / does |
|-------|---------|-----------------------|
| 1. **Session Archive** | Raw capture | Claude Code session stdin/stdout, timestamps, exit codes; project file/git/terminal events |
| 2. **Structured Project Memory** | Facts, not chat | Objective, decisions, failed attempts, constraints, changed files, progress |
| 3. **Context Compression** | Make state handoff-sized | LLM-summarized snapshot sized for a fresh web AI's context window |
| 4. **Handoff Engine** | Translate + deliver | Formats Layer 3 into a web-AI prompt and establishes the coding-agent bridge |

### Bridge A — Local Agent Capture
- Claude Code runs locally. The bridge **wraps/observes** it via a PTY to capture what it reads, writes, and runs.
- Every event streams to Layer 1 (Session Archive), cross-checked against the filesystem watcher and Git to confirm real changes (Layer 2).

### Bridge B — Web AI Coding-Agent Bridge (the core)
- A **browser extension** communicates with the desktop app over a local channel.
- The bridge gives the web AI **tools** it can call:
  - `read_file(path)`
  - `write_file(path, content)`
  - `run_command(cmd)` — executed in the local terminal/PTY
  - `list_directory(path)`
  - `git_status()` / `git_diff()`
- Tool calls are executed **locally** by the Rust core, with permission checks. Results are injected back into the web chat via the extension.

### Activity Trace
- Every tool call the web AI makes is recorded and cross-checked against the filesystem watcher — so the trace shows what **actually happened** on disk, not what the AI merely claimed.

### Data Flow
1. Claude Code works on the project → bridge captures state (Layer 1, cross-checked to Layer 2).
2. On interruption, Layer 3 compresses the state into a handoff.
3. Layer 4 formats it for the chosen web AI and, via the extension, starts a session with the handoff as opening context.
4. The web AI acts as a coding agent through local tool execution, with every action shown in the live activity trace.

### The Technical Opportunity
Different web AIs have different context limits, tool-format expectations, and behaviors. A simple conversation copy is not enough. The opportunity is a **translation + execution layer** that converts your local state into a useful task representation a web AI can act on, grounded against the real local project.

---

## 5. Tech Stack

### Application Shell — Tauri
- **Tauri** with a **Rust core** and **React / TypeScript** frontend.
- Chosen over Electron: smaller binary, native OS-level access without bundling Chromium + Node, and a smaller attack surface — critical since the app executes commands and reads source on the user's machine.

### System / OS Layer — Rust Only
| Concern | Technology |
|---------|------------|
| File watching | OS-native watchers (fsevents / inotify / ReadDirectoryChangesW) |
| Git operations | `git2` (libgit2) — diffs, status, staging, branch, commit history, **commit from the app** |
| Terminal / command exec | `portable-pty` — run commands, observe output, and **interact with the live terminal pane** |
| SQLite access | `rusqlite` |

The git panel (full workflow incl. commit) and the command terminal are powered directly by `git2` and `portable-pty` — no external git/terminal process needed.

### Local State — SQLite
Session archive (Layer 1) and structured memory (Layer 2) live in embedded SQLite — zero ops, fully local, no server dependency.

### Compression — Python + FastAPI
A local microservice for LLM-based context compression (Layer 3), isolated from the Rust core, called over localhost.

### Local Agent Capture — CLI Wrapping (PTY)
Claude Code (the local source agent) is run by the developer in their own terminal; project state for the handoff is gathered on demand from git, the filesystem watcher, and the web AI's own tool activity.

### Web AI Bridge — Browser Extension + Local IPC
- A **browser extension** (Chrome/Firefox) talks to the desktop app over a localhost/local channel (native messaging or WebSocket).
- The extension injects the handoff into the web chat and relays **tool calls** between the web AI and the local Rust core.

### Why Not C for the OS Layer
Rust provides C-level OS control with memory safety. Given the app executes commands and reads source, the security cost of C (buffer overflows, manual memory management, FFI complexity) outweighs any gain. **One systems language, not two.**

### Frontend Conventions
- React + TypeScript + Vite (Tauri default template).
- All UI state derives from events pushed by the Rust core over Tauri's IPC.

---

## 6. Full Feature List

### Local Capture (Claude Code)
- **F1 — PTY Session Capture:** Record Claude Code's stdin/stdout, timestamps, exit codes.
- **F2 — Session Archive:** Store raw sessions locally (Layer 1).
- **F3 — Fact Extraction:** Derive objective, decisions, failed attempts, constraints, changed files.
- **F4 — Grounding Signals:** Cross-check Git diffs, file mtimes, terminal output, test/build status.

### Web AI Coding-Agent Tools
- **F5 — read_file:** Web AI reads a local file.
- **F6 — write_file:** Web AI writes/edits a local file.
- **F7 — run_command:** Web AI runs a terminal command locally.
- **F8 — list_directory / git_status / git_diff:** Additional read and repo operations.
- **F9 — Tool-result relay:** Results of local tool calls returned to the web AI via the extension.

### Context & Handoff
- **F10 — Snapshot Compression:** Compress structured state into a handoff-sized summary (Layer 3).
- **F11 — Handoff Prompt:** Format compressed state for the chosen web AI (Layer 4).
- **F12 — Handoff Card:** "Claude Code interrupted — 64% progress — 8 files changed — 3 errors remaining" with "Continue with ChatGPT / Claude.ai / Gemini."

### UI — Live Activity Trace
- **F13 — Step Tree:** Show every action as a live, collapsible tree: which file read, which written, which command ran.
- **F14 — Cross-Correlation:** Only show "wrote a file" when both the reported action *and* the filesystem watcher confirm it.
- **F15 — Headroom Collapsing:** Latest 2–3 steps expanded; older collapse to one summary line.
- **F16 — Reserved Outcome Space:** Current action and handoff summary always visible.
- **F17 — Auto-Promotion:** On interruption, step data collapses into the handoff card.
- **F18 — Expand-on-Demand:** Click a collapsed group to re-expand.

### Desktop Control Center
- **F19 — Git Panel (full workflow):** View status, staged/unstaged changes, diffs, branches, and commit history via `git2`; stage/unstage files.
- **F20 — Commit from App:** Write a commit message and commit directly from the desktop UI, without touching the terminal.
- **F21 — Single Command Terminal:** see every command the web AI runs, streamed live (command header, output, exit status). Read-only — the developer runs their own terminal outside the app.
- **F22 — Unified Timeline:** The activity trace, terminal output, and git events are all correlated in one chronological view.

### Continuity Controls
- **F23 — Manual Interruption:** Explicit trigger → handoff card.
- **F24 — Continue on Web AI:** Choose ChatGPT / Claude.ai / Gemini and connect.
- **F25 — Automatic Failover (later):** Detect interruption and offer to connect a web AI.

### Security
- **F26 — Explicit Permissions:** Granular read/write/command/network permissions per tool.
- **F27 — Sandboxing:** Isolate command execution.
- **F28 — Secrets Protection:** Protect API keys, env vars, SSH config.
- **F29 — Encryption & Local Storage:** Encrypted local storage.
- **F30 — Audit Logs:** Record every read/write/command for review.
- **F31 — Command Approval (optional):** Prompt before sensitive commands.

### Team / Enterprise (later)
- **F32 — Shared State:** Shared project state and handoffs for teams.
- **F33 — Roles & Permissions:** SSO, RBAC.
- **F34 — Centralized Policies:** Organization-wide tool/command policies.
- **F35 — Self-Hosting:** Local/self-hosted deployment.

---

## 7. End-User Experience

The desktop app is the **control center** — one place where you see everything happening to your work, control your git, use your terminal, and hand off to a web AI. Here is the full experience, screen by screen.

### 7.1 Installation
- Install the desktop app (Tauri installer per OS: `.deb`/`.AppImage` on Linux, `.dmg` on macOS, `.msi`/`.exe` on Windows).
- Install the **browser extension** (Chrome/Firefox) and pair it with the desktop app.
- System requirements: a supported desktop OS; CLI agents (Claude Code, optionally Codex CLI) installed; enough disk for local archives. No Node/Rust toolchain needed by the end user.

### 7.2 Onboarding (screen by screen)
1. **Welcome** — "Your AI can change. Your work doesn't." One-line explanation, **Get Started** / **Skip Intro**.
2. **Privacy promise** — plain language: everything stays local by default; you approve tool access. **Continue** / **View Security Details**.
3. **Select project** — folder picker; shows detected VCS (Git) for confirmation.
4. **Detect local agents** — detects Claude Code (and optionally Codex CLI) with versions; enable/disable each.
5. **Permissions** — granular toggles: file read, file write, command execution, git operations, network for compression, extension pairing.
6. **Pair extension** — the browser extension connects to the desktop app over a local channel. Confirmation screen.
7. **Dashboard (done)** — you land on the control center with a "Start session" call to action.

### 7.3 The Control Center (Main Dashboard)
This is the heart of the app. Layout:

```
┌────────────────────────────────────────────────────────────┐
│  SIDEBAR      │  MAIN PANEL                                │
│               │  ┌──────────────────────────────────────┐  │
│  Projects     │  │  LIVE ACTIVITY TRACE                 │  │
│   • myapp  ▸  │  │  ▾ Refactor auth (Claude Code)       │  │
│               │  │      Reading auth.ts                 │  │
│  Sessions     │  │      ✏ Editing auth.ts [3 lines]     │  │
│   • #12 ▸     │  │      ▾ Running: npm test             │  │
│   • #11 ▸     │  └──────────────────────────────────────┘  │
│               │  ┌──────────────────────────────────────┐  │
│  Git          │  │  TERMINAL PANE                       │  │
│   • status    │  │  $ npm test                          │  │
│   • diff      │  │  PASS auth.test.ts                   │  │
│   • branch    │  └──────────────────────────────────────┘  │
│   • commit    │                                            │
│               │  STATUS BAR: ● Claude Code · running · 64% │
└───────────────┴────────────────────────────────────────────┘
```

- **Sidebar:** Projects, Sessions, Git.
- **Live Activity Trace:** real-time tree of every action.
- **Terminal Pane:** interactive — see live output and type.
- **Status Bar:** current agent, session state, watcher status, compression-service status.

### 7.4 Working in Claude Code Locally
- You launch Claude Code through the bridge (or the bridge observes it).
- Every action appears live in the **activity trace**: files read, files written, commands run, tests passing/failing.
- Each entry is cross-validated against the filesystem watcher — you only see "wrote auth.ts" when the file actually changed on disk.
- The **terminal pane** shows the real command output, and you can type into it if you need to intervene.

### 7.5 The Git Panel (full git workflow from the app)
You control your entire git workflow without ever opening a terminal:

- **Status:** working-tree changes, staged vs unstaged.
- **Diff:** see exactly what changed in each file (inline, colored).
- **Stage/Unstage:** stage files or hunks with a click.
- **Branch:** view branches, switch branches, see current branch.
- **History:** browse commit history, view past commits/diffs.
- **Commit from the app:**
  - Write a commit message in the message box.
  - See a live preview of what will be committed (staged changes).
  - Click **Commit** — committed via `git2` locally. Optional: push after commit.

### 7.6 The Single Command Terminal
- See **live command output** for anything the web AI runs via `run_command`: a `$ command` header, output as it arrives, then exit status.
- **Read-only** — there is no embedded shell; you run your own terminal (Claude Code, etc.) outside the app.
- Command output is correlated into the unified timeline with the activity trace.

### 7.7 Interruption → Handoff Card
- Claude Code hits its limit (or crashes, or you stop it).
- The bridge shows a **handoff card** built from real state:
  > "Claude Code interrupted — 64% progress — 8 files changed — 3 errors remaining — objective: implement auth — next step: build login form."
- Options: **Continue with ChatGPT / Claude.ai / Gemini**, **Edit context**, **Commit current work first**, **Discard**.

### 7.8 Connecting a Web AI as a Coding Agent
- You click **"Continue with ChatGPT"** (or Claude.ai, or Gemini).
- The bridge compresses the state and, via the browser extension, opens the web AI with the handoff as context.
- The web AI can now **call tools**: `read_file`, `write_file`, `run_command` — executed locally by the Rust core, results returned into the web chat.
- Back in the control center, the **activity trace updates live**:
  - "ChatGPT read auth.ts"
  - "ChatGPT wrote auth.ts"
  - "ChatGPT ran npm test → 2 failing"
- The **terminal pane** shows the real output of the commands the web AI ran.
- The **git panel** shows the new changes the web AI made — and you can **commit them from the app**.
- You can **approve sensitive commands** if you enabled that permission.
- The web AI continues the task — you do not re-explain the project.

### 7.9 The Web AI's "Thinking" View
- Like ChatGPT's `> thinking` animation, the bridge shows what the web AI is doing *with your machine* — grounding its activity in real file operations and command output, not just prose.
- You always know which file it read, which it wrote, and which command it ran — and the real effects are visible in the trace, terminal, and git panel.

### 7.10 Settings
- **General:** theme, language, launch-at-startup, notifications.
- **Projects:** add/remove monitored folders, per-project settings.
- **Local Agents:** detect/enable/disable Claude Code, Codex CLI.
- **Web AI Integration:** enable ChatGPT / Claude.ai / Gemini, extension pairing.
- **Git:** default branch, commit conventions, auto-push toggle.
- **Privacy & Security:** permissions toggles, command approval, audit log viewer, clear/export data.
- **Compression:** configure LLM provider/model for the compression service.
- **Updates:** check for updates, auto-update toggle.
- **About:** version, licenses, links.

### 7.11 Ongoing-Use Patterns
- **Continuation after crash:** the bridge detects a crashed/hung local session and offers a handoff even without user action.
- **Scheduled/long tasks:** the bridge keeps state so a long task can resume later on a web AI.
- **Commit-as-you-go:** as the web AI or local agent makes changes, you can review and commit from the control center.
- **Multi-AI workflow (later):** plan with one AI, code with another, review with another — each acting on your local machine.

---

## 8. Web AI Integration

The bridge turns a web chat AI into a coding agent through a **browser extension + local IPC**:

1. **Extension injects handoff** into the web chat (ChatGPT/Claude.ai/Gemini) as the opening context.
2. **Tool calls** — the web AI requests to read/write/run — are captured by the extension and forwarded to the desktop app's Rust core over a localhost channel.
3. **The Rust core executes** the tool call locally (with permission checks) and returns the result via the extension into the web chat.
4. **The activity trace** records every call, cross-validated by the filesystem watcher.

### Mechanism Notes
- **Preferred:** extension autofill + local IPC tool relay — stable, user-in-the-loop, keeps you in control of what runs on your machine.
- **Avoided as core:** DOM scraping / full browser automation — fragile (10/10 platform risk). The extension approach keeps the app's core independent of web-UI scraping.

### Tool Execution Policy
- Every tool call is subject to the user's configured permissions.
- Commands flagged sensitive can require explicit approval before execution.
- All tool calls are logged to the audit trail.

---

## 9. Security Model

The app can read source, write files, and run commands. This is powerful and dangerous. Security is a **product requirement**, not an add-on.

- **Explicit per-tool permissions** — read/write/command each gated.
- **Command approval** — optional confirmation for sensitive commands.
- **Sandboxing** — isolate command execution.
- **Secrets protection** — never log or leak keys/credentials.
- **Encryption & local storage** — encrypted archives and memory.
- **Audit logs** — every read/write/command recorded for review.
- **Clear data policies** — documented local-first guarantees.
- **Enterprise-grade** (later): SSO, compliance, centralized policies.

---

## 10. Business Model & Market

### Target Customers
| Segment | Profile |
|---------|---------|
| **Initial** | AI-heavy developers using Claude Code / local agents who hit limits and want to continue on web AIs |
| **Next** | Professional teams needing shared context, handoffs, auditability, model choice |
| **Long-term** | Enterprises requiring self-hosted deployment, SSO, permissions, compliance |

### Pricing Tiers
| Tier | Price | Includes |
|------|-------|----------|
| **Free** | $0 | Basic local continuity, limited web-AI tool access |
| **Pro** | ~$10–$30/mo | Unlimited project memory, multiple web AIs, advanced handoffs, command approval |
| **Team** | ~$20–$50+/user/mo | Shared state, permissions, analytics, audit |
| **Enterprise** | Custom | Self-hosting, security, SSO, compliance, dedicated support |

Prices must be validated through willingness-to-pay tests. The value proposition is **time saved and continuity**, not merely access to another AI.

### Competitive Landscape
Competitive: Claude Code, Codex, Cursor, Cline, Roo Code, and others. The opportunity is **not another agent** — it is the **neutral bridge** that lets web AIs act as coding agents on your machine and preserves continuity across interruptions.

**Biggest competitive threat:** AI providers building increasingly persistent agent environments. Counter: a neutral bridge connecting web AIs to local machines is the strategic opportunity.

---

## 11. Risk Register

| Risk | Severity | Mitigation |
|------|----------|------------|
| **Platform dependency (web UI changes)** | 10/10 | Extension + local IPC tool relay, not DOM scraping |
| **Security / command execution risk** | 9/10 | Per-tool permissions, command approval, sandboxing, audit logs |
| **Limits disappear** | High | Limits are the entry point; continuity + web-AI-as-agent is the value |
| **Technical difficulty (reliable tool relay)** | 7/10 | Stable extension + local IPC; robust permission model |
| **Initial defensibility** | 4/10 | Basic idea copyable; build moat via integrations, security, workflow |
| **Competition** | 8/10 | Neutral-bridge positioning; deep integration |

---

## 12. MVP Scope & Success Criteria

### MVP In Scope
1. Tauri desktop shell — single monitored project.
2. Rust file watcher + git state extraction.
3. PTY observation of **Claude Code** (capture activity).
4. SQLite structured memory — no compression service yet.
5. Live activity trace UI with headroom collapsing.
6. **Interactive terminal pane** — see and interact with live command output.
7. **Git panel (full workflow)** — status, diff, stage/unstage, branch, history, and **commit from the app**.
8. **Browser extension** paired with the desktop app.
9. Manual interruption → handoff card → **"Continue with ChatGPT"** with web AI able to **read files, write files, and run commands** locally.

### MVP Out of Scope
- Compression service (Layer 3).
- Multiple web AIs (start with one, e.g. ChatGPT).
- Team / enterprise features.
- Automatic failover.

### MVP Success Criterion
> A real coding task interrupted in Claude Code is continued by a **web AI** that genuinely reads, writes, and runs commands on the local project — without the developer re-explaining.

Validate with 5–10 real developers.

**Primary metric:** **Successful continuation rate** — of real interrupted tasks, how many a web AI can continue with real tool access, without re-explanation.

---

## 13. Phased Development Roadmap

### Phase 0 — Scaffold
- Tauri shell; Rust core (git2, portable-pty, fs watchers, rusqlite); SQLite schema; compression-service skeleton; CI/lint.

### Phase 1 — MVP: Prove Web-AI-as-Coding-Agent
- PTY observation of Claude Code + live activity trace.
- **Interactive terminal pane** (see + type into live command output).
- **Git panel** (status, diff, stage, branch, history, commit from app).
- Browser extension paired to desktop app.
- Handoff card → **Continue with ChatGPT**.
- Web AI tool access: read_file, write_file, run_command (local execution + result relay).
- **Exit gate:** real interrupted task continued by ChatGPT with real tool access, validated with 5–10 devs.

### Phase 2 — More Web AIs & Compression
- Add Claude.ai and Gemini as web-AI targets.
- Layer 3 compression service.
- More tools (git_status, list_directory) + command approval UX.
- **Exit gate:** multiple web AIs continue tasks with compressed context.

### Phase 3 — Automatic Failover
- Detect local-agent interruption automatically and offer web-AI continuation.
- **Exit gate:** automatic continuation works without re-explanation.
- **Implemented:** `failover.rs` state machines (local: `inactive → working → stalled → interrupted`, vetoed by any file change; web: WS-drop / silence detection), `transcript.rs` Claude Code JSONL reader feeding objective + context + end reason into the handoff, auto-delivery (`auto:true` → the extension inserts and submits the prompt), web→web failover to Claude.ai/Gemini + web→local hand-back, `failover_log` telemetry (successful continuation rate), and a failover panel in the UI.

### Phase 4 — Orchestration
- Route parts of a task across local + multiple web AIs with single project state.
- **Exit gate:** orchestrator routes work while preserving one source of truth.

### Phase 5 — Team & Enterprise
- Shared state, permissions, SSO, audit, compliance, self-hosting.
- **Exit gate:** enterprise security/audit requirements met.

---

## 14. Milestones & Definition of Done

| Milestone | Deliverable | Definition of Done |
|-----------|-------------|--------------------|
| **M0 — Scaffold** | Tauri shell + Rust core + SQLite schema | App launches; watcher & git extraction run; schema migrates |
| **M1 — Capture** | Project state captured from git + fs watcher + web-AI tool activity; activity trace + single command terminal render | Steps shown; file changes cross-validated; web-AI commands stream live |
| **M2 — Handoff** | Interruption → card → ChatGPT connects; git panel live | ChatGPT reads/writes/runs commands locally; changes visible in git panel and committable from app; developer does not re-explain; 5–10 devs validate |
| **M3 — Multi-web-AI** | Claude.ai + Gemini targets; compression live | Multiple web AIs continue with compressed context |
| **M4 — Failover** | Automatic detection + continuation | Auto-continuation works; continuation rate measured |
| **M5 — Orchestration** | Multi-AI routing, single state | Orchestrator routes; state consistent |
| **M6 — Enterprise** | Team/enterprise features | SSO, permissions, audit, self-hosting validated |

---

## 15. Validation Metrics

| Metric | Definition | Target (Phase 1) |
|--------|-----------|------------------|
| **Successful continuation rate** | % of interrupted tasks a web AI continues with real tool access, no re-explanation | High (5–10 devs) |
| **Weekly retained developers** | Week-over-week usage | Positive trend |
| **Handoffs per user** | Avg. web-AI continuations per user/week | Increasing |
| **Average time saved** | Dev-reported time saved | Positive, material |
| **Paid conversion** | Free → Pro | Growing |
| **Web-AI tool usage** | % of sessions using read/write/command | Growing |
| **User-reported trust** | Trust in the bridge's tool execution | High |

**Decision evidence:** The decisive evidence comes from real developers successfully handing off real unfinished tasks to a web AI and choosing to keep the product after the novelty wears off.

---

## 16. FAQ & Objections

**Q: Isn't this just a chat-history copier?**
No. The web AI gets **real tool access** — it reads, writes, and runs commands on your local machine. It becomes a coding agent, not a chat.

**Q: What if providers just raise their limits?**
Limits are the entry point. The value is turning web AIs into working coding agents on your machine and preserving continuity across interruptions.

**Q: Is it safe to let a web AI run commands on my machine?**
Security is core: per-tool permissions, command approval, sandboxing, and audit logs. Nothing runs without your configured consent.

**Q: Why not just use browser automation?**
DOM scraping breaks constantly (10/10 risk). We use a browser extension + local IPC tool relay, which is stable and keeps you in control.

**Q: Does this work with Claude.ai / Gemini too?**
Yes — the architecture targets ChatGPT, Claude.ai, and Gemini. MVP starts with one web AI (ChatGPT) and expands.

**Q: Can it be copied easily?**
The basic idea is copyable (4/10 initial defensibility); the moat is reliable tool relay, security, workflow integration, and handoff quality.

---

## Bottom Line

Build the bridge that lets a **web AI continue your coding work on your local machine** when your local agent is interrupted — with real read/write/terminal access and a live activity trace, so you never re-explain the project.

> **"Your AI can change. Your work doesn't."**
