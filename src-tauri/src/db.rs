use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

/// Schema migrations, versioned. Each entry applies in order and is
/// recorded in the `schema_migrations` table so migrations are idempotent.
pub const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_session_archive",
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            agent       TEXT NOT NULL,
            started_at  TEXT NOT NULL DEFAULT (datetime('now')),
            ended_at    TEXT,
            exit_code   INTEGER,
            objective   TEXT
        );

        CREATE TABLE IF NOT EXISTS session_events (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER NOT NULL REFERENCES sessions(id),
            kind        TEXT NOT NULL,      -- stdin | stdout | command | tool
            payload     TEXT NOT NULL,      -- the captured content
            ts          TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_session_events_session
            ON session_events(session_id);
        "#,
    ),
    (
        "0002_structured_project_memory",
        r#"
        CREATE TABLE IF NOT EXISTS objectives (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            text        TEXT NOT NULL,
            active      INTEGER NOT NULL DEFAULT 1,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS decisions (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            summary     TEXT NOT NULL,
            reason      TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS attempts (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            description TEXT NOT NULL,
            succeeded   INTEGER NOT NULL DEFAULT 0,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS constraints (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            text        TEXT NOT NULL,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS changed_files (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            path        TEXT NOT NULL,
            change_kind TEXT NOT NULL,      -- read | write | delete
            mtime       TEXT,
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_changed_files_path
            ON changed_files(path);

        CREATE TABLE IF NOT EXISTS progress (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            percent     INTEGER NOT NULL DEFAULT 0,
            note        TEXT,
            updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    ),
    (
        "0003_trace_and_audit",
        r#"
        CREATE TABLE IF NOT EXISTS settings (
            key         TEXT PRIMARY KEY,
            value       TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS trace_steps (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            kind        TEXT NOT NULL,      -- reading | editing | running | test | error
            file        TEXT,
            command     TEXT,
            detail      TEXT,
            confirmed   INTEGER NOT NULL DEFAULT 0,
            ts          TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_trace_steps_session
            ON trace_steps(session_id);

        CREATE TABLE IF NOT EXISTS audit_log (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            agent       TEXT NOT NULL,      -- claude | web | desktop
            tool        TEXT NOT NULL,
            args        TEXT,
            allowed     INTEGER NOT NULL,   -- 1 allowed / 0 denied
            approved_by TEXT NOT NULL DEFAULT 'auto',
            ok          INTEGER NOT NULL,
            ts          TEXT NOT NULL DEFAULT (datetime('now'))
        );
        "#,
    ),
    (
        "0004_failover",
        r#"
        CREATE TABLE IF NOT EXISTS failover_log (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            direction   TEXT NOT NULL,      -- local_to_web | web_to_web | web_to_local
            trigger     TEXT NOT NULL,      -- inactivity | ws_drop | manual
            idle_ms     INTEGER NOT NULL,
            payload     TEXT,
            target      TEXT,               -- chatgpt | claudeai | gemini | grok | local
            delivered   INTEGER NOT NULL DEFAULT 0,
            outcome     TEXT,
            ts          TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_failover_log_ts
            ON failover_log(ts);
        "#,
    ),
    (
        "0005_session_archive_v2",
        r#"
        ALTER TABLE sessions ADD COLUMN source TEXT;
        ALTER TABLE sessions ADD COLUMN cwd TEXT;
        ALTER TABLE sessions ADD COLUMN source_mtime INTEGER;

        CREATE UNIQUE INDEX IF NOT EXISTS idx_sessions_source
            ON sessions(source) WHERE source IS NOT NULL;

        ALTER TABLE session_events ADD COLUMN ts_ms INTEGER NOT NULL DEFAULT 0;

        CREATE INDEX IF NOT EXISTS idx_session_events_ts
            ON session_events(session_id, ts_ms);
        "#,
    ),
    (
        "0006_todos_and_handoff_requests",
        r#"
        CREATE TABLE IF NOT EXISTS todos (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id  INTEGER REFERENCES sessions(id),
            position    INTEGER NOT NULL,   -- the caller's ordering
            content     TEXT NOT NULL,
            status      TEXT NOT NULL,      -- pending | in_progress | completed
            active_form TEXT,               -- present-continuous form, for the UI
            created_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_todos_session
            ON todos(session_id, position);

        CREATE TABLE IF NOT EXISTS handoff_requests (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   INTEGER REFERENCES sessions(id),
            reason       TEXT NOT NULL,
            next_step    TEXT,
            acknowledged INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL DEFAULT (datetime('now'))
        );

        CREATE INDEX IF NOT EXISTS idx_handoff_requests_session
            ON handoff_requests(session_id, id);
        "#,
    ),
];

/// Open (or create) the database and apply any pending migrations.
pub fn open_and_migrate(path: &Path) -> rusqlite::Result<Connection> {
    let mut conn = Connection::open(path)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version     TEXT PRIMARY KEY,
            applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    for (version, sql) in MIGRATIONS {
        let applied: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
                [version],
                |row| row.get(0),
            )
            .unwrap_or(false);

        if !applied {
            let tx = conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )?;
            tx.commit()?;
        }
    }

    Ok(conn)
}

/// Report which migrations are currently applied (for diagnostics/CI).
pub fn applied_versions(conn: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare("SELECT version FROM schema_migrations ORDER BY version")?;
    let rows = stmt.query_map([], |row| row.get(0))?;
    rows.collect()
}

/// Persisted key/value settings (project root, ...).
pub fn set_setting(conn: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        (key, value),
    )?;
    Ok(())
}

pub fn get_setting(conn: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = ?1")?;
    let mut rows = stmt.query([key])?;
    match rows.next()? {
        Some(row) => Ok(Some(row.get(0)?)),
        None => Ok(None),
    }
}

/// Remove a setting entirely (used at startup to drop the old extension's
/// `pair_code` so no stale credential lingers in the DB).
pub fn delete_setting(conn: &Connection, key: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    Ok(())
}

/// Persist a parsed trace step.
pub fn record_trace_step(
    conn: &Connection,
    session_id: Option<i64>,
    kind: &str,
    file: Option<&str>,
    command: Option<&str>,
    detail: Option<&str>,
    confirmed: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO trace_steps (session_id, kind, file, command, detail, confirmed)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (session_id, kind, file, command, detail, confirmed as i64),
    )?;
    Ok(())
}

/// Mark recent editing steps for `path` as confirmed (watcher grounding).
pub fn confirm_trace_steps(conn: &Connection, path: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE trace_steps SET confirmed = 1
         WHERE kind = 'editing' AND file = ?1 AND confirmed = 0
           AND ts >= datetime('now', '-30 seconds')",
        [path],
    )
}

/// One audit-log entry (serde: mirrors frontend type).
#[derive(Debug, Clone, serde::Serialize)]
pub struct AuditEntry {
    pub agent: String,
    pub tool: String,
    pub args: String,
    pub allowed: bool,
    pub approved_by: String,
    pub ok: bool,
    pub ts: String,
}

pub fn record_audit(
    conn: &Connection,
    agent: &str,
    tool: &str,
    args: &str,
    allowed: bool,
    approved_by: &str,
    ok: bool,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO audit_log (agent, tool, args, allowed, approved_by, ok)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        (agent, tool, args, allowed as i64, approved_by, ok as i64),
    )?;
    Ok(())
}

pub fn last_audit(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<AuditEntry>> {
    let mut stmt = conn.prepare(
        "SELECT agent, tool, args, allowed, approved_by, ok, ts
         FROM audit_log ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(AuditEntry {
            agent: row.get(0)?,
            tool: row.get(1)?,
            args: row.get(2)?,
            allowed: row.get::<_, i64>(3)? != 0,
            approved_by: row.get(4)?,
            ok: row.get::<_, i64>(5)? != 0,
            ts: row.get(6)?,
        })
    })?;
    rows.collect()
}

/// Statistics for the handoff card (M2), derived from persisted trace.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TraceStats {
    pub files_changed: usize,
    pub errors: usize,
    pub steps: usize,
    pub last_step: Option<String>,
}

pub fn trace_stats(conn: &Connection) -> rusqlite::Result<TraceStats> {
    let files_changed: usize = conn.query_row(
        "SELECT COUNT(DISTINCT file) FROM trace_steps WHERE kind = 'editing' AND file IS NOT NULL",
        [],
        |r| r.get(0),
    )?;
    let errors: usize = conn.query_row(
        "SELECT COUNT(*) FROM trace_steps WHERE kind = 'error'",
        [],
        |r| r.get(0),
    )?;
    let steps: usize = conn.query_row("SELECT COUNT(*) FROM trace_steps", [], |r| r.get(0))?;
    let last_step: Option<String> = conn
        .query_row(
            "SELECT COALESCE(command, file, detail) FROM trace_steps ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();
    Ok(TraceStats {
        files_changed,
        errors,
        steps,
        last_step,
    })
}

/// One automatic-failover record (serde: mirrors frontend type).
#[derive(Debug, Clone, serde::Serialize)]
pub struct FailoverEntry {
    pub direction: String,
    pub trigger: String,
    pub idle_ms: i64,
    pub payload: Option<String>,
    pub target: Option<String>,
    pub delivered: bool,
    pub outcome: Option<String>,
    pub ts: String,
}

/// A failover record to insert (the DB fills in id/ts).
pub struct NewFailover<'a> {
    pub direction: &'a str,
    pub trigger: &'a str,
    pub idle_ms: i64,
    pub payload: Option<&'a str>,
    pub target: Option<&'a str>,
    pub delivered: bool,
    pub outcome: Option<&'a str>,
}

pub fn record_failover(conn: &Connection, entry: &NewFailover<'_>) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO failover_log (direction, trigger, idle_ms, payload, target, delivered, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        (
            entry.direction,
            entry.trigger,
            entry.idle_ms,
            entry.payload,
            entry.target,
            entry.delivered as i64,
            entry.outcome,
        ),
    )?;
    Ok(())
}

pub fn failover_log(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<FailoverEntry>> {
    let mut stmt = conn.prepare(
        "SELECT direction, trigger, idle_ms, payload, target, delivered, outcome, ts
         FROM failover_log ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(FailoverEntry {
            direction: row.get(0)?,
            trigger: row.get(1)?,
            idle_ms: row.get(2)?,
            payload: row.get(3)?,
            target: row.get(4)?,
            delivered: row.get::<_, i64>(5)? != 0,
            outcome: row.get(6)?,
            ts: row.get(7)?,
        })
    })?;
    rows.collect()
}

// --- session archive (Layer 1) ------------------------------------------------

/// One archived session (serde: mirrors frontend type).
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub id: i64,
    pub agent: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub objective: Option<String>,
    pub source: Option<String>,
    pub events: i64,
}

/// One archived timeline event.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionEventRow {
    pub kind: String,
    pub payload: String,
    pub ts_ms: i64,
}

/// A session to insert (the DB fills in id/started_at).
pub struct NewSession<'a> {
    pub agent: &'a str,
    pub source: &'a str,
    pub cwd: Option<&'a str>,
    pub objective: Option<&'a str>,
    /// File mtime (epoch ms) of the archived source — the dedupe key.
    pub source_mtime: i64,
}

pub fn find_session_by_source(conn: &Connection, source: &str) -> rusqlite::Result<Option<i64>> {
    let mut stmt = conn.prepare("SELECT id FROM sessions WHERE source = ?1")?;
    let mut rows = stmt.query([source])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

/// Insert a session row, or refresh the existing one for the same source
/// file. The precise timeline lives in `session_events.ts_ms`; the row's
/// own timestamps stay as archive-time defaults.
pub fn upsert_session(conn: &Connection, s: &NewSession<'_>) -> rusqlite::Result<i64> {
    if let Some(id) = find_session_by_source(conn, s.source)? {
        conn.execute(
            "UPDATE sessions
             SET agent = ?2, objective = ?3, cwd = ?4, source_mtime = ?5
             WHERE id = ?1",
            rusqlite::params![id, s.agent, s.objective, s.cwd, s.source_mtime],
        )?;
        Ok(id)
    } else {
        conn.execute(
            "INSERT INTO sessions (agent, objective, source, cwd, source_mtime)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![s.agent, s.objective, s.source, s.cwd, s.source_mtime],
        )?;
        Ok(conn.last_insert_rowid())
    }
}

/// Replace all events for a session with a fresh parse of its source.
pub fn replace_session_events(
    conn: &Connection,
    session_id: i64,
    events: &[SessionEventRow],
) -> rusqlite::Result<usize> {
    conn.execute(
        "DELETE FROM session_events WHERE session_id = ?1",
        [session_id],
    )?;
    for e in events {
        conn.execute(
            "INSERT INTO session_events (session_id, kind, payload, ts_ms)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![session_id, e.kind, e.payload, e.ts_ms],
        )?;
    }
    Ok(events.len())
}

pub fn list_sessions(conn: &Connection, limit: usize) -> rusqlite::Result<Vec<SessionSummary>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.agent, s.started_at, s.ended_at, s.objective, s.source,
                (SELECT COUNT(*) FROM session_events e WHERE e.session_id = s.id) AS events
         FROM sessions s
         WHERE s.source IS NOT NULL
         ORDER BY s.id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |row| {
        Ok(SessionSummary {
            id: row.get(0)?,
            agent: row.get(1)?,
            started_at: row.get(2)?,
            ended_at: row.get(3)?,
            objective: row.get(4)?,
            source: row.get(5)?,
            events: row.get(6)?,
        })
    })?;
    rows.collect()
}

pub fn newest_session_id(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "SELECT id FROM sessions WHERE source IS NOT NULL ORDER BY id DESC LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
}

pub fn session_events_for(
    conn: &Connection,
    session_id: i64,
    limit: usize,
) -> rusqlite::Result<Vec<SessionEventRow>> {
    let mut stmt = conn.prepare(
        "SELECT kind, payload, ts_ms FROM session_events
         WHERE session_id = ?1 ORDER BY ts_ms ASC, id ASC LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![session_id, limit as i64], |row| {
        Ok(SessionEventRow {
            kind: row.get(0)?,
            payload: row.get(1)?,
            ts_ms: row.get(2)?,
        })
    })?;
    rows.collect()
}

// --- structured project memory (Layer 2) --------------------------------------

/// Facts extracted from a session (serde: mirrors frontend type). This is
/// the persisted form; `facts::ExtractedFacts` converts into it.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ProjectFacts {
    pub objective: Option<String>,
    pub decisions: Vec<String>,
    pub failed_attempts: Vec<String>,
    pub constraints: Vec<String>,
    pub changed_files: Vec<String>,
    pub progress_percent: u8,
}

const FACT_TABLES: &[&str] = &[
    "objectives",
    "decisions",
    "attempts",
    "constraints",
    "changed_files",
    "progress",
];

/// Persist an extraction for a session, replacing any previous one so
/// re-extraction stays idempotent. The delete-then-insert runs in one
/// transaction: without it, a failure midway (constraint, disk, ...) would
/// strand the session half-wiped — old facts gone, new ones not yet in.
/// `unchecked_transaction` is safe here because every caller holds the app's
/// single connection behind its `Mutex` (or owns it outright), so nobody else
/// can be mid-statement on the same connection.
pub fn save_facts(conn: &Connection, session_id: i64, f: &ProjectFacts) -> rusqlite::Result<()> {
    let tx = conn.unchecked_transaction()?;
    for table in FACT_TABLES {
        tx.execute(
            &format!("DELETE FROM {table} WHERE session_id = ?1"),
            [session_id],
        )?;
    }
    if let Some(text) = f.objective.as_deref().filter(|t| !t.is_empty()) {
        tx.execute(
            "INSERT INTO objectives (session_id, text, active) VALUES (?1, ?2, 1)",
            (session_id, text),
        )?;
    }
    for d in &f.decisions {
        tx.execute(
            "INSERT INTO decisions (session_id, summary) VALUES (?1, ?2)",
            (session_id, d),
        )?;
    }
    for a in &f.failed_attempts {
        tx.execute(
            "INSERT INTO attempts (session_id, description, succeeded) VALUES (?1, ?2, 0)",
            (session_id, a),
        )?;
    }
    for c in &f.constraints {
        tx.execute(
            "INSERT INTO constraints (session_id, text) VALUES (?1, ?2)",
            (session_id, c),
        )?;
    }
    for p in &f.changed_files {
        tx.execute(
            "INSERT INTO changed_files (session_id, path, change_kind) VALUES (?1, ?2, 'write')",
            (session_id, p),
        )?;
    }
    tx.execute(
        "INSERT INTO progress (session_id, percent, note) VALUES (?1, ?2, 'heuristic')",
        rusqlite::params![session_id, f.progress_percent],
    )?;
    tx.commit()
}

/// Read back persisted facts for a session (defaults when none saved).
pub fn get_facts(conn: &Connection, session_id: i64) -> rusqlite::Result<ProjectFacts> {
    let objective = conn
        .query_row(
            "SELECT text FROM objectives WHERE session_id = ?1 AND active = 1
             ORDER BY id DESC LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .optional()?;
    let progress_percent = conn
        .query_row(
            "SELECT percent FROM progress WHERE session_id = ?1 ORDER BY id DESC LIMIT 1",
            [session_id],
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .map(|v| v.clamp(0, 100) as u8)
        .unwrap_or(0);
    Ok(ProjectFacts {
        objective,
        decisions: fact_column(conn, session_id, "decisions", "summary", 12)?,
        failed_attempts: fact_column_where(
            conn,
            session_id,
            "attempts",
            "description",
            "succeeded = 0",
            8,
        )?,
        constraints: fact_column(conn, session_id, "constraints", "text", 8)?,
        changed_files: fact_column_where(
            conn,
            session_id,
            "changed_files",
            "DISTINCT path",
            "change_kind = 'write'",
            30,
        )?,
        progress_percent,
    })
}

fn fact_column(
    conn: &Connection,
    session_id: i64,
    table: &str,
    column: &str,
    limit: usize,
) -> rusqlite::Result<Vec<String>> {
    fact_column_where(conn, session_id, table, column, "1 = 1", limit)
}

fn fact_column_where(
    conn: &Connection,
    session_id: i64,
    table: &str,
    select_expr: &str,
    cond: &str,
    limit: usize,
) -> rusqlite::Result<Vec<String>> {
    // Table/column names come from call sites above, never from user input.
    let sql = format!(
        "SELECT {select_expr} FROM {table} WHERE session_id = ?1 AND {cond} ORDER BY id LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([session_id], |row| row.get(0))?;
    rows.collect()
}

// --- Stage 3: the fact kinds a *connector* records --------------------------
//
// The tables below already hold facts the extractor derives from a Claude Code
// transcript. These are the other direction: a fact a caller states outright,
// through a tool. Same tables, same session key — the difference is who
// decided the fact was worth keeping.

/// A durable fact a session chose to record.
///
/// Three tables with the same shape — a session, a line of text, a timestamp —
/// and one operation over them. Naming the operation rather than the tables
/// means the three tools that expose it are three parse arms over one
/// implementation, and a fourth kind would be a variant, not a function.
pub enum Fact<'a> {
    Decision {
        summary: &'a str,
        reason: Option<&'a str>,
    },
    Constraint {
        text: &'a str,
    },
    Attempt {
        description: &'a str,
        succeeded: bool,
    },
}

impl Fact<'_> {
    /// The line of text this fact records.
    pub fn text(&self) -> &str {
        match self {
            Fact::Decision { summary, .. } => summary,
            Fact::Constraint { text } => text,
            Fact::Attempt { description, .. } => description,
        }
    }

    /// The wire name of this kind — the same word the tool and the answer use.
    pub fn kind(&self) -> &'static str {
        match self {
            Fact::Decision { .. } => "decision",
            Fact::Constraint { .. } => "constraint",
            Fact::Attempt { .. } => "attempt",
        }
    }
}

/// Insert one fact against a session, returning its row id.
pub fn record_fact(conn: &Connection, session_id: i64, fact: &Fact<'_>) -> rusqlite::Result<i64> {
    match fact {
        Fact::Decision { summary, reason } => conn.execute(
            "INSERT INTO decisions (session_id, summary, reason) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_id, summary, reason],
        )?,
        Fact::Constraint { text } => conn.execute(
            "INSERT INTO constraints (session_id, text) VALUES (?1, ?2)",
            rusqlite::params![session_id, text],
        )?,
        Fact::Attempt {
            description,
            succeeded,
        } => conn.execute(
            "INSERT INTO attempts (session_id, description, succeeded) VALUES (?1, ?2, ?3)",
            rusqlite::params![session_id, description, i64::from(*succeeded)],
        )?,
    };
    Ok(conn.last_insert_rowid())
}

/// Set the session's objective, retiring whatever was active.
///
/// Both statements run in one transaction because "exactly one active
/// objective" is the invariant `get_facts` relies on — it reads the newest
/// active row. A crash between the two would leave either none or two.
/// Returns the objective it replaced, so the caller can see what it displaced.
pub fn set_objective(
    conn: &Connection,
    session_id: i64,
    text: &str,
) -> rusqlite::Result<Option<String>> {
    let previous = conn
        .query_row(
            "SELECT text FROM objectives WHERE session_id = ?1 AND active = 1
             ORDER BY id DESC LIMIT 1",
            [session_id],
            |r| r.get(0),
        )
        .optional()?;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE objectives SET active = 0 WHERE session_id = ?1 AND active = 1",
        [session_id],
    )?;
    tx.execute(
        "INSERT INTO objectives (session_id, text, active) VALUES (?1, ?2, 1)",
        rusqlite::params![session_id, text],
    )?;
    tx.commit()?;
    Ok(previous)
}

/// The session row connector tool calls record into.
///
/// Fact tables are keyed by session, and the archive's sessions belong to
/// *Claude Code* transcripts. Filing a connector's decisions under one of
/// those would attribute them to the wrong agent, so the connector keeps a
/// session of its own — one row, found or created, never refreshed.
pub fn connector_session_id(conn: &Connection) -> rusqlite::Result<i64> {
    if let Some(id) = find_session_by_source(conn, CONNECTOR_SESSION_SOURCE)? {
        return Ok(id);
    }
    conn.execute(
        "INSERT INTO sessions (agent, source, cwd, source_mtime) VALUES (?1, ?2, NULL, 0)",
        rusqlite::params!["web-ai", CONNECTOR_SESSION_SOURCE],
    )?;
    Ok(conn.last_insert_rowid())
}

/// The `sessions.source` marker for the connector's own session. A `source`
/// that cannot collide with a transcript path: 0005 makes the column UNIQUE,
/// so this is also what keeps `connector_session_id` single-row.
pub const CONNECTOR_SESSION_SOURCE: &str = "mcp:connector";

/// One item on the session's task list.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Todo {
    pub content: String,
    pub status: String,
    pub active_form: Option<String>,
}

/// Replace the whole task list.
///
/// Replace, not merge: the caller sends the list it believes in, so a
/// completed item disappearing is a deliberate act rather than a lost update.
/// The delete and the inserts share a transaction for the same reason a
/// half-written list would be worse than either version.
pub fn replace_todos(
    conn: &Connection,
    session_id: i64,
    todos: &[Todo],
) -> rusqlite::Result<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute("DELETE FROM todos WHERE session_id = ?1", [session_id])?;
    for (i, t) in todos.iter().enumerate() {
        tx.execute(
            "INSERT INTO todos (session_id, position, content, status, active_form)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![session_id, i as i64, t.content, t.status, t.active_form],
        )?;
    }
    tx.commit()?;
    Ok(todos.len())
}

/// The session's task list, in the order it was written.
pub fn read_todos(conn: &Connection, session_id: i64) -> rusqlite::Result<Vec<Todo>> {
    let mut stmt = conn.prepare(
        "SELECT content, status, active_form FROM todos
         WHERE session_id = ?1 ORDER BY position",
    )?;
    let rows = stmt.query_map([session_id], |row| {
        Ok(Todo {
            content: row.get(0)?,
            status: row.get(1)?,
            active_form: row.get(2)?,
        })
    })?;
    rows.collect()
}

/// Note that the caller wants to hand back to the developer.
///
/// Recorded rather than acted on: the engine has no channel to interrupt the
/// desktop with, and inventing one here would put a policy decision in a tool.
/// The row is what the desktop reads to show the request.
pub fn record_handoff_request(
    conn: &Connection,
    session_id: i64,
    reason: &str,
    next_step: Option<&str>,
) -> rusqlite::Result<i64> {
    conn.execute(
        "INSERT INTO handoff_requests (session_id, reason, next_step) VALUES (?1, ?2, ?3)",
        rusqlite::params![session_id, reason, next_step],
    )?;
    Ok(conn.last_insert_rowid())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::ExtractedFacts;

    fn mem() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        for (_, sql) in MIGRATIONS {
            conn.execute_batch(sql).unwrap();
        }
        conn
    }

    /// A newly added migration upgrades a database that already has the older
    /// ones — the path a real installation takes.
    ///
    /// Every test above builds its database from an empty file, so all of them
    /// apply the migrations in one clean sweep. That is not what happens on a
    /// developer's machine: their database already exists, already has rows,
    /// and has already recorded 0001–0005 as applied. This is the only test
    /// that runs a migration against that, which is why it exists — the
    /// failure it guards (a migration that assumes an empty table, or one that
    /// re-runs because the version row was not written) is invisible to every
    /// other test in this file.
    #[test]
    fn the_newest_migration_upgrades_a_populated_database() {
        let dir = std::env::temp_dir().join(format!("lexsus-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("app.sqlite3");

        // An installed database: every migration but the newest, applied the
        // way the runner applies them.
        let (newest, _) = MIGRATIONS.last().expect("at least one migration");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version     TEXT PRIMARY KEY,
                applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )
        .unwrap();
        for (version, sql) in MIGRATIONS {
            if version == newest {
                break;
            }
            conn.execute_batch(sql).unwrap();
            conn.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )
            .unwrap();
        }
        let session = upsert_session(
            &conn,
            &NewSession {
                agent: "claude",
                source: "/t/old.jsonl",
                cwd: None,
                source_mtime: 0,
                objective: Some("an objective from before the upgrade"),
            },
        )
        .unwrap();
        // And facts in the tables the new migration does not touch, since a
        // migration that happened to re-run one of the older steps would show
        // up here first.
        set_objective(&conn, session, "a stated objective from before the upgrade").unwrap();
        record_fact(
            &conn,
            session,
            &Fact::Decision {
                summary: "decided before the upgrade",
                reason: None,
            },
        )
        .unwrap();
        drop(conn);

        // The upgrade.
        let conn = open_and_migrate(&path).unwrap();

        let survived: (String, Option<String>) = conn
            .query_row(
                "SELECT agent, objective FROM sessions WHERE id = ?1",
                [session],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("the upgrade lost the developer's session");
        assert_eq!(survived.0, "claude");
        assert_eq!(
            survived.1.as_deref(),
            Some("an objective from before the upgrade")
        );
        let facts = get_facts(&conn, session).unwrap();
        assert_eq!(
            facts.objective.as_deref(),
            Some("a stated objective from before the upgrade"),
            "the upgrade dropped a live objective"
        );
        assert_eq!(
            facts.decisions,
            vec!["decided before the upgrade".to_string()]
        );

        // The new tables are there and empty, and every version is recorded
        // exactly once.
        for table in ["todos", "handoff_requests"] {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap_or_else(|e| panic!("{table} does not exist after the upgrade: {e}"));
            assert_eq!(n, 0, "{table} came up non-empty");
        }
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied as usize, MIGRATIONS.len());
        drop(conn);

        // And running it again is a no-op — the runner is idempotent, so a
        // second launch must not re-run the ALTERs in 0005.
        let conn = open_and_migrate(&path).unwrap();
        let applied: i64 = conn
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0))
            .unwrap();
        assert_eq!(applied as usize, MIGRATIONS.len());
        drop(conn);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_session_dedupes_by_source_and_replaces_events() {
        let conn = mem();
        let id = upsert_session(
            &conn,
            &NewSession {
                agent: "claude",
                source: "/t/s1.jsonl",
                cwd: Some("/work/app"),
                objective: Some("ship auth"),
                source_mtime: 100,
            },
        )
        .unwrap();
        let again = upsert_session(
            &conn,
            &NewSession {
                agent: "claude",
                source: "/t/s1.jsonl",
                cwd: Some("/work/app"),
                objective: None,
                source_mtime: 200,
            },
        )
        .unwrap();
        assert_eq!(id, again);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);

        replace_session_events(
            &conn,
            id,
            &[
                SessionEventRow {
                    kind: "user".into(),
                    payload: "hello".into(),
                    ts_ms: 1,
                },
                SessionEventRow {
                    kind: "tool".into(),
                    payload: "Read src/a.rs".into(),
                    ts_ms: 2,
                },
            ],
        )
        .unwrap();
        // Re-replace shrinks, never duplicates.
        replace_session_events(
            &conn,
            id,
            &[SessionEventRow {
                kind: "error".into(),
                payload: "boom".into(),
                ts_ms: 3,
            }],
        )
        .unwrap();
        let events = session_events_for(&conn, id, 50).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].kind, "error");

        let sessions = list_sessions(&conn, 10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].events, 1);
        assert!(newest_session_id(&conn).unwrap().is_some());
    }

    #[test]
    fn facts_roundtrip_is_idempotent() {
        let conn = mem();
        let id = upsert_session(
            &conn,
            &NewSession {
                agent: "claude",
                source: "/t/s2.jsonl",
                cwd: None,
                objective: None,
                source_mtime: 1,
            },
        )
        .unwrap();
        let f = ExtractedFacts {
            objective: Some("implement auth".into()),
            decisions: vec!["use argon2 for hashing".into()],
            failed_attempts: vec!["bcrypt build failed on musl".into()],
            constraints: vec!["must not touch the payments module".into()],
            changed_files: vec!["src/auth.rs".into()],
            progress_percent: 55,
        };
        save_facts(&conn, id, &f.clone().into()).unwrap();
        save_facts(&conn, id, &f.into()).unwrap(); // re-extract replaces

        let got = get_facts(&conn, id).unwrap();
        assert_eq!(got.objective.as_deref(), Some("implement auth"));
        assert_eq!(got.decisions, vec!["use argon2 for hashing".to_string()]);
        assert_eq!(
            got.failed_attempts,
            vec!["bcrypt build failed on musl".to_string()]
        );
        assert_eq!(
            got.constraints,
            vec!["must not touch the payments module".to_string()]
        );
        assert_eq!(got.changed_files, vec!["src/auth.rs".to_string()]);
        assert_eq!(got.progress_percent, 55);

        let empty = get_facts(&conn, 999).unwrap();
        assert_eq!(empty, ProjectFacts::default());
    }

    #[test]
    fn save_facts_is_atomic_across_delete_then_insert() {
        let conn = mem();
        let id = upsert_session(
            &conn,
            &NewSession {
                agent: "claude",
                source: "/t/s3.jsonl",
                cwd: None,
                objective: None,
                source_mtime: 1,
            },
        )
        .unwrap();
        let original = ExtractedFacts {
            objective: Some("original goal".into()),
            decisions: vec!["original decision".into()],
            failed_attempts: vec!["original attempt".into()],
            constraints: vec!["original constraint".into()],
            changed_files: vec!["src/orig.rs".into()],
            progress_percent: 10,
        };
        save_facts(&conn, id, &original.clone().into()).unwrap();

        // Sabotage the *later* inserts: any INSERT into `constraints` (after
        // objectives, decisions and attempts have already been written) now
        // aborts. A non-transactional delete-then-insert would leave the
        // session half-wiped here; the transaction must roll back everything.
        conn.execute_batch(
            "CREATE TRIGGER sabotage_constraints
             BEFORE INSERT ON constraints
             BEGIN SELECT RAISE(ABORT, 'sabotage'); END;",
        )
        .unwrap();

        let replacement = ExtractedFacts {
            objective: Some("new goal".into()),
            decisions: vec!["new decision".into()],
            failed_attempts: vec!["new attempt".into()],
            constraints: vec!["new constraint".into()],
            changed_files: vec!["src/new.rs".into()],
            progress_percent: 90,
        };
        let res = save_facts(&conn, id, &replacement.into());
        assert!(res.is_err(), "sabotaged save must fail");

        // Rolled back: the original facts are fully intact, not half-deleted.
        let got = get_facts(&conn, id).unwrap();
        assert_eq!(got.objective.as_deref(), Some("original goal"));
        assert_eq!(got.decisions, vec!["original decision".to_string()]);
        assert_eq!(got.failed_attempts, vec!["original attempt".to_string()]);
        assert_eq!(got.constraints, vec!["original constraint".to_string()]);
        assert_eq!(got.changed_files, vec!["src/orig.rs".to_string()]);
        assert_eq!(got.progress_percent, 10);
    }
}
