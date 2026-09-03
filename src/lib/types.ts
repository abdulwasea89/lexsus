// Shared types mirroring the Rust core's serde structs.

export interface GitFileStatus {
  path: string;
  status: string;
  additions: number;
  deletions: number;
}

export interface FsEvent {
  path: string;
  kind: string;
}

// --- activity trace ----------------------------------------------------------

export interface TraceStep {
  kind: string; // reading | editing | running | test | error | fs
  file: string | null;
  command: string | null;
  detail: string | null;
  confirmed: boolean;
  agent: string; // web | watcher
  ts: number;
}

// --- terminal ----------------------------------------------------------------

export type TerminalRunEvent =
  | { kind: "start"; command: string }
  | { kind: "output"; data: string }
  | { kind: "exit"; code: number | null; timed_out: boolean; truncated: boolean };

// --- git panel ---------------------------------------------------------------

export interface FileDiff {
  path: string;
  status: string;
  added: number;
  deleted: number;
  patch: string;
}

export interface BranchInfo {
  name: string;
  is_current: boolean;
}

export interface CommitInfo {
  oid: string;
  message: string;
  author: string;
  timestamp: number;
}

// --- bridge ------------------------------------------------------------------

export interface BridgeTool {
  /** `offset` is the 1-based first line; omit it for the first chunk. */
  ReadFile?: { path: string; offset?: number; limit?: number };
  WriteFile?: { path: string; content: string };
  EditFile?: {
    path: string;
    old_string: string;
    new_string: string;
    replace_all?: boolean;
  };
  MultiEdit?: {
    path: string;
    edits: { old_string: string; new_string: string; replace_all?: boolean }[];
  };
  ApplyPatch?: { path: string; patch: string };
  DeleteFile?: { path: string };
  MoveFile?: { from: string; to: string };
  CopyFile?: { from: string; to: string };
  CreateDirectory?: { path: string };
  ReadManyFiles?: { paths: string[] };
  RunCommand?: { command: string };
  ListDirectory?: { path: string };
  GitStatus?: null;
}

export interface ToolResult {
  ok: boolean;
  output: string | null;
  error: string | null;
  pending: string | null;
}

export interface ApprovalRequested {
  id: number;
  summary: string;
  source: string;
  /** Destructive calls may destroy work — the card renders them as danger. */
  destructive: boolean;
  /** When set, the card can offer a session grant covering this call. */
  grantable?: {
    scope: "editing" | "commands";
    suggested_prefix: string | null;
  };
}

export interface SessionGrant {
  id: number;
  scope: "editing" | "commands";
  path_prefix: string | null;
  source: string;
}

/** Grants + paused snapshot; also the `bridge://grants-changed` payload. */
export interface GrantState {
  grants: SessionGrant[];
  paused: boolean;
}

export interface AuditEntry {
  agent: string;
  tool: string;
  args: string;
  allowed: boolean;
  approved_by: string;
  ok: boolean;
  ts: string;
}

// --- handoff -----------------------------------------------------------------

export interface Handoff {
  objective: string;
  progress_percent: number;
  files_changed: number;
  errors_remaining: number;
  next_step: string | null;
  files: string[];
  context: string | null;
  end_reason: string | null;
  decisions?: string[];
  failed_attempts?: string[];
  constraints?: string[];
  generated_at: string;
}

// --- session archive + project memory (F2/F3) --------------------------------

export interface SessionSummary {
  id: number;
  agent: string;
  started_at: string;
  ended_at: string | null;
  objective: string | null;
  source: string | null;
  events: number;
}

export interface SessionEvent {
  kind: string; // user | assistant | tool | error | summary
  payload: string;
  ts_ms: number;
}

export interface ArchiveReport {
  archived: number;
  refreshed: number;
  skipped: number;
}

export interface ProjectFacts {
  objective: string | null;
  decisions: string[];
  failed_attempts: string[];
  constraints: string[];
  changed_files: string[];
  progress_percent: number;
}

export interface FactsSnapshot {
  session_id: number | null;
  report: ArchiveReport;
  facts: ProjectFacts;
}

// --- failover ----------------------------------------------------------------

/** Failover state machines for both directions (local → web, web AI). */
export interface FailoverStatus {
  local: string; // inactive | working | stalled | interrupted
  web: string;
  local_idle_ms: number;
  web_idle_ms: number;
}

export interface FailoverLogEntry {
  direction: string; // local_to_web | web_to_web | web_to_local
  trigger: string; // inactivity | ws_drop | manual
  idle_ms: number;
  payload: string | null;
  target: string | null;
  delivered: boolean;
  outcome: string | null;
  ts: string;
}

export interface FailoverLocalEvent {
  ok: boolean;
  delivered?: boolean;
  idle_ms?: number;
  handoff?: Handoff;
  error?: string;
}

export interface FailoverWebEvent {
  idle_ms: number;
  trigger: string;
  handoff: Handoff | null;
}
