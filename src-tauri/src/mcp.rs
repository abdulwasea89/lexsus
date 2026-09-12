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
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{MaybeSendFuture, RequestContext};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::AppHandle;

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
    "git_status",
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
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
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

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, McpError>> + MaybeSendFuture + '_ {
        let name = request.name.to_string();
        let allow_write = self.allow_write.load(Ordering::SeqCst);
        // Arguments arrive as a JSON object; the parser takes a serde Value.
        let args = serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        let app = self.app.clone();
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
                crate::tool_call(&app, tool, bridge::SOURCE_MCP)
            })
            .await
            .map_err(|_| McpError::internal_error("mcp execution task failed", None))?;

            Ok(call_tool_response(result).into())
        }
    }
}

/// Turn a finished bridge call into the wire response.
///
/// Split out of `call_tool` so the shaped result can be asserted directly,
/// without standing up an MCP session to look at it.
fn call_tool_response(result: bridge::ToolResult) -> CallToolResult {
    if result.ok {
        let text = cap_text(result.output.as_deref().unwrap_or_default());
        let mut response = CallToolResult::success(vec![ContentBlock::text(text)]);
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
    // Default already supplies a fresh cancellation token + loopback-only
    // allowed hosts; we just prefer JSON request/response for stateless calls
    // (SEP-2567) over text/event-stream where possible.
    let mut config = StreamableHttpServerConfig::default();
    config.json_response = true;
    config.allowed_hosts.extend(allowed_hosts_from_env());
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
        for name in ["nope", "grep", "read_fil", ""] {
            assert!(!tool_visible(name, false), "{name}");
            assert!(!tool_visible(name, true), "{name}");
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
}
