import { invoke } from "@tauri-apps/api/core";
import type {
  ArchiveReport,
  AuditEntry,
  BranchInfo,
  BridgeTool,
  CommitInfo,
  FailoverLogEntry,
  FailoverStatus,
  FactsSnapshot,
  FileDiff,
  GitFileStatus,
  GrantState,
  Handoff,
  SessionEvent,
  SessionSummary,
  ToolResult,
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

export function pairGetCode(): Promise<string> {
  return invoke("pair_get_code");
}

export function pairStatus(): Promise<boolean> {
  return invoke("pair_status");
}

export function setObjective(text: string): Promise<void> {
  return invoke("set_objective", { text });
}

export function buildHandoff(): Promise<Handoff> {
  return invoke("build_handoff");
}

export function handoffSend(): Promise<Handoff> {
  return invoke("handoff_send");
}

// --- automatic failover ------------------------------------------------------

export function failoverStatus(): Promise<FailoverStatus> {
  return invoke("failover_status");
}

export function failoverReset(agent: "local" | "web"): Promise<void> {
  return invoke("failover_reset", { agent });
}

export function failoverDeliver(
  target: "chatgpt" | "claudeai" | "gemini" | "grok" | "local",
): Promise<Handoff> {
  return invoke("failover_deliver", { target });
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
