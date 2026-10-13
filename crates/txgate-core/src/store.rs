//! Local SQLite persistence: the request log and per-transaction decisions.

use std::path::Path;

use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use alloy_primitives::Address;

use crate::decode::Severity;
use crate::jsonrpc::Category;
use crate::record::{name_of, parse_name, Decision, RequestRecord, Status};
use crate::safe_state::SafeState;
use crate::safe_watch::{Finding, SafeAlert};

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

/// The user's own label for an address on one network.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AddressNote {
    pub chain_id: u64,
    pub address: alloy_primitives::Address,
    pub note: String,
}

/// Longest note kept, in characters.
pub const MAX_NOTE_CHARS: usize = 100;

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
CREATE TABLE IF NOT EXISTS safe_snapshots (
    chain_id INTEGER NOT NULL,
    safe     TEXT NOT NULL,
    state    TEXT NOT NULL,
    seen     TEXT NOT NULL,
    ts_ms    INTEGER NOT NULL,
    PRIMARY KEY (chain_id, safe)
);
CREATE TABLE IF NOT EXISTS safe_alerts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    ts_ms        INTEGER NOT NULL,
    chain_id     INTEGER NOT NULL,
    safe         TEXT NOT NULL,
    severity     TEXT NOT NULL,
    kind         TEXT NOT NULL,
    message      TEXT NOT NULL,
    safe_tx_hash TEXT,
    read         INTEGER NOT NULL DEFAULT 0,
    text         TEXT
);
CREATE TABLE IF NOT EXISTS address_notes (
    chain_id INTEGER NOT NULL,
    address  TEXT NOT NULL,
    note     TEXT NOT NULL,
    PRIMARY KEY (chain_id, address)
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
        // Logs from before alerts carried a translation.
        let has_text = conn.prepare("SELECT text FROM safe_alerts LIMIT 0").is_ok();
        if !has_text {
            conn.execute_batch("ALTER TABLE safe_alerts ADD COLUMN text TEXT")?;
        }
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

    /// Deletes every logged request except those in `keep`. Returns how many were removed.
    pub fn clear_requests(&self, keep: &[u64]) -> Result<usize> {
        let keep = keep.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        let conn = self.conn.lock();
        let removed = conn.execute(&format!("DELETE FROM requests WHERE id NOT IN ({keep})"), [])?;
        // A DELETE only marks pages free: rebuild the file and empty the WAL so
        // the cleared requests are really gone from disk.
        conn.execute_batch("VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(removed)
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

impl Store {
    /// The last reading of a watched Safe and the queued transaction hashes already alerted on.
    pub fn safe_snapshot(&self, chain_id: u64, safe: &Address) -> Result<Option<(SafeState, Vec<String>)>> {
        let row: Option<(String, String)> = self
            .conn
            .lock()
            .query_row(
                "SELECT state, seen FROM safe_snapshots WHERE chain_id = ?1 AND safe = ?2",
                params![chain_id as i64, safe.to_checksum(None)],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((state, seen)) => Ok(Some((serde_json::from_str(&state)?, serde_json::from_str(&seen)?))),
            None => Ok(None),
        }
    }

    pub fn save_safe_snapshot(&self, chain_id: u64, state: &SafeState, seen: &[String], ts_ms: u64) -> Result<()> {
        self.conn.lock().execute(
            "INSERT OR REPLACE INTO safe_snapshots (chain_id, safe, state, seen, ts_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![chain_id as i64, state.address.to_checksum(None), serde_json::to_string(state)?, serde_json::to_string(seen)?, ts_ms as i64],
        )?;
        Ok(())
    }

    /// Forgets the snapshots of Safes that are no longer watched, so watching
    /// one again starts from a fresh baseline.
    pub fn retain_safe_snapshots(&self, watched: &[(u64, Address)]) -> Result<()> {
        let conn = self.conn.lock();
        let rows: Vec<(i64, String)> = conn
            .prepare("SELECT chain_id, safe FROM safe_snapshots")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        for (chain_id, safe) in rows {
            let kept = watched.iter().any(|(c, a)| *c as i64 == chain_id && a.to_checksum(None) == safe);
            if !kept {
                conn.execute("DELETE FROM safe_snapshots WHERE chain_id = ?1 AND safe = ?2", params![chain_id, safe])?;
            }
        }
        Ok(())
    }

    /// Stores an alert and returns it with its id.
    pub fn add_safe_alert(&self, ts_ms: u64, chain_id: u64, safe: &Address, f: &Finding) -> Result<SafeAlert> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO safe_alerts (ts_ms, chain_id, safe, severity, kind, message, safe_tx_hash, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                ts_ms as i64,
                chain_id as i64,
                safe.to_checksum(None),
                name_of(f.severity),
                f.kind,
                f.message,
                f.safe_tx_hash,
                f.text.as_ref().map(serde_json::to_string).transpose()?
            ],
        )?;
        Ok(SafeAlert {
            id: conn.last_insert_rowid() as u64,
            ts_ms,
            chain_id,
            safe: *safe,
            severity: f.severity,
            kind: f.kind.to_string(),
            message: f.message.clone(),
            safe_tx_hash: f.safe_tx_hash.clone(),
            read: false,
            text: f.text.clone(),
        })
    }

    /// Newest first.
    pub fn safe_alerts(&self, limit: u32) -> Result<Vec<SafeAlert>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, ts_ms, chain_id, safe, severity, kind, message, safe_tx_hash, read, text FROM safe_alerts ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit.min(1000) as i64], |r| {
            Ok(SafeAlert {
                id: r.get::<_, i64>(0)? as u64,
                ts_ms: r.get::<_, i64>(1)? as u64,
                chain_id: r.get::<_, i64>(2)? as u64,
                safe: r.get::<_, String>(3)?.parse().unwrap_or_default(),
                severity: parse_name(&r.get::<_, String>(4)?).unwrap_or(Severity::Warning),
                kind: r.get(5)?,
                message: r.get(6)?,
                safe_tx_hash: r.get(7)?,
                read: r.get::<_, i64>(8)? != 0,
                text: r.get::<_, Option<String>>(9)?.and_then(|t| serde_json::from_str(&t).ok()),
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn mark_safe_alerts_read(&self) -> Result<()> {
        self.conn.lock().execute("UPDATE safe_alerts SET read = 1 WHERE read = 0", [])?;
        Ok(())
    }

    /// Every address note, on every network.
    pub fn address_notes(&self) -> Result<Vec<AddressNote>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare("SELECT chain_id, address, note FROM address_notes ORDER BY chain_id, address")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (chain_id, address, note) = row?;
            if let Ok(address) = address.parse() {
                out.push(AddressNote { chain_id: chain_id as u64, address, note });
            }
        }
        Ok(out)
    }

    /// Sets the note of `address` on `chain_id`; an empty note removes it.
    pub fn set_address_note(&self, chain_id: u64, address: &alloy_primitives::Address, note: &str) -> Result<()> {
        let note: String = note.trim().chars().take(MAX_NOTE_CHARS).collect();
        let conn = self.conn.lock();
        let key = format!("{address:#x}");
        if note.is_empty() {
            conn.execute("DELETE FROM address_notes WHERE chain_id = ?1 AND address = ?2", rusqlite::params![chain_id as i64, key])?;
        } else {
            conn.execute(
                "INSERT INTO address_notes (chain_id, address, note) VALUES (?1, ?2, ?3)
                 ON CONFLICT(chain_id, address) DO UPDATE SET note = excluded.note",
                rusqlite::params![chain_id as i64, key, note],
            )?;
        }
        Ok(())
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
    fn address_notes_are_kept_per_network() {
        use alloy_primitives::Address;
        let s = Store::open_in_memory().unwrap();
        let addr = Address::repeat_byte(0xab);
        s.set_address_note(1, &addr, "  Treasury  ").unwrap();
        s.set_address_note(8453, &addr, "Base hot wallet").unwrap();
        let notes = s.address_notes().unwrap();
        assert_eq!(notes.len(), 2);
        assert!(notes.contains(&AddressNote { chain_id: 1, address: addr, note: "Treasury".into() }), "trimmed: {notes:?}");

        s.set_address_note(1, &addr, "Cold storage").unwrap();
        assert!(s.address_notes().unwrap().contains(&AddressNote { chain_id: 1, address: addr, note: "Cold storage".into() }));

        // An empty note removes it, on that network only.
        s.set_address_note(1, &addr, " ").unwrap();
        assert_eq!(s.address_notes().unwrap(), vec![AddressNote { chain_id: 8453, address: addr, note: "Base hot wallet".into() }]);
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
    fn cleared_requests_do_not_linger_in_the_database_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.sqlite");
        let s = Store::open(&path).unwrap();
        for id in 1..=50 {
            let mut r = record(id, "eth_sendRawTransaction", Status::Approved);
            r.summary = format!("PRIVATE-MARKER-{id}");
            s.upsert(&r).unwrap();
        }
        s.clear_requests(&[]).unwrap();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let file = entry.unwrap().path();
            let bytes = std::fs::read(&file).unwrap();
            assert!(!bytes.windows(15).any(|w| w == b"PRIVATE-MARKER-"), "found in {}", file.display());
        }
    }

    #[test]
    fn alerts_keep_their_translation_also_in_an_older_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log.sqlite");
        // A log written before alerts carried a translation.
        Connection::open(&path)
            .unwrap()
            .execute_batch(
                "CREATE TABLE safe_alerts (id INTEGER PRIMARY KEY AUTOINCREMENT, ts_ms INTEGER NOT NULL, chain_id INTEGER NOT NULL, safe TEXT NOT NULL,
                 severity TEXT NOT NULL, kind TEXT NOT NULL, message TEXT NOT NULL, safe_tx_hash TEXT, read INTEGER NOT NULL DEFAULT 0);",
            )
            .unwrap();
        let s = Store::open(&path).unwrap();
        let text = crate::text::Text::new("alert.threshold_changed", [("old", "2"), ("new", "1")]);
        let f = Finding { severity: Severity::Danger, kind: "config_change", message: text.english(), safe_tx_hash: None, text: Some(text.clone()) };
        s.add_safe_alert(1, 1, &Address::repeat_byte(1), &f).unwrap();
        assert_eq!(s.safe_alerts(10).unwrap()[0].text, Some(text));
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
    fn keeps_safe_snapshots_and_alerts() {
        let s = Store::open_in_memory().unwrap();
        let safe = Address::repeat_byte(1);
        assert!(s.safe_snapshot(1, &safe).unwrap().is_none());
        let state = SafeState { address: safe, threshold: 2, ..Default::default() };
        s.save_safe_snapshot(1, &state, &["0xaa".into()], 5).unwrap();
        assert_eq!(s.safe_snapshot(1, &safe).unwrap(), Some((state, vec!["0xaa".to_string()])));
        assert!(s.safe_snapshot(10, &safe).unwrap().is_none(), "per chain");

        let f = Finding { severity: Severity::Danger, kind: "config_change", message: "m".into(), safe_tx_hash: None, text: None };
        let a = s.add_safe_alert(7, 1, &safe, &f).unwrap();
        s.add_safe_alert(8, 1, &safe, &f).unwrap();
        let list = s.safe_alerts(10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[1], a);
        s.mark_safe_alerts_read().unwrap();
        assert!(s.safe_alerts(10).unwrap().iter().all(|a| a.read));
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("db.sqlite");
        Store::open(&path).unwrap().upsert(&record(5, "eth_call", Status::Forwarded)).unwrap();
        assert_eq!(Store::open(&path).unwrap().max_id().unwrap(), 5);
    }
}
