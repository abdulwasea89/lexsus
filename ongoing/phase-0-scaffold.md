# Phase 0 — Scaffold & Foundation (Completed)

> **Status:** ✅ Done — committed on `developing`.
> **Branch:** `developing`
> **M0 Definition of Done:** Tauri app launches · Rust core modules compile & run · SQLite schema migrates · compression-service `/health` responds · lint/typecheck/build pass · `ongoing/` doc written.

> **Historical record.** This document was written while Lexsus still used a
> Chrome-extension / loopback-WebSocket transport; that transport was later removed
> for a desktop-local native MCP connector (see `2026-09-10-mcp-native-architecture.md`
> in this directory). Any setup step or architecture description below that refers to
> the extension, pairing codes, or `ws://127.0.0.1:45241` is preserved and no longer applies.

This document explains everything built in Phase 0, how it works, how it was verified, and how to extend it in Phase 1.

---

## 1. What Phase 0 Delivered

A runnable skeleton of the **AI Continuity Bridge** — the desktop "control center" that will (in Phase 1) let a **web AI act as a coding agent** on your local machine: reading files, writing files, and running terminal commands, with a live activity trace, an interactive terminal, and a full git workflow.

Phase 0 = the **foundation only**. No Claude Code observation, no browser extension, no web-AI tool relay yet — those are Phase 1. But every one of those features will build directly on the modules written here.

```
lexsus/
├── index.html, package.json, tsconfig*.json, vite.config.ts  # Vite + React + TS frontend
├── .gitignore, .prettierrc, eslint.config.js                 # conventions
├── src/                        # React/TS frontend (Tauri shell UI)
│   ├── main.tsx                # React entry
│   ├── App.tsx                 # "control center" shell demoing the Rust core
│   ├── App.css
│   └── lib/
│       ├── bridge.ts           # typed wrappers around Tauri/Rust commands
│       └── types.ts            # shared types mirrored from Rust
├── src-tauri/                  # Rust core (Tauri v2)
│   ├── Cargo.toml              # git2, portable-pty, notify, rusqlite
│   ├── tauri.conf.json, build.rs, capabilities/, icons/
│   └── src/
│       ├── main.rs             # binary entry → calls lib::run()
│       ├── lib.rs              # AppState + Tauri commands (IPC to frontend)
│       ├── db.rs               # SQLite + hand-rolled migration runner (Layers 1 & 2)
│       ├── git.rs              # git2: status, branch, commit
│       ├── pty.rs              # portable-pty: run commands + interactive shell
│       ├── watcher.rs          # notify: OS-native file watching
│       └── bridge.rs           # web-AI tool-call contracts (stub for Phase 1)
├── compression-service/        # Python + FastAPI (Layer 3 skeleton)
│   ├── main.py                 # /health + /compress(501 stub)
│   └── requirements.txt
├── .github/workflows/ci.yml    # CI: frontend lint/build, Rust fmt/clippy/check, Python health
└── ongoing/                    # ← this phase's documentation lives here
```

---

## 2. The Tech Stack (as actually scaffolded)

| Layer | Chosen | Why |
|-------|--------|-----|
| Application shell | **Tauri v2** + React 19 + TypeScript + Vite 7 | Smaller binary than Electron, native OS access, smaller attack surface |
| Systems language | **Rust** (one language) | C-level OS control with memory safety |
| Git | **`git2`** (libgit2, vendored) | diffs, status, staging, branches, commits — powers the future git panel |
| Terminal | **`portable-pty`** | spawn commands in a pseudo-terminal; powers the future interactive terminal pane + `run_command` tool |
| File watching | **`notify`** | OS-native backends (inotify / fsevents / ReadDirectoryChangesW) |
| Local state | **SQLite** via `rusqlite` (bundled) | fully local, zero ops |
| Compression | **Python + FastAPI** | local microservice for LLM context compression (skeleton now, real in Phase 2) |
| Frontend/IPC | Tauri commands over `invoke()` | typed wrappers in `src/lib/bridge.ts` |

> **Note:** `git2` uses `vendored-libgit2` + `vendored-openssl` features so no system OpenSSL dev headers are required at build time (only the Tauri GUI deps on Linux: webkit2gtk-4.1, etc.).

---

## 3. The Rust Core, Module by Module

### `db.rs` — SQLite + hand-rolled versioned migrations
- **Layer 1 (Session Archive):** `sessions`, `session_events` tables — capture agent stdin/stdout, timestamps, exit codes.
- **Layer 2 (Structured Project Memory):** `objectives`, `decisions`, `attempts`, `constraints`, `changed_files`, `progress` — "facts, not chat".
- **Migration runner:** a `schema_migrations` table records applied versions; each migration runs in a transaction and is idempotent.
- **API:**
  - `open_and_migrate(path)` → opens/creates the DB and applies pending migrations.
  - `applied_versions(&conn)` → the list of applied versions (used by CI/health).

```rust
pub const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_session_archive", "... CREATE TABLE sessions ..."),
    ("0002_structured_project_memory", "... CREATE TABLE objectives ..."),
];
```

### `git.rs` — git via `git2`
- `open_repo(path)` → open a repository.
- `current_branch(repo)` → current branch name.
- `status(repo)` → per-file `GitFileStatus { path, status, additions, deletions }` (untracked/staged/modified/deleted/renamed).
- `commit(repo, message)` → stage-all + commit, returning the new OID. **This is the "commit from the app" primitive** the git panel will call.

### `pty.rs` — terminal via `portable-pty`
- `run_command(cmd, cwd)` → runs a shell command in a PTY, captures output + exit code. **This is the `run_command` tool primitive** the web AI will call in Phase 1.
- `spawn_interactive_shell(cwd)` → spawns a long-lived shell PTY and returns a channel of output chunks. **This is the interactive terminal pane primitive.**

### `watcher.rs` — file watching via `notify`
- `watch(path)` → starts a recursive watcher on a folder and returns a channel of normalized `FsEvent { path, kind }` (created/modified/removed). This is the **grounding signal** that confirms a file *actually* changed on disk — the key to showing honest activity (not just what an agent *claimed*).

### `bridge.rs` — web-AI tool contracts (stub)
- Defines the `Tool` enum (`ReadFile`, `WriteFile`, `RunCommand`, `ListDirectory`, `GitStatus`) and `ToolResult`.
- `tool_not_implemented(tool)` → stub that returns a clear "not implemented in Phase 0" result. **The execution plumbing lands in Phase 1.**

### `lib.rs` — wiring + Tauri IPC
- Holds `AppState` (SQLite connection + project root) in a `Mutex`.
- Exposes Tauri commands the frontend calls via `invoke()`:
  - `init_database(db_path)` → migrate + return applied versions.
  - `set_project_root(path)` → set the monitored folder.
  - `git_status`, `git_branch`, `git_commit` → the git panel.
  - `run_command` → terminal/tool execution.
  - `start_watch` → begin file watching.
  - `bridge_tool` → Phase-1 stub.
  - `spawn_shell` → interactive shell.

---

## 4. The Frontend

- **`src/lib/types.ts`** — TS types mirroring Rust's serde structs (`GitFileStatus`, `CommandOutput`, `FsEvent`).
- **`src/lib/bridge.ts`** — thin typed wrappers around every Tauri command (e.g. `gitStatus()`, `runCommand(cmd)`).
- **`src/App.tsx`** — a minimal **control-center shell** that proves the Rust core is wired end-to-end:
  - shows current git branch + status table,
  - lets you write a commit message and commit,
  - has a mini "terminal" that runs a command and shows its output.
- **`src/App.css`** — dark control-center styling (sidebar + main panel + status bar).

This is intentionally minimal — Phase 1 replaces it with the full control center (activity trace, real terminal pane, git panel, handoff card).

---

## 5. The Compression Service (skeleton)

`compression-service/main.py` — a local FastAPI microservice on port 8000:

- `GET /health` → `{"status": "ok", "service": "compression-service"}` (liveness probe; used by the Rust core and Docker healthcheck).
- `POST /compress` → **501 stub** by design. It defines the request/response contracts (`CompressRequest`, `CompressResponse`) but raises 501 so callers can rely on the API shape without a half-working implementation. The LangChain-backed compression ships in Phase 2.

---

## 6. Conventions & CI

- **`.gitignore`** — node_modules, dist, Rust target, Python venv/`__pycache__`, `.env`, editor files.
- **`.prettierrc`** + **`eslint.config.js`** — formatting + lint rules (React hooks, TS strict).
- **`package.json` scripts:** `dev`, `build`, `preview`, `tauri`, `lint`, `typecheck`.
- **`.github/workflows/ci.yml`** — three jobs:
  1. **Frontend:** `npm ci` → `lint` → `typecheck` → `build`.
  2. **Rust core:** `cargo fmt --check` → `cargo clippy -D warnings`.
  3. **Compression service:** install FastAPI deps → assert `/health` returns 200.

---

## 7. How It Was Verified (on this machine)

Because this Linux machine lacks the Tauri GUI system deps (webkit2gtk) and no sudo is available, the **GUI itself cannot be launched here**. So the Rust core modules were verified through a **standalone test harness** (`/tmp/opencode/corecheck`) that compiled and exercised them directly:

```
[db]      applied migrations: ["0001_session_archive", "0002_structured_project_memory"]
[db]      sessions rows: 1          ← schema works, rows insert/read back
[pty]     exit=Some(0) output="hello-pty"   ← portable-pty executes commands
[watcher] registered on /tmp/aicb_watchcheck ← notify constructs + registers
[bridge]  stub ok=false err=Some("tool not yet implemented in Phase 0: ...")
CORE CHECK PASSED
```

**Frontend verified locally:** `npm run lint` ✓ · `npm run typecheck` ✓ · `npm run build` ✓ (clean).

**Compression service verified locally:**
```
GET /health   → 200 {"status":"ok","service":"compression-service"}
POST /compress → 501 (intentional stub)
```

Two real bugs were caught by the harness and fixed:
1. `db.rs`: `Connection` needed to be `mut` to call `.transaction()`.
2. `pty.rs`: `try_clone_reader()` returns `anyhow::Error` — now mapped into `std::io::Error`.

---

## 8. Prerequisites to Actually Run the GUI

To launch `npm run tauri dev` you need (Linux):
- Rust toolchain ✅ (installed: 1.97.1)
- System deps (need sudo): `libwebkit2gtk-4.1-dev build-essential libssl-dev libxdo-dev libayatana-appindicator3-dev librsvg2-dev file curl wget`
- `npm install` (done)

On macOS/Windows the equivalent Tauri prerequisites apply.

---

## 9. What's NOT in Phase 0 (deferred)

| Feature | Phase |
|---------|-------|
| Claude Code PTY observation + activity extraction | 1 |
| Browser extension + local IPC | 1 |
| Web-AI tool execution (`read_file`/`write_file`/`run_command`) | 1 |
| Handoff card + "Continue with ChatGPT" | 1 |
| Interactive terminal pane UI (core primitive done) | 1 |
| Git panel UI (core primitive done) | 1 |
| Context compression (Layer 3) | 2 |
| Automatic failover | 3 |
| Orchestration / team / enterprise | 4–5 |

---

## 10. How to Extend (Phase 1 starting points)

- **Web-AI `run_command`:** `pty::run_command` is ready — Phase 1 routes `bridge::Tool::RunCommand` to it with permission checks.
- **`read_file`/`write_file`:** trivial on top of `std::fs` + the existing permission model in `bridge.rs`.
- **Live trace grounding:** feed `watcher` events + parsed command output into the future activity tree; the cross-correlation already has both signals.
- **Commit from app:** `git::commit` is wired through `git_commit` and already exercised by the demo UI.
- **Interactive terminal:** `spawn_interactive_shell` exists; the UI pane is Phase 1.

---

*Written for the `ongoing/` series. Phase 1 will add `ongoing/phase-1-mvp.md`.*