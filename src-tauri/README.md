# src-tauri/ — Rust Core

Tauri 2 backend written in Rust. It owns the tools, the policy engine, storage,
and the connector; the frontend is a thin view over its Tauri commands.

## Commands

```bash
cargo check --lib
cargo fmt --all --check
cargo clippy --lib --all-targets -- -D warnings
```

Run the desktop app from the repo root with `pnpm tauri dev`.

## Modules

- `lib.rs` — app state, Tauri command surface, and bootstrap (owns the
  `mcp_allow_write` / `mcp_listening` flags and spawns the connector).
- `bridge.rs` — the tool engine and policy: tool specs + JSON schemas, parse,
  approval classes (`Auto` / `SensitivePathOnly` / `Always` / `Destructive`),
  sensitive paths, source-scoped session grants, kill switch, audit.
- `mcp.rs` — the single remote transport: a desktop-local MCP server (Streamable
  HTTP, `rmcp`) bound to loopback at `http://127.0.0.1:45147/mcp`. Read-only
  first — the write/command tools stay hidden until `allow_write` is on.
- `db.rs` — embedded SQLite (rusqlite): schema migrations, session archive, and
  structured project memory.
- `archive.rs` — Layer 1 Session Archive: mirror Claude Code's JSONL transcripts
  into SQLite idempotently (keyed by source path + mtime).
- `transcript.rs` — reader for Claude Code's JSONL transcript format.
- `facts.rs` — Layer 2 fact extraction: objective, decisions, failed attempts,
  constraints, changed files, rough progress.
- `git.rs` — `git2` layer for the git panel: status, diff, stage/unstage,
  branches, log, commit.
- `watcher.rs` — OS-native recursive file watcher (inotify / fsevents /
  ReadDirectoryChangesW via `notify`), normalized into one event type.
- `pty.rs` — PTY command execution: each `run_command` runs as a temporary child
  in its own PTY with a hard timeout and output cap; `run_command_stream` reports
  chunks as they arrive for the live terminal.
- `process.rs` — process registry: every spawned child, its owner, and kill
  targets for the cancel path.
- `shell.rs` — shell abstraction: runtime detection and one-shot `CommandBuilder`
  construction (PowerShell/Cmd on Windows, Sh/Bash/Zsh on Unix).
- `failover.rs` — automatic failover: local and web direction state machines that
  detect stalled work.

## Connector security posture

- Binds **loopback only**; no inbound port is opened. A cloud-hosted provider
  reaches it through a tunnel, and `LEXSUS_MCP_ALLOWED_HOSTS` (comma-separated)
  opts extra `Host` values into rmcp's DNS-rebinding guard — loopback is always
  allowed and is the default.
- `mcp_allow_write` gates the write/command tools, **default off**; seed it with
  `LEXSUS_MCP_ALLOW_WRITE=1` or flip it live from the app. `mcp_status` reports
  `{listening, endpoint, allow_write, workspace}`.
- The desktop is the sole approval authority; MCP approvals wait up to 120 s and
  results are capped at 140,000 chars. Session grants stay source-scoped.

_See `docs/architecture.md`, `docs/protocol-v2.md`, and `docs/tech-stack.md`._
