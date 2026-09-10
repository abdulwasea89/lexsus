# Trade-off Summary

| Decision | Chosen | Rejected | Why |
|----------|--------|----------|-----|
| Shell | Tauri | Electron | Smaller, safer; Electron acceptable if speed-to-prototype wins |
| Systems language | Rust only | Rust + C | C adds memory-safety risk with no capability gain here |
| Local agent capture | PTY-wrap CLI (Claude Code) | Raw API-only | Keeps the native tool-use loop; API-only forces rebuilding it |
| Web AI integration | Native MCP connector (loopback, built on `rmcp`) | Browser automation / DOM scraping | DOM scraping breaks on UI redesigns — flagged 10/10 platform risk. The extension that once relayed calls was also removed; the connector speaks the provider's own tool channel |
| State store | SQLite | Postgres / cloud DB | Local-first privacy requires no data leaving the machine |
| Search | Structured queries | Vector DB | Premature at MVP scale; add only if memory volume demands it |
| Git control | In-app git panel via `git2` | External git process / terminal-only | Full workflow (status/diff/stage/branch/history/commit) from the app |

## Key Risks

- **Web AI integration (10/10 risk)** — DOM scraping / browser automation was rejected early: it breaks on UI redesigns and anti-automation. A browser extension + local IPC relay superseded it, but has since been removed too. The risk is now uneven, rollout-gated provider connector support; it is mitigated by speaking the provider's **native** MCP tool channel rather than the DOM, exposing the connector read-only first and behind the desktop's approval gate.
- **Command execution risk** — app reads/writes files and runs commands; requires per-tool permissions, command approval, sandboxing, and audit logs.
- **Secret handling** — app touches source code, git history, and potentially credentials; requires minimal attack surface and careful secret handling.
