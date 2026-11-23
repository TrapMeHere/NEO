//! SQLite 快照库：journal 快照的元数据 + 内容（阶段 4 自动保存/恢复）。
//!
//! 与 `journal` 的分工：journal 是逐命令 WAL；快照库保存**周期性完整 journal 字节**
//! 与统计信息，便于误删/损坏后恢复，也便于展示“最近自动保存”列表。

use rusqlite::{Connection, OptionalExtension, params};

/// 快照元数据。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotMeta {
    /// 自增 id。
    pub id: i64,
    /// unix 秒。
    pub created_at: i64,
    /// 标签（例如帧数/版本）。
    pub label: String,
    /// 节点数。
    pub nodes: usize,
    /// way 数。
    pub ways: usize,
    /// relation 数。
    pub relations: usize,
    /// journal 字节数。
    pub bytes: usize,
}

/// 快照库。
pub struct SnapshotStore {
    conn: Connection,
}

impl SnapshotStore {
    /// 打开/创建数据库（自动建表）。
    pub fn open(path: impl AsRef<std::path::Path>) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        Self::from_conn(conn)
    }

    /// 内存库（测试）。
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

    /// 记录一个快照。
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

    /// 最近 N 个快照（新→旧）。
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

    /// 最近一个快照及其 journal 字节。
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

    /// 只保留最近 `keep` 个快照，返回删除数。
    pub fn prune(&self, keep: usize) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM snapshots WHERE id NOT IN
             (SELECT id FROM snapshots ORDER BY id DESC LIMIT ?1)",
            [keep as i64],
        )
    }
}
