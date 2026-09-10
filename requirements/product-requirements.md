# Product Requirements

## Vision

A local-first desktop app that lets a developer continue an interrupted AI coding session by connecting their local PC to a **web AI** (ChatGPT, Claude.ai, Gemini). The web AI becomes a real coding agent that can read files, write files, and run terminal commands on the local machine — with the developer not having to re-explain the project.

## Core Value Proposition

"When your local AI session is interrupted, continue on a web AI that actually works on your machine — without re-explaining the project."

## Functional Requirements

### 1. Activity Capture
- FR-1.1: Record every executed web-AI tool call (read_file / write_file / run_command) with timestamps.
- FR-1.2: Store captured activity in embedded SQLite (Layer 1 — Session Archive).
- FR-1.3: Cross-check activity against OS-native file watchers to confirm real file changes.

### 2. Project State Memory
- FR-2.1: Extract structured facts from captured activity: objective, decisions, failed attempts, constraints, changed files.
- FR-2.2: Store structured memory in SQLite (Layer 2 — Structured Project Memory).

### 3. Context Compression
- FR-3.1: Compress structured state into a snapshot sized for a fresh web AI's context window via local LLM microservice (Layer 3).

### 4. Handoff
- FR-4.1: Format compressed state into a web-AI-specific handoff prompt (Layer 4).
- FR-4.2: Deliver the handoff to the chosen web AI (ChatGPT / Claude.ai / Gemini) over the native MCP connector, and surface it in-app (copyable to the clipboard from the Handoff view). A `get_handoff` connector pull tool is the planned replacement for the manual copy.

### 5. Web AI Coding-Agent Tools
- FR-5.1: Let the web AI read local files (`read_file`).
- FR-5.2: Let the web AI write/edit local files (`write_file`).
- FR-5.3: Let the web AI run terminal commands locally (`run_command`).
- FR-5.4: Return tool-call results back to the web AI through the connector's native tool channel.

### 6. Live Activity Trace
- FR-6.1: Render observed actions (reads, writes, commands) as a live, collapsible step tree.
- FR-6.2: Cross-reference reported actions with the filesystem watcher to only report real file changes.

### 7. Desktop Control Center
- FR-7.1: Provide a single read-only terminal pane streaming every web-AI `run_command` live (command, output, exit status).
- FR-7.2: Provide a full git panel (status, diff, stage/unstage, branch, history).
- FR-7.3: Allow committing from the app (via `git2`).

### 8. Interruption & Continuation
- FR-8.1: Support a manual interruption trigger producing a handoff card.
- FR-8.2: Support "Continue with ChatGPT / Claude.ai / Gemini" to connect a web AI as a coding agent.

## Non-Functional Requirements

- NFR-1: **Local-first** — no data leaves the machine by default; SQLite only.
- NFR-2: **Security** — minimal attack surface; one systems language (Rust); per-tool permissions, command approval, audit logs; handle source code, git history, and secrets carefully.
- NFR-3: **Reliability** — interruptions (crashes, provider outages) must not lose captured project state.

## Constraints

- Desktop app must support macOS, Windows, and Linux.
- Native MCP connector for web-AI integration: a desktop-local MCP server bound to loopback, reached by the provider's own connector support (Claude.ai custom connectors first; any MCP-capable host, including Claude Code or Claude Desktop, can point straight at the loopback URL).
- Web-AI targets: ChatGPT, Claude.ai, Gemini.
- Local source agent: Claude Code (optionally Codex CLI) — run by the developer in their own terminal; the app does not host it.
- OS-native file watchers: fsevents / inotify / ReadDirectoryChangesW.
