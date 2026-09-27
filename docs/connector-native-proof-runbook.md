# Claude.ai Native-MCP Proof — Runbook

> **Goal:** prove the connector end-to-end: a real **Claude.ai** session invokes
> Lexsus tools through its **native MCP tool channel** (tool UI, no DOM scraping,
> no composer injection) and results + desktop approvals come back through that
> same channel — with no other transport in the loop.
> **Everything here is a launch + live-test guide** — the connector code lives in
> `src-tauri/src/mcp.rs` (committed on `developing`). This stage is manual: it
> needs your Claude plan, a dev tunnel, and your desktop window.
> Expected total time: ~45–75 min.

**Security invariants that hold throughout (not configurable here):**
- The desktop is the **only** approval authority. Writes, commands, and
  sensitive-path reads resolve only through the **desktop** approval banner
  (source label `mcp`). Claude.ai never sees or grants an approval.
- Grants stay **source-scoped**: a session grant earned by a `desktop` or `web`
  call never auto-approves an `mcp` call, and vice versa.
- The MCP server binds **loopback only** (`127.0.0.1:45147`). The dev tunnel
  connects **outbound** — no inbound port is opened on your machine.
- The connector surface is **read-only by default** (`allow_write` off). The
  write/command leg below is the exception you temporarily enable.
- Tool results are capped (~140k chars) so Claude never gets an unbounded blob.

---

## 1. What the connector looks like now

The MCP server is the **single** remote transport — there is no browser
extension path.

```
 Claude.ai (cloud)
    │  remote-MCP over public HTTPS (custom connector)
    ▼
 dev tunnel (LocalCan / cloudflared)  ──outbound──►  127.0.0.1:45147/mcp
                                                       rmcp Streamable-HTTP server
                                                            │  every call
                                                            ▼
                                      crate::tool_call(&app, tool, source="mcp")
                                                            │
                                  bridge engine: approvals · sensitive paths ·
                                  source-scoped grants · kill switch · audit/DB
```

- Endpoint: `http://127.0.0.1:45147/mcp` (`ADDR` = `127.0.0.1:45147`,
  `MCP_PATH` = `/mcp`).
- Read-only surface (default, `allow_write=false`):
  `list_tools`, `describe_tool`, `read_file`, `list_directory`,
  `read_many_files`, `git_status`.
- Write/command tools (gated behind `allow_write=true`):
  `write_file`, `edit_file`, `multi_edit`, `apply_patch`, `delete_file`,
  `move_file`, `copy_file`, `create_directory`, `run_command`.
- Every descriptor ships its SPECS summary + a per-tool JSON Schema
  `inputSchema` with `readOnlyHint` annotations on the read tools.
- `mcp`-sourced approval waits **≤ 120 s** (`APPROVAL_WAIT_SECS`), under the
  ~300 s connector tool timeout so a pending desktop decision is never killed
  by the provider first.

## 2. Prerequisites (one-time)

| Item | Check | Notes |
|------|-------|-------|
| The repo on `developing` | `git log --oneline -1` ≥ `b37a322` | the connector must be present |
| Rust toolchain | `rustc --version` | ≥ the toolchain CI uses |
| Node / pnpm | `pnpm --version` | the desktop app bundles via pnpm |
| **Claude plan with custom connectors** | Customize → Connectors → "Add custom connector" is visible | **Blocking.** Team/Enterprise orgs may disable custom connectors by policy |
| A dev-tunnel tool | LocalCan, or `cloudflared` | see §5 |
| MCP Inspector (optional, local gate) | [inspector](https://github.com/modelcontextprotocol/inspector) or Claude Desktop | for the no-tunnel smoke test |
| A project to read | a real repo on this machine, e.g. the `lexsus` checkout | bind it as the workspace in §3 |

## 3. Build + launch, and verify the endpoint locally (no tunnel yet)

```bash
cd <repo>/src-tauri
cargo clippy --lib --all-targets -- -D warnings   # should be clean
cd <repo> && pnpm tauri dev                       # first build ~5–10 min
```

When the desktop window opens:

1. Workbench → **Project** → pick the project you will read (e.g. the `lexsus`
   repo itself). The watcher starts and the Git panel populates — this sets
   `project_root`, against which all MCP tool paths are relative.
2. `mcp_status` is the single source of truth for the connector: `listening`,
   `endpoint`, `allow_write`, `workspace`. The app polls it on mount and the
   statusbar renders it; you can also invoke it directly. Expect
   `listening: true`, `endpoint: "http://127.0.0.1:45147/mcp"`, and your chosen
   `workspace`. Confirm the socket too:

   ```bash
   ss -ltn | grep 45147        # LISTEN 127.0.0.1:45147
   ```

3. Leave the desktop app running for the whole proof — it hosts the MCP
   endpoint *and* the approvals.

### 3a. Local smoke (optional but recommended): MCP Inspector or Claude Desktop

Point an MCP client at `http://127.0.0.1:45147/mcp`. With `allow_write` off it
must list exactly the six read-only tools, each with a schema. `read_file`
against a **non-sensitive** file inside the bound project returns content
immediately (Auto approval, no banner). `read_file` of a **sensitive** path
(e.g. `.env`, `~/.aws/credentials`) raises the desktop approval banner — Allow
or Deny resolves it and the result returns. This gate needs no Claude plan and
no tunnel; it is the fastest way to confirm the connector is healthy.

## 4. Pre-flight: the connector is configured from the dashboard

There is **no code patch** and **no env var** for the tunnel host any more: the
connector is started, stopped, re-ported and allowlisted from the dashboard,
and the tunnel is managed there too.

**4a. Allow the tunnel Host.** rmcp validates the inbound `Host` header and
rejects anything not loopback by default (DNS-rebinding guard). When you start
an in-app tunnel (§5), its public host is **auto-allowlisted** and the
connector restarts to apply it — no relaunch. You can also edit the allowlist
directly in the dashboard's **Public tunnel** card (`Apply hosts`); loopback is
always allowed and is the default, and anything beyond it is opt-in, never
hardcoded.

**4b. Enable the write leg.** `mcp_allow_write` seeds false at startup. For the
write/command proof in §7 it must be true **before** Claude re-fetches tools,
so flip the dashboard's **Allow writes & commands** switch — no rebuild, no
relaunch — and flip it back off when you are done with the write leg. (The
`LEXSUS_MCP_ALLOW_WRITE=1` env seed still works for a headless launch.)

## 5. Expose the loopback endpoint over HTTPS

Open the dashboard's **Public tunnel** card and press **Start** on a provider
your machine has (`cloudflared`, `ngrok`, or a custom command). The app spawns
the tunnel, scrapes its public HTTPS URL, prints it in the card, auto-adds the
host to the allowlist and restarts the connector. Claude connects from
Anthropic's cloud, so it needs that **public HTTPS** URL with a real
certificate.

For a **stable** host (needed if you ever switch to OAuth), use a named
tunnel — `cloudflared tunnel create` / `ngrok` reserved domain — and drive it
from the dashboard's **Custom command** field with `{port}` as the placeholder:

```
cloudflared tunnel run lexsus-proof
```

Confirm the endpoint answers through the tunnel:

```bash
curl -i https://<your-host>/mcp           # expect 405/400 with server headers, not 403
```

**Guardrails for this proof (keep them on):**
- **Short TTL.** Tear the tunnel down at the end of the session (§9).
- **Read-only first.** Start with §4b *not* set; add `LEXSUS_MCP_ALLOW_WRITE=1`
  only for the §7 write leg, then off again.
- **One workspace.** The bound `project_root` is the whole blast radius.
- **Kill switch armed.** The desktop's pause control (`bridge_pause`) stops the
  engine immediately; quitting the desktop app also kills the endpoint. Know
  where both are before you start.

> The proof does **not** open an inbound port: the tunnel process dials **out**
> to the tunnel provider and forwards public requests over that connection to
> `127.0.0.1:45147`. Firewall/NAT stay closed.

## 6. Claude.ai: add the custom connector

1. Claude.ai → **Customize** → **Connectors** → **Add custom connector**.
2. **Type:** MCP (Streamable HTTP). **URL:** the public tunnel URL from §5,
   with path: `https://<your-host>/mcp`.
3. **Authentication:** choose **None** for this dev proof — the connector then
   needs no OAuth dance, so you do **not** depend on a stable redirect host.
   (OAuth 2.1 + PKCE is the alternative when the product gate requires it;
   that is when the stable subdomain from §5 becomes mandatory.)
4. **Tools:** let Claude discover them from the server (read-only six appear).
5. Save, then enable the connector on the project/chat you will use.

Expected: Claude lists Lexsus tools with **native tool UI** — names,
descriptions and the JSON schemas from `tools/list`.

## 7. Live proof

### Read-only leg (connector is the only thing in the loop)

In the Claude chat, ask it to work against the bound repo with its Lexsus
tools, e.g.:

> Use your Lexsus tools on this repo. `list_directory` on `docs`, then
> `read_file` `docs/architecture.md`, then `git_status`.

Expect, through Claude's **native** tool channel:
- `list_directory docs` and `git_status` return immediately (Auto approval);
- `read_file docs/architecture.md` returns the file content;
- a **sensitive** read you also attempt (e.g. `.env`) raises the **desktop**
  approval banner labelled `mcp` — Allow/Deny there resolves it and Claude
  gets the result or the denial.

No other transport is involved in any of this.

### Write/approval leg (temporary `allow_write=true`, §4b; restart applied)

Re-add / re-point the connector (or trigger a fresh tool list) so Claude sees
the write tools too. Then, in a **new** chat with the connector enabled:

> Use `write_file` to create `scratch/proof.md` with the text "native MCP
> proof", then `git_status`.

Expect:
- Claude shows the native tool call; the **desktop** approval banner appears
  (source `mcp`, destructive/grantable hints correct for the tool);
- click **Allow** on the desktop **within ~120 s** — the request is waiting on
  you, and after the window it times out and the card expires;
- the write executes, `git_status` then reflects the new file, and the result
  returns through Claude's tool channel.

Also try a **Deny** on one call and confirm Claude receives the denial and
recovers gracefully (it is a caller-visible tool error, not a hang).

## 8. Exit conditions & evidence

Copy these into the record you keep (screenshots + logs):

| # | Check | Evidence |
|---|-------|----------|
| 1 | Claude lists ≥ the six read-only tools natively | screenshot of native tool picker |
| 2 | `read_file` on a real file returns content through the tool channel | chat transcript / screenshot |
| 3 | A sensitive-path read raised the desktop banner and resolved | screenshot of banner + audit row |
| 4 | `write_file` + `git_status` ran after a desktop Allow within the window | transcript + `git log`/`git status` |
| 5 | A Deny returned a graceful error to Claude | transcript |
| 6 | No approval was ever granted from Claude.ai / the connector page | review desktop audit (source stays `mcp`) |
| 7 | Audit trail shows the whole run | `bridge_audit` / SQLite rows |

**Exit condition (from the plan):** Claude.ai natively invokes Lexsus
`read_file` on a real repo; the result returns through its tool channel;
approval resolves on the desktop; no other transport is in the loop.

Capture UX bugs as you go (e.g. schema friction, description quality, approval
banner wording for `mcp` source, timing) — those feed the next stage.

## 9. Cleanup (do this the same session)

1. In the dashboard, press **Stop tunnel** (withdraws the host from the
   allowlist and kills the tunnel process group).
2. Quit the desktop app (kills the endpoint + approvals; any tunnel is also
   stopped on exit).
3. In Claude.ai: remove the custom connector.
4. Unset `LEXSUS_MCP_ALLOW_WRITE` if you used it, or confirm the dashboard's
   write switch is back off, so the app matches its default read-only posture.

## 10. Known deferrals to later stages

- **Handoff delivery over the connector** — MCP is pull-based and cannot push
  into a chat, so the handoff is built and copied to the clipboard from the
  Handoff view today. A `get_handoff` connector **pull tool** is the planned
  replacement; it is not built yet.
- **Configurable secret MCP path** — the port and host allow-list are now
  configurable from the dashboard; a configurable secret path (`/mcp`) is
  still hardcoded.
- **Stable production-grade exposure (hosted gateway + outbound relay)** — the
  loopback endpoint plus a dev tunnel is the proof-time path.
