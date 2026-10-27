use crate::{config::Source, parser::Parsed};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

pub struct Store {
    pub conn: Connection,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub generation: i64,
    pub offset: u64,
    pub identity: String,
    pub anchor: String,
}
pub struct Record {
    pub start: u64,
    pub end: u64,
    pub raw: String,
    pub parsed: Parsed,
}
pub struct Batch<'a> {
    pub host: &'a str,
    pub source: &'a Source,
    pub previous: &'a Checkpoint,
    pub next: &'a Checkpoint,
    pub records: Vec<Record>,
    pub observed_at: &'a str,
    pub health: &'a str,
}
impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            INSERT OR IGNORE INTO meta VALUES ('schema_version','1');
            CREATE TABLE IF NOT EXISTS sources (id TEXT PRIMARY KEY, config TEXT NOT NULL, generation INTEGER NOT NULL DEFAULT 0, offset INTEGER NOT NULL DEFAULT 0, identity TEXT NOT NULL DEFAULT '', anchor TEXT NOT NULL DEFAULT '', health TEXT NOT NULL DEFAULT 'unknown', last_read_at TEXT, diagnostics INTEGER NOT NULL DEFAULT 0);
            CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, source_id TEXT NOT NULL REFERENCES sources(id), generation INTEGER NOT NULL, byte_offset INTEGER NOT NULL, ordinal INTEGER NOT NULL DEFAULT 0, body TEXT NOT NULL, UNIQUE(source_id,generation,byte_offset,ordinal));
            CREATE TABLE IF NOT EXISTS workers (id TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS assignments (id TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS completions (dedup_key TEXT PRIMARY KEY, event_id INTEGER NOT NULL REFERENCES events(id), confidence TEXT NOT NULL);")?;
        let version: String = conn.query_row(
            "SELECT value FROM meta WHERE key='schema_version'",
            [],
            |r| r.get(0),
        )?;
        ensure!(version == "1", "unsupported database schema {version}");
        Ok(Self { conn })
    }
    pub fn read_only(path: &Path) -> Result<Self> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(Self { conn })
    }
    pub fn register(&self, host: &str, source: &Source) -> Result<Checkpoint> {
        self.conn
            .execute("INSERT OR IGNORE INTO meta VALUES ('host_id',?1)", [host])?;
        let existing: String =
            self.conn
                .query_row("SELECT value FROM meta WHERE key='host_id'", [], |r| {
                    r.get(0)
                })?;
        ensure!(
            existing == host,
            "database belongs to another host: {existing}"
        );
        let config = serde_json::to_string(source)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO sources(id,config) VALUES (?1,?2)",
            params![source.id, config],
        )?;
        let stored: String = self.conn.query_row(
            "SELECT config FROM sources WHERE id=?1",
            [&source.id],
            |r| r.get(0),
        )?;
        ensure!(
            stored == config,
            "source {} configuration changed; choose a new source ID or database",
            source.id
        );
        self.checkpoint(&source.id)
    }
    pub fn checkpoint(&self, id: &str) -> Result<Checkpoint> {
        Ok(self.conn.query_row(
            "SELECT generation,offset,identity,anchor FROM sources WHERE id=?1",
            [id],
            |r| {
                Ok(Checkpoint {
                    generation: r.get(0)?,
                    offset: r.get::<_, i64>(1)? as u64,
                    identity: r.get(2)?,
                    anchor: r.get(3)?,
                })
            },
        )?)
    }
    pub fn health(&self, id: &str, status: &str, at: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE sources SET health=?2,last_read_at=?3 WHERE id=?1",
            params![id, status, at],
        )?;
        Ok(())
    }
    pub fn commit(&mut self, batch: Batch<'_>, fail_before_commit: bool) -> Result<usize> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let (generation, offset): (i64, u64) = tx.query_row(
            "SELECT generation,offset FROM sources WHERE id=?1",
            [&batch.source.id],
            |r| Ok((r.get(0)?, r.get::<_, i64>(1)? as u64)),
        )?;
        ensure!(
            generation == batch.previous.generation && offset == batch.previous.offset,
            "checkpoint changed; another observer may be writing this source"
        );
        let count = batch.records.len();
        let mut diagnostics = 0;
        for record in batch.records {
            let mut p = record.parsed;
            let worker_key = p.worker_id.as_ref().map(|w| format!("{}:{w}", batch.host));
            let old: Option<Value> = if let Some(key) = &worker_key {
                tx.query_row("SELECT body FROM workers WHERE id=?1", [key], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?
                .map(|s| serde_json::from_str(&s))
                .transpose()?
            } else {
                None
            };
            if p.assignment_id.is_none()
                && matches!(p.kind.as_str(), "progress_observed" | "error_observed")
                && let Some(old) = &old
            {
                p.assignment_id = old["assignment_id"].as_str().map(str::to_owned);
                p.identity_confidence = old["identity_confidence"]
                    .as_str()
                    .unwrap_or("unknown")
                    .into();
                p.exponent = old["exponent"].as_u64();
            }
            let body = json!({"schema_version":1,"host_id":batch.host,"observed_at":batch.observed_at,"event_at":null,"event":p,"raw":record.raw,"provenance":{"source_id":batch.source.id,"generation":batch.next.generation,"byte_offset":record.start,"byte_end":record.end,"parser_version":1,"format":batch.source.format}});
            let inserted = tx.execute("INSERT OR IGNORE INTO events(source_id,generation,byte_offset,body) VALUES (?1,?2,?3,?4)", params![batch.source.id,batch.next.generation,i64::try_from(record.start)?,body.to_string()])?;
            if inserted == 0 {
                continue;
            }
            let id = tx.last_insert_rowid();
            if p.kind == "diagnostic" {
                diagnostics += 1;
                continue;
            }
            let projection = json!({"worker_id":p.worker_id,"assignment_id":p.assignment_id,"identity_confidence":p.identity_confidence,"exponent":p.exponent,"state":p.kind,"iteration":p.iteration,"total_iterations":p.total_iterations,"percent":p.percent,"last_observed_at":batch.observed_at,"last_event_id":id,"last_error":null});
            if let Some(key) = worker_key {
                let mut next = projection.clone();
                if p.kind == "error_observed" {
                    next = old.unwrap_or(next);
                    if next["state"] == "error_observed" {
                        next["state"] = json!("unknown");
                    }
                    next["last_error"] = json!({"message":p.diagnostic,"event_id":id,"observed_at":batch.observed_at});
                    next["last_observed_at"] = json!(batch.observed_at);
                } else if let Some(old) = old {
                    next["last_error"] = old["last_error"].clone();
                }
                tx.execute("INSERT INTO workers VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", params![key,next.to_string()])?;
            }
            if let Some(assignment) = &p.assignment_id {
                if p.kind != "error_observed" {
                    tx.execute("INSERT INTO assignments VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET body=excluded.body", params![assignment,projection.to_string()])?;
                }
                if p.kind == "completion_observed" {
                    let key = if p.identity_confidence == "explicit_assignment_id" {
                        format!("{assignment}:{}", p.result.as_deref().unwrap_or(""))
                    } else {
                        format!("source-event:{id}")
                    };
                    tx.execute(
                        "INSERT OR IGNORE INTO completions VALUES (?1,?2,?3)",
                        params![key, id, p.identity_confidence],
                    )?;
                }
            }
        }
        tx.execute("UPDATE sources SET generation=?2,offset=?3,identity=?4,anchor=?5,health=?6,last_read_at=?7,diagnostics=diagnostics+?8 WHERE id=?1", params![batch.source.id,batch.next.generation,i64::try_from(batch.next.offset)?,batch.next.identity,batch.next.anchor,batch.health,batch.observed_at,diagnostics])?;
        ensure!(!fail_before_commit, "injected failure before commit");
        tx.commit()?;
        Ok(count)
    }
    pub fn sources(&self) -> Result<Value> {
        let mut stmt = self.conn.prepare("SELECT id,config,generation,offset,health,last_read_at,diagnostics FROM sources ORDER BY id LIMIT 1000")?;
        let rows = stmt.query_map([], |r| Ok(json!({"id":r.get::<_,String>(0)?,"config":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(Value::Null),"generation":r.get::<_,i64>(2)?,"offset":r.get::<_,i64>(3)?,"health":r.get::<_,String>(4)?,"last_read_at":r.get::<_,Option<String>>(5)?,"diagnostics":r.get::<_,i64>(6)?})))?;
        Ok(Value::Array(rows.collect::<rusqlite::Result<Vec<_>>>()?))
    }
    pub fn projections(&self, table: &str) -> Result<Value> {
        ensure!(
            matches!(table, "workers" | "assignments"),
            "invalid projection"
        );
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT body FROM {table} ORDER BY id LIMIT 1000"))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Value::Array(
            rows.iter()
                .map(|s| serde_json::from_str(s))
                .collect::<serde_json::Result<Vec<_>>>()?,
        ))
    }
    pub fn events(&self, limit: usize, cursor: i64) -> Result<Value> {
        ensure!(
            (1..=500).contains(&limit) && cursor >= 0,
            "limit must be 1..500 and cursor must be nonnegative"
        );
        let mut stmt = self
            .conn
            .prepare("SELECT id,body FROM events WHERE id>?1 ORDER BY id LIMIT ?2")?;
        let rows = stmt
            .query_map(params![cursor, (limit + 1) as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let more = rows.len() > limit;
        let items: Vec<Value> = rows
            .iter()
            .take(limit)
            .map(|(id, s)| Ok(json!({"id":id,"record":serde_json::from_str::<Value>(s)?})))
            .collect::<serde_json::Result<_>>()?;
        let next = if more {
            items.last().map(|v| v["id"].clone())
        } else {
            None
        };
        Ok(json!({"items":items,"next_cursor":next}))
    }
    pub fn status(&self) -> Result<Value> {
        let (explicit,ambiguous): (i64,i64) = self.conn.query_row("SELECT COALESCE(SUM(confidence='explicit_assignment_id'),0),COALESCE(SUM(confidence!='explicit_assignment_id'),0) FROM completions", [], |r| Ok((r.get(0)?,r.get(1)?)))?;
        Ok(
            json!({"schema_version":1,"sources":self.sources()?,"workers":self.projections("workers")?,"completion_records":{"explicit_id_results":explicit,"ambiguous_records":ambiguous},"note":"Observed local output; quiet workers are not assumed stalled. Exponent-only identities may combine separate assignments. Counts are not account-wide totals."}),
        )
    }
}
