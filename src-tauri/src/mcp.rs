//! Desktop-local MCP server — the bridge's remote transport.
//!
//! Exposes the bridge tool engine as a Streamable-HTTP MCP server on
//! `127.0.0.1:45147/mcp`. Local MCP hosts (MCP Inspector, Claude Code/Desktop)
//! point at it directly; provider-native connectors (e.g. Claude.ai custom
//! connectors) reach the same endpoint through a tunnel or the hosted gateway.
//! One server, one tool engine, one approval system, one audit trail.
//!
//! Security posture mirrors the rest of the bridge:
//! - Bound to loopback only. The endpoint exposes no write tool unless
//!   `mcp_allow_write` is set (a connector is read-only first).
//! - Every call routes through `crate::tool_call(..., "mcp")`, so the desktop
//!   stays the sole approval authority and session grants stay source-scoped.
//! - Tool results are capped so a connector (provider ceiling ≈150k chars)
//!   never receives an unbounded blob.

use crate::bridge;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, GetPromptRequestParams,
    GetPromptResponse, GetPromptResult, ListPromptsResult, ListToolsResult, PaginatedRequestParams,
    Prompt, PromptMessage, Role, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Manager};

/// Loopback bind address for the connector's MCP endpoint. Bound to loopback
/// only, which is the security posture; reaching it from anywhere else is an
/// explicit opt-in (`LEXSUS_MCP_ALLOWED_HOSTS`).
pub const ADDR: &str = "127.0.0.1:45147";
/// MCP endpoints are usually mounted under `/mcp`.
pub const MCP_PATH: &str = "/mcp";
/// How long an MCP-originated call waits for the desktop approval. Kept well
/// under the ~300 s ceiling of provider connectors so the provider never
/// kills the request before the desktop decision lands.
pub const APPROVAL_WAIT_SECS: u64 = 120;
/// Connector tool-result cap (~140k chars), mirroring the ~150k-char result
/// ceiling of hosted connectors with margin to spare.
const RESULT_CHAR_CAP: usize = 140_000;

/// Non-mutating surface exposed whenever `allow_write` is off (and always).
const READ_ONLY: &[&str] = &[
    "list_tools",
    "describe_tool",
    "read_file",
    "list_directory",
    "read_many_files",
    "grep",
    "glob",
    "git_status",
    "git_diff",
    "git_log",
    "git_branches",
    "git_show",
    "git_commit_diff",
    // Reading the project memory changes nothing: the task list, the facts
    // and the session archive are all reads.
    "todo_read",
    "get_facts",
    "list_sessions",
    "get_handoff",
    // Phase 7–10 reads. The web pair reaches the network but changes nothing
    // local; the language-server four answer questions; the agent-loop pair
    // talk to the human rather than the workspace; and the media/finding
    // tools only report what is already there.
    "web_fetch",
    "web_search",
    "notebook_read",
    "lsp_diagnostics",
    "lsp_definition",
    "lsp_references",
    "lsp_symbols",
    "ask_user",
    "propose_plan",
    "monitor",
    "notify",
    "read_media",
    "report_findings",
];

/// Everything that mutates the workspace (or runs shell), gated behind
/// `allow_write`.
const WRITE: &[&str] = &[
    "write_file",
    "edit_file",
    "multi_edit",
    "apply_patch",
    "delete_file",
    "move_file",
    "copy_file",
    "create_directory",
    "run_command",
    "run_command_background",
    // Gated with the two that bracket it, even though it only reads. What it
    // reads is a *process's* output, not the workspace: text a command chose
    // to print, which the read-only promise says nothing about. And with
    // write off there cannot be a command for it to report on, so exposing it
    // would be a handle to nothing.
    "command_output",
    "kill_command",
    "git_add",
    "git_unstage",
    "git_commit",
    "git_checkout",
    "git_create_branch",
    // Recording a fact or a task list writes to the database rather than the
    // workspace, but it is still a write: it changes what the next reader —
    // the developer, in the desktop UI — will see.
    "todo_write",
    "set_objective",
    "remember_decision",
    "remember_constraint",
    "remember_attempt",
    "request_handoff",
    // Phase 7–10 writes. A notebook edit rewrites a document; a worktree is
    // created and removed through git; an artifact is a written file; and a
    // delegated task is gated because it can do any of the above.
    "notebook_edit",
    "delegate_task",
    "enter_worktree",
    "exit_worktree",
    "publish_artifact",
];

/// Canonical tool names the server exposes for a given `allow_write` state.
pub fn exposed_names(allow_write: bool) -> Vec<&'static str> {
    let mut names: Vec<&'static str> = READ_ONLY.to_vec();
    if allow_write {
        names.extend_from_slice(WRITE);
    }
    names
}

/// Whether a tool call is acceptable given the current `allow_write` state.
pub fn tool_visible(name: &str, allow_write: bool) -> bool {
    READ_ONLY.contains(&name) || (allow_write && WRITE.contains(&name))
}

/// Human-readable title for a tool descriptor: `read_file` → `Read file`.
/// Connector UIs show this next to the name, where the raw snake_case reads
/// as an identifier rather than a sentence.
fn title_for(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().replace('_', " "),
        None => String::new(),
    }
}

/// Build the rmcp [`Tool`] descriptor for one canonical tool name, from the
/// bridge's single source of truth: the SPECS row (summary, argument list,
/// approval class), the Stage-1 [`bridge::tool_input_schema`], and the
/// Stage-2 [`bridge::output_schema`]. Returns `None` for a name with no SPECS
/// row or schema (shouldn't happen — guarded by tests).
///
/// Everything here is derived, never authored: a tool added to SPECS shows up
/// on the connector with a description, a schema and correct hints without
/// this function being touched.
fn mcp_tool(name: &'static str) -> Option<Tool> {
    let spec = bridge::spec_by_name(name)?;
    let description = bridge::tool_description(name)?;
    let schema = bridge::tool_input_schema(name)?;
    let schema = schema.as_object()?.clone();
    let read_only = READ_ONLY.contains(&name);
    let destructive = spec.approval == bridge::Approval::Destructive;
    let mut tool = Tool::new_with_raw(name, Some(description.into()), schema)
        .with_title(title_for(name))
        .with_annotations(
            ToolAnnotations::default()
                .read_only(read_only)
                .destructive(destructive)
                // Reads and the meta tools are safe to repeat; a write that
                // lands twice is not, so only the read surface claims this.
                .idempotent(read_only),
        );
    // Declaring an output schema is a promise that `structuredContent`
    // conforms to it, so this is emitted only where `output_schema` has a row
    // — and `structured_output_matches_its_declared_schema` keeps the promise
    // honest rather than trusting it.
    if let Some(output) = bridge::output_schema(name)
        .as_ref()
        .and_then(|v| v.as_object())
    {
        tool = tool.with_raw_output_schema(Arc::new(output.clone()));
    }
    Some(tool)
}

/// A running MCP server whose every call funnels through the bridge engine.
/// `allow_write` is shared live state (toggled at runtime), not a boot-time
/// constant, so the exposed surface reflects the current policy without a
/// reconnect.
#[derive(Clone)]
struct LexsusServer {
    app: AppHandle,
    allow_write: Arc<AtomicBool>,
}

impl ServerHandler for LexsusServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_prompts()
                .build(),
        )
        .with_instructions(
            "Lexsus local-workspace tools. Reads run automatically unless the path is \
             sensitive (then the desktop app must approve). Write and command tools are \
             hidden until write access is enabled, and always require the desktop \
             approval.",
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
        let allow_write = self.allow_write.load(Ordering::SeqCst);
        let tools: Vec<Tool> = exposed_names(allow_write)
            .into_iter()
            .filter_map(mcp_tool)
            .collect();
        async move { Ok(ListToolsResult::with_all_items(tools)) }
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, McpError>> + MaybeSendFuture + '_ {
        let prompts = prompt_descriptors();
        async move { Ok(ListPromptsResult::with_all_items(prompts)) }
    }

    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResponse, McpError>> + MaybeSendFuture + '_ {
        let name = request.name.clone();
        let app = self.app.clone();
        async move {
            if name != CONTINUE_WORK_PROMPT {
                return Err(McpError::invalid_params(
                    format!("no such prompt: {name}"),
                    None,
                ));
            }
            // Building the card reads the transcript and the database, so it
            // goes on the blocking pool rather than a runtime worker.
            let prompt = tokio::task::spawn_blocking(move || continue_work_prompt(&app))
                .await
                .map_err(|_| McpError::internal_error("handoff task failed", None))??;
            Ok(prompt.into())
        }
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + MaybeSendFuture + '_ {
        let name = request.name.to_string();
        let allow_write = self.allow_write.load(Ordering::SeqCst);
        // Arguments arrive as a JSON object; the parser takes a serde Value.
        let args = serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        let app = self.app.clone();
        // The id of *this* call, so anything it spawns is registered under it
        // and `cancel_request` can find it. Namespaced because the desktop
        // path owns its processes by approval id, and the two id spaces must
        // not be able to collide.
        //
        // Naming the call rather than watching rmcp's `context.ct` is
        // deliberate. That token is only cancelled on a `CancelledNotification`
        // and is otherwise dropped when the call returns, so a watcher task
        // awaiting it would outlive every call that ends normally — one leaked
        // task per tool call, to handle the rare case. Naming is enough: a
        // caller that wants a running command stopped knows the id it is
        // cancelling and can say so.
        let owner = format!("mcp:{}", context.id);
        async move {
            // Not exposed under the current write policy → a caller-visible
            // tool error, not a protocol error: the tool *is* known, just
            // disabled.
            if !tool_visible(&name, allow_write) {
                return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                    "tool '{name}' is not enabled on this connector (write tools are disabled)"
                ))])
                .into());
            }

            let tool = match bridge::parse_tool_call(&name, &args) {
                Ok(tool) => tool,
                Err(message) => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                        "invalid arguments for '{name}': {message}"
                    ))])
                    .into());
                }
            };

            // Execution can block up to APPROVAL_WAIT_SECS on the desktop
            // decision, so never hold a runtime worker — run it on the
            // blocking pool.
            let result = tokio::task::spawn_blocking(move || {
                // Anything this call spawns is registered under the call's own
                // id, so `processes_list` attributes it and `cancel_request`
                // can reach it. The guard clears it on the way out, including
                // when the call panics.
                let _owner = crate::process::own_current_thread(Some(owner));
                crate::tool_call(&app, tool, bridge::SOURCE_MCP)
            })
            .await
            .map_err(|_| McpError::internal_error("mcp execution task failed", None))?;

            Ok(call_tool_response(result).into())
        }
    }
}

/// The one prompt this server offers: carry on from where the last session
/// stopped.
///
/// A prompt *as well as* the `get_handoff` tool, because a connector can
/// surface a prompt natively — the user gets "Continue work" in the composer
/// instead of having to know to ask for a handoff. The two are doors onto the
/// same card, not two implementations of it: both go through
/// [`crate::build_handoff_impl`], and both render it with
/// [`bridge::render_handoff`], so they cannot disagree about what was
/// happening.
pub const CONTINUE_WORK_PROMPT: &str = "continue_work";

/// What `prompts/list` advertises. A pure function of nothing, which is the
/// point: the handler above is then plumbing, and this is what a test
/// asserts.
fn prompt_descriptors() -> Vec<Prompt> {
    vec![Prompt::new(
        CONTINUE_WORK_PROMPT,
        Some(
            "Load the handoff card from the interrupted session and carry the \
             task on from its next step.",
        ),
        None,
    )]
}

/// Compose the prompt's text from a handoff card.
///
/// Pure: a card in, text out. Everything hard about this prompt is in the
/// composition — what to tell a model to *do* with a card — so keeping the
/// app handle out of it is what makes that assertable without a session, a
/// database or a desktop.
///
/// The cap lives here rather than at the call site because the card's size is
/// a property of the *card*: `objective` and `files` come from the session, so
/// an unusually busy one produces an unusually large prompt. Capping where the
/// text is made means there is no path that returns an uncapped one.
fn continue_work_text(handoff: &crate::Handoff) -> String {
    cap_text(&format!(
        "An earlier session on this project was interrupted. This is the \
         handoff card it left:\n\n{}\n\n\
         Carry the task on from the next step above. Do not retry anything \
         under \"failed attempts\" — those were already tried and did not \
         work. Anything under \"decisions\" is settled: do not re-open it \
         without saying why.",
        bridge::render_handoff(handoff)
    ))
}

/// Build the `continue_work` prompt — the effectful half of the pair.
fn continue_work_prompt(app: &AppHandle) -> Result<GetPromptResult, McpError> {
    let state = app.state::<crate::AppState>();
    let handoff = crate::build_handoff_impl(&state)
        .map_err(|e| McpError::internal_error(format!("could not build the handoff: {e}"), None))?;
    Ok(GetPromptResult::new(vec![PromptMessage::new_text(
        Role::User,
        continue_work_text(&handoff),
    )]))
}

/// Turn a finished bridge call into the wire response.
///
/// Split out of `call_tool` so the shaped result can be asserted directly,
/// without standing up an MCP session to look at it.
fn call_tool_response(result: bridge::ToolResult) -> CallToolResult {
    if result.ok {
        let text = cap_text(result.output.as_deref().unwrap_or_default());
        let mut blocks = vec![ContentBlock::text(text)];
        // A media result carries its bytes out of band, in `result.media`,
        // because base64 in the text or the structured half would be capped
        // into something unopenable. The caption above still says what it is.
        if let Some(media) = &result.media {
            if media.kind == "image" {
                blocks.push(ContentBlock::image(
                    media.base64.clone(),
                    media.media_type.clone(),
                ));
            } else {
                // A PDF is not an image; it crosses as an embedded blob
                // resource, which is the block the spec defines for exactly
                // this case.
                blocks.push(ContentBlock::resource(
                    rmcp::model::ResourceContents::blob(media.base64.clone(), "lexsus://media")
                        .with_mime_type(media.media_type.clone()),
                ));
            }
        }
        let mut response = CallToolResult::success(blocks);
        // Assigned to the field directly, never via
        // `CallToolResult::structured()`: that constructor sets `content` to a
        // raw `value.to_string()` dump, which would throw away the readable
        // text this result is built around.
        response.structured_content = result.structured;
        response
    } else {
        let message = result
            .error
            .as_deref()
            .unwrap_or("the tool failed (no detail)");
        let mut response = CallToolResult::error(vec![ContentBlock::text(message.to_string())]);
        // The structured code is the half a model can branch on: "retry with a
        // different path" and "your old_string did not match" are the same wall
        // of prose otherwise. Null when the failure carries no code — an
        // approval that timed out is not a tool error, and inventing a code for
        // it would be worse than admitting there isn't one.
        response.structured_content = Some(serde_json::json!({
            "error_code": result
                .error_code
                .as_ref()
                .and_then(|c| serde_json::to_value(c).ok()),
            "message": message,
        }));
        response
    }
}

/// Cap a tool-result string to [`RESULT_CHAR_CAP`] chars, keeping the existing
/// "cut and say so" convention so a model that hits the cap knows the result
/// is incomplete rather than believing it saw everything.
///
/// The marker is deliberately tool-neutral. It used to advise calling
/// `read_file` with an offset, which is only ever true for `read_file` — a
/// truncated `git_status` or `run_command` was told to page a file. A read
/// that gets cut now carries its continuation in `structured_content.next_offset`
/// instead, so the text does not have to guess at what it is.
pub fn cap_text(text: &str) -> String {
    if text.chars().count() <= RESULT_CHAR_CAP {
        return text.to_string();
    }
    let cut: String = text.chars().take(RESULT_CHAR_CAP).collect();
    format!("{cut}\n… [output truncated at {RESULT_CHAR_CAP} chars]")
}

/// Extra `Host` values the DNS-rebinding guard should accept, from
/// `LEXSUS_MCP_ALLOWED_HOSTS` (comma-separated). A tunnel or reverse proxy
/// forwards its own `Host`, so reaching the endpoint through one means either
/// listing that host here or rewriting `Host` at the proxy. Loopback is always
/// allowed and is the default; anything beyond it is opt-in, never hardcoded.
fn allowed_hosts_from_env() -> Vec<String> {
    std::env::var("LEXSUS_MCP_ALLOWED_HOSTS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .collect()
}

/// The config the endpoint actually runs with: JSON request/response for
/// stateless calls (SEP-2567) rather than text/event-stream where possible,
/// plus whichever hosts the DNS-rebinding guard should accept.
///
/// Rebuilt on demand rather than shared, so [`effective_allowed_hosts`] can
/// report the enforced list without a second, hand-maintained copy of rmcp's
/// loopback defaults drifting away from the ones `serve` passes in.
fn server_config() -> StreamableHttpServerConfig {
    let mut config = StreamableHttpServerConfig::default();
    config.json_response = true;
    config.allowed_hosts.extend(allowed_hosts_from_env());
    config
}

/// Host authorities the DNS-rebinding guard accepts, in the order rmcp checks
/// them. Surfaced through `mcp_status` because a tunnel whose host is missing
/// here is answered with a bare `403` — which a connector reads as "no MCP
/// server here" and falls back to OAuth, blaming the sign-in service for what
/// is really a Host mismatch.
pub fn effective_allowed_hosts() -> Vec<String> {
    server_config().allowed_hosts
}

/// Bind and serve the MCP endpoint forever. Runs on its own tokio runtime on
/// a detached thread (the same pattern as the rest of the core's background
/// work) so it never interferes with the Tauri event loop, and its
/// blocking-pool wait for approvals cannot stall anything else. `listening`
/// is flipped once the loopback socket is actually bound.
pub fn spawn_server(app: AppHandle, allow_write: Arc<AtomicBool>, listening: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("mcp")
            .build()
            .expect("mcp tokio runtime");
        rt.block_on(async move {
            if let Err(e) = serve(app, allow_write, listening).await {
                eprintln!("[mcp] server exited: {e}");
            }
        });
    });
}

async fn serve(
    app: AppHandle,
    allow_write: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
) -> std::io::Result<()> {
    let config = server_config();
    // A fresh handler per connection/session, each sharing the live flag and
    // the app handle.
    let factory = move || {
        Ok(LexsusServer {
            app: app.clone(),
            allow_write: allow_write.clone(),
        })
    };
    let service: StreamableHttpService<LexsusServer, LocalSessionManager> =
        StreamableHttpService::new(factory, Arc::new(LocalSessionManager::default()), config);

    let listener = tokio::net::TcpListener::bind(ADDR).await?;
    listening.store(true, Ordering::SeqCst);
    eprintln!("[mcp] listening on http://{ADDR}{MCP_PATH}");
    // The canonical endpoint is `/mcp`, but some provider connectors (Claude.ai)
    // treat the bare origin as the resource URL and POST `initialize` to `/`.
    // A 404 there is misread as an auth failure, so the same engine answers at
    // both paths. `/.well-known/*` is left to 404 so OAuth discovery still
    // reads as "no auth advertised".
    let router = axum::Router::new()
        .nest_service(MCP_PATH, service.clone())
        .route("/", axum::routing::any_service(service));
    axum::serve(listener, router)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The READ_ONLY/WRITE split must exactly partition the SPECS canonical
    /// names — no SPECS tool may be silently missing from the MCP surface,
    /// and none may appear twice.
    #[test]
    fn surface_partitions_all_spec_tools() {
        let all: Vec<&str> = bridge::SPECS.iter().map(|s| s.name).collect();
        let mut seen: Vec<&str> = Vec::new();
        for name in &all {
            let visible = tool_visible(name, true);
            assert!(visible, "every SPECS tool must be exposable: {name}");
            assert!(!seen.contains(name), "tool listed twice: {name}");
            seen.push(name);
        }
        assert_eq!(
            exposed_names(false),
            READ_ONLY,
            "no-write surface must be exactly READ_ONLY"
        );
        let read_only: Vec<&str> = exposed_names(false);
        let with_write: Vec<&str> = exposed_names(true);
        // True partition: with write enabled it's READ_ONLY ++ WRITE.
        let mut expect: Vec<&str> = READ_ONLY.to_vec();
        expect.extend_from_slice(WRITE);
        assert_eq!(with_write, expect);
        assert!(read_only.iter().all(|n| !WRITE.contains(n)));
        // And both halves are real SPECS names.
        for n in READ_ONLY.iter().chain(WRITE) {
            assert!(
                bridge::spec_by_name(n).is_some(),
                "unknown tool in surface: {n}"
            );
        }
    }

    /// The no-write surface must be genuinely non-mutating: its SPECS rows
    /// may never ask Always/Destructive approval (those are the write/
    /// command classes). Meta + read tools only.
    #[test]
    fn read_only_surface_has_no_write_tools() {
        for name in READ_ONLY {
            let approval = bridge::spec_by_name(name)
                .unwrap_or_else(|| panic!("no spec for {name}"))
                .approval;
            assert!(
                approval != bridge::Approval::Always && approval != bridge::Approval::Destructive,
                "{name} needs {approval:?} but sits in the read-only surface"
            );
        }
    }

    /// Every SPECS tool builds an mcp descriptor carrying its SPECS summary
    /// and a JSON-Schema inputSchema (the Stage-1 drift guard, checked from
    /// the MCP side too).
    #[test]
    fn every_exposed_tool_has_descriptor_and_schema() {
        for name in bridge::SPECS.iter().map(|s| s.name) {
            let t = mcp_tool(name).unwrap_or_else(|| panic!("no mcp Tool for {name}"));
            assert_eq!(t.name.as_ref(), name);
            assert!(
                t.description.is_some(),
                "{name} descriptor is missing its SPECS summary"
            );
            let props = t.input_schema.get("properties").and_then(|v| v.as_object());
            assert!(props.is_some(), "{name} inputSchema has no properties");
            // Read-only tools advertise the readOnlyHint a connector's
            // "disable write tools" gate can rely on.
            let expected_read_only = READ_ONLY.contains(&name);
            let hinted_read_only = t
                .annotations
                .as_ref()
                .and_then(|a| a.read_only_hint)
                .unwrap_or(false);
            assert_eq!(
                hinted_read_only, expected_read_only,
                "{name} readOnlyHint mismatch"
            );
        }
    }

    #[test]
    fn write_tools_hidden_until_enabled() {
        assert!(tool_visible("write_file", true));
        assert!(!tool_visible("write_file", false));
        assert!(tool_visible("read_file", false));
        assert!(tool_visible("read_file", true));
        assert!(tool_visible("run_command", true));
        assert!(!tool_visible("run_command", false));
    }

    /// The prompt is advertised under the name a connector will show, and its
    /// text carries the card rather than a second rendering of the state.
    ///
    /// This is the pure half of the pair — `continue_work_prompt`'s only
    /// remaining job is to fetch a `Handoff` and hand it here — which is what
    /// lets the composition be checked without a session, a database or a
    /// desktop.
    #[test]
    fn continue_work_carries_the_handoff_card() {
        assert_eq!(
            prompt_descriptors()
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            vec![CONTINUE_WORK_PROMPT]
        );

        let handoff = crate::Handoff {
            objective: "finish the memory tools".into(),
            progress_percent: 40,
            files_changed: 2,
            errors_remaining: 1,
            next_step: Some("wire the prompt".into()),
            files: vec!["src/mcp.rs".into()],
            context: None,
            end_reason: Some("the session ended".into()),
            decisions: vec!["one connector session".into()],
            failed_attempts: vec!["filing under the newest session".into()],
            constraints: vec!["no new dependencies".into()],
            generated_at: "0".into(),
        };

        let card = bridge::render_handoff(&handoff);
        let text = continue_work_text(&handoff);
        assert_eq!(
            text.matches(&card).count(),
            1,
            "the prompt must contain the card exactly once:\n{text}"
        );
        assert!(text.contains("finish the memory tools"), "{text}");
        // The two instructions that make the card actionable rather than
        // decorative: a model handed a card and nothing else will happily
        // repeat the failed attempt it just read about.
        assert!(text.contains("failed attempts"), "{text}");
        assert!(text.contains("decisions"), "{text}");

        // The card is unbounded — the objective and `files` both come from the
        // session — so the prompt goes through the same cap a tool result
        // does. A connector handed 200k chars has the request killed by the
        // provider instead of getting a truncated answer.
        let huge = crate::Handoff {
            objective: "x".repeat(RESULT_CHAR_CAP + 100),
            ..handoff.clone()
        };
        let capped = continue_work_text(&huge);
        assert!(capped.chars().count() <= RESULT_CHAR_CAP + 100);
        assert!(capped.contains("output truncated at"));
    }

    #[test]
    fn result_cap_truncates_and_marks() {
        let short = "hello";
        assert_eq!(cap_text(short), short);
        let big = "x".repeat(RESULT_CHAR_CAP + 50);
        let capped = cap_text(&big);
        // Exactly the cap's worth of original content, then a cut marker so a
        // model that hits the cap knows to page for the rest.
        assert!(capped.chars().take(RESULT_CHAR_CAP).all(|c| c == 'x'));
        assert!(capped.contains("output truncated at"));
        assert!(capped.ends_with(']'));
    }

    #[test]
    fn unknown_tool_never_in_exposed_surface() {
        // Names that are near misses, not roadmap tools: a name that is *on*
        // the roadmap stops being a near miss the day it is implemented, so
        // this list holds misspellings and the empty string instead.
        for name in ["nope", "read_fil", "read-file", "GREP ", ""] {
            assert!(!tool_visible(name, false), "{name}");
            assert!(!tool_visible(name, true), "{name}");
        }
    }

    /// The loopback defaults are the guard's floor, and
    /// `LEXSUS_MCP_ALLOWED_HOSTS` may only add to them — never replace them.
    /// `mcp_status.allowed_hosts` reports exactly this list, so pinning it
    /// keeps the UI honest about which `Host` values the endpoint answers.
    #[test]
    fn loopback_hosts_are_always_allowed() {
        let hosts = effective_allowed_hosts();
        for loopback in ["localhost", "127.0.0.1", "::1"] {
            assert!(
                hosts.iter().any(|h| h == loopback),
                "loopback host {loopback} missing from {hosts:?}"
            );
        }
    }

    /// Declaring `outputSchema` is a promise about `structuredContent`, so
    /// every exposed tool has to be able to keep it: a schema row, and a
    /// descriptor that actually advertises it. Paired with the bridge-side
    /// `structured_output_matches_its_declared_schema`, which checks the
    /// payloads match the promise.
    #[test]
    fn every_exposed_tool_declares_output_schema() {
        for name in bridge::SPECS.iter().map(|s| s.name) {
            let spec = bridge::spec_by_name(name).expect("spec row");
            assert!(
                bridge::output_schema(name).is_some(),
                "{name} has no output_schema row"
            );
            let tool = mcp_tool(name).unwrap_or_else(|| panic!("no descriptor for {name}"));
            assert!(
                tool.output_schema.is_some(),
                "{name} descriptor advertises no outputSchema"
            );
            assert!(tool.title.is_some(), "{name} descriptor has no title");
            let description = tool.description.as_deref().unwrap_or_default();
            assert!(
                description.contains("Approval:"),
                "{name} description omits the approval line"
            );
            if !spec.args.is_empty() {
                assert!(
                    description.contains("Args:"),
                    "{name} description omits its argument list"
                );
            }
        }
        // No schema for a name that isn't a tool. Aliases resolve on purpose
        // (same as `tool_input_schema`), so only genuinely unknown names are
        // checked here.
        for unknown in ["nope", "read_fil", "", "definitely_not_a_tool"] {
            assert!(bridge::output_schema(unknown).is_none(), "{unknown}");
        }
        assert!(
            bridge::output_schema("default_api.read_file").is_some(),
            "aliases must resolve to their canonical tool's schema"
        );
    }

    /// The whole point of the rework: a failing call tells the caller *how* it
    /// failed. Before, `error_code` was computed in the core and then dropped
    /// on the floor here, leaving the model to guess from prose.
    #[test]
    fn error_code_survives_the_mcp_boundary() {
        let failed = bridge::ToolResult::err_code(
            bridge::ErrorCode::StringNotFound,
            "a.txt: old_string not found",
        );
        let response = call_tool_response(failed);
        assert_eq!(response.is_error, Some(true));
        let structured = response.structured_content.expect("structured error");
        assert_eq!(structured["error_code"], "STRING_NOT_FOUND");
        assert_eq!(structured["message"], "a.txt: old_string not found");

        // A failure with no code says so rather than inventing one — an
        // approval that timed out is not a tool error.
        let uncoded = bridge::ToolResult::err("approval timed out");
        let response = call_tool_response(uncoded);
        let structured = response.structured_content.expect("structured error");
        assert!(structured["error_code"].is_null(), "{structured}");
    }

    /// Success keeps the readable text *and* carries the structured half.
    /// `CallToolResult::structured()` would have replaced the text with a JSON
    /// dump, so this pins the two together.
    #[test]
    fn success_keeps_readable_text_and_structured_content() {
        let ok = bridge::ToolResult::ok_structured(
            "edited a.txt — 1 replacement at line 4".to_string(),
            serde_json::json!({"path": "a.txt", "replacements": 1, "first_line": 4}),
        );
        let response = call_tool_response(ok);
        assert_eq!(response.is_error, Some(false));
        let text = match response.content.first() {
            Some(ContentBlock::Text(t)) => t.text.clone(),
            other => panic!("expected a text block, got {other:?}"),
        };
        assert!(
            text.starts_with("edited a.txt"),
            "readable text was replaced: {text}"
        );
        let structured = response.structured_content.expect("structured content");
        assert_eq!(structured["first_line"], 4);
    }
    /// **A tool that always asks, or that can destroy work, is never on the
    /// surface while writes are off.**
    ///
    /// `surface_partitions_all_spec_tools` bounds the surface, and
    /// `read_only_surface_has_no_write_tools` bounds the read-only half — but
    /// that second test walks `READ_ONLY` and checks each entry is benign. It
    /// therefore cannot see the dangerous omission: a *new* tool declared
    /// `Always` or `Destructive` and forgotten in `WRITE`. Here the tool's
    /// own approval class decides, so leaving it out of `WRITE` fails the
    /// build instead of shipping a destructive tool into a read-only session.
    ///
    /// Quantified over `SPECS`, so every future tool is covered on declaration.
    #[test]
    fn every_always_or_destructive_tool_is_write_gated() {
        let mut gated = 0usize;
        for spec in bridge::SPECS {
            let asks = matches!(
                spec.approval,
                bridge::Approval::Always | bridge::Approval::Destructive
            );
            if !asks {
                continue;
            }
            gated += 1;
            assert!(
                !tool_visible(spec.name, false),
                "{} requires {:?} approval but is exposed while writes are off",
                spec.name,
                spec.approval
            );
            assert!(
                !exposed_names(false).contains(&spec.name),
                "{} requires {:?} approval but is listed in the no-write surface",
                spec.name,
                spec.approval
            );
            // ...and turning writes on must expose it, or the gate is not a
            // gate but a blacklist.
            assert!(
                tool_visible(spec.name, true),
                "{} requires {:?} approval but is not exposed even with writes on",
                spec.name,
                spec.approval
            );
        }
        assert!(
            gated >= 6,
            "only {gated} always/destructive tools found — the approval classes \
             have drifted and this test is no longer covering the gate"
        );
    }

    /// A snapshot of the workspace tree: every entry, by path.
    ///
    /// A directory is recorded as `"rel/"` with no bytes, because
    /// `create_directory` mutates by adding an empty one — a files-only
    /// snapshot would let it through. File contents are compared, not mtimes:
    /// a rewrite that restores the same bytes is not a mutation the promise
    /// cares about, and mtime resolution would only add flakiness.
    fn tree_snapshot(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
        fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                // `.git` is excluded: `git_status` legitimately touches refs,
                // packs and the index, and none of that is workspace content.
                if rel == ".git" || rel.starts_with(".git/") {
                    continue;
                }
                if path.is_dir() {
                    out.push((format!("{rel}/"), Vec::new()));
                    walk(&path, base, out);
                } else if let Ok(bytes) = std::fs::read(&path) {
                    out.push((rel, bytes));
                }
            }
        }
        let mut out = Vec::new();
        walk(dir, dir, &mut out);
        out.sort();
        out
    }

    /// **The no-write surface cannot change the workspace.**
    ///
    /// This is the promise `allow_write == false` makes to whoever owns the
    /// folder, and `read_only_surface_has_no_write_tools` only *infers* it
    /// from the approval class. That inference is not sound: the class says
    /// how loudly a call asks, not what it does — `edit_file` and `multi_edit`
    /// are `SensitivePathOnly`, and `create_directory` is `Auto`, yet all
    /// three mutate and are correctly gated behind `WRITE`. So the surface is
    /// checked by running it rather than by reasoning about it: every exposed
    /// tool is driven over a real workspace, and the tree is compared byte for
    /// byte before and after.
    ///
    /// Quantified over [`exposed_names`], so a read tool added later is
    /// covered on declaration.
    #[test]
    fn the_no_write_surface_cannot_change_the_workspace() {
        let dir = std::env::temp_dir().join(format!("mcp-readonly-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha\nbeta\ngamma\n").unwrap();
        std::fs::write(dir.join("b.txt"), "one\ntwo\n").unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/nested.txt"), "nested\n").unwrap();
        // A repository with a commit, not just `.git`: `git_log`, `git_show`
        // and `git_commit_diff` all need history, and on a bare `init` they
        // would fail — which this test reads as a missing fixture rather than
        // as the read-only violation it is looking for.
        let repo = git2::Repository::init(&dir).unwrap();
        {
            let mut cfg = repo.config().unwrap();
            cfg.set_str("user.name", "Lexsus Test").unwrap();
            cfg.set_str("user.email", "test@example.invalid").unwrap();
        }
        repo.index()
            .unwrap()
            .add_all(["*"], git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        {
            let mut index = repo.index().unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = repo.signature().unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "fixtures", &tree, &[])
                .unwrap();
        }

        let before = tree_snapshot(&dir);
        // The database lives outside `dir` (see `test_state`), so a memory
        // tool writing to it cannot perturb the workspace snapshot below.
        let state = crate::test_state(&dir);
        let ctx = bridge::ToolCtx::with_state(Some(&dir), &state);

        let mut ran = 0usize;
        for name in exposed_names(false) {
            // Args shaped to *succeed*, so the test cannot pass by having
            // every call rejected before it does anything. `None` means this
            // fixture does not know the tool — and since the loop demands
            // every exposed name be driven, an unlisted tool fails here by
            // name rather than as a puzzling argument error.
            let args = match name {
                "read_file" => Some(serde_json::json!({"path": "a.txt"})),
                "list_directory" => Some(serde_json::json!({"path": "."})),
                "read_many_files" => {
                    Some(serde_json::json!({"paths": ["a.txt", "sub/nested.txt"]}))
                }
                "describe_tool" => Some(serde_json::json!({"name": "read_file"})),
                "grep" => Some(serde_json::json!({"pattern": "a", "path": "."})),
                "glob" => Some(serde_json::json!({"pattern": "*.txt", "path": "."})),
                "git_diff" => Some(serde_json::json!({})),
                "git_log" => Some(serde_json::json!({"limit": 5})),
                "git_show" | "git_commit_diff" => Some(serde_json::json!({"oid": "HEAD"})),
                "list_tools" | "git_status" | "git_branches" => Some(serde_json::json!({})),
                // The memory *reads*. They answer from the database, so the
                // fixture supplies one — an empty state would drive them all
                // down their "no database" error path and the test would pass
                // without ever exercising the tools.
                "todo_read" | "get_facts" | "list_sessions" | "get_handoff" => {
                    Some(serde_json::json!({}))
                }
                other => panic!(
                    "{other} is on the no-write surface but this test has no \
                     fixture for it — add one, and check it is really read-only"
                ),
            }
            .expect("fixture");
            let tool =
                bridge::parse_tool_call(name, &args).unwrap_or_else(|e| panic!("{name}: {e}"));
            let result = bridge::execute_in(&tool, &ctx, None);
            assert!(
                result.ok,
                "{name} failed on the read surface: {:?}",
                result.error
            );
            assert!(
                result.pending.is_none(),
                "{name} is on the no-write surface but needed approval"
            );
            ran += 1;
        }

        let after = tree_snapshot(&dir);
        assert_eq!(
            before.len(),
            after.len(),
            "the read-only surface added or removed a file: {:?} -> {:?}",
            before.iter().map(|(p, _)| p).collect::<Vec<_>>(),
            after.iter().map(|(p, _)| p).collect::<Vec<_>>()
        );
        for ((bp, bb), (ap, ab)) in before.iter().zip(&after) {
            assert_eq!(bp, ap, "the read-only surface renamed a file");
            assert_eq!(
                bb, ab,
                "the read-only surface rewrote {bp} — writes must not be                  reachable while allow_write is off"
            );
        }

        assert_eq!(
            ran,
            exposed_names(false).len(),
            "not every exposed tool was driven"
        );
        assert!(
            ran >= 5,
            "only {ran} read tools exercised — surface shrank?"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
