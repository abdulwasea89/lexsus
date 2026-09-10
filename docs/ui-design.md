# UI Design — The Desktop Control Center

The desktop app is the **control center** — one place where you see everything happening to your work. It combines a live activity trace, a single command terminal (the web AI's), and a full git workflow, all correlated in a single timeline, with the connector's state and every approval gate in the same window.

## Control Center Layout

```
┌──────────────────────────────────────────────────────────────────┐
│  RAIL          │  MAIN PANEL                                      │
│                │  ┌──────────────────────┬──────────────────────┐  │
│  Activity      │  │  COMMAND TERMINAL    │  LIVE ACTIVITY TRACE │  │
│  Git           │  │  $ npm test          │  ▾ Refactor auth     │  │
│  Handoff       │  │  PASS auth.test.ts   │      Reading auth.ts │  │
│  Memory        │  │                      │      Editing auth.ts │  │
│  Connector     │  │                      │      ▾ npm test      │  │
│                │  └──────────────────────┴──────────────────────┘  │
│  ────────────  │  ┌─────────────────────────────────────────────┐  │
│  Project &     │  │  APPROVAL BANNER (only when a call waits)   │  │
│   connector    │  │  source · summary · Allow / Deny · grant    │  │
│  Connector ·   │  └─────────────────────────────────────────────┘  │
│   read-only    │                                                  │
│                │  STATUS BAR: myapp · local working · web working │
│                │              · connector · ro · terminal idle    │
└────────────────┴──────────────────────────────────────────────────┘
```

- **Rail:** the app's navigation — Live activity trace, Git, Handoff, Project memory, Web-AI connector — plus **Project & connector** (folder picker + endpoint) at the bottom. Collapsible to icons only.
- **Live Activity Trace:** real-time tree of every web-AI action.
- **Command Terminal:** the single terminal in the app — a read-only live view of every command the web AI runs.
- **Approval Banner:** the gate. It appears above the panes only when a call is waiting on you.
- **Status Bar:** project, failover states, connector state, terminal state.

## Project & Connector

The rail's **Project & connector** entry opens one dialog that does two related things, because they are one decision:

- **Bind the workspace** — pick the folder the web AI works on. This is the connector's entire blast radius: every tool path is resolved against it (`resolve_path`), and nothing outside it is reachable.
- **Point a connector at it** — the dialog shows the live endpoint and whether the server is **Listening** or **Offline**, with the copy-paste instructions for adding a remote-MCP connector (Claude.ai → Customize → Connectors → *Add custom connector*). There is nothing to pair and no code to type: the address is the whole configuration.

The same state is mirrored in the **Web-AI connector** view, which is also the diagnostics surface (tool sandbox + audit trail). Its connector block shows:

| Row | Shows |
|---|---|
| **Endpoint** | `http://127.0.0.1:45147/mcp` — the live MCP endpoint, from `mcp_status` |
| **Workspace** | the bound project root, or `no project bound` |
| **Allow writes & commands** | the `mcp_allow_write` switch — off means the read-only six-tool surface; on exposes the nine write/command tools. Flipped live via `mcp_set_allow_write`: no rebuild, no reconnect, and every write still needs the desktop's approval |

## Live Activity Trace

As the agent (local or web AI) works, every observed action becomes a node in a live step tree.

```
▾ Session: Refactor auth (web AI)
        Reading auth.ts
        Reading db/schema.sql
    ✏   Editing auth.ts           [3 lines changed]
    ▾     Running: npm test
        ❌ 2 failing — auth.test.ts
    ✏   Editing auth.test.ts
    ▾     Running: npm test
        ✅ All passing
```

Each line is derived from two correlated signals:

1. **Bridge tool-call records** — what the web AI read, wrote, or ran
2. **Independent filesystem watcher** — confirming a file's mtime actually changed

Cross-referencing both avoids showing "wrote a file" when the agent merely printed a code block without saving.

## Headroom — Collapsing Process, Maximizing Outcome

The core UX problem: a long session generates far more step-detail than a user wants visible at once, but the summary/outcome should always stay prominent.

### Headroom Pattern

- **Default collapsed state** — only the current/most recent 2–3 steps are expanded; older steps auto-collapse into a single summarized line ("12 earlier steps — 6 files touched, 1 error resolved").
- **Reserved space for outcome** — the process trace occupies a shrinking, scrollable region at the top; the live "what's happening now" line and eventual handoff summary always keep guaranteed space at the bottom.
- **Auto-promotion on interruption** — the same step data collapses directly into the handoff card ("64% progress, 8 files changed, 3 errors remaining") — nothing is re-derived.
- **Expand-on-demand** — clicking any collapsed group re-expands its steps, useful for debugging without cluttering normal operation.

## Git Panel (full git workflow from the app)

- **Status:** working-tree changes, staged vs unstaged.
- **Diff:** see exactly what changed in each file (inline, colored).
- **Stage/Unstage:** stage files or hunks with a click.
- **Branch:** view branches, switch branches, see current branch.
- **History:** browse commit history, view past commits/diffs.
- **Commit from the app:** write a commit message, preview staged changes, click **Commit** (via `git2`); optional push.

## Command Terminal (the web AI's)

The app has exactly **one terminal** — the read-only live view of the commands the web AI runs on the project:

- Every approved `run_command` streams in as it executes: a `$ command` header, the live output, and a final exit/timed-out/truncated status.
- It is a **monitor, not a session**: there is no embedded shell or Claude Code pane, and no keyboard input. The user's control point is the **approval cards** (Allow/Deny) — they see each command the moment it starts and decide before it touches the machine.
- The user works in their own terminal (e.g. Claude Code) as usual; the app only watches what the *web AI* does.
- Everything shown is correlated into the unified timeline with the activity trace.

## Web AI Activity View

When a web AI is acting as a coding agent (attached through the MCP connector), the control center shows what it is doing *with your machine* — like ChatGPT's `> thinking` view, but grounded to real operations:

- "ChatGPT read auth.ts"
- "ChatGPT wrote auth.ts"
- "ChatGPT ran npm test → 2 failing"

These appear in the activity trace, the real output appears in the terminal pane, and the resulting changes appear in the git panel — where you can commit them from the app. The connector is pull-based, so the app never tries to reach back into the chat: the handoff is surfaced here (and copied to the clipboard from the Handoff view) instead of being injected into a conversation.
