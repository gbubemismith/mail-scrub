//! The machine-owned SQLite store. Schema lives in `migrations/`.

use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("cannot create database directory {}: {source}", path.display())]
    CreateDir {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("unknown run kind in database: {0}")]
    UnknownRunKind(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Full,
    Incremental,
}

impl RunKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RunKind::Full => "full",
            RunKind::Incremental => "incremental",
        }
    }

    fn from_db(s: &str) -> Result<Self, DbError> {
        match s {
            "full" => Ok(RunKind::Full),
            "incremental" => Ok(RunKind::Incremental),
            other => Err(DbError::UnknownRunKind(other.to_owned())),
        }
    }
}

/// What a finished Run reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunCounts {
    pub labelled: u64,
    pub filters_changed: u64,
    pub unknown: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub id: i64,
    pub started_at: i64,
    pub kind: RunKind,
    pub cursor_before: Option<String>,
    /// `None` until the Run finishes, so an interrupted Run is visible.
    pub cursor_after: Option<String>,
    pub counts: RunCounts,
    pub dry_run: bool,
}

#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if needed) the database file and bring the schema up to date.
    pub async fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|source| DbError::CreateDir {
                path: dir.to_owned(),
                source,
            })?;
        }
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        Self::connect(SqlitePoolOptions::new(), options).await
    }

    /// A throwaway database. One connection, because every in-memory
    /// connection would otherwise be its own empty database.
    pub async fn open_in_memory() -> Result<Self, DbError> {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")?;
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1);
        Self::connect(pool, options).await
    }

    async fn connect(
        pool: SqlitePoolOptions,
        options: SqliteConnectOptions,
    ) -> Result<Self, DbError> {
        let pool = pool.connect_with(options).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Record the start of a Run and return its id.
    pub async fn begin_run(
        &self,
        kind: RunKind,
        cursor_before: Option<&str>,
        dry_run: bool,
    ) -> Result<i64, DbError> {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let result = sqlx::query(
            "INSERT INTO run (started_at, kind, cursor_before, dry_run) VALUES (?, ?, ?, ?)",
        )
        .bind(started_at)
        .bind(kind.as_str())
        .bind(cursor_before)
        .bind(dry_run)
        .execute(&self.pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn finish_run(
        &self,
        run_id: i64,
        cursor_after: &str,
        counts: RunCounts,
    ) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE run SET cursor_after = ?, labelled = ?, filters_changed = ?, unknown = ? WHERE id = ?",
        )
        .bind(cursor_after)
        .bind(counts.labelled as i64)
        .bind(counts.filters_changed as i64)
        .bind(counts.unknown as i64)
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The cursor an Incremental Run should start from: where the last
    /// finished, non-dry Run got to. A dry run applied nothing, so the
    /// messages it saw still need visiting.
    pub async fn last_cursor(&self) -> Result<Option<String>, DbError> {
        let row = sqlx::query(
            "SELECT cursor_after FROM run WHERE dry_run = 0 AND cursor_after IS NOT NULL ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.get("cursor_after")))
    }

    /// All Runs, oldest first.
    pub async fn runs(&self) -> Result<Vec<RunRecord>, DbError> {
        let rows = sqlx::query("SELECT * FROM run ORDER BY id")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|r| {
                Ok(RunRecord {
                    id: r.get("id"),
                    started_at: r.get("started_at"),
                    kind: RunKind::from_db(r.get("kind"))?,
                    cursor_before: r.get("cursor_before"),
                    cursor_after: r.get("cursor_after"),
                    counts: RunCounts {
                        labelled: r.get::<i64, _>("labelled") as u64,
                        filters_changed: r.get::<i64, _>("filters_changed") as u64,
                        unknown: r.get::<i64, _>("unknown") as u64,
                    },
                    dry_run: r.get("dry_run"),
                })
            })
            .collect()
    }
}
