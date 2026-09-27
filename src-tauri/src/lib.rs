pub mod archive;

pub mod auth;

pub mod bgproc;

pub mod bridge;

pub mod db;

pub mod failover;

pub mod facts;

pub mod git;

pub mod lsp;

pub mod mcp;

pub mod media;

pub mod notebook;

pub mod pty;

pub mod process;

pub mod shell;

pub mod transcript;

pub mod tunnel;

pub mod watcher;

pub mod web;

use std::collections::{HashMap, VecDeque};

use std::path::PathBuf;

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};

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
    /// The connector's lifecycle: whether it is up, on which port, and how to
    /// stop it. Owning the socket state here rather than in a global is what
    /// lets the desktop start, stop and restart the endpoint — and what makes
    /// a second app instance start its own rather than fight over this one's.
    pub(crate) connector: mcp::Connector,
    /// The public tunnel, if the user started one. Killed on stop and on exit:
    /// a tunnel must never outlive the window that opened it.
    pub(crate) tunnel: tunnel::Tunnel,
    /// The port the connector is configured for. Persisted, so a restart comes
    /// back on the same endpoint rather than a new one.
    pub(crate) mcp_port: std::sync::atomic::AtomicU16,
    /// Extra `Host` values the user has allowlisted, beyond loopback. Kept in
    /// state because it is editable at runtime: a tunnel's host is discovered
    /// after the tunnel starts, and rmcp bakes the allowlist into the service
    /// at construction, so applying one means restarting the connector.
    pub(crate) mcp_configured_hosts: Mutex<Vec<String>>,
    /// Why the last start attempt failed, if it did.
    ///
    /// The auto-start in `setup()` has nobody to return an error to, and a
    /// connector that silently failed to bind is exactly the "endpoint looks
    /// fine, nothing answers" case this app keeps having to explain. Held here
    /// so `mcp_status` can report it.
    pub(crate) mcp_bind_error: Mutex<Option<String>>,
    /// Commands started with `run_command_background` and not yet stopped.
    ///
    /// In state rather than in a global, unlike [`process::registry`], so a
    /// command belongs to the app instance that started it: two instances
    /// cannot see or stop each other's, and a test gets its own empty roster
    /// instead of finding the previous test's strays in it.
    pub(crate) bgproc: bgproc::Manager,
    /// The worktree this session is currently working inside, if any.
    ///
    /// `enter_worktree` sets it and `exit_worktree` clears it; the bridge
    /// substitutes it for `project_root` when resolving every path, so the
    /// AI's edits land in the throwaway tree rather than the user's working
    /// tree. In state rather than in a global so it dies with the app and a
    /// second instance cannot inherit it.
    pub(crate) active_worktree: Mutex<Option<PathBuf>>,
    /// One lazily-started language server per workspace root.
    ///
    /// Keyed by root rather than a single slot so two open projects do not
    /// tear each other's server down. Dropped — and so killed — with the app.
    pub(crate) lsp_clients: Mutex<HashMap<PathBuf, lsp::Client>>,
    /// The app handle, so a tool running on the blocking pool can raise an
    /// event (a notification, or a question card) on the UI thread. Set once
    /// in `setup`; `None` in tests, which is why the tools that need it say so
    /// rather than pretending to have raised something.
    pub(crate) app_handle: Mutex<Option<tauri::AppHandle>>,
    /// Monotonic ids for `ask_user` / `propose_plan` question cards.
    pub(crate) question_seq: AtomicU64,
    /// Outstanding question cards, by id, each waiting on the answer the
    /// desktop resolves through `bridge_answer_question`.
    pub(crate) questions: Mutex<HashMap<u64, std::sync::mpsc::SyncSender<bridge::QuestionAnswer>>>,
}

/// A throwaway app state over a temporary database, for tests that need a
/// tool context reaching further than a workspace root.
///
/// The real [`AppState`] is what `get_handoff` builds against, so a test that
/// supplies one exercises the production path rather than a stub of it.
#[cfg(test)]
pub(crate) fn test_state(root: &std::path::Path) -> AppState {
    // The database lives *outside* `root`, on purpose. Inside, it is a binary
    // file in the workspace: a workspace-wide `grep` would have to walk it,
    // and — as the git fixtures found — committing the tree and then checking
    // out a branch without it deletes the file from under the live
    // connection.
    let db_path = root.parent().unwrap_or(root).join(format!(
        ".lexsus-test-{}.sqlite3",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    AppState {
        conn: Mutex::new(db::open_and_migrate(&db_path).unwrap()),
        project_root: Mutex::new(Some(root.to_path_buf())),
        fs_watcher: Mutex::new(None),
        bridge: Mutex::new(bridge::Bridge::new()),
        objective: Mutex::new(None),
        recent_edits: Mutex::new(VecDeque::new()),
        failover: Mutex::new(failover::ActivityMonitor::new()),
        mcp_allow_write: Arc::new(AtomicBool::new(false)),
        connector: mcp::Connector::new(),
        tunnel: tunnel::Tunnel::new(),
        mcp_port: std::sync::atomic::AtomicU16::new(mcp::DEFAULT_PORT),
        mcp_configured_hosts: Mutex::new(Vec::new()),
        mcp_bind_error: Mutex::new(None),
        bgproc: bgproc::Manager::new(),
        active_worktree: Mutex::new(None),
        lsp_clients: Mutex::new(HashMap::new()),
        app_handle: Mutex::new(None),
        question_seq: AtomicU64::new(1),
        questions: Mutex::new(HashMap::new()),
    }
}

/// A Tauri app over the mock runtime, for tests that need a real app to hang
/// state, events and managed values off.
///
/// `mock_app()` is a genuine `App`, not a stub: `State` resolution, event
/// emission and `manage` all behave as they do at runtime. That is what lets
/// the connector's real bind/serve/shutdown path run in a test — and therefore
/// what gets the tunnel and the lifecycle tests out of the "mocked it and hoped"
/// category. Requires the `test` feature's `mock_app`, so it is test-only.
#[cfg(test)]
pub(crate) fn test_app() -> tauri::AppHandle<tauri::test::MockRuntime> {
    tauri::test::mock_app().handle().clone()
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
    // Trim stray whitespace/newlines a picker or paste may have included.
    // `is_dir` follows symlinks, so a symlinked folder still counts.
    let p = std::path::PathBuf::from(path.trim());
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
///
/// A root that no longer exists (a deleted or renamed folder) is cleared
/// rather than returned, so the watcher never tries to watch a missing
/// directory and the frontend never has to surface "No such file or
/// directory" to the user.
#[tauri::command]
fn get_project_root(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let mut root = state.project_root.lock().unwrap();
    if let Some(p) = root.as_ref() {
        if !p.is_dir() {
            let _ = db::delete_setting(&state.conn.lock().unwrap(), "project_root");
            *root = None;
            return Ok(None);
        }
    }
    Ok(root.clone().map(|p| p.display().to_string()))
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
///
/// Generic over the runtime so the MCP server can be driven over Tauri's mock
/// runtime in tests; every caller in the app instantiates it with `Wry`.
pub(crate) fn tool_call<R: tauri::Runtime>(
    app: &AppHandle<R>,
    tool: bridge::Tool,
    source: &str,
) -> bridge::ToolResult {
    let state = app.state::<AppState>();
    let root = state.project_root.lock().unwrap().clone();
    // A full context, not just a root: a call may reach the database (the
    // memory tools) or the desktop's live state (the handoff). It is built
    // here, in one place, rather than at every call site.
    let ctx = bridge::ToolCtx::with_state(root.as_deref(), &state);
    let (result, approval_id, authorized_by) =
        state
            .bridge
            .lock()
            .unwrap()
            .submit_with_ctx(tool.clone(), source, &ctx);
    let Some(id) = approval_id else {
        // Auto-approved (or covered by a session grant): audit and trace.
        // The spec name, not a literal: the audit trail used to read
        // `mcp · tool · {"ReadFile":…}`, with the only copy of the tool name
        // buried in the args blob.
        let _ = db::record_audit(
            &state.conn.lock().unwrap(),
            source,
            bridge::spec(&tool).name,
            &serde_json::json!(tool).to_string(),
            true,
            &authorized_by,
            result.ok,
        );
        record_tool_trace(&state, app, &tool, source, result.ok);
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
                    bridge::spec(&req.tool).name,
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

/// Resolve a pending `ask_user` or `propose_plan` card.
///
/// The asker is blocked on a channel keyed by this id, exactly as a gated
/// tool blocks on its approval; removing the entry before sending means a
/// second answer for the same card finds nothing rather than resolving it
/// twice.
#[tauri::command]
fn bridge_answer_question(
    state: State<'_, AppState>,
    id: u64,
    option: Option<String>,
    answer: String,
) -> Result<(), String> {
    let tx = state
        .questions
        .lock()
        .unwrap()
        .remove(&id)
        .ok_or_else(|| "no such question".to_string())?;
    tx.send(bridge::QuestionAnswer { option, answer })
        .map_err(|_| "the asking call is no longer waiting".to_string())
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
    // Approving a call may execute any tool — including one that reaches the
    // database or the desktop — so it resolves against the same context the
    // request arrived with, not a bare root. This is the second of the two
    // places that context is built.
    let ctx = bridge::ToolCtx::with_state(root.as_deref(), &state);
    let (result, req) = state
        .bridge
        .lock()
        .unwrap()
        .resolve_with_ctx(id, allow, &ctx, Some(&mut stream))
        .ok_or_else(|| "no such approval request".to_string())?;
    let _ = db::record_audit(
        &state.conn.lock().unwrap(),
        &req.source,
        bridge::spec(&req.tool).name,
        &serde_json::json!(req.tool).to_string(),
        allow,
        if allow { "user" } else { "denied" },
        result.ok,
    );
    if allow {
        record_tool_trace(&state, &app, &req.tool, &req.source, result.ok);
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

/// Cancel a running tool call: kill every process it spawned (SIGTERM to the
/// process group, SIGKILL after a grace period). `owner` must match the id the
/// process registered under (`process::ProcessEntry.owner`); a process
/// registered with no owner is not reachable this way.
///
/// Two registries, because a call can spawn two kinds of process: `run_command`
/// registers with the global [`process::registry`], while
/// `run_command_background` jobs live in this app instance's [`bgproc::Manager`].
/// A cancel that reached only the first would leave the second running.
#[tauri::command]
fn cancel_request(state: State<'_, AppState>, owner: String) -> Result<usize, String> {
    let grace = Duration::from_millis(500);
    Ok(process::registry().kill_owner(&owner, grace) + state.bgproc.kill_owner(&owner))
}

/// Processes currently registered (live `run_command` executions).
#[tauri::command]
fn processes_list() -> Vec<process::ProcessEntry> {
    process::registry().list()
}

/// Record an executed tool call as a trace step, so the live activity
/// trace and handoff reflect real web-AI work.
///
/// Called on **every** executed call, successful or not: a failed write is
/// activity, and a trace that only shows the successes cannot answer "what has
/// this agent been trying".
pub(crate) fn record_tool_trace<R: tauri::Runtime>(
    state: &AppState,
    app: &AppHandle<R>,
    tool: &bridge::Tool,
    source: &str,
    ok: bool,
) {
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
        bridge::spec(tool).name,
        Some(source),
        ok,
    );
    let _ = app.emit(
        "trace://step",
        TraceStepEvent {
            kind: kind.to_string(),
            file: file.clone(),
            command: command.clone(),
            detail: None,
            confirmed: false,
            // The real source. Hardcoding `web` labelled every desktop-sandbox
            // call as remote traffic.
            agent: source.to_string(),
            ts: now_millis(),
        },
    );
    // Web-direction activity: the remote caller is making real tool calls.
    if source == bridge::SOURCE_MCP {
        state
            .failover
            .lock()
            .unwrap()
            .record_activity(failover::Agent::Web, "tool");
    }
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
/// is, whether it bound, what workspace it can reach, whether the write
/// surface is currently exposed, and which `Host` values the endpoint accepts.
#[derive(Clone, serde::Serialize)]
struct McpStatus {
    /// Whether an endpoint is live (a served socket, not merely a start).
    listening: bool,
    /// Whether a start has been issued and not yet stopped. Diverges from
    /// `listening` only if the serve loop ends on its own.
    running: bool,
    endpoint: String,
    /// The bound port, which is the OS's pick when `0` was configured.
    port: u16,
    uptime_secs: u64,
    /// Why the last start attempt failed, if it did — so the UI can say
    /// "port in use" instead of an unexplained "offline".
    bind_error: Option<String>,
    allow_write: bool,
    workspace: Option<String>,
    /// Host authorities the DNS-rebinding guard accepts. Anything else is
    /// answered with a bare `403` — which a remote connector reads as "no MCP
    /// server here" and retries as OAuth. Reported here so a tunnel host that
    /// is missing from the allowlist is visible in the UI rather than only in
    /// the connector's misleading sign-in error.
    allowed_hosts: Vec<String>,
    /// The user's editable allowlist, without the loopback values the guard
    /// always adds. This is what the dashboard edits; `allowed_hosts` is what
    /// is actually enforced.
    configured_hosts: Vec<String>,
    /// Where the auth token actually lives: `env`, `keyring` or `file`. Worth
    /// showing because the keyring is unavailable on a headless Linux box, and
    /// the user should know which store holds their credential rather than
    /// assume.
    auth_backend: String,
    /// A short digest of the live token, so the UI can say *which* token is in
    /// force without ever rendering it. Never the token itself — revealing that
    /// is a deliberate, separate action.
    token_fingerprint: String,
    /// How far a signed request's timestamp may be from now.
    signature_ttl_secs: u64,
    /// Whether a signature is required on inbound requests. Always false today
    /// — no MCP client can produce one — and reported so the UI does not imply
    /// a guarantee the wire does not carry. Responses are signed regardless.
    signature_required: bool,
}

fn mcp_status_of(state: &AppState, auth: &auth::Auth) -> McpStatus {
    let running = state.connector.running();
    let port = running
        .as_ref()
        .map(|r| r.port)
        .unwrap_or_else(|| state.mcp_port.load(std::sync::atomic::Ordering::SeqCst));
    let configured = state.mcp_configured_hosts.lock().unwrap().clone();
    McpStatus {
        listening: state.connector.is_listening(),
        running: state.connector.is_running(),
        endpoint: format!("http://{}:{}{}", mcp::MCP_HOST, port, mcp::MCP_PATH),
        port,
        uptime_secs: running.map(|r| r.uptime_secs).unwrap_or(0),
        bind_error: state.mcp_bind_error.lock().unwrap().clone(),
        allow_write: state.mcp_allow_write.load(Ordering::SeqCst),
        workspace: state
            .project_root
            .lock()
            .unwrap()
            .clone()
            .map(|p| p.display().to_string()),
        allowed_hosts: mcp::effective_allowed_hosts(&configured),
        configured_hosts: configured,
        auth_backend: auth.backend().as_str().to_string(),
        token_fingerprint: auth.fingerprint(),
        signature_ttl_secs: auth.ttl_secs(),
        signature_required: false,
    }
}

/// Start the connector from whatever is in state right now.
///
/// The one place a start happens, so the desktop button, the auto-start in
/// `setup` and the restart that applies an allowlist change cannot drift apart.
/// A failure is recorded in state as well as returned, because `setup` has
/// nobody to return it to.
fn start_connector<R: tauri::Runtime>(
    state: &AppState,
    app: &AppHandle<R>,
    auth: &Arc<auth::Auth>,
) -> Result<u16, String> {
    let port = state.mcp_port.load(std::sync::atomic::Ordering::SeqCst);
    let hosts = state.mcp_configured_hosts.lock().unwrap().clone();
    match state.connector.start(
        app.clone(),
        state.mcp_allow_write.clone(),
        auth.clone(),
        port,
        hosts,
    ) {
        Ok(bound) => {
            *state.mcp_bind_error.lock().unwrap() = None;
            // Persist what was bound, not what was asked for: with port `0` the
            // OS picks, and recording the pick is what makes the endpoint the
            // same one on the next launch.
            state
                .mcp_port
                .store(bound, std::sync::atomic::Ordering::SeqCst);
            let _ = db::set_setting(&state.conn.lock().unwrap(), "mcp_port", &bound.to_string());
            Ok(bound)
        }
        Err(e) => {
            *state.mcp_bind_error.lock().unwrap() = Some(e.clone());
            Err(e)
        }
    }
}

/// Stop and start, to apply a change that is baked in at bind time.
fn restart_connector<R: tauri::Runtime>(
    state: &AppState,
    app: &AppHandle<R>,
    auth: &Arc<auth::Auth>,
) -> Result<u16, String> {
    let _ = state.connector.stop();
    start_connector(state, app, auth)
}

/// Read the connector state (the desktop polls this on mount).
#[tauri::command]
fn mcp_status(state: State<'_, AppState>, auth: State<'_, Arc<auth::Auth>>) -> McpStatus {
    mcp_status_of(&state, &auth)
}

/// Start the MCP endpoint. Errors rather than silently doing nothing when the
/// port is taken, so the dashboard can show why.
#[tauri::command]
fn mcp_start(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
) -> Result<McpStatus, String> {
    start_connector(&state, &app, &auth)?;
    let status = mcp_status_of(&state, &auth);
    let _ = app.emit("mcp://status", &status);
    Ok(status)
}

/// Stop the MCP endpoint and release the port.
///
/// **Abrupt, and that is the point:** live SSE sessions are severed and any
/// in-flight `tools/call` is cancelled — including one parked on an approval,
/// which then resolves to nothing. See [`mcp::Connector::stop`].
#[tauri::command]
fn mcp_stop(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
) -> Result<McpStatus, String> {
    state.connector.stop()?;
    let status = mcp_status_of(&state, &auth);
    let _ = app.emit("mcp://status", &status);
    Ok(status)
}

/// Restart the endpoint: the only way to apply a new port or allowlist, both
/// of which rmcp consumes when the service is constructed.
#[tauri::command]
fn mcp_restart(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
) -> Result<McpStatus, String> {
    restart_connector(&state, &app, &auth)?;
    let status = mcp_status_of(&state, &auth);
    let _ = app.emit("mcp://status", &status);
    Ok(status)
}

/// Change the port and apply it. Persisted, so the endpoint survives a relaunch.
#[tauri::command]
fn mcp_set_port(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
    port: u16,
) -> Result<McpStatus, String> {
    let previous = state
        .mcp_port
        .swap(port, std::sync::atomic::Ordering::SeqCst);
    if state.connector.is_running() {
        if let Err(e) = restart_connector(&state, &app, &auth) {
            // Put the old port back: a failed rebind should not leave the app
            // configured for a port it could not actually serve on.
            state
                .mcp_port
                .store(previous, std::sync::atomic::Ordering::SeqCst);
            return Err(e);
        }
    } else {
        let _ = db::set_setting(&state.conn.lock().unwrap(), "mcp_port", &port.to_string());
    }
    let status = mcp_status_of(&state, &auth);
    let _ = app.emit("mcp://status", &status);
    Ok(status)
}

/// Replace the allowlist and apply it, restarting the endpoint if it was up.
///
/// Restarting costs the connected sessions — rmcp bakes the allowed hosts into
/// the service when it is built, so there is no way to apply one to a live
/// server. The UI says so rather than letting a client drop mysteriously.
#[tauri::command]
fn mcp_set_allowed_hosts(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
    hosts: Vec<String>,
) -> Result<McpStatus, String> {
    let status = apply_allowed_hosts(&state, &app, &auth, hosts)?;
    let _ = app.emit("mcp://status", &status);
    Ok(status)
}

/// The shared apply path for the editable allowlist: normalise, persist, and
/// restart the connector if it was up. Used by the dashboard command and by
/// the tunnel watcher, so a discovered host and a typed host cannot behave
/// differently.
fn apply_allowed_hosts<R: tauri::Runtime>(
    state: &AppState,
    app: &AppHandle<R>,
    auth: &Arc<auth::Auth>,
    hosts: Vec<String>,
) -> Result<McpStatus, String> {
    let normalised = normalise_hosts(hosts);
    {
        let mut current = state.mcp_configured_hosts.lock().unwrap();
        if *current == normalised {
            return Ok(mcp_status_of(state, auth));
        }
        *current = normalised.clone();
    }
    let _ = db::set_setting(
        &state.conn.lock().unwrap(),
        "mcp_allowed_hosts",
        &serde_json::json!(normalised).to_string(),
    );
    if state.connector.is_running() {
        restart_connector(state, app, auth)?;
    }
    Ok(mcp_status_of(state, auth))
}

/// Normalise host values on the way in: a stray scheme, path or port in a
/// host value silently never matches a `Host` header, which is the failure
/// this whole path exists to make visible.
fn normalise_hosts(hosts: Vec<String>) -> Vec<String> {
    hosts
        .into_iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .map(|h| {
            let without_scheme = h.split_once("://").map(|(_, rest)| rest).unwrap_or(&h);
            let authority = without_scheme
                .split(['/', '?', '#'])
                .next()
                .unwrap_or(without_scheme);
            let host = authority.split(':').next().unwrap_or(authority);
            host.trim_end_matches('.').to_string()
        })
        .filter(|h| !h.is_empty())
        .collect()
}

/// Open or close the connector's write surface at runtime. This is the only
/// thing that moves `allow_write`, which seeds from `LEXSUS_MCP_ALLOW_WRITE`
/// at launch and is otherwise read-only-first.
#[tauri::command]
fn mcp_set_allow_write(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
    enabled: bool,
) -> McpStatus {
    state.mcp_allow_write.store(enabled, Ordering::SeqCst);
    let status = mcp_status_of(&state, &auth);
    // Announced like every other state change, so the rail and status bar stay
    // in step with a toggle made in the dashboard.
    let _ = app.emit("mcp://status", &status);
    status
}

/// Hand the connector's bearer token to the desktop so the user can copy it
/// into their MCP host.
///
/// This is the one place the token crosses a boundary, and that boundary is
/// Tauri IPC — same process, same user, and the desktop is already the sole
/// approval authority for every gated call. It is never logged, never put in
/// `mcp_status`, and never sent over the network.
#[tauri::command]
fn mcp_reveal_token(auth: State<'_, Arc<auth::Auth>>) -> String {
    auth.reveal()
}

/// Replace the connector's bearer token, immediately and persistently.
///
/// The previous token stops working the moment this returns — there is no
/// grace period, so a caller still holding it starts getting `401`. Fails
/// rather than half-applying if the new token cannot be stored, since a
/// rotation that did not persist would break on the next launch.
#[tauri::command]
fn mcp_rotate_token(
    state: State<'_, AppState>,
    auth: State<'_, Arc<auth::Auth>>,
) -> Result<McpStatus, String> {
    auth.rotate().map_err(|e| e.message())?;
    Ok(mcp_status_of(&state, &auth))
}

/// Set the handoff objective (editable in the handoff panel).
#[tauri::command]
fn set_objective(state: State<'_, AppState>, text: String) -> Result<(), String> {
    *state.objective.lock().unwrap() = Some(text);
    Ok(())
}

// --- public tunnel -----------------------------------------------------------

/// Which tunnel tools this machine has, so the UI offers what will work.
#[tauri::command]
fn tunnel_detect(state: State<'_, AppState>) -> Vec<tunnel::Detection> {
    let port = state.mcp_port.load(std::sync::atomic::Ordering::SeqCst);
    tunnel::Tunnel::detect(port)
}

#[tauri::command]
fn tunnel_status(state: State<'_, AppState>) -> tunnel::TunnelStatus {
    state.tunnel.status()
}

/// Start a tunnel to the connector and allowlist whatever host it publishes.
///
/// **This is the one action in the app that can put a local tool server on the
/// internet**, so it is explicit and never automatic. The connector itself is
/// unchanged by it: the bearer token is still required on every request, so a
/// tunnel widens reachability, not authority. The endpoint must already be up —
/// a tunnel to a port nothing is listening on would publish a URL that only
/// ever 502s.
///
/// Returns as soon as the process is spawned. The public URL is not known until
/// the provider prints it, which is why this also spawns a watcher: when the
/// host arrives it is appended to the allowlist, persisted, and the connector
/// restarts to apply it (rmcp bakes the allowlist in at construction).
#[tauri::command]
fn tunnel_start(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    auth: State<'_, Arc<auth::Auth>>,
    provider: String,
    command: Option<String>,
) -> Result<tunnel::TunnelStatus, String> {
    if !state.connector.is_listening() {
        return Err("the MCP connector is not running — start it before exposing it".to_string());
    }
    let port = state.mcp_port.load(std::sync::atomic::Ordering::SeqCst);
    let status = state
        .tunnel
        .start(app.clone(), &provider, command.as_deref(), port)?;

    // Watch for the host to be discovered, then allowlist it. The tunnel's
    // reader threads emit `tunnel://update`; this waits on the state instead of
    // subscribing, so it works whichever thread wins the race. The `Arc` is
    // cloned out of the `State` because the watcher outlives the command, and
    // a `State` handle does not.
    let app_for_watch = app.clone();
    let auth_for_watch: Arc<auth::Auth> = auth.inner().clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(45);
        while Instant::now() < deadline {
            let Some(state) = app_for_watch.try_state::<AppState>() else {
                return;
            };
            let tunnel_state = state.tunnel.status();
            if !tunnel_state.running {
                return; // exited on its own; the tunnel log already says so
            }
            let Some(host) = tunnel_state.host else {
                drop(state);
                thread::sleep(Duration::from_millis(250));
                continue;
            };
            let already = state.mcp_configured_hosts.lock().unwrap().contains(&host);
            if already {
                return;
            }
            let mut hosts = state.mcp_configured_hosts.lock().unwrap().clone();
            hosts.push(host.clone());
            // The same path the dashboard's edit takes, so the restart that
            // applies a discovered host cannot behave differently from a typed
            // one.
            if let Err(e) = apply_allowed_hosts(&state, &app_for_watch, &auth_for_watch, hosts) {
                eprintln!("[tunnel] could not allowlist {host}: {e}");
            }
            return;
        }
    });

    Ok(status)
}

/// Stop the tunnel. Idempotent, and called on quit too, so a tunnel can never
/// outlive the window that opened it.
///
/// Also withdraws the tunnel's discovered host from the allowlist, so a URL
/// that is no longer being forwarded stops being accepted — a stale host in a
/// DNS-rebinding allowlist is reachability nobody is guarding any more.
#[tauri::command]
fn tunnel_stop(state: State<'_, AppState>, app: tauri::AppHandle) -> tunnel::TunnelStatus {
    let host = state.tunnel.status().host;
    let status = state.tunnel.stop();
    let _ = app.emit(tunnel::UPDATE_EVENT, ());
    if let Some(host) = host {
        let mut hosts = state.mcp_configured_hosts.lock().unwrap().clone();
        if hosts.iter().any(|h| h == &host) {
            hosts.retain(|h| h != &host);
            let auth: Arc<auth::Auth> = app.state::<Arc<auth::Auth>>().inner().clone();
            if let Err(e) = apply_allowed_hosts(&state, &app, &auth, hosts) {
                eprintln!("[tunnel] could not withdraw {host} from the allowlist: {e}");
            }
        }
    }
    status
}

// --- activity ----------------------------------------------------------------

/// The tool catalogue, derived from `SPECS` so it cannot drift from the engine
/// that actually enforces it.
fn tool_surface() -> ToolSurface {
    let specs = bridge::SPECS;
    let mut groups: Vec<CountEntry> = Vec::new();
    let mut approvals: Vec<CountEntry> = Vec::new();
    let mut kinds: Vec<CountEntry> = Vec::new();
    for spec in specs {
        bump(&mut groups, spec.group);
        bump(&mut approvals, approval_name(spec.approval));
        bump(&mut kinds, spec.trace_kind.unwrap_or("untraced"));
    }
    sort_desc(&mut groups);
    sort_desc(&mut approvals);
    sort_desc(&mut kinds);
    // The read/write split is the MCP surface partition, not the approval
    // class: READ_ONLY is what a connector sees while writes are off, and
    // WRITE is the rest. Reusing `exposed_names` means the catalogue cannot
    // drift from the gate that actually enforces it.
    let read_only = mcp::exposed_names(false).len();
    let write = mcp::exposed_names(true).len() - read_only;
    ToolSurface {
        total: specs.len(),
        read_only,
        write,
        groups,
        approvals,
        kinds,
    }
}

fn approval_name(approval: bridge::Approval) -> &'static str {
    match approval {
        bridge::Approval::Auto => "Auto",
        bridge::Approval::SensitivePathOnly => "SensitivePathOnly",
        bridge::Approval::Always => "Always",
        bridge::Approval::Destructive => "Destructive",
    }
}

#[derive(Clone, serde::Serialize)]
struct CountEntry {
    name: String,
    count: usize,
}

fn bump(entries: &mut Vec<CountEntry>, name: &str) {
    match entries.iter_mut().find(|e| e.name == name) {
        Some(entry) => entry.count += 1,
        None => entries.push(CountEntry {
            name: name.to_string(),
            count: 1,
        }),
    }
}

fn sort_desc(entries: &mut [CountEntry]) {
    entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
}

#[derive(Clone, serde::Serialize)]
struct ToolSurface {
    total: usize,
    read_only: usize,
    write: usize,
    groups: Vec<CountEntry>,
    approvals: Vec<CountEntry>,
    kinds: Vec<CountEntry>,
}

/// Recent trace steps, newest first — read back from the database so the
/// activity history survives a reload. The live `trace://step` event is a
/// stream, not a record; this is the record.
#[tauri::command]
fn activity_trace(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::TraceRow>, String> {
    db::recent_trace(&state.conn.lock().unwrap(), limit.unwrap_or(200)).map_err(|e| e.to_string())
}

#[tauri::command]
fn activity_stats(state: State<'_, AppState>) -> Result<db::ActivityStats, String> {
    db::activity_stats(&state.conn.lock().unwrap()).map_err(|e| e.to_string())
}

#[tauri::command]
fn activity_tool_usage(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::ToolUsage>, String> {
    db::tool_usage(&state.conn.lock().unwrap(), limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn activity_files(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::FileTouch>, String> {
    db::file_touches(&state.conn.lock().unwrap(), limit.unwrap_or(50)).map_err(|e| e.to_string())
}

#[tauri::command]
fn activity_commands(
    state: State<'_, AppState>,
    limit: Option<usize>,
) -> Result<Vec<db::CommandRun>, String> {
    db::command_history(&state.conn.lock().unwrap(), limit.unwrap_or(50)).map_err(|e| e.to_string())
}

/// The static half of the dashboard: the tool surface, which is code, not data.
#[tauri::command]
fn activity_tool_surface() -> ToolSurface {
    tool_surface()
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
            connector: mcp::Connector::new(),
            tunnel: tunnel::Tunnel::new(),
            mcp_port: AtomicU16::new(mcp::DEFAULT_PORT),
            mcp_configured_hosts: Mutex::new(Vec::new()),
            mcp_bind_error: Mutex::new(None),
            bgproc: bgproc::Manager::new(),
            active_worktree: Mutex::new(None),
            lsp_clients: Mutex::new(HashMap::new()),
            app_handle: Mutex::new(None),
            question_seq: AtomicU64::new(1),
            questions: Mutex::new(HashMap::new()),
        })
        .setup(|app| {
            // Hand the app handle to the state so tools running on the
            // blocking pool can raise UI events (notifications, questions).
            {
                let state = app.state::<AppState>();
                *state.app_handle.lock().unwrap() = Some(app.handle().clone());
            }
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
            // Persisted connector config: the port and the editable allowlist
            // the user chose last time, so a relaunch comes back on the same
            // endpoint and still accepts the same tunnel host.
            let persisted_port = db::get_setting(&state.conn.lock().unwrap(), "mcp_port")
                .ok()
                .flatten()
                .and_then(|v| v.parse::<u16>().ok())
                .unwrap_or(mcp::DEFAULT_PORT);
            state
                .mcp_port
                .store(persisted_port, std::sync::atomic::Ordering::SeqCst);
            let persisted_hosts = db::get_setting(&state.conn.lock().unwrap(), "mcp_allowed_hosts")
                .ok()
                .flatten()
                .and_then(|v| serde_json::from_str::<Vec<String>>(&v).ok())
                .unwrap_or_default();
            *state.mcp_configured_hosts.lock().unwrap() = persisted_hosts;
            spawn_failover_ticker(app.handle().clone());
            // The durable endpoint: a desktop-local MCP server on loopback,
            // which a provider connector reaches through a tunnel.
            //
            // The secure layer comes up first, and the endpoint binds only if
            // it did. A token that cannot be generated is fatal here on
            // purpose: an endpoint that failed to bind is a visible, harmless
            // failure, while one that bound without authentication looks
            // exactly like it is working.
            let auth = auth::Auth::load_or_init(&dir)
                .map_err(|e| format!("refusing to start the MCP endpoint unauthenticated: {e}"))?;
            app.manage(auth.clone());
            // Auto-start from persisted config, preserving the read-only-first
            // posture. A bind failure is recorded in state (and reported by
            // `mcp_status`) rather than printed to stderr and forgotten.
            let _ = start_connector(&state, app.handle(), &auth);
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
            bridge_answer_question,
            bridge_audit,
            bridge_grant_state,
            bridge_grant_revoke,
            bridge_pause,
            cancel_request,
            processes_list,
            mcp_status,
            mcp_start,
            mcp_stop,
            mcp_restart,
            mcp_set_port,
            mcp_set_allowed_hosts,
            mcp_set_allow_write,
            mcp_reveal_token,
            mcp_rotate_token,
            tunnel_detect,
            tunnel_start,
            tunnel_stop,
            tunnel_status,
            activity_trace,
            activity_stats,
            activity_tool_usage,
            activity_files,
            activity_commands,
            activity_tool_surface,
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
    app.run(|handle, event| {
        // Never leave a spawned command behind: killing on ExitRequested
        // (not Exit — by then the process is torn down) lets the registry
        // TERM→KILL the process groups while we can still signal.
        if let tauri::RunEvent::ExitRequested { .. } = event {
            process::registry().kill_all(Duration::from_millis(500));
            // Background commands are their own roster, in state rather than
            // in the global registry above. State outlives this callback, so
            // the manager's own `Drop` is not enough on its own: we may be
            // exiting without ever running it.
            if let Some(state) = handle.try_state::<AppState>() {
                state.bgproc.kill_all();
                // Stop the connector and any public tunnel before the window
                // goes away: a tunnel must never outlive the app that opened
                // it, and the connector thread should be joined, not detached.
                let _ = state.connector.stop();
                let _ = state.tunnel.stop();
            }
        }
    });
}
