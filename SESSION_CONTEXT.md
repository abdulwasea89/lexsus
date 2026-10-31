# Lexsus session context

## Current work — version 1 beta plan, 31 October 2026

The user requested implementation of the agreed desktop-first beta plan: reviewed read-only handoffs for Claude and ChatGPT desktop on Linux, Windows and macOS. Both clients and all three platforms are beta gates; provider approval and actual host testing remain unverified. No remote relay, live editing or commands are in beta scope. Preserve the substantial pre-existing staged, unstaged and untracked changes.

Baseline at dirty revision `7dd51aa`: `pnpm run typecheck` PASS; `cargo fmt --all -- --check` PASS. Initial `pnpm run lint` failed because ESLint scanned generated `src-tauri/target` assets; the ignore was corrected and lint passed with six existing warnings. `cargo test --manifest-path src-tauri/Cargo.toml --lib --offline` initially had 190 passes and nine failures, including three loopback binds blocked by sandbox. An unrestricted rerun had 192 passes and seven failures: two manifest/meta-tool tests, one LSP LocationLink test, two MCP approval-policy tests, connector startup timing, and an incomplete no-write test fixture. These were baseline results before the latest fixes.

Changed MCP policy to gate `propose_plan` when writes are disabled; fixed the LSP LocationLink URI field; moved the listening flag before the connector startup acknowledgement. `pnpm run build` PASS and `git diff --check` PASS. The Rust rerun after these edits was interrupted while compiling (exit 130); fixes are not yet verified by tests. The reviewed handoff, scoped profile, client integrations, installers, pilot and release gates remain unfinished. Next: rerun `cargo test --manifest-path src-tauri/Cargo.toml --lib --offline` with loopback access, address remaining baseline failures without weakening checks, then build the reviewed handoff and client integration in the agreed sequence.

Updated: 17 October 2026. This is a replaceable handoff note, not a product specification or authorization to execute pending work. Verify against the current checkout and user request on resume.

## Current objective

Complete the desktop foundation: harden the app shell and packaging, repair build blockers, validate available checks, and record platform acceptance limits. The user explicitly requested this implementation and then asked to continue after interruption.

## Product direction

Lexsus preserves reviewable coding-task state so another supported agent can continue an interrupted task. The Lexsus 2.0 PRD prioritizes capture → evidence review → bounded handoff → local continuation → verified useful progress. Remote read-only snapshots follow local validation. See `lexsus.md` for requirements and release gates.

## Completed in this session

- Created `docs/lexsus-full-report-2026-10-17.md` from a source review, covering purpose, features, status, gaps and priorities.
- Read the supplied instructions, PRD and engineering handbook; reread `AGENTS.md` after the user renamed the original `AGENT.md`.
- Added a required startup reading order and session-maintenance rules to `AGENTS.md`.
- Linked the PRD and handbook to the shared reading contract.
- Added `CLAUDE.md` and `GEMINI.md` as short entry points to the same rules.
- Corrected the instruction file's frontend command examples to the repository's pnpm scripts.
- Created `docs/build-progress.md`: implemented-component checklists, the 58-tool inventory with delegation marked unavailable, full-plan area coverage, and unchecked G0–G6 acceptance gates. The earlier 35–45% estimate is explicitly an unmeasured planning judgment.
- Desktop foundation changes: Lexsus product/window metadata; retained application identifier for data compatibility; pnpm build hooks; minimum window dimensions; production/dev CSP; app-wide render recovery; workspace startup retry and storage tolerance; accessible OAuth labels/copy button.
- Added `.github/workflows/desktop-build.yml` for unsigned DEB/NSIS/DMG artifacts and `docs/desktop-foundation.md` for build and installation acceptance.
- Repaired the existing bridge module split (imports, helper visibility, misplaced command-event derives) and added a command-event wire-contract regression test. Updated outdated OAuth test constructors to isolated token fixtures; preserved production authentication behavior.
- pnpm's dependency verification automatically refreshed the stale lockfile to match the current manifest; this was reported to the user and then validated with a frozen offline lockfile-only install.

## Decisions and constraints

- `AGENTS.md` is the canonical repository instruction file; avoid competing copies.
- `lexsus.md` owns intended behavior; source and actual verification establish implemented behavior.
- `ENGINEERING_STANDARDS.md` owns detailed design and review guidance.
- This file owns temporary progress; replace stale notes after meaningful milestones.
- Preserve all existing staged and unstaged user changes. The checkout already contained substantial UI and Rust changes before this documentation task.
- Keep secrets, raw transcripts and credentials out of memory files.

## Verification and known limits

- Frontend typecheck/build passed; lint passed with six existing warnings. Rust `cargo check --manifest-path src-tauri/Cargo.toml --offline` passed after bridge repairs. Rust formatting and diff whitespace checks pass.
- React Doctor rerun reports one remaining static redirect-URI index-key warning, with the three accessibility findings fixed (70/100 for the scanned changes).
- Native debug DEB build and Rust tests are in progress; do not mark them passed until actual completion is recorded. The initial test attempt exposed outdated OAuth fixture calls and failed compilation.
- No native GUI launch, clean installation, Windows/macOS packaging run, signing, startup timing, or real-provider validation has been performed.
- Historical project status contains conflicting test results and obsolete UI descriptions. No current green baseline is established by this session.
- The earlier source review found a single-dashboard UI, 58 registered tool names, unavailable delegation, and a compression endpoint returning 501. Recheck these observations before relying on them.
- No agent host's automatic loading behavior was tested. Persistence is through repository files, within hosts that read these instructions.

## Pending work / next action

Finish the in-progress native build and tests, inspect their outputs, repair any remaining build blockers, and update the validation record in `docs/desktop-foundation.md`. Keep platform installation acceptance unverified until observed. Preserve all pre-existing user changes.
