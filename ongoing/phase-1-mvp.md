# Phase 1 — MVP: Prove Web-AI-as-Coding-Agent (Code Complete)

> **Status:** 🔶 Code complete & machine-verified (13/13 Rust tests, clippy/fmt clean, frontend clean). M1 gate (live end-to-end smoke) and the exit gate remain. Not yet committed.
> **Branch:** `developing`
> **M1 Definition of Done:** web-AI tool-call capture + live activity trace · single read-only command terminal streaming `run_command` · git panel from app · browser extension paired to desktop app · handoff card → Continue with ChatGPT · web-AI tool access (read_file/write_file/run_command with local execution + result relay) · exit gate = real interrupted task continued by ChatGPT with real tool access, validated with 5–10 devs (metric: successful continuation rate).

> **Historical record.** This document was written while Lexsus still used a
> Chrome-extension / loopback-WebSocket transport; that transport was later removed
> for a desktop-local native MCP connector (see `2026-09-10-mcp-native-architecture.md`
> in this directory). Any setup step or architecture description below that refers to
> the extension, pairing codes, or `ws://127.0.0.1:45241` is preserved and no longer applies.

This document explains everything built in Phase 1, how it works, how it was verified, and what remains.

---

## 1. What Phase 1 Delivered

Phase 1 turns the Phase 0 skeleton into the real product: **web-AI tool-call capture** feeding a **live activity trace** grounded against a filesystem watcher, a **single read-only command terminal** that streams every `run_command` the web AI executes (command, live output, exit status), a **full git panel** (status/diff/stage/branch/history/commit), a **local WebSocket bridge + 6-digit pairing** connecting a **Chrome extension** to the desktop app, a **handoff card** that hands an interrupted task to ChatGPT, and **real web-AI tool access** — `read_file` / `write_file` / `run_command` executed locally with an approval policy and an audit trail.

> **Terminal model:** the app hosts exactly one terminal — the web AI's command monitor. It is read-only (no embedded shell, no Claude Code pane, no keyboard input); the developer's own terminal is where they run Claude Code. Command output streams live into the pane as it executes.

```
Phase 1 = M1.1..M1.7 (control center)  →  M1 gate  →  M2 (bridge + extension + handoff)
```

### Progress

| Milestone | What it ships | Status |
|-----------|---------------|--------|
| **M1.1** | Shell abstraction + hardened one-shot `run_command` streaming (Rust core) | ✅ **Done** |
| **M1.2** | Single command terminal (xterm.js streams `terminal://run` events) | ✅ **Done** |
| **M1.3** | Web-AI tool-call capture feeding the live trace | ✅ **Done** (code) |
| **M1.4** | Live activity trace — tool-call recording + watcher cross-correlation + headroom UI | ✅ **Done** (code) |
| **M1.5** | Watcher persistence (SQLite-backed structured memory, watcher state) | ✅ **Done** (code) |
| **M1.6** | Git backend — diff, stage/unstage, branches, history (`git2`) | ✅ **Done** (code) |
| **M1.7** | Git panel UI — status/diff/stage/branch/history/commit from app | ✅ **Done** (code) |
| **M1 gate** | Full control center verified end-to-end with a web AI | 🔴 todo (needs live ChatGPT + extension) |
| M2 | Bridge tool access (read/write/run with approvals + live terminal stream) + local WS + pairing | ✅ **Done** (code) |
| M2 | Browser extension paired to desktop app | ✅ **Done** (code) |
| M2 | Handoff card → "Continue with ChatGPT" | ✅ **Done** (code) |
| **Exit gate** | 5–10 real developers continue a real interrupted task | 🔴 todo |

---

## 2. The Tech Stack (Phase 1 deltas)

| Layer | Chosen | Why / change |
|-------|--------|------|
| Terminal rendering | **`@xterm/xterm` 6 + `@xterm/addon-fit` 11** | battle-tested web terminal; sends `\r` for Enter — exactly what Windows ConPTY expects |
| Terminal backend | **`portable-pty` 0.8** (Phase 0) | ConPTY on Windows, pty on Unix; one API for both |
| Shells | **PowerShell → Cmd fallback (Windows)**, `$SHELL` (Unix) | `shell.rs`; `COMSPEC` used only as Cmd's executable path, never for detection |
| Package manager | **pnpm** (switched from npm) | lockfile `pnpm-lock.yaml` committed; `tauri.conf.json` updated |
| WebSocket | **`tungstenite` 0.24** (added M2) | minimal WS server for the extension bridge on `127.0.0.1:45241` |
| Everything else | unchanged from Phase 0 (Tauri v2, React 19, TS, Vite 7, git2, notify, rusqlite) | — |

---

## 3. The Rust Core, Module by Module

### `shell.rs` — NEW in M1.1: shell abstraction
- `Shell` enum: `Sh | Bash | Zsh | Cmd | PowerShell`.
- `Shell::detect()` — Windows: **PowerShell first** (SystemRoot path check), Cmd fallback; Unix: `$SHELL` basename, default `sh`.
- `run_command(cmd)` builder for one-shot executions (no interactive-session builder — the app hosts no shell).
- Windows is a first-class target: tests exercise **both** PowerShell and Cmd, not just whatever `detect()` picks.

### `pty.rs` — REWRITTEN in M1.1: hardened one-shot command execution
- Every command runs as a **temporary child** in its own PTY — there is no persistent interactive session.
- **`run_command_stream(shell, cmd, cwd, timeout, max_output, on_output)`** — streams each output chunk to `on_output` as it arrives, so the bridge can render a web AI's command live; `run_command` is the buffered convenience wrapper.
- Completion = **process exit, not stream EOF** (see Windows findings); after exit, a 500 ms quiet-drain captures trailing output.
- Timeout → kill; output cap → truncate; both reported via `timed_out` / `truncated`.

### `db.rs` — M1.5: persistence
- Migration **0003_trace_and_audit**: `settings` (project_root, pair_code), `trace_steps` (kind/file/command/detail/agent/ts/confirmed/session_id), `audit_log` (agent/tool/args/allowed/approved_by/ok/ts).
- Helpers: `set/get_setting`, `record_trace_step`, `confirm_trace_steps`, `record_audit`, `last_audit`, `trace_stats`.
- DB auto-initialized at `app_data_dir/bridge.db` in the `setup` hook; project root + pair code **restored across launches**.

### `git.rs` — M1.6: full backend
- NEW: `diff_workdir` (per-file `FileDiff {path, status, added, deleted, patch}` via a single `diff.print` pass into a `BTreeMap`), `stage`/`unstage`/`stage_all`, `branches` + `checkout` (via `CheckoutBuilder.force()` + `tree.as_object()` — git2 0.19 APIs), `log` + `commit_diff`.

### `bridge.rs` — M2: web-AI tool execution + approvals
- `Tool` enum (serde externally-tagged, mirrors `types.ts`): `ReadFile | WriteFile | RunCommand | ListDirectory | GitStatus`.
- **Policy:** reads auto-approved except **sensitive paths** (`.env*`, `id_rsa`, credentials/secret/token/password/api_key, `.npmrc`, `.gitconfig`, `.netrc`, `*.pem/key/pfx/p12/ppk/crt`, `.git/config`); **writes and commands always require approval**.
- `resolve_path` — path containment (canonicalize + prefix check, parent-canonicalization for not-yet-existing files); nothing escapes the project root.
- `execute`: read cap 512 KB + binary detection; write; `run_command` 120 s / 1 MB streamed via `CommandEvent` (`start` / `output` / `exit`); list directory; git status.
- `submit` → auto-execute or queue `ApprovalRequest`; `resolve(id, allow, on_event)` executes (streaming to the terminal via `on_event`) and delivers results to waiting WS callers via `SyncSender` channels. 4 unit tests.

### `ws.rs` — NEW in M2: local WebSocket server
- `ws://127.0.0.1:45241` (loopback only), **6-digit pairing code** gates every connection.
- Protocol: `pair` / `pair-ok` / `pair-error` / `ping` / `pong`; `tool` (from extension) runs on a **spawned thread** so the connection stays free to receive `approve` while a call waits; `approve` (extension decision → resolve + audit + terminal stream + trace + UI event); `handoff-request` → builds + pushes the handoff card.
- App → extension: `tool-result`, `handoff` (pushed). `push_handoff` / `send` via `Arc<Mutex<WebSocket>>` (tungstenite 0.24 has no split/Sender).

### `lib.rs` — wiring everything
- `AppState`: conn, project_root, pair_code, ws_connected, ws_tx, bridge, objective, recent_edits.
- **`command_stream(app)`** — maps bridge `CommandEvent`s to `terminal://run` events (`{kind: start|output|exit}`) rendered live by the terminal pane.
- **`record_tool_trace(state, app, tool)`** — maps each executed tool call to a `trace://step` (reading/editing/running, agent=`web`), persists it, and pushes `recent_edits` so the watcher can confirm edits.
- **`start_watch`** — watcher thread: filters `.git`/`node_modules`/`target`, emits `fs://event`, cross-correlates with `recent_edits` → `trace://confirm` + `confirm_trace_steps`.
- **`tool_call(app, tool, source)`** — shared by desktop command and WS relay: submit → auto (audit + trace now) or approval-requested (event) → WS callers wait ≤ 300 s.
- **`build_handoff`** — honest heuristic progress from real trace stats (step count, errors), files touched, next step, editable objective.
- Commands: init_database, set/get_project_root, all git commands, start_watch, bridge_tool/approve/audit, pair_get_code/pair_status, set_objective, build_handoff, handoff_send (push to extension, clipboard fallback).

---

## 4. The Frontend

- **`src/components/TerminalPane.tsx`** (M1.2): xterm.js + FitAddon (dark theme, 5000-line scrollback); **read-only** — subscribes to `terminal://run` and renders `$ command` headers, live output, and exit/timed-out/truncated status. No keyboard input, no embedded shell, no tabs.
- **`src/components/ActivityTrace.tsx`** (M1.4): live steps with per-kind icons; editing steps show `✓ saved` / `… waiting` (watcher grounding); **headroom collapse** — newest 3 expanded, older steps fold into an "N earlier steps · M files touched" summary line (per `docs/ui-design.md`). Fed by web-AI tool calls, not PTY parsing.
- **`src/components/GitPanel.tsx`** (M1.7): Status tab (table with stage/unstage, status badges, diff preview, commit box), Branch tab (list + checkout), History tab (commit list + per-commit diff).
- **`src/components/BridgePanel.tsx`** (M2): pairing code display + connection dot, approval cards (Allow/Deny from the desktop), tool sandbox (read/write/run against real paths), audit trail.
- **`src/components/HandoffPanel.tsx`** (M2): builds the real handoff card (objective editable, progress/files/errors stats, next step) → **Continue with ChatGPT** (pushes to extension + clipboard fallback).
- **`src/App.tsx`**: sidebar (project root, **restored from settings on launch**), statusbar, two-column grid layout with all panels; `startWatch()` after root set.
- **`src/lib/bridge.ts` / `types.ts`**: typed wrappers for every command; `TraceStep/FileDiff/BranchInfo/CommitInfo/BridgeTool/ToolResult/ApprovalRequested/AuditEntry/Handoff/TerminalRunEvent` types.

---

## 5. The Chrome Extension (M2)

`extension/` — Manifest V3, no build step, load unpacked in Chrome:

- **`background.js`** — service worker: WS client with exponential-backoff reconnect + ping watchdog; auto-repair on stored code; routes `tool` (captured requests), `approve` (decisions), `pair`, `handoff-request` to the app; `tool-result` / `handoff` to the chatgpt.com tab.
- **`content.js`** — on `chatgpt.com`: **tool capture** — MutationObserver scans assistant messages for tool lines (`read_file("path")`, `write_file("path","content")`, `run_command("cmd")`, `list_directory("path")`, `git_status`) and relays them; **tool widget** — pending calls show Allow/Deny, results show output + "Insert result into chat"; **handoff card** — top-of-page card with objective/progress/files/errors and a "Continue with ChatGPT" button that types the full agent prompt into the composer.
- **`popup.html/js`** — status dot, 6-digit code entry + Pair/Unpair, "Send handoff to ChatGPT".

---

## 6. Conventions & CI

- **pnpm everywhere:** `pnpm-lock.yaml` committed, `package-lock.json` deleted, `tauri.conf.json` uses pnpm for dev/build.
- **Rust checks:** `cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` (clean).
- **Frontend checks:** `pnpm typecheck` (tsc --noEmit) + `pnpm lint` (eslint, react-hooks) + `pnpm build`.
- **Extension:** plain MV3 JS, no build step; linted by the browser at load.

---

## 7. How It Was Verified (on this machine)

### Test matrix — 13 Rust tests, ~4 s

| Area | Tests | Proves |
|------|-------|--------|
| shell | detect / one-shot builders | detection works, args carried |
| run_command | echo / stream / timeout / cap / failure / both-shells | exit 0 + output; **live chunk streaming**; kill-on-timeout; truncation; **Cmd and PowerShell both** execute |
| bridge (4) | sensitive paths / policy / path escape / execution+approval roundtrip (incl. command stream events) | M2 permission model + containment + streaming |

```
cargo test                          → 13 passed; 0 failed (~4s)
cargo fmt --check                   → clean
cargo clippy --all-targets -- -D warnings → clean
cargo build (lib + bin)             → clean
pnpm typecheck / lint / build       → clean (vite build OK)
```

### Two real Windows bugs found & fixed (with regression tests)

1. **ConPTY stream linger:** PowerShell 5.1 under ConPTY keeps the output stream open after the process exits — "wait for EOF" would hang until timeout. Fixed: completion = `try_wait` on the child + 500 ms quiet-drain.
2. **A bare `\n` is not Enter on Windows:** cmd/PowerShell under ConPTY *echo* LF-only lines but never execute them; `\r\n` is a real Enter. Tests send `\r\n` on Windows, `\n` on Unix — and the real UI is fine because xterm sends `\r` for Enter by design.

### Environment notes (this machine)

- Rust **1.97.1** at `%USERPROFILE%\.cargo\bin` (not on PATH in fresh shells).
- **Windows SDK 10.0.18362.0** installed (was missing `kernel32.lib`).
- Cargo builds require the MSVC environment:

```
cmd /c "call ""C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"" >nul && set PATH=%USERPROFILE%\.cargo\bin;%PATH% && pnpm tauri dev"
```

---

## 8. Manual Test Script (M1 gate — the remaining end-to-end check)

1. Launch the app (`pnpm tauri dev`, command above) → set project folder → watcher starts; the terminal pane shows a hint line (it has no session of its own).
2. **Extension**: load `extension/` unpacked → popup → enter the code from the Bridge panel → paired dot green.
3. Set an objective in the **Handoff panel** → click **Continue with ChatGPT** (or the popup's "Send handoff to ChatGPT") → handoff card appears on chatgpt.com → "Continue with ChatGPT" types the agent prompt.
4. Ask ChatGPT to do a step; when it writes `read_file("path")` / `run_command("...")`, the bridge executes locally, writes/commands ask for **Allow/Deny** (desktop panel or chat widget).
5. When you **Allow** a `run_command` (e.g. `npm test`), the **command terminal** streams it live: `$ npm test`, output as it arrives, then exit status.
6. The **activity trace** shows reading/editing/running steps live (agent `web`); edited files flip to `✓ saved` when the watcher confirms.
7. **Git panel**: stage/unstage, view diffs, commit, switch branches, browse history.
8. Mid-task, stop working → **Handoff panel**: rebuild → card shows honest progress/files/errors from the web AI's real work.

---

## 9. What's NOT Done Yet

| Remaining | What it involves |
|-----------|------------------|
| **M1 gate** | Live end-to-end run with a real web AI session — prep complete: multi-line tool capture shipped (`acb` fenced JSON blocks + streaming-safe dedupe in both content scripts), Windows runbook written (`ongoing/windows-prototype-runbook.md`). Remaining work is the live smoke itself on a Windows machine. |
| **Exit gate** | 5–10 real developers continue a real interrupted task with ChatGPT, metric: successful continuation rate |
| Hardening (out of Phase 1 scope) | Chrome mixed-content/WS permissions edge cases; extension → Firefox port; WS TLS for non-loopback use |

---

## 10. How to Extend (Phase 2 starting points)

- **Layer 3 compression:** `compression-service/main.py` (`/compress` 501 stub) becomes real — summarize trace + diff state into the handoff payload.
- **Pre-handoff context:** ✅ **Done (Phase 3)** — `transcript.rs` reads the developer's own Claude Code transcripts (`~/.claude/projects/*.jsonl`) on demand; the handoff card and auto-failover payload now carry the real task objective + a context snippet + end reason, no embedded terminal needed.
- **More web AIs:** ✅ **Done (Phase 3)** — the extension now targets Claude.ai + Gemini (`content-any.js`) in addition to chatgpt.com, so a dead ChatGPT session can fail over to them with the same handoff + tool-line prompt.
- **Automatic failover:** ✅ **Done (Phase 3)** — `failover.rs` detects a stopped local agent (inactivity, vetoable by any file change) and a dead web AI (WS drop / silence), auto-builds the enriched handoff and auto-delivers it (`auto:true` → the extension inserts + submits the prompt). Telemetry in `failover_log`.
- **Native messaging:** swap the WS transport for Chrome native messaging if a signed/loopback-secured channel is ever needed.

---

*Written for the `ongoing/` series, in the format of `phase-0-scaffold.md`. Phase 1 is code-complete; next update after the M1 gate smoke run and the exit gate.*