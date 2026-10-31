# UI Design — The Dashboard

The desktop app is a **single dashboard**. One window answers the only three questions that matter while a web AI is acting as a coding agent on your machine: *is it connected, what is it doing to my files, and what do I approve?*

There is no navigation rail and no view switcher. An earlier iteration had a rail with five views (activity trace, git, handoff, project memory, connector) plus a read-only terminal; commit `31758dc` folded that into one dashboard and the separate views were removed. The git, terminal, LSP, handoff and memory capabilities did not go with them — they remain MCP tools the web AI calls, and the `git_*` commands remain on the Tauri IPC surface. What changed is that the desktop app no longer duplicates surfaces the connector already owns.

## Layout

```
┌──────────────────────────────────────────────────────────────────────┐
│ ● ● ●   AI Continuity Bridge                              ☾  ⤢  ✕   │  titlebar (frameless)
├──────────────────────────────────────────────────────────────────────┤
│  APPROVAL BANNER   — only while a call is waiting on you             │
│  QUESTION BANNER   — ask_user / propose_plan cards                  │
│  GRANTS BAR        — revocable grants + the pause kill switch       │
│  FAILOVER BANNER   — stalled / interrupted / ws_drop alerts         │
├──────────────────────────────────────────────────────────────────────┤
│  Dashboard                                          [Refresh]       │
│  ┌────┐┌────┐┌────┐┌────┐┌────┐┌────┐                             │
│  │Tools││Calls││Read││Write││Cmds││Fail│  KPI strip                 │
│  └────┘└────┘└────┘└────┘└────┘└────┘                             │
│                                                                      │
│  ┌──────────────────────────────────────────────┐                    │
│  │  MCP connector              [power] [ro/rw] │                    │
│  │  http://127.0.0.1:45147/mcp          [copy]  │                    │
│  │  port · allowed hosts · read-only switch     │                    │
│  └──────────────────────────────────────────────┘                    │
│  ┌──────────────────────────────────────────────┐                    │
│  │  Public tunnel                     [start]   │                    │
│  └──────────────────────────────────────────────┘                    │
│                                                                      │
│  ▾ OAuth clients ────────────────────────────────────────  collapsed │
│  ▾ Tool surface ─────────────────────────────────────────  collapsed │
│  ▾ Activity by kind ────────────────────────────────────  collapsed │
│  ▾ Files ───────────────────────────────────────────────  collapsed │
│  ▾ Commands ────────────────────────────────────────────  collapsed │
│  ▸ Audit trail ──────────────────────────────────────────  collapsed │
│  ┌──────────────────────────────────────────────┐                    │
│  │  Recent activity                              │  always visible  │
│  └──────────────────────────────────────────────┘                    │
├──────────────────────────────────────────────────────────────────────┤
│  myapp · local working · web working · connector · rw · online      │  statusbar
└──────────────────────────────────────────────────────────────────────┘
```

## The four banners

They sit above the dashboard and are the only things that can interrupt it. Each appears only when it has something to say, so a quiet session shows none of them.

- **Approval banner** — the gate. Appears when a tool call is blocked on a decision. Shows the resolved absolute path for anything destructive, and offers "don't ask again for edits under *X/*" which mints a source-scoped session grant.
- **Question banner** — the `ask_user` and `propose_plan` tools render as cards: option buttons plus a free-text field.
- **Grants bar** — every live grant as a revocable chip, plus **Revoke all & pause**, the kill switch.
- **Failover banner** — the local/web activity monitors in `failover.rs` detected a stall, an interruption, or a dropped web socket. Dismissible per agent.

## Connector card

The card is the app's power switch and its only configuration surface.

| Row | Shows |
|---|---|
| **Power** | start / stop the loopback MCP server, live (`mcp_start` / `mcp_stop`) |
| **Endpoint** | `http://127.0.0.1:45147/mcp` with a copy button, and the Claude.ai steps for adding a custom connector |
| **Port / allowed hosts** | editable; changing either marks the server as needing `mcp_restart` to apply |
| **Read-only** | the `mcp_allow_write` switch. Off exposes the read-only surface; on exposes the write and command tools. Flipped live — no rebuild, no reconnect — and every write still needs the desktop's approval |

## Collapsible sections

Each is a card whose body is closed by default, so the dashboard opens at KPI-strip height regardless of how much history has accumulated.

- **OAuth clients** — create, reveal-once, rotate and delete OAuth 2.1 clients with PKCE for the connector.
- **Tool surface** — the 58 registered tools broken down by manifest group, read from `activity_tool_surface` (`SPECS` in `bridge/specs.rs`). This is the app's answer to "what can this thing actually do to my machine".
- **Activity by kind** — tool calls bucketed by kind.
- **Files** — reads vs writes per path.
- **Commands** — runs and their last outcome.
- **Audit trail** — every approval decision, allowed or denied, with its arguments and result summary.
- **Recent activity** — the always-visible tail of the same data.

All seven read from one `dashboard_activity` round-trip rather than seven separate commands, and refresh on a 30-second interval plus a 1-second debounce on `trace://step` bursts.

## Project & connector dialog

Opened from the dashboard header. One dialog, because binding a workspace and pointing a connector at it are one decision:

- **Bind the workspace** — pick the folder the web AI works on. This is the connector's entire blast radius: every tool path resolves against it via `resolve_path`, and nothing outside it is reachable.
- **Point a connector at it** — the dialog shows the live endpoint and whether the server is **Listening** or **Offline**. There is nothing to pair and no code to type; the address is the whole configuration.

## Signals and correlation

Activity is derived from two independent sources and cross-referenced:

1. **Bridge tool-call records** — what the web AI read, wrote, or ran.
2. **A filesystem watcher** — confirming a file's mtime actually changed.

Correlating both avoids showing "wrote a file" when the agent merely printed a code block without saving.

The connector is pull-based: the app never reaches back into the chat window. The handoff is built from SQLite state and copied to the clipboard by the user.

## Onboarding

First run only, gated on the `lexsus.onboarded` key: a typewriter intro followed by a seven-slide tour. Dismissal is permanent.

## What is deliberately absent

- **No embedded terminal.** The user runs their own agent in their own terminal. The app's control point is the approval cards, not a shell it hosts.
- **No git panel.** `git_*` stays reachable over MCP; a second UI for it duplicated the connector's surface without adding reach.
- **No polling spinner theater.** One batched command, one interval, one event stream.