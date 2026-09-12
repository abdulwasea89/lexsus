# The tool round trip: what the web AI actually receives

> **Written:** 2026-09-10 (second session)
> **Branch:** `developing` → merging to `main`
> **Scope:** rework how Phase 1's file/editing tools return results across the
> MCP boundary. Tool names and arguments are **unchanged**; the payload is not.
> **Read time:** ~5 min. If you only read one section, read **"The five
> defects"** and **"What a model sees now"**.

---

## Why this happened

The connector landed in the morning session and worked: a web AI could list
Lexsus tools over `http://127.0.0.1:45147/mcp` and call them. But "it works" and
"a model can do good work with it" turned out to be different claims. The
*plumbing* was sound and the *payload* was impoverished — every result crossed
the boundary as plain text, and several carried less information than the core
already had in hand.

The instruction was to remake the Phase 1 tools and improve the connection
between the web AI, the MCP connector and the app, for better tool connection
and output. The scope was settled with three decisions up front: **keep every
tool name and argument exactly as it is**, focus on **what the model sees**
rather than the desktop UI, and return **readable text plus `structuredContent`
with declared output schemas**.

## The five defects

Each was found by reading the code, not by guessing at improvements.

1. **Paging was expressed as call syntax, inside tool output.**
   `chunk_text()` in `bridge.rs` ended a truncated read with
   `[to continue, call: read_file("big.txt", 401)]`. This violates the
   invariant the same file documents forty lines above it: output handed to a
   model must **never** read as `name(args)`, because a model that echoes the
   result back fires a burst of real calls. The guarding test
   (`manifest_and_describe_are_not_call_syntax`) only ever inspected the
   manifest and `describe_tool`, so the footer sat there unnoticed.

2. **`ToolResult.error_code` was computed and then thrown away.**
   The core produces a precise `ErrorCode` — `STRING_NOT_FOUND`,
   `AMBIGUOUS_MATCH`, `PATCH_DOES_NOT_APPLY` — and `mcp.rs` sent only the
   message string. "Retry with a different path" and "your edit did not match"
   arrive as the same wall of prose. The model had no way to branch.

3. **Edits reported no evidence.** `edit_file` returned `"edited {path}"`.
   A model could not tell a landed edit from a silent no-op, so it re-read the
   file to find out — the flailing edit loop in miniature.

4. **The truncation marker lied.** `cap_text` always appended "call read_file
   with an offset to continue", including when what got truncated was
   `git_status` or `run_command`.

5. **Tool descriptions were one terse line.** `tools/list` advertised only the
   `SPECS` summary, ignoring the argument list and approval class that already
   existed in `describe_spec()`, and declared no `title` or `outputSchema`.

## What a model sees now

Every tool result crosses the boundary as **readable text plus a structured
JSON payload**, and each tool advertises an `outputSchema` in `tools/list`:

- **`read_file` returns `next_offset`** (with `start_line`, `end_line`,
  `total_lines`, `truncated`). Paging is a number to pass, not a phrase to
  imitate. The footer is now inert prose.
- **Edits report what changed** — `replacements`, `first_line`,
  `bytes_before`/`bytes_after` — via a new `EditOutcome` returned by
  `apply_str_edit`, which was already counting matches internally.
- **Failures carry their real `error_code`** in `structuredContent`. `null`
  when a failure genuinely has none: an approval timeout is not a tool error,
  and inventing a code for it would be worse than admitting there isn't one.
- **`list_directory` reports `kind` and `size`** per entry, not just names;
  `git_status` and `read_many_files` report per-file structure; `run_command`
  reports `exit_code`/`timed_out`/`truncated`.
- **Descriptors are derived, never authored**: summary + `Args:` + approval
  line from the `SPECS` row, a human-readable title, and `readOnlyHint` /
  `destructiveHint` / `idempotentHint`. A tool added to `SPECS` appears fully
  described on the connector without `mcp.rs` being touched.

## Two decisions worth knowing about

**`structured()` is a trap.** `rmcp`'s `CallToolResult::structured(v)` and
`structured_error(v)` set `content` to `vec![ContentBlock::text(v.to_string())]`
— a raw JSON dump. Using them would have *replaced* the readable text rather
than accompanying it, silently degrading every result. The response is built
with `success()`/`error()` and the public `structured_content` field is
assigned directly. A test (`success_keeps_readable_text_and_structured_content`)
pins this so a later "simplification" back to the constructor fails loudly.

**Declaring `outputSchema` is a promise.** MCP requires `structuredContent` to
conform to the schema the server advertises. So schemas must describe exactly
what the executors emit — no more, no less — and
`structured_output_matches_its_declared_schema` checks both directions
(no undeclared key, no missing required key). That test is why the structured
payloads live beside the data they describe, in the executor that already
knows the counts, rather than being re-derived by parsing output text.

## Verification

`cargo test --lib` had **not been run since the extension removal** — the
previous session's standing instruction was "do not run tests", and the last
known green was 62/0 on 2026-09-09, before that removal. The user lifted the
instruction for this pass, so it was run **before any edit** as a baseline:
**73 passed / 0 failed**. The extension removal did not break the suite.

After the rework: **78 passed / 0 failed**. The five new tests are:

| Test | What it holds |
|---|---|
| `no_tool_output_reads_as_call_syntax` | Drives every tool and asserts no result renders any tool name in call syntax — the test that would have caught the paging footer |
| `structured_output_matches_its_declared_schema` | Payload keys == schema keys, required included |
| `every_exposed_tool_declares_output_schema` | Every exposed tool has a schema row and advertises it |
| `error_code_survives_the_mcp_boundary` | A typed failure reaches the wire with its code |
| `success_keeps_readable_text_and_structured_content` | Text survives alongside structure |

All other gates held: `cargo fmt --check` clean, `cargo clippy --lib
--all-targets -- -D warnings` exit 0, `pnpm typecheck` clean, `pnpm lint` 0
errors, `pnpm build` succeeded.

## Open threads

1. **The live gate is now the top item.** Nothing here has been exercised by a
   real web-AI session. Run the **M1 live gate** through the connector
   (`docs/connector-native-proof-runbook.md`). In particular, confirm a real
   host surfaces `next_offset` rather than ignoring `structuredContent` — some
   clients only read the text block.
2. **`cargo test` still is not a CI gate**, but the blocker is gone: the suite
   is confirmed green, so adding it is now safe and trivial.
3. **`get_handoff` / the `continue_work` prompt** — pull-based continuity is
   still unbuilt.
4. **Phase 2 (`grep`/`glob`) and Phase 8 (LSP diagnostics)** remain the two
   highest-value capability gaps. The round trip is now good enough that
   finding code is the bottleneck rather than reading it.
