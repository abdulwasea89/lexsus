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

/// Build the rmcp [`Tool`] descriptor for one canonical tool name, from the
/// bridge's single source of truth: the SPECS summary + the Stage-1
/// [`bridge::tool_input_schema`]. Returns `None` for a name with no SPECS row
/// or schema (shouldn't happen — guarded by tests).
fn mcp_tool(name: &'static str) -> Option<Tool> {
    let summary = bridge::spec_by_name(name)?.summary;
    let schema = bridge::tool_input_schema(name)?;
    let schema = schema.as_object()?.clone();
    let read_only = READ_ONLY.contains(&name);
    Some(
        Tool::new_with_raw(name, Some(summary.into()), schema)
            .with_annotations(ToolAnnotations::default().read_only(read_only)),
    )
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

            if result.ok {
                let text = cap_text(result.output.as_deref().unwrap_or_default());
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]).into())
            } else {
                let message = result
                    .error
                    .as_deref()
                    .unwrap_or("the tool failed (no detail)");
                Ok(CallToolResult::error(vec![ContentBlock::text(message.to_string())]).into())
            }
        }
    }
}

/// Cap a tool-result string to [`RESULT_CHAR_CAP`] chars, keeping the existing
/// "cut and say so" convention so a model that hits the cap knows to page.
pub fn cap_text(text: &str) -> String {
    if text.chars().count() <= RESULT_CHAR_CAP {
        return text.to_string();
    }
    let cut: String = text.chars().take(RESULT_CHAR_CAP).collect();
    format!(
        "{cut}\n… [output truncated at {RESULT_CHAR_CAP} chars — call read_file with an offset to continue]"
    )
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
    let app = axum::Router::new().nest_service(MCP_PATH, service);
    axum::serve(listener, app)
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
}
