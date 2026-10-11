//! Local SQLite persistence: the request log and per-transaction decisions.

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::jsonrpc::Category;
use crate::record::{name_of, parse_name, Decision, RequestRecord, Status};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryQuery {
    pub chain_id: Option<u64>,
    pub category: Option<Category>,
    pub status: Option<Status>,
    /// Matches `from` or `to` (case-insensitive).
    pub address: Option<String>,
    /// Free text over method, summary and tx hash.
    pub text: Option<String>,
    pub since_ms: Option<u64>,
    pub until_ms: Option<u64>,
    pub limit: Option<u32>,
    /// Only records older than this id: stable paging while new requests arrive.
    pub before_id: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
#[error("storage error: {0}")]
pub struct StoreError(String);

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError(e.to_string())
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(e: serde_json::Error) -> Self {
        StoreError(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

pub struct Store {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS requests (
    id          INTEGER PRIMARY KEY,
    ts_ms       INTEGER NOT NULL,
    chain_id    INTEGER NOT NULL,
    method      TEXT NOT NULL,
    category    TEXT NOT NULL,
    client      TEXT NOT NULL,
    summary     TEXT NOT NULL,
    status      TEXT NOT NULL,
    duration_ms INTEGER,
    tx_hash     TEXT,
    from_addr   TEXT,
    to_addr     TEXT,
    request     TEXT NOT NULL,
    response    TEXT,
    detail      TEXT
);
CREATE INDEX IF NOT EXISTS requests_ts ON requests(ts_ms);
CREATE INDEX IF NOT EXISTS requests_hash ON requests(tx_hash);
CREATE TABLE IF NOT EXISTS decisions (
    dedupe_key TEXT PRIMARY KEY,
    decision   TEXT NOT NULL,
    ts_ms      INTEGER NOT NULL
);
";

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| StoreError(e.to_string()))?;
        }
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    /// Highest request id ever stored, so ids stay unique across restarts.
    pub fn max_id(&self) -> Result<u64> {
        let id: Option<i64> = self.conn.lock().query_row("SELECT MAX(id) FROM requests", [], |r| r.get(0))?;
        Ok(id.unwrap_or(0) as u64)
    }

    /// Inserts a record or replaces the one with the same id.
    pub fn upsert(&self, r: &RequestRecord) -> Result<()> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO requests
             (id, ts_ms, chain_id, method, category, client, summary, status, duration_ms,
              tx_hash, from_addr, to_addr, request, response, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                r.id as i64,
                r.ts_ms as i64,
                r.chain_id as i64,
                r.method,
                name_of(r.category),
                r.client,
                r.summary,
                name_of(r.status),
                r.duration_ms.map(|d| d as i64),
                r.tx_hash,
                r.from,
                r.to,
                serde_json::to_string(&r.request)?,
                r.response.as_ref().map(serde_json::to_string).transpose()?,
                r.detail.as_ref().map(serde_json::to_string).transpose()?,
            ],
        )?;
        Ok(())
    }

    pub fn get(&self, id: u64) -> Result<Option<RequestRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&format!("{SELECT_COLUMNS} WHERE id = ?1"))?;
        Ok(stmt.query_row([id as i64], row_to_record).optional()?)
    }

    /// Newest first.
    pub fn query(&self, q: &HistoryQuery) -> Result<Vec<RequestRecord>> {
        let mut sql = format!("{SELECT_COLUMNS} WHERE 1=1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        let mut push = |clause: &str, v: Box<dyn rusqlite::ToSql>, sql: &mut String| {
            args.push(v);
            sql.push_str(&clause.replace('?', &format!("?{}", args.len())));
        };
        if let Some(c) = q.chain_id {
            push(" AND chain_id = ?", Box::new(c as i64), &mut sql);
        }
        if let Some(c) = q.category {
            push(" AND category = ?", Box::new(name_of(c)), &mut sql);
        }
        if let Some(s) = q.status {
            push(" AND status = ?", Box::new(name_of(s)), &mut sql);
        }
        if let Some(a) = q.address.as_deref().filter(|a| !a.is_empty()) {
            let a = a.to_lowercase();
            push(" AND (lower(from_addr) = ? ", Box::new(a.clone()), &mut sql);
            push(" OR lower(to_addr) = ?)", Box::new(a), &mut sql);
        }
        if let Some(t) = q.text.as_deref().filter(|t| !t.is_empty()) {
            // Match the text literally: escape LIKE wildcards.
            let escaped = t.to_lowercase().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            let like = format!("%{escaped}%");
            push(" AND (lower(method) LIKE ? ESCAPE '\\'", Box::new(like.clone()), &mut sql);
            push(" OR lower(summary) LIKE ? ESCAPE '\\'", Box::new(like.clone()), &mut sql);
            push(" OR lower(coalesce(tx_hash, '')) LIKE ? ESCAPE '\\')", Box::new(like), &mut sql);
        }
        if let Some(s) = q.since_ms {
            push(" AND ts_ms >= ?", Box::new(s as i64), &mut sql);
        }
        if let Some(u) = q.until_ms {
            push(" AND ts_ms <= ?", Box::new(u as i64), &mut sql);
        }
        if let Some(b) = q.before_id {
            push(" AND id < ?", Box::new(b as i64), &mut sql);
        }
        sql.push_str(&format!(" ORDER BY id DESC LIMIT {}", q.limit.unwrap_or(200).min(1000)));
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), row_to_record)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// Marks requests that were still waiting for a decision as abandoned.
    /// Called on start-up: their HTTP connections died with the previous process.
    pub fn abandon_open(&self) -> Result<usize> {
        Ok(self
            .conn
            .lock()
            .execute("UPDATE requests SET status = 'abandoned' WHERE status IN ('pending', 'disconnected')", [])?)
    }

    pub fn record_decision(&self, dedupe_key: &str, decision: Decision, ts_ms: u64) -> Result<()> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO decisions (dedupe_key, decision, ts_ms) VALUES (?1, ?2, ?3)",
            params![dedupe_key, name_of(decision), ts_ms as i64],
        )?;
        Ok(())
    }

    pub fn decision(&self, dedupe_key: &str) -> Result<Option<Decision>> {
        let d: Option<String> = self
            .conn
            .lock()
            .query_row("SELECT decision FROM decisions WHERE dedupe_key = ?1", [dedupe_key], |r| r.get(0))
            .optional()?;
        Ok(d.and_then(|d| parse_name(&d)))
    }
}

const SELECT_COLUMNS: &str = "SELECT id, ts_ms, chain_id, method, category, client, summary, status, duration_ms,
    tx_hash, from_addr, to_addr, request, response, detail FROM requests";

fn row_to_record(r: &Row) -> rusqlite::Result<RequestRecord> {
    // Unreadable JSON (e.g. from an older version) is dropped rather than failing the row.
    let json = |s: Option<String>| s.and_then(|s| serde_json::from_str::<Value>(&s).ok());
    Ok(RequestRecord {
        id: r.get::<_, i64>(0)? as u64,
        ts_ms: r.get::<_, i64>(1)? as u64,
        chain_id: r.get::<_, i64>(2)? as u64,
        method: r.get(3)?,
        category: parse_name(&r.get::<_, String>(4)?).unwrap_or(Category::Read),
        client: r.get(5)?,
        summary: r.get(6)?,
        status: parse_name(&r.get::<_, String>(7)?).unwrap_or(Status::Invalid),
        duration_ms: r.get::<_, Option<i64>>(8)?.map(|d| d as u64),
        tx_hash: r.get(9)?,
        from: r.get(10)?,
        to: r.get(11)?,
        request: json(r.get(12)?).unwrap_or(Value::Null),
        response: json(r.get(13)?),
        detail: r.get::<_, Option<String>>(14)?.and_then(|s| serde_json::from_str(&s).ok()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn record(id: u64, method: &str, status: Status) -> RequestRecord {
        RequestRecord {
            id,
            ts_ms: id * 1000,
            chain_id: 1,
            method: method.into(),
            category: crate::jsonrpc::classify(method),
            client: "127.0.0.1".into(),
            summary: format!("summary {id}"),
            status,
            duration_ms: Some(5),
            tx_hash: None,
            from: Some("0xAbC".into()),
            to: None,
            request: json!({"method": method}),
            response: None,
            detail: None,
        }
    }

    #[test]
    fn stores_and_updates_records() {
        let s = Store::open_in_memory().unwrap();
        let mut r = record(1, "eth_sendRawTransaction", Status::Pending);
        s.upsert(&r).unwrap();
        r.status = Status::Approved;
        r.response = Some(json!({"result": "0x1"}));
        s.upsert(&r).unwrap();
        assert_eq!(s.get(1).unwrap().unwrap(), r);
        assert_eq!(s.max_id().unwrap(), 1);
    }

    #[test]
    fn queries_newest_first_with_filters() {
        let s = Store::open_in_memory().unwrap();
        s.upsert(&record(1, "eth_estimateGas", Status::Forwarded)).unwrap();
        s.upsert(&record(2, "eth_sendRawTransaction", Status::Rejected)).unwrap();
        s.upsert(&record(3, "eth_sendRawTransaction", Status::Approved)).unwrap();

        let all = s.query(&HistoryQuery::default()).unwrap();
        assert_eq!(all.iter().map(|r| r.id).collect::<Vec<_>>(), [3, 2, 1]);

        let sends = s.query(&HistoryQuery { category: Some(Category::Send), ..Default::default() }).unwrap();
        assert_eq!(sends.len(), 2);

        let rejected = s.query(&HistoryQuery { status: Some(Status::Rejected), ..Default::default() }).unwrap();
        assert_eq!(rejected[0].id, 2);

        let by_addr = s.query(&HistoryQuery { address: Some("0xabc".into()), ..Default::default() }).unwrap();
        assert_eq!(by_addr.len(), 3);

        let by_text = s.query(&HistoryQuery { text: Some("ESTIMATE".into()), ..Default::default() }).unwrap();
        assert_eq!(by_text[0].id, 1);

        let wildcard = s.query(&HistoryQuery { text: Some("%".into()), ..Default::default() }).unwrap();
        assert!(wildcard.is_empty());
    }

    #[test]
    fn pages_by_id_cursor_so_new_rows_do_not_shift_pages() {
        let s = Store::open_in_memory().unwrap();
        for id in 1..=5 {
            s.upsert(&record(id, "eth_sendRawTransaction", Status::Approved)).unwrap();
        }
        let first = s.query(&HistoryQuery { limit: Some(2), ..Default::default() }).unwrap();
        assert_eq!(first.iter().map(|r| r.id).collect::<Vec<_>>(), [5, 4]);
        // A new request arrives before the next page is loaded.
        s.upsert(&record(6, "eth_sendRawTransaction", Status::Approved)).unwrap();
        let next = s.query(&HistoryQuery { limit: Some(2), before_id: Some(4), ..Default::default() }).unwrap();
        assert_eq!(next.iter().map(|r| r.id).collect::<Vec<_>>(), [3, 2]);
    }

    #[test]
    fn open_requests_become_abandoned() {
        let s = Store::open_in_memory().unwrap();
        s.upsert(&record(1, "eth_sendRawTransaction", Status::Pending)).unwrap();
        s.upsert(&record(2, "eth_sendRawTransaction", Status::Disconnected)).unwrap();
        s.upsert(&record(3, "eth_sendRawTransaction", Status::Approved)).unwrap();
        assert_eq!(s.abandon_open().unwrap(), 2);
        assert_eq!(s.get(1).unwrap().unwrap().status, Status::Abandoned);
        assert_eq!(s.get(3).unwrap().unwrap().status, Status::Approved);
    }

    #[test]
    fn remembers_decisions() {
        let s = Store::open_in_memory().unwrap();
        assert_eq!(s.decision("0xhash").unwrap(), None);
        s.record_decision("0xhash", Decision::Rejected, 1).unwrap();
        s.record_decision("0xhash", Decision::Approved, 2).unwrap();
        assert_eq!(s.decision("0xhash").unwrap(), Some(Decision::Approved));
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        Store::open(&path).unwrap().upsert(&record(5, "eth_call", Status::Forwarded)).unwrap();
        assert_eq!(Store::open(&path).unwrap().max_id().unwrap(), 5);
    }
}
