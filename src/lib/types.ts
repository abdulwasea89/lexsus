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
  /** Stable code for a failure (`STRING_NOT_FOUND`, `AMBIGUOUS_MATCH`, …). */
  error_code: string | null;
  pending: string | null;
  /**
   * The machine-readable half of the result — the same facts as `output`.
   * Its shape is per-tool and mirrors the Rust `bridge::output_schema` row,
   * which is what the MCP connector advertises as `outputSchema`.
   */
  structured: unknown | null;
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

// --- MCP connector -----------------------------------------------------------

/** Live connector state, mirroring the Rust `McpStatus`. */
export interface McpStatus {
  /** True once the loopback endpoint actually bound. */
  listening: boolean;
  /** True from start until stop; may diverge from `listening` on a self-exit. */
  running: boolean;
  endpoint: string;
  /** The port actually bound — the OS's pick when 0 was configured. */
  port: number;
  /** Seconds since the connector was last started. */
  uptime_secs: number;
  /** Why the last start attempt failed, if it did — e.g. "port in use". */
  bind_error: string | null;
  /** Read-only-first: writes stay hidden until this is flipped. */
  allow_write: boolean;
  /** The bound workspace — the connector's whole blast radius. */
  workspace: string | null;
  /**
   * Host authorities the endpoint accepts. A tunnel whose host is missing
   * from this list gets a bare 403, which a remote connector misreads as a
   * sign-in problem.
   */
  allowed_hosts: string[];
  /**
   * Where the bearer token actually lives: `env`, `keyring` or `file`. The
   * keyring is unavailable on a headless Linux box, so the fallback is a
   * normal outcome rather than an error — shown so you know which store
   * holds your credential.
   */
  auth_backend: "env" | "keyring" | "file";
  /**
   * A short digest of the live token, so the UI can say *which* token is in
   * force without rendering it. Revealing the token is a separate action.
   */
  token_fingerprint: string;
  /** How far a signed request's timestamp may be from now, in seconds. */
  signature_ttl_secs: number;
  /**
   * Whether inbound requests must carry a signature. Always false: no MCP
   * client can compute one, so the bearer token carries authentication.
   * Every *response* is signed regardless.
   */
  signature_required: boolean;
  /**
   * The user's editable allowlist, without the loopback defaults the guard
   * always adds. `allowed_hosts` is what is enforced; this is what is edited.
   */
  configured_hosts: string[];
}

// --- connector tunnel + activity dashboard -----------------------------------

/** One tunnel provider the desktop can offer (mirrors Rust `Detection`). */
export interface TunnelDetection {
  provider: string;
  label: string;
  available: boolean;
  path: string | null;
  hint: string;
  preview: string;
}

/** A live tunnel's state (mirrors Rust `TunnelStatus`). */
export interface TunnelStatus {
  running: boolean;
  provider: string;
  url: string | null;
  host: string | null;
  port: number;
  log: string[];
}

/** A labelled count in the tool-surface breakdown. */
export interface CountEntry {
  name: string;
  count: number;
}

/** The tool catalogue derived from SPECS (mirrors Rust `ToolSurface`). */
export interface ToolSurface {
  total: number;
  read_only: number;
  write: number;
  groups: CountEntry[];
  approvals: CountEntry[];
  kinds: CountEntry[];
}

/** A `kind` bucket for the dashboard's by-kind breakdown. */
export interface KindCount {
  kind: string;
  count: number;
}

/** Dashboard headline numbers (mirrors Rust `ActivityStats`). */
export interface ActivityStats {
  total: number;
  tool_calls: number;
  files_read: number;
  files_written: number;
  commands_run: number;
  failures: number;
  denied: number;
  files: number;
  commands: number;
  span: [string, string] | null;
  by_kind: KindCount[];
}

/** One recent trace step, read back from the DB (mirrors Rust `TraceRow`). */
export interface TraceRow {
  ts: string;
  kind: string;
  tool: string | null;
  source: string | null;
  file: string | null;
  command: string | null;
  detail: string | null;
  ok: boolean;
}

/** Per-tool aggregate usage (mirrors Rust `ToolUsage`). */
export interface ToolUsage {
  tool: string;
  calls: number;
  failures: number;
  last_ts: string | null;
}

/** Per-file aggregate activity (mirrors Rust `FileTouch`). */
export interface FileTouch {
  file: string;
  reads: number;
  writes: number;
  last_ts: string | null;
}

/** Per-command aggregate history (mirrors Rust `CommandRun`). */
export interface CommandRun {
  command: string;
  runs: number;
  last_ok: boolean;
  last_ts: string | null;
}

/** One round-trip payload for the dashboard's whole activity half. */
export interface DashboardActivity {
  stats: ActivityStats;
  tools: ToolUsage[];
  files: FileTouch[];
  commands: CommandRun[];
  recent: TraceRow[];
  audit: AuditEntry[];
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
