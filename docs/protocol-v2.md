# AI Continuity Bridge — Communication Protocol v2.0

## 1. Overview

This document specifies the wire protocol between the Chrome Extension (content script + background service worker) and the Rust/Tauri desktop application over a local WebSocket.

**Transport:** WebSocket over TCP, loopback only (`ws://127.0.0.1:45241`)
**Framing:** JSON text messages (no binary frames)
**Authentication:** 6-digit pairing code, exchanged on connection

---

## 2. Connection Lifecycle

```
Extension                          Rust App
   |                                   |
   |──── TCP connect ─────────────────>|
   |──── WebSocket handshake ─────────>|
   |                                   |
   |──── {"type":"pair",               |
   |      "code":"123456",            |
   |      "proto":2} ────────────────>|
   |                                   |
   |<──── {"type":"pair-ok",           |
   |       "proto":2,                 |
   |       "server_version":"0.2.0"} ─|
   |                                   |
   |   [Connection ready]              |
   |                                   |
   |──── {"type":"ping"} ────────────>|
   |<──── {"type":"pong"} ───────────|
   |   (every 15s)                     |
   |                                   |
   |   [If no pong in 45s → reconnect] |
```

**Reconnection:** Exponential backoff (1s → 2s → 4s → 8s → 30s cap). On reconnect, auto-re-pair with stored code.

---

## 3. Message Envelope

All messages follow this structure:

```jsonc
{
  "id": "uuid-v4",           // Unique request ID (for request-response matching)
  "type": "message_type",    // One of the defined types
  "proto": 2,                // Protocol version (optional on non-pair messages)
  "timestamp": 1724000000000 // Unix ms (for logging/debugging)
}
```

---

## 4. Message Types

### Extension → Rust

| Type | Purpose | Required Fields |
|------|---------|-----------------|
| `pair` | Authenticate | `code`, `proto` |
| `ping` | Heartbeat | — |
| `tool_call` | Execute a tool | `id`, `tool`, `arguments` |
| `tool_approve` | User approved/denied | `id`, `allow` |
| `handoff_request` | Request handoff build | — |
| `cancel` | Kill the processes a request's `run_command` spawned | `id` |

### Rust → Extension

| Type | Purpose | Required Fields |
|------|---------|-----------------|
| `pair_ok` | Pairing succeeded | `proto`, `server_version` |
| `pair_error` | Pairing failed | `error` |
| `pong` | Heartbeat response | — |
| `tool_result` | Tool execution result | `id`, `status`, `result?`, `error?`, `meta?` |
| `tool_stream` | Streaming output chunk | `id`, `chunk`, `stream_id` |
| `handoff` | Pushed handoff payload | `payload` |
| `handoff_error` | Handoff build failed | `error` |
| `cancel_ok` | Cancellation done | `id`, `killed` |
| `error` | Malformed / unknown frame (connection survives) | `error` |

### Cancellation and idempotency

`cancel` with a request id SIGTERMs the process group of every `run_command`
that request spawned (SIGKILL after a 500ms grace) and replies
`cancel_ok` with the count. This is the only way to stop a long command
short of its 120s timeout — dropping the WebSocket connection does *not*
kill running commands.

A `tool_call` whose `id` already reached execution is refused with
`DUPLICATE_REQUEST` instead of re-running — the extension retries timed-out
calls with the same id, and a re-executed `write_file` or `run_command` is
not harmless. Frames above 10MB and frames with unknown `type` values are
answered with `error` and *do not* terminate the connection.

The extension's terminal widget shows a **Stop** button while a
`run_command` is in flight; it sends the `cancel` frame for that request
id, and a cancelled request is never retried on timeout. Cancelling a
command that is still awaiting desktop approval kills nothing — the
approval queue has no cancellation path yet.

---

## 5. Tool Call Request

```jsonc
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "type": "tool_call",
  "tool": "read_file",
  "arguments": {
    "path": "src/App.tsx",
    "offset": 401,   // 1-based first line; absent → 1
    "limit": 400     // max lines; absent → 400, always capped at 16KB
  },
  "timestamp": 1724000000000
}
```

---

## 6. Tool Result

```jsonc
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "type": "tool_result",
  "status": "success",       // "success" | "error" | "pending" | "denied" | "timeout"
  "result": {
    "output": "file contents...",
    "bytes": 12345
  },
  "error": null,
  "meta": {
    "tool": "read_file",
    "duration_ms": 42,
    "path": "src/App.tsx"
  }
}
```

### Status Values

| Status | Meaning | `result` | `error` |
|--------|---------|----------|---------|
| `success` | Tool executed OK | `{ output, bytes? }` | `null` |
| `error` | Tool failed | `null` | `{ code, message }` |
| `pending` | Awaiting user approval | `{ summary }` | `null` |
| `denied` | User denied the action | `null` | `{ code: "DENIED", message }` |
| `timeout` | Execution timed out | `null` | `{ code: "TIMEOUT", message }` |

---

## 7. Error Codes

| Code | Category | Auto-Retry |
|------|----------|------------|
| `FILE_NOT_FOUND` | Validation | No |
| `FILE_IS_BINARY` | Validation | No |
| `FILE_TOO_LARGE` | Validation | No |
| `PATH_ESCAPES_ROOT` | Security | No |
| `PERMISSION_DENIED` | Security | No |
| `SENSITIVE_PATH` | Security | No |
| `INVALID_ARGUMENTS` | Validation | No |
| `DUPLICATE_REQUEST` | Idempotency | No |
| `MALFORMED_JSON` | Detection | No |
| `EXECUTION_FAILED` | Runtime | Maybe |
| `COMMAND_TIMEOUT` | Runtime | Maybe |
| `CONNECTION_LOST` | Transport | Yes |
| `NOT_PAIRED` | Auth | No |
| `UNKNOWN_TOOL` | Validation | No |
| `INTERNAL_ERROR` | Server | Yes |
| `DENIED` | User | No |

---

## 8. Tool Definitions

Implemented today. Names are resolved through the shared spec table
(`src-tauri/src/bridge.rs::SPECS`, mirrored in `extension/tool-spec.js`), which
also accepts per-tool aliases — a model that emits `Read`, `bash` or
`default_api.read_file` still lands on the right tool.

| Tool | Required Args | Optional Args | Max Output | Approval |
|------|---------------|---------------|------------|----------|
| `read_file` | `path: string` | `offset: u32`, `limit: u32` | 16KB per chunk | Auto (unless sensitive) |
| `write_file` | `path, content` | — | — | Always |
| `run_command` | `command` | — | 1MB | Always |
| `list_directory` | `path` | — | 256KB | Auto |
| `git_status` | — | — | 64KB | Auto |
| `describe_tool` | `name` | — | — | Auto |
| `list_tools` | — | — | — | Auto |

`describe_tool` and `list_tools` answer from the spec table alone, so they work
before a project is opened — an AI that has lost the manifest can always
recover it.

### Chunked reads

`read_file` returns `cat -n` style numbered lines, and **pages** rather than
truncates. A chunk is at most 400 lines and at most 16KB, whichever comes
first; a single line longer than 16KB is still returned whole, so a minified
bundle makes progress instead of looping on the same offset. The file itself
must be under 16MB (`FILE_TOO_LARGE`, checked from metadata so an oversized
file is never loaded).

A file that fits in one chunk gets no footer. Anything larger ends with:

```
[chunk 1 of 3 · lines 1-400 of 1000 · 3.4 KB of 9.5 KB]
[to continue, call: read_file("src/App.tsx", 401)]
```

The last chunk says `[end of file]` instead. **The AI decides whether to
continue** — the extension never auto-pages. Auto-feeding chunks with
auto-submit would flood the chat, and an auto-submit loop is what froze the
host page in the first place.

The footer's leading `[` is load-bearing: `parseToolLine` strips list markers
and backticks but not brackets, so the footer cannot fire itself when the AI
echoes the result back. `offset` is clamped to the last line rather than
rejected — the AI is guessing at file length, and a hard error on a stale
offset would strand it mid-file. Both `offset` and `limit` are accepted as a
JSON number or as a quoted digit string, because models emit both.

Reserved by the protocol but **not yet implemented** (the core returns
`UNKNOWN_TOOL`): `search_files`/`grep`, `glob`, `edit_file`, the `git_*` write
tools, and the `recursive`/`max_depth` and `cwd`/`timeout_ms` optional args.

---

## 9. Tool Call Detection (Extension)

**Priority Order:**

1. `<acb_tool>` tags (highest reliability)
2. Fenced JSON blocks (```acb` or ```json`)
3. Function-call syntax, **anchored to the start of a line** (`read_file("path")`)
4. Inline JSON (lowest priority)

**Function-call syntax must begin its line.** Leading list markers, blockquote
arrows, backticks and `1.` ordinals are stripped first, but a call buried in a
sentence is ignored. Matching anywhere in the line meant prose executed: "you
can use `run_command`: npm test" opened a real approval card, and a bullet list
naming `git_status` ran it. Zero-argument tools therefore require empty parens
(`git_status()`, `list_tools()`).

**The manifest is deliberately not in call syntax.** `list_tools` and
`describe_tool` auto-insert their output into the chat, so the AI echoes it
back into the scanner. Rendered as `name(args)`, every row was a live call and
echoing the manifest executed the entire tool surface at once. Both
`bridge.rs::tool_manifest` and the extension's `manifest()` emit an aligned
`name  args  summary` table instead, and each is guarded by a test.

**Result size caps.** A result is truncated to 24KB before it is inserted into
the chat composer, with a `[truncated at N of M bytes]` marker naming the real
size, and to 128KB before a widget renders it (`[output truncated]`, the same
marker the core uses for oversized command output). `read_file` no longer needs
the composer cap — it chunks at 16KB in the core (§8) — so this is the backstop
for `run_command` output and large directory listings. These are display
limits; pushing a whole large file into a rich-text composer froze the host
page.

**Streaming Safety:**
- For `<acb_tool>` blocks: Wait for closing `</acb_tool>` tag
- For fenced blocks: Wait for closing ``` before extracting
- For function calls: Use balanced-paren matcher, wait for closing `)`
- Debounce: 800ms after last DOM mutation before scanning
- Mutations inside the extension's own UI (`#acb-root`, the stage indicator,
  the status pill, toasts, the handoff overlay) do not schedule a scan

**Deduplication:**
- Content-script level: `Set<string>` of JSON-serialized tool calls (FIFO, 200 max)
- Background level: `Map<string, {id, timestamp}>` of tool signatures (TTL 60s)

---

## 10. Request-Response Matching

- Extension generates UUID v4 for each tool call
- Background maintains `Map<id, {tool, timestamp, retries}>` for pending requests
- Rust echoes the `id` back in all responses for that request

**Timeout per Request:**

Derived from the shared spec table, not hardcoded per call site.

| Tool | Timeout | Retry |
|------|---------|-------|
| `read_file` | 10s | 1 |
| `write_file` | 15s | 0 |
| `run_command` | 120s | 0 |
| `list_directory` | 10s | 1 |
| `git_status` | 10s | 1 |
| `describe_tool` | 5s | 1 |
| `list_tools` | 5s | 1 |
| _unknown_ | 15s | 0 |

---

## 11. Streaming Output

For `run_command`, the Rust side can stream output chunks:

```jsonc
{
  "id": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "type": "tool_stream",
  "stream_id": "run_123",
  "chunk": {
    "kind": "output",        // "start" | "output" | "exit"
    "data": "PASS  auth.test.ts\n"
  },
  "timestamp": 1724000000000
}
```

---

## 12. Sequence Diagrams

### Success Flow — read_file

```
ChatGPT          Extension (content)    Extension (bg)        Rust App
   |                    |                    |                    |
   | outputs tool call  |                    |                    |
   |───────────────────>|                    |                    |
   |                    | sendTool({tool})   |                    |
   |                    |───────────────────>|                    |
   |                    |                    | tool_call (id=abc) |
   |                    |                    |───────────────────>|
   |                    |                    |                    | validate
   |                    |                    |                    | read file
   |                    |                    |                    | audit
   |                    |                    | tool_result (abc)  |
   |                    |                    |<───────────────────|
   |                    | tool-result (abc)  |                    |
   |                    |<───────────────────|                    |
   |                    | showToolWidget()   |                    |
```

### Success Flow — write_file (with approval)

```
ChatGPT          Extension (content)    Extension (bg)        Rust App
   |                    |                    |                    |
   | outputs acb block  |                    |                    |
   |───────────────────>|                    |                    |
   |                    | sendTool({tool})   |                    |
   |                    |───────────────────>|                    |
   |                    |                    | tool_call (abc)    |
   |                    |                    |───────────────────>|
   |                    |                    |                    | validate
   |                    |                    |                    | needs approval
   |                    |                    | tool_result (abc)  |
   |                    |                    |    status: pending |
   |                    |<───────────────────|                    |
   |                    | showToolCard()     |                    |
   |                    | render Allow/Deny  |                    |
   | user clicks Allow  |                    |                    |
   |                    | tool_approve       |                    |
   |                    |───────────────────>|                    |
   |                    |                    | approve (abc)      |
   |                    |                    |───────────────────>|
   |                    |                    |                    | execute
   |                    |                    |                    | audit
   |                    |                    | tool_result (abc)  |
   |                    |                    |<───────────────────|
   |                    | tool-result (abc)  |                    |
   |                    |<───────────────────|                    |
   |                    | showResultBlock()  |                    |
```

### Error Flow — file not found

```
ChatGPT          Extension (content)    Extension (bg)        Rust App
   |                    |                    |                    |
   | outputs tool call  |                    |                    |
   |───────────────────>|                    |                    |
   |                    | sendTool({tool})   |                    |
   |                    |───────────────────>|                    |
   |                    |                    | tool_call (abc)    |
   |                    |                    |───────────────────>|
   |                    |                    |                    | validate
   |                    |                    |                    | file not found
   |                    |                    | tool_result (abc)  |
   |                    |                    |   status: error    |
   |                    |                    |   error: FILE_NOT_FOUND
   |                    |<───────────────────|                    |
   |                    | showErrorWidget()  |                    |
```

### Connection Drop & Recovery

```
Extension (bg)                       Rust App
   |                                    |
   |   [WebSocket closes]              |
   |                                    |
   | set ws_connected = false          |
   | clear pending requests            |
   | notify content scripts            |
   |                                    |
   | scheduleReconnect(1s)             |
   |                                    |
   |──── TCP connect ─────────────────>|
   |──── WebSocket handshake ─────────>|
   |──── pair (code) ────────────────>|
   |<──── pair-ok ────────────────────|
   |                                    |
   | set ws_connected = true           |
   |                                    |
   |   [Pending requests were lost     |
   |    AI must re-emit tool calls]    |
```

---

## 13. Best Practices

### Extension Side

1. Always use `<acb_tool>` tags — instruct the AI to use explicit tags
2. Wait for complete JSON — never fire on partial streaming output
3. Signature-based dedup — JSON-serialize the tool call, check against Set
4. Request-response matching — use UUID, track pending requests with timeouts
5. Graceful degradation — if WS is down, show "offline" status

### Rust Side

1. Validate everything — tool name, arguments, paths, permissions
2. Audit every call — record to SQLite regardless of outcome
3. Timeout enforcement — every tool execution has a hard timeout
4. Output capping — prevent memory exhaustion from large outputs
5. Error specificity — use precise error codes, not generic strings

### Protocol Level

1. Protocol versioning — negotiate on pair, reject incompatible versions
2. Idempotency — retrying the same `id` should return the same result
3. Message size limits — reject messages > 10MB
4. Graceful close — send `close` frame before disconnecting
