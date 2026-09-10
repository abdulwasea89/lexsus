# Windows Prototype Runbook — Local PC ⇄ chatgpt.com

> **Goal:** a working end-to-end prototype on your Windows machine: work locally in Claude Code, hand off to ChatGPT in Chrome, and let it read / write / run commands on this PC through the bridge.
> **Everything is already built** (`src-tauri` core + `extension/`). This is a launch + live-test guide. Expected total time: ~30–45 min.

> **Historical record — do not follow these steps as written.** This runbook was
> written while Lexsus still used a Chrome-extension / loopback-WebSocket transport;
> that transport was later removed for a desktop-local native MCP connector (see
> `2026-09-10-mcp-native-architecture.md` in this directory). Every step below —
> the extension, the pairing code, `ws://127.0.0.1:45241` — is superseded and no longer applies to today's build.

---

## 1. Prerequisites (one-time)

| Tool | Check | Install if missing |
|------|-------|--------------------|
| Rust (MSVC) | `rustc --version` ≥ 1.97 | https://rustup.rs (default windows-msvc toolchain) |
| VS Build Tools | folder `C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools` exists | VS Installer → "Desktop development with C++" workload |
| Node LTS | `node --version` | https://nodejs.org |
| pnpm | `pnpm --version` | `corepack enable` then `corepack prepare pnpm@latest --activate` |
| Google Chrome | installed | https://chrome.google.com |
| WebView2 Runtime | preinstalled on Win 10 21H2+/Win 11 | https://developer.microsoft.com/microsoft-edge/webview2 |

## 2. Get the code

```powershell
git clone https://github.com/abdulwasea89/lexsus.git lexsus
cd lexsus
git checkout developing        # prototype branch
pnpm install
```

## 3. Run the desktop app

PowerShell can't load the MSVC env directly; use the proven wrapper (from `ongoing/phase-1-mvp.md` §7):

```powershell
cmd /c "call ""C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"" >nul && set PATH=%USERPROFILE%\.cargo\bin;%PATH% && pnpm tauri dev"
```

First build takes ~5–10 min. When the window opens:

1. Sidebar → **Browse folder…** → pick a real project (e.g. the lexsus repo itself).
2. Watcher starts; **Git panel** shows status; pairing code appears in the sidebar.
3. Leave it running.

> If port 45241 is busy from an earlier run: `Get-Process | Where-Object {$_.ProcessName -eq "lexsus"}` … or just reboot the app; the WS server binds `127.0.0.1:45241`.

## 4. Load the extension

1. Chrome → `chrome://extensions` → enable **Developer mode** (top right).
2. **Load unpacked** → select the `extension/` folder inside the repo.
3. Pin it. Click the icon → popup opens.

## 5. Pair (once per browser profile)

1. Copy the **6-digit code** from the desktop sidebar into the popup → **Pair**.
2. Both dots turn green (popup: *Paired · Connected*; desktop chip: *Paired*).
   - The service worker reconnects automatically if Chrome suspends it; re-enter the code only after a full Chrome restart.

## 6. The live test (the actual prototype)

Work with ChatGPT like a remote pair-programmer. Suggested first session:

| # | Do | Expect |
|---|----|--------|
| 1 | In the desktop **Handoff** panel set an objective ("add a README badge"), click **Continue with ChatGPT** | A dark **"Bridge handoff ready"** card appears at the top of chatgpt.com |
| 2 | Click **Continue with ChatGPT** on that card | The agent prompt lands in the composer and sends |
| 3 | Ask ChatGPT: *"read the README first, then propose the change"* | It replies containing `read_file("README.md")` → within ~1 s a result widget appears with the real file content; click **Insert result into chat** |
| 4 | Ask it to apply the edit | It emits an ```acb JSON block → desktop shows an **approval card** (writes always ask) |
| 5 | Click **Allow** (desktop or the page widget) | File changes on disk; activity trace shows the edit flip to **✓ saved** once the watcher confirms |
| 6 | Ask: *"run `git status` to verify"* | `run_command` approval → Allow → **output streams live into the desktop terminal pane**, exit code shown, result returned to ChatGPT |
| 7 | Desktop **Git panel**: stage the file, commit | Your repo changed by ChatGPT, committed from the app |
| 8 | Mid-task, stop replying for a moment; press **Rebuild** in Handoff panel | Card now reflects ChatGPT's own work (files touched, errors) |

**Tool formats ChatGPT may emit (both supported):**

```
read_file("src/app.ts")                    ← bare line
run_command("cargo test")

```acb                                      ← block form (required for writes)
{"tool":"write_file","path":"src/x.ts","content":"...multi-line..."}
```
 ```

Every call is permission-gated: reads auto-approve except sensitive paths (`.env`, keys, credentials); **writes and commands always require your Allow/Deny**. Everything is logged (Bridge panel → audit trail).

## 7. Troubleshooting

| Symptom | Fix |
|---------|-----|
| Popup says *"app not reachable"* | App not running / WS not up — relaunch §3; check firewall isn't blocking loopback (rare) |
| Pair fails instantly | Code rotated (app restarted regenerates only if DB cleared) — copy the current one from the sidebar |
| Handoff card never appears | Confirm paired dot is green; hard-reload the chatgpt.com tab (Ctrl+Shift+R) so content.js reinjects |
| ChatGPT ignores tool format | Paste: *"Use the bridge tools exactly as instructed: bare tool lines, and an acb fenced JSON block for write_file."* |
| Tool fires but nothing happens | Check Bridge panel audit trail — likely a **Deny** or sensitive-path block |
| Extension went quiet after ~30 s idle | Normal MV3 sleep; the worker wakes on next WS ping watchdog cycle (≤45 s). If stuck, click the extension icon |
| `link.exe not found` during cargo build | vcvars64.bat wasn't called — use the exact §3 command |
| ChatGPT DOM selectors drift | Tell me what you see (screenshot/console errors) — selector fixes are one-line changes in `extension/content.js` |

## 8. What "prototype works" means (M1 gate)

A real interrupted task continued by ChatGPT with genuine local tool access — no re-explaining. Record the outcome in `ongoing/phase-1-mvp.md` §Progress when done.
