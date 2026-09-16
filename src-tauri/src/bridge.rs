//! The web-AI coding-agent bridge (M2).
//!
//! Tool calls arrive from the native MCP server (`mcp.rs`), from the desktop
//! tool sandbox, or from the frontend's handoff flow. Every call is
//! policy-checked: reads are auto-approved (except sensitive paths), writes
//! and command execution always require an explicit user approval. All calls
//! are audited to SQLite.

use crate::{bgproc, db, git, lsp, media, notebook, pty, shell::Shell, web, AppState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One replacement inside a `multi_edit` batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Edit {
    pub old_string: String,
    pub new_string: String,
    #[serde(default)]
    pub replace_all: Option<bool>,
}

/// A web-AI tool call (serde: externally-tagged, mirrors `types.ts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Tool {
    ReadFile {
        path: String,
        /// 1-based first line to return. Absent → 1. Reads are chunked, so a
        /// large file is paged through rather than truncated or refused.
        #[serde(default)]
        offset: Option<u32>,
        /// Maximum lines to return. Absent → `CHUNK_LINES`. Always further
        /// bounded by `CHUNK_BYTES`.
        #[serde(default)]
        limit: Option<u32>,
    },
    WriteFile {
        path: String,
        content: String,
    },
    /// Replace one exact string in a file. The scalpel to `write_file`'s
    /// mallet: a one-line fix no longer resends the whole file.
    EditFile {
        path: String,
        old_string: String,
        new_string: String,
        #[serde(default)]
        replace_all: Option<bool>,
    },
    /// Several edits in one call, applied together — all of them or none.
    MultiEdit {
        path: String,
        edits: Vec<Edit>,
    },
    /// Apply a single-file unified diff (hunks with context lines).
    ApplyPatch {
        path: String,
        patch: String,
    },
    DeleteFile {
        path: String,
    },
    MoveFile {
        from: String,
        to: String,
    },
    CopyFile {
        from: String,
        to: String,
    },
    CreateDirectory {
        path: String,
    },
    /// Read several files in one round-trip. Each call costs a full trip
    /// through the connector and the approval queue, so batching several
    /// files into one call is a win.
    ReadManyFiles {
        paths: Vec<String>,
    },
    /// Regex content search over the workspace.
    ///
    /// The first tool whose cost is proportional to the tree rather than to
    /// its arguments, so it is capped by *stopping* the walk rather than by
    /// truncating what the walk produced.
    Grep {
        pattern: String,
        /// File or directory to search. Absent → the workspace root.
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        include: Option<String>,
        #[serde(default)]
        exclude: Option<String>,
        /// Absent → [`GrepMode::Content`].
        #[serde(default)]
        mode: Option<GrepMode>,
        #[serde(default)]
        max_results: Option<u32>,
    },
    /// Find files whose *path* matches a glob.
    Glob {
        pattern: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        max_results: Option<u32>,
    },
    RunCommand {
        command: String,
    },
    /// Start a command and return without waiting for it.
    ///
    /// `run_command` holds the call open until the command exits, so anything
    /// meant to keep running — a dev server, a watcher, a tail — cannot be
    /// started with it without consuming its whole timeout. This is the half
    /// that hands the caller a handle instead.
    RunCommandBackground {
        command: String,
    },
    /// Read what a background command has written since a cursor.
    CommandOutput {
        /// The handle `run_command_background` returned.
        id: u64,
        /// Absolute byte offset to read from. Absent → 0, which is the start
        /// of what is still kept.
        #[serde(default)]
        cursor: Option<u64>,
    },
    /// Stop a background command and everything it spawned.
    KillCommand {
        id: u64,
    },
    ListDirectory {
        path: String,
    },
    GitStatus,
    /// Working-tree diff against HEAD (staged + unstaged, untracked included).
    GitDiff {
        /// Restrict to one path. Absent → every changed file.
        #[serde(default)]
        path: Option<String>,
    },
    GitLog {
        #[serde(default)]
        limit: Option<u32>,
    },
    /// Stage a path, or everything when `path` is absent.
    GitAdd {
        #[serde(default)]
        path: Option<String>,
    },
    GitUnstage {
        path: String,
    },
    GitCommit {
        message: String,
    },
    GitBranches,
    /// Switch branch. Refuses on a dirty tree — never forces.
    GitCheckout {
        name: String,
    },
    GitCreateBranch {
        name: String,
        /// Revision to branch from. Absent → HEAD.
        #[serde(default)]
        base: Option<String>,
        #[serde(default)]
        checkout: Option<bool>,
    },
    /// One commit in full: message, author, and patch.
    GitShow {
        oid: String,
    },
    /// The patch alone for one commit.
    GitCommitDiff {
        oid: String,
    },
    /// Replace the session's task list.
    TodoWrite {
        todos: Vec<TodoItem>,
    },
    /// Read the session's task list back.
    TodoRead,
    /// Set the session's objective, retiring the previous one.
    SetObjective {
        text: String,
    },
    /// Record a decision, and optionally the reason for it.
    RememberDecision {
        summary: String,
        reason: Option<String>,
    },
    /// Record a constraint the work has to respect.
    RememberConstraint {
        text: String,
    },
    /// Record an attempt, so a failed one is not repeated.
    RememberAttempt {
        description: String,
        succeeded: Option<bool>,
    },
    /// Read back what a session has recorded.
    GetFacts {
        /// Absent → the connector's own session.
        session_id: Option<i64>,
    },
    /// List archived sessions, newest first.
    ListSessions {
        limit: Option<u32>,
    },
    /// Note that the caller wants to hand back to the developer.
    RequestHandoff {
        reason: String,
        next_step: Option<String>,
    },
    /// Pull the handoff card, built from the desktop's live state.
    GetHandoff,

    // --- Phase 7: web & the long tail -------------------------------------
    /// Fetch a URL and return it as readable text. SSRF-guarded: the URL is
    /// planned, every address it resolves to is checked, and each redirect
    /// hop is re-checked and pinned to the address that was cleared.
    WebFetch {
        url: String,
        /// Read cap in bytes. Absent → the tool's default; clamped to its max.
        #[serde(default)]
        max_bytes: Option<u64>,
    },
    /// Search the web. The only tool whose result is not local-first, so it
    /// is the only one that needs to leave the machine besides `web_fetch`.
    WebSearch {
        query: String,
        #[serde(default)]
        max_results: Option<u32>,
    },
    /// Read a Jupyter notebook as structured cells rather than raw JSON.
    NotebookRead {
        path: String,
    },
    /// Replace one cell's source in a notebook, by `cell_id`.
    NotebookEdit {
        path: String,
        cell_id: String,
        new_source: String,
        /// Optional cell type (`code` | `markdown`); absent → unchanged.
        #[serde(default)]
        cell_type: Option<String>,
    },
    /// Hand a bounded sub-task to a nested agent turn and collect its result.
    DelegateTask {
        task: String,
        #[serde(default)]
        context: Option<String>,
    },

    // --- Phase 8: code intelligence (LSP) ---------------------------------
    /// What is broken in a file (or the workspace) right now.
    LspDiagnostics {
        #[serde(default)]
        path: Option<String>,
        /// `error` | `warning` | `information` | `hint`; absent → errors and warnings.
        #[serde(default)]
        severity: Option<String>,
    },
    /// Where a symbol at a position is defined.
    LspDefinition {
        path: String,
        /// 1-based line, matching `read_file`.
        line: u32,
        /// 1-based column.
        character: u32,
    },
    /// What else references the symbol at a position — the rename-safety question.
    LspReferences {
        path: String,
        line: u32,
        character: u32,
        #[serde(default)]
        include_declaration: Option<bool>,
    },
    /// The symbols in a file, or matching a query across the workspace.
    LspSymbols {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        query: Option<String>,
    },

    // --- Phase 9: the agent loop ------------------------------------------
    /// Ask the developer a structured question and block until they answer.
    AskUser {
        question: String,
        /// 2–4 labelled choices. Empty → free text only.
        #[serde(default)]
        options: Vec<String>,
    },
    /// Submit a plan and wait for approval before acting on it.
    ProposePlan {
        plan: String,
        #[serde(default)]
        steps: Vec<String>,
    },
    /// Wait for a path to change or a background command to match a pattern.
    Monitor {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        command_id: Option<u64>,
        #[serde(default)]
        pattern: Option<String>,
        #[serde(default)]
        timeout_ms: Option<u32>,
    },
    /// Raise a desktop notification.
    Notify {
        title: String,
        body: String,
        /// `info` | `success` | `warning` | `error`; absent → `info`.
        #[serde(default)]
        level: Option<String>,
    },

    // --- Phase 10: isolation & delivery -----------------------------------
    /// Create a throwaway git worktree and point this session at it.
    EnterWorktree {
        /// Directory name; absent → a generated one.
        #[serde(default)]
        name: Option<String>,
    },
    /// Leave the current worktree, keeping or discarding its changes.
    ExitWorktree {
        /// `keep` | `discard`; absent → `keep` (refuses if dirty).
        #[serde(default)]
        action: Option<String>,
    },
    /// Return an image (or PDF) as a real media block rather than text.
    ReadMedia {
        path: String,
    },
    /// Hand a file to the user as a first-class artifact.
    PublishArtifact {
        path: String,
        #[serde(default)]
        title: Option<String>,
    },
    /// A structured review result: file, line, severity, claim, evidence.
    ReportFindings {
        findings: Vec<Finding>,
        #[serde(default)]
        summary: Option<String>,
    },

    /// Meta: the full argument schema for one tool. Needs no project root.
    DescribeTool {
        name: String,
    },
    /// Meta: every available tool, grouped. Needs no project root.
    ListTools,
}

/// One entry in a session's task list, as it crosses the wire.
///
/// Only `content` is required. A model that sends a bare list of strings means
/// the same thing as one that sends objects with no status, and both mean
/// "pending" — so the lenient form is the parser's job, and this type records
/// what actually arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    /// `pending` | `in_progress` | `completed`; absent → `pending`.
    #[serde(default)]
    pub status: Option<String>,
    /// Present-continuous form for the UI ("Running the tests").
    #[serde(default)]
    pub active_form: Option<String>,
}

/// One entry in a `report_findings` result.
///
/// A review is only actionable when each claim names *where* it is and *how
/// bad* it is, so both are required; `evidence` is the one line that lets the
/// reader check the claim without re-reading the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Finding {
    pub path: String,
    /// 1-based line the finding is about.
    pub line: u32,
    /// `error` | `warning` | `info`.
    pub severity: String,
    pub claim: String,
    #[serde(default)]
    pub evidence: Option<String>,
}

/// A card `ask_user` or `propose_plan` puts on the desktop, and the answer.
///
/// The same event-plus-blocking-wait shape as an approval, deliberately: the
/// infrastructure already exists, and a second concurrency model for "block
/// on a human" would be one more thing to get wrong. `kind` tells the UI
/// which card to draw — a question with options, or a plan to approve.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QuestionCard {
    pub id: u64,
    /// `question` | `plan`.
    pub kind: String,
    pub title: String,
    pub body: String,
    pub options: Vec<String>,
    pub source: String,
}

/// What the desktop sends back for a question card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionAnswer {
    /// The chosen label, or `None` when the answer was free text.
    pub option: Option<String>,
    pub answer: String,
}

/// When a tool call requires an explicit user decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Approval {
    /// Executes immediately.
    Auto,
    /// Executes immediately unless a target path is sensitive.
    SensitivePathOnly,
    /// Always asks.
    Always,
    /// Always asks, and may destroy work — the approval UI must show
    /// exactly what is affected.
    Destructive,
}

/// Static, per-tool metadata. This is the single source of truth: the
/// approval policy, trace kind, timeout, auto-insert behaviour and the
/// AI-facing manifest are all derived from here rather than repeated in
/// separate `match` arms across `bridge.rs`, `lib.rs` and `mcp.rs`.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    /// Alternate names accepted on the wire. A web AI emits whatever it
    /// remembers — `Grep`, `search_files`, `str_replace`, `bash` — so
    /// resolving aliases directly raises the tool-call hit rate.
    pub aliases: &'static [&'static str],
    /// Argument list for the manifest, e.g. `"path, offset?, limit?"`.
    pub args: &'static str,
    /// One terse line, shown in the manifest.
    pub summary: &'static str,
    pub approval: Approval,
    /// Activity-trace kind, or `None` for calls not worth tracing.
    pub trace_kind: Option<&'static str>,
    pub timeout_ms: u32,
    /// Read-only result: true for the read-only/meta tools, false for every
    /// tool that writes or runs a command. SPECS metadata only — no transport
    /// consumes it since the extension relay was removed.
    pub auto_insert: bool,
    /// Manifest grouping; must appear in [`GROUPS`].
    pub group: &'static str,
}

/// Manifest group order.
pub const GROUPS: &[&str] = &[
    "Reading",
    "Editing",
    "Commands",
    "Search",
    "Git",
    "Memory",
    "Planning",
    "Code",
    "Web",
    "Isolation",
    "Meta",
];

/// Caller/transport labels. Recorded in audits and grants, and compared so a
/// session grant stays scoped to the path that earned it (a desktop grant
/// never auto-approves an `mcp` call, and vice versa).
pub const SOURCE_DESKTOP: &str = "desktop"; // in-app sandbox / UI
pub const SOURCE_MCP: &str = "mcp"; // the connector's remote caller (mcp.rs)

/// Every tool the bridge can execute.
pub const SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "read_file",
        aliases: &["read", "view_file", "cat", "open_file"],
        args: "path, offset?",
        summary: "Read a file as numbered lines, in chunks for large files",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("reading"),
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Reading",
    },
    ToolSpec {
        name: "list_directory",
        aliases: &["ls", "list_dir", "list", "dir"],
        args: "path",
        summary: "List the entries of a directory",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Reading",
    },
    ToolSpec {
        name: "write_file",
        aliases: &["write", "create_file", "put_file"],
        args: "path, content",
        summary: "Overwrite a file with new content",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "edit_file",
        aliases: &["edit", "str_replace", "replace", "apply_edit"],
        args: "path, old_string, new_string, replace_all?",
        summary: "Replace one exact string in a file",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "multi_edit",
        aliases: &["multi_edit_file", "batch_edit", "edit_many"],
        args: "path, edits[]",
        summary: "Apply several exact-string edits to one file, atomically",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("editing"),
        timeout_ms: 20_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "apply_patch",
        aliases: &["patch", "unified_diff"],
        args: "path, patch",
        summary: "Apply a single-file unified diff",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 20_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "delete_file",
        aliases: &["remove_file", "rm_file", "remove"],
        args: "path",
        summary: "Delete a file (not directories)",
        approval: Approval::Destructive,
        trace_kind: Some("editing"),
        timeout_ms: 10_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "move_file",
        aliases: &["rename_file", "rename", "mv"],
        args: "from, to",
        summary: "Move or rename a file, overwriting the target",
        approval: Approval::Destructive,
        trace_kind: Some("editing"),
        timeout_ms: 10_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "copy_file",
        aliases: &["cp_file", "duplicate_file", "cp"],
        args: "from, to",
        summary: "Copy a file, overwriting the target",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 10_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "create_directory",
        aliases: &["mkdir", "create_dir", "make_directory"],
        args: "path",
        summary: "Create a directory and its parents",
        approval: Approval::Auto,
        trace_kind: Some("editing"),
        timeout_ms: 10_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "read_many_files",
        aliases: &["read_files", "read_many"],
        args: "paths[]",
        summary: "Read several files in one call, first chunk of each",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 15_000,
        auto_insert: true,
        group: "Reading",
    },
    ToolSpec {
        name: "grep",
        aliases: &[
            "search",
            "search_files",
            "rg",
            "ripgrep",
            "search_content",
            "find_in_files",
            "grep_search",
        ],
        args: "pattern, path?, include?, exclude?, mode?, max_results?",
        summary: "Search file contents by regex, with glob filters",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("reading"),
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Search",
    },
    ToolSpec {
        name: "glob",
        aliases: &[
            "find_files",
            "file_pattern",
            "find_by_name",
            "list_files",
            "search_files_by_name",
        ],
        args: "pattern, path?, max_results?",
        summary: "Find files by path pattern (* and ** supported)",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("reading"),
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Search",
    },
    ToolSpec {
        name: "run_command",
        aliases: &["bash", "shell", "execute", "terminal", "sh"],
        args: "command",
        summary: "Run a shell command in the project root",
        approval: Approval::Always,
        trace_kind: Some("running"),
        timeout_ms: 120_000,
        auto_insert: false,
        group: "Commands",
    },
    ToolSpec {
        name: "run_command_background",
        aliases: &["bash_background", "start_command", "run_async", "spawn"],
        args: "command",
        summary: "Start a command without waiting for it, and return a handle",
        approval: Approval::Always,
        trace_kind: Some("running"),
        // Short: starting is the fast half. The caller waits for the command
        // with `command_output`, which has its own timeout.
        timeout_ms: 20_000,
        auto_insert: false,
        group: "Commands",
    },
    ToolSpec {
        name: "command_output",
        aliases: &["read_output", "command_log", "tail_command", "get_output"],
        args: "id, cursor?",
        summary: "Read what a background command has written since a cursor",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 10_000,
        auto_insert: false,
        group: "Commands",
    },
    ToolSpec {
        name: "kill_command",
        aliases: &["stop_command", "kill_background", "command_kill"],
        args: "id",
        summary: "Stop a background command and everything it spawned",
        // Auto, not Always: the user already approved starting this command,
        // and stopping it is strictly less powerful than that. Making the
        // stop wait for a card would keep a runaway process alive until
        // somebody noticed.
        approval: Approval::Auto,
        trace_kind: Some("running"),
        timeout_ms: 20_000,
        auto_insert: false,
        group: "Commands",
    },
    ToolSpec {
        name: "git_status",
        aliases: &["status", "git_st"],
        args: "",
        summary: "Show changed files in the git working tree",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Git",
    },
    ToolSpec {
        name: "git_diff",
        aliases: &["diff", "show_changes", "git_d", "uncommitted"],
        args: "path?",
        summary: "Show the working-tree diff against HEAD",
        approval: Approval::SensitivePathOnly,
        trace_kind: None,
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Git",
    },
    ToolSpec {
        name: "git_log",
        aliases: &["log", "history", "git_history", "commits"],
        args: "limit?",
        summary: "List recent commits, newest first",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Git",
    },
    ToolSpec {
        name: "git_add",
        aliases: &["stage", "git_stage", "stage_file"],
        args: "path?",
        summary: "Stage a path, or everything when path is omitted",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Git",
    },
    ToolSpec {
        name: "git_unstage",
        aliases: &["unstage_file", "git_reset", "unstage_path"],
        args: "path",
        summary: "Unstage a path, leaving the working tree untouched",
        approval: Approval::SensitivePathOnly,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Git",
    },
    ToolSpec {
        name: "git_commit",
        aliases: &["commit", "git_ci", "create_commit"],
        args: "message",
        summary: "Commit the staged changes",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 30_000,
        auto_insert: false,
        group: "Git",
    },
    ToolSpec {
        name: "git_branches",
        aliases: &["branches", "list_branches", "git_branch_list"],
        args: "",
        summary: "List branches, marking the current one",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Git",
    },
    ToolSpec {
        name: "git_checkout",
        aliases: &["checkout", "switch_branch", "git_switch"],
        args: "name",
        summary: "Switch branch; refuses when the tree has uncommitted changes",
        approval: Approval::Destructive,
        trace_kind: Some("editing"),
        timeout_ms: 30_000,
        auto_insert: false,
        group: "Git",
    },
    ToolSpec {
        name: "git_create_branch",
        aliases: &["branch", "new_branch", "create_branch"],
        args: "name, base?, checkout?",
        summary: "Create a branch, optionally from a revision and checked out",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Git",
    },
    ToolSpec {
        name: "git_show",
        aliases: &["show_commit", "git_show_commit", "inspect_commit"],
        args: "oid",
        summary: "Show one commit's message, author and patch",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Git",
    },
    ToolSpec {
        name: "git_commit_diff",
        aliases: &["commit_diff", "git_diff_commit", "patch_for_commit"],
        args: "oid",
        summary: "Show the patch for one commit",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Git",
    },
    ToolSpec {
        name: "todo_write",
        aliases: &["todos", "write_todos", "set_todos", "plan_tasks"],
        args: "todos",
        summary: "Replace the session's task list",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "todo_read",
        aliases: &["read_todos", "get_todos", "list_todos"],
        args: "",
        summary: "Read the session's task list",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "set_objective",
        aliases: &["objective", "set_goal", "declare_objective"],
        args: "text",
        summary: "Set the objective for this work, retiring the previous one",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "remember_decision",
        aliases: &["record_decision", "log_decision", "decide"],
        args: "summary, reason?",
        summary: "Record a decision, and optionally why it was made",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "remember_constraint",
        aliases: &["record_constraint", "add_constraint"],
        args: "text",
        summary: "Record a constraint the work must respect",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "remember_attempt",
        aliases: &["record_attempt", "log_attempt", "tried"],
        args: "description, succeeded?",
        summary: "Record an attempt, so a failed one is not repeated",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "get_facts",
        aliases: &["facts", "read_facts", "project_memory", "what_do_you_know"],
        args: "session_id?",
        summary: "Read the objective, decisions, constraints and failed attempts",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "list_sessions",
        aliases: &["sessions", "session_history", "recent_sessions"],
        args: "limit?",
        summary: "List archived coding sessions, newest first",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "request_handoff",
        aliases: &["hand_back", "request_takeover", "ask_for_handoff"],
        args: "reason, next_step?",
        summary: "Ask the developer to take over, recording why",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "get_handoff",
        aliases: &["handoff", "where_were_we", "catch_up", "resume_context"],
        args: "",
        summary: "Pull the handoff card: objective, progress, files and next step",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Memory",
    },
    ToolSpec {
        name: "describe_tool",
        aliases: &["tool_help", "help", "tool_info"],
        args: "name",
        summary: "Show the full argument schema for one tool",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Meta",
    },
    ToolSpec {
        name: "list_tools",
        aliases: &["tools", "available_tools"],
        args: "",
        summary: "List every available tool, grouped",
        approval: Approval::Auto,
        trace_kind: None,
        timeout_ms: 5_000,
        auto_insert: true,
        group: "Meta",
    },
    // --- Phase 7: web & long tail -----------------------------------------
    ToolSpec {
        name: "web_fetch",
        aliases: &["fetch", "fetch_url", "http_get", "read_url"],
        args: "url, max_bytes?",
        summary: "Fetch a URL and return its readable text (SSRF-guarded)",
        approval: Approval::Auto,
        trace_kind: Some("web"),
        timeout_ms: 30_000,
        auto_insert: true,
        group: "Web",
    },
    ToolSpec {
        name: "web_search",
        aliases: &["search_web", "google", "search_internet"],
        args: "query, max_results?",
        summary: "Search the web and return titled results",
        approval: Approval::Auto,
        trace_kind: Some("web"),
        timeout_ms: 30_000,
        auto_insert: true,
        group: "Web",
    },
    ToolSpec {
        name: "notebook_read",
        aliases: &["read_notebook", "nb_read", "ipynb_read"],
        args: "path",
        summary: "Read a Jupyter notebook (.ipynb) as structured cells",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Reading",
    },
    ToolSpec {
        name: "notebook_edit",
        aliases: &["edit_notebook", "nb_edit", "ipynb_edit"],
        args: "path, cell_id, new_source, cell_type?",
        summary: "Replace one notebook cell's source, by cell_id",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Editing",
    },
    ToolSpec {
        name: "delegate_task",
        aliases: &["spawn_task", "subtask", "delegate"],
        args: "task, context?",
        summary: "Hand a bounded sub-task to a nested agent turn",
        approval: Approval::Always,
        trace_kind: Some("agent"),
        timeout_ms: 30_000,
        auto_insert: false,
        group: "Planning",
    },
    // --- Phase 8: code intelligence (LSP) ---------------------------------
    ToolSpec {
        name: "lsp_diagnostics",
        aliases: &["diagnostics", "errors", "problems", "lint"],
        args: "path?, severity?",
        summary: "Errors and warnings a language server reports right now",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Code",
    },
    ToolSpec {
        name: "lsp_definition",
        aliases: &["goto_definition", "definition", "go_to_def"],
        args: "path, line, character",
        summary: "Where the symbol at a position is defined",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Code",
    },
    ToolSpec {
        name: "lsp_references",
        aliases: &["references", "find_references", "callers"],
        args: "path, line, character, include_declaration?",
        summary: "Every place the symbol at a position is referenced",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Code",
    },
    ToolSpec {
        name: "lsp_symbols",
        aliases: &["symbols", "document_symbols", "workspace_symbols", "outline"],
        args: "path?, query?",
        summary: "Symbols in a file, or matching a query across the workspace",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 20_000,
        auto_insert: true,
        group: "Code",
    },
    // --- Phase 9: the agent loop ------------------------------------------
    ToolSpec {
        name: "ask_user",
        aliases: &["ask", "question", "clarify"],
        args: "question, options[]",
        summary: "Ask the developer a question and wait for the answer",
        approval: Approval::Auto,
        trace_kind: Some("planning"),
        timeout_ms: 300_000,
        auto_insert: true,
        group: "Planning",
    },
    ToolSpec {
        name: "propose_plan",
        aliases: &["plan", "submit_plan", "propose"],
        args: "plan, steps[]",
        summary: "Submit a plan and wait for approval before acting",
        approval: Approval::Always,
        trace_kind: Some("planning"),
        timeout_ms: 300_000,
        auto_insert: true,
        group: "Planning",
    },
    ToolSpec {
        name: "monitor",
        aliases: &["watch", "wait_for", "tail"],
        args: "path?, command_id?, pattern?, timeout_ms?",
        summary: "Wait for a path to change or a command to match a pattern",
        approval: Approval::Auto,
        trace_kind: Some("planning"),
        timeout_ms: 300_000,
        auto_insert: true,
        group: "Planning",
    },
    ToolSpec {
        name: "notify",
        aliases: &["notification", "alert", "ping"],
        args: "title, body, level?",
        summary: "Raise a desktop notification",
        approval: Approval::Auto,
        trace_kind: Some("planning"),
        timeout_ms: 5_000,
        auto_insert: false,
        group: "Planning",
    },
    // --- Phase 10: isolation & delivery -----------------------------------
    ToolSpec {
        name: "enter_worktree",
        aliases: &["create_worktree", "worktree_enter", "new_worktree"],
        args: "name?",
        summary: "Create a throwaway git worktree and work inside it",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 30_000,
        auto_insert: false,
        group: "Isolation",
    },
    ToolSpec {
        name: "exit_worktree",
        aliases: &["leave_worktree", "worktree_exit", "remove_worktree"],
        args: "action?",
        summary: "Leave the current worktree, keeping or discarding changes",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 30_000,
        auto_insert: false,
        group: "Isolation",
    },
    ToolSpec {
        name: "read_media",
        aliases: &["read_image", "view_image", "read_binary"],
        args: "path",
        summary: "Return an image or PDF as a real media block, not text",
        approval: Approval::Auto,
        trace_kind: Some("reading"),
        timeout_ms: 15_000,
        auto_insert: true,
        group: "Reading",
    },
    ToolSpec {
        name: "publish_artifact",
        aliases: &["artifact", "send_file", "deliver_file"],
        args: "path, title?",
        summary: "Hand a file to the user as a first-class artifact",
        approval: Approval::Always,
        trace_kind: Some("editing"),
        timeout_ms: 15_000,
        auto_insert: false,
        group: "Isolation",
    },
    ToolSpec {
        name: "report_findings",
        aliases: &["findings", "review", "report"],
        args: "findings[], summary?",
        summary: "Report a structured code review as actionable findings",
        approval: Approval::Auto,
        trace_kind: Some("planning"),
        timeout_ms: 10_000,
        auto_insert: true,
        group: "Planning",
    },
];

/// Canonical name of a tool variant.
pub fn tool_name(tool: &Tool) -> &'static str {
    match tool {
        Tool::ReadFile { .. } => "read_file",
        Tool::WriteFile { .. } => "write_file",
        Tool::EditFile { .. } => "edit_file",
        Tool::MultiEdit { .. } => "multi_edit",
        Tool::ApplyPatch { .. } => "apply_patch",
        Tool::DeleteFile { .. } => "delete_file",
        Tool::MoveFile { .. } => "move_file",
        Tool::CopyFile { .. } => "copy_file",
        Tool::CreateDirectory { .. } => "create_directory",
        Tool::ReadManyFiles { .. } => "read_many_files",
        Tool::RunCommand { .. } => "run_command",
        Tool::RunCommandBackground { .. } => "run_command_background",
        Tool::CommandOutput { .. } => "command_output",
        Tool::KillCommand { .. } => "kill_command",
        Tool::ListDirectory { .. } => "list_directory",
        Tool::Grep { .. } => "grep",
        Tool::Glob { .. } => "glob",
        Tool::GitStatus => "git_status",
        Tool::GitDiff { .. } => "git_diff",
        Tool::GitLog { .. } => "git_log",
        Tool::GitAdd { .. } => "git_add",
        Tool::GitUnstage { .. } => "git_unstage",
        Tool::GitCommit { .. } => "git_commit",
        Tool::GitBranches => "git_branches",
        Tool::GitCheckout { .. } => "git_checkout",
        Tool::GitCreateBranch { .. } => "git_create_branch",
        Tool::GitShow { .. } => "git_show",
        Tool::GitCommitDiff { .. } => "git_commit_diff",
        Tool::TodoWrite { .. } => "todo_write",
        Tool::TodoRead => "todo_read",
        Tool::SetObjective { .. } => "set_objective",
        Tool::RememberDecision { .. } => "remember_decision",
        Tool::RememberConstraint { .. } => "remember_constraint",
        Tool::RememberAttempt { .. } => "remember_attempt",
        Tool::GetFacts { .. } => "get_facts",
        Tool::ListSessions { .. } => "list_sessions",
        Tool::RequestHandoff { .. } => "request_handoff",
        Tool::GetHandoff => "get_handoff",
        Tool::WebFetch { .. } => "web_fetch",
        Tool::WebSearch { .. } => "web_search",
        Tool::NotebookRead { .. } => "notebook_read",
        Tool::NotebookEdit { .. } => "notebook_edit",
        Tool::DelegateTask { .. } => "delegate_task",
        Tool::LspDiagnostics { .. } => "lsp_diagnostics",
        Tool::LspDefinition { .. } => "lsp_definition",
        Tool::LspReferences { .. } => "lsp_references",
        Tool::LspSymbols { .. } => "lsp_symbols",
        Tool::AskUser { .. } => "ask_user",
        Tool::ProposePlan { .. } => "propose_plan",
        Tool::Monitor { .. } => "monitor",
        Tool::Notify { .. } => "notify",
        Tool::EnterWorktree { .. } => "enter_worktree",
        Tool::ExitWorktree { .. } => "exit_worktree",
        Tool::ReadMedia { .. } => "read_media",
        Tool::PublishArtifact { .. } => "publish_artifact",
        Tool::ReportFindings { .. } => "report_findings",
        Tool::DescribeTool { .. } => "describe_tool",
        Tool::ListTools => "list_tools",
    }
}

/// The spec for a tool call. Every [`Tool`] variant has a [`SPECS`] row —
/// `every_variant_has_a_spec` enforces it.
pub fn spec(tool: &Tool) -> &'static ToolSpec {
    let name = tool_name(tool);
    SPECS
        .iter()
        .find(|s| s.name == name)
        .expect("every Tool variant needs a SPECS row")
}

/// Normalize a wire tool name: strip any namespace prefix
/// (`default_api.read_file`), lowercase, and treat `-`/space as `_`.
fn normalize_tool_name(name: &str) -> String {
    name.rsplit('.')
        .next()
        .unwrap_or(name)
        .trim()
        .chars()
        .map(|c| match c {
            '-' | ' ' => '_',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// Look up a spec by canonical name or alias.
pub fn spec_by_name(name: &str) -> Option<&'static ToolSpec> {
    let n = normalize_tool_name(name);
    SPECS
        .iter()
        .find(|s| s.name == n || s.aliases.contains(&n.as_str()))
}

/// The compact, grouped tool manifest handed to a web AI. Deliberately
/// terse: full per-tool docs for every tool would crowd out the actual
/// project context, so the AI calls `describe_tool` for detail on demand.
///
/// Rendered as an aligned table, **never** as `name(args)`. This is the body
/// of `list_tools`, which is handed straight to the model, and text in call
/// syntax reads as a burst of tool calls when the model echoes it back. Keep
/// it inert.
pub fn tool_manifest() -> String {
    let mut out = String::new();
    for group in GROUPS {
        let rows: Vec<_> = SPECS.iter().filter(|s| &s.group == group).collect();
        if rows.is_empty() {
            continue;
        }
        out.push_str(group);
        out.push('\n');
        for s in rows {
            let args = if s.args.is_empty() { "—" } else { s.args };
            out.push_str(&format!("  {:<16} {:<17} {}\n", s.name, args, s.summary));
        }
    }
    out
}

/// Plain-language approval behavior. Shared by `describe_tool`'s output and
/// the MCP tool description so the two can never drift apart.
fn approval_phrase(approval: Approval) -> &'static str {
    match approval {
        Approval::Auto => "runs immediately",
        Approval::SensitivePathOnly => "runs immediately unless the path is sensitive",
        Approval::Always => "requires the user's approval",
        Approval::Destructive => "requires the user's approval (may destroy work)",
    }
}

/// The description the MCP surface advertises for a tool in `tools/list`.
///
/// A connector's model reads this instead of the manifest, so it carries the
/// argument list: the input schema's property names say nothing about order or
/// which arguments are optional, and a model that has to guess tends to invent
/// them. Kept to three lines because `tools/list` is paid for on every session.
pub fn tool_description(name: &str) -> Option<String> {
    let spec = spec_by_name(name)?;
    let mut out = spec.summary.to_string();
    if !spec.args.is_empty() {
        out.push_str(&format!("\nArgs: {}", spec.args));
    }
    out.push_str(&format!("\nApproval: {}", approval_phrase(spec.approval)));
    Some(out)
}

/// Full detail for one tool, for `describe_tool`. Also handed straight to the
/// model, so it avoids call syntax for the same reason as `tool_manifest`.
fn describe_spec(s: &ToolSpec) -> String {
    let approval = approval_phrase(s.approval);
    let mut out = format!("{}\n  {}\n", s.name, s.summary);
    if s.args.is_empty() {
        out.push_str("  Arguments: none\n");
    } else {
        out.push_str(&format!("  Arguments: {}\n", s.args));
    }
    out.push_str(&format!("  Approval: {approval}\n"));
    out.push_str(&format!("  Timeout: {}ms\n", s.timeout_ms));
    if !s.aliases.is_empty() {
        out.push_str(&format!("  Also accepted as: {}\n", s.aliases.join(", ")));
    }
    out
}

/// Parse a wire tool call into a [`Tool`]. The name is resolved through the
/// spec table's aliases first, because a web AI (or MCP connector) emits
/// whatever name it happens to remember (`Read`, `bash`,
/// `default_api.read_file`). Coercions are permissive on purpose: line
/// offsets arrive as JSON numbers from our own parser but as quoted strings
/// from a model writing raw JSON, and a single path often stands in where a
/// `paths` array belongs.
///
/// This is the shared entry point for every caller — the native MCP server
/// (`mcp.rs`), and the desktop/in-app path (`bridge_tool`) — so all paths
/// parse a given tool call identically.
pub fn parse_tool_call(tool_name: &str, args: &serde_json::Value) -> Result<Tool, String> {
    let spec = spec_by_name(tool_name).ok_or_else(|| format!("unknown tool: {tool_name}"))?;
    let str_arg = |key: &str| -> Result<String, String> {
        args[key]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("missing '{key}' argument"))
    };
    // First present key wins — a model guesses argument names the way it
    // guesses tool names (`from`/`src`, `old_string`/`old_str`/`find`).
    let str_arg_any = |keys: &[&str]| -> Result<String, String> {
        for k in keys {
            if let Some(s) = args[*k].as_str() {
                return Ok(s.to_string());
            }
        }
        Err(format!("missing '{}' argument", keys[0]))
    };
    // Line numbers arrive as a JSON number from our own parser, but a web AI
    // writing raw JSON often quotes them.
    let u32_arg = |key: &str| -> Option<u32> {
        args[key]
            .as_u64()
            .or_else(|| args[key].as_str().and_then(|s| s.trim().parse().ok()))
            .map(|n| n.min(u32::MAX as u64) as u32)
    };
    // A handle or a cursor: same quoting tolerance as the offsets above, but
    // the full 64-bit range — a byte offset into a long-running command's
    // output has no reason to stop at 4 GiB, and truncating one would silently
    // read from the wrong place.
    let u64_arg = |key: &str| -> Option<u64> {
        args[key]
            .as_u64()
            .or_else(|| args[key].as_str().and_then(|s| s.trim().parse().ok()))
    };
    // Bools get the same quoting treatment as offsets.
    let bool_arg = |key: &str| -> Option<bool> {
        args[key].as_bool().or_else(|| {
            args[key]
                .as_str()
                .and_then(|s| match s.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" | "yes" => Some(true),
                    "false" | "0" | "no" => Some(false),
                    _ => None,
                })
        })
    };
    // A model will send one path where an array belongs.
    let string_array_arg = |key: &str| -> Option<Vec<String>> {
        match args.get(key)? {
            serde_json::Value::Array(items) => items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>(),
            serde_json::Value::String(s) => Some(vec![s.clone()]),
            _ => None,
        }
    };
    // `mode` arrives as a string; a model may spell it the way the schema
    // documents it or the way the enum is named in Rust.
    let grep_mode_arg = |key: &str| -> Option<GrepMode> {
        match args[key].as_str()?.trim().to_ascii_lowercase().as_str() {
            "content" | "lines" | "matches" | "grep" => Some(GrepMode::Content),
            "files_with_matches" | "files" | "paths" | "fileswithmatches" => {
                Some(GrepMode::FilesWithMatches)
            }
            "count" | "counts" => Some(GrepMode::Count),
            _ => None,
        }
    };
    // A task list arrives as objects, and leniently: a bare string is an item
    // with no status, and `tasks` is accepted for `todos`. What it must not do
    // is quietly guess at a status — `normalise_todos` refuses those, after
    // this has recorded what actually arrived.
    let todos_arg = |key: &str| -> Result<Vec<TodoItem>, String> {
        let raw = match args[key]
            .as_array()
            .or_else(|| args["tasks"].as_array())
            .or_else(|| args["items"].as_array())
        {
            Some(a) => a,
            // Required, and refused rather than defaulted. An empty list
            // clears the task list, so "no list at all" and "an empty list"
            // must not mean the same thing: one is a caller that forgot, the
            // other is a deliberate act, and only the caller knows which.
            None => return Err(format!("missing '{key}' argument")),
        };
        raw.iter()
            .map(|v| match v {
                serde_json::Value::String(s) => Ok(TodoItem {
                    content: s.clone(),
                    status: None,
                    active_form: None,
                }),
                serde_json::Value::Object(o) => {
                    let content = ["content", "text", "task", "title"]
                        .iter()
                        .find_map(|k| o.get(*k).and_then(|v| v.as_str()))
                        .ok_or("each todo needs a `content` string")?;
                    Ok(TodoItem {
                        content: content.to_string(),
                        status: o.get("status").and_then(|v| v.as_str()).map(str::to_string),
                        active_form: ["active_form", "activeForm"]
                            .iter()
                            .find_map(|k| o.get(*k).and_then(|v| v.as_str()))
                            .map(str::to_string),
                    })
                }
                _ => Err("each todo must be an object or a string".to_string()),
            })
            .collect()
    };
    let edits_arg = |key: &str| -> Result<Vec<Edit>, String> {
        let items = match args.get(key) {
            Some(serde_json::Value::Array(items)) => items.clone(),
            _ => return Err(format!("missing '{key}' argument")),
        };
        let mut edits = Vec::with_capacity(items.len());
        for item in &items {
            let old_string = item["old_string"]
                .as_str()
                .ok_or("an edit is missing 'old_string'")?
                .to_string();
            let new_string = item["new_string"]
                .as_str()
                .ok_or("an edit is missing 'new_string'")?
                .to_string();
            let replace_all = item["replace_all"].as_bool().or_else(|| {
                item["replace_all"].as_str().and_then(|s| {
                    match s.trim().to_ascii_lowercase().as_str() {
                        "true" | "1" | "yes" => Some(true),
                        "false" | "0" | "no" => Some(false),
                        _ => None,
                    }
                })
            });
            edits.push(Edit {
                old_string,
                new_string,
                replace_all,
            });
        }
        Ok(edits)
    };
    match spec.name {
        "read_file" => Ok(Tool::ReadFile {
            path: str_arg("path")?,
            offset: u32_arg("offset"),
            limit: u32_arg("limit"),
        }),
        "write_file" => Ok(Tool::WriteFile {
            path: str_arg("path")?,
            content: str_arg("content")?,
        }),
        "edit_file" => Ok(Tool::EditFile {
            path: str_arg_any(&["path", "file"])?,
            old_string: str_arg_any(&["old_string", "old_str", "find"])?,
            new_string: str_arg_any(&["new_string", "new_str", "replace", "replace_with"])?,
            replace_all: bool_arg("replace_all"),
        }),
        "multi_edit" => Ok(Tool::MultiEdit {
            path: str_arg("path")?,
            edits: edits_arg("edits")?,
        }),
        "apply_patch" => Ok(Tool::ApplyPatch {
            path: str_arg("path")?,
            patch: str_arg("patch")?,
        }),
        "delete_file" => Ok(Tool::DeleteFile {
            path: str_arg("path")?,
        }),
        "move_file" => Ok(Tool::MoveFile {
            from: str_arg_any(&["from", "src", "source"])?,
            to: str_arg_any(&["to", "dest", "destination"])?,
        }),
        "copy_file" => Ok(Tool::CopyFile {
            from: str_arg_any(&["from", "src", "source"])?,
            to: str_arg_any(&["to", "dest", "destination"])?,
        }),
        "create_directory" => Ok(Tool::CreateDirectory {
            path: str_arg("path")?,
        }),
        "read_many_files" => Ok(Tool::ReadManyFiles {
            paths: string_array_arg("paths")
                .ok_or_else(|| "missing 'paths' argument".to_string())?,
        }),
        "grep" => Ok(Tool::Grep {
            pattern: str_arg_any(&["pattern", "query", "regex", "search"])?,
            // `path` is optional and means "the whole workspace" when absent.
            path: args["path"].as_str().map(str::to_string),
            include: str_arg_any(&["include", "glob", "file_pattern"]).ok(),
            exclude: str_arg_any(&["exclude", "exclude_glob"]).ok(),
            mode: grep_mode_arg("mode"),
            max_results: u32_arg("max_results"),
        }),
        "glob" => Ok(Tool::Glob {
            pattern: str_arg_any(&["pattern", "glob", "query"])?,
            path: args["path"].as_str().map(str::to_string),
            max_results: u32_arg("max_results"),
        }),
        "run_command" => Ok(Tool::RunCommand {
            command: str_arg("command")?,
        }),
        "run_command_background" => Ok(Tool::RunCommandBackground {
            command: str_arg_any(&["command", "cmd", "script"])?,
        }),
        "command_output" => Ok(Tool::CommandOutput {
            // Required: a read has to say which command it is reading. There
            // is no "the one you meant" that is not a guess, and guessing
            // wrong reads a different process's output as though it were the
            // one asked for.
            id: u64_arg("id").ok_or("missing 'id' argument")?,
            cursor: u64_arg("cursor"),
        }),
        "kill_command" => Ok(Tool::KillCommand {
            id: u64_arg("id").ok_or("missing 'id' argument")?,
        }),
        "list_directory" => Ok(Tool::ListDirectory {
            path: str_arg("path")?,
        }),
        "git_status" => Ok(Tool::GitStatus),
        "git_diff" => Ok(Tool::GitDiff {
            path: args["path"].as_str().map(str::to_string),
        }),
        "git_log" => Ok(Tool::GitLog {
            limit: u32_arg("limit"),
        }),
        "git_add" => Ok(Tool::GitAdd {
            path: args["path"].as_str().map(str::to_string),
        }),
        "git_unstage" => Ok(Tool::GitUnstage {
            path: str_arg("path")?,
        }),
        "git_commit" => Ok(Tool::GitCommit {
            message: str_arg_any(&["message", "commit_message", "summary", "subject"])?,
        }),
        "git_branches" => Ok(Tool::GitBranches),
        "git_checkout" => Ok(Tool::GitCheckout {
            name: str_arg_any(&["name", "branch", "branch_name"])?,
        }),
        "git_create_branch" => Ok(Tool::GitCreateBranch {
            name: str_arg_any(&["name", "branch", "branch_name"])?,
            base: args["base"]
                .as_str()
                .or_else(|| args["from"].as_str())
                .map(str::to_string),
            checkout: bool_arg("checkout"),
        }),
        "git_show" => Ok(Tool::GitShow {
            oid: str_arg_any(&["oid", "commit", "sha", "ref"])?,
        }),
        "git_commit_diff" => Ok(Tool::GitCommitDiff {
            oid: str_arg_any(&["oid", "commit", "sha", "ref"])?,
        }),
        "todo_write" => Ok(Tool::TodoWrite {
            todos: todos_arg("todos")?,
        }),
        "todo_read" => Ok(Tool::TodoRead),
        "set_objective" => Ok(Tool::SetObjective {
            text: str_arg_any(&["text", "objective", "goal"])?,
        }),
        "remember_decision" => Ok(Tool::RememberDecision {
            summary: str_arg_any(&["summary", "decision", "text"])?,
            reason: str_arg_any(&["reason", "why"]).ok(),
        }),
        "remember_constraint" => Ok(Tool::RememberConstraint {
            text: str_arg_any(&["text", "constraint", "summary"])?,
        }),
        "remember_attempt" => Ok(Tool::RememberAttempt {
            description: str_arg_any(&["description", "attempt", "text"])?,
            succeeded: args["succeeded"].as_bool(),
        }),
        "get_facts" => Ok(Tool::GetFacts {
            session_id: args["session_id"].as_i64(),
        }),
        "list_sessions" => Ok(Tool::ListSessions {
            limit: u32_arg("limit"),
        }),
        "request_handoff" => Ok(Tool::RequestHandoff {
            reason: str_arg_any(&["reason", "why", "summary"])?,
            next_step: str_arg_any(&["next_step", "next"]).ok(),
        }),
        "get_handoff" => Ok(Tool::GetHandoff),
        "describe_tool" => Ok(Tool::DescribeTool {
            name: str_arg("name")?,
        }),
        "list_tools" => Ok(Tool::ListTools),
        "web_fetch" => Ok(Tool::WebFetch {
            url: str_arg_any(&["url", "link", "href"])?,
            max_bytes: u64_arg("max_bytes"),
        }),
        "web_search" => Ok(Tool::WebSearch {
            query: str_arg_any(&["query", "q", "search", "text"])?,
            max_results: u32_arg("max_results"),
        }),
        "notebook_read" => Ok(Tool::NotebookRead {
            path: str_arg("path")?,
        }),
        "notebook_edit" => Ok(Tool::NotebookEdit {
            path: str_arg("path")?,
            cell_id: str_arg_any(&["cell_id", "cell", "id"])?,
            new_source: str_arg_any(&["new_source", "source", "content", "code"])?,
            cell_type: str_arg_any(&["cell_type", "type"]).ok(),
        }),
        "delegate_task" => Ok(Tool::DelegateTask {
            task: str_arg_any(&["task", "prompt", "description"])?,
            context: str_arg_any(&["context", "background"]).ok(),
        }),
        "lsp_diagnostics" => Ok(Tool::LspDiagnostics {
            path: args["path"].as_str().map(str::to_string),
            severity: str_arg_any(&["severity", "level"]).ok(),
        }),
        "lsp_definition" => Ok(Tool::LspDefinition {
            path: str_arg("path")?,
            line: u32_arg("line").ok_or("missing 'line' argument")?,
            character: u32_arg("character").ok_or("missing 'character' argument")?,
        }),
        "lsp_references" => Ok(Tool::LspReferences {
            path: str_arg("path")?,
            line: u32_arg("line").ok_or("missing 'line' argument")?,
            character: u32_arg("character").ok_or("missing 'character' argument")?,
            include_declaration: bool_arg("include_declaration"),
        }),
        "lsp_symbols" => Ok(Tool::LspSymbols {
            path: args["path"].as_str().map(str::to_string),
            query: str_arg_any(&["query", "q", "name"]).ok(),
        }),
        "ask_user" => Ok(Tool::AskUser {
            question: str_arg_any(&["question", "prompt", "text"])?,
            options: string_array_arg("options").unwrap_or_default(),
        }),
        "propose_plan" => Ok(Tool::ProposePlan {
            plan: str_arg_any(&["plan", "text", "summary"])?,
            steps: string_array_arg("steps").unwrap_or_default(),
        }),
        "monitor" => Ok(Tool::Monitor {
            path: args["path"].as_str().map(str::to_string),
            command_id: u64_arg("command_id"),
            pattern: str_arg_any(&["pattern", "match", "regex"]).ok(),
            timeout_ms: u32_arg("timeout_ms"),
        }),
        "notify" => Ok(Tool::Notify {
            title: str_arg_any(&["title", "heading"])?,
            body: str_arg_any(&["body", "message", "text"])?,
            level: str_arg_any(&["level", "severity"]).ok(),
        }),
        "enter_worktree" => Ok(Tool::EnterWorktree {
            name: str_arg_any(&["name", "branch", "dir"]).ok(),
        }),
        "exit_worktree" => Ok(Tool::ExitWorktree {
            action: str_arg_any(&["action", "mode"]).ok(),
        }),
        "read_media" => Ok(Tool::ReadMedia {
            path: str_arg("path")?,
        }),
        "publish_artifact" => Ok(Tool::PublishArtifact {
            path: str_arg("path")?,
            title: str_arg_any(&["title", "name"]).ok(),
        }),
        "report_findings" => Ok(Tool::ReportFindings {
            findings: findings_arg(args)?,
            summary: str_arg_any(&["summary", "overview"]).ok(),
        }),
        other => Err(format!("tool not implemented: {other}")),
    }
}

/// Parse a `report_findings` list leniently but not silently.
///
/// `severity` is folded to the three levels the renderer understands; an
/// unrecognised one is refused rather than downgraded, because a review that
/// says `severity: "blocker"` and is rendered as `info` has lost the one
/// thing the reader needed.
fn findings_arg(args: &serde_json::Value) -> Result<Vec<Finding>, String> {
    let items = args["findings"]
        .as_array()
        .ok_or("missing 'findings' argument")?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let path = item["path"].as_str().ok_or("a finding is missing 'path'")?;
        let line = item["line"]
            .as_u64()
            .or_else(|| item["line"].as_str().and_then(|s| s.trim().parse().ok()))
            .ok_or("a finding is missing 'line'")?;
        let severity = match item["severity"]
            .as_str()
            .unwrap_or("warning")
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "error" | "err" | "high" => "error",
            "warning" | "warn" | "medium" => "warning",
            "info" | "information" | "low" | "note" => "info",
            other => return Err(format!("unknown finding severity: {other}")),
        };
        let claim = item["claim"]
            .as_str()
            .or_else(|| item["message"].as_str())
            .ok_or("a finding is missing 'claim'")?;
        out.push(Finding {
            path: path.to_string(),
            line: line.min(u32::MAX as u64) as u32,
            severity: severity.to_string(),
            claim: claim.to_string(),
            evidence: item["evidence"].as_str().map(str::to_string),
        });
    }
    Ok(out)
}

/// JSON Schema (object form) for one tool's arguments — the source for the
/// MCP connector's `tools/list` `inputSchema`. Only the *canonical* argument
/// names appear: the parser still tolerates aliases on the wire, but a
/// connector should advertise the primary names so model output stays
/// predictable. Mirrors [`parse_tool_call`]; the drift-guard tests below
/// keep the two honest to each other. Returns `None` for an unknown tool.
pub fn tool_input_schema(tool_name: &str) -> Option<serde_json::Value> {
    let spec = spec_by_name(tool_name)?;
    let schema = match spec.name {
        "read_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to read, relative to the workspace" },
                "offset": { "type": "integer", "minimum": 1, "description": "1-based first line (default 1)" },
                "limit": { "type": "integer", "minimum": 1, "description": "Max lines (default CHUNK_LINES)" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "list_directory" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to list, relative to the workspace" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "write_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to overwrite" },
                "content": { "type": "string", "description": "Full new contents" }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        }),
        "edit_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to edit" },
                "old_string": { "type": "string", "description": "Exact text to replace" },
                "new_string": { "type": "string", "description": "Replacement text" },
                "replace_all": { "type": "boolean", "description": "Replace every occurrence (default false)" }
            },
            "required": ["path", "old_string", "new_string"],
            "additionalProperties": false
        }),
        "multi_edit" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to edit" },
                "edits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "old_string": { "type": "string" },
                            "new_string": { "type": "string" },
                            "replace_all": { "type": "boolean" }
                        },
                        "required": ["old_string", "new_string"],
                        "additionalProperties": false
                    },
                    "description": "Exact-string edits, applied atomically together"
                }
            },
            "required": ["path", "edits"],
            "additionalProperties": false
        }),
        "apply_patch" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File the patch applies to" },
                "patch": { "type": "string", "description": "Single-file unified diff with context lines" }
            },
            "required": ["path", "patch"],
            "additionalProperties": false
        }),
        "delete_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to delete (not directories)" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "move_file" | "copy_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "description": "Source path" },
                "to": { "type": "string", "description": "Destination path" }
            },
            "required": ["from", "to"],
            "additionalProperties": false
        }),
        "create_directory" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Directory to create, including parents" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "read_many_files" => serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Files to read; a single path string is also accepted"
                }
            },
            "required": ["paths"],
            "additionalProperties": false
        }),
        "grep" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Regular expression to search for" },
                "path": { "type": "string", "description": "File or directory to search, relative to the workspace (default: the whole workspace)" },
                "include": { "type": "string", "description": "Only search files whose path matches this glob, e.g. '*.rs'" },
                "exclude": { "type": "string", "description": "Skip files whose path matches this glob, e.g. '*.min.js'" },
                "mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "content = matching lines (default), files_with_matches = paths only, count = matches per file"
                },
                "max_results": { "type": "integer", "description": "Stop after this many matches (default 200, and 200 is the ceiling)" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        "glob" => serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": { "type": "string", "description": "Glob to match paths against, e.g. '**/*.rs' or '*.md'" },
                "path": { "type": "string", "description": "Directory to search under (default: the whole workspace)" },
                "max_results": { "type": "integer", "description": "Stop after this many paths (default 500, and 500 is the ceiling)" }
            },
            "required": ["pattern"],
            "additionalProperties": false
        }),
        "run_command" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command, run in the project root" }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        "run_command_background" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "Shell command, run in the project root and left running" }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
        "command_output" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Handle from run_command_background" },
                "cursor": {
                    "type": "integer",
                    "description": "Byte offset from a previous read's next_cursor; absent → from the start of what is kept"
                }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        "kill_command" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Handle from run_command_background" }
            },
            "required": ["id"],
            "additionalProperties": false
        }),
        "git_status" => serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
        "git_diff" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Restrict the diff to one path (default: every changed file)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "git_log" => serde_json::json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "How many commits to list (default 20, 200 maximum)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "git_add" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to stage; omit to stage every change" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "git_unstage" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to unstage; the working tree is left alone" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "git_commit" => serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string", "description": "Commit message; the first line is the subject" }
            },
            "required": ["message"],
            "additionalProperties": false
        }),
        "git_branches" => serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
        "git_checkout" => serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Branch to switch to (local branches only)" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        "git_create_branch" => serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Name of the new branch" },
                "base": { "type": "string", "description": "Revision to branch from (default: HEAD)" },
                "checkout": { "type": "boolean", "description": "Switch to it after creating (default false)" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        "git_show" => serde_json::json!({
            "type": "object",
            "properties": {
                "oid": { "type": "string", "description": "Commit id or revision, e.g. 'HEAD~1'" }
            },
            "required": ["oid"],
            "additionalProperties": false
        }),
        "git_commit_diff" => serde_json::json!({
            "type": "object",
            "properties": {
                "oid": { "type": "string", "description": "Commit id or revision" }
            },
            "required": ["oid"],
            "additionalProperties": false
        }),
        "todo_write" => serde_json::json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete list, in order. It replaces the previous list.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string", "description": "What the task is" },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Defaults to pending"
                            },
                            "active_form": { "type": "string", "description": "Present-continuous form for the UI, e.g. 'Running the tests'" }
                        },
                        "required": ["content"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["todos"],
            "additionalProperties": false
        }),
        "todo_read" => serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
        "set_objective" => serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "What this work is trying to achieve" }
            },
            "required": ["text"],
            "additionalProperties": false
        }),
        "remember_decision" => serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "The decision, in one line" },
                "reason": { "type": "string", "description": "Why it was chosen — the part a future reader cannot reconstruct" }
            },
            "required": ["summary"],
            "additionalProperties": false
        }),
        "remember_constraint" => serde_json::json!({
            "type": "object",
            "properties": {
                "text": { "type": "string", "description": "The constraint, e.g. 'must run on Python 3.9'" }
            },
            "required": ["text"],
            "additionalProperties": false
        }),
        "remember_attempt" => serde_json::json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "What was tried" },
                "succeeded": { "type": "boolean", "description": "Whether it worked; only failures are surfaced in the handoff" }
            },
            "required": ["description"],
            "additionalProperties": false
        }),
        "get_facts" => serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "integer", "description": "Which session to read (default: the connector's own; see list_sessions)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "list_sessions" => serde_json::json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "How many sessions (default 20, 100 maximum)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "request_handoff" => serde_json::json!({
            "type": "object",
            "properties": {
                "reason": { "type": "string", "description": "Why the developer is needed" },
                "next_step": { "type": "string", "description": "What they should do first" }
            },
            "required": ["reason"],
            "additionalProperties": false
        }),
        "get_handoff" => serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
        "describe_tool" => serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Tool whose full argument schema to show" }
            },
            "required": ["name"],
            "additionalProperties": false
        }),
        "list_tools" => serde_json::json!({
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false
        }),
        "web_fetch" => serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "Absolute http(s) URL to fetch" },
                "max_bytes": { "type": "integer", "description": "Read cap in bytes (clamped to the tool's maximum)" }
            },
            "required": ["url"],
            "additionalProperties": false
        }),
        "web_search" => serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query" },
                "max_results": { "type": "integer", "description": "How many results (default 8, 20 maximum)" }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
        "notebook_read" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": ".ipynb file to read" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "notebook_edit" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": ".ipynb file to edit" },
                "cell_id": { "type": "string", "description": "The cell's id (as returned by notebook_read)" },
                "new_source": { "type": "string", "description": "Replacement source for the cell" },
                "cell_type": { "type": "string", "enum": ["code", "markdown"], "description": "Change the cell type; absent → unchanged" }
            },
            "required": ["path", "cell_id", "new_source"],
            "additionalProperties": false
        }),
        "delegate_task" => serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string", "description": "The sub-task to hand off" },
                "context": { "type": "string", "description": "Context the sub-task needs" }
            },
            "required": ["task"],
            "additionalProperties": false
        }),
        "lsp_diagnostics" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to check (default: every file the server has opened)" },
                "severity": { "type": "string", "enum": ["error", "warning", "information", "hint"], "description": "Minimum severity to report (default: error and warning)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "lsp_definition" | "lsp_references" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File the position is in" },
                "line": { "type": "integer", "minimum": 1, "description": "1-based line" },
                "character": { "type": "integer", "minimum": 1, "description": "1-based column" },
                "include_declaration": { "type": "boolean", "description": "references only: include the declaration itself (default true)" }
            },
            "required": ["path", "line", "character"],
            "additionalProperties": false
        }),
        "lsp_symbols" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Document to outline; absent → search the workspace" },
                "query": { "type": "string", "description": "Workspace symbols matching this query" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "ask_user" => serde_json::json!({
            "type": "object",
            "properties": {
                "question": { "type": "string", "description": "The question to put to the developer" },
                "options": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "2–4 labelled choices; empty means a free-text answer"
                }
            },
            "required": ["question"],
            "additionalProperties": false
        }),
        "propose_plan" => serde_json::json!({
            "type": "object",
            "properties": {
                "plan": { "type": "string", "description": "What you intend to do, in prose" },
                "steps": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "The discrete steps, in order"
                }
            },
            "required": ["plan"],
            "additionalProperties": false
        }),
        "monitor" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File or directory to watch for changes" },
                "command_id": { "type": "integer", "description": "Background command whose output to watch" },
                "pattern": { "type": "string", "description": "Regular expression the new output or path must match" },
                "timeout_ms": { "type": "integer", "description": "How long to wait before giving up (default 60000)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "notify" => serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string", "description": "Notification title" },
                "body": { "type": "string", "description": "Notification body" },
                "level": { "type": "string", "enum": ["info", "success", "warning", "error"], "description": "Severity (default info)" }
            },
            "required": ["title", "body"],
            "additionalProperties": false
        }),
        "enter_worktree" => serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Worktree directory name (default: a generated one)" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "exit_worktree" => serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "enum": ["keep", "discard"], "description": "keep (default, refuses if dirty) or discard" }
            },
            "required": [],
            "additionalProperties": false
        }),
        "read_media" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Image or PDF file to return as media" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "publish_artifact" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "File to hand to the user" },
                "title": { "type": "string", "description": "Display title for the artifact" }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
        "report_findings" => serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": "string", "description": "One-line summary of the review" },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "line": { "type": "integer", "minimum": 1 },
                            "severity": { "type": "string", "enum": ["error", "warning", "info"] },
                            "claim": { "type": "string" },
                            "evidence": { "type": "string" }
                        },
                        "required": ["path", "line", "severity", "claim"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["findings"],
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}

/// The JSON Schema for a tool's [`ToolResult::structured`] payload, or `None`
/// for a name with no SPECS row.
///
/// This is the `outputSchema` the MCP surface declares, which puts a real
/// constraint on it: MCP requires the `structuredContent` we return to
/// *conform* to the schema we advertise, so these rows must describe exactly
/// what the executors emit and nothing more. `structured_content_conforms_to_declared_schema`
/// holds the two in step — when you teach an executor a new field, teach its
/// schema in the same commit.
///
/// Nullable fields are `["integer", "null"]` rather than omitted-keys, because
/// a model paging a file is better served by an explicit `next_offset: null`
/// (there is no more) than by having to distinguish "absent" from "last page".
pub fn output_schema(tool_name: &str) -> Option<serde_json::Value> {
    let spec = spec_by_name(tool_name)?;
    let schema = match spec.name {
        "read_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The file that was read" },
                "start_line": { "type": "integer", "description": "1-based first line of this chunk" },
                "end_line": { "type": "integer", "description": "1-based last line of this chunk" },
                "total_lines": { "type": "integer", "description": "Lines in the whole file" },
                "bytes_shown": { "type": "integer", "description": "Bytes of file content in this chunk" },
                "total_bytes": { "type": "integer", "description": "Bytes in the whole file" },
                "truncated": { "type": "boolean", "description": "True when lines remain after this chunk" },
                "next_offset": {
                    "type": ["integer", "null"],
                    "description": "Pass as `offset` to read the next chunk; null at end of file"
                }
            },
            "required": [
                "path", "start_line", "end_line", "total_lines",
                "bytes_shown", "total_bytes", "truncated", "next_offset"
            ],
            "additionalProperties": false
        }),
        "read_many_files" => serde_json::json!({
            "type": "object",
            "properties": {
                "requested": { "type": "integer", "description": "Paths asked for" },
                "shown": { "type": "integer", "description": "Paths whose content was returned" },
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "status": {
                                "type": "string",
                                "enum": ["shown", "skipped", "too_large", "error"],
                                "description": "skipped = sensitive path; too_large = over this batch's budget"
                            },
                            "lines": { "type": "integer", "description": "Line count, when known" },
                            "reason": { "type": "string", "description": "Why it was not shown" }
                        },
                        "required": ["path", "status"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["requested", "shown", "files"],
            "additionalProperties": false
        }),
        "grep" => serde_json::json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["content", "files_with_matches", "count"],
                    "description": "Which output shape was produced"
                },
                "matches": {
                    "type": "array",
                    "description": "Matching lines — populated in content mode, empty otherwise",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Workspace-relative path" },
                            "line": { "type": "integer", "description": "1-based line number" },
                            "text": { "type": "string", "description": "The matching line, clipped to 400 characters" }
                        },
                        "required": ["path", "line", "text"],
                        "additionalProperties": false
                    }
                },
                "files": {
                    "type": "array",
                    "description": "Files with at least one match — populated in files_with_matches and count modes",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Workspace-relative path" },
                            "count": { "type": "integer", "description": "Matching lines in that file" }
                        },
                        "required": ["path", "count"],
                        "additionalProperties": false
                    }
                },
                "scanned": { "type": "integer", "description": "Files actually read" },
                "truncated": {
                    "type": "boolean",
                    "description": "True when the result cap stopped the search before the tree was exhausted"
                },
                "failures": {
                    "type": "array",
                    "description": "Paths that could not be searched; the walk continued past them",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "reason": { "type": "string", "description": "Why it could not be read" }
                        },
                        "required": ["path", "reason"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["mode", "matches", "files", "scanned", "truncated", "failures"],
            "additionalProperties": false
        }),
        "glob" => serde_json::json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Workspace-relative paths that matched"
                },
                "scanned": { "type": "integer", "description": "Files examined" },
                "truncated": {
                    "type": "boolean",
                    "description": "True when the result cap stopped the walk before the tree was exhausted"
                }
            },
            "required": ["paths", "scanned", "truncated"],
            "additionalProperties": false
        }),
        "list_directory" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "The directory that was listed" },
                "count": { "type": "integer", "description": "Number of entries" },
                "entries": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Entry name, not a full path" },
                            "kind": { "type": "string", "enum": ["file", "dir", "other"] },
                            "size": { "type": ["integer", "null"], "description": "Bytes, for files only" }
                        },
                        "required": ["name", "kind", "size"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["path", "count", "entries"],
            "additionalProperties": false
        }),
        "git_status" => serde_json::json!({
            "type": "object",
            "properties": {
                "clean": { "type": "boolean", "description": "True when nothing has changed" },
                "count": { "type": "integer", "description": "Changed files" },
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "status": { "type": "string", "description": "Short git status code, e.g. M, A, ??" },
                            "additions": { "type": "integer" },
                            "deletions": { "type": "integer" }
                        },
                        "required": ["path", "status", "additions", "deletions"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["clean", "count", "files"],
            "additionalProperties": false
        }),
        // Write/edit results carry evidence of what actually changed, so the
        // model can verify its own edit instead of re-reading the file to
        // find out whether it landed.
        "write_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "bytes_written": { "type": "integer", "description": "Bytes now in the file" }
            },
            "required": ["path", "bytes_written"],
            "additionalProperties": false
        }),
        "edit_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "replacements": { "type": "integer", "description": "How many occurrences were replaced" },
                "first_line": { "type": ["integer", "null"], "description": "1-based line of the first replacement" },
                "bytes_before": { "type": "integer" },
                "bytes_after": { "type": "integer" }
            },
            "required": ["path", "replacements", "first_line", "bytes_before", "bytes_after"],
            "additionalProperties": false
        }),
        "multi_edit" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "edits_applied": { "type": "integer", "description": "Edits in the batch that matched" },
                "replacements": { "type": "integer", "description": "Total occurrences replaced" },
                "lines": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "1-based line of each edit's first replacement, in order"
                }
            },
            "required": ["path", "edits_applied", "replacements", "lines"],
            "additionalProperties": false
        }),
        "apply_patch" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "hunks_applied": { "type": "integer" },
                "lines": {
                    "type": "array",
                    "items": { "type": "integer" },
                    "description": "1-based line where each hunk was applied, in order"
                }
            },
            "required": ["path", "hunks_applied", "lines"],
            "additionalProperties": false
        }),
        "delete_file" => serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
            "additionalProperties": false
        }),
        "move_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "from": { "type": "string" },
                "to": { "type": "string" }
            },
            "required": ["from", "to"],
            "additionalProperties": false
        }),
        "copy_file" => serde_json::json!({
            "type": "object",
            "properties": {
                "from": { "type": "string" },
                "to": { "type": "string" },
                "bytes": { "type": "integer", "description": "Bytes copied" }
            },
            "required": ["from", "to", "bytes"],
            "additionalProperties": false
        }),
        "create_directory" => serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"],
            "additionalProperties": false
        }),
        "run_command" => serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string" },
                "exit_code": {
                    "type": ["integer", "null"],
                    "description": "Process exit status; null when it was killed"
                },
                "timed_out": { "type": "boolean" },
                "truncated": { "type": "boolean", "description": "Output hit the capture cap" }
            },
            "required": ["command", "exit_code", "timed_out", "truncated"],
            "additionalProperties": false
        }),
        "run_command_background" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Handle for command_output and kill_command" },
                "pid": { "type": ["integer", "null"] },
                "command": { "type": "string" }
            },
            "required": ["id", "pid", "command"],
            "additionalProperties": false
        }),
        "command_output" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "status": {
                    "type": "string",
                    "enum": ["running", "exited", "killed", "failed"],
                    "description": "How the command is doing: killed = stopped by kill_command"
                },
                "exit_code": {
                    "type": ["integer", "null"],
                    "description": "Process exit status; null while running, and for a command the runtime never started"
                },
                "cursor": { "type": "integer", "description": "Where this read started" },
                "next_cursor": {
                    "type": "integer",
                    "description": "Pass back as `cursor` to read only what is new"
                },
                "lost": {
                    "type": "boolean",
                    "description": "Output was dropped before you read it — the command outran what is kept"
                },
                "more": { "type": "boolean", "description": "Output is waiting past this read" },
                "complete": {
                    "type": "boolean",
                    "description": "Ended and fully drained; false while output may still be arriving"
                },
                "elapsed_ms": { "type": "integer" }
            },
            "required": [
                "id", "status", "exit_code", "cursor", "next_cursor",
                "lost", "more", "complete", "elapsed_ms"
            ],
            "additionalProperties": false
        }),
        "kill_command" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer" },
                "status": {
                    "type": "string",
                    "enum": ["running", "exited", "killed", "failed"],
                    "description": "How the command ended"
                },
                "exit_code": { "type": ["integer", "null"] },
                "already_finished": {
                    "type": "boolean",
                    "description": "True when it had stopped on its own, so nothing was signalled"
                }
            },
            "required": ["id", "status", "exit_code", "already_finished"],
            "additionalProperties": false
        }),
        "list_tools" => serde_json::json!({
            "type": "object",
            "properties": {
                "tools": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Canonical names of every tool this connector exposes"
                }
            },
            "required": ["tools"],
            "additionalProperties": false
        }),
        "git_diff" => serde_json::json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer", "description": "Changed files reported" },
                "truncated": { "type": "boolean", "description": "True when a patch was omitted for size" },
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string", "description": "Workspace-relative path" },
                            "status": { "type": "string", "description": "untracked | modified | deleted | renamed" },
                            "added": { "type": "integer", "description": "Lines added" },
                            "deleted": { "type": "integer", "description": "Lines deleted" },
                            "patch": { "type": "string", "description": "Unified diff; empty when omitted for size" },
                            "patch_omitted": { "type": "boolean", "description": "True when the patch did not fit the result budget" }
                        },
                        "required": ["path", "status", "added", "deleted", "patch", "patch_omitted"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["count", "truncated", "files"],
            "additionalProperties": false
        }),
        "git_log" => serde_json::json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer", "description": "Commits returned" },
                "commits": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "oid": { "type": "string", "description": "Full commit id" },
                            "summary": { "type": "string", "description": "First line of the message" },
                            "author": { "type": "string" },
                            "timestamp": { "type": "integer", "description": "Unix seconds" }
                        },
                        "required": ["oid", "summary", "author", "timestamp"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["count", "commits"],
            "additionalProperties": false
        }),
        "git_branches" => serde_json::json!({
            "type": "object",
            "properties": {
                "current": { "type": ["string", "null"], "description": "Checked-out branch, null when detached" },
                "count": { "type": "integer" },
                "branches": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "is_current": { "type": "boolean" }
                        },
                        "required": ["name", "is_current"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["current", "count", "branches"],
            "additionalProperties": false
        }),
        "git_add" | "git_unstage" => serde_json::json!({
            "type": "object",
            "properties": {
                "scoped_to": { "type": ["string", "null"], "description": "The path acted on; null means every change" }
            },
            "required": ["scoped_to"],
            "additionalProperties": false
        }),
        "git_commit" => serde_json::json!({
            "type": "object",
            "properties": {
                "oid": { "type": "string", "description": "The new commit's id" },
                "summary": { "type": "string", "description": "First line of the message" }
            },
            "required": ["oid", "summary"],
            "additionalProperties": false
        }),
        "git_checkout" | "git_create_branch" => serde_json::json!({
            "type": "object",
            "properties": {
                "branch": { "type": "string", "description": "The branch that was switched to or created" },
                "created": { "type": "boolean", "description": "True when this call created it" },
                "checked_out": { "type": "boolean", "description": "True when it is now the current branch" }
            },
            "required": ["branch", "created", "checked_out"],
            "additionalProperties": false
        }),
        "git_show" => serde_json::json!({
            "type": "object",
            "properties": {
                "oid": { "type": "string" },
                "summary": { "type": "string", "description": "First line of the message" },
                "author": { "type": "string" },
                "email": { "type": ["string", "null"] },
                "timestamp": { "type": "integer", "description": "Unix seconds" },
                "patch": { "type": "string", "description": "Unified diff against the parent; empty when omitted for size" },
                "patch_omitted": { "type": "boolean" }
            },
            "required": ["oid", "summary", "author", "email", "timestamp", "patch", "patch_omitted"],
            "additionalProperties": false
        }),
        "git_commit_diff" => serde_json::json!({
            "type": "object",
            "properties": {
                "oid": { "type": "string" },
                "patch": { "type": "string", "description": "Unified diff against the parent; empty when omitted for size" },
                "patch_omitted": { "type": "boolean" }
            },
            "required": ["oid", "patch", "patch_omitted"],
            "additionalProperties": false
        }),
        "todo_write" | "todo_read" => serde_json::json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer", "description": "Items in the list" },
                "todos": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending", "in_progress", "completed"] },
                            "active_form": { "type": ["string", "null"] }
                        },
                        "required": ["content", "status", "active_form"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["count", "todos"],
            "additionalProperties": false
        }),
        "set_objective" => serde_json::json!({
            "type": "object",
            "properties": {
                "objective": { "type": "string", "description": "The objective now in force" },
                "previous": { "type": ["string", "null"], "description": "What it replaced, if anything" }
            },
            "required": ["objective", "previous"],
            "additionalProperties": false
        }),
        "remember_decision" | "remember_constraint" | "remember_attempt" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Row id of the recorded fact" },
                "kind": { "type": "string", "enum": ["decision", "constraint", "attempt"] },
                "session_id": { "type": "integer" }
            },
            "required": ["id", "kind", "session_id"],
            "additionalProperties": false
        }),
        "get_facts" => serde_json::json!({
            "type": "object",
            "properties": {
                "session_id": { "type": "integer", "description": "The session that was read" },
                "objective": { "type": ["string", "null"] },
                "progress_percent": { "type": "integer" },
                "decisions": { "type": "array", "items": { "type": "string" } },
                "failed_attempts": { "type": "array", "items": { "type": "string" } },
                "constraints": { "type": "array", "items": { "type": "string" } },
                "changed_files": { "type": "array", "items": { "type": "string" } }
            },
            "required": [
                "session_id", "objective", "progress_percent", "decisions",
                "failed_attempts", "constraints", "changed_files"
            ],
            "additionalProperties": false
        }),
        "list_sessions" => serde_json::json!({
            "type": "object",
            "properties": {
                "count": { "type": "integer" },
                "sessions": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "agent": { "type": "string" },
                            "objective": { "type": ["string", "null"] },
                            "started_at": { "type": "string" },
                            "events": { "type": "integer" }
                        },
                        "required": ["id", "agent", "objective", "started_at", "events"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["count", "sessions"],
            "additionalProperties": false
        }),
        "request_handoff" => serde_json::json!({
            "type": "object",
            "properties": {
                "id": { "type": "integer", "description": "Row id of the request" },
                "objective": { "type": ["string", "null"], "description": "The objective in force when it was made" }
            },
            "required": ["id", "objective"],
            "additionalProperties": false
        }),
        "get_handoff" => serde_json::json!({
            "type": "object",
            "properties": {
                "objective": { "type": "string" },
                "progress_percent": { "type": "integer" },
                "files_changed": { "type": "integer" },
                "errors_remaining": { "type": "integer" },
                "next_step": { "type": ["string", "null"] },
                "files": { "type": "array", "items": { "type": "string" } },
                "context": { "type": ["string", "null"] },
                "end_reason": { "type": ["string", "null"] },
                "decisions": { "type": "array", "items": { "type": "string" } },
                "failed_attempts": { "type": "array", "items": { "type": "string" } },
                "constraints": { "type": "array", "items": { "type": "string" } },
                "generated_at": { "type": "string" }
            },
            "additionalProperties": false
        }),
        "describe_tool" => serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "summary": { "type": "string" },
                "args": { "type": "string", "description": "Argument list as written in the manifest" },
                "approval": { "type": "string", "description": "When the desktop app must approve this tool" }
            },
            "required": ["name", "summary", "args", "approval"],
            "additionalProperties": false
        }),
        "web_fetch" => serde_json::json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL the bytes actually came from (after redirects)" },
                "status": { "type": "integer", "description": "HTTP status code" },
                "content_type": { "type": "string" },
                "bytes": { "type": "integer", "description": "Body bytes read before text reduction" },
                "truncated": { "type": "boolean" },
                "reduced_html": { "type": "boolean", "description": "True when the body was HTML and was reduced to text" },
                "redirects": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["url", "status", "content_type", "bytes", "truncated", "reduced_html", "redirects"],
            "additionalProperties": false
        }),
        "web_search" => serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "results": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "url": { "type": "string" },
                            "snippet": { "type": "string" }
                        },
                        "required": ["title", "url", "snippet"],
                        "additionalProperties": false
                    }
                },
                "truncated": { "type": "boolean" }
            },
            "required": ["query", "results", "truncated"],
            "additionalProperties": false
        }),
        "notebook_read" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "kernel": { "type": ["string", "null"] },
                "cells": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "cell_id": { "type": "string" },
                            "cell_type": { "type": "string" },
                            "source": { "type": "string" },
                            "outputs": { "type": "integer" }
                        },
                        "required": ["cell_id", "cell_type", "source", "outputs"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["path", "kernel", "cells"],
            "additionalProperties": false
        }),
        "notebook_edit" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "cell_id": { "type": "string" },
                "cell_type": { "type": "string" },
                "bytes_written": { "type": "integer" }
            },
            "required": ["path", "cell_id", "cell_type", "bytes_written"],
            "additionalProperties": false
        }),
        "delegate_task" => serde_json::json!({
            "type": "object",
            "properties": {
                "task": { "type": "string" },
                "accepted": { "type": "boolean" },
                "token": { "type": ["string", "null"], "description": "Handle the parent can poll for the result" },
                "note": { "type": "string" }
            },
            "required": ["task", "accepted", "token", "note"],
            "additionalProperties": false
        }),
        "lsp_diagnostics" => serde_json::json!({
            "type": "object",
            "properties": {
                "server": { "type": ["string", "null"], "description": "Language server that answered, or null when none is available" },
                "scanned": { "type": "integer", "description": "Files checked" },
                "diagnostics": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "line": { "type": "integer" },
                            "character": { "type": "integer" },
                            "severity": { "type": "string" },
                            "message": { "type": "string" },
                            "source": { "type": "string" }
                        },
                        "required": ["path", "line", "character", "severity", "message", "source"],
                        "additionalProperties": false
                    }
                },
                "truncated": { "type": "boolean" },
                "unavailable": { "type": "boolean", "description": "True when no server for this project could be started" }
            },
            "required": ["server", "scanned", "diagnostics", "truncated", "unavailable"],
            "additionalProperties": false
        }),
        "lsp_definition" | "lsp_references" => serde_json::json!({
            "type": "object",
            "properties": {
                "server": { "type": ["string", "null"] },
                "locations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "line": { "type": "integer" },
                            "character": { "type": "integer" }
                        },
                        "required": ["path", "line", "character"],
                        "additionalProperties": false
                    }
                },
                "truncated": { "type": "boolean" },
                "unavailable": { "type": "boolean" }
            },
            "required": ["server", "locations", "truncated", "unavailable"],
            "additionalProperties": false
        }),
        "lsp_symbols" => serde_json::json!({
            "type": "object",
            "properties": {
                "server": { "type": ["string", "null"] },
                "symbols": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "kind": { "type": "string" },
                            "path": { "type": "string" },
                            "line": { "type": "integer" },
                            "container": { "type": ["string", "null"] }
                        },
                        "required": ["name", "kind", "path", "line", "container"],
                        "additionalProperties": false
                    }
                },
                "truncated": { "type": "boolean" },
                "unavailable": { "type": "boolean" }
            },
            "required": ["server", "symbols", "truncated", "unavailable"],
            "additionalProperties": false
        }),
        "ask_user" => serde_json::json!({
            "type": "object",
            "properties": {
                "question": { "type": "string" },
                "option": { "type": ["string", "null"], "description": "The chosen label, or null when the answer is free text" },
                "answer": { "type": "string", "description": "The developer's answer" },
                "timed_out": { "type": "boolean" }
            },
            "required": ["question", "option", "answer", "timed_out"],
            "additionalProperties": false
        }),
        "propose_plan" => serde_json::json!({
            "type": "object",
            "properties": {
                "approved": { "type": "boolean" },
                "comment": { "type": ["string", "null"], "description": "The reviewer's note on allow or deny" },
                "steps": { "type": "integer", "description": "Steps pinned into the trace" },
                "timed_out": { "type": "boolean" }
            },
            "required": ["approved", "comment", "steps", "timed_out"],
            "additionalProperties": false
        }),
        "monitor" => serde_json::json!({
            "type": "object",
            "properties": {
                "matched": { "type": "boolean" },
                "reason": { "type": "string", "description": "What fired: path change, pattern match, or timeout" },
                "detail": { "type": ["string", "null"] },
                "elapsed_ms": { "type": "integer" }
            },
            "required": ["matched", "reason", "detail", "elapsed_ms"],
            "additionalProperties": false
        }),
        "notify" => serde_json::json!({
            "type": "object",
            "properties": {
                "title": { "type": "string" },
                "level": { "type": "string" },
                "raised": { "type": "boolean" }
            },
            "required": ["title", "level", "raised"],
            "additionalProperties": false
        }),
        "enter_worktree" | "exit_worktree" => serde_json::json!({
            "type": "object",
            "properties": {
                "worktree": { "type": ["string", "null"], "description": "Absolute path of the worktree" },
                "branch": { "type": ["string", "null"] },
                "active": { "type": "boolean", "description": "Whether the session is now inside a worktree" },
                "removed": { "type": "boolean" }
            },
            "required": ["worktree", "branch", "active", "removed"],
            "additionalProperties": false
        }),
        "read_media" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "media_type": { "type": "string", "description": "MIME type of the returned block" },
                "bytes": { "type": "integer" },
                "kind": { "type": "string", "enum": ["image", "pdf", "text"] }
            },
            "required": ["path", "media_type", "bytes", "kind"],
            "additionalProperties": false
        }),
        "publish_artifact" => serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "artifact": { "type": "string", "description": "Absolute path of the published copy" },
                "title": { "type": "string" },
                "bytes": { "type": "integer" },
                "media_type": { "type": "string" }
            },
            "required": ["path", "artifact", "title", "bytes", "media_type"],
            "additionalProperties": false
        }),
        "report_findings" => serde_json::json!({
            "type": "object",
            "properties": {
                "summary": { "type": ["string", "null"] },
                "count": { "type": "integer" },
                "errors": { "type": "integer" },
                "warnings": { "type": "integer" },
                "findings": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "line": { "type": "integer" },
                            "severity": { "type": "string" },
                            "claim": { "type": "string" },
                            "evidence": { "type": ["string", "null"] }
                        },
                        "required": ["path", "line", "severity", "claim", "evidence"],
                        "additionalProperties": false
                    }
                }
            },
            "required": ["summary", "count", "errors", "warnings", "findings"],
            "additionalProperties": false
        }),
        _ => return None,
    };
    Some(schema)
}

/// Filesystem paths a call touches, for the sensitive-path policy. Tools
/// that take two paths must return both, or a secret can be laundered by
/// copying it to an innocuous name.
pub fn tool_paths(tool: &Tool) -> Vec<&str> {
    match tool {
        Tool::ReadFile { path, .. }
        | Tool::WriteFile { path, .. }
        | Tool::EditFile { path, .. }
        | Tool::MultiEdit { path, .. }
        | Tool::ApplyPatch { path, .. }
        | Tool::DeleteFile { path }
        | Tool::CreateDirectory { path }
        | Tool::ListDirectory { path } => {
            vec![path.as_str()]
        }
        // Both sides of a path pair: a secret can be laundered by copying
        // it to an innocuous name, so the target is checked too.
        Tool::MoveFile { from, to } | Tool::CopyFile { from, to } => {
            vec![from.as_str(), to.as_str()]
        }
        // A search screens its `path` argument here, and every file the walk
        // then reaches is screened again by `walk_files`. Both are needed:
        // this covers the path the caller named, that one covers the files it
        // did not. `pattern` is not a path and is deliberately absent.
        Tool::Grep { path, .. } | Tool::Glob { path, .. } => {
            path.iter().map(String::as_str).collect()
        }
        // The index-level git tools take a path, so they screen like any
        // other path-bearing tool: staging `.env` asks, exactly as reading it
        // would. (The *revision* arguments — `base`, `oid`, `name` — name
        // commits and branches, not files, and are deliberately absent.)
        Tool::GitAdd { path } => path.iter().map(String::as_str).collect(),
        Tool::GitUnstage { path } => vec![path.as_str()],
        Tool::GitDiff { path } => path.iter().map(String::as_str).collect(),
        // Phase 7–10 paths. A `url` is not a filesystem path and a worktree
        // `name` is not one either, so neither appears here; the notebook,
        // media, artifact, monitor and LSP tools all name a file.
        Tool::NotebookRead { path }
        | Tool::NotebookEdit { path, .. }
        | Tool::LspDefinition { path, .. }
        | Tool::LspReferences { path, .. }
        | Tool::ReadMedia { path }
        | Tool::PublishArtifact { path, .. } => vec![path.as_str()],
        Tool::LspDiagnostics { path, .. }
        | Tool::LspSymbols { path, .. }
        | Tool::Monitor { path, .. } => path.iter().map(String::as_str).collect(),
        // A batch's paths are filtered individually at execution; the trace
        // carries the count instead (see `detail`).
        Tool::ReadManyFiles { .. }
        | Tool::RunCommand { .. }
        | Tool::RunCommandBackground { .. }
        | Tool::CommandOutput { .. }
        | Tool::KillCommand { .. }
        | Tool::GitStatus
        | Tool::GitLog { .. }
        | Tool::GitCommit { .. }
        | Tool::GitBranches
        | Tool::GitCheckout { .. }
        | Tool::GitCreateBranch { .. }
        | Tool::GitShow { .. }
        | Tool::GitCommitDiff { .. }
        | Tool::TodoWrite { .. }
        | Tool::TodoRead
        | Tool::SetObjective { .. }
        | Tool::RememberDecision { .. }
        | Tool::RememberConstraint { .. }
        | Tool::RememberAttempt { .. }
        | Tool::GetFacts { .. }
        | Tool::ListSessions { .. }
        | Tool::RequestHandoff { .. }
        | Tool::GetHandoff
        | Tool::WebFetch { .. }
        | Tool::WebSearch { .. }
        | Tool::DelegateTask { .. }
        | Tool::AskUser { .. }
        | Tool::ProposePlan { .. }
        | Tool::Notify { .. }
        | Tool::EnterWorktree { .. }
        | Tool::ExitWorktree { .. }
        | Tool::ReportFindings { .. }
        | Tool::DescribeTool { .. }
        | Tool::ListTools => vec![],
    }
}

/// The per-call detail shown beside the tool name in the approval UI,
/// audit log and activity trace.
pub fn detail(tool: &Tool) -> Option<String> {
    match tool {
        // The offset is part of the detail so the trace and audit log show
        // which chunk was read, not just the same path N times.
        Tool::ReadFile { path, offset, .. } => Some(match offset {
            Some(n) if *n > 1 => format!("{path} from line {n}"),
            _ => path.clone(),
        }),
        Tool::ListDirectory { path } => Some(path.clone()),
        Tool::WriteFile { path, content } => Some(format!("{path} ({} bytes)", content.len())),
        Tool::EditFile {
            path,
            old_string,
            new_string,
            ..
        } => Some(format!(
            "{path} ({} → {} bytes)",
            old_string.len(),
            new_string.len()
        )),
        Tool::MultiEdit { path, edits } => Some(format!("{path} ({} edits)", edits.len())),
        Tool::ApplyPatch { path, patch } => Some(format!("{path} ({} bytes patch)", patch.len())),
        Tool::DeleteFile { path } => Some(path.clone()),
        Tool::MoveFile { from, to } | Tool::CopyFile { from, to } => Some(format!("{from} → {to}")),
        Tool::CreateDirectory { path } => Some(path.clone()),
        Tool::ReadManyFiles { paths } => Some(format!("{} files", paths.len())),
        Tool::RunCommand { command } => Some(command.clone()),
        Tool::RunCommandBackground { command } => Some(command.clone()),
        // The handle rather than the command: two reads of the same command
        // are told apart by which one they are reading, and `kill_command`
        // has nothing else to say about itself.
        Tool::CommandOutput { id, .. } | Tool::KillCommand { id } => Some(format!("#{id}")),
        // The pattern and where it was looked for — the two facts that make
        // two greps in a row distinguishable in the trace.
        Tool::Grep { pattern, path, .. } => {
            Some(format!("{pattern} in {}", path.as_deref().unwrap_or(".")))
        }
        Tool::Glob { pattern, path, .. } => {
            Some(format!("{pattern} in {}", path.as_deref().unwrap_or(".")))
        }
        Tool::GitDiff { path } => Some(path.clone().unwrap_or_else(|| "all changes".into())),
        Tool::GitLog { limit } => Some(format!("last {}", limit.unwrap_or(50))),
        Tool::GitAdd { path } => Some(path.clone().unwrap_or_else(|| "everything".into())),
        Tool::GitUnstage { path } => Some(path.clone()),
        // Only the first line: a commit message is multi-line, and the trace
        // row is one line.
        Tool::GitCommit { message } => Some(message.lines().next().unwrap_or("").to_string()),
        Tool::GitCheckout { name } => Some(name.clone()),
        Tool::GitCreateBranch { name, base, .. } => Some(match base {
            Some(b) => format!("{name} from {b}"),
            None => name.clone(),
        }),
        Tool::GitShow { oid } | Tool::GitCommitDiff { oid } => Some(oid.clone()),
        Tool::GitBranches => None,
        // The text that was recorded *is* the distinguishing fact — two
        // `remember_decision` calls differ only in their summary, and the
        // audit line is worth more with it than without.
        Tool::TodoWrite { todos } => Some(format!("{} item(s)", todos.len())),
        Tool::SetObjective { text } => Some(clip_chars(text, 60)),
        Tool::RememberDecision { summary, .. } => Some(clip_chars(summary, 60)),
        Tool::RememberConstraint { text } => Some(clip_chars(text, 60)),
        Tool::RememberAttempt { description, .. } => Some(clip_chars(description, 60)),
        Tool::GetFacts { session_id } => Some(match session_id {
            Some(id) => format!("session {id}"),
            None => "this session".to_string(),
        }),
        Tool::ListSessions { limit } => limit.map(|n| format!("{n} max")),
        Tool::RequestHandoff { reason, .. } => Some(clip_chars(reason, 60)),
        Tool::TodoRead | Tool::GetHandoff => None,
        Tool::DescribeTool { name } => Some(name.clone()),
        Tool::GitStatus | Tool::ListTools => None,
        // Phase 7–10. The detail is the one fact that tells two calls apart in
        // the trace: a URL, a query, a position, a cell.
        Tool::WebFetch { url, .. } => Some(clip_chars(url, 80)),
        Tool::WebSearch { query, .. } => Some(clip_chars(query, 80)),
        Tool::NotebookRead { path } | Tool::ReadMedia { path } => Some(path.clone()),
        Tool::PublishArtifact { path, .. } => Some(path.clone()),
        Tool::NotebookEdit { path, cell_id, .. } => Some(format!("{path} [{cell_id}]")),
        Tool::DelegateTask { task, .. } => Some(clip_chars(task, 60)),
        Tool::LspDiagnostics { path, .. } => {
            Some(path.clone().unwrap_or_else(|| "workspace".into()))
        }
        Tool::LspDefinition {
            path,
            line,
            character,
        }
        | Tool::LspReferences {
            path,
            line,
            character,
            ..
        } => Some(format!("{path}:{line}:{character}")),
        Tool::LspSymbols { path, query } => Some(
            query
                .clone()
                .or_else(|| path.clone())
                .unwrap_or_else(|| "workspace".into()),
        ),
        Tool::AskUser { question, .. } => Some(clip_chars(question, 60)),
        Tool::ProposePlan { plan, .. } => Some(clip_chars(plan, 60)),
        Tool::Monitor { path, command_id, .. } => match (path, command_id) {
            (Some(p), _) => Some(p.clone()),
            (None, Some(id)) => Some(format!("command #{id}")),
            (None, None) => None,
        },
        Tool::Notify { title, .. } => Some(clip_chars(title, 60)),
        Tool::EnterWorktree { name } => {
            Some(name.clone().unwrap_or_else(|| "new worktree".into()))
        }
        Tool::ExitWorktree { action } => {
            Some(action.clone().unwrap_or_else(|| "keep".into()))
        }
        Tool::ReportFindings { findings, .. } => Some(format!("{} finding(s)", findings.len())),
    }
}

/// A streaming event for a running command. Mirrors the `terminal://run`
/// event the UI renders.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CommandEvent {
    Start {
        command: String,
    },
    Output {
        data: String,
    },
    Exit {
        code: Option<i32>,
        timed_out: bool,
        truncated: bool,
    },
}

/// The error vocabulary, declared once.
///
/// Three things must agree about every code: the Rust variant, the name on
/// the wire (serde's `SCREAMING_SNAKE_CASE` rename), and the name `Display`
/// prints. Keeping those as three hand-maintained lists is how they drift, so
/// this macro takes one list and derives all three — including
/// [`ErrorCode::ALL`], which the exhaustive tests quantify over. Adding a code
/// is one line, and there is no way to add one that `ALL` does not know about.
///
/// The identifier and the wire string are both spelled out rather than
/// computed because serde derives the second from the first; the
/// `every_error_code_is_named_consistently` test asserts the two never
/// disagree.
macro_rules! error_codes {
    ($($variant:ident => $wire:literal),* $(,)?) => {
        /// Structured error code for tool call failures.
        #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum ErrorCode {
            $($variant),*
        }

        impl ErrorCode {
            /// Every variant, in declaration order.
            pub const ALL: &'static [ErrorCode] = &[$(ErrorCode::$variant),*];

            /// The wire name — the one place a code's spelling is written.
            pub const fn name(&self) -> &'static str {
                match self {
                    $(ErrorCode::$variant => $wire),*
                }
            }
        }
    };
}

error_codes! {
    // Files and paths.
    FileNotFound => "FILE_NOT_FOUND",
    FileIsBinary => "FILE_IS_BINARY",
    FileTooLarge => "FILE_TOO_LARGE",
    PathEscapesRoot => "PATH_ESCAPES_ROOT",
    PermissionDenied => "PERMISSION_DENIED",
    SensitivePath => "SENSITIVE_PATH",
    NotADirectory => "NOT_A_DIRECTORY",
    NotAFile => "NOT_A_FILE",
    // Arguments, edits and diffs.
    InvalidArguments => "INVALID_ARGUMENTS",
    StringNotFound => "STRING_NOT_FOUND",
    AmbiguousMatch => "AMBIGUOUS_MATCH",
    PatchDoesNotApply => "PATCH_DOES_NOT_APPLY",
    InvalidDiff => "INVALID_DIFF",
    // Search.
    RegexInvalid => "REGEX_INVALID",
    GlobInvalid => "GLOB_INVALID",
    // Git.
    NotAGitRepo => "NOT_A_GIT_REPO",
    WorktreeDirty => "WORKTREE_DIRTY",
    // Commands and processes.
    BridgePaused => "BRIDGE_PAUSED",
    ExecutionFailed => "EXECUTION_FAILED",
    CommandTimeout => "COMMAND_TIMEOUT",
    ProcessNotFound => "PROCESS_NOT_FOUND",
    OutputGone => "OUTPUT_GONE",
    TooManyProcesses => "TOO_MANY_PROCESSES",
    // Language servers.
    LspUnavailable => "LSP_UNAVAILABLE",
    LspProtocolError => "LSP_PROTOCOL_ERROR",
    // Network.
    NetworkBlocked => "NETWORK_BLOCKED",
    // Notebooks.
    NotebookInvalid => "NOTEBOOK_INVALID",
    // The agent loop and delegation.
    AgentNotAvailable => "AGENT_NOT_AVAILABLE",
    DelegateBudgetExhausted => "DELEGATE_BUDGET_EXHAUSTED",
    // Dispatch.
    UnknownTool => "UNKNOWN_TOOL",
    InternalError => "INTERNAL_ERROR",
    Denied => "DENIED",
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// A structured error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolError {
    pub code: ErrorCode,
    pub message: String,
}

/// Result of a tool call. `pending` is set when the call awaits an
/// explicit user approval (the caller should wait for resolution).
///
/// `structured` is the machine-readable half of the same result: the facts
/// in `output`, as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub ok: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<ErrorCode>,
    pub pending: Option<String>,
    /// The same facts as `output`, as JSON — a line offset or a replacement
    /// count the model should *read* rather than scrape out of prose. Its
    /// shape is the tool's row in [`output_schema`], which the MCP surface
    /// declares as `outputSchema`; the two are held in step by the
    /// `structured_content_conforms_to_declared_schema` test. `None` for
    /// results with nothing to structure (errors the caller can't act on,
    /// approval pendings).
    pub structured: Option<serde_json::Value>,
    /// Binary content (an image, or a PDF) that must cross the MCP boundary as
    /// a real content block rather than as text or as a JSON string.
    ///
    /// Kept out of `structured` on purpose: base64 expands by a third and the
    /// connector caps a text result at 140,000 characters, so an image in the
    /// structured half would be truncated into something unopenable. The MCP
    /// layer reads this field and emits the right block; the desktop ignores
    /// it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media: Option<MediaPayload>,
}

/// A media block, ready for the MCP boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaPayload {
    /// `image/png`, `application/pdf`, …
    pub media_type: String,
    pub base64: String,
    /// `image` | `pdf` — which content block to emit.
    pub kind: String,
}

impl ToolResult {
    pub fn ok(output: String) -> Self {
        Self {
            ok: true,
            output: Some(output),
            error: None,
            error_code: None,
            pending: None,
            structured: None,
            media: None,
        }
    }

    /// A successful call whose facts are also available as JSON. The text in
    /// `output` stays the primary, readable form — a human reads the trace —
    /// while `structured` spares a model from parsing it.
    pub fn ok_structured(output: String, structured: serde_json::Value) -> Self {
        Self {
            ok: true,
            output: Some(output),
            error: None,
            error_code: None,
            pending: None,
            structured: Some(structured),
            media: None,
        }
    }

    /// A successful call whose payload is media plus a short readable caption.
    pub fn ok_media(output: String, structured: serde_json::Value, media: MediaPayload) -> Self {
        Self {
            ok: true,
            output: Some(output),
            error: None,
            error_code: None,
            pending: None,
            structured: Some(structured),
            media: Some(media),
        }
    }

    pub fn err<S: Into<String>>(error: S) -> Self {
        Self {
            ok: false,
            output: None,
            error: Some(error.into()),
            error_code: None,
            pending: None,
            structured: None,
            media: None,
        }
    }
    pub fn err_code(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: None,
            error: Some(message.into()),
            error_code: Some(code),
            pending: None,
            structured: None,
            media: None,
        }
    }
    pub fn pending<S: Into<String>>(summary: S) -> Self {
        Self {
            ok: false,
            output: None,
            error: None,
            error_code: None,
            pending: Some(summary.into()),
            structured: None,
            media: None,
        }
    }
}

/// What a session grant covers. Scoped by the manifest group so a grant
/// reads the way the user thinks about it ("edits", "commands") rather
/// than by internal approval classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrantScope {
    /// The "Editing" group: write_file, edit_file, multi_edit, apply_patch,
    /// copy_file. Destructive tools (delete_file, move_file) are in the
    /// group too but a grant never covers them — that rule lives in
    /// [`grant_matches`] and is not overridable.
    Editing,
    /// The "Commands" group: run_command.
    Commands,
}

impl GrantScope {
    pub fn from_group(group: &str) -> Option<Self> {
        match group {
            "Editing" => Some(GrantScope::Editing),
            "Commands" => Some(GrantScope::Commands),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GrantScope::Editing => "editing",
            GrantScope::Commands => "commands",
        }
    }
}

/// One user-issued session grant: "auto-approve {scope} under {prefix} for
/// this session". In-memory only — it dies with the app, so there is no
/// standing permission to forget about; the SQLite audit keeps the record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionGrant {
    pub id: u64,
    pub scope: GrantScope,
    /// Relative path prefix under the project root, or `None` for the whole
    /// project. Commands have no path, so their grants carry `None`.
    pub path_prefix: Option<String>,
    /// Who created it ("web" | "desktop"); it only auto-approves calls from
    /// the same source, so a desktop grant never silently covers an MCP
    /// connector's calls.
    pub source: String,
}

/// A queued approval request shown to the user.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub id: u64,
    pub tool: Tool,
    pub summary: String,
    pub source: String, // web | desktop
    /// The request id that asked for this tool, captured from
    /// `process::execution_owner()` at submit time. A gated `run_command`
    /// executes on the desktop's `bridge_approve` thread, not the caller's —
    /// carrying the owner here is what lets the spawned PTY still be
    /// attributed to (and cancellable by) the original request. `None` when
    /// the call came from a caller that owns nothing (every desktop call).
    pub owner: Option<String>,
}

/// Bridge state shared via AppState.
#[derive(Default)]
pub struct Bridge {
    pub pending: Mutex<Vec<ApprovalRequest>>,
    /// Remote callers waiting for approval resolution, keyed by request id.
    pub channels: Mutex<HashMap<u64, SyncSender<ToolResult>>>,
    /// Active session grants (the Phase 6 approval-engine slice).
    pub grants: Mutex<Vec<SessionGrant>>,
    /// Kill switch: when set, every tool call is refused until unpaused.
    pub paused: std::sync::atomic::AtomicBool,
    pub next_id: AtomicU64,
}

impl Bridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Route a tool call: auto-execute, execute under a session grant, or
    /// queue for approval. Returns `(result, approval_id)` — if the call
    /// needs approval, `result.pending` is set and `approval_id` is the id
    /// to resolve. The `approved_by` string out-parameter records whether
    /// execution happened via "auto" or a "grant" (for the audit log).
    pub fn submit(
        &self,
        tool: Tool,
        source: &str,
        root: Option<&Path>,
    ) -> (ToolResult, Option<u64>) {
        let (result, approval_id, _how) = self.submit_with_audit(tool, source, root);
        (result, approval_id)
    }

    /// As [`submit`], but also reports how an auto-execution was authorized
    /// ("auto" or "grant:<scope>[:<prefix>]") for the audit log.
    ///
    /// The root-only form; [`submit_with_ctx`](Self::submit_with_ctx) is the
    /// context-aware one that the memory and agent-loop tools need.
    pub fn submit_with_audit(
        &self,
        tool: Tool,
        source: &str,
        root: Option<&Path>,
    ) -> (ToolResult, Option<u64>, String) {
        self.submit_with_ctx(tool, source, &ToolCtx::root_only(root))
    }

    /// As [`submit_with_audit`](Self::submit_with_audit), against an explicit
    /// [`ToolCtx`] — which is how a tool reaches the database or the desktop.
    pub(crate) fn submit_with_ctx(
        &self,
        tool: Tool,
        source: &str,
        ctx: &ToolCtx,
    ) -> (ToolResult, Option<u64>, String) {
        if self.paused.load(Ordering::SeqCst) {
            return (
                ToolResult::err_code(
                    ErrorCode::BridgePaused,
                    "bridge paused by user — no tool calls are running",
                ),
                None,
                "paused".to_string(),
            );
        }
        match needs_approval(&tool) {
            None => (execute_in(&tool, ctx, None), None, "auto".to_string()),
            Some(reason) => {
                if let Some(grant) = self
                    .grants
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|g| grant_matches(g, &tool, source, ctx.root))
                {
                    let label = grant_label(grant);
                    return (execute_in(&tool, ctx, None), None, label);
                }
                let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
                self.pending.lock().unwrap().push(ApprovalRequest {
                    id,
                    summary: describe_for_approval(&tool, ctx.root),
                    tool,
                    source: source.to_string(),
                    owner: crate::process::execution_owner(),
                });
                (
                    ToolResult::pending(format!("{reason} (request #{id})")),
                    Some(id),
                    "pending".to_string(),
                )
            }
        }
    }

    /// Resolve a pending approval. Executes the tool when allowed,
    /// delivers the result to any waiting remote caller, and returns the
    /// result plus the resolved request (for auditing). `on_event`
    /// receives command stream events while a `run_command` executes.
    ///
    /// The root-only form; [`resolve_with_ctx`](Self::resolve_with_ctx) is
    /// the context-aware one.
    pub fn resolve(
        &self,
        id: u64,
        allow: bool,
        root: Option<&Path>,
        on_event: Option<&mut dyn FnMut(CommandEvent)>,
    ) -> Option<(ToolResult, ApprovalRequest)> {
        self.resolve_with_ctx(id, allow, &ToolCtx::root_only(root), on_event)
    }

    /// As [`resolve`](Self::resolve), against an explicit [`ToolCtx`]. The
    /// approval card can approve a call to *any* tool, including one that
    /// needs the database, so the resolving caller must supply the same
    /// context the request came in with.
    pub(crate) fn resolve_with_ctx(
        &self,
        id: u64,
        allow: bool,
        ctx: &ToolCtx,
        on_event: Option<&mut dyn FnMut(CommandEvent)>,
    ) -> Option<(ToolResult, ApprovalRequest)> {
        let mut pending = self.pending.lock().unwrap();
        let idx = pending.iter().position(|p| p.id == id)?;
        let req = pending.remove(idx);
        drop(pending);

        let result = if allow {
            // Execute attributed to the original request (see
            // `ApprovalRequest::owner`), so a cancel for that request can
            // kill a `run_command` spawned here on the desktop's thread —
            // this thread is Tauri's command runner, which the caller has
            // long since stopped being.
            let _owner = crate::process::own_current_thread(req.owner.clone());
            execute_in(&req.tool, ctx, on_event)
        } else {
            ToolResult::err_code(
                ErrorCode::Denied,
                format!("denied by user: {}", req.summary),
            )
        };

        if let Some(tx) = self.channels.lock().unwrap().remove(&id) {
            let _ = tx.send(result.clone());
        }
        Some((result, req))
    }

    /// Remove a still-pending approval without executing it — used when the
    /// waiting caller (the MCP connector) gave up before the user decided
    /// (approval timed out). Drops the request's channel too, and returns the
    /// removed request so the caller can audit it and dismiss the card. A
    /// later [`resolve`](Self::resolve) on the same id finds nothing and so
    /// executes nothing — an Allow on the now-stale card can no longer run the
    /// tool.
    pub fn expire(&self, id: u64) -> Option<ApprovalRequest> {
        let mut pending = self.pending.lock().unwrap();
        let idx = pending.iter().position(|p| p.id == id)?;
        let req = pending.remove(idx);
        drop(pending);
        self.channels.lock().unwrap().remove(&id);
        Some(req)
    }

    /// Add a session grant; returns its id.
    pub fn grant_add(&self, scope: GrantScope, path_prefix: Option<String>, source: &str) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        self.grants.lock().unwrap().push(SessionGrant {
            id,
            scope,
            path_prefix,
            source: source.to_string(),
        });
        id
    }

    /// Revoke one grant by id.
    pub fn grant_revoke(&self, id: u64) -> bool {
        let mut grants = self.grants.lock().unwrap();
        match grants.iter().position(|g| g.id == id) {
            Some(i) => {
                grants.remove(i);
                true
            }
            None => false,
        }
    }

    /// Revoke every grant (the kill switch's first half).
    pub fn grants_clear(&self) -> usize {
        let n = self.grants.lock().unwrap().len();
        self.grants.lock().unwrap().clear();
        n
    }

    /// Set or clear the paused flag (the kill switch's second half). Pausing
    /// also revokes every grant — a kill switch that left standing
    /// auto-approvals armed would not be a kill switch.
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
        if paused {
            self.grants.lock().unwrap().clear();
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }
}

/// Human-readable summary of a tool call (for the approval UI + audit).
pub fn describe(tool: &Tool) -> String {
    let name = tool_name(tool);
    match detail(tool) {
        Some(d) => format!("{name} {d}"),
        None => name.to_string(),
    }
}

/// Paths that always require explicit approval, even for reads.
pub fn is_sensitive_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let lower = path.to_string_lossy().to_lowercase();
    if name.starts_with(".env") {
        return true;
    }
    for needle in [
        "id_rsa",
        "id_dsa",
        "credentials",
        "secret",
        "token",
        "password",
        "api_key",
        "apikey",
        ".npmrc",
        ".gitconfig",
        ".netrc",
    ] {
        if name.contains(needle) {
            return true;
        }
    }
    for ext in ["pem", "key", "pfx", "p12", "ppk", "crt"] {
        if path
            .extension()
            .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case(ext))
        {
            return true;
        }
    }
    // .git internals (config, credentials) are off-limits to reads.
    lower.contains(".git\\config") || lower.contains(".git/config")
}

/// Approval policy: `Some(reason)` when the call needs user approval.
/// Derived entirely from the tool's [`ToolSpec`] — adding a tool means
/// adding a `SPECS` row, not editing this function.
pub fn needs_approval(tool: &Tool) -> Option<String> {
    let s = spec(tool);
    match s.approval {
        Approval::Auto => None,
        Approval::SensitivePathOnly => tool_paths(tool)
            .into_iter()
            .any(|p| is_sensitive_path(Path::new(p)))
            .then(|| "read of sensitive path".to_string()),
        Approval::Always => Some(s.name.to_string()),
        Approval::Destructive => Some(format!("{} (destructive)", s.name)),
    }
}

/// Does a grant auto-approve this call? The rules, in order:
///
/// - Same source that created it (a desktop grant never covers web calls).
/// - The tool's group is the grant's scope.
/// - **Destructive never auto-approves.** Not overridable by any grant.
/// - No touched path is sensitive — otherwise `copy_file .env notes.txt`
///   launders a secret to a name the read gate doesn't stop at.
/// - Every path resolves under the grant's prefix (via `resolve_path`, not
///   a raw `starts_with`, so a `..`-laden path can't widen the scope).
pub fn grant_matches(grant: &SessionGrant, tool: &Tool, source: &str, root: Option<&Path>) -> bool {
    if grant.source != source {
        return false;
    }
    let s = spec(tool);
    if s.approval == Approval::Destructive {
        return false;
    }
    if GrantScope::from_group(s.group) != Some(grant.scope) {
        return false;
    }
    let paths = tool_paths(tool);
    if paths.iter().any(|p| is_sensitive_path(Path::new(p))) {
        return false;
    }
    match (&grant.path_prefix, root) {
        (None, _) => true,
        (Some(prefix), Some(root)) => {
            // Resolve the prefix and each path independently, then compare
            // canonical absolute paths — joining "{prefix}/{path}" first
            // would let a `src/../root.txt` path normalize its way back
            // under the prefix and widen the grant's scope.
            let Ok(base) = resolve_path(root, prefix) else {
                return false;
            };
            paths
                .iter()
                .all(|p| resolve_path(root, p).is_ok_and(|abs| abs.starts_with(&base)))
        }
        // No root to anchor a prefix against: don't guess, don't approve.
        (Some(_), None) => false,
    }
}

/// Audit label for a grant-authorized execution.
fn grant_label(grant: &SessionGrant) -> String {
    match &grant.path_prefix {
        Some(p) => format!("grant:{}:{p}", grant.scope.as_str()),
        None => format!("grant:{}", grant.scope.as_str()),
    }
}

/// What an approval card can offer as a follow-up grant, for the
/// `bridge://approval-requested` payload. `None` when the tool cannot be
/// grant-covered (destructive) or has no meaningful scope.
pub fn grantable(tool: &Tool) -> Option<(GrantScope, Option<String>)> {
    let s = spec(tool);
    if s.approval == Approval::Destructive {
        return None;
    }
    let scope = GrantScope::from_group(s.group)?;
    // The suggested prefix is the directory of the tool's first path — the
    // common "auto-approve edits under src/" shape. Commands have no path.
    let prefix = match tool_paths(tool).first() {
        Some(p) => {
            let dir = Path::new(p).parent()?;
            match dir.to_string_lossy().to_string() {
                d if d.is_empty() || d == "." => None,
                d => Some(d),
            }
        }
        None => None,
    };
    Some((scope, prefix))
}

/// The summary shown on the approval card. Destructive tools resolve their
/// paths against the project root so the card shows exactly what disappears
/// (`delete_file /home/me/proj/old.ts`), not a relative name that could be
/// any of three nested files with the same name.
pub fn describe_for_approval(tool: &Tool, root: Option<&Path>) -> String {
    let name = tool_name(tool);
    let resolve = |p: &str| -> String {
        root.and_then(|r| resolve_path(r, p).ok())
            .map(|abs| abs.display().to_string())
            .unwrap_or_else(|| p.to_string())
    };
    match tool {
        Tool::DeleteFile { path } => format!("{name} {}", resolve(path)),
        Tool::MoveFile { from, to } => {
            format!("{name} {} → {}", resolve(from), resolve(to))
        }
        // Non-destructive tools keep the terse default.
        _ => describe(tool),
    }
}

/// Ceiling on the whole file. Only one chunk is ever returned, so this is a
/// sanity limit on what we will scan, not on what the AI can read.
const READ_CAP: u64 = 16 * 1024 * 1024;

/// One chunk's budget. Bounds a single read to `CHUNK_LINES` lines and
/// `CHUNK_BYTES` of content, so one call returns a readable page rather than
/// an unbounded dump. The 24KB chat-composer cap this was originally sized to
/// fit belonged to the deleted browser extension; the only hard ceiling left
/// is the connector's `RESULT_CHAR_CAP` (140,000 chars, `mcp.rs`), which this
/// sits far inside. A single line longer than `CHUNK_BYTES` is still returned
/// whole — chunking must always make progress, and that cap is the backstop.
const CHUNK_LINES: usize = 400;
const CHUNK_BYTES: usize = 16 * 1024;

/// Greedy chunk boundaries as `(start, end)` line indexes, half-open.
///
/// A chunk takes at most `want` lines and at most `CHUNK_BYTES`, except that
/// it always takes at least one line — otherwise a single very long line
/// (minified JS, a data blob) would make no progress.
fn chunk_bounds(lines: &[&str], want: usize) -> Vec<(usize, usize)> {
    let mut bounds = Vec::new();
    let mut start = 0;
    while start < lines.len() {
        let mut end = start;
        let mut bytes = 0usize;
        while end < lines.len() && end - start < want {
            let next = lines[end].len() + 1;
            if bytes + next > CHUNK_BYTES && end > start {
                break;
            }
            bytes += next;
            end += 1;
        }
        bounds.push((start, end));
        start = end;
    }
    bounds
}

/// Render one chunk of a file as `cat -n` style numbered lines, with a footer
/// naming the exact call that returns the next chunk.
///
/// Files are paged rather than truncated. Silently cutting the tail would
/// leave the AI unable to see the rest of the file, and returning the whole
/// thing (up to `READ_CAP`) would swamp both the model's context and the
/// connector's result cap.
/// One page of a file, plus the facts needed to ask for the next one.
///
/// The point of the struct is `next_offset`. The footer used to spell paging
/// out as a call — `read_file("big.txt", 401)` — which is exactly the call
/// syntax `tool_manifest`'s doc comment forbids: a model that echoes the
/// result back turns the footer into a burst of fabricated calls. Carrying the
/// offset as data keeps the text inert and gives the model a number to pass
/// instead of a phrase to imitate.
struct Chunk {
    /// The rendered page. Never contains call syntax.
    text: String,
    /// 1-based first line shown; 0 when the file is empty.
    start_line: usize,
    /// 1-based last line shown; 0 when the file is empty.
    end_line: usize,
    total_lines: usize,
    /// The `offset` that continues from here; `None` at end of file.
    next_offset: Option<u32>,
    bytes_shown: usize,
    total_bytes: usize,
}

fn chunk_text(path: &str, text: &str, offset: Option<u32>, limit: Option<u32>) -> Chunk {
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len();
    if total == 0 {
        return Chunk {
            text: format!("[{path} is empty]\n"),
            start_line: 0,
            end_line: 0,
            total_lines: 0,
            next_offset: None,
            bytes_shown: 0,
            total_bytes: 0,
        };
    }

    // Clamped rather than rejected: the AI is guessing at file length, and a
    // hard error on a stale offset would strand it mid-file.
    let start = (offset.unwrap_or(1).max(1) as usize - 1).min(total - 1);
    let want = limit.map(|l| l.max(1) as usize).unwrap_or(CHUNK_LINES);

    let bounds = chunk_bounds(&lines, want);
    // Only meaningful when the request lands on a boundary — which it does for
    // an initial read and for any offset taken from a previous footer.
    let chunk_no = bounds
        .iter()
        .position(|(s, _)| *s == start)
        .map(|i| (i + 1, bounds.len()));
    let end = bounds
        .iter()
        .find(|(s, _)| *s == start)
        .map(|(_, e)| *e)
        .unwrap_or_else(|| {
            // Off-boundary offset: apply the same budget from where we are.
            chunk_bounds(&lines[start..], want)
                .first()
                .map(|(_, e)| start + e)
                .unwrap_or(total)
        });

    let shown: usize = lines[start..end].iter().map(|l| l.len() + 1).sum();
    let next_offset = if end < total {
        Some((end + 1) as u32)
    } else {
        None
    };

    let width = total.to_string().len().max(4);
    let mut body = String::with_capacity(CHUNK_BYTES + 256);
    for (i, line) in lines[start..end].iter().enumerate() {
        body.push_str(&format!("{:>width$}| {}\n", start + i + 1, line));
    }

    // Size of the whole file, read before the closure takes ownership of the
    // rendered page under a different name.
    let total_bytes = text.len();
    let chunk = |body: String| Chunk {
        text: body,
        start_line: start + 1,
        end_line: end,
        total_lines: total,
        next_offset,
        bytes_shown: shown,
        total_bytes,
    };

    // A file that fits in one chunk reads exactly as it did before.
    if start == 0 && end == total {
        return chunk(body);
    }

    body.push('\n');
    match chunk_no {
        Some((i, n)) => body.push_str(&format!("[chunk {i} of {n} · ")),
        None => body.push('['),
    }
    body.push_str(&format!(
        "lines {}-{} of {} · {} of {}]\n",
        start + 1,
        end,
        total,
        human_bytes(shown),
        human_bytes(text.len())
    ));
    if end < total {
        // Deliberately prose, not a call: the offset to pass is `next_offset`.
        body.push_str(&format!("[more below — continues at line {}]\n", end + 1));
    } else {
        body.push_str("[end of file]\n");
    }
    chunk(body)
}

fn human_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{:.1} KB", n as f64 / 1024.0)
    }
}

/// Lexically normalize a path: resolve `.` and `..` components without
/// touching the filesystem (symlink resolution is the existing-path
/// branch's job, via `canonicalize`). A `..` that would pop past the
/// path's own root is kept as a literal `..`, so a containment check on
/// the result still sees the escape instead of silently wrapping.
fn normalize_path(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Resolve a tool path against the project root; rejects paths escaping it.
fn resolve_path(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let candidate = root.join(rel);
    if candidate.exists() {
        // Existing target: canonicalize resolves symlinks and `..` for real.
        let canonical = candidate.canonicalize().map_err(|e| e.to_string())?;
        if !canonical.starts_with(&root) {
            return Err(format!("path escapes project root: {rel}"));
        }
        return Ok(canonical);
    }
    // The target may not exist yet (write_file to a new file). Canonicalize
    // the deepest existing ancestor, then re-append the non-existent
    // remainder — lexically normalized first: re-joining the raw `rel`
    // would leave literal `..` components in place, and the component-wise
    // starts_with check below does not resolve them, so
    // `notes/../../../.bashrc` would pass and the OS would resolve the
    // `..`s at write time, escaping the root.
    let mut parent = candidate.parent().unwrap_or(&root).to_path_buf();
    while !parent.exists() {
        if !parent.pop() {
            break;
        }
    }
    let canonical_parent = parent.canonicalize().map_err(|e| e.to_string())?;
    let remainder = candidate.strip_prefix(&parent).map_err(|e| e.to_string())?;
    let normalized = normalize_path(&canonical_parent.join(remainder));
    if !normalized.starts_with(&root) {
        return Err(format!("path escapes project root: {rel}"));
    }
    Ok(normalized)
}

/// `resolve_path` mapped to the error convention `execute` returns.
fn resolve_tool_path(root: &Path, rel: &str) -> Result<PathBuf, ToolResult> {
    resolve_path(root, rel).map_err(|e| {
        if e.contains("escapes project root") {
            ToolResult::err_code(ErrorCode::PathEscapesRoot, e)
        } else {
            ToolResult::err_code(ErrorCode::FileNotFound, e)
        }
    })
}

/// Read a file as text with the same binary/cap checks `read_file` applies.
fn read_text_file(p: &Path, rel: &str) -> Result<String, ToolResult> {
    let md = match std::fs::metadata(p) {
        Ok(md) if md.is_dir() => {
            return Err(ToolResult::err_code(
                ErrorCode::InvalidArguments,
                format!("is a directory: {rel}"),
            ));
        }
        Ok(md) => md,
        Err(e) => {
            return Err(ToolResult::err_code(
                ErrorCode::FileNotFound,
                format!("{rel}: {e}"),
            ))
        }
    };
    if md.len() > READ_CAP {
        return Err(ToolResult::err_code(
            ErrorCode::FileTooLarge,
            format!("{rel}: file too large ({} bytes)", md.len()),
        ));
    }
    match std::fs::read(p) {
        Ok(bytes) => {
            if bytes.contains(&0) {
                return Err(ToolResult::err_code(
                    ErrorCode::FileIsBinary,
                    format!("{rel}: binary file ({} bytes, not shown)", bytes.len()),
                ));
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        }
        Err(e) => Err(ToolResult::err_code(
            ErrorCode::ExecutionFailed,
            format!("{rel}: {e}"),
        )),
    }
}

/// Apply one exact-string replacement to `text`. Empty `old` is rejected
/// (it would match everywhere), a missing match is `StringNotFound`, and
/// multiple matches without `replace_all` are `AmbiguousMatch` — the count
/// is in the message so the AI can disambiguate on its next attempt.
/// What one string replacement actually did. The new text plus the evidence of
/// it — a model told only "edited" has to re-read the file to find out whether
/// its edit landed, which is the loop this exists to break.
struct EditOutcome {
    text: String,
    /// Occurrences replaced.
    replacements: usize,
    /// 1-based line of the first replacement, in the text as it was *before*.
    first_line: usize,
}

fn apply_str_edit(
    text: &str,
    old: &str,
    new: &str,
    replace_all: bool,
) -> Result<EditOutcome, ToolError> {
    if old.is_empty() {
        return Err(ToolError {
            code: ErrorCode::InvalidArguments,
            message: "old_string is empty — it would match everywhere".into(),
        });
    }
    // Line of the first hit, counted over the original text — after the
    // replacement the offsets have all shifted.
    let first_line = text
        .find(old)
        .map(|i| text[..i].lines().count() + 1)
        .unwrap_or(1);
    let matches = text.match_indices(old).count();
    match matches {
        0 => Err(ToolError {
            code: ErrorCode::StringNotFound,
            message: "old_string not found".into(),
        }),
        1 => Ok(EditOutcome {
            text: text.replacen(old, new, 1),
            replacements: 1,
            first_line,
        }),
        _ if replace_all => Ok(EditOutcome {
            text: text.replace(old, new),
            replacements: matches,
            first_line,
        }),
        _ => Err(ToolError {
            code: ErrorCode::AmbiguousMatch,
            message: format!(
                "old_string matches {matches} times — extend it to be unique, or pass replace_all"
            ),
        }),
    }
}

/// One hunk of a unified diff: the 1-based old-file start line plus the
/// lines to match (context `' '` and removed `'-'`) and the lines to put in
/// their place (context and added `'+'`).
struct Hunk {
    old_start: usize,
    match_lines: Vec<String>,  // context + removals, in order
    output_lines: Vec<String>, // context + additions, in order
}

/// Parse a single-file unified diff into hunks. `---`/`+++` headers and
/// `\ No newline at end of file` markers are ignored; the target path comes
/// from the tool argument, not the headers (which a web AI often mangles).
fn parse_hunks(patch: &str) -> Result<Vec<Hunk>, ToolError> {
    let mut hunks = Vec::new();
    let mut current: Option<Hunk> = None;
    for line in patch.lines() {
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("diff ") {
            continue;
        }
        if let Some(rest) = line.strip_prefix("@@") {
            // Close out the previous hunk.
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            // `@@ -12,3 +13,4 @@ ...` — only the old start matters; the
            // rest is advisory and models get counts wrong.
            let old = rest
                .split_whitespace()
                .next()
                .and_then(|s| s.strip_prefix('-'))
                .and_then(|s| s.split(',').next().map(str::parse::<usize>))
                .transpose()
                .map_err(|_| ToolError {
                    code: ErrorCode::InvalidArguments,
                    message: format!("bad hunk header: {line}"),
                })?
                .ok_or_else(|| ToolError {
                    code: ErrorCode::InvalidArguments,
                    message: format!("bad hunk header: {line}"),
                })?;
            current = Some(Hunk {
                old_start: old.max(1),
                match_lines: Vec::new(),
                output_lines: Vec::new(),
            });
            continue;
        }
        if line.starts_with('\\') {
            // "\ No newline at end of file" — informational; treat the
            // patch as applying to the newline-terminated form.
            continue;
        }
        let Some(hunk) = current.as_mut() else {
            // Prose around the first @@ header (explanations, code fences)
            // is common; skip it rather than rejecting the patch.
            continue;
        };
        match line.chars().next() {
            Some(' ') | Some('\t') => {
                let l = &line[1..];
                hunk.match_lines.push(l.to_string());
                hunk.output_lines.push(l.to_string());
            }
            Some('-') => hunk.match_lines.push(line[1..].to_string()),
            Some('+') => hunk.output_lines.push(line[1..].to_string()),
            // A line with no prefix — the model dropped the leading space
            // off a context line, or wrote trailing prose. Treating it as
            // context fails safe: a wrong guess makes the hunk not match
            // (PATCH_DOES_NOT_APPLY) rather than silently mis-applying.
            _ => {
                hunk.match_lines.push(line.to_string());
                hunk.output_lines.push(line.to_string());
            }
        }
    }
    if let Some(h) = current.take() {
        hunks.push(h);
    }
    if hunks.is_empty() {
        return Err(ToolError {
            code: ErrorCode::InvalidArguments,
            message: "no hunks found — expected @@ -a,b +c,d @@ sections".into(),
        });
    }
    Ok(hunks)
}

/// How far a hunk's stated position may drift before it is declared
/// unappliable (lines). `patch` and `git apply` search similarly; without
/// it, a stale offset in the model's head would reject a valid edit.
const HUNK_DRIFT: isize = 20;

/// Apply parsed hunks to the file's lines. Every hunk must apply or none
/// does — the caller only writes on `Ok`.
fn apply_hunks(lines: &[String], hunks: &[Hunk]) -> Result<Vec<String>, ToolError> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut consumed = 0usize; // lines of the old file already emitted/skipped
    for (i, hunk) in hunks.iter().enumerate() {
        let want = hunk.old_start.saturating_sub(1); // 0-based expected start
                                                     // Exact position first, then a drift search forward and backward.
        let mut found: Option<usize> = None;
        for delta in 0..=HUNK_DRIFT {
            for cand in [
                want.checked_add_signed(delta),
                delta.checked_neg().and_then(|d| want.checked_add_signed(d)),
            ] {
                let Some(c) = cand.filter(|&c| c >= consumed) else {
                    continue;
                };
                if c + hunk.match_lines.len() <= lines.len()
                    && lines[c..c + hunk.match_lines.len()] == hunk.match_lines[..]
                {
                    found = Some(c);
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
        let Some(at) = found else {
            return Err(ToolError {
                code: ErrorCode::PatchDoesNotApply,
                message: format!(
                    "hunk {} (from line {}) does not match the file",
                    i + 1,
                    hunk.old_start
                ),
            });
        };
        // Emit the untouched lines before the hunk, then its output.
        out.extend(lines[consumed..at].iter().cloned());
        out.extend(hunk.output_lines.iter().cloned());
        consumed = at + hunk.match_lines.len();
    }
    out.extend(lines[consumed..].iter().cloned());
    Ok(out)
}

/// How many files a `read_many_files` call may batch, and the batch's total
/// byte budget. The budget keeps one batch to a single readable page: a
/// rendered 400-line chunk is ~16KB of content plus ~2.5KB of line numbers and
/// a footer, so 20KB admits one full chunk with headroom for headers. The
/// figure was originally chosen to fit the browser extension's 24KB chat
/// composer cap; that cap is gone, but the sizing still holds.
const MANY_FILES_MAX: usize = 20;
const MANY_BYTES_BUDGET: usize = 20 * 1024;

/// Everything a tool may need beyond its own arguments.
///
/// Almost every tool needs only `root`. The rest are escape hatches for the
/// tools that reach further: the memory tools (SQLite), the handoff tools
/// (`build_handoff_impl`, which needs the whole `AppState`), and the
/// agent-loop tools (which must reach the desktop to ask a question or
/// raise a notification).
///
/// What a tool needs beyond its own arguments.
///
/// Today that is only the workspace root. The point of the type is the
/// seam it creates: a tool is handed its context rather than reaching for
/// a global, so the tools that will need more — the memory tools (SQLite),
/// the handoff tools (the app's state), the agent-loop tools (the desktop)
/// — extend *this* type instead of every call site. See
/// [`execute_in`], which is what such a tool is executed through.
///
/// `pub(crate)`, not `pub`: this is internal plumbing. The root-only
/// [`execute`] stays `pub` as the stable surface.
pub(crate) struct ToolCtx<'a> {
    pub(crate) root: Option<&'a Path>,
    /// The app's shared state, when the caller has it.
    ///
    /// Workspace-only tools ignore it. The memory and handoff tools have
    /// nowhere to read or write without it, and say exactly that rather than
    /// answering "no history" — which is not the same answer as "no
    /// database", and only one of them is information.
    pub(crate) state: Option<&'a AppState>,
}

impl<'a> ToolCtx<'a> {
    /// A context carrying only a workspace root — what the tests and every
    /// caller that is not the desktop uses.
    pub(crate) fn root_only(root: Option<&'a Path>) -> Self {
        Self { root, state: None }
    }

    pub(crate) fn with_state(root: Option<&'a Path>, state: &'a AppState) -> Self {
        Self {
            root,
            state: Some(state),
        }
    }
}

/// Execute a tool call locally. `root: None` → tool requires the root.
/// `on_event` streams command events while a `run_command` executes.
///
/// A thin shim over [`execute_in`] for callers that have a workspace and
/// nothing else — the desktop sandbox and most tests. Tools that need the
/// database or the desktop go through [`execute_in`] with a full [`ToolCtx`].
pub fn execute(
    tool: &Tool,
    root: Option<&Path>,
    on_event: Option<&mut dyn FnMut(CommandEvent)>,
) -> ToolResult {
    execute_in(tool, &ToolCtx::root_only(root), on_event)
}

/// Execute a tool call against an explicit context.
pub(crate) fn execute_in(
    tool: &Tool,
    ctx: &ToolCtx,
    mut on_event: Option<&mut dyn FnMut(CommandEvent)>,
) -> ToolResult {
    // Meta-tools answer from the spec table, so they work before a project
    // is opened — an AI that has lost the manifest can always recover it.
    match tool {
        Tool::ListTools => {
            let tools: Vec<&str> = SPECS.iter().map(|s| s.name).collect();
            return ToolResult::ok_structured(
                tool_manifest(),
                serde_json::json!({ "tools": tools }),
            );
        }
        Tool::DescribeTool { name } => {
            return match spec_by_name(name) {
                Some(s) => ToolResult::ok_structured(
                    describe_spec(s),
                    serde_json::json!({
                        "name": s.name,
                        "summary": s.summary,
                        "args": s.args,
                        "approval": approval_phrase(s.approval),
                    }),
                ),
                None => ToolResult::err_code(
                    ErrorCode::UnknownTool,
                    format!("unknown tool: {name}. Call list_tools for the full list."),
                ),
            };
        }
        _ => {}
    }

    // Memory tools answer from the database rather than the workspace, so
    // they run before the project-root check: a task list has to be reachable
    // with no project open, and a handoff is precisely what you want when
    // nothing is.
    match tool {
        Tool::TodoWrite { .. }
        | Tool::TodoRead
        | Tool::SetObjective { .. }
        | Tool::RememberDecision { .. }
        | Tool::RememberConstraint { .. }
        | Tool::RememberAttempt { .. }
        | Tool::GetFacts { .. }
        | Tool::ListSessions { .. }
        | Tool::RequestHandoff { .. }
        | Tool::GetHandoff => return run_memory_tool(tool, ctx),
        _ => {}
    }

    // Phases 7–10. Dispatched together because each one carries its own root
    // and state handling: the web tools need no workspace, the LSP and
    // isolation tools need the app state for a cached server or the active
    // worktree, and the agent-loop tools need the desktop. Folding them into
    // one entry point keeps that decision in one place instead of spreading
    // it across the big match below.
    match tool {
        Tool::WebFetch { .. }
        | Tool::WebSearch { .. }
        | Tool::NotebookRead { .. }
        | Tool::NotebookEdit { .. }
        | Tool::DelegateTask { .. }
        | Tool::LspDiagnostics { .. }
        | Tool::LspDefinition { .. }
        | Tool::LspReferences { .. }
        | Tool::LspSymbols { .. }
        | Tool::AskUser { .. }
        | Tool::ProposePlan { .. }
        | Tool::Monitor { .. }
        | Tool::Notify { .. }
        | Tool::EnterWorktree { .. }
        | Tool::ExitWorktree { .. }
        | Tool::ReadMedia { .. }
        | Tool::PublishArtifact { .. }
        | Tool::ReportFindings { .. } => return run_phase_tool(tool, ctx),
        _ => {}
    }

    // Inside a worktree the session's root is the worktree, not the project
    // the user opened: every path-resolving tool below lands there. The
    // override lives in state so `enter_worktree` and `exit_worktree` only
    // have to set and clear one value.
    let active = ctx
        .state
        .and_then(|s| s.active_worktree.lock().ok().and_then(|g| g.clone()));
    let root = active.as_deref().or(ctx.root);
    let Some(root) = root else {
        return ToolResult::err_code(ErrorCode::InternalError, "project root not set");
    };

    match tool {
        Tool::ReadFile {
            path,
            offset,
            limit,
        } => {
            let p = match resolve_path(root, path) {
                Ok(p) => p,
                Err(e) => {
                    if e.contains("escapes project root") {
                        return ToolResult::err_code(ErrorCode::PathEscapesRoot, e);
                    }
                    return ToolResult::err_code(ErrorCode::FileNotFound, e);
                }
            };
            let md = match std::fs::metadata(&p) {
                Ok(md) if md.is_dir() => {
                    return ToolResult::err_code(
                        ErrorCode::InvalidArguments,
                        format!("is a directory: {path}"),
                    );
                }
                Ok(md) => md,
                Err(e) => {
                    return ToolResult::err_code(ErrorCode::FileNotFound, format!("{path}: {e}"));
                }
            };
            // Checked before reading, so a huge file is never loaded at all.
            if md.len() > READ_CAP {
                return ToolResult::err_code(
                    ErrorCode::FileTooLarge,
                    format!("{path}: file too large ({} bytes)", md.len()),
                );
            }
            match std::fs::read(&p) {
                Ok(bytes) => {
                    if bytes.contains(&0) {
                        return ToolResult::err_code(
                            ErrorCode::FileIsBinary,
                            format!("{}: binary file ({} bytes, not shown)", path, bytes.len()),
                        );
                    }
                    let text = String::from_utf8_lossy(&bytes);
                    let chunk = chunk_text(path, &text, *offset, *limit);
                    let structured = serde_json::json!({
                        "path": path,
                        "start_line": chunk.start_line,
                        "end_line": chunk.end_line,
                        "total_lines": chunk.total_lines,
                        "bytes_shown": chunk.bytes_shown,
                        "total_bytes": chunk.total_bytes,
                        "truncated": chunk.next_offset.is_some(),
                        "next_offset": chunk.next_offset,
                    });
                    ToolResult::ok_structured(chunk.text, structured)
                }
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
            }
        }
        Tool::WriteFile { path, content } => {
            let p = match resolve_path(root, path) {
                Ok(p) => p,
                Err(e) => {
                    if e.contains("escapes project root") {
                        return ToolResult::err_code(ErrorCode::PathEscapesRoot, e);
                    }
                    return ToolResult::err_code(ErrorCode::FileNotFound, e);
                }
            };
            if let Some(parent) = p.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult::err_code(
                        ErrorCode::ExecutionFailed,
                        format!("{path}: {e}"),
                    );
                }
            }
            match std::fs::write(&p, content.as_bytes()) {
                Ok(()) => {
                    let structured = serde_json::json!({
                        "path": path,
                        "bytes_written": content.len(),
                    });
                    ToolResult::ok_structured(
                        format!("wrote {} bytes to {path}", content.len()),
                        structured,
                    )
                }
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
            }
        }
        Tool::EditFile {
            path,
            old_string,
            new_string,
            replace_all,
        } => {
            let p = match resolve_tool_path(root, path) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let text = match read_text_file(&p, path) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let bytes_before = text.len();
            match apply_str_edit(&text, old_string, new_string, replace_all.unwrap_or(false)) {
                Ok(outcome) => match std::fs::write(&p, outcome.text.as_bytes()) {
                    Ok(()) => {
                        // The evidence, not just the fact. "edited x.ts" left a
                        // model no way to tell a landed edit from a silent
                        // no-op, so it re-read the file to check.
                        let structured = serde_json::json!({
                            "path": path,
                            "replacements": outcome.replacements,
                            "first_line": outcome.first_line,
                            "bytes_before": bytes_before,
                            "bytes_after": outcome.text.len(),
                        });
                        ToolResult::ok_structured(
                            format!(
                                "edited {path} — {} replacement{} at line {}",
                                outcome.replacements,
                                if outcome.replacements == 1 { "" } else { "s" },
                                outcome.first_line
                            ),
                            structured,
                        )
                    }
                    Err(e) => {
                        ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}"))
                    }
                },
                Err(e) => ToolResult::err_code(e.code, format!("{path}: {}", e.message)),
            }
        }
        Tool::MultiEdit { path, edits } => {
            if edits.is_empty() {
                return ToolResult::err_code(
                    ErrorCode::InvalidArguments,
                    "edits is empty — nothing to apply",
                );
            }
            let p = match resolve_tool_path(root, path) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let text = match read_text_file(&p, path) {
                Ok(t) => t,
                Err(r) => return r,
            };
            // Fold every edit through the in-memory text before touching
            // the file: one bad edit leaves the file exactly as it was.
            let mut new_text = text;
            let mut replacements = 0usize;
            // Line of each edit's first hit, as the file stood when that edit
            // ran — earlier edits in the batch have already shifted the text.
            let mut lines: Vec<usize> = Vec::with_capacity(edits.len());
            for (i, e) in edits.iter().enumerate() {
                match apply_str_edit(
                    &new_text,
                    &e.old_string,
                    &e.new_string,
                    e.replace_all.unwrap_or(false),
                ) {
                    Ok(outcome) => {
                        replacements += outcome.replacements;
                        lines.push(outcome.first_line);
                        new_text = outcome.text;
                    }
                    Err(err) => {
                        return ToolResult::err_code(
                            err.code,
                            format!("{path}: edit {} of {}: {}", i + 1, edits.len(), err.message),
                        );
                    }
                }
            }
            match std::fs::write(&p, new_text.as_bytes()) {
                Ok(()) => {
                    let structured = serde_json::json!({
                        "path": path,
                        "edits_applied": edits.len(),
                        "replacements": replacements,
                        "lines": lines,
                    });
                    ToolResult::ok_structured(
                        format!(
                            "applied {} edits to {path} — {replacements} replacement{}",
                            edits.len(),
                            if replacements == 1 { "" } else { "s" }
                        ),
                        structured,
                    )
                }
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
            }
        }
        Tool::ApplyPatch { path, patch } => {
            let p = match resolve_tool_path(root, path) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let text = match read_text_file(&p, path) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let hunks = match parse_hunks(patch) {
                Ok(h) => h,
                Err(e) => return ToolResult::err_code(e.code, format!("{path}: {}", e.message)),
            };
            let lines: Vec<String> = text.lines().map(str::to_string).collect();
            let new_lines = match apply_hunks(&lines, &hunks) {
                Ok(l) => l,
                Err(e) => return ToolResult::err_code(e.code, format!("{path}: {}", e.message)),
            };
            let mut new_text = new_lines.join("\n");
            if text.ends_with('\n') {
                new_text.push('\n');
            }
            match std::fs::write(&p, new_text.as_bytes()) {
                Ok(()) => {
                    // Hunks carry the line they were built against in the file
                    // the patch came from; applying one shifts the rest, so
                    // these are anchors, not a final address.
                    let hunk_lines: Vec<usize> = hunks.iter().map(|h| h.old_start).collect();
                    let structured = serde_json::json!({
                        "path": path,
                        "hunks_applied": hunks.len(),
                        "lines": hunk_lines,
                    });
                    ToolResult::ok_structured(
                        format!("applied {} hunks to {path}", hunks.len()),
                        structured,
                    )
                }
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
            }
        }
        Tool::DeleteFile { path } => {
            let p = match resolve_tool_path(root, path) {
                Ok(p) => p,
                Err(r) => return r,
            };
            match std::fs::metadata(&p) {
                Ok(md) if md.is_dir() => {
                    return ToolResult::err_code(
                        ErrorCode::InvalidArguments,
                        format!("is a directory: {path} — this tool deletes files only"),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    return ToolResult::err_code(ErrorCode::FileNotFound, format!("{path}: {e}"));
                }
            }
            match std::fs::remove_file(&p) {
                Ok(()) => ToolResult::ok_structured(
                    format!("deleted {path}"),
                    serde_json::json!({ "path": path }),
                ),
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
            }
        }
        Tool::MoveFile { from, to } => {
            let src = match resolve_tool_path(root, from) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let dst = match resolve_tool_path(root, to) {
                Ok(p) => p,
                Err(r) => return r,
            };
            if !src.exists() {
                return ToolResult::err_code(ErrorCode::FileNotFound, format!("{from}: not found"));
            }
            if let Some(parent) = dst.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{to}: {e}"));
                }
            }
            // Overwrites the target — the approval card shows both paths.
            match std::fs::rename(&src, &dst) {
                Ok(()) => ToolResult::ok_structured(
                    format!("moved {from} → {to}"),
                    serde_json::json!({ "from": from, "to": to }),
                ),
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{from}: {e}")),
            }
        }
        Tool::CopyFile { from, to } => {
            let src = match resolve_tool_path(root, from) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let dst = match resolve_tool_path(root, to) {
                Ok(p) => p,
                Err(r) => return r,
            };
            if !src.exists() {
                return ToolResult::err_code(ErrorCode::FileNotFound, format!("{from}: not found"));
            }
            if let Some(parent) = dst.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    return ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{to}: {e}"));
                }
            }
            match std::fs::copy(&src, &dst) {
                Ok(n) => ToolResult::ok_structured(
                    format!("copied {from} → {to} ({n} bytes)"),
                    serde_json::json!({ "from": from, "to": to, "bytes": n }),
                ),
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{from}: {e}")),
            }
        }
        Tool::CreateDirectory { path } => {
            let p = match resolve_tool_path(root, path) {
                Ok(p) => p,
                Err(r) => return r,
            };
            match std::fs::create_dir_all(&p) {
                Ok(()) => ToolResult::ok_structured(
                    format!("created directory {path}"),
                    serde_json::json!({ "path": path }),
                ),
                Err(e) => ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
            }
        }
        Tool::ReadManyFiles { paths } => {
            if paths.is_empty() {
                return ToolResult::err_code(ErrorCode::InvalidArguments, "paths is empty");
            }
            if paths.len() > MANY_FILES_MAX {
                return ToolResult::err_code(
                    ErrorCode::InvalidArguments,
                    format!("{} paths — batch at most {MANY_FILES_MAX}", paths.len()),
                );
            }
            let mut out = String::new();
            let mut budget = MANY_BYTES_BUDGET;
            let mut shown = 0usize;
            // One row per requested path, in request order, so a caller can
            // tell "you skipped my .env" apart from "the file was empty"
            // without reading the prose.
            let mut files: Vec<serde_json::Value> = Vec::with_capacity(paths.len());
            for path in paths {
                // Sensitive paths are skipped, not refused: one .env in a
                // batch of package files shouldn't block the rest.
                if is_sensitive_path(Path::new(path)) {
                    out.push_str(&format!("[{path} — skipped: sensitive]\n"));
                    files.push(serde_json::json!({
                        "path": path, "status": "skipped", "reason": "sensitive path"
                    }));
                    continue;
                }
                let p = match resolve_tool_path(root, path) {
                    Ok(p) => p,
                    Err(r) => {
                        let msg = r.error.unwrap_or_default();
                        out.push_str(&format!("[{path} — {msg}]\n"));
                        files.push(serde_json::json!({
                            "path": path, "status": "error", "reason": msg
                        }));
                        continue;
                    }
                };
                let text = match read_text_file(&p, path) {
                    Ok(t) => t,
                    Err(r) => {
                        let msg = r.error.unwrap_or_default();
                        out.push_str(&format!("[{path} — {msg}]\n"));
                        files.push(serde_json::json!({
                            "path": path, "status": "error", "reason": msg
                        }));
                        continue;
                    }
                };
                if budget == 0 {
                    out.push_str(&format!(
                        "[{path} — batch budget spent; read it separately]\n"
                    ));
                    files.push(serde_json::json!({
                        "path": path, "status": "too_large", "reason": "batch budget spent"
                    }));
                    continue;
                }
                let chunk = chunk_text(path, &text, None, None);
                if chunk.text.len() > budget {
                    let lines = text.lines().count();
                    out.push_str(&format!(
                        "[{path} — {lines} lines, too large for this batch; read it separately]\n"
                    ));
                    files.push(serde_json::json!({
                        "path": path, "status": "too_large", "lines": lines,
                        "reason": "over this batch's byte budget"
                    }));
                    continue;
                }
                out.push_str(&format!("── {path} ──\n"));
                out.push_str(&chunk.text);
                budget -= chunk.text.len();
                shown += 1;
                files.push(serde_json::json!({
                    "path": path, "status": "shown", "lines": chunk.total_lines
                }));
            }
            out.push_str(&format!("\n[{shown} of {} files shown]\n", paths.len()));
            let structured = serde_json::json!({
                "requested": paths.len(),
                "shown": shown,
                "files": files,
            });
            ToolResult::ok_structured(out, structured)
        }
        Tool::RunCommand { command } => {
            if let Some(cb) = on_event.as_mut() {
                cb(CommandEvent::Start {
                    command: command.clone(),
                });
            }
            let out = {
                let mut forward = |chunk: String| {
                    if let Some(cb) = on_event.as_mut() {
                        cb(CommandEvent::Output { data: chunk });
                    }
                };
                // Register the PTY child so a `cancel` for the owning
                // request can kill the whole process group mid-run — the
                // registry owner is the request id around execution
                // (calls with no owner register with none).
                let mut reg_id = None;
                let mut on_spawn = |pid: u32| {
                    reg_id = Some(crate::process::registry().register(
                        pid,
                        crate::process::ProcessKind::Command,
                        command.clone(),
                        crate::process::execution_owner(),
                    ));
                };
                let out = pty::run_command_stream(
                    Shell::detect(),
                    command,
                    root,
                    Duration::from_secs(120),
                    1_048_576,
                    &mut forward,
                    Some(&mut on_spawn),
                );
                // Normal exit: the pid is gone; drop it from the registry
                // so it cannot be signalled by a late cancel.
                if let Some(id) = reg_id {
                    crate::process::registry().unregister(id);
                }
                out
            };
            let out = match out {
                Ok(o) => o,
                Err(e) => {
                    if let Some(cb) = on_event.as_mut() {
                        cb(CommandEvent::Exit {
                            code: None,
                            timed_out: false,
                            truncated: false,
                        });
                    }
                    return ToolResult::err_code(
                        ErrorCode::ExecutionFailed,
                        format!("run_command failed: {e}"),
                    );
                }
            };
            if let Some(cb) = on_event.as_mut() {
                cb(CommandEvent::Exit {
                    code: out.exit_code,
                    timed_out: out.timed_out,
                    truncated: out.truncated,
                });
            }
            let mut text = out.output;
            if out.timed_out {
                text.push_str("\n[timed out — process killed]");
                return ToolResult::err_code(ErrorCode::CommandTimeout, text);
            }
            if out.truncated {
                text.push_str("\n[output truncated]");
            }
            text.push_str(&format!("\n[exit code: {}]", out.exit_code.unwrap_or(-1)));
            let structured = serde_json::json!({
                "command": command,
                "exit_code": out.exit_code,
                "timed_out": out.timed_out,
                "truncated": out.truncated,
            });
            ToolResult::ok_structured(text, structured)
        }
        // The three background-command tools carry their own dispatch: they
        // need the manager from the app state as well as this root, and the
        // gate that names which half is missing belongs in one place.
        Tool::RunCommandBackground { .. }
        | Tool::CommandOutput { .. }
        | Tool::KillCommand { .. } => run_bg_tool(tool, ctx),
        Tool::ListDirectory { path } => {
            let p = match resolve_path(root, path) {
                Ok(p) => p,
                Err(e) => {
                    if e.contains("escapes project root") {
                        return ToolResult::err_code(ErrorCode::PathEscapesRoot, e);
                    }
                    return ToolResult::err_code(ErrorCode::FileNotFound, e);
                }
            };
            let mut entries: Vec<(String, &'static str, Option<u64>)> = Vec::new();
            match std::fs::read_dir(&p) {
                Ok(read) => {
                    for entry in read.flatten() {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        // `file_type` comes off the directory read itself; only
                        // files pay for a stat, and only to report a size.
                        let kind = match entry.file_type() {
                            Ok(t) if t.is_dir() => "dir",
                            Ok(t) if t.is_file() => "file",
                            _ => "other",
                        };
                        let size = if kind == "file" {
                            entry.metadata().ok().map(|m| m.len())
                        } else {
                            None
                        };
                        entries.push((name, kind, size));
                    }
                }
                Err(e) => {
                    return ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}"))
                }
            }
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let names: Vec<&str> = entries.iter().map(|(n, _, _)| n.as_str()).collect();
            let mut out = format!("[{} entries]\n", names.len());
            out.push_str(&names.join("\n"));

            // The text stays the bare name list it always was; which entries
            // are directories, and how big the files are, is what the model
            // would otherwise have to guess or spend a call finding out.
            let listing: Vec<serde_json::Value> = entries
                .iter()
                .map(|(name, kind, size)| {
                    serde_json::json!({ "name": name, "kind": kind, "size": size })
                })
                .collect();
            let structured = serde_json::json!({
                "path": path,
                "count": names.len(),
                "entries": listing,
            });
            ToolResult::ok_structured(out, structured)
        }
        Tool::Grep {
            pattern,
            path,
            include,
            exclude,
            mode,
            max_results,
        } => {
            // The plan is built before the tree is touched, so a bad regex
            // costs a compile rather than a walk.
            let plan = match plan_grep(
                pattern,
                include.as_deref(),
                exclude.as_deref(),
                *mode,
                *max_results,
            ) {
                Ok(p) => p,
                Err(r) => return r,
            };
            let target = match SearchTarget::resolve(root, path.as_deref()) {
                Ok(t) => t,
                Err(r) => return r,
            };
            run_grep(&plan, &target, root)
        }
        Tool::Glob {
            pattern,
            path,
            max_results,
        } => {
            let pattern = match PathGlob::new(pattern, "pattern") {
                Ok(g) => g,
                Err(r) => return r,
            };
            let target = match SearchTarget::resolve(root, path.as_deref()) {
                Ok(t) => t,
                Err(r) => return r,
            };
            let max = max_results
                .map(|n| (n.max(1) as usize).min(GLOB_MAX_RESULTS))
                .unwrap_or(GLOB_MAX_RESULTS);
            run_glob(&pattern, &target, root, max)
        }
        Tool::GitStatus => {
            let repo = match git::open_repo(root) {
                Ok(r) => r,
                Err(e) => {
                    return ToolResult::err_code(
                        ErrorCode::ExecutionFailed,
                        format!("not a git repo: {e}"),
                    );
                }
            };
            let statuses = match git::status(&repo) {
                Ok(s) => s,
                Err(e) => {
                    return ToolResult::err_code(
                        ErrorCode::ExecutionFailed,
                        format!("git status: {e}"),
                    );
                }
            };
            if statuses.is_empty() {
                return ToolResult::ok_structured(
                    "working tree clean".to_string(),
                    serde_json::json!({ "clean": true, "count": 0, "files": [] }),
                );
            }
            let mut out = format!("[{} changed files]\n", statuses.len());
            for s in &statuses {
                out.push_str(&format!(
                    "{} [{} +{}/-{}]\n",
                    s.path, s.status, s.additions, s.deletions
                ));
            }
            let files: Vec<serde_json::Value> = statuses
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "path": s.path,
                        "status": s.status,
                        "additions": s.additions,
                        "deletions": s.deletions,
                    })
                })
                .collect();
            let structured = serde_json::json!({
                "clean": false,
                "count": statuses.len(),
                "files": files,
            });
            ToolResult::ok_structured(out, structured)
        }
        Tool::GitDiff { path } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            let diffs = match git::diff_workdir(&repo) {
                Ok(d) => d,
                Err(e) => return git_failed("git diff", e),
            };
            let selected: Vec<&git::FileDiff> = match path {
                Some(want) => diffs.iter().filter(|d| &d.path == want).collect(),
                None => diffs.iter().collect(),
            };
            if selected.is_empty() {
                let said = match path {
                    Some(want) => format!("no uncommitted changes to {want}\n"),
                    None => "working tree clean\n".to_string(),
                };
                return ToolResult::ok_structured(
                    said,
                    serde_json::json!({ "count": 0, "truncated": false, "files": [] }),
                );
            }
            // One budget for the whole result, not one per file: the caller
            // gets as much patch as fits and an honest count of what did not.
            let mut budget = GIT_PATCH_BUDGET;
            let mut omitted = 0usize;
            let mut out = format!("[{} changed files]\n", selected.len());
            let mut files = Vec::with_capacity(selected.len());
            for d in &selected {
                out.push_str(&format!(
                    "── {} [{} +{}/-{}]\n",
                    d.path, d.status, d.added, d.deleted
                ));
                let (patch, skipped) = fit_patch(&d.patch, &mut budget);
                if skipped {
                    omitted += 1;
                    out.push_str("[patch omitted: does not fit the result budget]\n");
                } else {
                    out.push_str(&patch);
                    if !patch.ends_with('\n') {
                        out.push('\n');
                    }
                }
                files.push(serde_json::json!({
                    "path": d.path,
                    "status": d.status,
                    "added": d.added,
                    "deleted": d.deleted,
                    "patch": patch,
                    "patch_omitted": skipped,
                }));
            }
            if omitted > 0 {
                out.push_str(&format!("\n[{omitted} patch(es) omitted]\n"));
            }
            ToolResult::ok_structured(
                out,
                serde_json::json!({
                    "count": files.len(),
                    "truncated": omitted > 0,
                    "files": files,
                }),
            )
        }
        Tool::GitLog { limit } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            let limit = limit
                .map(|n| (n.max(1) as usize).min(GIT_LOG_MAX))
                .unwrap_or(20);
            // An unborn HEAD is not a failure: a fresh repository has no
            // commits, and saying so is the whole answer. `log` reports it as
            // an error because git2 has no other way to say it.
            if repo.head().is_err() {
                return ToolResult::ok_structured(
                    "no commits yet\n".to_string(),
                    serde_json::json!({ "count": 0, "commits": [] }),
                );
            }
            let commits = match git::log(&repo, limit) {
                Ok(c) => c,
                Err(e) => return git_failed("git log", e),
            };
            let mut out = String::new();
            for c in &commits {
                // `message` may be multi-line; the subject is what a log line
                // shows, and the full body is available through git_show.
                let subject = c.message.lines().next().unwrap_or("").trim();
                out.push_str(&format!(
                    "{} {} ({})\n",
                    &c.oid[..7.min(c.oid.len())],
                    subject,
                    c.author
                ));
            }
            let rows: Vec<serde_json::Value> = commits
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "oid": c.oid,
                        "summary": c.message.lines().next().unwrap_or("").trim(),
                        "author": c.author,
                        "timestamp": c.timestamp,
                    })
                })
                .collect();
            ToolResult::ok_structured(
                out,
                serde_json::json!({ "count": rows.len(), "commits": rows }),
            )
        }
        Tool::GitBranches => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            let branches = match git::branches(&repo) {
                Ok(b) => b,
                Err(e) => return git_failed("git branch", e),
            };
            let current = git::current_branch(&repo);
            let mut out = String::new();
            for b in &branches {
                out.push_str(if b.is_current { "* " } else { "  " });
                out.push_str(&b.name);
                out.push('\n');
            }
            let rows: Vec<serde_json::Value> = branches
                .iter()
                .map(|b| serde_json::json!({ "name": b.name, "is_current": b.is_current }))
                .collect();
            ToolResult::ok_structured(
                out,
                serde_json::json!({
                    "current": current,
                    "count": rows.len(),
                    "branches": rows,
                }),
            )
        }
        Tool::GitAdd { path } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            // The path reaches the index, not the disk, so containment has to
            // be checked here rather than by the walk that a search would do.
            if let Some(p) = path {
                if let Err(e) = resolve_path(root, p) {
                    if e.contains("escapes project root") {
                        return ToolResult::err_code(ErrorCode::PathEscapesRoot, e);
                    }
                    return ToolResult::err_code(ErrorCode::InvalidArguments, e);
                }
            }
            let staged = match path {
                Some(p) => git::stage(&repo, p).map(|()| p.clone()),
                None => git::stage_all(&repo).map(|()| "*".to_string()),
            };
            match staged {
                Ok(which) => {
                    let scoped = path.clone();
                    ToolResult::ok_structured(
                        format!("staged {which}\n"),
                        serde_json::json!({ "scoped_to": scoped }),
                    )
                }
                Err(e) => git_failed("git add", e),
            }
        }
        Tool::GitUnstage { path } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            if let Err(e) = resolve_path(root, path) {
                if e.contains("escapes project root") {
                    return ToolResult::err_code(ErrorCode::PathEscapesRoot, e);
                }
                return ToolResult::err_code(ErrorCode::InvalidArguments, e);
            }
            // "Nothing to unstage" is a routine answer, not a failure to
            // apologise for — but it is an answer that has to *say* so rather
            // than report success for a call that changed nothing.
            match git::has_staged_change(&repo, path) {
                Ok(true) => {}
                Ok(false) => {
                    return ToolResult::err_code(
                        ErrorCode::InvalidArguments,
                        format!("nothing to unstage: {path} has no staged change"),
                    );
                }
                Err(e) => return git_failed("git status", e),
            }
            match git::unstage(&repo, path) {
                Ok(()) => ToolResult::ok_structured(
                    format!("unstaged {path}\n"),
                    serde_json::json!({ "scoped_to": path }),
                ),
                Err(e) => git_failed("git unstage", e),
            }
        }
        Tool::GitCommit { message } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            if message.trim().is_empty() {
                return ToolResult::err_code(
                    ErrorCode::InvalidArguments,
                    "commit message is empty",
                );
            }
            // Committing with nothing staged writes an empty commit — legal in
            // git, essentially never what was meant, and confusing to undo.
            match git::has_staged_changes(&repo) {
                Ok(true) => {}
                Ok(false) => {
                    return ToolResult::err_code(
                        ErrorCode::InvalidArguments,
                        "nothing staged to commit — call git_add first",
                    );
                }
                Err(e) => return git_failed("git status", e),
            }
            match git::commit(&repo, message) {
                Ok(oid) => {
                    let summary = message.lines().next().unwrap_or("").trim().to_string();
                    ToolResult::ok_structured(
                        format!("committed {} {summary}\n", oid),
                        serde_json::json!({ "oid": oid.to_string(), "summary": summary }),
                    )
                }
                Err(e) => git_failed("git commit", e),
            }
        }
        Tool::GitCheckout { name } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            // Refuse before anything moves. `git::checkout` refuses too, but
            // it can only report *a* dirty path in its message; going through
            // `dirty_paths` here lets the code be `WorktreeDirty` and the
            // message name several, so the caller can see the shape of the
            // problem rather than one line of it.
            match git::dirty_paths(&repo) {
                Ok(dirty) if !dirty.is_empty() => return worktree_dirty(name, &dirty),
                Ok(_) => {}
                Err(e) => return git_failed("git status", e),
            }
            match git::checkout(&repo, name) {
                Ok(()) => ToolResult::ok_structured(
                    format!("switched to {name}\n"),
                    serde_json::json!({
                        "branch": name,
                        "created": false,
                        "checked_out": true,
                    }),
                ),
                Err(e) => git_failed("git checkout", e),
            }
        }
        Tool::GitCreateBranch {
            name,
            base,
            checkout,
        } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            let checkout = checkout.unwrap_or(false);
            match git::create_branch(&repo, name, base.as_deref(), checkout) {
                Ok(()) => ToolResult::ok_structured(
                    format!(
                        "created {name}{}\n",
                        match base {
                            Some(b) => format!(" from {b}"),
                            None => " at HEAD".to_string(),
                        }
                    ),
                    serde_json::json!({
                        "branch": name,
                        "created": true,
                        "checked_out": checkout,
                    }),
                ),
                Err(e) => git_failed("git branch", e),
            }
        }
        Tool::GitShow { oid } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            match git::show(&repo, oid) {
                Ok(c) => {
                    let mut budget = GIT_PATCH_BUDGET;
                    let (patch, omitted) = fit_patch(&c.patch, &mut budget);
                    let mut out = format!(
                        "commit {}\nAuthor: {} <{}>\n\n{}\n",
                        c.oid,
                        c.author,
                        c.email.as_deref().unwrap_or(""),
                        c.message.trim_end(),
                    );
                    if omitted {
                        out.push_str("\n[patch omitted: does not fit the result budget]\n");
                    } else {
                        out.push_str(&patch);
                    }
                    ToolResult::ok_structured(
                        out,
                        serde_json::json!({
                            "oid": c.oid,
                            "summary": c.summary,
                            "author": c.author,
                            "email": c.email,
                            "timestamp": c.timestamp,
                            "patch": patch,
                            "patch_omitted": omitted,
                        }),
                    )
                }
                Err(e) => git_failed("git show", e),
            }
        }
        Tool::GitCommitDiff { oid } => {
            let repo = match open_workspace_repo(root) {
                Ok(r) => r,
                Err(r) => return r,
            };
            match git::commit_diff(&repo, oid) {
                Ok(patch) => {
                    let mut budget = GIT_PATCH_BUDGET;
                    let (patch, omitted) = fit_patch(&patch, &mut budget);
                    let out = if omitted {
                        format!("[{oid}: patch omitted, does not fit the result budget]\n")
                    } else {
                        patch.clone()
                    };
                    ToolResult::ok_structured(
                        out,
                        serde_json::json!({
                            "oid": oid,
                            "patch": patch,
                            "patch_omitted": omitted,
                        }),
                    )
                }
                Err(e) => git_failed("git show", e),
            }
        }
        // Handled above, before the project-root check.
        Tool::DescribeTool { .. }
        | Tool::ListTools
        | Tool::TodoWrite { .. }
        | Tool::TodoRead
        | Tool::SetObjective { .. }
        | Tool::RememberDecision { .. }
        | Tool::RememberConstraint { .. }
        | Tool::RememberAttempt { .. }
        | Tool::GetFacts { .. }
        | Tool::ListSessions { .. }
        | Tool::RequestHandoff { .. }
        | Tool::GetHandoff
        | Tool::WebFetch { .. }
        | Tool::WebSearch { .. }
        | Tool::NotebookRead { .. }
        | Tool::NotebookEdit { .. }
        | Tool::DelegateTask { .. }
        | Tool::LspDiagnostics { .. }
        | Tool::LspDefinition { .. }
        | Tool::LspReferences { .. }
        | Tool::LspSymbols { .. }
        | Tool::AskUser { .. }
        | Tool::ProposePlan { .. }
        | Tool::Monitor { .. }
        | Tool::Notify { .. }
        | Tool::EnterWorktree { .. }
        | Tool::ExitWorktree { .. }
        | Tool::ReadMedia { .. }
        | Tool::PublishArtifact { .. }
        | Tool::ReportFindings { .. } => unreachable!(),
    }
}

// --- memory ----------------------------------------------------------------
//
// Ten tools over the tables the fact extractor already writes. What this layer
// adds is the *pull* direction: until now the only way knowledge entered the
// project memory was `facts::extract` reading a Claude Code transcript, so a
// web AI could read the developer's conclusions but never record its own.
//
// Two decisions shape everything below:
//
// * **A session of its own.** The fact tables are keyed by session, and the
//   archive's sessions belong to Claude Code transcripts. Filing a connector's
//   decisions under one of those would attribute them to the wrong agent, so
//   the connector gets one row of its own, found or created.
// * **"No answer" is not "no database".** A tool that cannot reach the
//   database says so, rather than reporting an empty memory — the first is a
//   bug in the caller, the second is information, and confusing them is how a
//   model concludes the project has no constraints and proceeds to violate
//   them.

/// Ceiling for `list_sessions`. A caller asking for every session ever has
/// asked for a result it cannot use.
const SESSION_LIST_MAX: usize = 100;

/// The database behind a tool call, or a coded error naming what is missing.
fn db_guard<'a>(
    ctx: &ToolCtx<'a>,
) -> Result<std::sync::MutexGuard<'a, rusqlite::Connection>, ToolResult> {
    let state = ctx.state.ok_or_else(|| {
        ToolResult::err_code(
            ErrorCode::InternalError,
            "this tool reads the project memory, which lives in the desktop \
             database, and the caller did not supply it",
        )
    })?;
    state.conn.lock().map_err(|_| {
        ToolResult::err_code(ErrorCode::InternalError, "the database lock is poisoned")
    })
}

fn db_failed(what: &str, e: rusqlite::Error) -> ToolResult {
    ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{what}: {e}"))
}

/// The session the connector records into. Created on first use, so a fresh
/// install has somewhere to file the first decision.
fn connector_session(conn: &rusqlite::Connection) -> Result<i64, ToolResult> {
    db::connector_session_id(conn).map_err(|e| db_failed("could not open the connector session", e))
}

/// The session a memory tool should act on.
///
/// A caller may name one — `list_sessions` is how it learns the ids — but the
/// default is the connector's own row.
fn resolve_session(conn: &rusqlite::Connection, requested: Option<i64>) -> Result<i64, ToolResult> {
    let Some(id) = requested else {
        return connector_session(conn);
    };
    let known: i64 = conn
        .query_row("SELECT COUNT(*) FROM sessions WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .map_err(|e| db_failed("could not look up the session", e))?;
    if known == 0 {
        return Err(ToolResult::err_code(
            ErrorCode::FileNotFound,
            format!("no session {id} — call list_sessions for the ids that exist"),
        ));
    }
    Ok(id)
}

/// Validate a task list, refusing the whole list on one bad entry.
///
/// The alternative — clamping an unknown status to `pending` — would store a
/// list the caller did not send and then hand it back as truth. A list is one
/// value, so one bad item invalidates it (ch. 12: fail-fast for dependent
/// data).
fn normalise_todos(items: &[TodoItem]) -> Result<Vec<db::Todo>, ToolResult> {
    const STATUSES: &[&str] = &["pending", "in_progress", "completed"];
    let mut out = Vec::with_capacity(items.len());
    for (i, t) in items.iter().enumerate() {
        let content = t.content.trim();
        if content.is_empty() {
            return Err(ToolResult::err_code(
                ErrorCode::InvalidArguments,
                format!("todo {} has empty content", i + 1),
            ));
        }
        let status = t.status.as_deref().unwrap_or("pending");
        if !STATUSES.contains(&status) {
            return Err(ToolResult::err_code(
                ErrorCode::InvalidArguments,
                format!(
                    "todo {} has unknown status {status:?} — use one of: {}",
                    i + 1,
                    STATUSES.join(", ")
                ),
            ));
        }
        out.push(db::Todo {
            content: content.to_string(),
            status: status.to_string(),
            active_form: t
                .active_form
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        });
    }
    Ok(out)
}

/// The task list as text: a status mark per item, and the tally.
fn render_todos(items: &[db::Todo]) -> String {
    if items.is_empty() {
        return "task list is empty\n".to_string();
    }
    let done = items.iter().filter(|t| t.status == "completed").count();
    let mut out = format!("[{done}/{} done]\n", items.len());
    for t in items {
        let mark = match t.status.as_str() {
            "completed" => "x",
            "in_progress" => ">",
            _ => " ",
        };
        out.push_str(&format!("[{mark}] {}\n", t.content));
    }
    out
}

fn todo_json(items: &[db::Todo]) -> Vec<serde_json::Value> {
    items
        .iter()
        .map(|t| {
            serde_json::json!({
                "content": t.content,
                "status": t.status,
                "active_form": t.active_form,
            })
        })
        .collect()
}

/// One recorded fact, with the shared validation and answer shape.
///
/// The three `remember_*` tools differ only in the [`db::Fact`] they build.
/// Everything that must stay identical across them — refusing empty text,
/// resolving the session, shaping the answer — lives here once, so the three
/// tools cannot drift apart.
fn remember(ctx: &ToolCtx<'_>, fact: db::Fact<'_>) -> ToolResult {
    let kind = fact.kind();
    let text = fact.text().trim();
    if text.is_empty() {
        return ToolResult::err_code(
            ErrorCode::InvalidArguments,
            format!("{kind} text is empty — nothing to record"),
        );
    }
    let conn = match db_guard(ctx) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let session = match resolve_session(&conn, None) {
        Ok(s) => s,
        Err(r) => return r,
    };
    match db::record_fact(&conn, session, &fact) {
        Ok(id) => ToolResult::ok_structured(
            format!("recorded {kind}: {}\n", clip_chars(text, 160)),
            serde_json::json!({ "id": id, "kind": kind, "session_id": session }),
        ),
        Err(e) => db_failed(&format!("could not record the {kind}"), e),
    }
}

fn render_facts(f: &db::ProjectFacts) -> String {
    let mut out = match &f.objective {
        Some(o) => format!("objective: {o}\n"),
        None => "objective: (none recorded)\n".to_string(),
    };
    out.push_str(&format!(
        "progress: {}%  ·  {} decision(s)  ·  {} constraint(s)  ·  {} \
         failed attempt(s)  ·  {} changed file(s)\n",
        f.progress_percent,
        f.decisions.len(),
        f.constraints.len(),
        f.failed_attempts.len(),
        f.changed_files.len(),
    ));
    for (heading, rows) in [
        ("decisions", &f.decisions),
        ("constraints", &f.constraints),
        ("failed attempts (do not repeat)", &f.failed_attempts),
        ("changed files", &f.changed_files),
    ] {
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{heading}:\n"));
        for r in rows.iter() {
            out.push_str(&format!("  - {r}\n"));
        }
    }
    out
}

/// The handoff card as text.
///
/// Text as well as structure, because a handoff is the one result a caller is
/// likely to *paste* into a fresh conversation, where prose is what survives.
pub(crate) fn render_handoff(h: &crate::Handoff) -> String {
    let mut out = format!(
        "objective: {}\nprogress: {}%  ·  {} file(s) changed  ·  {} error(s) open\n",
        h.objective, h.progress_percent, h.files_changed, h.errors_remaining
    );
    if let Some(reason) = &h.end_reason {
        out.push_str(&format!("ended: {reason}\n"));
    }
    if let Some(next) = &h.next_step {
        out.push_str(&format!("next step: {next}\n"));
    }
    for (heading, rows) in [
        ("decisions", &h.decisions),
        ("constraints", &h.constraints),
        ("failed attempts (do not repeat)", &h.failed_attempts),
        ("files", &h.files),
    ] {
        if rows.is_empty() {
            continue;
        }
        out.push_str(&format!("\n{heading}:\n"));
        for r in rows.iter() {
            out.push_str(&format!("  - {r}\n"));
        }
    }
    if let Some(ctx) = &h.context {
        out.push_str(&format!("\ncontext:\n{ctx}\n"));
    }
    out.push_str(&format!("\n(generated {})\n", h.generated_at));
    out
}

/// Run one of the memory tools.
///
/// These reach the database rather than the workspace, so none of them takes a
/// project root — and `get_handoff` builds its card from the desktop's own
/// view of the project rather than from the call's root argument, which is why
/// it does not take one either.
/// How long `ask_user` / `propose_plan` may wait for the developer before
/// giving up. Matches the connector's approval window: a call that blocks
/// longer than the transport allows is a call the caller has already stopped
/// waiting for.
const QUESTION_WAIT: Duration = Duration::from_secs(120);

/// Tools whose result can exceed a page; caps shared by the renderers.
const DIAGNOSTIC_CAP: usize = 100;
const LOCATION_CAP: usize = 200;
const SYMBOL_CAP: usize = 200;
const MONITOR_CAP_MS: u32 = 120_000;

/// The root for a Phase 7–10 tool: the active worktree when the session is
/// inside one, otherwise the project root.
fn phase_root(ctx: &ToolCtx<'_>) -> Result<PathBuf, ToolResult> {
    let active = ctx
        .state
        .and_then(|s| s.active_worktree.lock().ok().and_then(|g| g.clone()));
    active
        .or_else(|| ctx.root.map(Path::to_path_buf))
        .ok_or_else(|| ToolResult::err_code(ErrorCode::InternalError, "project root not set"))
}

/// Resolve a workspace-relative path, mapping the failure to the same codes
/// the file tools use so a caller can branch on them.
fn phase_path(root: &Path, path: &str) -> Result<PathBuf, ToolResult> {
    resolve_path(root, path).map_err(|e| {
        if e.contains("escapes project root") {
            ToolResult::err_code(ErrorCode::PathEscapesRoot, e)
        } else {
            ToolResult::err_code(ErrorCode::FileNotFound, e)
        }
    })
}

/// Dispatch every Phase 7–10 tool. Each arm is responsible for its own root
/// and state, because those needs differ per tool.
fn run_phase_tool(tool: &Tool, ctx: &ToolCtx<'_>) -> ToolResult {
    match tool {
        Tool::WebFetch { url, max_bytes } => run_web_fetch(url, *max_bytes),
        Tool::WebSearch {
            query,
            max_results,
        } => run_web_search(query, *max_results),
        Tool::NotebookRead { path } => run_notebook_read(ctx, path),
        Tool::NotebookEdit {
            path,
            cell_id,
            new_source,
            cell_type,
        } => run_notebook_edit(ctx, path, cell_id, new_source, cell_type.as_deref()),
        Tool::DelegateTask { task, .. } => ToolResult::err_code(
            ErrorCode::AgentNotAvailable,
            format!(
                "there is no agent runtime for delegate_task to hand '{task}' to in this build; \
                 do the work with the tools you already have, or ask the developer to run it"
            ),
        ),
        Tool::LspDiagnostics { path, severity } => {
            run_lsp_diagnostics(ctx, path.as_deref(), severity.as_deref())
        }
        Tool::LspDefinition {
            path,
            line,
            character,
        } => run_lsp_definition(ctx, path, *line, *character),
        Tool::LspReferences {
            path,
            line,
            character,
            include_declaration,
        } => run_lsp_references(ctx, path, *line, *character, *include_declaration),
        Tool::LspSymbols { path, query } => {
            run_lsp_symbols(ctx, path.as_deref(), query.as_deref())
        }
        Tool::AskUser { question, options } => {
            run_ask_user(ctx, question, options)
        }
        Tool::ProposePlan { plan, steps } => run_propose_plan(ctx, plan, steps),
        Tool::Monitor {
            path,
            command_id,
            pattern,
            timeout_ms,
        } => run_monitor(ctx, path.as_deref(), *command_id, pattern.as_deref(), *timeout_ms),
        Tool::Notify {
            title,
            body,
            level,
        } => run_notify(ctx, title, body, level.as_deref()),
        Tool::EnterWorktree { name } => run_enter_worktree(ctx, name.as_deref()),
        Tool::ExitWorktree { action } => run_exit_worktree(ctx, action.as_deref()),
        Tool::ReadMedia { path } => run_read_media(ctx, path),
        Tool::PublishArtifact { path, title } => {
            run_publish_artifact(ctx, path, title.as_deref())
        }
        Tool::ReportFindings { findings, summary } => {
            run_report_findings(findings, summary.as_deref())
        }
        // Every variant is listed in the caller's pre-dispatch; reaching here
        // would mean that list and this one drifted.
        other => ToolResult::err_code(
            ErrorCode::InternalError,
            format!("unhandled phase tool: {other:?}"),
        ),
    }
}

// --- web ---------------------------------------------------------------------

fn run_web_fetch(url: &str, max_bytes: Option<u64>) -> ToolResult {
    let plan = match web::plan(url, max_bytes) {
        Ok(p) => p,
        Err(r) => return ToolResult::err_code(ErrorCode::NetworkBlocked, r.message()),
    };
    match web::fetch(&plan) {
        Ok(page) => {
            let text = if page.body.trim().is_empty() {
                format!(
                    "{} {} — no text content ({} bytes, {})",
                    page.status, page.status_text, page.bytes, page.content_type
                )
            } else {
                page.body.clone()
            };
            ToolResult::ok_structured(
                text,
                serde_json::json!({
                    "url": page.url,
                    "status": page.status,
                    "content_type": page.content_type,
                    "bytes": page.bytes,
                    "truncated": page.truncated,
                    "reduced_html": page.reduced_html,
                    "redirects": page.redirects,
                }),
            )
        }
        Err(r) => ToolResult::err_code(ErrorCode::NetworkBlocked, r.message()),
    }
}

fn run_web_search(query: &str, max_results: Option<u32>) -> ToolResult {
    let want = max_results.unwrap_or(8).clamp(1, 20) as usize;
    match web::search(query, max_results) {
        Ok(results) => {
            let truncated = results.len() >= want;
            let text = if results.is_empty() {
                format!("no results for {query:?}")
            } else {
                results
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        format!(
                            "{}. {}\n   {}\n   {}",
                            i + 1,
                            r.title,
                            r.url,
                            r.snippet
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            ToolResult::ok_structured(
                text,
                serde_json::json!({
                    "query": query,
                    "results": results,
                    "truncated": truncated,
                }),
            )
        }
        Err(r) => ToolResult::err_code(ErrorCode::NetworkBlocked, r.message()),
    }
}

// --- notebooks ---------------------------------------------------------------

fn notebook_error(path: &str, e: notebook::NotebookError) -> ToolResult {
    match e {
        notebook::NotebookError::Io(msg) => {
            ToolResult::err_code(ErrorCode::FileNotFound, format!("{path}: {msg}"))
        }
        notebook::NotebookError::Invalid(msg) => {
            ToolResult::err_code(ErrorCode::NotebookInvalid, format!("{path}: {msg}"))
        }
        notebook::NotebookError::NoSuchCell(id) => {
            ToolResult::err_code(ErrorCode::NotebookInvalid, format!("{path}: no cell '{id}'"))
        }
    }
}

fn render_notebook(path: &str, nb: &notebook::Notebook) -> String {
    let mut out = format!(
        "{path} — {} cell(s){}\n",
        nb.cells.len(),
        nb.kernel
            .as_deref()
            .map(|k| format!(", kernel {k}"))
            .unwrap_or_default()
    );
    for cell in &nb.cells {
        out.push_str(&format!(
            "\n--- cell {} [{}] ({} output(s)) ---\n{}\n",
            cell.id, cell.cell_type, cell.outputs, cell.source
        ));
    }
    out
}

fn run_notebook_read(ctx: &ToolCtx<'_>, path: &str) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // An `Auto` read that reached every path would be a way around the
    // sensitive-path gate `read_file` sits behind.
    if is_sensitive_path(&p) {
        return ToolResult::err_code(
            ErrorCode::SensitivePath,
            format!("{path}: sensitive path, not read"),
        );
    }
    match notebook::read(&p) {
        Ok(nb) => {
            let cells: Vec<serde_json::Value> = nb
                .cells
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "cell_id": c.id,
                        "cell_type": c.cell_type,
                        "source": c.source,
                        "outputs": c.outputs,
                    })
                })
                .collect();
            ToolResult::ok_structured(
                render_notebook(path, &nb),
                serde_json::json!({ "path": path, "kernel": nb.kernel, "cells": cells }),
            )
        }
        Err(e) => notebook_error(path, e),
    }
}

fn run_notebook_edit(
    ctx: &ToolCtx<'_>,
    path: &str,
    cell_id: &str,
    new_source: &str,
    cell_type: Option<&str>,
) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if is_sensitive_path(&p) {
        return ToolResult::err_code(
            ErrorCode::SensitivePath,
            format!("{path}: sensitive path, not edited"),
        );
    }
    match notebook::edit(&p, cell_id, new_source, cell_type) {
        Ok(outcome) => ToolResult::ok_structured(
            format!(
                "edited {} cell {} [{}] ({} bytes)",
                path, outcome.cell_id, outcome.cell_type, outcome.bytes_written
            ),
            serde_json::json!({
                "path": path,
                "cell_id": outcome.cell_id,
                "cell_type": outcome.cell_type,
                "bytes_written": outcome.bytes_written,
            }),
        ),
        Err(e) => notebook_error(path, e),
    }
}

// --- language server ---------------------------------------------------------

/// Run `f` against the cached server for `root`, starting one if needed.
///
/// A protocol failure evicts the client, so a server that has died is
/// replaced on the next request instead of failing every call until the app
/// restarts. An unavailable server is not cached, so a project that gains a
/// server (a `Cargo.toml` added during the session) is noticed.
fn with_lsp<R>(
    ctx: &ToolCtx<'_>,
    root: &Path,
    f: impl FnOnce(&mut lsp::Client) -> Result<R, lsp::LspError>,
) -> Result<R, ToolResult> {
    let Some(state) = ctx.state else {
        return Err(ToolResult::err_code(
            ErrorCode::LspUnavailable,
            "no language-server registry in this context",
        ));
    };
    let mut clients = state.lsp_clients.lock().unwrap();
    if !clients.contains_key(root) {
        match lsp::Client::start(root) {
            Ok(c) => {
                clients.insert(root.to_path_buf(), c);
            }
            Err(e) => {
                return Err(ToolResult::err_code(
                    ErrorCode::LspUnavailable,
                    e.to_string(),
                ))
            }
        }
    }
    let outcome = {
        let client = clients.get_mut(root).expect("just inserted");
        f(client)
    };
    match outcome {
        Ok(v) => Ok(v),
        Err(e) => {
            if matches!(e, lsp::LspError::Protocol(_)) {
                clients.remove(root);
            }
            let code = match e {
                lsp::LspError::Unavailable(_) => ErrorCode::LspUnavailable,
                lsp::LspError::Protocol(_) => ErrorCode::LspProtocolError,
            };
            Err(ToolResult::err_code(code, e.to_string()))
        }
    }
}

/// Severity as a rank, so "at least a warning" is a comparison.
fn severity_rank(sev: &str) -> u8 {
    match sev {
        "error" => 0,
        "warning" => 1,
        "information" => 2,
        _ => 3,
    }
}

fn run_lsp_diagnostics(
    ctx: &ToolCtx<'_>,
    path: Option<&str>,
    min_severity: Option<&str>,
) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let resolved = match path {
        Some(p) => match phase_path(&root, p) {
            Ok(p) => Some(p),
            Err(e) => return e,
        },
        None => None,
    };
    let min = severity_rank(min_severity.unwrap_or("warning"));
    let (server, mut diagnostics) = match with_lsp(ctx, &root, |c| {
        let server = c.server.clone();
        c.diagnostics(resolved.as_deref()).map(|d| (server, d))
    }) {
        Ok(v) => v,
        Err(e) => return e,
    };
    diagnostics.retain(|d| severity_rank(&d.severity) <= min);
    diagnostics.sort_by(|a, b| {
        severity_rank(&a.severity)
            .cmp(&severity_rank(&b.severity))
            .then_with(|| (a.path.as_str(), a.line, a.character).cmp(&(b.path.as_str(), b.line, b.character)))
    });
    // Dedupe: servers happily report the same diagnostic from two passes.
    diagnostics.dedup_by(|a, b| {
        a.path == b.path && a.line == b.line && a.character == b.character && a.message == b.message
    });
    let truncated = diagnostics.len() > DIAGNOSTIC_CAP;
    diagnostics.truncate(DIAGNOSTIC_CAP);
    let scanned = if resolved.is_some() { 1 } else { 0 };
    let json: Vec<serde_json::Value> = diagnostics
        .iter()
        .map(|d| {
            serde_json::json!({
                "path": d.path,
                "line": d.line,
                "character": d.character,
                "severity": d.severity,
                "message": d.message,
                "source": d.source,
            })
        })
        .collect();
    let text = if diagnostics.is_empty() {
        format!("no diagnostics from {server}")
    } else {
        let mut out = format!("{} diagnostic(s) from {server}\n", diagnostics.len());
        for d in &diagnostics {
            out.push_str(&format!(
                "{}:{}:{} {} {}\n",
                d.path, d.line, d.character, d.severity, d.message
            ));
        }
        if truncated {
            out.push_str("[more diagnostics withheld]\n");
        }
        out
    };
    ToolResult::ok_structured(
        text,
        serde_json::json!({
            "server": server,
            "scanned": scanned,
            "diagnostics": json,
            "truncated": truncated,
            "unavailable": false,
        }),
    )
}

fn location_json(locs: &[lsp::Location]) -> Vec<serde_json::Value> {
    locs.iter()
        .map(|l| {
            serde_json::json!({
                "path": l.path,
                "line": l.line,
                "character": l.character,
            })
        })
        .collect()
}

fn render_locations(verb: &str, locs: &[lsp::Location]) -> String {
    if locs.is_empty() {
        format!("no {verb} found")
    } else {
        locs.iter()
            .map(|l| format!("{}:{}:{}", l.path, l.line, l.character))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn run_lsp_definition(ctx: &ToolCtx<'_>, path: &str, line: u32, character: u32) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let pos = lsp::Position { line, character };
    let (server, mut locs) = match with_lsp(ctx, &root, |c| {
        let server = c.server.clone();
        c.definition(&p, pos).map(|l| (server, l))
    }) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let truncated = locs.len() > LOCATION_CAP;
    locs.truncate(LOCATION_CAP);
    ToolResult::ok_structured(
        render_locations("definitions", &locs),
        serde_json::json!({
            "server": server,
            "locations": location_json(&locs),
            "truncated": truncated,
            "unavailable": false,
        }),
    )
}

fn run_lsp_references(
    ctx: &ToolCtx<'_>,
    path: &str,
    line: u32,
    character: u32,
    include_declaration: Option<bool>,
) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let pos = lsp::Position { line, character };
    let include = include_declaration.unwrap_or(true);
    let (server, mut locs) = match with_lsp(ctx, &root, |c| {
        let server = c.server.clone();
        c.references(&p, pos, include).map(|l| (server, l))
    }) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let truncated = locs.len() > LOCATION_CAP;
    locs.truncate(LOCATION_CAP);
    ToolResult::ok_structured(
        render_locations("references", &locs),
        serde_json::json!({
            "server": server,
            "locations": location_json(&locs),
            "truncated": truncated,
            "unavailable": false,
        }),
    )
}

fn run_lsp_symbols(
    ctx: &ToolCtx<'_>,
    path: Option<&str>,
    query: Option<&str>,
) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let resolved = match path {
        Some(p) => match phase_path(&root, p) {
            Ok(p) => Some(p),
            Err(e) => return e,
        },
        None => None,
    };
    let (server, mut symbols) = match with_lsp(ctx, &root, |c| {
        let server = c.server.clone();
        c.symbols(resolved.as_deref(), query).map(|s| (server, s))
    }) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let truncated = symbols.len() > SYMBOL_CAP;
    symbols.truncate(SYMBOL_CAP);
    let json: Vec<serde_json::Value> = symbols
        .iter()
        .map(|s| {
            serde_json::json!({
                "name": s.name,
                "kind": s.kind,
                "path": s.path,
                "line": s.line,
                "container": s.container,
            })
        })
        .collect();
    let text = if symbols.is_empty() {
        "no symbols found".to_string()
    } else {
        symbols
            .iter()
            .map(|s| {
                format!(
                    "{} {}  {}:{}",
                    s.kind, s.name, s.path, s.line
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    ToolResult::ok_structured(
        text,
        serde_json::json!({
            "server": server,
            "symbols": json,
            "truncated": truncated,
            "unavailable": false,
        }),
    )
}

// --- agent loop --------------------------------------------------------------

/// Put a card on the desktop and block until it is answered.
///
/// `Ok(None)` is a timeout — the developer did not answer in time — which is
/// not the same as "no desktop": one is a person who stepped away, the other
/// is a harness that cannot ask, and only the second is a tool failure.
fn ask_desktop(
    ctx: &ToolCtx<'_>,
    kind: &str,
    title: &str,
    body: &str,
    options: &[String],
) -> Result<Option<QuestionAnswer>, ToolResult> {
    let Some(state) = ctx.state else {
        return Err(ToolResult::err_code(
            ErrorCode::AgentNotAvailable,
            "no desktop is attached to answer; ask the developer in the chat instead",
        ));
    };
    let id = state.question_seq.fetch_add(1, Ordering::SeqCst);
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    state.questions.lock().unwrap().insert(id, tx);
    let card = QuestionCard {
        id,
        kind: kind.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        options: options.to_vec(),
        source: SOURCE_MCP.to_string(),
    };
    let emitted = state
        .app_handle
        .lock()
        .unwrap()
        .as_ref()
        .map(|h| {
            use tauri::Emitter;
            h.emit("bridge://question-requested", &card).is_ok()
        })
        .unwrap_or(false);
    if !emitted {
        state.questions.lock().unwrap().remove(&id);
        return Err(ToolResult::err_code(
            ErrorCode::AgentNotAvailable,
            "no desktop is attached to answer; ask the developer in the chat instead",
        ));
    }
    match rx.recv_timeout(QUESTION_WAIT) {
        Ok(answer) => Ok(Some(answer)),
        Err(_) => {
            state.questions.lock().unwrap().remove(&id);
            Ok(None)
        }
    }
}

fn run_ask_user(ctx: &ToolCtx<'_>, question: &str, options: &[String]) -> ToolResult {
    match ask_desktop(
        ctx,
        "question",
        "The assistant has a question",
        question,
        options,
    ) {
        Ok(Some(a)) => ToolResult::ok_structured(
            format!("answer: {}", a.answer),
            serde_json::json!({
                "question": question,
                "option": a.option,
                "answer": a.answer,
                "timed_out": false,
            }),
        ),
        Ok(None) => ToolResult::ok_structured(
            format!("no answer within {}s", QUESTION_WAIT.as_secs()),
            serde_json::json!({
                "question": question,
                "option": serde_json::Value::Null,
                "answer": "",
                "timed_out": true,
            }),
        ),
        Err(e) => e,
    }
}

fn run_propose_plan(ctx: &ToolCtx<'_>, plan: &str, steps: &[String]) -> ToolResult {
    let body = if steps.is_empty() {
        plan.to_string()
    } else {
        format!(
            "{plan}\n\n{}",
            steps
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}. {s}", i + 1))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let options = vec!["Allow".to_string(), "Deny".to_string()];
    match ask_desktop(ctx, "plan", "Plan for approval", &body, &options) {
        Ok(Some(a)) => {
            let approved = a
                .option
                .as_deref()
                .map(|o| o.eq_ignore_ascii_case("allow"))
                .unwrap_or(false);
            let comment = (!a.answer.trim().is_empty()).then(|| a.answer.clone());
            ToolResult::ok_structured(
                if approved {
                    "plan approved".to_string()
                } else {
                    format!("plan denied{}", comment.as_deref().map(|c| format!(": {c}")).unwrap_or_default())
                },
                serde_json::json!({
                    "approved": approved,
                    "comment": comment,
                    "steps": steps.len(),
                    "timed_out": false,
                }),
            )
        }
        Ok(None) => ToolResult::ok_structured(
            "plan approval timed out; do not proceed".to_string(),
            serde_json::json!({
                "approved": false,
                "comment": serde_json::Value::Null,
                "steps": steps.len(),
                "timed_out": true,
            }),
        ),
        Err(e) => e,
    }
}

fn run_notify(ctx: &ToolCtx<'_>, title: &str, body: &str, level: Option<&str>) -> ToolResult {
    let level = level.unwrap_or("info");
    let raised = ctx
        .state
        .and_then(|s| s.app_handle.lock().unwrap().clone())
        .map(|h| {
            use tauri::Emitter;
            h.emit(
                "bridge://notify",
                serde_json::json!({ "title": title, "body": body, "level": level }),
            )
            .is_ok()
        })
        .unwrap_or(false);
    ToolResult::ok_structured(
        format!("[{level}] {title}: {body}"),
        serde_json::json!({ "title": title, "level": level, "raised": raised }),
    )
}

fn run_monitor(
    ctx: &ToolCtx<'_>,
    path: Option<&str>,
    command_id: Option<u64>,
    pattern: Option<&str>,
    timeout_ms: Option<u32>,
) -> ToolResult {
    let timeout = Duration::from_millis(
        timeout_ms.unwrap_or(60_000).clamp(1_000, MONITOR_CAP_MS) as u64,
    );
    let re = match pattern {
        Some(p) => match regex::Regex::new(p) {
            Ok(re) => Some(re),
            Err(e) => {
                return ToolResult::err_code(
                    ErrorCode::RegexInvalid,
                    format!("bad pattern {p:?}: {e}"),
                )
            }
        },
        None => None,
    };
    match (path, command_id) {
        (Some(path), None) => monitor_path(ctx, path, re.as_ref(), timeout),
        (None, Some(id)) => monitor_command(ctx, id, re.as_ref(), timeout),
        _ => ToolResult::err_code(
            ErrorCode::InvalidArguments,
            "monitor needs exactly one of `path` or `command_id`",
        ),
    }
}

fn monitor_path(
    ctx: &ToolCtx<'_>,
    path: &str,
    re: Option<&regex::Regex>,
    timeout: Duration,
) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let started = Instant::now();
    let before = fingerprint(&p);
    loop {
        thread::sleep(Duration::from_millis(250));
        let after = fingerprint(&p);
        if after != before {
            let detail = match re {
                None => Some(format!("{path} changed")),
                Some(re) => match std::fs::read_to_string(&p) {
                    Ok(text) => re.find(&text).map(|m| {
                        format!("matched {:?}", m.as_str())
                    }),
                    Err(_) => None,
                },
            };
            if detail.is_some() {
                return ToolResult::ok_structured(
                    detail.clone().unwrap(),
                    serde_json::json!({
                        "matched": true,
                        "reason": "path",
                        "detail": detail,
                        "elapsed_ms": started.elapsed().as_millis() as u64,
                    }),
                );
            }
        }
        if started.elapsed() >= timeout {
            return ToolResult::ok_structured(
                format!("no match within {}ms", timeout.as_millis()),
                serde_json::json!({
                    "matched": false,
                    "reason": "timeout",
                    "detail": serde_json::Value::Null,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
        }
    }
}

/// A cheap change signal: existence, size, and modification time.
fn fingerprint(p: &Path) -> Option<(u64, Option<std::time::SystemTime>)> {
    std::fs::metadata(p)
        .ok()
        .map(|m| (m.len(), m.modified().ok()))
}

fn monitor_command(
    ctx: &ToolCtx<'_>,
    id: u64,
    re: Option<&regex::Regex>,
    timeout: Duration,
) -> ToolResult {
    let Some(state) = ctx.state else {
        return ToolResult::err_code(
            ErrorCode::InternalError,
            "no background-command manager in this context",
        );
    };
    let manager = &state.bgproc;
    let started = Instant::now();
    let mut cursor = 0u64;
    loop {
        match manager.snapshot(id, cursor) {
            Ok(s) => {
                cursor = s.next_cursor;
                let matched = match re {
                    None => !s.output.is_empty(),
                    Some(re) => re.is_match(&s.output),
                };
                if matched {
                    return ToolResult::ok_structured(
                        format!("command #{id} output matched"),
                        serde_json::json!({
                            "matched": true,
                            "reason": "command",
                            "detail": s.output.chars().take(2000).collect::<String>(),
                            "elapsed_ms": started.elapsed().as_millis() as u64,
                        }),
                    );
                }
                if s.complete {
                    return ToolResult::ok_structured(
                        format!("command #{id} finished without a match"),
                        serde_json::json!({
                            "matched": false,
                            "reason": "command_finished",
                            "detail": serde_json::Value::Null,
                            "elapsed_ms": started.elapsed().as_millis() as u64,
                        }),
                    );
                }
            }
            Err(lookup) => {
                return ToolResult::err_code(
                    missing_command_code(lookup),
                    missing_command_message(lookup, id),
                )
            }
        }
        if started.elapsed() >= timeout {
            return ToolResult::ok_structured(
                format!("no match within {}ms", timeout.as_millis()),
                serde_json::json!({
                    "matched": false,
                    "reason": "timeout",
                    "detail": serde_json::Value::Null,
                    "elapsed_ms": started.elapsed().as_millis() as u64,
                }),
            );
        }
        thread::sleep(Duration::from_millis(250));
    }
}

// --- isolation & delivery ----------------------------------------------------

fn run_enter_worktree(ctx: &ToolCtx<'_>, name: Option<&str>) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let Some(state) = ctx.state else {
        return ToolResult::err_code(
            ErrorCode::InternalError,
            "no app state to record the worktree in",
        );
    };
    if state.active_worktree.lock().unwrap().is_some() {
        return ToolResult::err_code(
            ErrorCode::InvalidArguments,
            "already inside a worktree; call exit_worktree first",
        );
    }
    let name = name.map(str::to_string).unwrap_or_else(|| {
        format!(
            "wt-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        )
    });
    let repo = match git::open_repo(&root) {
        Ok(r) => r,
        Err(e) => return ToolResult::err_code(ErrorCode::NotAGitRepo, format!("{root:?}: {e}")),
    };
    match git::add_worktree(&repo, &root, &name) {
        Ok(info) => {
            *state.active_worktree.lock().unwrap() = Some(PathBuf::from(&info.path));
            ToolResult::ok_structured(
                format!(
                    "entered worktree {} on branch {} — subsequent tools work there",
                    info.path, info.branch
                ),
                serde_json::json!({
                    "worktree": info.path,
                    "branch": info.branch,
                    "active": true,
                    "removed": false,
                }),
            )
        }
        Err(e) => ToolResult::err_code(
            ErrorCode::ExecutionFailed,
            format!("could not create worktree {name:?}: {e}"),
        ),
    }
}

fn run_exit_worktree(ctx: &ToolCtx<'_>, action: Option<&str>) -> ToolResult {
    let Some(state) = ctx.state else {
        return ToolResult::err_code(ErrorCode::InternalError, "no app state");
    };
    let active = state.active_worktree.lock().unwrap().clone();
    let Some(wt_path) = active else {
        return ToolResult::err_code(
            ErrorCode::InvalidArguments,
            "not inside a worktree",
        );
    };
    let discard = action.map(|a| a.eq_ignore_ascii_case("discard")) == Some(true);
    let project_root = ctx.root.map(Path::to_path_buf).unwrap_or_else(|| wt_path.clone());
    let repo = match git::open_repo(&project_root) {
        Ok(r) => r,
        Err(e) => return ToolResult::err_code(ErrorCode::NotAGitRepo, e.to_string()),
    };
    let name = wt_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if discard {
        if let Err(e) = git::remove_worktree(&repo, &name, true) {
            return ToolResult::err_code(
                ErrorCode::ExecutionFailed,
                format!("could not remove worktree {name:?}: {e}"),
            );
        }
    }
    *state.active_worktree.lock().unwrap() = None;
    ToolResult::ok_structured(
        if discard {
            format!("left and removed worktree {name}")
        } else {
            format!("left worktree {name} (kept on disk)")
        },
        serde_json::json!({
            "worktree": wt_path.to_string_lossy(),
            "branch": name,
            "active": false,
            "removed": discard,
        }),
    )
}

fn run_read_media(ctx: &ToolCtx<'_>, path: &str) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    if is_sensitive_path(&p) {
        return ToolResult::err_code(
            ErrorCode::SensitivePath,
            format!("{path}: sensitive path, not returned as media"),
        );
    }
    let md = match std::fs::metadata(&p) {
        Ok(md) if md.is_dir() => {
            return ToolResult::err_code(
                ErrorCode::InvalidArguments,
                format!("{path} is a directory"),
            )
        }
        Ok(md) => md,
        Err(e) => return ToolResult::err_code(ErrorCode::FileNotFound, format!("{path}: {e}")),
    };
    if md.len() > media::MAX_MEDIA_BYTES {
        return ToolResult::err_code(
            ErrorCode::FileTooLarge,
            format!(
                "{path}: {} bytes exceeds the {} byte media cap",
                md.len(),
                media::MAX_MEDIA_BYTES
            ),
        );
    }
    let Some(mime) = media::mime_for(&p) else {
        return ToolResult::err_code(
            ErrorCode::InvalidArguments,
            format!("{path}: not a media type this tool returns; use read_file"),
        );
    };
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => return ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{path}: {e}")),
    };
    let kind = match media::kind_for(mime) {
        media::MediaKind::Image => "image",
        media::MediaKind::Pdf => "pdf",
        media::MediaKind::Text => "text",
    };
    let encoded = media::base64_encode(&bytes);
    ToolResult::ok_media(
        format!("{path} — {mime}, {} bytes", bytes.len()),
        serde_json::json!({
            "path": path,
            "media_type": mime,
            "bytes": bytes.len(),
            "kind": kind,
        }),
        MediaPayload {
            media_type: mime.to_string(),
            base64: encoded,
            kind: kind.to_string(),
        },
    )
}

fn run_publish_artifact(ctx: &ToolCtx<'_>, path: &str, title: Option<&str>) -> ToolResult {
    let root = match phase_root(ctx) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let p = match phase_path(&root, path) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) => return ToolResult::err_code(ErrorCode::FileNotFound, format!("{path}: {e}")),
    };
    let file_name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "artifact".to_string());
    let dir = root.join(".lexsus/artifacts");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return ToolResult::err_code(ErrorCode::ExecutionFailed, format!("artifacts dir: {e}"));
    }
    let dest = dir.join(&file_name);
    if let Err(e) = std::fs::write(&dest, &bytes) {
        return ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{file_name}: {e}"));
    }
    let media_type = media::mime_for(&p).unwrap_or("application/octet-stream");
    let title = title.unwrap_or(&file_name);
    ToolResult::ok_structured(
        format!("published {title} -> {}", dest.display()),
        serde_json::json!({
            "path": path,
            "artifact": dest.to_string_lossy(),
            "title": title,
            "bytes": bytes.len(),
            "media_type": media_type,
        }),
    )
}

fn run_report_findings(findings: &[Finding], summary: Option<&str>) -> ToolResult {
    let errors = findings.iter().filter(|f| f.severity == "error").count();
    let warnings = findings.iter().filter(|f| f.severity == "warning").count();
    let mut text = String::new();
    if let Some(s) = summary {
        text.push_str(s);
        text.push('\n');
    }
    text.push_str(&format!(
        "{} finding(s): {errors} error, {warnings} warning\n",
        findings.len()
    ));
    for f in findings {
        text.push_str(&format!(
            "[{}] {}:{} {}{}\n",
            f.severity,
            f.path,
            f.line,
            f.claim,
            f.evidence
                .as_deref()
                .map(|e| format!("  ({e})"))
                .unwrap_or_default()
        ));
    }
    let json: Vec<serde_json::Value> = findings
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "line": f.line,
                "severity": f.severity,
                "claim": f.claim,
                "evidence": f.evidence,
            })
        })
        .collect();
    ToolResult::ok_structured(
        text,
        serde_json::json!({
            "summary": summary,
            "count": findings.len(),
            "errors": errors,
            "warnings": warnings,
            "findings": json,
        }),
    )
}

fn run_memory_tool(tool: &Tool, ctx: &ToolCtx<'_>) -> ToolResult {
    match tool {
        Tool::TodoWrite { todos } => {
            // Validate before writing: a list is one value, so nothing is
            // stored unless all of it is.
            let items = match normalise_todos(todos) {
                Ok(i) => i,
                Err(r) => return r,
            };
            let conn = match db_guard(ctx) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let session = match connector_session(&conn) {
                Ok(s) => s,
                Err(r) => return r,
            };
            if let Err(e) = db::replace_todos(&conn, session, &items) {
                return db_failed("could not save the task list", e);
            }
            // Answer from what was *stored*, not from what was handed in.
            // The two agree by construction today, which is exactly why the
            // difference would never be noticed: echoing the input would
            // report success for a write that quietly kept the old rows too.
            // Reading back costs one query and makes the answer evidence.
            match db::read_todos(&conn, session) {
                Ok(stored) => ToolResult::ok_structured(
                    render_todos(&stored),
                    serde_json::json!({ "count": stored.len(), "todos": todo_json(&stored) }),
                ),
                Err(e) => db_failed("could not read the task list back", e),
            }
        }
        Tool::TodoRead => {
            let conn = match db_guard(ctx) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let session = match connector_session(&conn) {
                Ok(s) => s,
                Err(r) => return r,
            };
            match db::read_todos(&conn, session) {
                Ok(items) => ToolResult::ok_structured(
                    render_todos(&items),
                    serde_json::json!({ "count": items.len(), "todos": todo_json(&items) }),
                ),
                Err(e) => db_failed("could not read the task list", e),
            }
        }
        Tool::SetObjective { text } => {
            let text = text.trim();
            if text.is_empty() {
                return ToolResult::err_code(
                    ErrorCode::InvalidArguments,
                    "objective text is empty",
                );
            }
            let conn = match db_guard(ctx) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let session = match connector_session(&conn) {
                Ok(s) => s,
                Err(r) => return r,
            };
            match db::set_objective(&conn, session, text) {
                Ok(previous) => {
                    let mut out = format!("objective set: {text}\n");
                    if let Some(prev) = &previous {
                        out.push_str(&format!("(replaced: {prev})\n"));
                    }
                    ToolResult::ok_structured(
                        out,
                        serde_json::json!({ "objective": text, "previous": previous }),
                    )
                }
                Err(e) => db_failed("could not set the objective", e),
            }
        }
        Tool::RememberDecision { summary, reason } => remember(
            ctx,
            db::Fact::Decision {
                summary: summary.trim(),
                // An empty reason is no reason: storing "" would make the
                // handoff claim a rationale it does not have.
                reason: reason.as_deref().map(str::trim).filter(|r| !r.is_empty()),
            },
        ),
        Tool::RememberConstraint { text } => {
            remember(ctx, db::Fact::Constraint { text: text.trim() })
        }
        Tool::RememberAttempt {
            description,
            succeeded,
        } => remember(
            ctx,
            db::Fact::Attempt {
                description: description.trim(),
                succeeded: succeeded.unwrap_or(false),
            },
        ),
        Tool::GetFacts { session_id } => {
            let conn = match db_guard(ctx) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let session = match resolve_session(&conn, *session_id) {
                Ok(s) => s,
                Err(r) => return r,
            };
            match db::get_facts(&conn, session) {
                Ok(f) => {
                    let body = render_facts(&f);
                    ToolResult::ok_structured(
                        body,
                        serde_json::json!({
                            "session_id": session,
                            "objective": f.objective,
                            "progress_percent": f.progress_percent,
                            "decisions": f.decisions,
                            "failed_attempts": f.failed_attempts,
                            "constraints": f.constraints,
                            "changed_files": f.changed_files,
                        }),
                    )
                }
                Err(e) => db_failed("could not read the project memory", e),
            }
        }
        Tool::ListSessions { limit } => {
            let conn = match db_guard(ctx) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let limit = limit
                .map(|n| (n.max(1) as usize).min(SESSION_LIST_MAX))
                .unwrap_or(20);
            match db::list_sessions(&conn, limit) {
                Ok(rows) => {
                    let mut out = String::new();
                    for s in &rows {
                        out.push_str(&format!(
                            "#{} {} — {} ({} event(s), {})\n",
                            s.id,
                            s.agent,
                            s.objective.as_deref().unwrap_or("(no objective)"),
                            s.events,
                            s.started_at,
                        ));
                    }
                    if out.is_empty() {
                        out.push_str("no archived sessions\n");
                    }
                    let sessions: Vec<serde_json::Value> = rows
                        .iter()
                        .map(|s| {
                            serde_json::json!({
                                "id": s.id,
                                "agent": s.agent,
                                "objective": s.objective,
                                "started_at": s.started_at,
                                "events": s.events,
                            })
                        })
                        .collect();
                    ToolResult::ok_structured(
                        out,
                        serde_json::json!({ "count": sessions.len(), "sessions": sessions }),
                    )
                }
                Err(e) => db_failed("could not list sessions", e),
            }
        }
        Tool::RequestHandoff { reason, next_step } => {
            let reason = reason.trim();
            if reason.is_empty() {
                return ToolResult::err_code(
                    ErrorCode::InvalidArguments,
                    "handoff reason is empty — say why the developer is needed",
                );
            }
            let conn = match db_guard(ctx) {
                Ok(c) => c,
                Err(r) => return r,
            };
            let session = match connector_session(&conn) {
                Ok(s) => s,
                Err(r) => return r,
            };
            // Read the objective before recording, so the request carries the
            // context it was made in rather than a later one.
            let objective = db::get_facts(&conn, session).ok().and_then(|f| f.objective);
            match db::record_handoff_request(&conn, session, reason, next_step.as_deref()) {
                Ok(id) => ToolResult::ok_structured(
                    format!(
                        "handoff requested: {reason}\n\
                         The desktop will show this to the developer.\n"
                    ),
                    serde_json::json!({ "id": id, "objective": objective }),
                ),
                Err(e) => db_failed("could not record the handoff request", e),
            }
        }
        Tool::GetHandoff => {
            let Some(state) = ctx.state else {
                return ToolResult::err_code(
                    ErrorCode::InternalError,
                    "the handoff is built from the desktop's live state, which \
                     this caller did not supply",
                );
            };
            match crate::build_handoff_impl(state) {
                Ok(h) => {
                    let body = render_handoff(&h);
                    let structured = serde_json::to_value(&h).unwrap_or(serde_json::Value::Null);
                    ToolResult::ok_structured(body, structured)
                }
                Err(e) => ToolResult::err_code(
                    ErrorCode::ExecutionFailed,
                    format!("could not build the handoff: {e}"),
                ),
            }
        }
        // The caller only routes memory tools here.
        _ => ToolResult::err_code(
            ErrorCode::InternalError,
            format!("{} is not a memory tool", tool_name(tool)),
        ),
    }
}

// --- background commands -----------------------------------------------------
//
// The three tools over `bgproc.rs`. Each one is a thin render of a manager
// call: the state machine, the window and the cleanup live in that module, and
// what happens here is argument checking and turning a `Result` into the two
// halves of a `ToolResult`.

/// Run one of the three background-command tools.
///
/// These need a workspace to run *in* and a manager to be registered *with*,
/// and the two come from different halves of the context. Neither substitutes
/// for the other, so each is checked and named separately.
fn run_bg_tool(tool: &Tool, ctx: &ToolCtx<'_>) -> ToolResult {
    let Some(root) = ctx.root else {
        return ToolResult::err_code(ErrorCode::InternalError, "project root not set");
    };
    let Some(state) = ctx.state else {
        return ToolResult::err_code(
            ErrorCode::InternalError,
            "no background-command manager in this context",
        );
    };
    let manager = &state.bgproc;

    match tool {
        Tool::RunCommandBackground { command } => {
            match manager.start(Shell::detect(), command, root) {
                Ok(started) => ToolResult::ok_structured(
                    // Naming the readers is the point of the sentence: a
                    // handle the caller does not know how to use is the same
                    // as no handle at all.
                    format!(
                        "command #{} started in the background{}\n\
                         [read it with command_output, stop it with kill_command]\n",
                        started.id,
                        match started.pid {
                            Some(pid) => format!(" (pid {pid})"),
                            None => String::new(),
                        }
                    ),
                    serde_json::json!({
                        "id": started.id,
                        "pid": started.pid,
                        "command": started.command,
                    }),
                ),
                Err(bgproc::StartError::Busy { running, cap }) => ToolResult::err_code(
                    ErrorCode::TooManyProcesses,
                    format!(
                        "{running} background commands are already running (limit {cap}); \
                         stop one with kill_command first"
                    ),
                ),
                Err(bgproc::StartError::Spawn(e)) => ToolResult::err_code(
                    ErrorCode::ExecutionFailed,
                    format!("could not start the command: {e}"),
                ),
            }
        }
        Tool::CommandOutput { id, cursor } => {
            // Absent means "from the start of what is kept", which is what a
            // first read wants. Defaulting to the *end* would make the first
            // read of a command that has already printed something look like
            // a command that printed nothing.
            match manager.snapshot(*id, cursor.unwrap_or(0)) {
                Ok(s) => ToolResult::ok_structured(
                    bgproc::render_snapshot(&s),
                    serde_json::json!({
                        "id": s.id,
                        "status": s.status.state(),
                        "exit_code": s.status.exit_code(),
                        "cursor": s.cursor,
                        "next_cursor": s.next_cursor,
                        "lost": s.lost,
                        "more": s.more,
                        "complete": s.complete,
                        "elapsed_ms": s.elapsed_ms,
                    }),
                ),
                Err(lookup) => ToolResult::err_code(
                    missing_command_code(lookup),
                    missing_command_message(lookup, *id),
                ),
            }
        }
        Tool::KillCommand { id } => match manager.kill(*id) {
            Ok(k) => ToolResult::ok_structured(
                bgproc::render_kill(&k),
                serde_json::json!({
                    "id": k.id,
                    "status": k.status.state(),
                    "exit_code": k.status.exit_code(),
                    "already_finished": k.already_finished,
                }),
            ),
            Err(lookup) => ToolResult::err_code(
                missing_command_code(lookup),
                missing_command_message(lookup, *id),
            ),
        },
        // The caller only routes background-command tools here.
        _ => ToolResult::err_code(
            ErrorCode::InternalError,
            format!("{} is not a background-command tool", tool_name(tool)),
        ),
    }
}

/// Two different facts, two different codes: an id that was never issued is a
/// caller mistake, and an id whose output has been released is not.
fn missing_command_code(lookup: bgproc::Lookup) -> ErrorCode {
    match lookup {
        bgproc::Lookup::Unknown => ErrorCode::ProcessNotFound,
        bgproc::Lookup::Evicted => ErrorCode::OutputGone,
    }
}

fn missing_command_message(lookup: bgproc::Lookup, id: u64) -> String {
    match lookup {
        bgproc::Lookup::Unknown => format!(
            "no background command #{id}. run_command_background returns the handle to use here."
        ),
        bgproc::Lookup::Evicted => format!(
            "command #{id} finished long enough ago that its output has been released; \
             only the most recent finished commands are kept readable"
        ),
    }
}

// --- git -------------------------------------------------------------------
//
// Ten tools over `git.rs`, which already did the work: eight are wiring, and
// only `create_branch` and `show` needed new code there. What this layer adds
// is the part `git.rs` cannot know about — the error *vocabulary* a caller
// reasons with.
//
// `git.rs` returns `git2::Error`, whose `message()` is prose. That is fine for
// the desktop panel, which shows it to a person, and wrong for a tool call,
// which returns it to a program: "no such branch" and "you have unsaved work"
// are both just strings, so a caller can only tell them apart by matching
// English. Every tool below maps the failure to an `ErrorCode` instead, and
// uses the specific one wherever the situation is distinguishable.

/// Open the workspace repository, or `NOT_A_GIT_REPO`.
fn open_workspace_repo(root: &Path) -> Result<git2::Repository, ToolResult> {
    git::open_repo(root).map_err(|e| {
        ToolResult::err_code(
            ErrorCode::NotAGitRepo,
            format!("{} is not a git repository: {e}", root.display()),
        )
    })
}

/// A `git2::Error` from an operation that has no more specific code.
fn git_failed(what: &str, e: git2::Error) -> ToolResult {
    ToolResult::err_code(ErrorCode::ExecutionFailed, format!("{what}: {e}"))
}

/// Total patch text a single git tool result will carry.
///
/// Diffs are the one thing these tools return whose size the caller does not
/// choose, so it is bounded here rather than left to the connector's cap —
/// which truncates the *string* and would leave `structured.patch` claiming a
/// size the payload does not contain. Files past the budget are still listed,
/// with their counts, and marked as not included: the answer stays complete
/// about *what* changed even when it is partial about *how*.
const GIT_PATCH_BUDGET: usize = 96 * 1024;

/// Ceiling for `git_log`'s `limit`. A caller asking for 10 000 commits is
/// asking for a result it cannot use; the cap is the same "stop, don't
/// truncate" rule the search tools follow.
const GIT_LOG_MAX: usize = 200;

/// Report a `git_checkout` that the working tree blocks.
///
/// The roadmap invariant is that checkout refuses rather than forcing, so this
/// is a routine outcome, not an error to apologise for — hence its own code,
/// and a message that names the files so the caller can act on it.
fn worktree_dirty(name: &str, dirty: &[String]) -> ToolResult {
    let mut msg = format!(
        "cannot switch to '{name}': {} uncommitted change(s) — commit or stash \
         before switching",
        dirty.len()
    );
    for path in dirty.iter().take(3) {
        msg.push_str("\n  ");
        msg.push_str(path);
    }
    if dirty.len() > 3 {
        msg.push_str(&format!("\n  ...and {} more", dirty.len() - 3));
    }
    ToolResult::err_code(ErrorCode::WorktreeDirty, msg)
}

/// Split a patch across the byte budget, reporting what was left out.
///
/// Returns the patch text that fits and the number of files whose patch did
/// not — the `take`s happen before the `collect` (ch. 5 §5.3), so a large diff
/// stops being *formatted*, not just being printed.
fn fit_patch(patch: &str, budget: &mut usize) -> (String, bool) {
    if patch.len() <= *budget {
        *budget -= patch.len();
        return (patch.to_string(), false);
    }
    ("".to_string(), true)
}

// --- search ---------------------------------------------------------------
//
// `grep` and `glob` are the first tools whose cost is proportional to the
// *workspace* rather than to their arguments, which is what makes them where
// the book's two rules about work start to bite:
//
//   * Ch 13 §13.1 — split argument handling from traversal. `plan_grep`
//     compiles and validates the request with no filesystem access at all, so
//     the part that a model can get wrong is testable on its own; `run_grep`
//     is the thin effectful shell that walks and reads.
//   * Ch 5 §5.3 — a cap must *stop* the work, not truncate its result. The
//     walk is callback-driven and takes a `ControlFlow::Break`, so a search
//     that has found its last match stops reading files instead of reading
//     every one of them and discarding the tail. Without this a `grep` for a
//     common token in a large tree is unbounded work behind a bounded answer.

/// Directories never descended into. Not a cosmetic tidy-up: a single
/// `node_modules` can hold millions of files, and the cap above would still
/// pay for every one of them.
const WALK_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".next",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    "vendor",
    "coverage",
];

/// Depth past which the walk stops descending. A workspace nested this deeply
/// is pathological; the cap is here so a cycle the filesystem does not show as
/// a symlink (a bind mount, a runaway tree) cannot exhaust the stack.
const WALK_MAX_DEPTH: usize = 48;

/// How many failures a result names individually before summarising the rest.
/// The *count* is always exact — only the listing is bounded, so a directory
/// of unreadable files cannot turn one call into an unbounded answer.
const MAX_REPORTED_FAILURES: usize = 10;

/// Search reads a whole file but returns only matching lines, so its cap is
/// about memory rather than about how much the caller is shown — generously
/// above `READ_CAP`, which bounds what a *reader* returns.
const SEARCH_FILE_CAP: u64 = 8 * 1024 * 1024;

/// A matched line is quoted back up to here. One minified file can hold a
/// whole megabyte on a single line; quoting it verbatim would spend the whole
/// result budget on one match.
const GREP_LINE_CHARS: usize = 400;

/// Reported matches before `grep` stops. High enough that a real search rarely
/// reaches it, low enough that a search for a common token cannot return a
/// payload the connector then has to truncate.
const GREP_MAX_RESULTS: usize = 200;

/// Reported paths before `glob` stops.
const GLOB_MAX_RESULTS: usize = 500;

/// Paths a walk could not use, in the order they were met.
///
/// This is Ch 12 §12.4.1's `Validation` shape — a `head` that is always
/// present once there is a failure, plus an accumulating `tail` — kept as a
/// list because the successes are reported alongside rather than as the other
/// side of a sum type. The property the book cares about is what matters
/// here: a walk in which one file is unreadable must still return the matches
/// in all the others, *and* must say that it could not read that one, rather
/// than quietly reporting a smaller answer.
struct Failures {
    rows: Vec<serde_json::Value>,
    total: usize,
}

impl Failures {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            total: 0,
        }
    }

    fn push(&mut self, path: &str, reason: impl Into<String>) {
        let reason: String = reason.into();
        self.total += 1;
        if self.rows.len() < MAX_REPORTED_FAILURES {
            self.rows
                .push(serde_json::json!({ "path": path, "reason": reason }));
        }
    }

    fn any(&self) -> bool {
        self.total > 0
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!(self.rows)
    }

    /// The human-readable half, or `None` when there is nothing to say — so a
    /// clean search never carries an empty "0 failures" footer.
    fn prose(&self) -> Option<String> {
        if !self.any() {
            return None;
        }
        let mut out = format!("\n[{} path(s) could not be searched:\n", self.total);
        for row in &self.rows {
            out.push_str(&format!(
                "  {} — {}\n",
                row["path"].as_str().unwrap_or("?"),
                row["reason"].as_str().unwrap_or("")
            ));
        }
        if self.total > self.rows.len() {
            out.push_str(&format!("  ...and {} more\n", self.total - self.rows.len()));
        }
        out.push_str("]\n");
        Some(out)
    }
}

/// A glob plus how to apply it.
///
/// A pattern with no `/` in it is matched against the *file name*, so `*.rs`
/// means "every Rust file" rather than "a Rust file at the workspace root".
/// The second reading is what a path glob literally says and is essentially
/// never what a caller means.
struct PathGlob {
    pattern: glob::Pattern,
    bare: bool,
}

impl PathGlob {
    fn new(pattern: &str, what: &str) -> Result<Self, ToolResult> {
        Ok(Self {
            bare: !pattern.contains('/'),
            pattern: glob::Pattern::new(pattern).map_err(|e| {
                ToolResult::err_code(
                    ErrorCode::GlobInvalid,
                    format!("invalid {what} glob {pattern:?}: {e}"),
                )
            })?,
        })
    }

    fn matches(&self, rel: &str) -> bool {
        if self.pattern.matches_path(Path::new(rel)) {
            return true;
        }
        if !self.bare {
            return false;
        }
        Path::new(rel)
            .file_name()
            .is_some_and(|n| self.pattern.matches(&n.to_string_lossy()))
    }
}

/// Walk every file under `start`, depth-first and in sorted order, calling `f`
/// with the absolute path and its path relative to `base`.
///
/// `f` returns [`ControlFlow::Break`] to stop the walk immediately — that is
/// the whole point of the shape (see the section note above).
///
/// Three classes of path are never visited:
///
///   * noise directories ([`WALK_SKIP_DIRS`]), which are cost without meaning;
///   * **every** symlink. A symlink inside the workspace can resolve to a file
///     outside it, and `resolve_path`'s containment check cannot help here
///     because the walk builds these paths itself — so the cheapest sound
///     answer is not to follow them at all;
///   * sensitive paths, by the same rule the read tools honour. A walk that
///     could reach `.env` would make `grep` a way *around* the sensitive-path
///     gate that every other reader goes through.
///
/// Returns the number of files visited.
fn walk_files(
    start: &Path,
    base: &Path,
    f: &mut impl FnMut(&Path, &str) -> ControlFlow<()>,
) -> usize {
    fn rec(
        dir: &Path,
        base: &Path,
        depth: usize,
        f: &mut impl FnMut(&Path, &str) -> ControlFlow<()>,
        visited: &mut usize,
    ) -> ControlFlow<()> {
        if depth > WALK_MAX_DEPTH {
            return ControlFlow::Continue(());
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return ControlFlow::Continue(());
        };
        // Sorted so two runs over an unchanged tree report the same order, and
        // so a truncated result is at least deterministic.
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            // `symlink_metadata` is what *enforces* the no-follow rule below:
            // it reports the link itself, so a symlink is neither `is_dir()`
            // nor `is_file()` and cannot be descended into or read. Swapping
            // this for `metadata` (which follows) is a one-word change that
            // opens the hole — `the_walk_does_not_follow_a_symlink_out_of_the_
            // workspace` fails on exactly that mutation.
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            // Redundant given the above, and kept deliberately: it states the
            // rule rather than leaving it to follow from two type checks, and
            // it is what would still hold if the call above were ever
            // changed to `metadata`.
            if meta.file_type().is_symlink() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(base) else {
                continue;
            };
            let rel = rel.to_string_lossy().into_owned();
            if is_sensitive_path(Path::new(&rel)) {
                continue;
            }
            if meta.is_dir() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if WALK_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                if rec(&path, base, depth + 1, f, visited).is_break() {
                    return ControlFlow::Break(());
                }
            } else if meta.is_file() {
                *visited += 1;
                if f(&path, &rel).is_break() {
                    return ControlFlow::Break(());
                }
            }
        }
        ControlFlow::Continue(())
    }

    let mut visited = 0usize;
    let _ = rec(start, base, 0, f, &mut visited);
    visited
}

/// What a search runs over: one file, or a tree.
///
/// `grep` and `glob` share this so both accept a file where a directory would
/// go — `grep path: Cargo.toml` is a reasonable thing to ask for.
enum SearchTarget {
    File(PathBuf),
    Tree(PathBuf),
}

impl SearchTarget {
    /// Resolve the optional `path` argument against the workspace root.
    fn resolve(root: &Path, path: Option<&str>) -> Result<Self, ToolResult> {
        let resolved = match path {
            None => root.to_path_buf(),
            Some(p) => resolve_tool_path(root, p)?,
        };
        if resolved.is_file() {
            Ok(SearchTarget::File(resolved))
        } else if resolved.is_dir() {
            Ok(SearchTarget::Tree(resolved))
        } else {
            Err(ToolResult::err_code(
                ErrorCode::NotADirectory,
                format!("no such file or directory: {}", path.unwrap_or(".")),
            ))
        }
    }

    /// Visit every file to search. Returns the number visited.
    fn visit(&self, base: &Path, f: &mut impl FnMut(&Path, &str) -> ControlFlow<()>) -> usize {
        match self {
            // A single file needs no sensitive-path check: it arrived through
            // the `path` argument, which `tool_paths` reports and the approval
            // gate already screened. The walk-time filter exists for the files
            // the caller never named.
            SearchTarget::File(p) => {
                let Ok(rel) = p.strip_prefix(base) else {
                    return 0;
                };
                let rel = rel.to_string_lossy().into_owned();
                let _ = f(p, &rel);
                1
            }
            SearchTarget::Tree(dir) => walk_files(dir, base, f),
        }
    }
}

/// Read a file for searching.
///
/// `Err(reason)` says why it could not be searched; the caller decides whether
/// that is worth reporting. A walk that stopped at the first binary file would
/// be useless, and one that reported every binary file would bury the real
/// permission errors — so the reason is returned rather than judged here.
fn read_for_search(p: &Path) -> Result<String, String> {
    let md = std::fs::metadata(p).map_err(|e| e.to_string())?;
    if md.len() > SEARCH_FILE_CAP {
        return Err(format!("too large to search ({} bytes)", md.len()));
    }
    let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
    if bytes.contains(&0) {
        return Err("binary".to_string());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// What `grep` should report. A property of the request, not of the tree —
/// choosing the mode is how a caller avoids paying for output it will discard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrepMode {
    /// Matching lines, with path and line number.
    Content,
    /// Just the files that contain a match.
    FilesWithMatches,
    /// A match count per file.
    Count,
}

/// A validated, compiled `grep` request. Building one touches no files.
struct GrepPlan {
    regex: regex::Regex,
    include: Option<PathGlob>,
    exclude: Option<PathGlob>,
    mode: GrepMode,
    max_results: usize,
}

/// The pure half of `grep`: every way a caller can be wrong about the request
/// is caught here, before any directory is opened.
fn plan_grep(
    pattern: &str,
    include: Option<&str>,
    exclude: Option<&str>,
    mode: Option<GrepMode>,
    max_results: Option<u32>,
) -> Result<GrepPlan, ToolResult> {
    let regex = regex::Regex::new(pattern).map_err(|e| {
        ToolResult::err_code(
            ErrorCode::RegexInvalid,
            format!("invalid regex {pattern:?}: {e}"),
        )
    })?;
    Ok(GrepPlan {
        regex,
        include: include.map(|p| PathGlob::new(p, "include")).transpose()?,
        exclude: exclude.map(|p| PathGlob::new(p, "exclude")).transpose()?,
        mode: mode.unwrap_or(GrepMode::Content),
        // Clamped rather than refused: a caller asking for more than the
        // ceiling gets the ceiling and a `truncated` flag, which is more
        // useful than an error telling it to guess a smaller number.
        max_results: max_results
            .map(|n| (n.max(1) as usize).min(GREP_MAX_RESULTS))
            .unwrap_or(GREP_MAX_RESULTS),
    })
}

/// The effectful half of `grep`.
fn run_grep(plan: &GrepPlan, target: &SearchTarget, base: &Path) -> ToolResult {
    let mut rows: Vec<serde_json::Value> = Vec::new();
    let mut files: Vec<serde_json::Value> = Vec::new();
    let mut failures = Failures::new();
    let mut truncated = false;
    let mut unsearchable = 0usize;

    let visited = target.visit(base, &mut |path, rel| {
        if let Some(inc) = &plan.include {
            if !inc.matches(rel) {
                return ControlFlow::Continue(());
            }
        }
        if let Some(exc) = &plan.exclude {
            if exc.matches(rel) {
                return ControlFlow::Continue(());
            }
        }
        let text = match read_for_search(path) {
            Ok(t) => t,
            Err(reason) => {
                // Binary and oversized files are the ordinary shape of a walk.
                // Counting them keeps a search over a tree of binaries honest
                // without listing each one as though it were a problem.
                if reason.starts_with("too large") || reason == "binary" {
                    unsearchable += 1;
                } else {
                    failures.push(rel, reason);
                }
                return ControlFlow::Continue(());
            }
        };
        match plan.mode {
            GrepMode::Content => {
                for (i, line) in text.lines().enumerate() {
                    if !plan.regex.is_match(line) {
                        continue;
                    }
                    if rows.len() >= plan.max_results {
                        truncated = true;
                        return ControlFlow::Break(());
                    }
                    rows.push(serde_json::json!({
                        "path": rel,
                        "line": i + 1,
                        "text": clip_chars(line.trim_end(), GREP_LINE_CHARS),
                    }));
                }
            }
            GrepMode::FilesWithMatches | GrepMode::Count => {
                let count = text.lines().filter(|l| plan.regex.is_match(l)).count();
                if count == 0 {
                    return ControlFlow::Continue(());
                }
                if files.len() >= plan.max_results {
                    truncated = true;
                    return ControlFlow::Break(());
                }
                files.push(serde_json::json!({ "path": rel, "count": count }));
            }
        }
        ControlFlow::Continue(())
    });

    // The prose half mirrors the mode, so a caller that ignores the structured
    // payload still gets the shape it asked for rather than everything.
    let mut out = String::new();
    for row in &rows {
        out.push_str(&format!(
            "{}:{}: {}\n",
            row["path"].as_str().unwrap_or(""),
            row["line"],
            row["text"].as_str().unwrap_or("")
        ));
    }
    for row in &files {
        match plan.mode {
            GrepMode::Count => out.push_str(&format!(
                "{}: {}\n",
                row["path"].as_str().unwrap_or(""),
                row["count"]
            )),
            _ => out.push_str(&format!("{}\n", row["path"].as_str().unwrap_or(""))),
        }
    }

    let hits = rows.len() + files.len();
    if hits == 0 {
        out.push_str(&format!("no matches in {visited} file(s) scanned\n"));
    } else if truncated {
        out.push_str(&format!(
            "\n[stopped at the {}-result cap; refine the pattern or raise \
             max_results]\n",
            plan.max_results
        ));
    }
    if unsearchable > 0 {
        out.push_str(&format!(
            "[{unsearchable} binary or oversized file(s) not searched]\n"
        ));
    }
    if let Some(note) = failures.prose() {
        out.push_str(&note);
    }

    ToolResult::ok_structured(
        out,
        serde_json::json!({
            "mode": plan.mode,
            "matches": rows,
            "files": files,
            "scanned": visited,
            "truncated": truncated,
            "failures": failures.json(),
        }),
    )
}

/// The whole of `glob`: a target, a pattern, and a cap.
fn run_glob(pattern: &PathGlob, target: &SearchTarget, base: &Path, max: usize) -> ToolResult {
    let mut paths: Vec<String> = Vec::new();
    let mut truncated = false;

    let visited = target.visit(base, &mut |_path, rel| {
        if !pattern.matches(rel) {
            return ControlFlow::Continue(());
        }
        if paths.len() >= max {
            truncated = true;
            return ControlFlow::Break(());
        }
        paths.push(rel.to_string());
        ControlFlow::Continue(())
    });

    let mut out = if paths.is_empty() {
        format!("no path matches, {visited} file(s) scanned\n")
    } else {
        let mut s = paths.join("\n");
        s.push('\n');
        s
    };
    if truncated {
        out.push_str(&format!("\n[stopped at the {max}-result cap]\n"));
    }

    ToolResult::ok_structured(
        out,
        serde_json::json!({
            "paths": paths,
            "scanned": visited,
            "truncated": truncated,
        }),
    )
}

/// Clip to `max` characters on a char boundary, marking that it happened.
fn clip_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let clipped: String = s.chars().take(max).collect();
    format!("{clipped}…")
}

/// Create a channel a remote caller can wait on for approval resolution.
pub fn wait_channel() -> (SyncSender<ToolResult>, Receiver<ToolResult>) {
    sync_channel(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_paths_are_detected() {
        for p in [
            ".env",
            ".env.local",
            "config/id_rsa",
            "certs/server.pem",
            "secrets.json",
            ".git/config",
            "credentials.txt",
            "api_key.txt",
        ] {
            assert!(is_sensitive_path(Path::new(p)), "{p} should be sensitive");
        }
        for p in ["src/main.ts", "README.md", "package.json", "docs/notes.txt"] {
            assert!(!is_sensitive_path(Path::new(p)), "{p} should be safe");
        }
    }

    #[test]
    fn approval_policy() {
        assert!(needs_approval(&Tool::ReadFile {
            path: "src/a.ts".into(),
            offset: None,
            limit: None,
        })
        .is_none());
        assert!(needs_approval(&Tool::ReadFile {
            path: ".env".into(),
            offset: None,
            limit: None,
        })
        .is_some());
        assert!(needs_approval(&Tool::WriteFile {
            path: "a.ts".into(),
            content: "x".into()
        })
        .is_some());
        assert!(needs_approval(&Tool::RunCommand {
            command: "echo hi".into()
        })
        .is_some());
        assert!(needs_approval(&Tool::ListDirectory { path: ".".into() }).is_none());
        assert!(needs_approval(&Tool::GitStatus).is_none());

        // Phase 1 tools.
        assert!(needs_approval(&Tool::EditFile {
            path: "src/a.ts".into(),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: None,
        })
        .is_none());
        assert!(needs_approval(&Tool::EditFile {
            path: ".env".into(),
            old_string: "x".into(),
            new_string: "y".into(),
            replace_all: None,
        })
        .is_some());
        assert!(needs_approval(&Tool::MultiEdit {
            path: "src/a.ts".into(),
            edits: vec![],
        })
        .is_none());
        assert!(needs_approval(&Tool::ApplyPatch {
            path: "a.ts".into(),
            patch: String::new(),
        })
        .is_some());
        assert!(needs_approval(&Tool::DeleteFile {
            path: "a.ts".into()
        })
        .is_some());
        assert!(needs_approval(&Tool::MoveFile {
            from: "a".into(),
            to: "b".into()
        })
        .is_some());
        assert!(needs_approval(&Tool::CopyFile {
            from: "a".into(),
            to: "b".into()
        })
        .is_some());
        assert!(needs_approval(&Tool::CreateDirectory { path: "d".into() }).is_none());
        assert!(needs_approval(&Tool::ReadManyFiles {
            paths: vec!["a".into()]
        })
        .is_none());
    }

    #[test]
    fn path_pair_tools_report_both_paths() {
        // A secret laundered by copying it to an innocuous name must still
        // trip the sensitive-path gate: both sides are reported.
        let t = Tool::CopyFile {
            from: "notes.txt".into(),
            to: "secrets.txt".into(),
        };
        assert_eq!(tool_paths(&t), vec!["notes.txt", "secrets.txt"]);
        assert!(tool_paths(&t)
            .iter()
            .any(|p| is_sensitive_path(Path::new(p))));

        let t = Tool::MoveFile {
            from: ".env".into(),
            to: "notes.txt".into(),
        };
        assert!(tool_paths(&t)
            .iter()
            .any(|p| is_sensitive_path(Path::new(p))));
    }

    #[test]
    fn resolve_path_rejects_escape() {
        let dir = std::env::temp_dir();
        assert!(resolve_path(&dir, "../outside").is_err());
        #[cfg(windows)]
        assert!(resolve_path(&dir, "..\\outside").is_err());
        assert!(resolve_path(&dir, "sub").is_ok());
    }

    #[test]
    fn resolve_path_rejects_dotdot_on_missing_target() {
        // The fallback branch (target doesn't exist yet) used to re-join
        // the raw rel, leaving literal `..` components that the
        // component-wise starts_with check does not resolve — so this
        // wrote to <home>/.bashrc while claiming to stay in the root.
        let dir = std::env::temp_dir();
        assert!(resolve_path(&dir, "notes/../../../.bashrc").is_err());
        assert!(resolve_path(&dir, "a/b/../../../outside.txt").is_err());
        assert!(resolve_path(&dir, "/etc/passwd-ish").is_err());
        // `..` that cancels out inside the root is still fine.
        let ok = resolve_path(&dir, "a/../b.txt").unwrap();
        assert!(ok.ends_with("b.txt"), "{ok:?}");
        assert!(ok.starts_with(dir.canonicalize().unwrap()));
    }

    #[test]
    fn resolve_path_new_file_in_new_dir_stays_in_root() {
        // write_file's common legit case: deepest existing ancestor is the
        // root itself, remainder creates new directories.
        let dir = std::env::temp_dir();
        let p = resolve_path(&dir, "new_dir/nested/file.txt").unwrap();
        assert!(p.ends_with("new_dir/nested/file.txt"));
        assert!(p.starts_with(dir.canonicalize().unwrap()));
    }

    #[test]
    fn execution_roundtrip() {
        let dir = std::env::temp_dir().join(format!("bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        // write -> read roundtrip
        let r = execute(
            &Tool::WriteFile {
                path: "hello.txt".into(),
                content: "bridge hi".into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        let r = execute(
            &Tool::ReadFile {
                path: "hello.txt".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        assert!(r.ok);
        // Single-chunk reads are numbered but carry no footer.
        assert_eq!(r.output.as_deref(), Some("   1| bridge hi\n"));

        // list directory
        let r = execute(&Tool::ListDirectory { path: ".".into() }, Some(&dir), None);
        assert!(r.ok);
        assert!(r.output.unwrap().contains("hello.txt"));

        // command
        let mut events = Vec::new();
        let r = execute(
            &Tool::RunCommand {
                command: "echo pty-ok".into(),
            },
            Some(&dir),
            Some(&mut |event| events.push(format!("{event:?}"))),
        );
        assert!(r.ok, "{:?}", r.error);
        assert!(r.output.unwrap().contains("pty-ok"));
        assert!(
            events.iter().any(|e| e.starts_with("Start")),
            "expected a Start event, got {events:?}"
        );
        assert!(
            events.iter().any(|e| e.starts_with("Exit")),
            "expected an Exit event, got {events:?}"
        );

        // escape rejected
        let r = execute(
            &Tool::ReadFile {
                path: "../../etc/hosts".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        assert!(!r.ok);

        // missing root
        assert!(!execute(&Tool::GitStatus, None, None).ok);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn approval_flow_resolves() {
        let dir = std::env::temp_dir().join(format!("bridge-approve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bridge = Bridge::new();

        let (result, id) = bridge.submit(
            Tool::RunCommand {
                command: "echo approved".into(),
            },
            "web",
            Some(&dir),
        );
        assert!(result.pending.is_some());
        let id = id.unwrap();

        let (tx, rx) = wait_channel();
        bridge.channels.lock().unwrap().insert(id, tx);

        let (r, req) = bridge.resolve(id, true, Some(&dir), None).unwrap();
        assert!(r.ok);
        assert_eq!(req.id, id);
        assert!(rx.recv_timeout(Duration::from_secs(2)).unwrap().ok);
        assert!(bridge.pending.lock().unwrap().is_empty());

        // denial
        let (result, id) = bridge.submit(
            Tool::RunCommand {
                command: "echo denied".into(),
            },
            "web",
            Some(&dir),
        );
        assert!(result.pending.is_some());
        let (r, _) = bridge
            .resolve(id.unwrap(), false, Some(&dir), None)
            .unwrap();
        assert!(!r.ok);
        assert!(r.error.unwrap().contains("denied"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A gated command is approved on the desktop's own thread, long after the
    /// asking thread has returned. If the owner were not carried with the
    /// request, the PTY would register with no owner and `cancel_request`
    /// could never reach it — so a caller that gave up could not stop the
    /// command it caused to run.
    #[test]
    fn an_approved_command_stays_owned_by_the_request_that_asked() {
        let dir = temp_project("ownership");
        let bridge = Bridge::new();

        // Asked for by a request… (the question is asked from the caller's
        // thread, which is all `execution_owner` reads).
        let (result, id) = {
            let _asking = crate::process::own_current_thread(Some("req_owner".into()));
            bridge.submit(
                Tool::RunCommand {
                    command: "sleep 30".into(),
                },
                SOURCE_MCP,
                Some(&dir),
            )
        };
        assert!(result.pending.is_some());
        let id = id.unwrap();
        assert_eq!(
            bridge.pending.lock().unwrap()[0].owner.as_deref(),
            Some("req_owner"),
            "the queue must remember who asked"
        );

        std::thread::scope(|scope| {
            // …and approved from one that owns nothing, as Tauri's command
            // thread does.
            let approving = scope.spawn(|| {
                assert!(crate::process::execution_owner().is_none());
                let _ = bridge.resolve(id, true, Some(&dir), None);
            });

            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut owned = Vec::new();
            while std::time::Instant::now() < deadline {
                owned = crate::process::registry().by_owner("req_owner");
                if !owned.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            assert_eq!(
                owned.len(),
                1,
                "the approved command must register under the request that asked"
            );
            assert_eq!(
                crate::process::registry().kill_owner("req_owner", Duration::from_millis(200)),
                1
            );
            approving.join().unwrap();
        });

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Fresh temp dir unique to this test (tests run in parallel; each
    /// caller passes its own tag).
    fn temp_project(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bridge-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn edit_file_replaces_exactly_once() {
        let dir = temp_project("edit");
        execute(
            &Tool::WriteFile {
                path: "a.txt".into(),
                content: "alpha\nbeta\ngamma\n".into(),
            },
            Some(&dir),
            None,
        );

        let r = execute(
            &Tool::EditFile {
                path: "a.txt".into(),
                old_string: "beta".into(),
                new_string: "BETA".into(),
                replace_all: None,
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        let read = execute(
            &Tool::ReadFile {
                path: "a.txt".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        assert!(read.output.unwrap().contains("BETA"));

        // Not found.
        let r = execute(
            &Tool::EditFile {
                path: "a.txt".into(),
                old_string: "delta".into(),
                new_string: "x".into(),
                replace_all: None,
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::StringNotFound));

        // Ambiguous without replace_all.
        execute(
            &Tool::WriteFile {
                path: "b.txt".into(),
                content: "x x x\n".into(),
            },
            Some(&dir),
            None,
        );
        let r = execute(
            &Tool::EditFile {
                path: "b.txt".into(),
                old_string: "x".into(),
                new_string: "y".into(),
                replace_all: None,
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::AmbiguousMatch));
        assert!(r.error.unwrap().contains("3"), "message names the count");

        // replace_all resolves it.
        let r = execute(
            &Tool::EditFile {
                path: "b.txt".into(),
                old_string: "x".into(),
                new_string: "y".into(),
                replace_all: Some(true),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok);

        // Empty old_string is refused.
        let r = execute(
            &Tool::EditFile {
                path: "b.txt".into(),
                old_string: String::new(),
                new_string: "y".into(),
                replace_all: None,
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::InvalidArguments));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn multi_edit_is_atomic() {
        let dir = temp_project("multiedit");
        execute(
            &Tool::WriteFile {
                path: "a.txt".into(),
                content: "one\ntwo\nthree\nfour\n".into(),
            },
            Some(&dir),
            None,
        );

        // The second edit targets a string that does not exist — the batch
        // fails and the file must be exactly as it was (not half-edited).
        let r = execute(
            &Tool::MultiEdit {
                path: "a.txt".into(),
                edits: vec![
                    Edit {
                        old_string: "two".into(),
                        new_string: "TWO".into(),
                        replace_all: None,
                    },
                    Edit {
                        old_string: "THREE".into(),
                        new_string: "3".into(),
                        replace_all: None,
                    },
                ],
            },
            Some(&dir),
            None,
        );
        assert!(!r.ok);
        let read = execute(
            &Tool::ReadFile {
                path: "a.txt".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        let out = read.output.unwrap();
        assert!(
            out.contains("two") && !out.contains("TWO"),
            "batch not atomic: {out}"
        );

        // A valid sequential batch applies in order.
        let r = execute(
            &Tool::MultiEdit {
                path: "a.txt".into(),
                edits: vec![
                    Edit {
                        old_string: "two".into(),
                        new_string: "THREE".into(),
                        replace_all: None,
                    },
                    Edit {
                        old_string: "THREE".into(),
                        new_string: "3".into(),
                        replace_all: None,
                    },
                ],
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        let read = execute(
            &Tool::ReadFile {
                path: "a.txt".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        let out = read.output.unwrap();
        assert!(out.contains("3") && !out.contains("two"));

        // An empty batch is rejected outright.
        let r = execute(
            &Tool::MultiEdit {
                path: "a.txt".into(),
                edits: vec![],
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::InvalidArguments));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_patch_roundtrip() {
        let dir = temp_project("patch");
        execute(
            &Tool::WriteFile {
                path: "a.txt".into(),
                content: "one\ntwo\nthree\nfour\nfive\n".into(),
            },
            Some(&dir),
            None,
        );

        // A clean patch with headers and context.
        let patch =
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,5 +1,5 @@\n one\n-two\n+TWO\n three\n four\n five\n";
        let r = execute(
            &Tool::ApplyPatch {
                path: "a.txt".into(),
                patch: patch.into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        let read = execute(
            &Tool::ReadFile {
                path: "a.txt".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        assert!(read.output.unwrap().contains("TWO"));

        // Wrong context → PATCH_DOES_NOT_APPLY, file untouched.
        let bad = "@@ -1,3 +1,3 @@\n one\n nonexistent context\n three\n";
        let r = execute(
            &Tool::ApplyPatch {
                path: "a.txt".into(),
                patch: bad.into(),
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::PatchDoesNotApply));
        let read = execute(
            &Tool::ReadFile {
                path: "a.txt".into(),
                offset: None,
                limit: None,
            },
            Some(&dir),
            None,
        );
        assert!(read.output.unwrap().contains("TWO"));

        // A stale offset (line numbers far off) still applies via the
        // drift search.
        let drifted = "@@ -9,2 +9,2 @@\n four\n-five\n+FIVE\n";
        let r = execute(
            &Tool::ApplyPatch {
                path: "a.txt".into(),
                patch: drifted.into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);

        // No hunks at all.
        let r = execute(
            &Tool::ApplyPatch {
                path: "a.txt".into(),
                patch: "just some prose".into(),
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::InvalidArguments));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_management_tools_roundtrip() {
        let dir = temp_project("fileops");
        execute(
            &Tool::WriteFile {
                path: "src/deep/a.txt".into(),
                content: "content".into(),
            },
            Some(&dir),
            None,
        );

        // create_directory, parents included.
        let r = execute(
            &Tool::CreateDirectory {
                path: "x/y/z".into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok);
        assert!(dir.join("x/y/z").is_dir());

        // copy_file into a fresh nested target.
        let r = execute(
            &Tool::CopyFile {
                from: "src/deep/a.txt".into(),
                to: "x/y/b.txt".into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        assert!(dir.join("x/y/b.txt").exists());

        // move_file overwrites the target and creates parents.
        execute(
            &Tool::WriteFile {
                path: "victim.txt".into(),
                content: "old".into(),
            },
            Some(&dir),
            None,
        );
        let r = execute(
            &Tool::MoveFile {
                from: "x/y/b.txt".into(),
                to: "victim.txt".into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        assert!(!dir.join("x/y/b.txt").exists());
        assert_eq!(
            std::fs::read_to_string(dir.join("victim.txt")).unwrap(),
            "content"
        );

        // delete_file removes a file, refuses a directory, refuses escape.
        let r = execute(
            &Tool::DeleteFile {
                path: "victim.txt".into(),
            },
            Some(&dir),
            None,
        );
        assert!(r.ok);
        assert!(!dir.join("victim.txt").exists());

        let r = execute(&Tool::DeleteFile { path: "src".into() }, Some(&dir), None);
        assert_eq!(r.error_code, Some(ErrorCode::InvalidArguments));

        let r = execute(
            &Tool::DeleteFile {
                path: "../../outside.txt".into(),
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::PathEscapesRoot));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_many_files_batches_and_skips() {
        let dir = temp_project("readmany");
        for (name, content) in [
            ("a.txt", "alpha\n"),
            ("b.txt", "beta\n"),
            (".env", "SECRET=1\n"),
        ] {
            execute(
                &Tool::WriteFile {
                    path: name.into(),
                    content: content.into(),
                },
                Some(&dir),
                None,
            );
        }

        let r = execute(
            &Tool::ReadManyFiles {
                paths: vec![
                    "a.txt".into(),
                    "b.txt".into(),
                    ".env".into(),
                    "missing.txt".into(),
                ],
            },
            Some(&dir),
            None,
        );
        assert!(r.ok, "{:?}", r.error);
        let out = r.output.unwrap();
        assert!(out.contains("── a.txt ──"), "{out}");
        assert!(out.contains("alpha"));
        assert!(out.contains("beta"));
        assert!(out.contains("skipped: sensitive"), ".env must be skipped");
        assert!(!out.contains("SECRET=1"));
        assert!(out.contains("missing.txt"), "the miss is named, not silent");
        assert!(out.contains("[2 of 4 files shown]"));

        // Over the batch cap.
        let r = execute(
            &Tool::ReadManyFiles {
                paths: (0..21).map(|i| format!("f{i}.txt")).collect(),
            },
            Some(&dir),
            None,
        );
        assert_eq!(r.error_code, Some(ErrorCode::InvalidArguments));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_grants_matrix() {
        let dir = temp_project("grants");
        // Seeds — the sensitive-copy case needs a real source file to exist.
        for (path, content) in [
            ("src/a.txt", "x"),
            ("root.txt", "x"),
            ("src/secrets.txt", "x"),
        ] {
            execute(
                &Tool::WriteFile {
                    path: path.into(),
                    content: content.into(),
                },
                Some(&dir),
                None,
            );
        }
        let bridge = Bridge::new();
        // A non-sensitive edit is SensitivePathOnly: it runs without asking.
        let (r, _, how) = bridge.submit_with_audit(
            Tool::EditFile {
                path: "src/a.txt".into(),
                old_string: "x".into(),
                new_string: "y".into(),
                replace_all: None,
            },
            "web",
            Some(&dir),
        );
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(how, "auto", "edits only ask on sensitive paths");

        // Grants exercise the Always-class tools in the Editing group
        // (write_file / apply_patch / copy_file) — the ones that ask on
        // every call and can legally be covered.
        let write = |path: &str| Tool::WriteFile {
            path: path.into(),
            content: "y".into(),
        };

        // Without a grant, an Always-gated write asks.
        let (r, id, how) = bridge.submit_with_audit(write("src/b.txt"), "web", Some(&dir));
        assert!(r.pending.is_some());
        assert_eq!(how, "pending");
        bridge.resolve(id.unwrap(), false, Some(&dir), None); // discard

        // An editing grant under src/ auto-approves an Always write there…
        bridge.grant_add(GrantScope::Editing, Some("src".into()), "web");
        let (r, id, how) = bridge.submit_with_audit(write("src/c.txt"), "web", Some(&dir));
        assert!(r.ok, "{:?}", r.error);
        assert!(id.is_none());
        assert_eq!(how, "grant:editing:src");

        // …but not outside the prefix.
        let (r, _, _) = bridge.submit_with_audit(write("root.txt"), "web", Some(&dir));
        assert!(r.pending.is_some(), "prefix must confine the grant");

        // …not for the desktop source.
        let (r, _, _) = bridge.submit_with_audit(write("src/d.txt"), "desktop", Some(&dir));
        assert!(r.pending.is_some(), "grants are source-scoped");

        // …never for destructive tools.
        let (r, _, _) = bridge.submit_with_audit(
            Tool::DeleteFile {
                path: "src/a.txt".into(),
            },
            "web",
            Some(&dir),
        );
        assert!(r.pending.is_some(), "destructive never auto-approves");

        // …and never on a sensitive path (secret laundering).
        let (r, _, _) = bridge.submit_with_audit(
            Tool::CopyFile {
                from: "src/secrets.txt".into(),
                to: "src/notes.txt".into(),
            },
            "web",
            Some(&dir),
        );
        assert!(r.pending.is_some(), "sensitive paths bypass grants");

        // A `..`-laden path can't widen the prefix.
        let (r, _, _) = bridge.submit_with_audit(write("src/../root.txt"), "web", Some(&dir));
        assert!(r.pending.is_some(), "a .. escape must not match the grant");

        // Kill switch: revoke + pause.
        bridge.set_paused(true);
        assert!(bridge.grants.lock().unwrap().is_empty());
        let (r, _, _) = bridge.submit_with_audit(write("src/e.txt"), "web", Some(&dir));
        assert_eq!(r.error_code, Some(ErrorCode::BridgePaused));
        bridge.set_paused(false);
        let (r, _, _) = bridge.submit_with_audit(write("src/f.txt"), "web", Some(&dir));
        assert!(r.pending.is_some(), "unpause must not resurrect grants");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn destructive_cards_show_absolute_paths() {
        let dir = temp_project("cards");
        let s = describe_for_approval(
            &Tool::DeleteFile {
                path: "src/deep/a.txt".into(),
            },
            Some(&dir),
        );
        let abs = dir.canonicalize().unwrap().join("src/deep/a.txt");
        assert!(s.contains(abs.display().to_string().as_str()), "{s}");
        assert!(s.starts_with("delete_file "), "{s}");

        let s = describe_for_approval(
            &Tool::MoveFile {
                from: "a".into(),
                to: "b".into(),
            },
            Some(&dir),
        );
        assert!(s.contains("→"), "{s}");

        // Non-destructive tools keep the terse default; without a root the
        // raw path is shown rather than guessed at.
        assert_eq!(
            describe_for_approval(
                &Tool::WriteFile {
                    path: "a".into(),
                    content: "x".into()
                },
                Some(&dir)
            ),
            "write_file a (1 bytes)"
        );
        assert_eq!(
            describe_for_approval(&Tool::DeleteFile { path: "a".into() }, None),
            "delete_file a"
        );

        // What an approval card may offer as a grant.
        let g = grantable(&Tool::EditFile {
            path: "src/deep/a.ts".into(),
            old_string: String::new(),
            new_string: String::new(),
            replace_all: None,
        });
        assert_eq!(g, Some((GrantScope::Editing, Some("src/deep".to_string()))));
        assert_eq!(
            grantable(&Tool::RunCommand {
                command: "npm test".into()
            }),
            Some((GrantScope::Commands, None))
        );
        assert_eq!(
            grantable(&Tool::DeleteFile { path: "a".into() }),
            None,
            "destructive is never grantable"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One row per variant. Kept exhaustive by the `match` below, so adding
    /// a `Tool` variant without a `SPECS` row fails to compile here.
    fn every_variant() -> Vec<Tool> {
        let all = vec![
            Tool::ReadFile {
                path: "a".into(),
                offset: None,
                limit: None,
            },
            Tool::WriteFile {
                path: "a".into(),
                content: String::new(),
            },
            Tool::EditFile {
                path: "a".into(),
                old_string: "o".into(),
                new_string: "n".into(),
                replace_all: None,
            },
            Tool::MultiEdit {
                path: "a".into(),
                edits: vec![Edit {
                    old_string: "o".into(),
                    new_string: "n".into(),
                    replace_all: None,
                }],
            },
            Tool::ApplyPatch {
                path: "a".into(),
                patch: "@@ -1 +1 @@\n-o\n+n".into(),
            },
            Tool::DeleteFile { path: "a".into() },
            Tool::MoveFile {
                from: "a".into(),
                to: "b".into(),
            },
            Tool::CopyFile {
                from: "a".into(),
                to: "b".into(),
            },
            Tool::CreateDirectory { path: "d".into() },
            Tool::ReadManyFiles {
                paths: vec!["a".into()],
            },
            Tool::Grep {
                pattern: "x".into(),
                path: None,
                include: None,
                exclude: None,
                mode: None,
                max_results: None,
            },
            Tool::Glob {
                pattern: "*.rs".into(),
                path: None,
                max_results: None,
            },
            Tool::RunCommand {
                command: "true".into(),
            },
            Tool::RunCommandBackground {
                command: "true".into(),
            },
            Tool::CommandOutput {
                id: 1,
                cursor: None,
            },
            Tool::KillCommand { id: 1 },
            Tool::ListDirectory { path: ".".into() },
            Tool::GitStatus,
            Tool::GitDiff { path: None },
            Tool::GitLog { limit: None },
            Tool::GitAdd { path: None },
            Tool::GitUnstage { path: "a".into() },
            Tool::GitCommit {
                message: "m".into(),
            },
            Tool::GitBranches,
            Tool::GitCheckout { name: "b".into() },
            Tool::GitCreateBranch {
                name: "b".into(),
                base: None,
                checkout: None,
            },
            Tool::GitShow { oid: "HEAD".into() },
            Tool::GitCommitDiff { oid: "HEAD".into() },
            Tool::TodoWrite {
                todos: vec![TodoItem {
                    content: "do it".into(),
                    status: None,
                    active_form: None,
                }],
            },
            Tool::TodoRead,
            Tool::SetObjective {
                text: "ship it".into(),
            },
            Tool::RememberDecision {
                summary: "s".into(),
                reason: None,
            },
            Tool::RememberConstraint { text: "c".into() },
            Tool::RememberAttempt {
                description: "a".into(),
                succeeded: None,
            },
            Tool::GetFacts { session_id: None },
            Tool::ListSessions { limit: None },
            Tool::RequestHandoff {
                reason: "r".into(),
                next_step: None,
            },
            Tool::GetHandoff,
            Tool::DescribeTool {
                name: "read_file".into(),
            },
            Tool::ListTools,
        ];
        for t in &all {
            // Exhaustive: the compiler flags a new variant missing above.
            match t {
                Tool::ReadFile { .. }
                | Tool::WriteFile { .. }
                | Tool::EditFile { .. }
                | Tool::MultiEdit { .. }
                | Tool::ApplyPatch { .. }
                | Tool::DeleteFile { .. }
                | Tool::MoveFile { .. }
                | Tool::CopyFile { .. }
                | Tool::CreateDirectory { .. }
                | Tool::ReadManyFiles { .. }
                | Tool::Grep { .. }
                | Tool::Glob { .. }
                | Tool::RunCommand { .. }
                | Tool::RunCommandBackground { .. }
                | Tool::CommandOutput { .. }
                | Tool::KillCommand { .. }
                | Tool::ListDirectory { .. }
                | Tool::GitStatus
                | Tool::GitDiff { .. }
                | Tool::GitLog { .. }
                | Tool::GitAdd { .. }
                | Tool::GitUnstage { .. }
                | Tool::GitCommit { .. }
                | Tool::GitBranches
                | Tool::GitCheckout { .. }
                | Tool::GitCreateBranch { .. }
                | Tool::GitShow { .. }
                | Tool::GitCommitDiff { .. }
                | Tool::TodoWrite { .. }
                | Tool::TodoRead
                | Tool::SetObjective { .. }
                | Tool::RememberDecision { .. }
                | Tool::RememberConstraint { .. }
                | Tool::RememberAttempt { .. }
                | Tool::GetFacts { .. }
                | Tool::ListSessions { .. }
                | Tool::RequestHandoff { .. }
                | Tool::GetHandoff
                | Tool::DescribeTool { .. }
                | Tool::ListTools => {}
            }
        }
        all
    }

    #[test]
    fn every_variant_has_a_spec() {
        for tool in every_variant() {
            let s = spec(&tool); // panics when the row is missing
            assert!(
                GROUPS.contains(&s.group),
                "{}: group {:?} is not in GROUPS",
                s.name,
                s.group
            );
            assert!(!s.summary.is_empty(), "{}: needs a summary", s.name);
            assert!(s.timeout_ms > 0, "{}: needs a timeout", s.name);
        }
    }

    #[test]
    fn tool_names_and_aliases_are_unique() {
        let mut seen: Vec<&str> = Vec::new();
        for s in SPECS {
            for name in std::iter::once(&s.name).chain(s.aliases.iter()) {
                assert!(
                    !seen.contains(name),
                    "{name:?} is claimed twice — resolution would be order-dependent"
                );
                seen.push(*name);
            }
        }
    }

    #[test]
    fn aliases_resolve() {
        for (input, expected) in [
            ("read_file", "read_file"),
            ("Read", "read_file"),
            ("cat", "read_file"),
            ("bash", "run_command"),
            ("Shell", "run_command"),
            ("ls", "list_directory"),
            ("list-dir", "list_directory"),
            ("default_api.write_file", "write_file"),
            ("  git_status  ", "git_status"),
        ] {
            assert_eq!(
                spec_by_name(input).map(|s| s.name),
                Some(expected),
                "{input:?} should resolve to {expected}"
            );
        }
        assert!(spec_by_name("teleport").is_none());
    }

    #[test]
    fn meta_tools_need_no_project_root() {
        let r = execute(&Tool::ListTools, None, None);
        assert!(r.ok, "{:?}", r.error);
        let manifest = r.output.unwrap();
        for s in SPECS {
            assert!(manifest.contains(s.name), "manifest omits {}", s.name);
        }

        let r = execute(
            &Tool::DescribeTool {
                // A tool on the roadmap but not yet built. `web_search` is the
                // one this project has deliberately deferred, so it is the
                // name least likely to be implemented out from under this
                // assertion.
                name: "web_search".into(),
            },
            None,
            None,
        );
        assert!(!r.ok);
        assert!(
            r.error.unwrap().contains("list_tools"),
            "point the AI at a recovery path"
        );

        // An alias is enough to look a tool up.
        let r = execute(&Tool::DescribeTool { name: "cat".into() }, None, None);
        assert!(r.ok, "{:?}", r.error);
        assert!(r.output.unwrap().contains("read_file"));
    }

    #[test]
    fn describe_keeps_the_summary_format() {
        assert_eq!(
            describe(&Tool::ReadFile {
                path: "src/a.ts".into(),
                offset: None,
                limit: None,
            }),
            "read_file src/a.ts"
        );
        assert_eq!(
            describe(&Tool::WriteFile {
                path: "a.ts".into(),
                content: "abc".into()
            }),
            "write_file a.ts (3 bytes)"
        );
        assert_eq!(describe(&Tool::GitStatus), "git_status");
    }

    #[test]
    fn manifest_stays_small() {
        // Progressive disclosure: the manifest competes with real project
        // context in the AI's window, so keep it terse even at 44 tools.
        let manifest = tool_manifest();
        assert!(
            manifest.len() < 4_000,
            "manifest is {} bytes — move detail into describe_tool",
            manifest.len()
        );
    }

    #[test]
    fn manifest_and_describe_are_not_call_syntax() {
        // `list_tools` and `describe_tool` output goes straight into a model's
        // context, and the model may echo it back. In call syntax every row
        // was a live tool call: echoing the manifest ran the whole surface,
        // approval cards and all. Keep the text inert.
        let mut text = tool_manifest();
        for s in SPECS {
            text.push_str(&describe_spec(s));
        }
        for s in SPECS {
            assert!(
                !text.contains(&format!("{}(", s.name)),
                "{} is rendered in call syntax — it will execute when echoed",
                s.name
            );
        }
        // run_command's pattern makes the quote optional, so unquoted parens
        // anywhere in this text are enough to fire it.
        assert!(!text.contains("(shell command)"));
    }

    /// A command that runs until something stops it, in whatever shell this
    /// platform has.
    ///
    /// The background-command fixtures need a command that is still running
    /// when the next tool in SPECS order reads it — a command that had already
    /// finished would let `command_output` and `kill_command` pass while never
    /// exercising the states they exist for.
    fn background_sleeper() -> String {
        match Shell::detect() {
            Shell::PowerShell => "Start-Sleep -Seconds 30".to_string(),
            Shell::Cmd => "timeout /t 30 /nobreak".to_string(),
            _ => "sleep 30".to_string(),
        }
    }

    /// Args tuned so each tool actually succeeds against the workspace the two
    /// tests below build. `sample_args` only has to *parse*; these have to run,
    /// because the point is to inspect a real payload rather than skip every
    /// tool that happened to fail.
    ///
    /// Depends on SPECS order: the file tools build on each other (write, then
    /// edit, then move), the background-command trio builds on the handle the
    /// first of it returns, and both tests iterate `SPECS` in declaration
    /// order.
    /// Tools whose success depends on something a hermetic test cannot
    /// supply: the network (the web pair), an installed language server (the
    /// LSP four), or the desktop (the agent-loop pair and delegation — the
    /// test context carries no `AppHandle`, deliberately).
    ///
    /// They are still parsed and schema-checked by the surface-wide tests;
    /// only the executions that *require success* skip them. Every name here
    /// is asserted to be a real tool, so a rename cannot silently widen the
    /// skip.
    fn needs_outside_world(name: &str) -> bool {
        matches!(
            name,
            "web_fetch"
                | "web_search"
                | "lsp_diagnostics"
                | "lsp_definition"
                | "lsp_references"
                | "lsp_symbols"
                | "ask_user"
                | "propose_plan"
                | "delegate_task"
        )
    }

    fn succeeding_args(name: &str) -> serde_json::Value {
        match name {
            "read_file" => serde_json::json!({"path": "long.txt"}),
            "write_file" => serde_json::json!({"path": "a.txt", "content": "x"}),
            "edit_file" => {
                serde_json::json!({"path": "a.txt", "old_string": "x", "new_string": "y"})
            }
            "multi_edit" => serde_json::json!({
                "path": "a.txt",
                "edits": [{"old_string": "y", "new_string": "z"}]
            }),
            "apply_patch" => serde_json::json!({
                "path": "a.txt", "patch": "@@ -1 +1 @@\n-z\n+w"
            }),
            "delete_file" => serde_json::json!({"path": "b.txt"}),
            "move_file" => serde_json::json!({"from": "a.txt", "to": "moved.txt"}),
            "copy_file" => serde_json::json!({"from": "moved.txt", "to": "copy.txt"}),
            "create_directory" => serde_json::json!({"path": "d"}),
            "read_many_files" => serde_json::json!({"paths": ["moved.txt"]}),
            "run_command" => serde_json::json!({"command": "echo hi"}),
            // A command that runs until something stops it, so the trio below
            // is exercised for real rather than against a command that had
            // already finished: `command_output` reports a *running* process
            // and `kill_command` actually has to kill one. It also leaves no
            // work behind — the kill is the cleanup.
            "run_command_background" => serde_json::json!({"command": background_sleeper()}),
            // Handle 1, because this workspace's manager is fresh and the
            // command above is the only thing that has started anything. The
            // surface-wide tests walk SPECS in declaration order, which is
            // what makes the id knowable here — the same reliance the git
            // fixtures place on that order.
            "command_output" => serde_json::json!({"id": 1}),
            "kill_command" => serde_json::json!({"id": 1}),
            // Deliberately searches the *tree*, so the walk, the cap and the
            // structured payload are all exercised by the surface-wide tests.
            "grep" => serde_json::json!({"pattern": "alpha", "path": "."}),
            "glob" => serde_json::json!({"pattern": "*.txt"}),
            // The git fixtures lean on the workspace's one commit and its
            // `second` branch, and they run in SPECS order — so by the time
            // these fire the earlier editing tools have left the tree dirty,
            // which is what makes `git_diff` produce a patch rather than the
            // empty clean-tree answer. `git_add` stages everything so the
            // `git_unstage` that follows has a staged path to remove, and the
            // commit after that has something to record.
            "git_diff" => serde_json::json!({}),
            "git_log" => serde_json::json!({"limit": 5}),
            "git_add" => serde_json::json!({}),
            "git_unstage" => serde_json::json!({"path": "moved.txt"}),
            "git_commit" => serde_json::json!({"message": "test commit\n\nbody"}),
            "git_branches" => serde_json::json!({}),
            "git_checkout" => serde_json::json!({"name": "second"}),
            "git_create_branch" => serde_json::json!({"name": "third"}),
            "git_show" => serde_json::json!({"oid": "HEAD"}),
            "git_commit_diff" => serde_json::json!({"oid": "HEAD"}),
            // The memory fixture is one call per tool, in SPECS order, against
            // the same database: the write tools leave facts behind for the
            // reading tools that follow, so `get_facts` and `get_handoff`
            // report a session that actually has something in it.
            "todo_write" => serde_json::json!({
                "todos": [
                    {"content": "read the plan", "status": "completed"},
                    {"content": "write the tools", "status": "in_progress",
                     "active_form": "writing the tools"},
                ]
            }),
            "todo_read" => serde_json::json!({}),
            "set_objective" => serde_json::json!({"text": "land the memory tools"}),
            "remember_decision" => serde_json::json!({
                "summary": "keep the connector's own session row",
                "reason": "transcript sessions belong to another agent"
            }),
            "remember_constraint" => serde_json::json!({"text": "no new dependencies"}),
            "remember_attempt" => serde_json::json!({
                "description": "file facts under the newest session",
                "succeeded": false
            }),
            "get_facts" => serde_json::json!({}),
            "list_sessions" => serde_json::json!({"limit": 5}),
            "request_handoff" => serde_json::json!({"reason": "a decision is needed"}),
            "get_handoff" => serde_json::json!({}),
            // Phase 7–10. The ones that need the outside world are skipped by
            // `needs_outside_world`; these fixtures still parse so the
            // schema and path tests can reach them.
            "web_fetch" => serde_json::json!({"url": "ftp://example.invalid/"}),
            "web_search" => serde_json::json!({"query": ""}),
            "notebook_read" => serde_json::json!({"path": "nb.ipynb"}),
            "notebook_edit" => serde_json::json!({
                "path": "nb.ipynb", "cell_id": "code-1", "new_source": "print(2)\n"
            }),
            "delegate_task" => serde_json::json!({"task": "summarise"}),
            "lsp_diagnostics" => serde_json::json!({}),
            "lsp_definition" => serde_json::json!({"path": "a.txt", "line": 1, "character": 1}),
            "lsp_references" => serde_json::json!({
                "path": "a.txt", "line": 1, "character": 1, "include_declaration": true
            }),
            "lsp_symbols" => serde_json::json!({"path": "a.txt"}),
            "ask_user" => serde_json::json!({"question": "which?", "options": ["a", "b"]}),
            "propose_plan" => serde_json::json!({"plan": "do it", "steps": ["one"]}),
            "monitor" => serde_json::json!({"path": ".", "timeout_ms": 1000}),
            "notify" => serde_json::json!({"title": "t", "body": "b", "level": "info"}),
            "enter_worktree" => serde_json::json!({}),
            "exit_worktree" => serde_json::json!({"action": "discard"}),
            "read_media" => serde_json::json!({"path": "pic.png"}),
            "publish_artifact" => serde_json::json!({"path": "long.txt", "title": "log"}),
            "report_findings" => serde_json::json!({
                "summary": "one issue",
                "findings": [{"path": "a.txt", "line": 1, "severity": "warning", "claim": "x"}]
            }),
            other => sample_args(other),
        }
    }

    /// A workspace with a chunking-length file, a small file, and a git repo
    /// (so `git_status` succeeds and gets checked like everything else).
    fn structured_workspace(tag: &str) -> PathBuf {
        let dir = temp_project(tag);
        std::fs::write(dir.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        std::fs::write(dir.join("b.txt"), "one\ntwo\n").unwrap();
        let long: String = (1..=1000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("long.txt"), &long).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/nested.txt"), "nested\n").unwrap();
        // A notebook and a tiny image, so the Phase 7/10 content tools have
        // something real to read and edit rather than only an error path.
        std::fs::write(
            dir.join("nb.ipynb"),
            serde_json::json!({
                "cells": [
                    {"cell_type": "markdown", "source": ["# Fixture\n"], "metadata": {}},
                    {"cell_type": "code", "id": "code-1", "source": ["print(1)\n"], "outputs": [], "metadata": {}}
                ],
                "metadata": {"kernelspec": {"name": "python3"}},
                "nbformat": 4,
                "nbformat_minor": 5
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            dir.join("pic.png"),
            [0x89u8, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a],
        )
        .unwrap();
        let repo = git2::Repository::init(&dir).unwrap();
        // A configured identity and one commit, so the git tools have real
        // history to read. Without the config `git::commit` fails on
        // `signature()`, which would leave every commit-shaped tool exercised
        // only on its error path.
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Lexsus Test").unwrap();
            cfg.set_str("user.email", "test@example.invalid").unwrap();
        }
        git::stage_all(&repo).unwrap();
        let first = git::commit(&repo, "fixtures").unwrap();
        // A named second branch, so `git_checkout` has a destination that does
        // not depend on what libgit2 calls the initial branch.
        repo.branch("second", &repo.find_commit(first).unwrap(), false)
            .unwrap();
        dir
    }

    /// The structured workspace plus a live app state over a temporary
    /// database.
    ///
    /// The surface-wide properties iterate every tool in `SPECS`, and the
    /// memory tools have nowhere to read or write without a database — so the
    /// fixture supplies one. Without it they would be checked only on their
    /// "no database" error path, which is exactly the vacuity
    /// `every_tool_succeeds_on_its_succeeding_fixture` exists to catch.
    fn stateful_workspace(tag: &str) -> (PathBuf, crate::AppState) {
        let dir = structured_workspace(tag);
        let state = crate::test_state(&dir);
        (dir, state)
    }

    /// Drive every tool on the read-only *and* the write surface and check no
    /// result — success or failure — ever renders a tool name in call syntax.
    ///
    /// `manifest_and_describe_are_not_call_syntax` was too narrow to catch
    /// this: it only ever looked at the manifest and `describe_tool`, so the
    /// `read_file` chunking footer spelled out `read_file("big.txt", 401)` in
    /// tool *results* for as long as it liked. Echoing a result back is as
    /// easy as echoing a manifest, so every result the engine can produce is
    /// checked here.
    #[test]
    fn no_tool_output_reads_as_call_syntax() {
        let (dir, state) = stateful_workspace("call-syntax");
        let ctx = ToolCtx::with_state(Some(&dir), &state);

        for spec in SPECS {
            // Built through the parser, the same way the connector builds one.
            let tool = parse_tool_call(spec.name, &succeeding_args(spec.name))
                .unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            let result = execute_in(&tool, &ctx, None);
            for text in [result.output.as_deref(), result.error.as_deref()]
                .into_iter()
                .flatten()
            {
                for other in SPECS {
                    assert!(
                        !text.contains(&format!("{}(", other.name)),
                        "{}'s result renders {} in call syntax — echoing it \
                         back would run a tool:\n{text}",
                        spec.name,
                        other.name
                    );
                }
            }
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every tool succeeds on its `succeeding_args` fixture.
    ///
    /// The surface-wide properties below all *tolerate* failure, which is
    /// right — an error result needs checking too — but it means a fixture
    /// that quietly stopped reaching the tool would leave them green while
    /// testing the error path. This is the test that says the fixtures still
    /// exercise what they are named for; it caught `git_unstage` pointing at a
    /// path nothing had staged, and `git_checkout` needing a commit the
    /// workspace did not have.
    #[test]
    fn every_tool_succeeds_on_its_succeeding_fixture() {
        let (dir, state) = stateful_workspace("succeeds");
        let ctx = ToolCtx::with_state(Some(&dir), &state);

        let mut failed = Vec::new();
        for spec in SPECS {
            if needs_outside_world(spec.name) {
                continue;
            }
            let tool = parse_tool_call(spec.name, &succeeding_args(spec.name))
                .unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            let result = execute_in(&tool, &ctx, None);
            if !result.ok {
                failed.push(format!("{}: {:?}", spec.name, result.error));
            }
        }
        assert!(
            failed.is_empty(),
            "these tools no longer succeed on their fixture, so every other \
             property is checking their error path instead of their logic:\n{}",
            failed.join("\n")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `git_unstage` on a path with nothing staged says so instead of
    /// reporting success.
    ///
    /// `Index::remove_path` accepts a path that is not there, so the naive
    /// implementation returns `Ok` for a mistyped name — and, worse, for a
    /// committed file it stages that file's *deletion*. Refusing is both the
    /// honest answer and the useful one.
    #[test]
    fn git_unstage_says_when_there_is_nothing_to_unstage() {
        let dir = structured_workspace("unstage-clean");

        // `a.txt` is committed and untouched: in the index, but with no
        // staged change, which is the case the weak "is it in the index?"
        // check would have waved through.
        let tool = parse_tool_call("git_unstage", &serde_json::json!({"path": "a.txt"})).unwrap();
        let result = execute(&tool, Some(&dir), None);
        assert!(!result.ok, "an unchanged file is not something to unstage");
        assert_eq!(result.error_code, Some(ErrorCode::InvalidArguments));

        // And the refusal left the file alone rather than queueing its
        // removal, which is what `remove_path` would have done.
        let repo = git2::Repository::open(&dir).unwrap();
        assert!(!git::has_staged_change(&repo, "a.txt").unwrap());
        assert!(std::fs::read_to_string(dir.join("a.txt"))
            .unwrap()
            .contains("alpha"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Unstaging a real change puts the index back to HEAD and leaves the
    /// working tree exactly as it was.
    #[test]
    fn git_unstage_restores_the_committed_version() {
        let dir = structured_workspace("unstage-round-trip");
        std::fs::write(dir.join("a.txt"), "edited\n").unwrap();

        let add = parse_tool_call("git_add", &serde_json::json!({"path": "a.txt"})).unwrap();
        assert!(execute(&add, Some(&dir), None).ok);

        let staged = git2::Repository::open(&dir).unwrap();
        assert!(
            git::has_staged_change(&staged, "a.txt").unwrap(),
            "stage first"
        );

        let un = parse_tool_call("git_unstage", &serde_json::json!({"path": "a.txt"})).unwrap();
        assert!(execute(&un, Some(&dir), None).ok);

        // A fresh handle on purpose: libgit2 caches the index inside the
        // `Repository`, so a handle opened before the unstage can answer from
        // the pre-unstage index. Each tool call opens its own, which is why
        // this only ever bites the test.
        let after = git2::Repository::open(&dir).unwrap();
        assert!(
            !git::has_staged_change(&after, "a.txt").unwrap(),
            "unstage should return the index entry to HEAD's version"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("a.txt")).unwrap(),
            "edited\n",
            "unstage is index-only; the working tree must not be touched"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `git_add` takes a directory.
    ///
    /// `Index::add_path` answers "could not find 'sub' to stat" for a
    /// directory, which would make the ordinary request — stage this folder —
    /// fail for a reason the caller cannot act on. The pathspec form recurses.
    #[test]
    fn git_add_stages_a_directory() {
        let dir = structured_workspace("add-dir");
        // A file the fixture commit does not already contain. Staging is a
        // no-op for a path whose index entry and HEAD entry already agree, so
        // a settled file would let this pass without staging anything.
        std::fs::write(dir.join("sub/fresh.txt"), "fresh\n").unwrap();

        let tool = parse_tool_call("git_add", &serde_json::json!({"path": "sub"})).unwrap();
        let result = execute(&tool, Some(&dir), None);
        assert!(result.ok, "{:?}", result.error);

        let repo = git2::Repository::open(&dir).unwrap();
        assert!(
            git::has_staged_change(&repo, "sub/fresh.txt").unwrap(),
            "staging a directory must stage what is inside it"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `git_checkout` refuses a dirty tree, and refuses it with a *code*.
    ///
    /// The refusal is the roadmap's invariant — a force checkout discards
    /// work — but the part a caller can act on is that the failure is
    /// `WORKTREE_DIRTY` rather than prose, so a client can offer "commit or
    /// stash" without matching English.
    #[test]
    fn git_checkout_refuses_a_dirty_tree_with_a_code() {
        let dir = structured_workspace("checkout-dirty");
        std::fs::write(dir.join("a.txt"), "uncommitted\n").unwrap();

        let tool = parse_tool_call("git_checkout", &serde_json::json!({"name": "second"})).unwrap();
        let result = execute(&tool, Some(&dir), None);

        assert!(!result.ok, "checkout must refuse a dirty tree");
        assert_eq!(result.error_code, Some(ErrorCode::WorktreeDirty));
        assert!(
            result.error.as_deref().unwrap_or("").contains("a.txt"),
            "the refusal should name the blocking file: {:?}",
            result.error
        );

        // Refusing has to mean *nothing moved*. The code is only worth having
        // if the branch did not switch anyway.
        let repo = git2::Repository::open(&dir).unwrap();
        assert_ne!(
            git::current_branch(&repo).as_deref(),
            Some("second"),
            "the refusal reported failure but switched branch anyway"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `git_commit` with nothing staged is refused rather than writing an
    /// empty commit.
    #[test]
    fn git_commit_refuses_an_empty_index() {
        let dir = structured_workspace("commit-empty");

        let tool =
            parse_tool_call("git_commit", &serde_json::json!({"message": "nothing"})).unwrap();
        let result = execute(&tool, Some(&dir), None);

        assert!(!result.ok);
        assert_eq!(result.error_code, Some(ErrorCode::InvalidArguments));

        // The refusal is only worth the error if no commit was created.
        let repo = git2::Repository::open(&dir).unwrap();
        assert_eq!(
            git::log(&repo, 10).unwrap().len(),
            1,
            "an empty commit landed"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `git_show` and `git_commit_diff` take a revision, not only a hex id.
    ///
    /// `git::show` used `Oid::from_str`, so the schema's "commit id or
    /// revision, e.g. 'HEAD~1'" was a promise the code did not keep — the
    /// documented example failed.
    #[test]
    fn git_show_accepts_a_revision_name() {
        let dir = structured_workspace("revparse");

        for rev in ["HEAD", "second"] {
            let tool = parse_tool_call("git_show", &serde_json::json!({"oid": rev})).unwrap();
            let result = execute(&tool, Some(&dir), None);
            assert!(result.ok, "git_show {rev:?} failed: {:?}", result.error);
        }
        // A name that resolves to nothing is an error, not a panic.
        let tool =
            parse_tool_call("git_show", &serde_json::json!({"oid": "no-such-thing"})).unwrap();
        assert!(!execute(&tool, Some(&dir), None).ok);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The patch budget is shared across the whole result, and a patch that
    /// does not fit does not spend it.
    ///
    /// So a change too large to include is dropped while a small change after
    /// it still arrives. The alternative — spending the budget on the big file
    /// anyway — would let one huge diff silently swallow every smaller one
    /// behind it, which is the failure mode the cap exists to prevent.
    #[test]
    fn a_patch_too_large_to_include_does_not_drop_the_small_ones_after_it() {
        let dir = structured_workspace("diff-budget");
        let huge: String = (1..=60_000).map(|i| format!("changed {i}\n")).collect();
        std::fs::write(dir.join("a.txt"), &huge).unwrap();
        // A *small* change to a file that sorts after the huge one.
        std::fs::write(dir.join("long.txt"), "line 1\nchanged\n").unwrap();

        let tool = parse_tool_call("git_diff", &serde_json::json!({})).unwrap();
        let result = execute(&tool, Some(&dir), None);
        assert!(result.ok, "{:?}", result.error);

        let s = result
            .structured
            .expect("git_diff always structures its result");
        assert_eq!(
            s["truncated"], true,
            "a 600 kB patch should not fit the budget"
        );
        assert_eq!(s["count"], 2);
        assert_eq!(s["files"][0]["path"], "a.txt");
        assert_eq!(s["files"][0]["patch_omitted"], true);
        assert_eq!(s["files"][1]["path"], "long.txt");
        assert_eq!(
            s["files"][1]["patch_omitted"], false,
            "a patch that did not fit must not spend the budget for later files"
        );
        // The omitted file is still *reported*: complete about what changed,
        // partial about how.
        assert!(result.output.unwrap_or_default().contains("a.txt"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every tool that returns a structured payload emits exactly the keys its
    /// `output_schema` advertises, and every key the schema marks `required`.
    ///
    /// MCP makes `outputSchema` a promise about `structuredContent`, so a
    /// field an executor adds without a schema row is a broken promise rather
    /// than a harmless extra — this is what keeps the two halves honest.
    #[test]
    fn structured_output_matches_its_declared_schema() {
        let (dir, state) = stateful_workspace("structured");
        let ctx = ToolCtx::with_state(Some(&dir), &state);

        let mut structured_tools = Vec::new();
        for spec in SPECS {
            let tool = parse_tool_call(spec.name, &succeeding_args(spec.name))
                .unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            let result = execute_in(&tool, &ctx, None);
            let schema = output_schema(spec.name)
                .unwrap_or_else(|| panic!("no output schema for {}", spec.name));
            let props = schema["properties"].as_object().expect("properties");
            let required: Vec<&str> = schema["required"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();

            let Some(structured) = result.structured.as_ref() else {
                // Errors legitimately carry no payload; a *success* without one
                // means a tool was added with no structured channel at all.
                assert!(
                    !result.ok,
                    "{} succeeded without a structured payload",
                    spec.name
                );
                continue;
            };
            structured_tools.push(spec.name);

            let emitted = structured.as_object().expect("structured is an object");
            for key in emitted.keys() {
                assert!(
                    props.contains_key(key),
                    "{} emits '{key}' but its schema does not list it",
                    spec.name
                );
            }
            for key in &required {
                assert!(
                    emitted.contains_key(*key),
                    "{} schema requires '{key}' but nothing was emitted",
                    spec.name
                );
            }
        }

        // A guard on the guard: if the args above ever stop landing, this test
        // would pass by checking nothing at all.
        assert!(
            structured_tools.len() >= 12,
            "only {} tools produced a structured payload — the fixtures have \
             drifted and this test is no longer covering the surface: {structured_tools:?}",
            structured_tools.len()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_file_reads_whole_with_no_footer() {
        let out = chunk_text("a.txt", "one\ntwo\nthree\n", None, None);
        assert_eq!(out.text, "   1| one\n   2| two\n   3| three\n");
        // The whole file in one page: no next offset to hand out.
        assert_eq!(out.next_offset, None);
        assert_eq!((out.start_line, out.end_line, out.total_lines), (1, 3, 3));
    }

    #[test]
    fn empty_file_is_reported_not_blank() {
        let out = chunk_text("a.txt", "", None, None);
        assert_eq!(out.text, "[a.txt is empty]\n");
        assert_eq!(out.total_lines, 0);
        assert_eq!(out.next_offset, None);
    }

    #[test]
    fn large_file_pages_and_hands_out_the_next_offset() {
        let text = (1..=1000)
            .map(|i| format!("line {i}\n"))
            .collect::<String>();

        let first = chunk_text("big.txt", &text, None, None);
        assert!(
            first.text.starts_with("   1| line 1\n"),
            "{:.40}",
            first.text
        );
        assert!(first
            .text
            .contains(&format!("{:>4}| line {}\n", CHUNK_LINES, CHUNK_LINES)));
        assert!(!first
            .text
            .contains(&format!("| line {}\n", CHUNK_LINES + 1)));
        assert!(first.text.contains("[chunk 1 of 3 · lines 1-400 of 1000"));
        // The offset is the paging primitive — data to pass, not a call to copy.
        assert_eq!(first.next_offset, Some(401));
        assert!(
            !first.text.contains("read_file("),
            "the footer is call syntax again: {}",
            first.text
        );

        // Following `next_offset` must land exactly where the last chunk stopped.
        let second = chunk_text("big.txt", &text, Some(401), None);
        assert!(
            second.text.starts_with(" 401| line 401\n"),
            "{:.40}",
            second.text
        );
        assert!(second
            .text
            .contains("[chunk 2 of 3 · lines 401-800 of 1000"));
        assert_eq!(second.next_offset, Some(801));

        let third = chunk_text("big.txt", &text, Some(801), None);
        assert!(third
            .text
            .contains("[chunk 3 of 3 · lines 801-1000 of 1000"));
        assert!(third.text.contains("[end of file]"));
        // End of file: nothing left to page to.
        assert_eq!(third.next_offset, None);
    }

    #[test]
    fn chunk_is_bounded_by_bytes_not_just_lines() {
        // 100 lines of 1KB each: the line budget is 400, so bytes must be what
        // stops it, or one chunk would blow past CHUNK_BYTES.
        let text = (0..100)
            .map(|_| format!("{}\n", "x".repeat(1024)))
            .collect::<String>();
        let out = chunk_text("wide.txt", &text, None, None);
        assert!(
            out.text.len() < CHUNK_BYTES * 2,
            "chunk was {} bytes",
            out.text.len()
        );
        assert!(out.next_offset.is_some());
    }

    #[test]
    fn one_overlong_line_still_makes_progress() {
        // A minified bundle is a single line far over CHUNK_BYTES. Returning an
        // empty chunk would leave the AI looping on the same offset forever.
        let text = format!("{}\nsecond\n", "y".repeat(CHUNK_BYTES * 3));
        let out = chunk_text("min.js", &text, None, None);
        assert!(out.text.starts_with("   1| yyy"));
        assert_eq!(out.next_offset, Some(2));
    }

    #[test]
    fn offset_past_the_end_is_clamped_not_an_error() {
        // The AI guesses at file length; stranding it on a stale offset is worse
        // than showing the last line.
        let out = chunk_text("a.txt", "one\ntwo\n", Some(99), None);
        assert!(out.text.contains("| two"), "{}", out.text);
    }

    #[test]
    fn explicit_limit_is_honoured() {
        let text = (1..=50).map(|i| format!("line {i}\n")).collect::<String>();
        let out = chunk_text("a.txt", &text, Some(10), Some(5));
        assert!(out.text.starts_with("  10| line 10\n"), "{:.40}", out.text);
        assert!(out.text.contains("  14| line 14\n"));
        // Matched with the rendered body's pipe, so the footer's "continues at
        // line 15" doesn't read as a leaked body line.
        assert!(!out.text.contains("| line 15"));
        assert_eq!(out.next_offset, Some(15));
    }

    // ── parse_tool_call / tool_input_schema drift guards ────────────────
    // The MCP connector advertises `tool_input_schema` and feeds MCP args
    // through `parse_tool_call`. These tests pin the two together: every
    // SPECS tool has a schema, schemas only exist for SPECS tools, and a
    // canonical args object shaped exactly like the schema actually parses —
    // so a name/type mismatch between schema and parser cannot slip through.

    /// A schema-shaped, valid args object per canonical tool. No aliases —
    /// only the canonical keys the schema advertises.
    fn sample_args(name: &str) -> serde_json::Value {
        match name {
            "read_file" => serde_json::json!({"path": "a.txt", "offset": 1, "limit": 5}),
            "list_directory" => serde_json::json!({"path": "."}),
            "write_file" => serde_json::json!({"path": "a.txt", "content": "x"}),
            "edit_file" => {
                serde_json::json!({"path": "a.txt", "old_string": "a", "new_string": "b"})
            }
            "multi_edit" => serde_json::json!({
                "path": "a.txt",
                "edits": [{"old_string": "a", "new_string": "b"}]
            }),
            "apply_patch" => serde_json::json!({"path": "a.txt", "patch": "@@ -1 +1 @@\n-a\n+b"}),
            "delete_file" => serde_json::json!({"path": "a.txt"}),
            "move_file" => serde_json::json!({"from": "a", "to": "b"}),
            "copy_file" => serde_json::json!({"from": "a", "to": "b"}),
            "create_directory" => serde_json::json!({"path": "d"}),
            "read_many_files" => serde_json::json!({"paths": ["a.txt", "b.txt"]}),
            "grep" => serde_json::json!({"pattern": "alpha", "path": "."}),
            "glob" => serde_json::json!({"pattern": "*.txt"}),
            "run_command" => serde_json::json!({"command": "echo hi"}),
            "run_command_background" => serde_json::json!({"command": "sleep 30"}),
            "command_output" => serde_json::json!({"id": 1, "cursor": 0}),
            "kill_command" => serde_json::json!({"id": 1}),
            "git_status" => serde_json::json!({}),
            "git_diff" => serde_json::json!({"path": "a.txt"}),
            "git_log" => serde_json::json!({"limit": 5}),
            "git_add" => serde_json::json!({"path": "a.txt"}),
            "git_unstage" => serde_json::json!({"path": "a.txt"}),
            "git_commit" => serde_json::json!({"message": "a subject\n\nand a body"}),
            "git_branches" => serde_json::json!({}),
            "git_checkout" => serde_json::json!({"name": "second"}),
            "git_create_branch" => {
                serde_json::json!({"name": "third", "base": "HEAD", "checkout": false})
            }
            "git_show" => serde_json::json!({"oid": "HEAD"}),
            "git_commit_diff" => serde_json::json!({"oid": "HEAD"}),
            "todo_write" => serde_json::json!({
                "todos": [{"content": "do it", "status": "in_progress", "active_form": "doing it"}]
            }),
            "todo_read" => serde_json::json!({}),
            "set_objective" => serde_json::json!({"text": "ship the memory tools"}),
            "remember_decision" => {
                serde_json::json!({"summary": "keep one connector session", "reason": "attribution"})
            }
            "remember_constraint" => serde_json::json!({"text": "no new dependencies"}),
            "remember_attempt" => {
                serde_json::json!({"description": "tried the newest session", "succeeded": false})
            }
            "get_facts" => serde_json::json!({"session_id": 1}),
            "list_sessions" => serde_json::json!({"limit": 5}),
            "request_handoff" => {
                serde_json::json!({"reason": "a decision is needed", "next_step": "pick a store"})
            }
            "get_handoff" => serde_json::json!({}),
            "describe_tool" => serde_json::json!({"name": "read_file"}),
            "list_tools" => serde_json::json!({}),
            // Phase 7–10. `web_fetch` deliberately names a non-fetchable
            // scheme: `plan` refuses it before any socket opens, so the
            // surface-wide tests exercise the tool without a network. The
            // language-server and agent-loop tools fail fast here (no server
            // installed, no desktop attached), which is why
            // `needs_outside_world` skips them where success is required.
            "web_fetch" => serde_json::json!({"url": "ftp://example.invalid/"}),
            "web_search" => serde_json::json!({"query": ""}),
            "notebook_read" => serde_json::json!({"path": "nb.ipynb"}),
            "notebook_edit" => serde_json::json!({
                "path": "nb.ipynb", "cell_id": "cell-0", "new_source": "edited\n"
            }),
            "delegate_task" => serde_json::json!({"task": "t"}),
            "lsp_diagnostics" => serde_json::json!({}),
            "lsp_definition" => serde_json::json!({"path": "a.txt", "line": 1, "character": 1}),
            "lsp_references" => serde_json::json!({
                "path": "a.txt", "line": 1, "character": 1, "include_declaration": true
            }),
            "lsp_symbols" => serde_json::json!({"path": "a.txt"}),
            "ask_user" => serde_json::json!({"question": "which?", "options": ["a", "b"]}),
            "propose_plan" => serde_json::json!({"plan": "do it", "steps": ["one"]}),
            "monitor" => serde_json::json!({"path": ".", "timeout_ms": 1000}),
            "notify" => serde_json::json!({"title": "t", "body": "b"}),
            "enter_worktree" => serde_json::json!({}),
            "exit_worktree" => serde_json::json!({"action": "discard"}),
            "read_media" => serde_json::json!({"path": "pic.png"}),
            "publish_artifact" => serde_json::json!({"path": "long.txt"}),
            "report_findings" => serde_json::json!({
                "summary": "one issue",
                "findings": [{"path": "a.txt", "line": 1, "severity": "warning", "claim": "x"}]
            }),
            other => panic!("sample_args missing tool: {other}"),
        }
    }

    #[test]
    fn every_spec_row_has_a_schema_and_schema_advertises_only_spec_tools() {
        for spec in SPECS {
            let s = tool_input_schema(spec.name)
                .unwrap_or_else(|| panic!("no schema for tool {}", spec.name));
            assert_eq!(s["type"], "object", "{}", spec.name);
            let props = s["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("schema for {} lacks properties", spec.name));
            let required = s["required"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|v| v.as_str().expect("required entries are strings"))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            for r in &required {
                assert!(
                    props.contains_key(*r),
                    "{}: required '{r}' not in properties",
                    spec.name
                );
            }
        }
        // Unknown / unmapped names resolve to no schema (guards against a
        // stray arm naming a tool outside SPECS).
        for unknown in ["nope", "read_fil", "readfile", "definitely_not_a_tool"] {
            assert!(tool_input_schema(unknown).is_none(), "{unknown}");
        }
    }

    #[test]
    fn alias_lookup_resolves_to_canonical_schema() {
        // spec_by_name resolves aliases; the schema must land on the same
        // canonical row whether asked by alias or canonical name.
        for alias in ["Read", "view_file", "cat", "default_api.read_file"] {
            assert_eq!(
                tool_input_schema(alias),
                tool_input_schema("read_file"),
                "alias {alias} should map to read_file's schema"
            );
        }
    }

    #[test]
    fn schema_shaped_args_parse_to_the_same_tool() {
        for spec in SPECS {
            let args = sample_args(spec.name);
            let tool =
                parse_tool_call(spec.name, &args).unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            assert_eq!(
                tool_name(&tool),
                spec.name,
                "schema-shaped args for {} parsed to the wrong tool",
                spec.name
            );
        }
    }

    #[test]
    fn every_schema_required_arg_is_truly_required_by_the_parser() {
        for spec in SPECS {
            let s = tool_input_schema(spec.name).expect("schema");
            let required: Vec<&str> = s["required"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            for key in required {
                let mut args = sample_args(spec.name);
                args.as_object_mut().unwrap().remove(key);
                let err = parse_tool_call(spec.name, &args)
                    .expect_err(&format!("{} should require '{key}'", spec.name));
                assert!(
                    err.contains(&format!("missing '{key}'")) || err.contains("missing"),
                    "{} without '{key}': unexpected error: {err}",
                    spec.name
                );
            }
        }
    }

    #[test]
    fn parser_coercions_survive_the_lift() {
        // Behaviour guarantees a loose, model-written args object has always
        // relied on, and that MCP args must share.
        // Single string stands in for a paths array.
        let t = parse_tool_call("read_many_files", &serde_json::json!({"paths": "a.txt"}))
            .expect("single-string paths");
        assert_eq!(
            t,
            Tool::ReadManyFiles {
                paths: vec!["a.txt".into()]
            }
        );
        // Quoted numbers coerce for offsets.
        let t = parse_tool_call(
            "read_file",
            &serde_json::json!({"path": "a", "offset": "12"}),
        )
        .expect("quoted offset");
        assert_eq!(
            t,
            Tool::ReadFile {
                path: "a".into(),
                offset: Some(12),
                limit: None
            }
        );
        // Alias argument names are tolerated.
        let t = parse_tool_call(
            "edit_file",
            &serde_json::json!({
                "file": "a.txt", "find": "x", "replace": "y"
            }),
        )
        .expect("alias arg names");
        assert_eq!(
            t,
            Tool::EditFile {
                path: "a.txt".into(),
                old_string: "x".into(),
                new_string: "y".into(),
                replace_all: None
            }
        );
        // Unknown tool name is an error, not a panic.
        assert!(parse_tool_call("no_such_tool", &serde_json::json!({})).is_err());
    }
    // --- the property suite ----------------------------------------------
    //
    // *Functional Programming in Scala* ch. 8 makes two points that shape
    // everything below. First, a property "reveals hidden assumptions" that
    // examples paper over. Second, when a domain is small and closed,
    // quantifying over it exhaustively is a proof rather than the absence of
    // evidence. The tool × group × approval matrix here is small and closed,
    // so every property below is stated over all of `SPECS` — never over a
    // hand-written sample. The payoff is that a tool added tomorrow is
    // covered the moment it is declared, and the forcing function is that
    // `Tool`'s `match`es are wildcard-free: a new variant does not compile
    // until `tool_paths`, `spec` and `output_schema` all know about it.

    /// Path-valued keys a tool's input schema may declare.
    const PATH_ARG_KEYS: &[&str] = &["path", "from", "to", "paths"];

    /// Tools that screen their path arguments per item at *execution* time
    /// instead of up front through [`tool_paths`].
    ///
    /// Exactly one, and it is the documented batch case: `read_many_files`
    /// marks a sensitive entry `status: "skipped"` rather than refusing the
    /// whole batch, so its `tool_paths` is deliberately empty (see its arm).
    /// Any other name appearing here would be a hole in the approval gate,
    /// which is why `every_per_item_exception_is_a_real_tool` pins the list.
    const PER_ITEM_PATH_SCREENED: &[&str] = &["read_many_files"];

    /// Paths a model may emit: mostly escapes, plus three legal shapes so the
    /// property cannot pass by refusing everything.
    const PATH_CORPUS: &[&str] = &[
        "..",
        "../outside",
        "../",
        "../../etc/passwd",
        "a/../../outside",
        "a/b/../../../outside.txt",
        "notes/../../../.bashrc",
        "sub/../../outside",
        "./../outside",
        "a/./../../outside",
        ".../outside",
        "\\\\..\\\\..\\\\windows",
        "/etc/passwd",
        "//etc/passwd",
        // Legal, and must stay legal — cancelling back inside the root.
        "sub/..",
        "a/../b.txt",
        "new_dir/nested/f.txt",
    ];

    /// Every code has one spelling, in both halves of the boundary.
    ///
    /// `ErrorCode` reaches a connector two ways: serde writes it by name into
    /// `structuredContent`/the error payload, and `Display` writes it into
    /// prose. A client that reads one and compares against the other sees two
    /// different codes if they drift. `name()` is the single source; this
    /// checks the other two against it.
    #[test]
    fn every_error_code_is_named_consistently() {
        let mut seen: Vec<&str> = Vec::new();
        for code in ErrorCode::ALL {
            let wire = code.name();
            assert!(
                !seen.contains(&wire),
                "ErrorCode::ALL lists {wire} more than once"
            );
            seen.push(wire);

            // The serde rename and Display must agree, or the same failure
            // reads as two different codes depending on which half a client
            // looked at.
            let serialized = serde_json::to_value(code)
                .expect("a unit enum always serializes")
                .as_str()
                .expect("a unit enum serializes to a string")
                .to_string();
            assert_eq!(
                wire, serialized,
                "{wire} is serialized as {serialized} — the two halves disagree"
            );
            assert_eq!(wire, code.to_string());

            // SCREAMING_SNAKE, so a code is safe to compare as an opaque
            // token in a protocol that also carries prose.
            assert!(
                wire.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_'),
                "{wire} is not SCREAMING_SNAKE"
            );
            assert!(
                wire.starts_with(|c: char| c.is_ascii_uppercase()),
                "{wire} does not begin with a letter"
            );
            assert!(!wire.ends_with('_'), "{wire} ends with a separator");
        }

        // A guard on the guard: iterating an empty or truncated list would
        // make every assertion above vacuous. The count is deliberate — it
        // makes *any* change to the vocabulary a conscious edit rather than a
        // silent one, and that includes additions, which are exactly the
        // changes most likely to be made in passing.
        assert_eq!(
            seen.len(),
            32,
            "the error vocabulary changed size — update this tripwire and \
             confirm the new codes are the ones you meant to add: {seen:?}"
        );
    }

    #[test]
    fn every_per_item_exception_is_a_real_tool() {
        // Otherwise the list above could name a tool that no longer exists
        // and quietly exempt nothing, while looking like it exempts something.
        for name in PER_ITEM_PATH_SCREENED {
            assert!(
                spec_by_name(name).is_some(),
                "PER_ITEM_PATH_SCREENED names {name}, which is not a tool"
            );
        }
    }

    /// **No accepted path resolves outside the workspace.**
    ///
    /// Stated over every tool and a corpus of escapes, rather than over the
    /// two examples the older tests carried. The check is deliberately
    /// one-directional: `resolve_path` returning `Err` is always acceptable
    /// (refusing a legal path is a usability bug), while returning a path
    /// outside the canonical root is a sandbox escape.
    #[test]
    fn no_accepted_path_resolves_outside_the_root() {
        let dir = structured_workspace("escape-proof");
        let canonical = dir.canonicalize().unwrap();

        let mut accepted = 0usize;
        let mut tools_with_paths = 0usize;
        for spec in SPECS {
            let tool = parse_tool_call(spec.name, &succeeding_args(spec.name))
                .unwrap_or_else(|e| panic!("{}: {e}", spec.name));

            // The tool's own declared paths first, so a tool that ships a
            // surprising path is checked even if the corpus never names it.
            let declared: Vec<String> = tool_paths(&tool).into_iter().map(str::to_string).collect();
            if !declared.is_empty() {
                tools_with_paths += 1;
            }

            for raw in declared
                .iter()
                .map(String::as_str)
                .chain(PATH_CORPUS.iter().copied())
            {
                if let Ok(p) = resolve_path(&dir, raw) {
                    assert!(
                        p.starts_with(&canonical),
                        "{}: resolve_path accepted {raw:?} and produced {} — \
                         outside {}",
                        spec.name,
                        p.display(),
                        canonical.display()
                    );
                    accepted += 1;
                }
            }
        }

        // Guards on the guard: the corpus must actually produce successes
        // (otherwise this proves only that everything errors), and the
        // iteration must actually be finding path-bearing tools.
        assert!(
            accepted >= 3,
            "only {accepted} corpus paths were accepted — the corpus is \
             misaligned with the root and this test is near-vacuous"
        );
        assert!(
            tools_with_paths >= 8,
            "only {tools_with_paths} tools reported a path — `tool_paths` has \
             drifted and this test is no longer covering the surface"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **What the approval gate screens covers what the call carries.**
    ///
    /// Both `needs_approval` and `grant_matches` screen a tool by asking
    /// `tool_paths`. A path argument the extractor does not return is
    /// therefore neither approved nor refused on its own merits — the
    /// `copy_file .env notes.txt` laundering shape, where a secret moves to a
    /// name no read gate stops at. So: every path-valued slot the *schema*
    /// advertises must come back out of `tool_paths`.
    ///
    /// The sentinels are distinct per slot so a failure names the slot rather
    /// than just the tool.
    #[test]
    fn tool_paths_covers_every_path_the_schema_advertises() {
        for spec in SPECS {
            if PER_ITEM_PATH_SCREENED.contains(&spec.name) {
                continue;
            }
            let schema = tool_input_schema(spec.name).expect("every spec has a schema");
            let props = schema["properties"].as_object().expect("properties");

            let mut args = sample_args(spec.name);
            let mut expected: Vec<String> = Vec::new();
            for key in props.keys() {
                if !PATH_ARG_KEYS.contains(&key.as_str()) {
                    continue;
                }
                let sentinel = format!("sentinel-{}-{key}", spec.name);
                // `paths` is an array slot; every other path slot is a string.
                args[key] = match args.get(key) {
                    Some(serde_json::Value::Array(_)) => serde_json::json!([sentinel.clone()]),
                    _ => serde_json::Value::String(sentinel.clone()),
                };
                expected.push(sentinel);
            }
            if expected.is_empty() {
                continue;
            }

            let tool =
                parse_tool_call(spec.name, &args).unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            let found: Vec<&str> = tool_paths(&tool);
            for want in &expected {
                assert!(
                    found.iter().any(|p| p == want),
                    "{}: its schema advertises a path slot that `tool_paths` \
                     does not return ({want:?} not in {found:?}) — the \
                     approval gate would not screen it",
                    spec.name
                );
            }
        }
    }
    // --- memory ----------------------------------------------------------

    /// Run one memory tool against a live database.
    fn memory_call(
        dir: &Path,
        state: &crate::AppState,
        name: &str,
        args: serde_json::Value,
    ) -> ToolResult {
        let tool = parse_tool_call(name, &args).unwrap_or_else(|e| panic!("{name}: {e}"));
        execute_in(&tool, &ToolCtx::with_state(Some(dir), state), None)
    }

    /// A tool that cannot reach the database says so instead of reporting an
    /// empty memory.
    ///
    /// "There are no constraints" and "there is nowhere to read constraints
    /// from" are different answers, and only the first is information. A model
    /// told the second as though it were the first concludes the project has
    /// no constraints and proceeds to break them.
    #[test]
    fn a_memory_tool_without_a_database_says_which_half_is_missing() {
        let dir = structured_workspace("memory-nodb");

        for name in [
            "todo_write",
            "todo_read",
            "set_objective",
            "remember_decision",
            "get_facts",
            "request_handoff",
            "get_handoff",
        ] {
            let tool = parse_tool_call(name, &succeeding_args(name)).unwrap();
            // A root and nothing else: the desktop's own context, without the
            // database behind it.
            let result = execute(&tool, Some(&dir), None);
            assert!(!result.ok, "{name} claimed success with no database");
            let err = result.error.unwrap_or_default();
            assert!(
                err.contains("database") || err.contains("live state"),
                "{name} blamed something other than the missing context: {err}"
            );
            assert_eq!(result.error_code, Some(ErrorCode::InternalError));
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A task list with one bad status is refused whole, and nothing is
    /// stored.
    ///
    /// The alternative — clamping `"sorta"` to `pending` — stores a list the
    /// caller did not send and hands it straight back as truth. A list is one
    /// value (ch. 12), so one bad item invalidates it.
    #[test]
    fn an_unknown_todo_status_refuses_the_whole_list() {
        let (dir, state) = stateful_workspace("todo-status");

        let first = memory_call(
            &dir,
            &state,
            "todo_write",
            serde_json::json!({"todos": [{"content": "keep me", "status": "pending"}]}),
        );
        assert!(first.ok, "{:?}", first.error);

        let bad = memory_call(
            &dir,
            &state,
            "todo_write",
            serde_json::json!({"todos": [
                {"content": "fine", "status": "pending"},
                {"content": "not fine", "status": "sorta"},
            ]}),
        );
        assert!(!bad.ok, "an unknown status is not a status");
        assert_eq!(bad.error_code, Some(ErrorCode::InvalidArguments));
        assert!(
            bad.error.unwrap_or_default().contains("sorta"),
            "the refusal should name the status it did not recognise"
        );

        // The refused write stored nothing, and did not half-replace the list
        // that was already there.
        let read = memory_call(&dir, &state, "todo_read", serde_json::json!({}));
        let s = read.structured.unwrap();
        assert_eq!(s["count"].as_u64(), Some(1));
        assert_eq!(s["todos"][0]["content"], "keep me");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `todo_write` writes the list it was given, not a merge with the old
    /// one.
    ///
    /// An item disappearing is how a caller says "this is done and I no longer
    /// want to see it"; a merge would keep resurrecting it.
    #[test]
    fn todo_write_replaces_rather_than_merges() {
        let (dir, state) = stateful_workspace("todo-replace");

        // Bare strings, which the parser lifts to pending items — the same
        // leniency a model-written argument object gets everywhere else.
        let r = memory_call(
            &dir,
            &state,
            "todo_write",
            serde_json::json!({"todos": ["a", "b", "c"]}),
        );
        assert!(r.ok, "{:?}", r.error);
        assert_eq!(r.structured.unwrap()["count"].as_u64(), Some(3));

        let r = memory_call(
            &dir,
            &state,
            "todo_write",
            serde_json::json!({"todos": ["a", "b"]}),
        );
        assert!(r.ok, "{:?}", r.error);
        let s = r.structured.unwrap();
        assert_eq!(s["count"].as_u64(), Some(2), "the list was merged, not set");
        assert_eq!(s["todos"][0]["status"], "pending");

        // And the text is the list, so the trace shows what was stored.
        let text = r.output.clone().unwrap_or_default();
        assert!(text.contains("b"), "{text}");
        assert!(!text.contains("c"), "a dropped item is still in the answer");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A recorded fact lands in the connector's own session, and creates no
    /// other.
    ///
    /// The archive's sessions belong to Claude Code transcripts; filing a web
    /// AI's decision under one of those would credit it to the wrong agent,
    /// and the desktop UI reads exactly these tables.
    #[test]
    fn a_recorded_fact_lands_in_the_connectors_own_session() {
        let (dir, state) = stateful_workspace("memory-session");

        // A session that already exists — an archived Claude Code transcript.
        // Without it, "file under the connector's own row" and "file under
        // whatever session is newest" are the same instruction, and the test
        // would pass either way.
        let other = {
            let conn = state.conn.lock().unwrap();
            db::upsert_session(
                &conn,
                &db::NewSession {
                    agent: "claude-code",
                    source: "/tmp/an-archived-transcript.jsonl",
                    cwd: None,
                    source_mtime: 0,
                    objective: Some("someone else's task"),
                },
            )
            .unwrap()
        };

        let r = memory_call(
            &dir,
            &state,
            "remember_decision",
            serde_json::json!({"summary": "use sqlite", "reason": "it is already here"}),
        );
        assert!(r.ok, "{:?}", r.error);
        let id = r.structured.unwrap()["session_id"].as_i64().unwrap();

        let conn = state.conn.lock().unwrap();
        assert_eq!(db::connector_session_id(&conn).unwrap(), id);
        let facts = db::get_facts(&conn, id).unwrap();
        assert_eq!(facts.decisions, vec!["use sqlite".to_string()]);
        let sessions: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sessions, 2, "recording a fact invented a session");
        // ...and it went to the connector's row, not to the newest session —
        // which, in a fresh install with an archive beside it, is someone
        // else's.
        let theirs = db::get_facts(&conn, other).unwrap();
        assert!(
            theirs.decisions.is_empty(),
            "a connector's decision was filed against another agent's session"
        );
        drop(conn);

        // An empty reason is no reason: the row must not claim a rationale it
        // was not given.
        let r = memory_call(
            &dir,
            &state,
            "remember_decision",
            serde_json::json!({"summary": "ship it", "reason": "   "}),
        );
        assert!(r.ok, "{:?}", r.error);
        let conn = state.conn.lock().unwrap();
        let reason: Option<String> = conn
            .query_row(
                "SELECT reason FROM decisions WHERE summary = 'ship it'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(reason, None, "whitespace became a rationale");
        drop(conn);

        // An empty fact is refused rather than stored as a blank line.
        let r = memory_call(
            &dir,
            &state,
            "remember_constraint",
            serde_json::json!({"text": "  "}),
        );
        assert!(!r.ok);
        assert_eq!(r.error_code, Some(ErrorCode::InvalidArguments));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `get_handoff` returns the card the desktop builds, and says it in
    /// words as well as fields.
    ///
    /// This is the pull-based handoff the extension removal cost, so the tool
    /// and the desktop command must not drift into two different answers to
    /// "what was happening?".
    #[test]
    fn get_handoff_returns_the_same_card_the_desktop_builds() {
        let (dir, state) = stateful_workspace("handoff-card");

        // A card with something in it. An empty handoff is a fine answer but a
        // poor fixture: "build the same card" is trivial when the card is
        // empty, so the objective comes from the desktop's own state and the
        // decisions from the project memory — the two halves
        // `build_handoff_impl` joins.
        *state.objective.lock().unwrap() = Some("finish the memory tools".to_string());
        let seeded = memory_call(
            &dir,
            &state,
            "remember_decision",
            serde_json::json!({"summary": "answer from what was stored"}),
        );
        assert!(seeded.ok, "{:?}", seeded.error);

        let r = memory_call(&dir, &state, "get_handoff", serde_json::json!({}));
        assert!(r.ok, "{:?}", r.error);

        let text = r.output.clone().unwrap_or_default();
        let mut got = r.structured.unwrap();
        let mut want = serde_json::to_value(crate::build_handoff_impl(&state).unwrap()).unwrap();
        // `generated_at` is a clock reading, so two builds differ by design.
        // Everything else is state, and must match exactly.
        for v in [&mut got, &mut want] {
            v.as_object_mut().unwrap().remove("generated_at");
        }
        assert_eq!(
            got, want,
            "the tool and the desktop build two different handoffs"
        );

        // The text carries the card rather than pointing at it: a handoff is
        // the thing a caller pastes into a fresh conversation.
        let objective = want["objective"].as_str().unwrap();
        assert_eq!(objective, "finish the memory tools");
        assert!(text.contains(objective), "{text}");
        assert!(text.contains("progress:"), "{text}");
        assert!(
            text.contains("answer from what was stored"),
            "the card's decisions did not reach the text:\n{text}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The fact a caller records is the fact `get_facts` and the handoff
    /// report.
    ///
    /// One round trip through all three read paths, because a write-only
    /// memory is indistinguishable from no memory until something reads it.
    #[test]
    fn recorded_facts_come_back_out_of_the_project_memory() {
        let (dir, state) = stateful_workspace("memory-roundtrip");

        for (name, args) in [
            (
                "set_objective",
                serde_json::json!({"text": "land the memory tools"}),
            ),
            (
                "remember_decision",
                serde_json::json!({"summary": "keep one connector session"}),
            ),
            (
                "remember_constraint",
                serde_json::json!({"text": "no new dependencies"}),
            ),
            (
                "remember_attempt",
                serde_json::json!({"description": "file under the newest session",
                                   "succeeded": false}),
            ),
        ] {
            let r = memory_call(&dir, &state, name, args);
            assert!(r.ok, "{name}: {:?}", r.error);
        }

        let r = memory_call(&dir, &state, "get_facts", serde_json::json!({}));
        assert!(r.ok, "{:?}", r.error);
        let s = r.structured.unwrap();
        assert_eq!(s["objective"], "land the memory tools");
        assert_eq!(s["decisions"][0], "keep one connector session");
        assert_eq!(s["constraints"][0], "no new dependencies");
        assert_eq!(s["failed_attempts"][0], "file under the newest session");

        // Nothing traced a step, so progress is the honest floor rather than
        // a fabricated number.
        assert_eq!(s["progress_percent"].as_u64(), Some(0));

        // A successful attempt is not a failed one, so it must not appear in
        // the list headed "do not repeat".
        let r = memory_call(
            &dir,
            &state,
            "remember_attempt",
            serde_json::json!({"description": "the obvious fix worked", "succeeded": true}),
        );
        assert!(r.ok, "{:?}", r.error);
        let r = memory_call(&dir, &state, "get_facts", serde_json::json!({}));
        let s = r.structured.unwrap();
        let failed: Vec<&str> = s["failed_attempts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(failed, vec!["file under the newest session"]);

        // `list_sessions` sees the connector's row, which is how a caller
        // learns the id `get_facts` will accept.
        let r = memory_call(&dir, &state, "list_sessions", serde_json::json!({}));
        assert!(r.ok, "{:?}", r.error);
        let s = r.structured.unwrap();
        assert_eq!(s["count"].as_u64(), Some(1), "{s}");
        let id = s["sessions"][0]["id"].as_i64().unwrap();

        let r = memory_call(
            &dir,
            &state,
            "get_facts",
            serde_json::json!({"session_id": id}),
        );
        assert!(r.ok, "the id list_sessions returned is not accepted");

        // And a name for a session that does not exist is a refusal, not an
        // empty memory.
        let r = memory_call(
            &dir,
            &state,
            "get_facts",
            serde_json::json!({"session_id": id + 99}),
        );
        assert!(!r.ok);
        assert_eq!(r.error_code, Some(ErrorCode::FileNotFound));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- search ----------------------------------------------------------

    fn search_project(tag: &str) -> PathBuf {
        let dir = temp_project(tag);
        std::fs::write(dir.join("a.rs"), "fn main() { needle(); }\n").unwrap();
        std::fs::write(dir.join("b.txt"), "no match here\n").unwrap();
        std::fs::write(dir.join("c.rs"), "// needle again\n").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/d.rs"), "needle deep\n").unwrap();
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        std::fs::write(dir.join("node_modules/e.rs"), "needle in noise\n").unwrap();
        dir
    }

    fn grep_in(dir: &Path, args: serde_json::Value) -> ToolResult {
        let tool = parse_tool_call("grep", &args).unwrap_or_else(|e| panic!("{e}"));
        execute(&tool, Some(dir), None)
    }

    #[test]
    fn grep_reports_path_line_and_text() {
        let dir = search_project("grep-content");
        let r = grep_in(&dir, serde_json::json!({"pattern": "needle", "path": "."}));
        assert!(r.ok, "{:?}", r.error);
        let s = r.structured.clone().unwrap();
        assert_eq!(s["mode"], "content");
        let matches = s["matches"].as_array().unwrap();
        assert!(matches.len() >= 3, "{matches:?}");
        // Every row is a real file:line, and line numbers are 1-based.
        for m in matches {
            assert_eq!(m["line"], 1, "each fixture has the match on line 1: {m:?}");
            assert!(m["text"].as_str().unwrap().contains("needle"));
        }
        // The prose half carries the same facts in `path:line: text` form.
        let out = r.output.unwrap();
        assert!(out.contains("a.rs:1:"), "{out}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_modes_change_the_shape_not_the_finding() {
        let dir = search_project("grep-modes");
        let files = grep_in(
            &dir,
            serde_json::json!({"pattern": "needle", "path": ".", "mode": "files_with_matches"}),
        );
        let s = files.structured.clone().unwrap();
        assert_eq!(s["mode"], "files_with_matches");
        assert!(s["matches"].as_array().unwrap().is_empty());
        let named: Vec<&str> = s["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap())
            .collect();
        assert!(
            named.contains(&"a.rs") && named.contains(&"c.rs"),
            "{named:?}"
        );
        assert!(
            named.contains(&"sub/d.rs"),
            "the walk must recurse: {named:?}"
        );

        let counts = grep_in(
            &dir,
            serde_json::json!({"pattern": "needle", "path": ".", "mode": "count"}),
        );
        let s = counts.structured.unwrap();
        assert_eq!(s["mode"], "count");
        for f in s["files"].as_array().unwrap() {
            assert_eq!(f["count"], 1, "{f:?}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A cap stops the work; it does not truncate its result** (ch. 5 §5.3).
    ///
    /// The difference is measurable: with the cap at 2, the walk must stop
    /// after reading about two files. Reading all of them and discarding the
    /// tail would produce the same *answer* and be unbounded work, so
    /// asserting on the answer alone would not catch the regression.
    #[test]
    fn grep_cap_stops_the_walk_rather_than_truncating_it() {
        let dir = temp_project("grep-cap");
        for i in 0..40 {
            std::fs::write(dir.join(format!("f{i:02}.txt")), "needle\n").unwrap();
        }
        let r = grep_in(
            &dir,
            serde_json::json!({"pattern": "needle", "path": ".", "max_results": 2}),
        );
        let s = r.structured.unwrap();
        assert_eq!(s["matches"].as_array().unwrap().len(), 2);
        assert_eq!(s["truncated"], true);
        let scanned = s["scanned"].as_u64().unwrap();
        assert!(
            scanned <= 4,
            "the cap did not stop the walk — {scanned} of 40 files were read \
             to produce 2 matches"
        );
        assert!(r.output.unwrap().contains("cap"), "the cut is reported");

        // And the same search without a cap reports no truncation.
        let all = grep_in(&dir, serde_json::json!({"pattern": "needle", "path": "."}));
        let s = all.structured.unwrap();
        assert_eq!(s["truncated"], false);
        assert_eq!(s["matches"].as_array().unwrap().len(), 40);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A search cannot reach a path the read tools are barred from.**
    ///
    /// This is the property that makes `grep` safe to hand a model: it walks
    /// the tree itself, so its path argument is not a path list and the
    /// approval gate cannot enumerate what it will touch. The walk has to
    /// enforce the sensitivity rule on its own.
    #[test]
    fn grep_never_reaches_a_sensitive_path() {
        let dir = temp_project("grep-sensitive");
        std::fs::write(dir.join("notes.txt"), "the secret word is aardvark\n").unwrap();
        std::fs::write(dir.join(".env"), "TOKEN=aardvark\n").unwrap();
        std::fs::write(dir.join("id_rsa"), "aardvark\n").unwrap();
        std::fs::create_dir_all(dir.join("secrets")).unwrap();
        std::fs::write(dir.join("secrets/keys.json"), "aardvark\n").unwrap();

        let r = grep_in(
            &dir,
            serde_json::json!({"pattern": "aardvark", "path": "."}),
        );
        let out = r.output.clone().unwrap();
        let s = r.structured.unwrap();
        let found = serde_json::to_string(&s["matches"]).unwrap();
        assert!(
            found.contains("notes.txt"),
            "the ordinary file is found: {out}"
        );
        for barred in [".env", "id_rsa", "keys.json"] {
            assert!(
                !found.contains(barred),
                "{barred} was searched — the walk does not honour the \
                 sensitive-path rule:\n{out}"
            );
            assert!(!out.contains(barred), "{out}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn the_walk_does_not_follow_a_symlink_out_of_the_workspace() {
        let outside = temp_project("grep-outside");
        std::fs::write(outside.join("loot.txt"), "aardvark\n").unwrap();
        let dir = temp_project("grep-symlink");
        std::fs::write(dir.join("inside.txt"), "nothing here\n").unwrap();
        std::os::unix::fs::symlink(outside.join("loot.txt"), dir.join("link.txt")).unwrap();
        std::os::unix::fs::symlink(&outside, dir.join("linkdir")).unwrap();

        let r = grep_in(
            &dir,
            serde_json::json!({"pattern": "aardvark", "path": "."}),
        );
        let out = r.output.clone().unwrap();
        assert!(
            !out.contains("aardvark"),
            "a symlink let the search read a file outside the workspace:\n{out}"
        );

        let g = execute(
            &parse_tool_call(
                "glob",
                &serde_json::json!({"pattern": "*.txt", "path": "."}),
            )
            .unwrap(),
            Some(&dir),
            None,
        );
        let out = g.output.unwrap();
        assert!(out.contains("inside.txt"), "{out}");
        assert!(!out.contains("loot.txt"), "glob followed a symlink: {out}");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn grep_skips_noise_directories() {
        let dir = search_project("grep-noise");
        let r = grep_in(&dir, serde_json::json!({"pattern": "needle", "path": "."}));
        let out = r.output.unwrap();
        assert!(
            !out.contains("node_modules"),
            "the walk descended into node_modules:\n{out}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_include_and_exclude_filter_by_path() {
        let dir = search_project("grep-filters");
        let only_rs = grep_in(
            &dir,
            serde_json::json!({"pattern": "needle", "path": ".", "include": "*.rs"}),
        );
        let s = only_rs.structured.unwrap();
        let paths: Vec<&str> = s["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap())
            .collect();
        assert!(!paths.is_empty(), "the filter excluded everything");
        assert!(paths.iter().all(|p| p.ends_with(".rs")), "{paths:?}");

        let without_deep = grep_in(
            &dir,
            serde_json::json!({"pattern": "needle", "path": ".", "exclude": "sub/*"}),
        );
        let s = without_deep.structured.unwrap();
        let paths: Vec<&str> = s["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["path"].as_str().unwrap())
            .collect();
        assert!(!paths.contains(&"sub/d.rs"), "{paths:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bad_search_arguments_are_coded_errors() {
        let dir = search_project("grep-bad");
        // An unbalanced group is a regex error, not a walk that finds nothing.
        let r = grep_in(&dir, serde_json::json!({"pattern": "a(", "path": "."}));
        assert!(!r.ok);
        assert_eq!(r.error_code, Some(ErrorCode::RegexInvalid));

        // A malformed filter arrives as its own code, so a caller can tell
        // "your pattern is broken" from "your filter is broken".
        let r = grep_in(
            &dir,
            serde_json::json!({"pattern": "a", "path": ".", "include": "["}),
        );
        assert_eq!(r.error_code, Some(ErrorCode::GlobInvalid));

        let r = grep_in(
            &dir,
            serde_json::json!({"pattern": "a", "path": "nope.txt"}),
        );
        assert_eq!(r.error_code, Some(ErrorCode::NotADirectory));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn glob_matches_bare_patterns_at_any_depth() {
        let dir = search_project("glob-bare");
        let r = execute(
            &parse_tool_call("glob", &serde_json::json!({"pattern": "*.rs", "path": "."})).unwrap(),
            Some(&dir),
            None,
        );
        let s = r.structured.unwrap();
        let paths: Vec<&str> = s["paths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        // `*.rs` with no `/` means "any .rs file", not "a .rs file in the
        // root" — the second reading matches nothing here and would be
        // useless, since `sub/d.rs` is the interesting one.
        assert!(paths.contains(&"a.rs"), "{paths:?}");
        assert!(paths.contains(&"sub/d.rs"), "{paths:?}");
        assert!(paths.iter().all(|p| p.ends_with(".rs")), "{paths:?}");
        assert!(
            !paths.iter().any(|p| p.contains("node_modules")),
            "{paths:?}"
        );

        // An explicit `**/` means the same thing and is also accepted.
        let r = execute(
            &parse_tool_call(
                "glob",
                &serde_json::json!({"pattern": "**/*.rs", "path": "."}),
            )
            .unwrap(),
            Some(&dir),
            None,
        );
        let s = r.structured.unwrap();
        assert!(s["paths"].as_array().unwrap().len() >= 2, "{s:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- background commands -------------------------------------------------
    //
    // The three tools are one story — start, read, stop — and are tested as
    // one: any of them alone is satisfied by a stub, and the thing that can
    // actually be wrong is the handoff between them (an id that cannot be
    // read, a read that starts in the wrong place, a stop that leaves bytes
    // unread).

    fn bg_output(ctx: &ToolCtx<'_>, id: u64, cursor: Option<u64>) -> (String, serde_json::Value) {
        let r = execute_in(&Tool::CommandOutput { id, cursor }, ctx, None);
        assert!(r.ok, "{:?}", r.error);
        (r.output.unwrap(), r.structured.unwrap())
    }

    #[test]
    fn a_background_command_is_started_read_and_stopped() {
        let (dir, state) = stateful_workspace("background");
        let ctx = ToolCtx::with_state(Some(&dir), &state);

        // Prints a marker, then stays alive until it is stopped — the shape of
        // the thing `run_command` cannot do.
        let started = execute_in(
            &Tool::RunCommandBackground {
                command: "echo bg-marker; sleep 30".into(),
            },
            &ctx,
            None,
        );
        assert!(started.ok, "{:?}", started.error);
        let started = started.structured.unwrap();
        let id = started["id"]
            .as_u64()
            .expect("a handle to read the command by");
        assert!(
            started["pid"].as_u64().is_some(),
            "a started command must report the process it started: {started:?}"
        );

        // A first read carries no cursor, and must show what the command has
        // *already* written rather than only what comes after — otherwise the
        // first look at a command that has already printed reads as a command
        // that printed nothing.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut read = bg_output(&ctx, id, None);
        while !read.0.contains("bg-marker") && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            read = bg_output(&ctx, id, None);
        }
        assert!(
            read.0.contains("bg-marker"),
            "the first read must reach back to the start: {:?}",
            read.0
        );
        assert_eq!(read.1["status"], "running");

        // Resuming from the cursor returns only what is new, which here is
        // nothing — and says so rather than repeating the marker.
        let next = read.1["next_cursor"].as_u64().unwrap();
        let resumed = bg_output(&ctx, id, Some(next));
        assert!(
            !resumed.0.contains("bg-marker"),
            "the marker was already read: {:?}",
            resumed.0
        );
        assert_eq!(resumed.1["next_cursor"].as_u64(), Some(next));
        assert_eq!(resumed.1["more"], false);

        let killed = execute_in(&Tool::KillCommand { id }, &ctx, None);
        assert!(killed.ok, "{:?}", killed.error);
        let killed = killed.structured.unwrap();
        assert_eq!(
            killed["status"], "killed",
            "a stopped command says it was stopped, not that it exited: {killed:?}"
        );
        assert_eq!(killed["already_finished"], false);

        // The output outlives the stop, so what the command printed on its way
        // down is still readable afterwards.
        let after = bg_output(&ctx, id, None);
        assert!(after.0.contains("bg-marker"), "{:?}", after.0);
        assert_eq!(after.1["complete"], true);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reading an id that was never handed out is `PROCESS_NOT_FOUND`, and one
    /// whose output has been released is `OUTPUT_GONE` — the caller can act on
    /// the difference (stop guessing vs. start the command again), so
    /// collapsing them into one "not found" would throw away the answer.
    #[test]
    fn a_missing_command_says_which_kind_of_missing() {
        let (dir, state) = stateful_workspace("background-missing");
        let ctx = ToolCtx::with_state(Some(&dir), &state);

        let r = execute_in(
            &Tool::CommandOutput {
                id: 9999,
                cursor: None,
            },
            &ctx,
            None,
        );
        assert!(!r.ok);
        assert_eq!(r.error_code, Some(ErrorCode::ProcessNotFound));
        let r = execute_in(&Tool::KillCommand { id: 9999 }, &ctx, None);
        assert_eq!(r.error_code, Some(ErrorCode::ProcessNotFound));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
