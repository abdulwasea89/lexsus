# Product Architecture

Lexsus connects your local coding work to **MCP-capable web AIs** (Claude.ai first, any MCP host in general). It captures real project state while you work locally, and when your local agent is interrupted, it turns a web AI into a **working coding agent** on your machine — with real read, write, and terminal access.

There is one transport, and four layers behind it.

- **The transport — the native MCP connector.** A desktop-local MCP server (`src-tauri/src/mcp.rs`) bound to loopback on `http://127.0.0.1:45147/mcp`. The web AI calls Lexsus tools through its **own native tool channel**; Lexsus never touches the provider's page.
- **Bridge A — Local Agent Capture.** Gathers real project state for the handoff (git, filesystem watcher, Claude Code transcripts, web-AI tool activity). The developer's own terminal (Claude Code) is not hosted or mirrored by the app.
- **Bridge B — Web AI Coding-Agent Bridge.** The connector above. Its `run_command` output streams live into the app's single read-only terminal.

> **History.** Earlier iterations reached the browser through a Chrome MV3 extension: a loopback WebSocket on `ws://127.0.0.1:45241` with 6-digit pairing, a DOM watcher on the provider page, and composer injection. That path has been **removed entirely** (`ws.rs`, `extension/`, the `tungstenite`/`getrandom` dependencies, and the `pair_*`/`handoff_send`/`failover_deliver` commands are gone). The provider-native MCP connector replaced it: a native tool channel beats DOM scraping, which was always the highest platform risk in this project. See [protocol-v2.md](protocol-v2.md) for the wire detail.

## The Four Layers (State Capture → Delivery)

| Layer | Purpose | What it stores / does |
|-------|---------|-----------------------|
| 1. Session Archive | Raw capture | Executed web-AI tool calls (reads/writes/commands), timestamps; project file/git/terminal events |
| 2. Structured Project Memory | Facts, not chat | Objective, decisions, failed attempts, constraints, changed files, progress |
| 3. Context Compression | Make state handoff-sized | LLM-summarized snapshot sized for a fresh web AI's context window |
| 4. Handoff Engine | Translate + deliver | Formats Layer 3 into a web-AI prompt and establishes the coding-agent bridge |

## Layer Details

### 1. Session Archive

The lowest-level capture. Every executed web-AI tool call (read, write, run) is recorded to SQLite with timestamps alongside filesystem and git events. This is the raw, lossless record of what happened to the project.

### 2. Structured Project Memory

The archive is interpreted into structured facts — the objective, decisions, failed attempts, constraints, changed files, and progress. This is "facts, not chat", cross-checked against the filesystem watcher and Git so it reflects real state, not an AI's self-report.

### 3. Context Compression

The structured state is summarized by an LLM into a snapshot sized to fit a fresh web AI's context window, so a web AI can pick up the work without reading the full session history. The FastAPI service exists but `/compress` is still a stub — today's handoff is formatted from uncompressed structured facts.

### 4. Handoff Engine

The compressed snapshot is formatted into a web-AI-specific handoff prompt. The connector then gives that AI real tools (read/write/run) against the local Rust core and returns results through its native tool channel.

**Delivery is pull-based.** MCP gives a server no way to push a message into a chat, so the old "inject the handoff into the composer" behaviour is gone with the extension. Today the Handoff view builds the text and copies it to your clipboard (failover interruptions additionally surface in-app). A `get_handoff` connector **tool** — so the AI fetches the handoff itself — is the planned replacement and is not built yet.

## Data Flow

1. You work on the project in your own terminal (e.g. Claude Code); the app watches the project folder.
2. On interruption, you open the app and build a handoff from the objective plus git/fs-watcher/trace state (Layer 3 compresses it; the compression service remains future work) and paste it into the web AI's chat.
3. The web AI connects to Lexsus through its native MCP connector and sees the tool surface.
4. The web AI acts as a coding agent through local tool execution (`read_file`, `write_file`, `run_command`). Every `run_command` streams live into the app's single terminal pane; every action is recorded in the live activity trace and grounded against the real local project.

## The Connector

The connector is deliberately thin. Everything that makes Lexsus safe — approvals, sensitive paths, session grants, the kill switch, path containment — lives below it in `bridge.rs`, unchanged and transport-independent. Any caller, whatever its source, goes through the same policy engine.

```
      web AI (Claude.ai, or any MCP host)
                    │  native tool channel
                    ▼
   ┌────────────────────────────────────────────┐
   │ mcp.rs — rmcp Streamable HTTP              │
   │ 127.0.0.1:45147/mcp · loopback only        │
   │ tools/list gated by the write flag         │
   │ tools/call → parse → tool_call("mcp")      │
   └────────────────────────────────────────────┘
                    │
                    ▼
   ┌────────────────────────────────────────────┐
   │ bridge.rs — the policy engine              │
   │ approvals · sensitive paths · grants ·     │
   │ kill switch · containment · audit          │
   └────────────────────────────────────────────┘
                    │
                    ▼
            user-approved local workspace
```

**Read-only first.** `mcp_allow_write` is an in-memory flag, default off. While it is off, `tools/list` simply does not advertise the write and command tools; flipping it hides or re-exposes them at runtime with no rebuild and no reconnect. It is seeded from `LEXSUS_MCP_ALLOW_WRITE` and toggled live from the Web-AI connector view (`mcp_set_allow_write`). The `mcp_status` command reports `{listening, endpoint, allow_write, workspace}` to the UI.

**Loopback, and only loopback, by default.** rmcp's DNS-rebinding guard rejects requests whose `Host` it doesn't recognise, and Lexsus accepts loopback hosts out of the box. Because a cloud-hosted provider connects from *its* infrastructure rather than your machine, reaching it requires an explicit HTTPS tunnel — and that tunnel's host must be opted in via `LEXSUS_MCP_ALLOWED_HOSTS` (comma-separated). Nothing beyond loopback is ever hardcoded.

A free byproduct of binding to loopback: any local MCP host — Claude Code, Claude Desktop, the MCP Inspector — can point at the same endpoint with no tunnel at all.

**The desktop is the only approval authority.** No provider exposes an approval primitive we can rely on, so gated calls block inside `tools/call` while the desktop banner decides, for at most 120 s — comfortably under provider connector timeouts. Results are capped at 140,000 characters so a connector result can't blow up a chat's context.

## The Technical Opportunity

Different web AIs have different context limits, tool-format expectations, and behaviors. A simple conversation copy is not enough. The opportunity is a **translation + execution layer** that converts local state into a useful task representation a web AI can act on, grounded against the real local project — turning any web chat into a real coding agent on the user's machine.
