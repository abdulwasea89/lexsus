# Tool Roadmap — 44 Tools Across Phases 0–7

> The tool surface the web AI sees, phase by phase: what is **built**, what is
> **planned**, and the invariants every new tool must uphold.
>
> Status date: 2026-09-03. **15 of 44 built.**

This is the capability roadmap for the coding-agent bridge. It is deliberately
separate from `full-plan.md` §13, whose "Phase 0"–"Phase 7" describe the
project's *scaffolding* history (Tauri shell, WebSocket bridge, MVP). Here,
"Phase 0"–"Phase 7" refer to tool-surface expansion only.

## Where the tool surface lives

Every tool must be registered in **three places that must never drift**:

| Place | File | Role |
|---|---|---|
| Rust registry | `src-tauri/src/bridge.rs` — `SPECS` | name, aliases, approval class, timeout, group, auto-insert |
| JS registry | `extension/tool-spec.js` — `TOOLS` | same fields plus the `lineRe` the chat parser matches |
| Wire parser | `src-tauri/src/ws.rs` — `parse_tool_call_v2` | argument coercion for lenient model output |

`scripts/check-spec-sync.mjs` verifies the two registries agree and runs ~50
behaviour checks (alias resolution, prose inertness, composer caps, host
coverage). **Run it after touching any tool.** It is the only thing standing
between a hand-edited table and a silent half-registered tool.

## Approval classes

Every tool carries exactly one, and it decides what the user sees:

| Class | Meaning | Tools today |
|---|---|---|
| `Auto` | executes, result auto-inserted into the chat | `read_file`, `list_directory`, `git_status`, `list_tools`, `describe_tool`, `create_directory`, `read_many_files` |
| `SensitivePathOnly` | asks only when `is_sensitive_path()` fires | `read_file`, `edit_file`, `multi_edit` |
| `Always` | asks every time | `write_file`, `run_command`, `apply_patch`, `copy_file` |
| `Destructive` | asks every time **and the card must show what disappears** | `delete_file`, `move_file` |

The session-grants slice of Phase 6 has landed: a grant ("auto-approve edits
under `src/` for this session") auto-approves matching `Always`/`SensitivePathOnly`
calls, **never** a `Destructive` one and never a sensitive path, and the
GrantsBar's kill switch revokes every grant and pauses the bridge. Grants are
in-memory only — they die with the app.

## Phases at a glance

| Phase | Theme | Built | Added | Total |
|---|---|---|---|---|
| — | original MVP tools | 5 | — | 5 |
| 0 | registry + progressive disclosure | 5 | 2 | 7 |
| 1 | files & editing | 8 (+1 early) | 8 | 15 |
| 2 | search | 0 | 2 | 17 |
| 3 | git | 0 | 10 | 27 |
| 4 | project memory | 0 | 9 | 36 |
| 5 | background commands | 0 | 3 | 39 |
| 6 | approval policy engine | — | 0 | 39 |
| 7 | web & long tail | 0 | 5 | 44 |

Of the 37 not yet built, **~18 are wiring over code that already exists** — all
of Phase 3 (git.rs has every function), 6 of 9 in Phase 4 (the SQLite tables and
extraction are built), and the thin file ops.

---

## Phase 0 — Registry + Progressive Disclosure ✅ DONE

**Goal:** one source of truth for the tool surface, plus a way for a chat to
discover it without a handoff.

| Tool | Approval | Status |
|---|---|---|
| `list_tools` | Auto | ✅ built (`1a141e8`) |
| `describe_tool` | Auto | ✅ built (`1a141e8`) |

Also landed in this phase (commits `1a141e8`, `1aca1f3`, `c830e35`, `34eb826`):

- Single-source `SPECS`/`TOOLS` registry with `check-spec-sync.mjs` as the
  drift guard.
- Manifest and prompt output are **inert by construction** — a call must begin
  its line, so an AI echoing the manifest fires nothing.
- Composer and terminal render caps (`COMPOSER_CAP`, `RENDER_CAP`).
- Chunked `read_file` (see Phase 1).
- Host-drift guard covering the six places the web-AI host list lives.

**Exit criterion — met:** the 5 original tools round-tripping in both a
ChatGPT and a Claude.ai chat.

---

## Phase 1 — Files & Editing (8 tools) ✅ DONE

**Goal:** precise edits instead of whole-file rewrites. Today the only way the
AI can change code is `write_file` on an entire file, overwriting the chunked-read
protocol — every edit is a full-file round trip.

`rm`/`mv`/`cp`/`mkdir` are already reachable through `run_command`, so the
case for dedicated tools is not capability but **sandboxing** (`run_command`
is not path-checked; `delete_file "../../x"` is rejected by `resolve_path()`),
**structured approval** (a card that shows what disappears, not a shell
string), and **portability** (`rm` vs `del` depends on the detected shell).

| Tool | Approval | Status |
|---|---|---|
| `edit_file` | SensitivePathOnly | ✅ built — str-replace edit, the single highest-value tool in this plan |
| `multi_edit` | SensitivePathOnly | ✅ built — batched `edit_file` ops, applied atomically |
| `apply_patch` | Always | ✅ built — unified diff with context-line drift search; `PatchDoesNotApply` |
| `delete_file` | **Destructive** | ✅ built — card shows the resolved absolute path; refuses directories |
| `move_file` | **Destructive** | ✅ built — **both** paths through `is_sensitive_path()` |
| `copy_file` | Always | ✅ built — same both-paths rule |
| `create_directory` | Auto | ✅ built — `create_dir_all` |
| `read_many_files` | Auto | ✅ built — ≤20 paths, 20KB batch budget, sensitive paths skipped with a notice |

New error codes: `StringNotFound`, `AmbiguousMatch`, `PatchDoesNotApply`,
`BridgePaused`.

**⚠️ Gating — resolved:** the first `Destructive` tools shipped together with
the session-grants slice of Phase 6 (grants scoped by tool class + path
prefix, destructive never grantable, GrantsBar kill switch), so the approval
card does not become reflexive clicking. What remains of Phase 6 is polish:
persisted grant policies and per-tool configuration.

---

## Phase 2 — Search (2 tools)

**Goal:** the AI can find things instead of guessing filenames.

| Tool | Approval | Notes |
|---|---|---|
| `grep` | Auto | `regex` crate, content search |
| `glob` | Auto | `glob` crate, filename patterns |

**Invariant — non-negotiable:** every result from both tools is filtered
through `is_sensitive_path()`. Without it, search becomes a secret-exfiltration
path that bypasses the `read_file` gate entirely: `grep ".env"` must return
nothing for `.env` itself even though the file matches.

Gated behind the chunked-read work (now landed): search output can exceed
anything the UI handles, so results need the same paging treatment `read_file`
got.

---

## Phase 3 — Git (10 tools) — the cheapest phase

**Goal:** expose the git workflow the app already has to the AI using it.
Almost pure wiring: `git.rs` has every function, and most already have Tauri
commands in `lib.rs` for the UI panel. Only `git_create_branch` and
`git_show`-class tools need new logic.

| Tool | Approval | Backing |
|---|---|---|
| `git_diff` | Auto | `git.rs::diff_workdir`, `lib.rs:122` |
| `git_log` | Auto | `git.rs::log`, `lib.rs:152` |
| `git_add` | Always | `git.rs::stage`/`stage_all` |
| `git_unstage` | Always | `git.rs::unstage` |
| `git_commit` | Always | `git.rs::commit` |
| `git_branches` | Auto | `git.rs::branches`, `lib.rs:142` |
| `git_create_branch` | Always | ~20 new lines in `git.rs` |
| `git_checkout` | **Destructive** | `git.rs::checkout`, `lib.rs:147` — **must refuse on a dirty tree** |
| `git_commit_diff` | Auto | `git.rs::commit_diff`, `lib.rs:160` |
| `git_show` | Auto | new: render a commit (message + diff) by oid |

---

## Phase 4 — Project Memory (9 tools)

**Goal:** this tier is the product thesis, not just capability — the AI that
inherits the work also inherits the *why*.

| Tool | Backing |
|---|---|
| `todo_write` / `todo_read` | new `0006_todos` migration |
| `set_objective` | facts tables (F3) already built |
| `remember_decision` | built |
| `remember_constraint` | built |
| `remember_attempt` | built |
| `get_facts` | extraction already built |
| `list_sessions` | built |
| `request_handoff` | handoff-request at `ws.rs:394` already built |

Feeds `build_handoff_impl()`: todos become part of the next handoff prompt.

---

## Phase 5 — Background Commands (3 tools)

**Goal:** long-running commands that don't hold the tool channel open.

| Tool | Notes |
|---|---|
| `run_command_background` | needs `spawn_command_background()` in `pty.rs` |
| `command_output` | reads a bounded ring buffer per command |
| `kill_command` | Always |

`appendLine` in `ui-components.js` was kept rAF-coalesced precisely so these
can stream into the terminal.

---

## Phase 6 — Approval Policy Engine (0 tools)

No new tools — this is what contains the ~15 gated ones from Phases 1, 3 and 5:

- **Session grants scoped by tool class and path prefix — landed with Phase 1**
  ("auto-approve writes under `src/` for this session"). In-memory only;
  source-scoped (a web-created grant never covers desktop calls).
- **Destructive never auto-approves.** Not overridable by a grant. Landed.
- **A visible kill switch**: the GrantsBar's "Revoke all & pause" button
  revokes every grant and pauses the bridge. Landed.

Remaining for a later pass: persisted grant policies, per-tool
configuration, and grant expiry.

---

## Phase 7 — Web & Long Tail (5 tools)

| Tool | Notes |
|---|---|
| `web_fetch` | **must block `169.254.169.254` and non-loopback private ranges** — SSRF against cloud metadata and the LAN |
| `web_search` | needs an external API; the only tool whose result isn't local-first |
| `notebook_read` / `notebook_edit` | Jupyter `.ipynb` as structured JSON |
| `delegate_task` | spawn a sub-task and collect its result |

---

## Invariants every new tool must uphold

These are the standing rules the first 7 tools established. A PR that breaks
any of them is wrong regardless of what it adds:

1. **Register in both tables.** `SPECS` (bridge.rs) and `TOOLS`
   (tool-spec.js), plus `parse_tool_call_v2` if the args are non-trivial.
   `node scripts/check-spec-sync.mjs` must stay green.
2. **Tool output must not parse as tool calls.** If the AI echoes a result,
   nothing fires. The anchored line parser and the manifest's no-call-syntax
   rule exist because this class of bug froze the host page.
3. **Cap what you return.** `COMPOSER_CAP` for anything auto-inserted,
   `RENDER_CAP` for anything rendered; page like `read_file` does if results
   can be large.
4. **Sensitive-path filtering applies to lists, not just reads.** `grep`,
   `glob`, `list_directory`-class results included.
5. **Destructive tools show what disappears** and refuse unsafe states
   (`git_checkout` + dirty tree).
6. **Both paths checked on path-pair operations** (`move_file`, `copy_file`).
7. **Every prompt/prompt-section line is inert.** Add the line to the
   self-check in `check-spec-sync.mjs` if you add instructional text.

## Verification per phase

Offline (run all of these before handing anything over):

```bash
node scripts/check-spec-sync.mjs
node --check extension/tool-spec.js && node --check extension/content.js \
  && node --check extension/content-any.js && node --check extension/background.js
cd src-tauri && cargo test --lib && cargo fmt --check && cargo clippy --lib --all-targets
cd .. && pnpm build
```

Live (the real gate — offline checks cannot prove the page doesn't freeze or
the tool doesn't fire): manifest paste produces zero widgets and zero approval
cards; each new tool round-trips in a ChatGPT **and** a Claude.ai chat; the
approval card shows what the plan says it must show.
