//! The tool registry: what exists, how it is named, and how it is grouped.
//!
//! `SPECS` is the single source of truth for the tool surface. Every tool is
//! registered here exactly once, with the approval class and manifest group
//! that `parse_tool_call` (`schema.rs`) and the policy engine (`super`) both
//! read. Adding a tool means adding an entry here and a match arm there.

use super::{Approval, Tool, ToolSpec};
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
        aliases: &[
            "symbols",
            "document_symbols",
            "workspace_symbols",
            "outline",
        ],
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
pub(super) fn approval_phrase(approval: Approval) -> &'static str {
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
pub(super) fn describe_spec(s: &ToolSpec) -> String {
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
