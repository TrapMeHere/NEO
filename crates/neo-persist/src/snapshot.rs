//! SQLite snapshot store: metadata + content of journal snapshots (phase 4 autosave/recovery).
//!
//! Division of labor with `journal`: the journal is a per-command WAL; the snapshot store keeps
//! **periodic complete journal bytes** plus statistics, for recovery after accidental deletion/corruption
//! and for showing the "recent autosaves" list.

use rusqlite::{Connection, OptionalExtension, params};

/// Snapshot metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// Auto-increment id.
    pub id: i64,
    /// Unix seconds.
    pub created_at: i64,
    /// Label (e.g. frame count/version).
    pub label: String,
    /// Node count.
    pub nodes: usize,
    /// Way count.
    pub ways: usize,
    /// Relation count.
    pub relations: usize,
    /// Journal size in bytes.
    pub bytes: usize,
}

/// Snapshot store.
pub struct SnapshotStore {
    conn: Connection,
}

impl SnapshotStore {
    /// Open/create the database (creates tables automatically).
    pub fn open(path: impl AsRef<std::path::Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_conn(conn)
    }

    /// In-memory database (for tests).
    pub fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> rusqlite::Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS snapshots (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 created_at INTEGER NOT NULL,
                 label TEXT NOT NULL,
                 nodes INTEGER NOT NULL,
                 ways INTEGER NOT NULL,
                 relations INTEGER NOT NULL,
                 journal BLOB NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    /// Record a snapshot.
    pub fn record(
        &self,
        label: &str,
        journal: &[u8],
        nodes: usize,
        ways: usize,
        relations: usize,
    ) -> rusqlite::Result<i64> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.conn.execute(
            "INSERT INTO snapshots (created_at, label, nodes, ways, relations, journal)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![now, label, nodes as i64, ways as i64, relations as i64, journal],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The latest N snapshots (newest → oldest).
    pub fn list(&self, limit: usize) -> rusqlite::Result<Vec<SnapshotMeta>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, label, nodes, ways, relations, length(journal)
             FROM snapshots ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit as i64], |row| {
            Ok(SnapshotMeta {
                id: row.get(0)?,
                created_at: row.get(1)?,
                label: row.get(2)?,
                nodes: row.get::<_, i64>(3)? as usize,
                ways: row.get::<_, i64>(4)? as usize,
                relations: row.get::<_, i64>(5)? as usize,
                bytes: row.get::<_, i64>(6)? as usize,
            })
        })?;
        rows.collect()
    }

    /// The most recent snapshot and its journal bytes.
    pub fn latest(&self) -> rusqlite::Result<Option<(SnapshotMeta, Vec<u8>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at, label, nodes, ways, relations, length(journal)
             FROM snapshots ORDER BY id DESC LIMIT 1",
        )?;
        let meta = stmt
            .query_row([], |row| {
                Ok(SnapshotMeta {
                    id: row.get(0)?,
                    created_at: row.get(1)?,
                    label: row.get(2)?,
                    nodes: row.get::<_, i64>(3)? as usize,
                    ways: row.get::<_, i64>(4)? as usize,
                    relations: row.get::<_, i64>(5)? as usize,
                    bytes: row.get::<_, i64>(6)? as usize,
                })
            })
            .optional()?;
        let Some(meta) = meta else {
            return Ok(None);
        };
        let bytes: Vec<u8> =
            self.conn
                .query_row("SELECT journal FROM snapshots WHERE id = ?1", [meta.id], |row| {
                    row.get(0)
                })?;
        Ok(Some((meta, bytes)))
    }

    /// Keep only the latest `keep` snapshots; returns the number deleted.
    pub fn prune(&self, keep: usize) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM snapshots WHERE id NOT IN
             (SELECT id FROM snapshots ORDER BY id DESC LIMIT ?1)",
            [keep as i64],
        )
    }
}
