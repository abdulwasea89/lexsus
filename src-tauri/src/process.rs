//! Process registry.
//!
//! Every child process the app spawns — PTY shells running `run_command` — is
//! registered here with its pid and owning request. The registry answers three
//! questions the runtime cannot answer otherwise:
//!
//!   * what is running right now (`list`),
//!   * who owns it (`ProcessEntry.owner` — the WS request id),
//!   * how to stop it safely (`kill` — SIGTERM first, SIGKILL only after a
//!     grace period, and to the whole process *group* so a shell's children
//!     don't survive it).
//!
//! Before this existed, cancelling meant dropping the WebSocket connection:
//! the running command kept going until its own timeout, and `child.kill()`
//! on timeout killed only the shell process, not the pipeline it spawned.
//!
//! Safety rules baked in here, not at call sites:
//!   * never signal a pid that was not registered by this process;
//!   * never signal pid 1 / self / obviously wrong pids;
//!   * TERM before KILL, always, with a bounded grace window.

use serde::Serialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// What spawned the process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessKind {
    /// A `run_command` tool call executing in a PTY.
    Command,
}

impl ProcessKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProcessKind::Command => "command",
        }
    }
}

/// One registered child process.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessEntry {
    /// Registry-internal id (not the pid).
    pub id: u64,
    pub pid: u32,
    pub kind: ProcessKind,
    pub label: String,
    /// The WS request id that spawned it, if any (desktop calls have none).
    pub owner: Option<String>,
    pub started_at: String,
}

pub struct ProcessRegistry {
    processes: Mutex<HashMap<u64, ProcessEntry>>,
    next_id: AtomicU64,
}

/// The app-wide registry. Global because `bridge::execute` (which spawns the
/// PTY) has no access to Tauri state; registration must not depend on
/// threading an extra parameter through every layer.
static REGISTRY: std::sync::OnceLock<ProcessRegistry> = std::sync::OnceLock::new();

pub fn registry() -> &'static ProcessRegistry {
    REGISTRY.get_or_init(ProcessRegistry::new)
}

impl Default for ProcessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessRegistry {
    pub fn new() -> Self {
        Self {
            processes: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Register a child pid. Returns the registry id used for later
    /// `unregister` / `kill` calls.
    pub fn register(
        &self,
        pid: u32,
        kind: ProcessKind,
        label: impl Into<String>,
        owner: Option<String>,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let entry = ProcessEntry {
            id,
            pid,
            kind,
            label: label.into(),
            owner,
            started_at: now_string(),
        };
        self.processes.lock().unwrap().insert(id, entry);
        id
    }

    /// Remove a process from the registry (normal exit). Safe to call twice.
    pub fn unregister(&self, id: u64) {
        self.processes.lock().unwrap().remove(&id);
    }

    pub fn list(&self) -> Vec<ProcessEntry> {
        self.processes.lock().unwrap().values().cloned().collect()
    }

    pub fn get(&self, id: u64) -> Option<ProcessEntry> {
        self.processes.lock().unwrap().get(&id).cloned()
    }

    /// Registry ids of every process owned by a request.
    pub fn by_owner(&self, owner: &str) -> Vec<u64> {
        self.processes
            .lock()
            .unwrap()
            .values()
            .filter(|e| e.owner.as_deref() == Some(owner))
            .map(|e| e.id)
            .collect()
    }

    /// Gracefully stop a registered process: SIGTERM to the process group,
    /// then SIGKILL after `grace` if it is still alive. Returns false when
    /// no such registry entry exists — we never signal pids we did not
    /// register.
    pub fn kill(&self, id: u64, grace: Duration) -> bool {
        let entry = match self.get(id) {
            Some(e) => e,
            None => return false,
        };
        kill_tree(entry.pid, grace);
        self.unregister(id);
        true
    }

    /// Stop everything owned by a request (used by the `cancel` frame).
    pub fn kill_owner(&self, owner: &str, grace: Duration) -> usize {
        let ids = self.by_owner(owner);
        let n = ids.len();
        for id in ids {
            self.kill(id, grace);
        }
        n
    }

    /// Stop every registered process (app shutdown).
    pub fn kill_all(&self, grace: Duration) {
        let ids: Vec<u64> = self.processes.lock().unwrap().keys().copied().collect();
        for id in ids {
            self.kill(id, grace);
        }
    }
}

fn now_string() -> String {
    // `datetime('now')`-compatible UTC timestamp without pulling in a date
    // crate: SQLite will store what we give it; second precision is fine.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

// --- execution ownership -----------------------------------------------------

thread_local! {
    /// The WS request id whose tool call is executing on *this* thread, if
    /// any. Set by the ws.rs handler around `tool_call` so the PTY spawned
    /// deep inside `bridge::execute` can be attributed to the request without
    /// threading the id through every layer.
    static EXECUTION_OWNER: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Attribute everything spawned on this thread to `owner` (a WS request id).
/// The ws.rs handler sets it before executing a tool call and clears it
/// after; desktop-originated calls leave it unset.
pub fn set_execution_owner(owner: Option<String>) {
    EXECUTION_OWNER.with(|o| *o.borrow_mut() = owner);
}

/// The request id that owns whatever this thread is about to spawn.
pub fn execution_owner() -> Option<String> {
    EXECUTION_OWNER.with(|o| o.borrow().clone())
}

// --- signal handling ---------------------------------------------------------

/// Pids that must never be signalled no matter what.
fn pid_is_protected(pid: u32) -> bool {
    pid <= 1 || pid == std::process::id()
}

#[cfg(unix)]
fn kill_tree(pid: u32, grace: Duration) {
    if pid_is_protected(pid) {
        return;
    }
    // PTY children are session leaders, so the process group id equals the
    // child pid and `kill(-pid, …)` reaches the whole tree (the shell *and*
    // the pipeline it spawned). If the group signal fails (ESRCH), fall back
    // to the single pid.
    unsafe {
        if libc::kill(-(pid as i32), libc::SIGTERM) != 0 {
            libc::kill(pid as i32, libc::SIGTERM);
        }
    }
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        if !alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if alive(pid) {
        unsafe {
            if libc::kill(-(pid as i32), libc::SIGKILL) != 0 {
                libc::kill(pid as i32, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(not(unix))]
fn kill_tree(pid: u32, _grace: Duration) {
    if pid_is_protected(pid) {
        return;
    }
    // Windows: no process groups; terminate the single process (and its
    // children, via /T).
    if let Ok(p) = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .status()
    {
        let _ = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_and_unregister() {
        let reg = ProcessRegistry::new();
        let id = reg.register(999_999, ProcessKind::Command, "test", None);
        assert_eq!(reg.list().len(), 1);
        reg.unregister(id);
        assert!(reg.list().is_empty());
        reg.unregister(id); // idempotent
    }

    #[test]
    fn by_owner_filters() {
        let reg = ProcessRegistry::new();
        reg.register(999_998, ProcessKind::Command, "a", Some("req_1".into()));
        reg.register(999_997, ProcessKind::Command, "b", Some("req_2".into()));
        reg.register(999_996, ProcessKind::Command, "c", None);
        assert_eq!(reg.by_owner("req_1").len(), 1);
        assert_eq!(reg.by_owner("nope").len(), 0);
    }

    #[test]
    fn kill_unknown_id_is_a_no_op() {
        let reg = ProcessRegistry::new();
        assert!(!reg.kill(42, Duration::from_millis(10)));
    }

    #[test]
    fn protected_pids_are_never_signalled() {
        assert!(pid_is_protected(1));
        assert!(pid_is_protected(std::process::id()));
        assert!(!pid_is_protected(999_999));
    }

    #[test]
    fn execution_owner_roundtrip() {
        assert!(execution_owner().is_none());
        set_execution_owner(Some("req_9".into()));
        assert_eq!(execution_owner().as_deref(), Some("req_9"));
        set_execution_owner(None);
        assert!(execution_owner().is_none());
    }
}
