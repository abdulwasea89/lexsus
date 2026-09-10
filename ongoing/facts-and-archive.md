# Session Archive & Fact Extraction (F2 + F3) — Complete

> **Status:** ✅ Code complete & verified (31/31 Rust tests, clippy/fmt clean, tsc/eslint/vite clean).
> **Scope:** completes **F2 — Session Archive** and **F3 — Fact Extraction** (Layer 1 + Layer 2 of the architecture), which had been partial since Phase 0.
> **Branch:** `developing`

> **Historical record.** This document was written while Lexsus still used a
> Chrome-extension / loopback-WebSocket transport; that transport was later removed
> for a desktop-local native MCP connector (see `2026-09-10-mcp-native-architecture.md`
> in this directory). Any setup step or architecture description below that refers to
> the extension, pairing codes, or `ws://127.0.0.1:45241` is preserved and no longer applies.

---

## What this delivers

Layer 2 ("structured project memory — facts, not chat") existed only as empty SQLite tables since Phase 0. This work makes both layers real:

- **F2:** every Claude Code transcript for the monitored project is mirrored into the local archive (`sessions` + `session_events`), idempotently keyed by source path + file mtime — parse once per change, never duplicate.
- **F3:** each archived session gets structured facts extracted and persisted into project memory (`objectives`, `decisions`, `attempts`, `constraints`, `changed_files`, `progress`) — decisions, dead ends, and rules that survive into every handoff.

```
~/.claude/projects/<munged>/<uuid>.jsonl   ← Claude Code's own record
        │  transcript.rs (parser, now with timeline events)
        ▼
archive.rs  ── persist_context() ──►  SQLite sessions / session_events   (F2)
        │            └── facts::extract()
        ▼                                    ▼
Handoff card (decisions/constraints/attempts fields)      Memory panel UI   (F3)
```

## Changes by module

### `transcript.rs` — parser upgrades
- New `TranscriptEvent { ts_ms, kind, payload }` timeline (kinds: user / assistant / tool / error / summary), capped at 400 events × 300 chars; timestamps parsed from each line's RFC3339 `timestamp` field via a dependency-free epoch converter (`days_from_civil`, handles `Z` / ±HH:MM offsets, tolerates missing tz).
- New `files_written` (write/edit/notebookedit tools only) distinct from `files_touched` — changed-files facts no longer include mere reads.

### `archive.rs` — NEW (F2)
- `persist_context(conn, ctx)` → upserts a session row (`source` unique index, mtime dedupe), replaces its events, saves extracted facts. Re-running is always safe.
- `archive_project(conn, dir, root)` → mirrors all transcripts for the project (munged dir first, cwd-matching fallback scan); returns an `ArchiveReport {archived, refreshed, skipped}` plus the newest `(session_id, context)`.
- Auto-archive: `build_handoff_impl` persists the newest transcript as a side effect, so Layers 1–2 stay current with zero extra clicks.

### `facts.rs` — NEW (F3)
- Sentence-level heuristics over user/assistant text: decision markers ("decided", "switched to", "instead of", …), failure markers vs success markers (a "tests pass now" sentence never becomes a failed attempt), constraint markers ("must not", "never", "avoid", …).
- Deterministic progress heuristic: written files dominate, commands help, errors penalize (clamped 5–95% when there is activity, 0 otherwise).
- Caps everywhere (10 decisions / 8 attempts / 8 constraints / 240-char sentences) so a chatty session can't balloon the handoff payload.

### `db.rs` — migration `0005_session_archive_v2` + memory API
- `sessions.source/cwd/source_mtime` (+ unique index on source), `session_events.ts_ms`; old DBs migrate in place.
- Archive API: `upsert_session`, `replace_session_events`, `list_sessions`, `session_events_for`, `newest_session_id`.
- Memory API: `save_facts` (delete-then-insert = idempotent re-extraction), `get_facts`, `ProjectFacts`.

### Handoff enrichment
`Handoff` gains optional `decisions` / `failed_attempts` / `constraints` arrays (skipped when empty — extension ignores unknown keys). The clipboard/prompt text now includes them as explicit blocks ("Decisions made", "Failed attempts (do not retry)", "Constraints"), so the next web AI inherits *why*, not just *what*.

## UI

New **Project memory** panel (`MemoryPanel.tsx`) between Activity trace and Handoff:
- **Scan & extract** — one click runs archive + extraction; toast reports refreshed/skipped counts.
- Facts view: objective, top decisions / failed attempts / constraints (color-coded), changed-file chips, heuristic progress.
- Archived sessions list (event counts) → click to expand the last 30 timeline events inline.
- Handoff panel shows an "extracted facts" section whenever facts exist.

## Verification

| Check | Result |
|-------|--------|
| `cargo test` | **31 passed** (13 prior + 18 new: epoch parsing, event timeline, facts heuristics/caps/progress, db roundtrip idempotency, archive skip-refresh-scan flows) |
| `cargo clippy --all-targets -- -D warnings` | clean |
| `cargo fmt --check` | clean |
| `tsc --noEmit` / `eslint` | clean (0 errors) |
| `vite build` | OK |

## What's still open (unchanged)

- M1 gate live smoke + exit gate validation (5–10 devs).
- F10 compression (Layer 3) — these facts are exactly the input it will summarize.
