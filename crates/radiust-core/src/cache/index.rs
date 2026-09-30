use crate::errors::{CoreError, CoreResult};
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexFormat {
    Native,
    PythonEntries,
}

pub struct CacheIndex {
    connection: Connection,
    format: IndexFormat,
}

impl CacheIndex {
    pub fn open(path: impl AsRef<Path>) -> CoreResult<Self> {
        let path = path.as_ref();
        let format = Self::detect_format(path)?;
        let connection = Connection::open(path).map_err(|e| CoreError::Cache(e.to_string()))?;
        match format {
            IndexFormat::PythonEntries => validate_columns(
                &connection,
                "entries",
                &[
                    "key",
                    "kind",
                    "path",
                    "size_bytes",
                    "sha256",
                    "validator",
                    "created_at",
                    "last_accessed_at",
                    "revalidated_at",
                    "expires_at",
                ],
            )?,
            IndexFormat::Native => {
                connection.execute_batch("CREATE TABLE IF NOT EXISTS objects (key TEXT PRIMARY KEY, path TEXT NOT NULL, size_bytes INTEGER NOT NULL, sha256 TEXT NOT NULL, expires_at INTEGER, last_accessed INTEGER NOT NULL DEFAULT 0)")
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                let columns = table_columns(&connection, "objects")?;
                if !columns.contains("last_accessed") {
                    connection.execute(
                        "ALTER TABLE objects ADD COLUMN last_accessed INTEGER NOT NULL DEFAULT 0",
                        [],
                    ).map_err(|e| CoreError::Cache(e.to_string()))?;
                }
                validate_columns(
                    &connection,
                    "objects",
                    &["key", "path", "size_bytes", "sha256", "expires_at", "last_accessed"],
                )?;
            }
        }
        Ok(Self { connection, format })
    }

    pub(super) fn open_readonly(path: impl AsRef<Path>) -> CoreResult<Self> {
        let path = path.as_ref();
        let format = Self::detect_format(path)?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        match format {
            IndexFormat::PythonEntries => validate_columns(
                &connection,
                "entries",
                &[
                    "key",
                    "kind",
                    "path",
                    "size_bytes",
                    "sha256",
                    "validator",
                    "created_at",
                    "last_accessed_at",
                    "revalidated_at",
                    "expires_at",
                ],
            )?,
            IndexFormat::Native => validate_columns(
                &connection,
                "objects",
                &["key", "path", "size_bytes", "sha256", "expires_at", "last_accessed"],
            )?,
        }
        Ok(Self { connection, format })
    }

    /// Detect the Python cache schema without creating tables or changing the
    /// database. This lets cache startup avoid treating Python-owned objects as
    /// unindexed Rust files.
    pub fn has_legacy_entries(path: &Path) -> CoreResult<bool> {
        Ok(Self::detect_format(path)? == IndexFormat::PythonEntries)
    }

    pub fn has_native_objects_table(path: &Path) -> CoreResult<bool> {
        Ok(Self::detect_format(path)? == IndexFormat::Native
            && path.exists()
            && table_exists(path, "objects")?)
    }

    pub(super) fn is_legacy(&self) -> bool {
        self.format == IndexFormat::PythonEntries
    }

    fn detect_format(path: &Path) -> CoreResult<IndexFormat> {
        if !path.exists() {
            return Ok(IndexFormat::Native);
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        let mut statement = connection
            .prepare(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
            )
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        let tables = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| CoreError::Cache(e.to_string()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        let has_entries = tables.iter().any(|name| name == "entries");
        let has_objects = tables.iter().any(|name| name == "objects");
        match (has_entries, has_objects) {
            (true, true) => Err(CoreError::Cache(
                "cache index contains both Python entries and Rust objects tables".into(),
            )),
            (true, false) => {
                validate_columns(
                    &connection,
                    "entries",
                    &[
                        "key",
                        "kind",
                        "path",
                        "size_bytes",
                        "sha256",
                        "validator",
                        "created_at",
                        "last_accessed_at",
                        "revalidated_at",
                        "expires_at",
                    ],
                )?;
                Ok(IndexFormat::PythonEntries)
            }
            (false, true) => {
                let columns = table_columns(&connection, "objects")?;
                if !["key", "path", "size_bytes", "sha256", "expires_at"]
                    .iter()
                    .all(|column| columns.contains(*column))
                {
                    return Err(CoreError::Cache("unsupported Rust cache objects schema".into()));
                }
                Ok(IndexFormat::Native)
            }
            (false, false) if tables.is_empty() => Ok(IndexFormat::Native),
            (false, false) => {
                Err(CoreError::Cache("cache index contains an unsupported table layout".into()))
            }
        }
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
        match self.format {
            IndexFormat::Native => {
                self.connection.execute("INSERT OR REPLACE INTO objects(key,path,size_bytes,sha256,expires_at,last_accessed) VALUES (?1,?2,?3,?4,?5,?6)", params![key, path, size_bytes as i64, sha256, expires_at, now])
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
            }
            IndexFormat::PythonEntries => {
                let now_text = timestamp_text(now)?;
                let expiry_text = expires_at.map(timestamp_text).transpose()?;
                let kind = if path_uses_mosaics(path) { "mosaic" } else { "object" };
                self.connection.execute(
                    "INSERT INTO entries(key,kind,path,size_bytes,sha256,validator,created_at,last_accessed_at,revalidated_at,expires_at) VALUES (?1,?2,?3,?4,?5,NULL,?6,?6,?6,?7) ON CONFLICT(key) DO UPDATE SET kind=excluded.kind,path=excluded.path,size_bytes=excluded.size_bytes,sha256=excluded.sha256,validator=NULL,created_at=excluded.created_at,last_accessed_at=excluded.last_accessed_at,revalidated_at=excluded.revalidated_at,expires_at=excluded.expires_at",
                    params![key, kind, path, size_bytes as i64, sha256, now_text, expiry_text],
                ).map_err(|e| CoreError::Cache(e.to_string()))?;
            }
        }
        Ok(())
    }

    pub fn get_entry(&self, key: &str) -> CoreResult<Option<CacheEntry>> {
        match self.format {
            IndexFormat::Native => {
                let raw = self
                    .connection
                    .query_row(
                        "SELECT key,path,size_bytes,sha256,expires_at,last_accessed FROM objects WHERE key=?1",
                        params![key],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, i64>(2)?,
                                row.get::<_, String>(3)?,
                                row.get::<_, Option<i64>>(4)?,
                                row.get::<_, i64>(5)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                raw.map(|(key, path, size, sha256, expires_at, last_accessed)| {
                    Ok(CacheEntry {
                        key,
                        path,
                        size_bytes: checked_size(size)?,
                        sha256,
                        expires_at,
                        last_accessed,
                    })
                })
                .transpose()
            }
            IndexFormat::PythonEntries => {
                let raw = self
                    .connection
                    .query_row(
                        "SELECT key,kind,path,size_bytes,sha256,expires_at,last_accessed_at FROM entries WHERE key=?1",
                        params![key],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, String>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, i64>(3)?,
                                row.get::<_, String>(4)?,
                                row.get::<_, Option<String>>(5)?,
                                row.get::<_, String>(6)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                raw.map(legacy_entry).transpose()
            }
        }
    }

    pub fn get(&self, key: &str) -> CoreResult<Option<(String, u64, String)>> {
        Ok(self.get_entry(key)?.map(|entry| (entry.path, entry.size_bytes, entry.sha256)))
    }

    pub fn entries(&self) -> CoreResult<Vec<CacheEntry>> {
        match self.format {
            IndexFormat::Native => {
                let mut statement = self
                    .connection
                    .prepare(
                        "SELECT key,path,size_bytes,sha256,expires_at,last_accessed FROM objects",
                    )
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, i64>(5)?,
                        ))
                    })
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                rows.map(|row| {
                    let (key, path, size, sha256, expires_at, last_accessed) =
                        row.map_err(|e| CoreError::Cache(e.to_string()))?;
                    Ok(CacheEntry {
                        key,
                        path,
                        size_bytes: checked_size(size)?,
                        sha256,
                        expires_at,
                        last_accessed,
                    })
                })
                .collect()
            }
            IndexFormat::PythonEntries => {
                let mut statement = self
                    .connection
                    .prepare("SELECT key,kind,path,size_bytes,sha256,expires_at,last_accessed_at FROM entries")
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                let rows = statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, String>(6)?,
                        ))
                    })
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
                rows.map(|row| legacy_entry(row.map_err(|e| CoreError::Cache(e.to_string()))?))
                    .collect()
            }
        }
    }

    pub fn remove(&self, key: &str) -> CoreResult<()> {
        let table = match self.format {
            IndexFormat::Native => "objects",
            IndexFormat::PythonEntries => "entries",
        };
        self.connection
            .execute(&format!("DELETE FROM {table} WHERE key=?1"), params![key])
            .map_err(|e| CoreError::Cache(e.to_string()))?;
        Ok(())
    }

    pub fn touch(&self, key: &str) -> CoreResult<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
        match self.format {
            IndexFormat::Native => {
                self.connection
                    .execute("UPDATE objects SET last_accessed=?1 WHERE key=?2", params![now, key])
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
            }
            IndexFormat::PythonEntries => {
                self.connection
                    .execute(
                        "UPDATE entries SET last_accessed_at=?1 WHERE key=?2",
                        params![timestamp_text(now)?, key],
                    )
                    .map_err(|e| CoreError::Cache(e.to_string()))?;
            }
        }
        Ok(())
    }
}

fn table_exists(path: &Path, table: &str) -> CoreResult<bool> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| CoreError::Cache(e.to_string()))?;
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
            params![table],
            |row| row.get(0),
        )
        .map_err(|e| CoreError::Cache(e.to_string()))
}

fn table_columns(
    connection: &Connection,
    table: &str,
) -> CoreResult<std::collections::HashSet<String>> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(|e| CoreError::Cache(e.to_string()))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| CoreError::Cache(e.to_string()))?
        .collect::<Result<std::collections::HashSet<_>, _>>()
        .map_err(|e| CoreError::Cache(e.to_string()))?;
    Ok(columns)
}

fn validate_columns(connection: &Connection, table: &str, required: &[&str]) -> CoreResult<()> {
    let columns = table_columns(connection, table)?;
    if required.iter().all(|column| columns.contains(*column)) {
        Ok(())
    } else {
        Err(CoreError::Cache(format!("unsupported cache {table} schema")))
    }
}

fn legacy_entry(
    (key, kind, path, size, sha256, expires_at, last_accessed_at): (
        String,
        String,
        String,
        i64,
        String,
        Option<String>,
        String,
    ),
) -> CoreResult<CacheEntry> {
    if !matches!(kind.as_str(), "object" | "mosaic") {
        return Err(CoreError::Cache("legacy cache entry has an unknown kind".into()));
    }
    Ok(CacheEntry {
        key,
        path,
        size_bytes: checked_size(size)?,
        sha256,
        expires_at: parse_timestamp(expires_at.as_deref())?,
        last_accessed: parse_timestamp(Some(&last_accessed_at))?
            .ok_or_else(|| CoreError::Cache("legacy cache last_accessed_at is null".into()))?,
    })
}

fn checked_size(size: i64) -> CoreResult<u64> {
    u64::try_from(size).map_err(|_| CoreError::Cache("cache entry has a negative size".into()))
}

fn parse_timestamp(value: Option<&str>) -> CoreResult<Option<i64>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value).map(|value| value.timestamp()).map_err(|error| {
                CoreError::Cache(format!("invalid legacy cache timestamp: {error}"))
            })
        })
        .transpose()
}

fn timestamp_text(timestamp: i64) -> CoreResult<String> {
    DateTime::<Utc>::from_timestamp(timestamp, 0)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Secs, false))
        .ok_or_else(|| CoreError::Cache("cache timestamp is out of range".into()))
}

fn path_uses_mosaics(path: &str) -> bool {
    Path::new(path).components().any(|component| component.as_os_str() == "mosaics")
}
