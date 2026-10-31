//! Argument decoding and JSON Schema generation for the tool surface.
//!
//! `parse_tool_call` is the one place a `Tool` value is built, so every
//! caller — the MCP server and the desktop sandbox alike — decodes a given
//! call identically. The two schema functions emit the `inputSchema` and
//! `outputSchema` a host sees in `tools/list`; both are derived from the
//! same match arms as the parser, which is what keeps them in step with it.

use super::{clip_chars, spec_by_name, Edit, Finding, GrepMode, TodoItem, Tool};
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
        Tool::Monitor {
            path, command_id, ..
        } => match (path, command_id) {
            (Some(p), _) => Some(p.clone()),
            (None, Some(id)) => Some(format!("command #{id}")),
            (None, None) => None,
        },
        Tool::Notify { title, .. } => Some(clip_chars(title, 60)),
        Tool::EnterWorktree { name } => Some(name.clone().unwrap_or_else(|| "new worktree".into())),
        Tool::ExitWorktree { action } => Some(action.clone().unwrap_or_else(|| "keep".into())),
        Tool::ReportFindings { findings, .. } => Some(format!("{} finding(s)", findings.len())),
    }
}
