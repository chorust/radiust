use crate::errors::{CoreError, CoreResult};
use rusqlite::{Connection, params};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub key: String,
    pub path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub expires_at: Option<i64>,
    pub last_accessed: i64,
}

pub struct CacheIndex {
    connection: Connection,
}

impl CacheIndex {
    pub fn open(path: impl AsRef<Path>) -> CoreResult<Self> {
        let connection = Connection::open(path).map_err(|e| CoreError::Cache(e.to_string()))?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS objects (key TEXT PRIMARY KEY, path TEXT NOT NULL, size_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, expires_at INTEGER, last_accessed INTEGER NOT NULL DEFAULT 0)")
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        let _ = connection
            .execute("ALTER TABLE objects ADD COLUMN last_accessed INTEGER NOT NULL DEFAULT 0", []);
        Ok(Self { connection })
    }

    pub fn put(
        &self,
        key: &str,
        path: &str,
        size_bytes: u64,
        sha256: &str,
        expires_at: Option<i64>,
    ) -> CoreResult<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        self.connection.execute("INSERT OR REPLACE INTO objects(key,path,size_bytes,sha256,expires_at,last_accessed) VALUES (?1,?2,?3,?4,?5,?6)", params![key, path, size_bytes as i64, sha256, expires_at, now])
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> CoreResult<Option<(String, u64, String)>> {
        let mut statement = self
            .connection
            .prepare("SELECT path,size_bytes,sha256 FROM objects WHERE key=?1")
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        let mut rows =
            statement.query(params![key]).map_err(|e| CoreError::Cache(e.to_string()))?;
        if let Some(row) = rows.next().map_err(|e| CoreError::Cache(e.to_string()))? {
            Ok(Some((
                row.get(0).map_err(|e| CoreError::Cache(e.to_string()))?,
                row.get::<_, i64>(1).map_err(|e| CoreError::Cache(e.to_string()))? as u64,
                row.get(2).map_err(|e| CoreError::Cache(e.to_string()))?,
            )))
        } else {
            Ok(None)
        }
    }

    pub fn entries(&self) -> CoreResult<Vec<CacheEntry>> {
        let mut statement = self
            .connection
            .prepare("SELECT key,path,size_bytes,sha256,expires_at,last_accessed FROM objects")
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        let rows = statement
            .query_map([], |row| {
                Ok(CacheEntry {
                    key: row.get(0)?,
                    path: row.get(1)?,
                    size_bytes: row.get::<_, i64>(2)? as u64,
                    sha256: row.get(3)?,
                    expires_at: row.get(4)?,
                    last_accessed: row.get(5)?,
                })
            })
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        rows.map(|row| row.map_err(|e| CoreError::Cache(e.to_string()))).collect()
    }

    pub fn remove(&self, key: &str) -> CoreResult<()> {
        self.connection
            .execute("DELETE FROM objects WHERE key=?1", params![key])
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        Ok(())
    }

    pub fn touch(&self, key: &str) -> CoreResult<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        self.connection
            .execute("UPDATE objects SET last_accessed=?1 WHERE key=?2", params![now, key])
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        Ok(())
    }
}
