import { invoke } from "@tauri-apps/api/core";
import type {
  ActivityStats,
  ArchiveReport,
  AuditEntry,
  BranchInfo,
  BridgeTool,
  CommandRun,
  CommitInfo,
  DashboardActivity,
  FailoverLogEntry,
  FailoverStatus,
  FactsSnapshot,
  FileDiff,
  FileTouch,
  GitFileStatus,
  GrantState,
  Handoff,
  McpStatus,
  SessionEvent,
  SessionSummary,
  ToolResult,
  ToolSurface,
  ToolUsage,
  TraceRow,
  TunnelDetection,
  TunnelStatus,
} from "./types";

/** Thin typed wrapper around the Rust core's Tauri commands. */

export function initDatabase(dbPath: string): Promise<string[]> {
  return invoke("init_database", { dbPath });
}

export function setProjectRoot(path: string): Promise<void> {
  return invoke("set_project_root", { path });
}

export function getProjectRoot(): Promise<string | null> {
  return invoke("get_project_root");
}

/** Pick a folder with the native dialog (driven from Rust, not the JS wrapper). */
export function pickProjectFolder(): Promise<string | null> {
  return invoke("pick_project_folder");
}

// --- git ---------------------------------------------------------------------

export function gitStatus(): Promise<GitFileStatus[]> {
  return invoke("git_status");
}

export function gitBranch(): Promise<string | null> {
  return invoke("git_branch");
}

export function gitCommit(message: string): Promise<string> {
  return invoke("git_commit", { message });
}

export function gitDiff(): Promise<FileDiff[]> {
  return invoke("git_diff");
}

export function gitStage(path: string): Promise<void> {
  return invoke("git_stage", { path });
}

export function gitUnstage(path: string): Promise<void> {
  return invoke("git_unstage", { path });
}

export function gitStageAll(): Promise<void> {
  return invoke("git_stage_all");
}

export function gitBranches(): Promise<BranchInfo[]> {
  return invoke("git_branches");
}

export function gitCheckout(name: string): Promise<void> {
  return invoke("git_checkout", { name });
}

export function gitLog(limit?: number): Promise<CommitInfo[]> {
  return invoke("git_log", { limit });
}

export function gitCommitDiff(oid: string): Promise<string> {
  return invoke("git_commit_diff", { oid });
}

// --- watcher -----------------------------------------------------------------

export function startWatch(): Promise<void> {
  return invoke("start_watch");
}

// --- M2 bridge ---------------------------------------------------------------

export function bridgeTool(tool: BridgeTool): Promise<ToolResult> {
  return invoke("bridge_tool", { tool });
}

export function bridgeApprove(
  id: number,
  allow: boolean,
  grant?: { scope: "editing" | "commands"; path_prefix: string | null },
): Promise<ToolResult> {
  return invoke("bridge_approve", { id, allow, grant: grant ?? null });
}

/**
 * Answer an `ask_user` / `propose_plan` card. The asking MCP call is blocked
 * on this; `option` is the chosen label (or null for free text).
 */
export function bridgeAnswerQuestion(
  id: number,
  option: string | null,
  answer: string,
): Promise<void> {
  return invoke("bridge_answer_question", { id, option, answer });
}

export function bridgeAudit(limit?: number): Promise<AuditEntry[]> {
  return invoke("bridge_audit", { limit });
}

// --- session grants (Phase 6 slice) ------------------------------------------

export function bridgeGrantState(): Promise<GrantState> {
  return invoke("bridge_grant_state");
}

export function bridgeGrantRevoke(id: number): Promise<boolean> {
  return invoke("bridge_grant_revoke", { id });
}

/** The kill switch: revoke every grant and pause the bridge (or unpause). */
export function bridgePause(paused: boolean): Promise<void> {
  return invoke("bridge_pause", { paused });
}

/** The connector's live state (endpoint, workspace, write surface). */
export function mcpStatus(): Promise<McpStatus> {
  return invoke("mcp_status");
}

/** Open or close the connector's write surface at runtime. */
export function mcpSetAllowWrite(enabled: boolean): Promise<McpStatus> {
  return invoke("mcp_set_allow_write", { enabled });
}

/** Start the MCP endpoint. Errors (e.g. port in use) surface to the caller. */
export function mcpStart(): Promise<McpStatus> {
  return invoke("mcp_start");
}

/** Stop the MCP endpoint and release its port. Abrupt, by design. */
export function mcpStop(): Promise<McpStatus> {
  return invoke("mcp_stop");
}

/** Restart the endpoint (applies a new port or allowlist). */
export function mcpRestart(): Promise<McpStatus> {
  return invoke("mcp_restart");
}

/** Change the connector's port and apply it, persisting for next launch. */
export function mcpSetPort(port: number): Promise<McpStatus> {
  return invoke("mcp_set_port", { port });
}

/** Replace the editable allowlist and restart the connector to apply it. */
export function mcpSetAllowedHosts(hosts: string[]): Promise<McpStatus> {
  return invoke("mcp_set_allowed_hosts", { hosts });
}

/**
 * The bearer token a connector must present. Only reachable over Tauri IPC
 * from this webview — the same process that already grants approvals — so
 * showing it costs nothing the desktop surface did not already hold.
 */
export function mcpRevealToken(): Promise<string> {
  return invoke("mcp_reveal_token");
}

/** Mint a new token. The old one stops working immediately, with no restart. */
export function mcpRotateToken(): Promise<McpStatus> {
  return invoke("mcp_rotate_token");
}

// --- public tunnel -----------------------------------------------------------

/** Which tunnel tools this machine has, so the UI offers what will work. */
export function tunnelDetect(): Promise<TunnelDetection[]> {
  return invoke("tunnel_detect");
}

/** The live tunnel state (running flag, URL, host, log tail). */
export function tunnelStatus(): Promise<TunnelStatus> {
  return invoke("tunnel_status");
}

/**
 * Start a tunnel to the connector. The public host is discovered
 * asynchronously and auto-allowlisted once the provider prints it.
 */
export function tunnelStart(
  provider: string,
  command?: string,
): Promise<TunnelStatus> {
  return invoke("tunnel_start", { provider, command: command ?? null });
}

/** Stop the tunnel and everything it spawned. */
export function tunnelStop(): Promise<TunnelStatus> {
  return invoke("tunnel_stop");
}

// --- activity dashboard ------------------------------------------------------

/** Newest-first trace steps with tool/source/outcome. */
export function activityTrace(limit?: number): Promise<TraceRow[]> {
  return invoke("activity_trace", { limit: limit ?? null });
}

export function activityStats(): Promise<ActivityStats> {
  return invoke("activity_stats");
}

export function activityToolUsage(limit?: number): Promise<ToolUsage[]> {
  return invoke("activity_tool_usage", { limit: limit ?? null });
}

export function activityFiles(limit?: number): Promise<FileTouch[]> {
  return invoke("activity_files", { limit: limit ?? null });
}

export function activityCommands(limit?: number): Promise<CommandRun[]> {
  return invoke("activity_commands", { limit: limit ?? null });
}

export function activityToolSurface(): Promise<ToolSurface> {
  return invoke("activity_tool_surface");
}

/** Stats + tools + files + commands + recent + audit in one IPC round-trip. */
export function dashboardActivity(): Promise<DashboardActivity> {
  return invoke("dashboard_activity");
}

export function setObjective(text: string): Promise<void> {
  return invoke("set_objective", { text });
}

export function buildHandoff(): Promise<Handoff> {
  return invoke("build_handoff");
}

// --- automatic failover ------------------------------------------------------

export function failoverStatus(): Promise<FailoverStatus> {
  return invoke("failover_status");
}

export function failoverReset(agent: "local" | "web"): Promise<void> {
  return invoke("failover_reset", { agent });
}

export function failoverLog(limit?: number): Promise<FailoverLogEntry[]> {
  return invoke("failover_log", { limit });
}

// --- session archive + project memory (F2/F3) --------------------------------

export function sessionsArchive(): Promise<ArchiveReport> {
  return invoke("sessions_archive");
}

export function sessionsList(limit?: number): Promise<SessionSummary[]> {
  return invoke("sessions_list", { limit });
}

export function sessionEventsGet(
  sessionId: number,
  limit?: number,
): Promise<SessionEvent[]> {
  return invoke("session_events_get", { sessionId, limit });
}

export function factsExtract(): Promise<FactsSnapshot> {
  return invoke("facts_extract");
}
