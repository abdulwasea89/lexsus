# Infrastructure

## Components

| Component | Tech | Runtime | Port |
|-----------|------|---------|------|
| Desktop App | Tauri 2 + React + TypeScript | Native (host) | — |
| Rust Core | Rust (git2, rusqlite, notify, tungstenite) | Native (host) | — |
| Chrome Extension | MV3 (JS) | Browser | — |
| Compression Service | Python 3.12 + FastAPI + LangChain | Docker | 8000 |

## Networking

- **WebSocket (extension ↔ app):** `ws://127.0.0.1:45241` — loopback-only, 6-digit pairing code
- **Compression service:** `http://localhost:8000` — local only
- **SQLite:** Embedded, file-based — no network

## Docker

The only containerized service is the **compression-service** (Layer 3).

```bash
# Start
docker compose up -d

# Stop
docker compose down

# Logs
docker compose logs -f compression-service
```

Environment variables (set in `.env`):

| Variable | Default | Description |
|----------|---------|-------------|
| `AICB_LLM_PROVIDER` | `openai` | LLM provider for compression |
| `AICB_LLM_MODEL` | `gpt-4o-mini` | Model used for summarization |

## Local Development

```bash
# Frontend + Tauri core
pnpm install
pnpm tauri dev

# Compression service (without Docker)
pip install -r compression-service/requirements.txt
uvicorn main:app --port 8000 --app-dir compression-service
```

## Data Flow

```
Chrome Extension ──WebSocket──▶ Rust Core ──SQLite──▶ Session Archive
                                │
                                ├──▶ Live Activity Trace (React UI)
                                └──▶ Compression Service (optional, Docker)
```

## Security

- All traffic is loopback (`127.0.0.1`) — no external exposure
- `write_file` and `run_command` require user approval (Allow/Deny)
- No Electron — minimal attack surface via Tauri
- Secrets via `.env` (not committed)
