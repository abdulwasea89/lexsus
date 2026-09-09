# Claude.ai Native-MCP Proof — Runbook (Stage 3)

> **Goal:** prove the additive Path B connector end-to-end: a real **Claude.ai**
> session invokes Lexsus tools through its **native MCP tool channel** (tool UI,
> no DOM scraping, no composer injection) and results + desktop approvals come
> back through that same channel — while the browser-extension path (Path A)
> stays loaded and working.
> **Everything here is a launch + live-test guide** — the code landed in
> Stage 2 (`src-tauri/src/mcp.rs`, committed on `developing`). This stage is
> manual: it needs your Claude plan, a dev tunnel, and your desktop window.
> Expected total time: ~45–75 min.

**Security invariants that hold throughout (not configurable here):**
- The desktop is the **only** approval authority. Writes, commands, and
  sensitive-path reads resolve only through the **desktop** approval banner
  (source label `mcp`). Claude.ai never sees or grants an approval.
- Grants stay **source-scoped**: a session grant earned by an `web` (extension)
  call never auto-approves an `mcp` call, and vice versa.
- The MCP server binds **loopback only** (`127.0.0.1:45147`). The dev tunnel
  connects **outbound** — no inbound port is opened on your machine.
- The connector surface is **read-only by default** (`allow_write` off). The
  write/command leg below is the exception you temporarily enable.
- Tool results are capped (~140k chars) so Claude never gets an unbounded blob.

---

## 1. What Path B looks like now (Stage 2, committed)

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
  `MCP_PATH` = `/mcp`) — a different port than the extension WS (`45241`).
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
| The repo on `developing` | `git log --oneline -1` ≈ `b37a322` | Stage 2 must be present |
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

1. Sidebar → **Browse folder…** → pick the project you will read (e.g. the
   `lexsus` repo itself). The watcher starts and the Git panel populates —
   this sets `project_root`, against which all MCP tool paths are relative.
2. From the terminal that launched the app you should see
   `[mcp] listening on http://127.0.0.1:45147/mcp`. Confirm the socket:

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
no tunnel; it is the fastest way to confirm Stage 2 is healthy.

## 4. Pre-flight: the two things Stage 3 still needs (temporary, local-only)

These are deliberately **not** committed — Stage 4 replaces them with real
config/UI. Apply them by hand before the tunneled run.

**4a. Allow the tunnel Host.** rmcp validates the inbound `Host` header and
rejects anything not loopback by default (DNS-rebinding guard). A dev tunnel
forwards the **public** host, so without this every request 403s.

In `src-tauri/src/mcp.rs`, `serve()` — right after
`config.json_response = true;` — add your tunnel host:

```rust
// TEMP (Stage 3 proof only): the dev tunnel forwards the public Host
// header; the loopback-only default would reject it.
config.allowed_hosts.push("lexsus-proof.trycloudflare.com".to_string());
```

Use your actual stable subdomain. If you are using a throwaway
`*.trycloudflare.com` URL whose host changes every run, either pin it first
(§5b) or — for this short-lived, read-only, secret-URL proof only — replace the
two lines above with `config.disable_allowed_hosts();` (removes host
validation; do not ship this).

**4b. Enable the write leg.** `mcp_allow_write` seeds false at startup. For the
write/command proof in §7 you need it true **before** Claude re-fetches tools,
so flip it and relaunch:

In `src-tauri/src/lib.rs`, in the `.manage(AppState { … })` block change
`mcp_allow_write: Arc::new(AtomicBool::new(false))` to
`mcp_allow_write: Arc::new(AtomicBool::new(true))`, rebuild, and relaunch the
desktop app. Restore `false` when you are back on the read-only leg. Stage 4's
BridgeView will expose this as a live toggle instead of a rebuild.

After 4a/4b: `cargo clippy --lib --all-targets -- -D warnings` clean, then
relaunch.

## 5. Expose the loopback endpoint over HTTPS

Choose **one** tunnel. Claude connects from Anthropic's cloud, so it needs a
**public HTTPS** URL with a real certificate.

### 5a. Recommended: LocalCan (documented for this use case)

Add a local endpoint `127.0.0.1:45147` → it gives you a stable `https://…` URL
with a real cert. **Give the tunnel a stable subdomain** — do not rely on a
host that reshuffles.

### 5b. cloudflared

Named tunnel (stable host — needed if you ever switch to OAuth):

```bash
cloudflared tunnel --url http://127.0.0.1:45147            # throwaway URL
# or, for a stable host on a domain you control:
cloudflared tunnel login && cloudflared tunnel create lexsus-proof
# put the hostname in the tunnel config, then:
cloudflared tunnel run lexsus-proof
```

Quick tunnels get a random `https://<random>.trycloudflare.com`; copy the exact
host into the §4a patch and re-run. Confirm the endpoint answers:

```bash
curl -i https://<your-host>/mcp           # expect 405/400 with server headers, not 403
```

**Guardrails for this proof (keep them on):**
- **Short TTL.** Tear the tunnel down at the end of the session (§9).
- **Read-only first.** Start with §4b set to `false`; flip to `true` only for
  the §7 write leg, then back off.
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

### Read-only leg (connector is the only thing in the loop; extension still loaded)

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

The extension (Path A) is **not** involved in any of this, and should still be
usable if you re-enable it in another chat.

### Write/approval leg (temporary `allow_write=true`, §4b; restart applied)

Re-add / re-point the connector (or trigger a fresh tool list) so Claude sees
the write tools too. Then, in a **new** chat with the connector enabled:

> Use `write_file` to create `scratch/stage3-proof.md` with the text "Path B
> native proof", then `git_status`.

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
| 6 | Extension still pairs and drives a call when re-enabled | chat screenshot |
| 7 | No approval was ever granted from Claude.ai / the connector page | review desktop audit (source stays `mcp`) |
| 8 | Audit trail shows the whole run | `bridge_audit` / SQLite rows |

**Exit condition (from the plan):** Claude.ai natively invokes Lexsus
`read_file` on a real repo; the result returns through its tool channel;
approval resolves on the desktop; the extension is not in the loop (but still
works when re-enabled).

Capture UX bugs as you go (e.g. schema friction, description quality, approval
banner wording for `mcp` source, timing) — those feed Stage 4.

## 9. Cleanup (do this the same session)

1. Quit the desktop app (kills the endpoint + approvals).
2. Tear down the tunnel (LocalCan stop / `cloudflared tunnel` Ctrl-C).
3. In Claude.ai: remove the custom connector.
4. Revert §4a and §4b patches so the working tree matches the committed
   Stage 2 code again.

## 10. Known deferrals to later stages

- **Live runtime write toggle / connector status in BridgeView** — Stage 4
  (the §4b rebuild becomes a switch, and the connector shows alongside the
  extension pairing card).
- **Configurable secret MCP path + host allow-list** behind the tunnel —
  Stage 4/5 config, not a hardcoded patch.
- **Stable production-grade exposure (hosted gateway + outbound relay)** —
  Stage 5; the extension remains the no-cloud fallback everywhere.
