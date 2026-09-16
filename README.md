<div align="center">

# Lexsus

**Your AI can change. Your work doesn't.**

[![Stars](https://img.shields.io/github/stars/abdulwasea89/lexsus?style=flat-square)](https://github.com/abdulwasea89/lexsus/stargazers)
[![License](https://img.shields.io/github/license/abdulwasea89/lexsus?style=flat-square)](LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/abdulwasea89/lexsus/ci.yml?style=flat-square)](https://github.com/abdulwasea89/lexsus/actions)
[![Rust](https://img.shields.io/badge/Rust-stable-1e3a5f?style=flat-square&logo=rust&logoColor=white)](src-tauri)
[![TypeScript](https://img.shields.io/badge/TypeScript-3178C6?style=flat-square&logo=typescript&logoColor=white)](src)
[![Tauri](https://img.shields.io/badge/Tauri_2-24C8DB?style=flat-square&logo=tauri&logoColor=white)](src-tauri)
[![MCP](https://img.shields.io/badge/MCP-Streamable_HTTP-6E56CF?style=flat-square)](src-tauri/src/mcp.rs)

**[简体中文](README.zh-CN.md) · English**

A local-first Tauri + Rust desktop app that turns an **MCP-capable web AI — Claude.ai, or any MCP host — into a real coding agent on your machine**. When your local agent (like Claude Code) hits its usage limit, crashes, or you just want to switch, Lexsus captures your real project state and hands the work off — so the web AI can **read your files, write files, and run terminal commands**, and you never re-explain the project.

</div>

---

## Contents

- [Key Features](#key-features)
- [How It Works](#how-it-works)
- [Tools at a Glance](#tools-at-a-glance)
- [Quick Start](#quick-start)
- [Usage](#usage)
- [Security Model](#security-model)
- [Status & Roadmap](#status--roadmap)
- [Troubleshooting](#troubleshooting)
- [Repository Layout](#repository-layout)
- [Documentation & Learning Paths](#documentation--learning-paths)
- [Contributing](#contributing)
- [Community & Support](#community--support)
- [License](#license)

## Key Features

|     | Feature                                | What it means for you                                                                                                                                                                                                                                                                                                             |
| --- | -------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 🔌  | **Native MCP connector, not scraping** | Your web AI talks to Lexsus through its **own tool channel** — a desktop-local MCP server on `http://127.0.0.1:45147/mcp`. Native tool UI, native results, no browser extension, no DOM watching, no composer injection.                                                                                                          |
| 🔀  | **Handoff, not copy-paste**            | One click packages the real state of your project — objective, decisions, failed attempts, constraints, changed files — into a prompt any web AI can continue from. Facts, not chat.                                                                                                                                              |
| 🛠️  | **Real coding-agent tools**            | 58 tools executed locally by the Rust core, not simulated in the browser — file reading (chunked) and editing, file management, shell commands (foreground + background), search, full git workflow, project memory, web fetch/search, code intelligence, and agent-loop primitives. See [Tools at a Glance](#tools-at-a-glance). |
| 👁️  | **Live activity trace**                | Every read, write, and command the web AI performs shows up in real time, cross-checked against the filesystem watcher — nothing is claimed without evidence.                                                                                                                                                                     |
| 🛡️  | **Approval gates + session grants**    | Writes, commands, and destructive calls pause for your **Allow / Deny**. Tick "don't ask again" to grant a class of edits for the session; the kill switch revokes every grant and pauses the bridge. Every command streams live into the app's single read-only terminal so you see exactly what runs.                           |
| 🚦  | **Read-only first**                    | The connector exposes read tools only until you flip "Allow writes & commands" — live, from the desktop, no rebuild and no reconnect.                                                                                                                                                                                             |
| 🧠  | **Structured project memory**          | Sessions are archived to embedded SQLite and distilled into objective, decisions, failed attempts, constraints, changed files, and heuristic progress.                                                                                                                                                                            |
| 🔒  | **Local-first by design**              | The connector binds to loopback only, embedded SQLite, and a minimal Tauri surface instead of Electron. Nothing leaves the machine unless you deliberately expose it.                                                                                                                                                             |

## How It Works

One connector, four layers — from raw capture to a delivered handoff:

1. **Capture** — the app records real file, git, and terminal activity into a lossless Session Archive (Layer 1).
2. **Structure** — the archive is distilled into facts, not chat: objective, decisions, failed attempts, constraints, changed files (Layer 2).
3. **Compress** — an optional Python/FastAPI service summarizes the state into a handoff-sized snapshot for a fresh context window (Layer 3).
4. **Deliver** — the Handoff Engine formats it for your chosen web AI, and the native MCP connector gives that AI real tools against the local project (Layer 4).

Full detail in [docs/architecture.md](docs/architecture.md).

## Tools at a Glance

The connector exposes 58 tools (source of truth: `SPECS` in `src-tauri/src/bridge.rs`), grouped by capability:

- **Reading:** `read_file` (chunked, numbered lines), `read_many_files`, `list_directory`, `notebook_read`, `read_media`
- **Editing:** `write_file`, `edit_file`, `multi_edit`, `apply_patch`, `delete_file`, `move_file`, `copy_file`, `create_directory`, `notebook_edit`
- **Commands:** `run_command`, `run_command_background`, `command_output`, `kill_command`
- **Search:** `grep` (regex + glob filters), `glob` (path patterns)
- **Git:** `git_status`, `git_diff`, `git_log`, `git_add`, `git_unstage`, `git_commit`, `git_branches`, `git_checkout`, `git_create_branch`, `git_show`, `git_commit_diff` — powered by `git2`, no external git process, committable from the app
- **Memory:** `todo_write`, `todo_read`, `set_objective`, `remember_decision`, `remember_constraint`, `remember_attempt`, `get_facts`, `list_sessions`, `request_handoff`, `get_handoff`
- **Web:** `web_fetch` (SSRF-guarded), `web_search`
- **Code intelligence:** `lsp_diagnostics`, `lsp_definition`, `lsp_references`, `lsp_symbols`
- **Agent loop:** `delegate_task`, `ask_user`, `propose_plan`, `monitor`, `notify`, `report_findings`
- **Isolation & delivery:** `enter_worktree`, `exit_worktree`, `publish_artifact`
- **Meta:** `list_tools`, `describe_tool` — the AI discovers the surface itself, so a chat can be primed without a handoff

Results are capped at 140,000 characters with explicit truncation markers. The full wire protocol is specified in [docs/protocol-v2.md](docs/protocol-v2.md), and the phased build-out with invariants in [docs/tool-roadmap.md](docs/tool-roadmap.md).

## Quick Start

**Prerequisites:** Node.js 20+, Rust (stable), [pnpm](https://pnpm.io). Python 3.12 is only needed for the optional compression service.

```bash
git clone https://github.com/abdulwasea89/lexsus.git
cd lexsus
pnpm install
pnpm tauri dev
```

When the app starts, the connector binds `http://127.0.0.1:45147/mcp` — the status bar shows `connector · ro` (read-only) and the **Web-AI connector** view shows the exact endpoint and bound workspace.

Connect a web AI:

1. **Claude.ai** — Customize → Connectors → **Add custom connector**, and give it the endpoint. Claude.ai connects from Anthropic's cloud, so in development expose the loopback server through a short-lived HTTPS tunnel (see [docs/connector-native-proof-runbook.md](docs/connector-native-proof-runbook.md)); add that tunnel's host to the DNS-rebinding allowlist (see env vars below).
2. **A local MCP host** (Claude Code, Claude Desktop, the MCP Inspector) — point it straight at `http://127.0.0.1:45147/mcp`. No tunnel needed.

The connector starts **read-only**. Flip **Allow writes & commands** in the Web-AI connector view when you want the write and command tools exposed — every one of them still asks for your approval on the desktop.

| Env var                                            | Purpose                                                                                                      |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| `LEXSUS_MCP_ALLOW_WRITE=1`                         | Seed the connector with writes & commands exposed on launch (still approval-gated).                          |
| `LEXSUS_MCP_ALLOWED_HOSTS=your-tunnel.example.com` | Allowlist additional hosts (e.g. a dev tunnel) for DNS-rebinding protection. Comma-separated, additive only. |

Optional — the LLM context-compression service (Layer 3):

```bash
docker compose up -d            # or run it directly:
pip install -r compression-service/requirements.txt
uvicorn main:app --port 8000 --app-dir compression-service
```

## Usage

### 1. Connect and watch

Open a project in the desktop app and connect your web AI. The **live activity trace** shows every action it takes; each approved `run_command` streams into the single read-only terminal and lands in the git panel where you can commit from the app.

### 2. Hand off, not log out

Hit your local agent's limit? Build a handoff from the app — or let the web AI pull it itself with the `get_handoff` tool. It packages the extracted facts — objective, decisions, failed attempts, constraints, changed files — so the AI inherits _why_, not just _what_, and won't retry dead ends.

> [!NOTE]
> The connector is **pull-based**: MCP has no way to push a message into a chat, so the handoff is surfaced in-app (copy to clipboard) and also retrievable by the AI via `get_handoff`.

### 3. The web AI works like an agent

The AI calls Lexsus tools through its own native tool channel. Underneath, that is an MCP `tools/call`:

```jsonc
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "tools/call",
  "params": {
    "name": "read_file",
    "arguments": { "path": "src/App.tsx", "offset": 401 },
  },
}
```

Large files come back one chunk at a time as numbered lines, with a footer naming the exact call that returns the next chunk — the AI pages through what it needs instead of being handed a whole file it can't absorb. `run_command` output streams back chunk-by-chunk, and the write/run tools block on your **Allow / Deny** on the desktop before touching disk or shell.

## Security Model

- **Loopback-only:** the MCP server binds `127.0.0.1:45147` (see `src-tauri/src/mcp.rs`). Remote hosts are rejected unless explicitly allowlisted via `LEXSUS_MCP_ALLOWED_HOSTS`.
- **Read-only first:** write/command tools stay hidden until you opt in, live, from the desktop.
- **Every mutation is gated:** writes, commands, and destructive calls (`delete_file`, `move_file`, `git_checkout`, …) pause for approval, showing the resolved absolute path.
- **Session grants + kill switch:** "don't ask again" grants a class of edits for the session only; the kill switch revokes all grants and pauses the bridge.
- **Local-first storage:** session archive and project memory live in embedded SQLite. No cloud dependency; the optional tunnel for Claude.ai is short-lived and explicit.

## Status & Roadmap

> [!NOTE]
> **Status:** early-stage MVP — the core bridge works end-to-end (archive, facts, handoff, tool relay, live terminal). The compression service (`/compress`) is still a stub; a [5–10 developer validation](requirements/mvp-scope.md) comes before scaling features.

What's next: hardening the approval/audit surface, completing compression (Layer 3), and expanding the tool surface per [docs/tool-roadmap.md](docs/tool-roadmap.md). Adding a tool? Read the invariants there first: a new tool is registered once in `SPECS` (`src-tauri/src/bridge.rs`) with a matching JSON Schema, and the read-only/write gate must keep hiding it until writes are allowed.

## Troubleshooting

| Symptom                           | Likely cause / fix                                                                                                                                         |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Web AI can't reach the endpoint   | Confirm the app is running and bound to `http://127.0.0.1:45147/mcp`; local hosts need no tunnel, Claude.ai cloud needs the HTTPS tunnel from the runbook. |
| Tunnel host rejected              | Add it to `LEXSUS_MCP_ALLOWED_HOSTS` and restart; the allowlist is additive to loopback.                                                                   |
| Write tools not visible to the AI | Flip **Allow writes & commands** in the Web-AI connector view (or launch with `LEXSUS_MCP_ALLOW_WRITE=1`).                                                 |
| Approval card never resolves      | Check the desktop app is focused — approvals block the tool call until you Allow/Deny.                                                                     |
| `/compress` returns stub data     | Expected: the compression service is still a stub (see Status above).                                                                                      |

## Repository Layout

```
├── src/                    # React + TypeScript frontend (Tauri shell)
├── src-tauri/              # Rust core — git2, portable-pty, notify, rusqlite, native MCP server
│   └── src/
│       ├── mcp.rs          # the connector: rmcp Streamable HTTP on loopback
│       └── bridge.rs       # tool engine: SPECS registry, policy, approvals, audit
├── compression-service/    # Optional Python FastAPI context compression (Layer 3)
├── docs/                   # Architecture, connector protocol, tech stack, UI design, tool roadmap
├── requirements/           # Product requirements, MVP scope, trade-offs
├── ongoing/                # Active work logs and runbooks
├── extension/              # Browser-extension prototype artifacts
├── scripts/                # Dev utilities (spec sync checks, tunnel helpers)
└── public/                 # Static web assets
```

## Documentation & Learning Paths

Reading the docs in this order takes you from "what is this" to "how the wire works":

| #   | Doc                                                                              | Covers                                                                      |
| --- | -------------------------------------------------------------------------------- | --------------------------------------------------------------------------- |
| 1   | [docs/architecture.md](docs/architecture.md)                                     | The connector and the four layers                                           |
| 2   | [docs/tech-stack.md](docs/tech-stack.md)                                         | Tauri + Rust systems stack, security rationale                              |
| 3   | [docs/protocol-v2.md](docs/protocol-v2.md)                                       | Connector protocol, tool schemas, approvals, error codes, sequence diagrams |
| 4   | [docs/ui-design.md](docs/ui-design.md)                                           | The control-center UI: activity trace, terminal, git panel                  |
| 5   | [docs/tool-roadmap.md](docs/tool-roadmap.md)                                     | The tool surface, phase by phase: built vs planned, and the invariants      |
| 6   | [docs/connector-native-proof-runbook.md](docs/connector-native-proof-runbook.md) | Exposing the connector and proving natively with Claude.ai                  |
| 7   | [requirements/product-requirements.md](requirements/product-requirements.md)     | Product & MVP scope, success criteria                                       |
| 8   | [ongoing/facts-and-archive.md](ongoing/facts-and-archive.md)                     | Completed work: session archive (F2) + fact extraction (F3)                 |

## Contributing

Contributions are welcome — the CI already enforces quality on every PR (frontend lint/typecheck/build, `cargo fmt`/`clippy`, compression-service health).

1. **Fork** the repo and create a branch (`git checkout -b feat/your-idea`).
2. **Make your change** — keep it focused, add a test where reasonable.
3. **Open a pull request** — CI runs automatically and must pass.

Check the [issues](https://github.com/abdulwasea89/lexsus/issues) for easy entry points. Note: there's no CONTRIBUTING.md yet — if you'd like to drive its conventions, start a discussion.

## Community & Support

- 💬 Ask questions and propose features in [GitHub Discussions](https://github.com/abdulwasea89/lexsus/discussions).
- 🐛 Report bugs via [issues](https://github.com/abdulwasea89/lexsus/issues).
- ⭐ Enjoy the project? **Star the repo** — it's the fastest way to help the bridge reach more developers.

## License

Released under the [MIT License](LICENSE). Built with [Tauri](https://tauri.app), [React](https://react.dev), the [Rust](https://www.rust-lang.org) ecosystem (`git2`, `portable-pty`, `rusqlite`, `notify`, `rmcp`), and [FastAPI](https://fastapi.tiangolo.com).
