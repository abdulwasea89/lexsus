# src/ — Frontend (React / TypeScript)

React 19 + TypeScript + Vite frontend for the Tauri 2 desktop shell. It renders
the dashboard and talks to the Rust core exclusively over Tauri IPC.

## Commands

```bash
pnpm dev          # Vite dev server on :1420 (the Tauri dev URL)
pnpm build        # tsc + vite build → ../dist
pnpm typecheck    # tsc --noEmit
pnpm lint         # eslint .
```

Run the full desktop app from the repo root with `pnpm tauri dev`.

## Layout

The app is a single screen. `App.tsx` renders the titlebar, the four global
banners, the dashboard, the project dialog and the status bar.

- `src/views/DashboardView.tsx` — the whole main surface: the KPI strip, the
  MCP connector card, the public tunnel card, and the collapsible sections
  (OAuth clients, tool surface, activity by kind, files, commands, audit
  trail, recent activity). All of it reads from one `dashboard_activity`
  round-trip.
- `src/components/` — `Titlebar`, `Statusbar`, `ApprovalBanner`,
  `QuestionBanner`, `GrantsBar`, `FailoverBanner`, `Onboarding`,
  `ProjectDialog` (project + connector setup), `ErrorBoundary`, and the
  `ui/` primitives (shadcn-style, on Base UI).
- `src/hooks/` — `useApprovals`, `useQuestions`, `useFailover`, `useTheme`,
  `useTauriEvent`.
- `src/lib/bridge.ts` — typed wrappers over the Tauri commands (the IPC boundary).
- `src/lib/types.ts` — the shared types mirrored from the Rust core.
- `src/App.tsx` — shell: banner composition, project selection, watcher wiring.

## What was removed

Commit `31758dc` folded the workbench into the dashboard. `BridgeView`,
`GitView`, `MemoryView`, `HandoffView`, `TraceView`, `WorkbenchRail`,
`TerminalPane` and `GettingStarted` were unreachable and have been deleted,
along with the `ui/` primitives only they used (`dropdown-menu`, `select`,
`tabs`, `label`, `collapsible`, `empty`, `alert`).

The capabilities those views surfaced did not go with them: `git_*`, the PTY
tools, the LSP tools, the handoff and the project-memory tools are all still
on the Rust side and still reachable by the web AI over MCP. What was removed
is the duplicated desktop surface, not the feature.

_See `docs/ui-design.md` for the design and `docs/architecture.md` for the system._