pub mod archive;

pub mod bridge;

pub mod db;

pub mod failover;

pub mod facts;

pub mod git;

pub mod mcp;

pub mod pty;

pub mod process;

pub mod shell;

pub mod transcript;

pub mod watcher;

use std::collections::VecDeque;

use std::path::PathBuf;

use std::sync::atomic::{AtomicBool, Ordering};

use std::sync::{Arc, Mutex};

use std::thread;

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager, State};

/// App-managed shared state: the SQLite connection, the watched project
/// root, the MCP connector's live policy, and the approval queue.
pub(crate) struct AppState {
    pub(crate) conn: Mutex<rusqlite::Connection>,
    pub(crate) project_root: Mutex<Option<PathBuf>>,
    /// The one live project watcher (trace grounding + local-activity veto).
    /// Replaced — never stacked — on each `start_watch`, so a stale watcher
    /// from an earlier root cannot keep vetoing the local failover idle timer.
    pub(crate) fs_watcher: Mutex<Option<notify::RecommendedWatcher>>,
    pub(crate) bridge: Mutex<bridge::Bridge>,
    pub(crate) objective: Mutex<Option<String>>,
    /// Recent "editing X" steps, for watcher cross-correlation.
    pub(crate) recent_edits: Mutex<VecDeque<(String, Instant)>>,
    /// Automatic-failover state machines (local + web directions).
    pub(crate) failover: Mutex<failover::ActivityMonitor>,
    /// Live read-only/write gate for the native-MCP connector. Off by
    /// default: the connector surface is read-only until this is flipped,
    /// which then also re-exposes the write/command tools. Toggled at runtime
    /// from the desktop — no rebuild, no reconnect.
    pub(crate) mcp_allow_write: Arc<AtomicBool>,
    /// Whether the connector actually bound its loopback socket. Set by
    /// [`mcp::spawn_server`] once the listener is up, so the UI can tell
    /// "endpoint live" from "endpoint failed to bind".
    pub(crate) mcp_listening: Arc<AtomicBool>,
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
    let watch = watcher::watch(&root).map_err(|e| e.to_string())?;
    // Split so the notify handle (which keeps the OS watch alive) can be held
    // in app state — a single, replaceable watch — while this thread drains
    // its events.
    let (handle, rx) = watch.into_parts();

    // Replace, never stack: overwriting drops the previous `RecommendedWatcher`,
    // which stops its OS watch *and* disconnects its channel, so the previous
    // reader thread below wakes and exits. Before this, every `start_watch`
    // (mount + each project switch) leaked a watcher whose events kept
    // recording local activity and vetoing the local failover idle timer.
    *state.fs_watcher.lock().unwrap() = Some(handle);

    thread::spawn(move || {
        let state = app.state::<AppState>();
        // Ends when this watch is replaced (channel disconnects): the new
        // watch has its own reader, so an exited thread is not a leak.
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
/// `bridge_tool` command (desktop) and the native MCP server (`mcp.rs`).
pub(crate) fn tool_call(app: &AppHandle, tool: bridge::Tool, source: &str) -> bridge::ToolResult {
    let state = app.state::<AppState>();
    let root = state.project_root.lock().unwrap().clone();
    let (result, approval_id, authorized_by) =
        state
            .bridge
            .lock()
            .unwrap()
            .submit_with_audit(tool.clone(), source, root.as_deref());
    let Some(id) = approval_id else {
        // Auto-approved (or covered by a session grant): audit and trace.
        let _ = db::record_audit(
            &state.conn.lock().unwrap(),
            source,
            "tool",
            &serde_json::json!(tool).to_string(),
            true,
            &authorized_by,
            result.ok,
        );
        if result.ok {
            record_tool_trace(&state, app, &tool);
        }
        return result;
    };
    // What the card shows: destructive tools carry resolved absolute paths,
    // and grantable tools can be offered a session grant checkbox.
    let summary = bridge::describe_for_approval(&tool, root.as_deref());
    let destructive = bridge::spec(&tool).approval == bridge::Approval::Destructive;
    let grantable = (!destructive)
        .then(|| bridge::grantable(&tool))
        .flatten()
        .map(|(scope, prefix)| {
            serde_json::json!({"scope": scope.as_str(), "suggested_prefix": prefix})
        });
    let _ = app.emit(
        "bridge://approval-requested",
        serde_json::json!({
            "id": id,
            "summary": summary,
            "source": source,
            "destructive": destructive,
            "grantable": grantable,
        }),
    );
    // Remote callers (the MCP connector) wait here for the user's decision
    // on the desktop. The desktop itself never blocks: it resolves the same
    // request through `bridge_approve`.
    let timeout = match source {
        bridge::SOURCE_MCP => Duration::from_secs(crate::mcp::APPROVAL_WAIT_SECS),
        // Desktop sandbox / anything else: return the queued marker; the UI
        // resolves via bridge_approve and gets the executed result there.
        _ => return result,
    };

    let (tx, rx) = bridge::wait_channel();
    state
        .bridge
        .lock()
        .unwrap()
        .channels
        .lock()
        .unwrap()
        .insert(id, tx);

    match rx.recv_timeout(timeout) {
        Ok(r) => r,
        Err(_) => {
            // The remote caller waited its full window and was already told
            // the call failed, so this approval must not stay executable.
            // Dequeue it (and drop its channel): an Allow on the now-stale
            // card then resolves nothing instead of silently running the
            // write long after — with nobody watching.
            let expired = state.bridge.lock().unwrap().expire(id);
            if let Some(req) = expired {
                let _ = db::record_audit(
                    &state.conn.lock().unwrap(),
                    &req.source,
                    "tool",
                    &serde_json::json!(req.tool).to_string(),
                    false,
                    "timeout",
                    false,
                );
                let _ = app.emit(
                    "bridge://approval-resolved",
                    serde_json::json!({
                        "id": id,
                        "allowed": false,
                        "result": bridge::ToolResult::err("approval timed out"),
                    }),
                );
            }
            bridge::ToolResult::err("approval timed out")
        }
    }
}

/// Desktop tool sandbox entry — the in-app caller path (the only other
/// transport is the native MCP server).
#[tauri::command]
fn bridge_tool(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    tool: bridge::Tool,
) -> Result<bridge::ToolResult, String> {
    let _ = &state;
    Ok(tool_call(&app, tool, bridge::SOURCE_DESKTOP))
}

/// A session grant offered from an approval card ("don't ask again for
/// edits under src/ this session").
#[derive(Debug, serde::Deserialize)]
struct GrantRequest {
    scope: bridge::GrantScope,
    path_prefix: Option<String>,
}

/// The grants + paused snapshot the UI renders; also the payload of
/// `bridge://grants-changed`.
#[derive(Clone, serde::Serialize)]
struct GrantState {
    grants: Vec<bridge::SessionGrant>,
    paused: bool,
}

fn grant_state(state: &AppState) -> GrantState {
    let bridge = state.bridge.lock().unwrap();
    let grants = bridge.grants.lock().unwrap().clone();
    GrantState {
        grants,
        paused: bridge.is_paused(),
    }
}

/// Resolve a pending approval: execute (allow) or deny. With `grant`, also
/// creates a session grant covering this class of call.
#[tauri::command]
fn bridge_approve(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: u64,
    allow: bool,
    grant: Option<GrantRequest>,
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
    if allow {
        if let Some(g) = &grant {
            state
                .bridge
                .lock()
                .unwrap()
                .grant_add(g.scope, g.path_prefix.clone(), &req.source);
            let _ = app.emit("bridge://grants-changed", grant_state(&state));
        }
    }
    let _ = app.emit(
        "bridge://approval-resolved",
        serde_json::json!({"id": id, "allowed": allow, "result": result}),
    );
    Ok(result)
}

/// Current session grants and the paused flag.
#[tauri::command]
fn bridge_grant_state(state: State<'_, AppState>) -> GrantState {
    grant_state(&state)
}

/// Revoke one session grant by id.
#[tauri::command]
fn bridge_grant_revoke(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    id: u64,
) -> Result<bool, String> {
    let revoked = state.bridge.lock().unwrap().grant_revoke(id);
    let _ = app.emit("bridge://grants-changed", grant_state(&state));
    Ok(revoked)
}

/// The kill switch: revoke every grant and pause the bridge (or unpause).
/// Pausing revokes too — a kill switch that left standing auto-approvals
/// armed would not be a kill switch.
#[tauri::command]
fn bridge_pause(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    paused: bool,
) -> Result<(), String> {
    state.bridge.lock().unwrap().set_paused(paused);
    let _ = app.emit("bridge://grants-changed", grant_state(&state));
    Ok(())
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
/// (SIGTERM to the process group, SIGKILL after a grace period). `owner` must
/// match the id the process registered under (`process::ProcessEntry.owner`);
/// a process registered with no owner is not reachable this way.
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
    // Web-direction activity: the remote caller is making real tool calls.
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

// --- MCP connector -----------------------------------------------------------

/// The connector's live state as the desktop renders it: where the endpoint
/// is, whether it bound, what workspace it can reach, and whether the write
/// surface is currently exposed.
#[derive(Clone, serde::Serialize)]
struct McpStatus {
    listening: bool,
    endpoint: String,
    allow_write: bool,
    workspace: Option<String>,
}

fn mcp_status_of(state: &AppState) -> McpStatus {
    McpStatus {
        listening: state.mcp_listening.load(Ordering::SeqCst),
        endpoint: format!("http://{}{}", mcp::ADDR, mcp::MCP_PATH),
        allow_write: state.mcp_allow_write.load(Ordering::SeqCst),
        workspace: state
            .project_root
            .lock()
            .unwrap()
            .clone()
            .map(|p| p.display().to_string()),
    }
}

/// Read the connector state (the desktop polls this on mount).
#[tauri::command]
fn mcp_status(state: State<'_, AppState>) -> McpStatus {
    mcp_status_of(&state)
}

/// Open or close the connector's write surface at runtime. This is the only
/// thing that moves `allow_write`, which seeds from `LEXSUS_MCP_ALLOW_WRITE`
/// at launch and is otherwise read-only-first.
#[tauri::command]
fn mcp_set_allow_write(state: State<'_, AppState>, enabled: bool) -> McpStatus {
    state.mcp_allow_write.store(enabled, Ordering::SeqCst);
    mcp_status_of(&state)
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
/// desktop command and the automatic-failover paths).
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
    // The connector is pull-based: the desktop can no longer push a handoff
    // into a chat the way the extension relay could. The interruption is
    // surfaced in-app instead, carrying the same handoff the connector's
    // `get_handoff` tool hands back to a connected session.
    let payload_str = payload.to_string();
    let _ = db::record_failover(
        &state.conn.lock().unwrap(),
        &db::NewFailover {
            direction: "local_to_web",
            trigger: "inactivity",
            idle_ms: idle,
            payload: Some(&payload_str),
            target: None,
            delivered: false,
            outcome: Some("offered in-app"),
        },
    );
    let _ = app.emit(
        "failover://local",
        serde_json::json!({"ok": true, "delivered": false, "idle_ms": idle, "handoff": handoff}),
    );
}

/// Direction B trigger: the remote caller went quiet mid-work. With no
/// persistent socket left to observe, this fires on inactivity alone.
/// Surface a card offering another web AI or handing back to the local
/// terminal.
fn run_web_failover(app: &AppHandle) {
    let state = app.state::<AppState>();
    let idle = failover::idle_ms(&state.failover.lock().unwrap(), failover::Agent::Web);
    // The native connector attaches and detaches on its own schedule, so
    // there is no persistent socket here to observe: this direction now
    // fires on inactivity alone.
    let trigger = "inactivity";
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
        let now = Instant::now();
        let mut monitor = state.failover.lock().unwrap();
        let local = monitor.check(failover::Agent::Local, false, now);
        // `true` means "remote attached": with the connector there is no
        // socket to watch, so the web direction is inactivity-only.
        let web = monitor.check(failover::Agent::Web, true, now);
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

/// Whether the connector starts with its write surface open. Off unless
/// `LEXSUS_MCP_ALLOW_WRITE` is set to a truthy value — read-only-first is the
/// security posture, and the desktop can still flip it live afterwards.
fn mcp_allow_write_seed() -> bool {
    matches!(
        std::env::var("LEXSUS_MCP_ALLOW_WRITE").as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            conn: Mutex::new(rusqlite::Connection::open_in_memory().expect("in-memory db")),
            project_root: Mutex::new(None),
            fs_watcher: Mutex::new(None),
            bridge: Mutex::new(bridge::Bridge::new()),
            objective: Mutex::new(None),
            recent_edits: Mutex::new(VecDeque::new()),
            failover: Mutex::new(failover::ActivityMonitor::new()),
            // Read-only first: the connector exposes no write tool until the
            // desktop flips it live (or `LEXSUS_MCP_ALLOW_WRITE` seeds it on).
            mcp_allow_write: Arc::new(AtomicBool::new(mcp_allow_write_seed())),
            mcp_listening: Arc::new(AtomicBool::new(false)),
        })
        .setup(|app| {
            // Auto-init: app-data SQLite, persisted settings, connector.
            let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let db_path = dir.join("bridge.db");
            let conn = db::open_and_migrate(&db_path).map_err(|e| e.to_string())?;
            let root = db::get_setting(&conn, "project_root").ok().flatten();
            // The old extension's persisted pairing code is obsolete — drop it
            // so no stale credential lingers in the DB.
            let _ = db::delete_setting(&conn, "pair_code");
            let state = app.state::<AppState>();
            *state.conn.lock().unwrap() = conn;
            *state.project_root.lock().unwrap() = root.map(PathBuf::from);
            spawn_failover_ticker(app.handle().clone());
            // The durable endpoint: a desktop-local MCP server on loopback,
            // which a provider connector reaches through a tunnel.
            mcp::spawn_server(
                app.handle().clone(),
                state.mcp_allow_write.clone(),
                state.mcp_listening.clone(),
            );
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
            bridge_grant_state,
            bridge_grant_revoke,
            bridge_pause,
            cancel_request,
            processes_list,
            mcp_status,
            mcp_set_allow_write,
            set_objective,
            build_handoff,
            failover_status,
            failover_reset,
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
