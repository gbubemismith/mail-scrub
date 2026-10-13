//! The machine-owned `SQLite` store. Schema lives in `migrations/`.

use std::path::Path;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

use crate::classifier::{Decision, Signal, Source, TypeDecision, sender_host};
use crate::client::MessageMeta;

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
    #[error("unreadable decision in database: {0}")]
    BadDecision(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    Full,
    Incremental,
}

impl RunKind {
    #[must_use]
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

/// A stored Decision and whether its labels were written to the mailbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRecord {
    pub decision: Decision,
    pub run_id: i64,
    /// `false` for a dry run, which records what it would have done.
    pub applied: bool,
}

#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Open (creating if needed) the database file and bring the schema up to date.
    ///
    /// # Errors
    ///
    /// Returns an error if the directory or file cannot be created, or the
    /// migrations fail.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the migrations fail.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the insert fails.
    pub async fn begin_run(
        &self,
        kind: RunKind,
        cursor_before: Option<&str>,
        dry_run: bool,
    ) -> Result<i64, DbError> {
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs().cast_signed());
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

    /// Record how a Run ended.
    ///
    /// # Errors
    ///
    /// Returns an error if the update fails.
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
        .bind(counts.labelled.cast_signed())
        .bind(counts.filters_changed.cast_signed())
        .bind(counts.unknown.cast_signed())
        .bind(run_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Store the message and its Decision, replacing any earlier Decision.
    ///
    /// # Errors
    ///
    /// Returns an error if the writes fail.
    pub async fn record_decision(
        &self,
        run_id: i64,
        message: &MessageMeta,
        decision: &Decision,
        applied: bool,
    ) -> Result<(), DbError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO message (id, sender_address, sender_domain) VALUES (?, ?, ?) \
             ON CONFLICT (id) DO UPDATE SET sender_address = excluded.sender_address, \
             sender_domain = excluded.sender_domain",
        )
        .bind(&message.id)
        .bind(&message.sender_address)
        .bind(sender_host(&message.sender_address))
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT OR REPLACE INTO decision \
             (message_id, sender_category, sender_source, email_type, type_signal, signal_key, run_id, applied) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&decision.message_id)
        .bind(decision.sender_category.as_ref().map(|(c, _)| c))
        .bind(decision.sender_category.as_ref().map(|(_, s)| s.as_str()))
        .bind(decision.email_type.as_ref().map(|t| &t.email_type))
        .bind(decision.email_type.as_ref().map(|t| t.signal.as_str()))
        .bind(decision.email_type.as_ref().map(|t| &t.signal_key))
        .bind(run_id)
        .bind(applied)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// All stored Decisions, ordered by message id.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a row holds an unknown Source or Signal.
    pub async fn decisions(&self) -> Result<Vec<DecisionRecord>, DbError> {
        let rows = sqlx::query("SELECT * FROM decision ORDER BY message_id")
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(decision_from_row).collect()
    }

    /// The cursor an Incremental Run should start from: where the last
    /// finished, non-dry Run got to. A dry run applied nothing, so the
    /// messages it saw still need visiting.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails.
    pub async fn last_cursor(&self) -> Result<Option<String>, DbError> {
        let row = sqlx::query(
            "SELECT cursor_after FROM run WHERE dry_run = 0 AND cursor_after IS NOT NULL ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| r.get("cursor_after")))
    }

    /// All Runs, oldest first.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails or a row holds an unknown run kind.
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
                        labelled: r.get::<i64, _>("labelled").cast_unsigned(),
                        filters_changed: r.get::<i64, _>("filters_changed").cast_unsigned(),
                        unknown: r.get::<i64, _>("unknown").cast_unsigned(),
                    },
                    dry_run: r.get("dry_run"),
                })
            })
            .collect()
    }
}

fn decision_from_row(r: &sqlx::sqlite::SqliteRow) -> Result<DecisionRecord, DbError> {
    let sender_category = match (
        r.get::<Option<String>, _>("sender_category"),
        r.get::<Option<String>, _>("sender_source"),
    ) {
        (Some(category), Some(source)) => Some((
            category,
            source.parse::<Source>().map_err(DbError::BadDecision)?,
        )),
        _ => None,
    };
    let email_type = match (
        r.get::<Option<String>, _>("email_type"),
        r.get::<Option<String>, _>("type_signal"),
        r.get::<Option<String>, _>("signal_key"),
    ) {
        (Some(email_type), Some(signal), Some(signal_key)) => Some(TypeDecision {
            email_type,
            signal: signal.parse::<Signal>().map_err(DbError::BadDecision)?,
            signal_key,
        }),
        _ => None,
    };
    Ok(DecisionRecord {
        decision: Decision {
            message_id: r.get("message_id"),
            sender_category,
            email_type,
        },
        run_id: r.get("run_id"),
        applied: r.get("applied"),
    })
}
