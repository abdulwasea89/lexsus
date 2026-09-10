# src/ — Frontend (React / TypeScript)

React 19 + TypeScript + Vite frontend for the Tauri 2 desktop shell. It renders
the workbench — trace, git, handoff, memory, and connector views — and talks to
the Rust core exclusively over Tauri IPC.

## Commands

```bash
pnpm dev          # Vite dev server on :1420 (the Tauri dev URL)
pnpm build        # tsc + vite build → ../dist
pnpm typecheck    # tsc --noEmit
pnpm lint         # eslint .
```

Run the full desktop app from the repo root with `pnpm tauri dev`.

## Layout

- `src/views/` — the right-hand workbench views:
  - `TraceView` — live activity trace (reads, writes, commands cross-checked
    against the filesystem watcher).
  - `GitView` — git panel: status, diff, stage/unstage, branches, history, commit.
  - `HandoffView` — build the handoff snapshot and copy it into a web AI's chat.
  - `MemoryView` — sessions archive and structured project facts.
  - `BridgeView` — tool inventory, audit trail, and the connector's write switch.
  - `ViewShell` — shared chrome (header strip over a scrolling body) for the views.
- `src/components/` — `Titlebar`, `WorkbenchRail`, `Statusbar`, `TerminalPane`,
  `ApprovalBanner`, `GrantsBar`, `FailoverBanner`, `ProjectDialog` (project +
  connector setup), and the `ui/` primitives.
- `src/hooks/` — `useApprovals` (approval/grant state) and `useTheme`.
- `src/lib/bridge.ts` — typed wrappers over the Tauri commands (the IPC boundary).
- `src/lib/types.ts` — the shared types mirrored from the Rust core.
- `src/App.tsx` — workbench shell: view routing, project selection, watcher wiring.

_See `docs/ui-design.md` for the design and `docs/architecture.md` for the system._
