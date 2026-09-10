# The extension is gone: Lexsus is now MCP-native

> **Written:** 2026-09-10 (morning session)
> **Branch:** `developing` → merging to `main`
> **Scope:** remove the browser-extension transport, promote the native MCP
> connector to the single transport, and repoint every doc at it.
> **Read time:** ~6 min. If you only read one section, read **"What this means
> for the product"** and **"Open threads"**.

---

## What happened

For most of this project's life, Lexsus reached the browser through a **Chrome
MV3 extension**. It paired over a loopback WebSocket (`ws://127.0.0.1:45241`)
with a 6-digit code, watched the provider page's DOM, parsed tool calls out of
the model's *visible output*, and pasted results back into the composer as a
follow-up message.

A second, provider-native path had been built alongside it: a **desktop-local
MCP server**. As of this change, the extension path is **deleted** and the MCP
connector is **the only transport**.

This was a deliberate reversal. The earlier plan explicitly kept the extension
as a permanent, supported fallback ("Path A"), on the reasoning that provider
connector support is uneven — Gemini had no broad consumer MCP, ChatGPT/Codex
varied, Claude and Grok depended on plan and rollout. That reasoning is still
true about *coverage*; it was overruled on *cost*. Maintaining two transports
meant maintaining two parsers, two drift guards, two security surfaces and two
documentation stories, and the extension's half was the half that was
structurally fragile.

## Why remove it rather than keep both

The extension's core technique was **DOM scraping plus output parsing**, which
this project had already flagged as its single highest platform risk (10/10) in
[`requirements/trade-offs.md`](../requirements/trade-offs.md). Concretely, it
meant:

- **It broke when the provider redesigned their page.** A CSS selector is not
  an interface contract.
- **The AI had to emit tool calls as visible text**, and Lexsus had to parse
  them back out. Every prompt line had to be *inert by construction* so an AI
  echoing a manifest didn't fire real tools — an entire class of bug that
  existed only because of how the transport worked.
- **Results were re-injected into the composer**, which is the provider's
  input box, not an API.
- **Two registries had to be kept in sync by hand** (`SPECS` in Rust, `TOOLS`
  in JS) with a script (`check-spec-sync.mjs`) standing between a hand-edited
  table and a silent half-registered tool.

The native connector removes all of that. The provider gives the model a real
tool channel; Lexsus answers MCP requests on loopback. No page, no selectors,
no parsing of model prose, no composer.

## What was deleted

| Removed | Why |
|---|---|
| `src-tauri/src/ws.rs` (582 lines) | The loopback WebSocket server: connection accept, 6-digit pairing with brute-force lockout, `push_handoff`, `new_pair_code`, `parse_tool_call_v2`, `ws_connected` tracking |
| `extension/` (10 files) | The MV3 extension: background worker, content scripts for ChatGPT and Claude/Gemini/Grok, tool widgets, popup with the pairing code |
| `scripts/check-spec-sync.mjs` | Its only job was keeping the Rust `SPECS` table aligned with `extension/tool-spec.js`; with one registry it has nothing to sync |
| `tungstenite`, `getrandom` (Cargo deps) | Both existed solely for `ws.rs` |
| 4 Tauri commands | `pair_get_code`, `pair_status`, `handoff_send`, `failover_deliver` |
| 5 `AppState` fields | `pair_code`, `ws_connected`, `ws_tx` and friends |
| Frontend pairing UI | The `pair://*` event listeners, the pairing block in the project dialog, the `paired` prop threaded through 5 components |
| Transport-era error codes | `ErrorCode::MALFORMED_JSON`, `ConnectionLost`, `NotPaired` — all three were raised only by the WebSocket path and had zero construction sites left |
| The `SOURCE_WEB` label | Its last use was an unreachable 300 s approval-window arm; nothing has set source `web` since the extension was deleted |
| Stale comments in 5 modules | `bridge.rs`, `lib.rs`, `failover.rs`, `process.rs`, `db.rs` and `mcp.rs` still narrated the WebSocket, the extension and the composer caps in their doc-comments — the code now describes itself accurately |

**One live bug fell out of the sweep.** `ApprovalBanner.tsx` labelled an
approval by testing `source === "web"`, and the connector sends `source: "mcp"`
— so every approval arriving over MCP was announcing itself as *"Desktop
requests:"*, which is the one label that means "you did this yourself, go ahead
and approve it". Fixed to test for `mcp`. Worth remembering as the general
hazard here: renaming a source label is a rename across a language boundary
that no compiler checks.

## What replaced it

**One transport:** `src-tauri/src/mcp.rs` — an `rmcp` 3.2 MCP server speaking
**Streamable HTTP**, bound to loopback at `http://127.0.0.1:45147/mcp`. It runs
on its own OS thread with its own multi-thread tokio runtime, started from
Tauri's `.setup()`.

**Read-only first, by default.** A new `mcp_allow_write` flag gates the whole
write/command surface. While it is off, `tools/list` simply does not advertise
those tools — the AI cannot call what it cannot see. It is seeded from
`LEXSUS_MCP_ALLOW_WRITE` and flipped **live** from the desktop
(`mcp_set_allow_write`) with no rebuild and no reconnect, which is also the
moment the old `mcp.rs` temp patch (`mcp_allow_write = true`) became a real
runtime control.

**An explicit host allowlist, not a hardcoded one.** rmcp guards against DNS
rebinding by rejecting unknown `Host` headers. Loopback is always accepted;
anything beyond it is opted in per-environment via `LEXSUS_MCP_ALLOWED_HOSTS`
(comma-separated). This replaced the temporary line that hardcoded the ngrok
host used for the proof run — the capability survives, the hardcoding does not.

**What did *not* change** is the part that matters most: everything below the
transport. The `bridge.rs` policy engine — approvals, `is_sensitive_path()`,
source-scoped session grants, the kill switch, `resolve_path` containment, the
audit trail, PTY safety, watcher grounding — is untouched. Callers are tagged
`desktop` / `web` / `mcp`, and **the desktop remains the sole approval
authority**, because no provider exposes an approval primitive that can be
relied on. The MCP approval wait is 120 s, comfortably under provider connector
timeouts; results are capped at 140,000 characters.

**A free byproduct:** any *local* MCP host (Claude Code, Claude Desktop, the
MCP Inspector) can point at the same loopback endpoint with no tunnel at all.

## What this means for the product

**One capability was genuinely lost, and it is worth being honest about it.**
The extension could *push*: it could inject a handoff into the chat, and it
could auto-deliver a continuation when your local session died. **MCP cannot
push into a conversation.** Nothing in the protocol lets a server speak into a
chat unprompted.

So delivery became **pull-based**:

- The **Handoff view** builds the handoff and copies it to your clipboard; you
  paste it as the opening message. (It rebuilds from current state at the
  moment you click, so an edited objective is included.)
- A **failover interruption** now surfaces **in-app** — a banner, and a row in
  the DB recording that the handoff was *offered*, not delivered. Previously it
  claimed `delivered: true`.

The right fix is a `get_handoff` connector **tool**, so the AI can pull the
handoff itself and the clipboard step disappears. It is **not built yet** — it
is documented as the next tool in
[`docs/tool-roadmap.md`](../docs/tool-roadmap.md) §Phase 4, together with the
one structural obstacle (the bridge executor's `(tool, source, root)` signature
has no `AppState`, and the handoff builder needs one).

Also unchanged-but-worth-repeating: **`failover.rs` still runs both state
machines.** The local direction (`inactive→working→stalled→interrupted`, vetoed
by real file changes) is untouched and now emits an in-app event. The web
direction is now **inactivity-only** — it used to also detect a dropped socket,
and there is no socket to drop any more.

## Verification — what was actually run

Honest status, because the docs previously overstated a few things:

| Check | Result |
|---|---|
| `cargo check --lib` | ✅ clean |
| `cargo fmt --all --check` | ✅ clean |
| `cargo clippy --lib --all-targets -- -D warnings` | ✅ exit 0, no warnings |
| `pnpm typecheck` | ✅ clean |
| `pnpm lint` | ⚠️ 0 errors, 6 pre-existing warnings |
| `pnpm build` | ✅ succeeded |
| **`cargo test --lib`** | ⚠️ **not run** — see below |

**The Rust unit tests were deliberately not run in this session** (the user's
standing instruction for this work). They were last green at **62 passed / 0
failed** on 2026-09-09, before this change. The change removed `ws.rs` and its
tests, lifted the parser into `bridge.rs`, and touched `mcp.rs`'s config — so
the test suite's *composition* changed even though the policy engine did not.
**Running `cargo test --lib` is the first thing to do on the next pass**, and
the result should be recorded here.

For the same reason, `cargo test` was **not** added to CI as part of this
change. Pushing an unverified gate to `main` would be worse than the existing
gap.

## Open threads

1. **Run `cargo test --lib`** and record the result here. This is the top item;
   it is the only unverified gate in the change. ✅/❌ and the count belong in
   the table above.
2. **Build `get_handoff`** (tool-roadmap Phase 4) — restores pull-based
   continuity and retires the clipboard step. Decide the `AppState` seam first.
3. **The live proof is still not done.** Everything above is machine-verified.
   No real web-AI session has yet driven a Lexsus tool through the native
   connector. The runbook for it is
   [`docs/connector-native-proof-runbook.md`](../docs/connector-native-proof-runbook.md);
   the exit condition is a real Claude.ai session calling `read_file` against a
   real repo with the approval resolving on the desktop.
4. **MCP resources and prompts are unused.** Lexsus currently exposes only
   tools. Resources (workspace files, live `git status`) and a `continue_work`
   **prompt** — which is the handoff, delivered natively — are both natural
   fits and are sketched in the roadmap's "beyond tools" section. The prompt is
   arguably a better answer to item 2 than the tool is.
5. **Phase 2 (`grep`/`glob`) and Phase 8 (LSP diagnostics)** remain the two
   highest-value tool gaps; the roadmap now argues Phase 8 is the bigger one.
6. **Branding is still mixed on purpose.** `com.aicb.bridge`, `productName:
   "ai-continuity-bridge"` and the crate name were **not** changed — that was
   explicitly out of scope for this pass. The repo, README and UI say Lexsus;
   the identifiers do not.
7. **The per-call cancel path is half-dead, and the removal caused it.**
   `cancel_request` (still a registered Tauri command) kills a running command
   by matching `ProcessEntry.owner`, and that owner used to be the WebSocket
   request id set by `ws.rs`. Nothing sets it now, so every process registers
   with `owner: None` and the command can match nothing. No UI calls it, so
   nothing is visibly broken — but "stop this command" is unreachable. Re-wire
   the owner to the MCP request id, or delete the machinery; the half state is
   the one option that isn't acceptable. (Recorded in `PROJECT_STATUS.md` §7.11.)
8. **The compression service is still a stub** (`/compress` → 501); today's
   handoff is uncompressed structured facts.

## Reading order for someone new to this change

1. [`docs/architecture.md`](../docs/architecture.md) — the connector and the four layers, with the removal noted.
2. [`docs/protocol-v2.md`](../docs/protocol-v2.md) — the connector wire protocol (rewritten; it used to describe the WebSocket).
3. [`docs/tool-roadmap.md`](../docs/tool-roadmap.md) — the surface, plus Phases 8–10 and the Claude Code parity map added in this pass.
4. [`docs/connector-native-proof-runbook.md`](../docs/connector-native-proof-runbook.md) — how to expose the connector and prove it live.
