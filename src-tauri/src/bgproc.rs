//! Background commands.
//!
//! `run_command` is a one-shot: the call stays open until the command exits,
//! so a dev server, a watcher, or anything meant to run until it is stopped
//! can never be started with it — the caller waits out the timeout and gets a
//! killed process instead of a live one. This module is the other half: start
//! without waiting, read what has been written since a cursor, and stop it
//! along with everything it spawned.
//!
//! Three termination modes, one cleanup path. A command can end because it ran
//! out of work (End), because `kill_command` stopped it (Kill), or because the
//! runtime failed around it (Error) — and all three go through the same reap.
//! The process group is torn down by a `Drop` guard rather than by a call
//! somebody has to remember to make, which is the bracket from the book's
//! `resource(acquire)(use)(release)`: Rust's `Drop` *is* the release half.
//!
//! Output is a bounded window over the stream rather than an unbounded buffer,
//! because a command that prints forever must not be able to grow the app's
//! memory without limit. The window slides; the caller's cursor is an
//! **absolute** byte offset, so sliding it renumbers nothing and a reader that
//! fell behind is told its bytes are gone rather than handed a stream with a
//! hole at the front it cannot see.

use crate::shell::Shell;

use portable_pty::{native_pty_system, PtySize};

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Bytes of output one command keeps. Older bytes are dropped from the front,
/// and a reader is told so rather than silently losing the beginning.
const WINDOW_BYTES: usize = 64 * 1024;

/// Bytes one read returns at most. The window bounds what is *kept*; this
/// bounds what is *sent*, so reading a chatty command does not spend the
/// caller's whole context on it.
const READ_BYTES: usize = 16 * 1024;

/// How much a single `read()` pulls out of the PTY at a time.
const CHUNK: usize = 4096;

/// Concurrent background commands. A live command is a resource the caller
/// cannot see, so this cap refuses rather than warns.
const MAX_RUNNING: usize = 8;

/// Finished commands kept readable. Past this the oldest are released.
const MAX_RETAINED: usize = 32;

/// How many released ids to remember. Bounded like everything else here: the
/// distinction only has to survive long enough to explain one failed read.
const MAX_EVICTED: usize = 64;

/// SIGTERM→SIGKILL grace when a command is stopped.
const KILL_GRACE: Duration = Duration::from_millis(500);

/// How long `kill` waits for the reap before answering anyway. Longer than
/// `KILL_GRACE` because the escalation itself has to finish first.
const REAP_WAIT: Duration = Duration::from_millis(1200);

// --- the output window -------------------------------------------------------

/// A bounded ring of the bytes a command has produced, addressed by absolute
/// offsets from the start of the stream.
///
/// The state is the pair `(dropped, kept)`, and every push advances it by one
/// chunk — the fold the book writes as `loop(z)(f)`. Keeping the *absolute*
/// offset rather than an index into the buffer is what makes it addressable
/// while it slides: a caller hands back the cursor it was given, and either
/// gets the bytes from there or is told they are gone.
#[derive(Debug)]
struct Window {
    buf: VecDeque<u8>,
    /// Absolute offset of `buf.front()`. Everything before it was dropped.
    start: u64,
    /// Absolute offset one past the last byte written.
    end: u64,
    cap: usize,
}

/// What one read yields.
#[derive(Debug)]
struct Drain {
    bytes: Vec<u8>,
    /// Offset to read from next.
    next: u64,
    /// Bytes before the caller's cursor were dropped before it could read
    /// them.
    lost: bool,
    /// Bytes remain past `next`.
    more: bool,
}

impl Window {
    fn new(cap: usize) -> Self {
        debug_assert!(cap > 0, "a window that keeps nothing cannot be read from");
        Self {
            buf: VecDeque::new(),
            start: 0,
            end: 0,
            cap: cap.max(1),
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        self.end += bytes.len() as u64;
        self.buf.extend(bytes.iter().copied());
        // Drop from the *front*: a reader that has fallen behind loses the
        // oldest output, which is the right end to lose, because the newest
        // output is the one that explains what the command is doing now.
        if self.buf.len() > self.cap {
            let excess = self.buf.len() - self.cap;
            self.buf.drain(..excess);
            self.start += excess as u64;
        }
    }

    fn drain(&self, cursor: u64, max_bytes: usize) -> Drain {
        // Clamped at both ends. A cursor past the end is a caller error but a
        // harmless one — it reads nothing — and clamping is what keeps the
        // subtraction below from wrapping.
        let from = cursor.clamp(self.start, self.end);
        let skip = (from - self.start) as usize;
        let take = (self.end - from).min(max_bytes as u64) as usize;
        let bytes: Vec<u8> = self.buf.iter().skip(skip).take(take).copied().collect();
        let next = from + bytes.len() as u64;
        Drain {
            bytes,
            next,
            lost: cursor < self.start,
            more: next < self.end,
        }
    }
}

// --- lifecycle ---------------------------------------------------------------

/// How a command ended. The three modes stay distinct so a reader can tell
/// "it finished" from "you stopped it" from "it never really started" — a
/// single `running: false` would collapse all of that into one useless bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Running,
    /// Exited on its own.
    Exited {
        code: Option<i32>,
    },
    /// Stopped by `kill_command`.
    Killed {
        code: Option<i32>,
    },
    /// The runtime failed around it.
    Failed {
        message: String,
    },
}

impl Status {
    /// The wire word for this status.
    pub fn state(&self) -> &'static str {
        match self {
            Status::Running => "running",
            Status::Exited { .. } => "exited",
            Status::Killed { .. } => "killed",
            Status::Failed { .. } => "failed",
        }
    }

    pub fn exit_code(&self) -> Option<i32> {
        match self {
            Status::Running => None,
            Status::Exited { code } | Status::Killed { code } => *code,
            // A command the runtime could not run has no exit code to report;
            // saying 1 here would invent one.
            Status::Failed { .. } => None,
        }
    }

    fn is_running(&self) -> bool {
        matches!(self, Status::Running)
    }
}

/// Kills the process group when dropped.
///
/// Held by the waiter thread for the life of the child, so a panic or an
/// early return inside that thread still tears the group down. Disarmed once
/// the child is reaped, because after a reap the pid is free to be reused and
/// a signal would no longer be aimed at us.
struct KillOnDrop {
    pid: Option<u32>,
    armed: Arc<AtomicBool>,
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if self.armed.load(Ordering::SeqCst) {
            if let Some(pid) = self.pid {
                crate::process::kill_tree(pid, KILL_GRACE);
            }
        }
    }
}

/// One command's live state. Shared with its two threads; the manager only
/// ever holds the `Arc`.
struct Job {
    id: u64,
    command: String,
    pid: Option<u32>,
    started: Instant,
    /// The request id this command was started under (`process::execution_owner`
    /// at start time), if any. This manager keeps its own registry rather than
    /// using `process::registry`, so ownership has to be recorded here too —
    /// otherwise `cancel_request` would reach a `run_command`'s shell but not
    /// the background command started by the same caller.
    owner: Option<String>,
    status: Mutex<Status>,
    window: Mutex<Window>,
    /// Set by the reader thread when the PTY reaches EOF. Deliberately not
    /// folded into `status`: the child being reaped and the stream being
    /// drained are two different events that finish at different times, and
    /// the gap between them is exactly where "there might be more output"
    /// lives.
    drained: AtomicBool,
    /// Set before a kill is signalled, so the reaper can label the ending
    /// `Killed` rather than `Exited`. The two run concurrently, so a command
    /// that exits at the same instant it is killed may be labelled either
    /// way — inherent to the situation, not to this code.
    killed: AtomicBool,
    /// Whether this command is still ours to signal. Cleared by the waiter
    /// thread the moment the child is reaped.
    armed: Arc<AtomicBool>,
}

impl Job {
    fn is_running(&self) -> bool {
        self.status.lock().unwrap().is_running()
    }

    /// Stop the process group. `armed` is the authority rather than `status`:
    /// it is cleared by the reaper, so it never claims a live process after
    /// the reap. Returns false when there is nothing left to signal.
    fn signal(&self, grace: Duration) -> bool {
        let Some(pid) = self.pid else {
            return false;
        };
        if !self.armed.load(Ordering::SeqCst) {
            return false;
        }
        crate::process::kill_tree(pid, grace);
        true
    }

    fn snapshot(&self, cursor: u64) -> Snapshot {
        let status = self.status.lock().unwrap().clone();
        let drain = self.window.lock().unwrap().drain(cursor, READ_BYTES);
        Snapshot {
            id: self.id,
            command: self.command.clone(),
            pid: self.pid,
            output: String::from_utf8_lossy(&drain.bytes).into_owned(),
            cursor,
            next_cursor: drain.next,
            lost: drain.lost,
            more: drain.more,
            // Ended *and* drained. A reaped child whose reader is still
            // draining has output on the way, so claiming completeness here
            // would be the exact lie this field exists to prevent.
            complete: !status.is_running() && self.drained.load(Ordering::SeqCst),
            status,
            elapsed_ms: self.started.elapsed().as_millis() as u64,
        }
    }
}

/// One read of a command's output.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub id: u64,
    pub command: String,
    pub pid: Option<u32>,
    pub status: Status,
    /// The bytes at and after `cursor`, up to the read cap.
    pub output: String,
    /// Where this read started.
    pub cursor: u64,
    /// Pass back as `cursor` to read only what is new.
    pub next_cursor: u64,
    pub lost: bool,
    pub more: bool,
    pub complete: bool,
    pub elapsed_ms: u64,
}

/// A command that was started.
#[derive(Debug, Clone)]
pub struct Started {
    pub id: u64,
    pub pid: Option<u32>,
    pub command: String,
}

/// What one `kill` did.
#[derive(Debug, Clone)]
pub struct KillOutcome {
    pub id: u64,
    pub command: String,
    /// True when the command had already ended, so nothing was signalled.
    pub already_finished: bool,
    pub status: Status,
}

/// Why a lookup failed. Kept apart because they are different facts, and only
/// one of them means the caller's id was wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// No command with that id was ever started.
    Unknown,
    /// There was one; it finished long enough ago that its output has been
    /// released.
    Evicted,
}

/// Why a command could not be started.
#[derive(Debug, Clone)]
pub enum StartError {
    /// Too many commands are already live.
    Busy { running: usize, cap: usize },
    /// The PTY or the child could not be created.
    Spawn(String),
}

/// The background commands of one app process.
///
/// Resident in `AppState` rather than in a global, so two app instances — and
/// two tests — cannot see each other's commands.
pub struct Manager {
    jobs: Mutex<Jobs>,
    next_id: AtomicU64,
}

#[derive(Default)]
struct Jobs {
    live: HashMap<u64, Arc<Job>>,
    /// Ids whose output has been released. Remembered so a read of an old id
    /// is answered "that output is gone" rather than "no such command" — the
    /// first tells the caller its id was right, which is the fact it needs to
    /// decide whether to start the command again.
    evicted: VecDeque<u64>,
}

impl Default for Manager {
    fn default() -> Self {
        Self::new()
    }
}

impl Manager {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(Jobs::default()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Start a command and return without waiting for it.
    ///
    /// The whole start happens under the registry lock. Spawning a child is
    /// sub-millisecond, and holding the lock is what makes the running-count
    /// check and the insertion one step — otherwise two callers could each
    /// see room for the last slot and both take it.
    ///
    /// The owner is read from the thread rather than passed in, so the
    /// attribution cannot be forgotten at a call site: whatever request owns
    /// this thread owns the command it starts, exactly as for the PTY in
    /// `bridge::execute`.
    pub fn start(&self, shell: Shell, command: &str, cwd: &Path) -> Result<Started, StartError> {
        let owner = crate::process::execution_owner();
        let mut jobs = self.jobs.lock().unwrap();
        jobs.retire_finished();

        let running = jobs.live.values().filter(|j| j.is_running()).count();
        if running >= MAX_RUNNING {
            return Err(StartError::Busy {
                running,
                cap: MAX_RUNNING,
            });
        }

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| StartError::Spawn(e.to_string()))?;
        let mut builder = shell.run_command(command);
        builder.cwd(cwd);
        let child = pair
            .slave
            .spawn_command(builder)
            .map_err(|e| StartError::Spawn(e.to_string()))?;
        drop(pair.slave);

        let pid = child.process_id();
        // Armed before anything else can fail: every early return below now
        // takes the child's process group down with it, so a failure to set
        // up the reader cannot leak a running command.
        let armed = Arc::new(AtomicBool::new(true));
        let guard = KillOnDrop {
            pid,
            armed: Arc::clone(&armed),
        };

        let reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| StartError::Spawn(e.to_string()))?;
        drop(pair.master);

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let job = Arc::new(Job {
            id,
            command: command.to_string(),
            pid,
            started: Instant::now(),
            owner,
            status: Mutex::new(Status::Running),
            window: Mutex::new(Window::new(WINDOW_BYTES)),
            drained: AtomicBool::new(false),
            killed: AtomicBool::new(false),
            armed: Arc::clone(&armed),
        });
        jobs.live.insert(id, Arc::clone(&job));

        // Spawned while the lock is held, so a job in the map always has its
        // threads running: nothing can observe a registered command that has
        // no reader behind it.
        //
        // Both threads hold the `Arc<Job>`, which is what keeps the window
        // alive for a reader that is still draining a command the manager has
        // already released.
        thread::spawn({
            let job = Arc::clone(&job);
            move || read_loop(reader, job)
        });
        thread::spawn(move || wait_loop(child, job, armed, guard));

        Ok(Started {
            id,
            pid,
            command: command.to_string(),
        })
    }

    /// Read a command's output from `cursor`.
    pub fn snapshot(&self, id: u64, cursor: u64) -> Result<Snapshot, Lookup> {
        let job = self.job(id)?;
        Ok(job.snapshot(cursor))
    }

    /// Stop a command and everything it spawned.
    ///
    /// Waits for the reap before answering, so the result carries the real
    /// ending instead of "still running" — Kill converted back into an
    /// ordinary termination, as the book's `kill` helper does. The output is
    /// not drained away: it stays in the window, where the caller can still
    /// read it.
    pub fn kill(&self, id: u64) -> Result<KillOutcome, Lookup> {
        let job = self.job(id)?;
        let before = job.status.lock().unwrap().clone();
        if !before.is_running() {
            return Ok(KillOutcome {
                id,
                command: job.command.clone(),
                already_finished: true,
                status: before,
            });
        }

        // Set before signalling: the reaper may observe the exit at any
        // moment, and it is this flag — not the timing — that decides whether
        // the ending is labelled a kill.
        job.killed.store(true, Ordering::SeqCst);
        job.signal(KILL_GRACE);

        let deadline = Instant::now() + REAP_WAIT;
        let mut status = before;
        while Instant::now() < deadline {
            let now = job.status.lock().unwrap().clone();
            if !now.is_running() {
                status = now;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        Ok(KillOutcome {
            id,
            command: job.command.clone(),
            already_finished: false,
            status,
        })
    }

    /// Stop every live command, and report how many were stopped. Called when
    /// the app shuts down — a process this app started must not outlive it.
    pub fn kill_all(&self) -> usize {
        self.kill_matching(|_| true)
    }

    /// Stop every live command started by `owner`, and report how many were
    /// stopped. The background half of `cancel_request`: a caller giving up on
    /// a tool call must not leave that call's commands running.
    pub fn kill_owner(&self, owner: &str) -> usize {
        self.kill_matching(|j| j.owner.as_deref() == Some(owner))
    }

    /// Signal every live command matching `which`, with the `kill_all` /
    /// `kill_owner` policy — mark it killed (so the reaper labels the ending
    /// rather than the timing) and signal the group. Selection happens under
    /// the lock; signalling does not, since it waits on process death.
    fn kill_matching(&self, which: impl Fn(&Job) -> bool) -> usize {
        let live: Vec<Arc<Job>> = {
            let jobs = self.jobs.lock().unwrap();
            jobs.live
                .values()
                .filter(|j| j.is_running() && which(j))
                .cloned()
                .collect()
        };
        let n = live.len();
        for job in live {
            job.killed.store(true, Ordering::SeqCst);
            job.signal(KILL_GRACE);
        }
        n
    }

    fn job(&self, id: u64) -> Result<Arc<Job>, Lookup> {
        let jobs = self.jobs.lock().unwrap();
        if let Some(job) = jobs.live.get(&id) {
            return Ok(Arc::clone(job));
        }
        Err(if jobs.evicted.contains(&id) {
            Lookup::Evicted
        } else {
            Lookup::Unknown
        })
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // Load-bearing, not belt-and-braces: the waiter thread holds the
        // `KillOnDrop` guard but is blocked in `wait()`, so it cannot fire
        // until the command ends on its own. Dropping the manager is the
        // event that says it never will.
        self.kill_all();
    }
}

impl Jobs {
    /// Release the oldest finished commands once too many have piled up.
    ///
    /// Finished, not merely old: a running command is the one thing a caller
    /// cannot reconstruct, so it is never released to make room. Eviction in
    /// id order keeps it deterministic — the same commands disappear in the
    /// same sequence every run.
    fn retire_finished(&mut self) {
        let mut finished: Vec<u64> = self
            .live
            .iter()
            .filter(|(_, j)| !j.is_running())
            .map(|(id, _)| *id)
            .collect();
        if finished.len() <= MAX_RETAINED {
            return;
        }
        finished.sort_unstable();
        let excess = finished.len() - MAX_RETAINED;
        for id in &finished[..excess] {
            self.live.remove(id);
            self.evicted.push_back(*id);
            while self.evicted.len() > MAX_EVICTED {
                self.evicted.pop_front();
            }
        }
    }
}

// --- the two threads ---------------------------------------------------------

/// Pump the PTY into the window until the stream ends.
fn read_loop(mut reader: Box<dyn Read + Send>, job: Arc<Job>) {
    let mut buf = [0u8; CHUNK];
    loop {
        match reader.read(&mut buf) {
            // EOF, or EIO after the last slave fd closed — both mean the
            // stream is over.
            Ok(0) | Err(_) => break,
            Ok(n) => match job.window.lock() {
                Ok(mut w) => w.push(&buf[..n]),
                // Poisoned by a panic elsewhere: stop rather than spin.
                Err(_) => break,
            },
        }
    }
    job.drained.store(true, Ordering::SeqCst);
}

/// Reap the child, then record how it ended.
fn wait_loop(
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    job: Arc<Job>,
    armed: Arc<AtomicBool>,
    guard: KillOnDrop,
) {
    let status = match child.wait() {
        Ok(exit) => {
            let code = Some(exit.exit_code() as i32);
            if job.killed.load(Ordering::SeqCst) {
                Status::Killed { code }
            } else {
                Status::Exited { code }
            }
        }
        Err(e) => Status::Failed {
            message: e.to_string(),
        },
    };
    // Disarm first: the child is reaped, so the pid is free to be reused and
    // any signal from here on would be aimed at someone else's process.
    armed.store(false, Ordering::SeqCst);
    *job.status.lock().unwrap() = status;
    drop(guard);
}

// --- rendering ---------------------------------------------------------------

/// The one-line ending, or how long it has been running.
///
/// Takes the two facts rather than a whole [`Snapshot`], because `kill`
/// reports an ending for a command it has no output for — building a
/// throwaway snapshot to reuse this would be a helper with a caller-shaped
/// hole in it.
fn status_phrase(status: &Status, elapsed_ms: u64) -> String {
    let secs = elapsed_ms as f64 / 1000.0;
    match status {
        Status::Running => format!("running for {secs:.1}s"),
        Status::Exited { code: Some(c) } => format!("exited with code {c} after {secs:.1}s"),
        Status::Exited { code: None } => format!("exited after {secs:.1}s"),
        Status::Killed { code: Some(c) } => format!("stopped after {secs:.1}s (exit code {c})"),
        Status::Killed { code: None } => format!("stopped after {secs:.1}s"),
        Status::Failed { message } => format!("the runtime failed around it: {message}"),
    }
}

/// The text half of a `command_output` result: the bytes themselves, then an
/// honest footer about where they came from.
pub(crate) fn render_snapshot(s: &Snapshot) -> String {
    let mut out = String::new();
    if s.lost {
        out.push_str(
            "[earlier output was dropped — this command has produced more than is kept]\n",
        );
    }
    out.push_str(&s.output);
    if !s.output.is_empty() && !s.output.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "[command #{} — {}",
        s.id,
        status_phrase(&s.status, s.elapsed_ms)
    ));
    if s.more {
        out.push_str("; more output is waiting");
    } else if s.output.is_empty() {
        // True whether or not the command ever wrote anything: the claim is
        // about this cursor, not about the command.
        out.push_str(&format!("; nothing written since cursor {}", s.cursor));
    }
    out.push_str(&format!("; next cursor {}]\n", s.next_cursor));
    out
}

/// The text half of `kill_command`.
pub(crate) fn render_kill(k: &KillOutcome) -> String {
    // `KillOutcome` carries no duration: the command is over by the time this
    // renders, and the phrase's "after Ns" would be the age of a clock we no
    // longer have. Zero reads as "no duration to report", which is the truth
    // here rather than a fabricated one.
    let phrase = status_phrase(&k.status, 0);
    if k.already_finished {
        // The post-condition — it is not running — already held. Saying so is
        // more useful than a bare success, and there is nothing to redo.
        format!(
            "command #{} had already finished ({}); nothing to stop\n",
            k.id, phrase
        )
    } else {
        format!(
            "stopped command #{} ({}); its output is still readable\n",
            k.id, phrase
        )
    }
}

// --- tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    /// A command that runs until it is stopped, in whatever shell this
    /// platform has.
    fn sleeper() -> String {
        match Shell::detect() {
            Shell::PowerShell => "Start-Sleep -Seconds 30".to_string(),
            Shell::Cmd => "timeout /t 30 /nobreak".to_string(),
            _ => "sleep 30".to_string(),
        }
    }

    fn echo(text: &str) -> String {
        match Shell::detect() {
            Shell::PowerShell => format!("Write-Output '{text}'"),
            _ => format!("echo {text}"),
        }
    }

    /// Poll until the predicate holds, up to a short deadline. Used where the
    /// fact being asserted arrives on another thread.
    fn eventually(f: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if f() {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        f()
    }

    #[cfg(unix)]
    fn pid_alive(pid: u32) -> bool {
        // Signal 0 delivers nothing; it only asks the kernel whether the pid
        // exists. Over pids we spawned, failure is ESRCH.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    // --- the window, on its own (no processes involved) ----------------------

    #[test]
    fn a_window_keeps_only_its_newest_bytes_and_says_what_it_dropped() {
        let mut w = Window::new(10);
        w.push(b"0123456789");
        assert_eq!(w.drain(0, 100).bytes, b"0123456789");

        w.push(b"abcde");
        let d = w.drain(0, 100);
        // The oldest five are gone; the five kept are the newest five bytes
        // of the stream, in order.
        assert_eq!(d.bytes, b"56789abcde");
        assert!(d.lost, "a reader that fell behind must be told");
        assert_eq!(d.next, 15);
        assert!(!d.more);
    }

    #[test]
    fn a_cursor_is_absolute_so_sliding_the_window_renumbers_nothing() {
        // Big enough that the cursor survives the slide, so this is about
        // renumbering and not about loss — that has its own test above.
        let mut w = Window::new(20);
        w.push(b"0123456789");
        let first = w.drain(0, 4);
        assert_eq!(first.bytes, b"0123");
        assert_eq!(first.next, 4);
        assert!(first.more, "six more bytes are already waiting");

        // The window slides underneath the reader …
        w.push(b"abcde");
        // … and the cursor still means the same offset it did before.
        let second = w.drain(first.next, 100);
        assert_eq!(second.bytes, b"456789abcde");
        assert!(!second.lost, "cursor 4 is still inside the window");
        assert!(!second.more);
    }

    #[test]
    fn a_read_is_capped_and_reports_that_more_awaits() {
        let mut w = Window::new(100);
        w.push(b"0123456789");
        let d = w.drain(0, 4);
        assert_eq!(d.bytes, b"0123");
        assert_eq!(d.next, 4);
        assert!(d.more);
        assert!(!d.lost);
    }

    #[test]
    fn a_cursor_past_the_end_reads_nothing_rather_than_panicking() {
        let mut w = Window::new(10);
        w.push(b"abc");
        let d = w.drain(99, 10);
        assert!(d.bytes.is_empty());
        assert_eq!(d.next, 3, "it clamps to the end, it does not run past it");
        assert!(!d.lost);
        assert!(!d.more);
    }

    #[test]
    fn an_empty_window_reads_empty_at_cursor_zero() {
        let w = Window::new(10);
        let d = w.drain(0, 10);
        assert!(d.bytes.is_empty());
        assert_eq!(d.next, 0);
        assert!(!d.lost && !d.more);
    }

    // --- eviction, over a stand-in registry ----------------------------------
    //
    // Pure bookkeeping, so it is checked without spawning anything: the whole
    // point is what happens at MAX_RETAINED, and reaching that with real
    // processes would make the test about the platform rather than the rule.

    fn fake_job(id: u64, status: Status) -> Arc<Job> {
        owned_fake_job(id, status, None)
    }

    fn owned_fake_job(id: u64, status: Status, owner: Option<&str>) -> Arc<Job> {
        Arc::new(Job {
            id,
            command: format!("cmd {id}"),
            pid: None,
            started: Instant::now(),
            owner: owner.map(str::to_string),
            status: Mutex::new(status),
            window: Mutex::new(Window::new(16)),
            drained: AtomicBool::new(true),
            killed: AtomicBool::new(false),
            armed: Arc::new(AtomicBool::new(false)),
        })
    }

    #[test]
    fn finished_commands_are_released_oldest_first_and_only_when_too_many() {
        let mut jobs = Jobs::default();
        for id in 1..=(MAX_RETAINED as u64 + 3) {
            jobs.live
                .insert(id, fake_job(id, Status::Exited { code: Some(0) }));
        }
        jobs.retire_finished();

        assert_eq!(jobs.live.len(), MAX_RETAINED);
        assert_eq!(
            jobs.evicted.iter().copied().collect::<Vec<_>>(),
            vec![1, 2, 3],
            "the oldest finished commands go first"
        );
        assert!(jobs.live.contains_key(&(MAX_RETAINED as u64 + 3)));
    }

    /// Ownership selects exactly the live commands of one request: another
    /// request's command and an unowned one must both survive, and a command
    /// that has already finished is not re-stopped. `signal` is a no-op on
    /// these stand-ins (no pid), which is what makes this about the
    /// *selection* and nothing else — there is no reaper here to retire the
    /// ones already signalled.
    #[test]
    fn kill_owner_selects_only_that_owners_live_commands() {
        let manager = Manager::new();
        {
            let mut jobs = manager.jobs.lock().unwrap();
            jobs.live
                .insert(1, owned_fake_job(1, Status::Running, Some("req_a")));
            jobs.live
                .insert(2, owned_fake_job(2, Status::Running, Some("req_b")));
            jobs.live.insert(
                3,
                owned_fake_job(3, Status::Exited { code: Some(0) }, Some("req_a")),
            );
            jobs.live
                .insert(4, owned_fake_job(4, Status::Running, None));
        }

        assert_eq!(
            manager.kill_owner("req_a"),
            1,
            "only the one live command of req_a, not its finished one"
        );
        assert_eq!(manager.kill_owner("nobody"), 0);
        assert_eq!(
            manager.kill_all(),
            3,
            "req_b's and the unowned command were never cancelled"
        );
    }

    /// A command that has ended is not *complete* until its reader has
    /// finished. The child being reaped and the stream being drained are two
    /// events that finish at different times, and the gap between them is the
    /// tail of a command that prints and exits — exactly the output a caller
    /// that stops at `complete` would lose.
    #[test]
    fn an_ended_command_is_not_complete_until_its_reader_has_drained() {
        let ended = owned_fake_job(1, Status::Exited { code: Some(0) }, None);
        ended.drained.store(false, Ordering::SeqCst);
        assert!(
            !ended.snapshot(0).complete,
            "reaped, but the reader is still draining what it wrote"
        );
        ended.drained.store(true, Ordering::SeqCst);
        assert!(ended.snapshot(0).complete, "ended and drained is complete");

        let running = owned_fake_job(2, Status::Running, None);
        assert!(
            !running.snapshot(0).complete,
            "drained, but the command is still running"
        );
    }

    #[test]
    fn a_running_command_is_never_released_to_make_room() {
        let mut jobs = Jobs::default();
        // One live command, oldest of all, buried under finished ones.
        jobs.live.insert(1, fake_job(1, Status::Running));
        for id in 2..=(MAX_RETAINED as u64 + 2) {
            jobs.live
                .insert(id, fake_job(id, Status::Exited { code: Some(0) }));
        }
        jobs.retire_finished();

        assert!(
            jobs.live.contains_key(&1),
            "a running command is the one thing the caller cannot reconstruct"
        );
        assert!(jobs.live.len() <= MAX_RETAINED + 1);
    }

    #[test]
    fn an_evicted_id_is_remembered_so_a_late_read_can_say_which_kind_of_missing() {
        let manager = Manager::new();
        {
            let mut jobs = manager.jobs.lock().unwrap();
            jobs.evicted.push_back(7);
        }
        assert_eq!(manager.snapshot(7, 0).unwrap_err(), Lookup::Evicted);
        assert_eq!(
            manager.snapshot(8, 0).unwrap_err(),
            Lookup::Unknown,
            "an id that never existed is a different answer from one that was released"
        );
    }

    #[test]
    fn the_evicted_list_is_itself_bounded() {
        let mut jobs = Jobs::default();
        for id in 1..=(MAX_EVICTED as u64 + 5) {
            jobs.evicted.push_back(id);
            while jobs.evicted.len() > MAX_EVICTED {
                jobs.evicted.pop_front();
            }
        }
        assert_eq!(jobs.evicted.len(), MAX_EVICTED);
        assert_eq!(jobs.evicted.front().copied(), Some(6));
    }

    // --- the real thing ------------------------------------------------------

    #[test]
    fn a_background_command_runs_and_its_output_reads_back() {
        let m = Manager::new();
        let started = m
            .start(Shell::detect(), &echo("bgproc-hello"), &tmpdir())
            .unwrap();

        let s = eventually(|| {
            let s = m.snapshot(started.id, 0).unwrap();
            s.complete
        })
        .then(|| m.snapshot(started.id, 0).unwrap())
        .expect("the command should have finished");

        assert!(s.output.contains("bgproc-hello"), "read: {:?}", s.output);
        assert!(!s.lost && !s.more);
        assert_eq!(s.status.state(), "exited");
        assert!(s.complete, "ended and drained");

        // Reading again from the end yields nothing new, which is what makes
        // the cursor worth handing back.
        let again = m.snapshot(started.id, s.next_cursor).unwrap();
        assert_eq!(again.output, "");
        assert!(!again.more);
    }

    #[test]
    fn a_killed_command_stops_running_and_says_so() {
        let m = Manager::new();
        let started = m.start(Shell::detect(), &sleeper(), &tmpdir()).unwrap();
        assert_eq!(
            m.snapshot(started.id, 0).unwrap().status,
            Status::Running,
            "a sleeper must still be running when we stop it"
        );

        let killed = m.kill(started.id).unwrap();
        assert!(!killed.already_finished);
        assert!(
            !killed.status.is_running(),
            "kill must wait for the reap, not report 'still running': {:?}",
            killed.status
        );
        assert_eq!(
            m.snapshot(started.id, 0).unwrap().status.state(),
            "killed",
            "the ending is a kill, not an exit it chose"
        );

        #[cfg(unix)]
        assert!(
            !pid_alive(started.pid.unwrap()),
            "the process group must be gone, not just unreferenced"
        );
    }

    #[test]
    fn killing_a_command_that_already_finished_is_a_no_op_not_an_error() {
        let m = Manager::new();
        let started = m
            .start(Shell::detect(), &echo("bgproc-done"), &tmpdir())
            .unwrap();
        assert!(eventually(|| !m
            .snapshot(started.id, 0)
            .unwrap()
            .status
            .is_running()));

        let killed = m.kill(started.id).unwrap();
        assert!(killed.already_finished);
        assert!(
            !killed.status.is_running(),
            "the post-condition — not running — is what the caller asked for, and it holds"
        );
    }

    #[test]
    fn dropping_the_manager_stops_what_it_started() {
        let pid = {
            let m = Manager::new();
            let started = m.start(Shell::detect(), &sleeper(), &tmpdir()).unwrap();
            let pid = started.pid.expect("a PTY child reports its pid");
            #[cfg(unix)]
            assert!(pid_alive(pid), "the sleeper should be live before the drop");
            pid
            // `m` drops here — the app shutting down.
        };

        // The waiter thread is blocked in `wait()`, so its guard cannot have
        // fired: only the manager's own teardown can have stopped this.
        #[cfg(unix)]
        assert!(
            eventually(|| !pid_alive(pid)),
            "a command the app started must not outlive the app"
        );
        #[cfg(not(unix))]
        let _ = pid;
    }

    #[test]
    fn a_full_roster_refuses_the_next_command_instead_of_queueing_it() {
        let m = Manager::new();
        let mut started = Vec::new();
        for i in 0..MAX_RUNNING {
            started.push(
                m.start(Shell::detect(), &sleeper(), &tmpdir())
                    .unwrap_or_else(|e| panic!("command {i}: {e:?}")),
            );
        }

        match m.start(Shell::detect(), &sleeper(), &tmpdir()) {
            Err(StartError::Busy { running, cap }) => {
                assert_eq!((running, cap), (MAX_RUNNING, MAX_RUNNING));
            }
            other => panic!("the {MAX_RUNNING}-command cap must refuse, got {other:?}"),
        }

        // Refusing must not have disturbed what is already running.
        for s in &started {
            assert_eq!(m.snapshot(s.id, 0).unwrap().status, Status::Running);
        }
        m.kill_all();
    }

    #[test]
    fn concurrent_starts_cannot_both_take_the_last_slot() {
        // The check and the insert are one step under the lock; if they were
        // not, this would start MAX_RUNNING + 1 commands.
        let m = Arc::new(Manager::new());
        let ok = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::new();

        for _ in 0..(MAX_RUNNING + 4) {
            let m = Arc::clone(&m);
            let ok = Arc::clone(&ok);
            handles.push(thread::spawn(move || {
                if m.start(Shell::detect(), &sleeper(), &tmpdir()).is_ok() {
                    ok.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(ok.load(Ordering::SeqCst), MAX_RUNNING as u64);
        m.kill_all();
    }

    #[test]
    fn the_footer_says_where_the_bytes_came_from() {
        let running = Snapshot {
            id: 3,
            command: "npm run dev".into(),
            pid: Some(11),
            status: Status::Running,
            output: "ready\n".into(),
            cursor: 0,
            next_cursor: 6,
            lost: false,
            more: false,
            complete: false,
            elapsed_ms: 1500,
        };
        let text = render_snapshot(&running);
        assert!(text.contains("ready"), "{text}");
        assert!(text.contains("command #3"), "{text}");
        assert!(text.contains("running for 1.5s"), "{text}");
        assert!(text.contains("next cursor 6"), "{text}");

        // A reader that fell behind is told, because a stream with an
        // invisible hole in it reads as a complete stream.
        let behind = Snapshot {
            lost: true,
            more: true,
            ..running.clone()
        };
        let text = render_snapshot(&behind);
        assert!(text.contains("dropped"), "{text}");
        assert!(text.contains("more output is waiting"), "{text}");

        // Nothing at this cursor is a claim about the cursor, not about the
        // command — one that has printed plenty can still have nothing new.
        let quiet = Snapshot {
            output: String::new(),
            cursor: 4096,
            next_cursor: 4096,
            ..running
        };
        let text = render_snapshot(&quiet);
        assert!(text.contains("nothing written since cursor 4096"), "{text}");
    }

    #[test]
    fn every_ending_renders_and_every_state_has_its_own_code_shape() {
        // Exhaustive over the small closed domain: the four statuses, with
        // and without a code where one is optional.
        let statuses = [
            Status::Running,
            Status::Exited { code: Some(0) },
            Status::Exited { code: None },
            Status::Killed { code: Some(143) },
            Status::Killed { code: None },
            Status::Failed {
                message: "no pty".into(),
            },
        ];
        for status in statuses {
            let s = Snapshot {
                id: 1,
                command: "c".into(),
                pid: None,
                status: status.clone(),
                output: String::new(),
                cursor: 0,
                next_cursor: 0,
                lost: false,
                more: false,
                complete: !status.is_running(),
                elapsed_ms: 100,
            };
            let text = render_snapshot(&s);
            assert!(!text.is_empty());
            assert!(!text.contains("NaN"), "{text}");
            if status.is_running() {
                assert_eq!(status.exit_code(), None, "a running command has no code");
            }
            if matches!(status, Status::Failed { .. }) {
                assert_eq!(
                    status.exit_code(),
                    None,
                    "an invented exit code is worse than none"
                );
            }
        }
    }
}
