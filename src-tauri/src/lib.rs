pub mod archive;

pub mod bridge;

pub mod db;

pub mod failover;

pub mod facts;

pub mod git;

pub mod pty;

pub mod process;

pub mod shell;

pub mod transcript;

pub mod watcher;

pub mod ws;

use std::collections::VecDeque;

use std::path::PathBuf;

use std::sync::atomic::AtomicBool;

use std::sync::{Arc, Mutex};

use std::thread;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager, State};

/// App-managed shared state: the SQLite connection, the watched project
/// root, the extension WebSocket, and the approval queue.
pub(crate) struct AppState {
    pub(crate) conn: Mutex<rusqlite::Connection>,
    pub(crate) project_root: Mutex<Option<PathBuf>>,
    pub(crate) pair_code: Mutex<String>,
    pub(crate) ws_connected: AtomicBool,
    pub(crate) ws_tx: Mutex<Option<Arc<Mutex<tungstenite::WebSocket<std::net::TcpStream>>>>>,
    pub(crate) bridge: Mutex<bridge::Bridge>,
    pub(crate) objective: Mutex<Option<String>>,
    /// Recent "editing X" steps, for watcher cross-correlation.
    pub(crate) recent_edits: Mutex<VecDeque<(String, Instant)>>,
    /// Automatic-failover state machines (local + web directions).
    pub(crate) failover: Mutex<failover::ActivityMonitor>,
}

/// A trace step emitted to the UI (mirrors `TraceStep` in the frontend).
#[derive(Clone, serde::Serialize)]
struct TraceStepEvent {
    kind: String,
    file: Option<String>,
    command: Option<String>,
    detail: Option<String>,
    confirmed: bool,
    agent: String,
    ts: u64,
}

// --- commands ----------------------------------------------------------------

#[tauri::command]
fn init_database(state: State<'_, AppState>, db_path: String) -> Result<Vec<String>, String> {
    let conn = db::open_and_migrate(std::path::Path::new(&db_path)).map_err(|e| e.to_string())?;
    let applied = db::applied_versions(&conn).map_err(|e| e.to_string())?;
    *state.conn.lock().unwrap() = conn;
    Ok(applied)
}

/// Set the project folder this app monitors (persisted).
#[tauri::command]
fn set_project_root(state: State<'_, AppState>, path: String) -> Result<(), String> {
    let p = std::path::PathBuf::from(path);
    if !p.is_dir() {
        return Err(format!("not a directory: {}", p.display()));
    }
    *state.project_root.lock().unwrap() = Some(p.clone());
    let _ = db::set_setting(
        &state.conn.lock().unwrap(),
        "project_root",
        &p.display().to_string(),
    );
    Ok(())
}

/// Restore the persisted project root (frontend calls on startup).
#[tauri::command]
fn get_project_root(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state
        .project_root
        .lock()
        .unwrap()
        .clone()
        .map(|p| p.display().to_string()))
}

// --- git panel ---------------------------------------------------------------

#[tauri::command]
fn git_status(state: State<'_, AppState>) -> Result<Vec<git::GitFileStatus>, String> {
    with_repo(state, git::status)
}

#[tauri::command]
fn git_branch(state: State<'_, AppState>) -> Result<Option<String>, String> {
    with_repo(state, |repo| Ok(git::current_branch(repo)))
}

#[tauri::command]
fn git_commit(state: State<'_, AppState>, message: String) -> Result<String, String> {
    with_repo(state, |repo| {
        git::commit(repo, &message).map(|oid| oid.to_string())
    })
}

#[tauri::command]
fn git_diff(state: State<'_, AppState>) -> Result<Vec<git::FileDiff>, String> {
    with_repo(state, git::diff_workdir)
}

#[tauri::command]
fn git_stage(state: State<'_, AppState>, path: String) -> Result<(), String> {
    with_repo(state, |repo| git::stage(repo, &path))
}

#[tauri::command]
fn git_unstage(state: State<'_, AppState>, path: String) -> Result<(), String> {
    with_repo(state, |repo| git::unstage(repo, &path))
}

#[tauri::command]
fn git_stage_all(state: State<'_, AppState>) -> Result<(), String> {
    with_repo(state, git::stage_all)
}

#[tauri::command]
fn git_branches(state: State<'_, AppState>) -> Result<Vec<git::BranchInfo>, String> {
    with_repo(state, git::branches)
}

#[tauri::command]
fn git_checkout(state: State<'_, AppState>, name: String) -> Result<(), String> {
    with_repo(state, |repo| git::checkout(repo, &name))
}

#[tauri::command]
fn git_log(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<git::CommitInfo>, String> {
    with_repo(state, |repo| git::log(repo, limit.unwrap_or(50)))
}

#[tauri::command]
fn git_commit_diff(state: State<'_, AppState>, oid: String) -> Result<String, String> {
    with_repo(state, |repo| git::commit_diff(repo, &oid))
}

fn with_repo<T>(
    state: State<'_, AppState>,
    f: impl FnOnce(&git2::Repository) -> Result<T, git2::Error>,
) -> Result<T, String> {
    let root = state
        .project_root
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "project root not set".to_string())?;
    let repo = git::open_repo(&root).map_err(|e| e.to_string())?;
    f(&repo).map_err(|e| e.to_string())
}

// --- watcher (trace grounding) -----------------------------------------------

#[tauri::command]
fn start_watch(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<(), String> {
    let root = state
        .project_root
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "project root not set".to_string())?;
    let rx = watcher::watch(&root).map_err(|e| e.to_string())?;
    thread::spawn(move || {
        let state = app.state::<AppState>();
        while let Ok(ev) = rx.recv() {
            let raw = ev.path.to_string_lossy().into_owned();

            // Ignore noise dirs — the trace should show project work.
            if raw.contains("\\.git\\")
                || raw.contains("/.git/")
                || raw.contains("node_modules")
                || raw.contains("\\target\\")
                || raw.contains("/target/")
            {
                continue;
            }
            let rel = ev
                .path
                .strip_prefix(&root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or(raw);
            let _ = app.emit(
                "fs://event",
                serde_json::json!({"path": rel, "kind": ev.kind}),
            );

            // Local activity: any file change counts as "the developer's
            // own terminal is working" and vetoes a pending failover.
            state
                .failover
                .lock()
                .unwrap()
                .record_activity(failover::Agent::Local, "fs");

            // Grounding: did the web AI recently claim to edit this file?
            let confirmed = {
                let mut ring = state.recent_edits.lock().unwrap();
                let pos = ring
                    .iter()
                    .position(|(f, t)| f == &rel && t.elapsed() < Duration::from_secs(30));
                if let Some(i) = pos {
                    ring.remove(i);
                    true
                } else {
                    false
                }
            };
            if confirmed {
                let _ = db::confirm_trace_steps(&state.conn.lock().unwrap(), &rel);
                let _ = app.emit("trace://confirm", serde_json::json!({"path": rel}));
            }
        }
    });
    Ok(())
}

// --- bridge: web-AI tools + approvals ----------------------------------------

/// Stream a running command into the UI terminal pane.
pub(crate) fn command_stream(app: &AppHandle) -> impl FnMut(bridge::CommandEvent) + '_ {
    let app = app.clone();
    move |event| {
        let _ = app.emit("terminal://run", event);
    }
}

/// Route a tool call through the approval policy. Shared by the
/// `bridge_tool` command (desktop) and the WebSocket handler (web).
pub(crate) fn tool_call(app: &AppHandle, tool: bridge::Tool, source: &str) -> bridge::ToolResult {
    let state = app.state::<AppState>();
    let root = state.project_root.lock().unwrap().clone();
    let (result, approval_id) =
        state
            .bridge
            .lock()
            .unwrap()
            .submit(tool.clone(), source, root.as_deref());
    let Some(_id) = approval_id else {
        // Auto-approved: audit and trace immediately.
        let _ = db::record_audit(
            &state.conn.lock().unwrap(),
            source,
            "tool",
            &serde_json::json!(tool).to_string(),
            true,
            "auto",
            result.ok,
        );
        if result.ok {
            record_tool_trace(&state, app, &tool);
        }
        return result;
    };
    let summary = bridge::describe(&tool);
    let _ = app.emit(
        "bridge://approval-requested",
        serde_json::json!({"id": _id, "summary": summary, "source": source}),
    );
    if source == "web" {
        // WS caller: wait for the user's decision (up to 5 minutes).
        let (tx, rx) = bridge::wait_channel();
        state
            .bridge
            .lock()
            .unwrap()
            .channels
            .lock()
            .unwrap()
            .insert(_id, tx);

        match rx.recv_timeout(Duration::from_secs(300)) {
            Ok(r) => r,
            Err(_) => bridge::ToolResult::err("approval timed out"),
        }
    } else {
        result
    }
}

/// Desktop tool sandbox / extension relay entry.
#[tauri::command]
fn bridge_tool(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    tool: bridge::Tool,
) -> Result<bridge::ToolResult, String> {
    let _ = &state;
    Ok(tool_call(&app, tool, "desktop"))
}

/// Resolve a pending approval: execute (allow) or deny.
#[tauri::command]
fn bridge_approve(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: u64,
    allow: bool,
) -> Result<bridge::ToolResult, String> {
    let root = state.project_root.lock().unwrap().clone();
    let mut stream = command_stream(&app);
    let (result, req) = state
        .bridge
        .lock()
        .unwrap()
        .resolve(id, allow, root.as_deref(), Some(&mut stream))
        .ok_or_else(|| "no such approval request".to_string())?;
    let _ = db::record_audit(
        &state.conn.lock().unwrap(),
        &req.source,
        "tool",
        &serde_json::json!(req.tool).to_string(),
        allow,
        if allow { "user" } else { "denied" },
        result.ok,
    );
    if allow && result.ok {
        record_tool_trace(&state, &app, &req.tool);
    }
    let _ = app.emit(
        "bridge://approval-resolved",
        serde_json::json!({"id": id, "allowed": allow, "result": result}),
    );
    Ok(result)
}

/// Recent audit trail (approval + auto-executed tool calls).
#[tauri::command]
fn bridge_audit(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::AuditEntry>, String> {
    db::last_audit(&state.conn.lock().unwrap(), limit.unwrap_or(30)).map_err(|e| e.to_string())
}

/// Cancel a running tool call: kill every process its `run_command` spawned
/// (SIGTERM to the process group, SIGKILL after a grace period). `owner` is
/// the WS request id the extension got back with its tool_result, or the
/// id shown on the approval card.
#[tauri::command]
fn cancel_request(owner: String) -> Result<usize, String> {
    Ok(process::registry().kill_owner(&owner, Duration::from_millis(500)))
}

/// Processes currently registered (live `run_command` executions).
#[tauri::command]
fn processes_list() -> Vec<process::ProcessEntry> {
    process::registry().list()
}

/// Record an executed tool call as a trace step, so the live activity
/// trace and handoff reflect real web-AI work.
pub(crate) fn record_tool_trace(state: &AppState, app: &AppHandle, tool: &bridge::Tool) {
    let Some(kind) = bridge::spec(tool).trace_kind else {
        return;
    };
    // A trace row carries either the file it touched or the command it ran.
    let (file, command) = match tool {
        bridge::Tool::RunCommand { command } => (None, Some(command.clone())),
        _ => (
            bridge::tool_paths(tool).first().map(|p| p.to_string()),
            None,
        ),
    };
    let _ = db::record_trace_step(
        &state.conn.lock().unwrap(),
        None,
        kind,
        file.as_deref(),
        command.as_deref(),
        None,
        false,
    );
    let _ = app.emit(
        "trace://step",
        TraceStepEvent {
            kind: kind.to_string(),
            file: file.clone(),
            command: command.clone(),
            detail: None,
            confirmed: false,
            agent: "web".to_string(),
            ts: now_millis(),
        },
    );
    // Web activity: the paired web AI is making real tool calls.
    state
        .failover
        .lock()
        .unwrap()
        .record_activity(failover::Agent::Web, "tool");
    if kind == "editing" {
        if let Some(file) = file {
            let mut ring = state.recent_edits.lock().unwrap();
            ring.push_back((file, Instant::now()));
            while ring.len() > 64 {
                ring.pop_front();
            }
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// --- pairing + handoff -------------------------------------------------------

/// Get (or generate) the 6-digit pairing code for the extension.
/// Session-scoped: never persisted — see the bootstrap note in `run()`.
#[tauri::command]
fn pair_get_code(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<String, String> {
    let mut code = state.pair_code.lock().unwrap();
    if code.is_empty() {
        *code = ws::new_pair_code();
        let _ = app.emit("pair://code", code.clone());
    }
    Ok(code.clone())
}

/// Is an extension currently paired?
#[tauri::command]
fn pair_status(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.ws_connected.load(std::sync::atomic::Ordering::SeqCst))
}

/// Set the handoff objective (editable in the handoff panel).
#[tauri::command]
fn set_objective(state: State<'_, AppState>, text: String) -> Result<(), String> {
    *state.objective.lock().unwrap() = Some(text);
    Ok(())
}

/// Handoff card payload, built from persisted trace state + (optionally)
/// the developer's own Claude Code transcript for real task context.
#[derive(Clone, serde::Serialize)]
pub struct Handoff {
    pub objective: String,
    pub progress_percent: u8,
    pub files_changed: usize,
    pub errors_remaining: usize,
    pub next_step: Option<String>,
    pub files: Vec<String>,
    pub context: Option<String>,
    pub end_reason: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_attempts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<String>,
    pub generated_at: String,
}

/// Build the handoff card from persisted trace state (shared by the
/// desktop command and the extension's handoff-request).
pub(crate) fn build_handoff_impl(state: &AppState) -> Result<Handoff, String> {
    // Transcript first (reads ~/.claude), then DB. Read root before
    // taking the DB lock so parsing never blocks the app.
    let root = state.project_root.lock().unwrap().clone();
    let transcript = root
        .as_deref()
        .and_then(transcript::load_for)
        .filter(|t| t.objective.is_some() || t.message_snippet.is_some());
    let conn = state.conn.lock().unwrap();

    // Archive the transcript and refresh extracted facts as a side effect,
    // so Layer 1/2 memory stays current whenever a handoff is built.
    let mut session_id = db::newest_session_id(&conn).map_err(|e| e.to_string())?;
    if let Some(t) = &transcript {
        if let Ok(id) = archive::persist_context(&conn, t) {
            session_id = Some(id);
        }
    }
    let mut decisions = Vec::new();
    let mut failed_attempts = Vec::new();
    let mut constraints = Vec::new();
    if let Some(sid) = session_id {
        if let Ok(f) = db::get_facts(&conn, sid) {
            decisions = f.decisions;
            failed_attempts = f.failed_attempts;
            constraints = f.constraints;
        }
    }
    let stats = db::trace_stats(&conn).map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT file FROM trace_steps WHERE kind = 'editing' AND file IS NOT NULL",
        )
        .map_err(|e| e.to_string())?;
    let files: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let objective = state
        .objective
        .lock()
        .unwrap()
        .clone()
        .or_else(|| transcript.as_ref().and_then(|t| t.objective.clone()))
        .unwrap_or_else(|| "Continue the interrupted coding task".to_string());
    let context = transcript.as_ref().and_then(|t| t.message_snippet.clone());
    let end_reason = transcript.as_ref().and_then(|t| t.end_reason.clone());

    // Honest heuristic progress: test results and error count shape it.
    let progress_percent = if stats.steps == 0 {
        5
    } else {
        let base = 30 + 10 * stats.steps.min(6) as u8;
        if stats.errors > 0 {
            base.saturating_sub(15)
        } else if stats.steps >= 8 {
            base.min(85)
        } else {
            base.min(70)
        }
    };
    let generated_at = now_millis().to_string();
    Ok(Handoff {
        objective,
        progress_percent,
        files_changed: stats.files_changed,
        errors_remaining: stats.errors,
        next_step: stats.last_step,
        files,
        context,
        end_reason,
        decisions,
        failed_attempts,
        constraints,
        generated_at,
    })
}

#[tauri::command]
fn build_handoff(state: State<'_, AppState>) -> Result<Handoff, String> {
    build_handoff_impl(&state)
}

/// Send the handoff to the paired extension (returns the payload; false
/// when no extension is paired — the frontend falls back to copy).
#[tauri::command]
fn handoff_send(state: State<'_, AppState>, app: tauri::AppHandle) -> Result<Handoff, String> {
    let handoff = build_handoff(state)?;
    let payload = serde_json::to_value(&handoff).map_err(|e| e.to_string())?;
    if !ws::push_handoff(&app, &payload) {
        // still return the payload; frontend offers clipboard fallback
    }
    Ok(handoff)
}

// --- automatic failover -------------------------------------------------------

/// Direction A trigger: the developer's own terminal went quiet for long
/// enough. Build the enriched handoff and push it to the web AI with
/// `auto: true` so it picks the task up without being asked.
fn run_local_failover(app: &AppHandle) {
    let state = app.state::<AppState>();
    let idle = failover::idle_ms(&state.failover.lock().unwrap(), failover::Agent::Local);
    let Ok(handoff) = build_handoff_impl(&state) else {
        let _ = app.emit(
            "failover://local",
            serde_json::json!({"ok": false, "idle_ms": idle, "error": "handoff build failed"}),
        );
        return;
    };
    let mut payload = match serde_json::to_value(&handoff) {
        Ok(v) => v,
        Err(e) => {
            let _ = app.emit(
                "failover://local",
                serde_json::json!({"ok": false, "idle_ms": idle, "error": format!("handoff serialization failed: {e}")}),
            );
            return;
        }
    };
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("auto".into(), serde_json::json!(true));
        obj.insert("direction".into(), serde_json::json!("local_to_web"));
        obj.insert("target".into(), serde_json::json!("chatgpt"));
    }
    let delivered = ws::push_handoff(app, &payload);
    let payload_str = payload.to_string();
    let _ = db::record_failover(
        &state.conn.lock().unwrap(),
        &db::NewFailover {
            direction: "local_to_web",
            trigger: "inactivity",
            idle_ms: idle,
            payload: Some(&payload_str),
            target: Some("chatgpt"),
            delivered,
            outcome: if delivered {
                Some("auto-continued on ChatGPT")
            } else {
                Some("unpaired — offered in-app")
            },
        },
    );
    let _ = app.emit(
        "failover://local",
        serde_json::json!({"ok": true, "delivered": delivered, "idle_ms": idle, "handoff": handoff}),
    );
}

/// Direction B trigger: the paired web AI died mid-work (extension WS
/// dropped or it went silent). Surface a card offering another web AI or
/// handing back to the local terminal.
fn run_web_failover(app: &AppHandle) {
    let state = app.state::<AppState>();
    let idle = failover::idle_ms(&state.failover.lock().unwrap(), failover::Agent::Web);
    let ws_down = !state.ws_connected.load(std::sync::atomic::Ordering::SeqCst);
    let trigger = if ws_down { "ws_drop" } else { "inactivity" };
    let _ = db::record_failover(
        &state.conn.lock().unwrap(),
        &db::NewFailover {
            direction: "web_to_web",
            trigger,
            idle_ms: idle,
            payload: None,
            target: None,
            delivered: false,
            outcome: Some("offered switch in app"),
        },
    );
    let handoff = build_handoff_impl(&state).ok();
    let _ = app.emit(
        "failover://web",
        serde_json::json!({"idle_ms": idle, "trigger": trigger, "handoff": handoff}),
    );
}

/// The failover ticker: evaluate both state machines periodically and act
/// on transitions. Spawned once at startup.
fn spawn_failover_ticker(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(30));
        let state = app.state::<AppState>();
        let ws_connected = state.ws_connected.load(std::sync::atomic::Ordering::SeqCst);
        let now = Instant::now();
        let mut monitor = state.failover.lock().unwrap();
        let local = monitor.check(failover::Agent::Local, false, now);
        let web = monitor.check(failover::Agent::Web, ws_connected, now);
        let status = serde_json::json!({
            "local": monitor.state(failover::Agent::Local).label(),
            "web": monitor.state(failover::Agent::Web).label(),
            "local_idle_ms": failover::idle_ms(&monitor, failover::Agent::Local),
            "web_idle_ms": failover::idle_ms(&monitor, failover::Agent::Web),
        });
        drop(monitor);
        let _ = app.emit("failover://status", status);
        if local == failover::Check::Interrupted {
            run_local_failover(&app);
        }
        if web == failover::Check::Interrupted {
            run_web_failover(&app);
        }
    });
}

/// Current failover state (both directions) for the UI status indicator.
#[tauri::command]
fn failover_status(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let monitor = state.failover.lock().unwrap();
    Ok(serde_json::json!({
        "local": monitor.state(failover::Agent::Local).label(),
        "web": monitor.state(failover::Agent::Web).label(),
        "local_idle_ms": failover::idle_ms(&monitor, failover::Agent::Local),
        "web_idle_ms": failover::idle_ms(&monitor, failover::Agent::Web),
    }))
}

/// Reset a failover state machine (dismiss / keep waiting / hand back).
#[tauri::command]
fn failover_reset(state: State<'_, AppState>, agent: String) -> Result<(), String> {
    let mut monitor = state.failover.lock().unwrap();
    let agent = match agent.as_str() {
        "local" => failover::Agent::Local,
        "web" => failover::Agent::Web,
        _ => return Err("agent must be 'local' or 'web'".into()),
    };
    monitor.reset(agent);
    Ok(())
}

/// Direction B continuation: deliver the current handoff to a chosen
/// target (`chatgpt | claudeai | gemini | grok`), or hand back to the local
/// terminal (`local`).
#[tauri::command]
fn failover_deliver(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    target: String,
) -> Result<Handoff, String> {
    // Must stay in step with TARGET_HOSTS in extension/background.js — the
    // extension is what actually opens the tab for the target we name here.
    if !matches!(
        target.as_str(),
        "chatgpt" | "claudeai" | "gemini" | "grok" | "local"
    ) {
        return Err("target must be chatgpt, claudeai, gemini, grok or local".into());
    }
    let handoff = build_handoff_impl(&state)?;
    if target == "local" {
        // Hand back: the developer resumes in their own terminal. Log it
        // and re-arm the web monitor.
        let _ = db::record_failover(
            &state.conn.lock().unwrap(),
            &db::NewFailover {
                direction: "web_to_local",
                trigger: "manual",
                idle_ms: 0,
                payload: None,
                target: Some("local"),
                delivered: false,
                outcome: Some("developer resumes locally"),
            },
        );
        state.failover.lock().unwrap().reset(failover::Agent::Web);
        return Ok(handoff);
    }
    let payload = serde_json::to_value(&handoff)
        .map(|mut v| {
            if let Some(obj) = v.as_object_mut() {
                obj.insert("auto".into(), serde_json::json!(true));
                obj.insert("direction".into(), serde_json::json!("web_to_web"));
                obj.insert("target".into(), serde_json::json!(target));
            }
            v
        })
        .map_err(|e| e.to_string())?;
    let delivered = ws::push_handoff(&app, &payload);
    let _ = db::record_failover(
        &state.conn.lock().unwrap(),
        &db::NewFailover {
            direction: "web_to_web",
            trigger: "manual",
            idle_ms: 0,
            payload: Some(&payload.to_string()),
            target: Some(&target),
            delivered,
            outcome: if delivered {
                Some("delivered to target")
            } else {
                Some("unpaired — offered in-app")
            },
        },
    );
    state.failover.lock().unwrap().reset(failover::Agent::Web);
    Ok(handoff)
}

/// Recent automatic-failover records (feeds the continuation-rate metric).
#[tauri::command]
fn failover_log(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::FailoverEntry>, String> {
    db::failover_log(&state.conn.lock().unwrap(), limit.unwrap_or(20)).map_err(|e| e.to_string())
}

// --- session archive + project memory (F2/F3) ---------------------------------

/// Snapshot of one session's extracted facts, plus the archive pass that
/// produced it (mirrors frontend `FactsSnapshot`).
#[derive(Clone, serde::Serialize)]
pub struct FactsSnapshot {
    pub session_id: Option<i64>,
    pub report: archive::ArchiveReport,
    pub facts: db::ProjectFacts,
}

fn projects_dir_or_err() -> Result<PathBuf, String> {
    transcript::claude_projects_dir().ok_or_else(|| "no ~/.claude/projects directory".into())
}

fn require_root(state: &AppState) -> Result<PathBuf, String> {
    state
        .project_root
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "project root not set".to_string())
}

/// Mirror this project's Claude Code transcripts into the local SQLite
/// archive (idempotent — unchanged files are skipped).
#[tauri::command]
fn sessions_archive(state: State<'_, AppState>) -> Result<archive::ArchiveReport, String> {
    let root = require_root(&state)?;
    let dir = projects_dir_or_err()?;
    let conn = state.conn.lock().unwrap();
    let (report, _newest) = archive::archive_project(&conn, &dir, &root)?;
    Ok(report)
}

/// Archived sessions for this project (newest first).
#[tauri::command]
fn sessions_list(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::SessionSummary>, String> {
    db::list_sessions(&state.conn.lock().unwrap(), limit.unwrap_or(20)).map_err(|e| e.to_string())
}

/// Timeline events of one archived session.
#[tauri::command]
fn session_events_get(
    state: State<'_, AppState>,
    session_id: i64,
    limit: Option<usize>,
) -> Result<Vec<db::SessionEventRow>, String> {
    db::session_events_for(
        &state.conn.lock().unwrap(),
        session_id,
        limit.unwrap_or(100),
    )
    .map_err(|e| e.to_string())
}

/// Archive transcripts, extract structured facts from the newest session,
/// persist them into project memory, and return the stored view.
#[tauri::command]
fn facts_extract(state: State<'_, AppState>) -> Result<FactsSnapshot, String> {
    let root = require_root(&state)?;
    let dir = projects_dir_or_err()?;
    let conn = state.conn.lock().unwrap();
    let (report, newest) = archive::archive_project(&conn, &dir, &root)?;
    let session_id = match newest {
        Some((id, _ctx)) => Some(id),
        None => db::newest_session_id(&conn).map_err(|e| e.to_string())?,
    };
    let facts = match session_id {
        Some(sid) => db::get_facts(&conn, sid).map_err(|e| e.to_string())?,
        None => db::ProjectFacts::default(),
    };
    Ok(FactsSnapshot {
        session_id,
        report,
        facts,
    })
}

// --- app bootstrap -----------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            conn: Mutex::new(rusqlite::Connection::open_in_memory().expect("in-memory db")),
            project_root: Mutex::new(None),
            pair_code: Mutex::new(String::new()),
            ws_connected: AtomicBool::new(false),
            ws_tx: Mutex::new(None),
            bridge: Mutex::new(bridge::Bridge::new()),
            objective: Mutex::new(None),
            recent_edits: Mutex::new(VecDeque::new()),
            failover: Mutex::new(failover::ActivityMonitor::new()),
        })
        .setup(|app| {
            // Auto-init: app-data SQLite, persisted settings, WS server.
            let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let db_path = dir.join("bridge.db");
            let conn = db::open_and_migrate(&db_path).map_err(|e| e.to_string())?;
            let root = db::get_setting(&conn, "project_root").ok().flatten();
            // Pairing code: fresh from the CSPRNG at every launch, memory
            // only. It used to be persisted in SQLite and reused forever —
            // a standing credential that any local process could brute
            // force at leisure (or read straight out of the DB). The
            // extension re-pairs once per app session, like Bluetooth.
            let _ = db::delete_setting(&conn, "pair_code");
            let code = ws::new_pair_code();
            let state = app.state::<AppState>();
            *state.conn.lock().unwrap() = conn;
            *state.project_root.lock().unwrap() = root.map(PathBuf::from);
            *state.pair_code.lock().unwrap() = code;
            ws::spawn_server(app.handle().clone());
            spawn_failover_ticker(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            init_database,
            set_project_root,
            get_project_root,
            git_status,
            git_branch,
            git_commit,
            git_diff,
            git_stage,
            git_unstage,
            git_stage_all,
            git_branches,
            git_checkout,
            git_log,
            git_commit_diff,
            start_watch,
            bridge_tool,
            bridge_approve,
            bridge_audit,
            cancel_request,
            processes_list,
            pair_get_code,
            pair_status,
            set_objective,
            build_handoff,
            handoff_send,
            failover_status,
            failover_reset,
            failover_deliver,
            failover_log,
            sessions_archive,
            sessions_list,
            session_events_get,
            facts_extract,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(|_handle, event| {
        // Never leave a spawned command behind: killing on ExitRequested
        // (not Exit — by then the process is torn down) lets the registry
        // TERM→KILL the process groups while we can still signal.
        if let tauri::RunEvent::ExitRequested { .. } = event {
            process::registry().kill_all(Duration::from_millis(500));
        }
    });
}
