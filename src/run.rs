//! One Run: Lister → Fetcher → Classifier → Applier over bounded channels.
//!
//! Only the Lister, Fetcher and Applier touch the [`EmailClient`]. The stages
//! are futures joined on the caller's task, so a failure in any stage drops
//! the others and the Run ends with that error.

use std::fmt;

use tokio::sync::mpsc;

use crate::classifier::{self, Decision};
use crate::client::{ClientError, EmailClient, MessageId, MessageMeta};
use crate::config::Config;
use crate::db::{Db, DbError, RunCounts, RunKind};

/// Ids per metadata request.
const BATCH_SIZE: usize = 50;
/// Bounded so a slow stage pushes back on the one before it.
const CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    pub dry_run: bool,
    /// Ignore the stored cursor and walk the whole mailbox.
    pub full: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: i64,
    pub kind: RunKind,
    pub dry_run: bool,
    pub labelled: u64,
    pub filters_changed: u64,
    pub unknown: u64,
}

impl fmt::Display for RunSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let dry = if self.dry_run { " [dry run]" } else { "" };
        write!(
            f,
            "Run {} ({}){dry}: {} labelled, {} filters changed, {} unknown",
            self.run_id,
            self.kind.as_str(),
            self.labelled,
            self.filters_changed,
            self.unknown,
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("stored cursor {cursor:?} is not valid for this provider: {reason}")]
    BadCursor { cursor: String, reason: String },
    #[error("pipeline stage stopped early: {0}")]
    StageStopped(&'static str),
}

pub async fn execute<C: EmailClient>(
    client: &C,
    db: &Db,
    config: &Config,
    options: RunOptions,
) -> Result<RunSummary, RunError> {
    let stored = if options.full {
        None
    } else {
        db.last_cursor().await?
    };
    let cursor_before = stored
        .as_deref()
        .map(|text| {
            text.parse::<C::Cursor>().map_err(|e| RunError::BadCursor {
                cursor: text.to_owned(),
                reason: e.to_string(),
            })
        })
        .transpose()?;
    let kind = if cursor_before.is_some() {
        RunKind::Incremental
    } else {
        RunKind::Full
    };

    let run_id = db
        .begin_run(kind, stored.as_deref(), options.dry_run)
        .await?;

    let (id_tx, id_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (meta_tx, meta_rx) = mpsc::channel(CHANNEL_CAPACITY);
    let (decision_tx, decision_rx) = mpsc::channel(CHANNEL_CAPACITY);

    let (cursor_after, (), (), counts) = tokio::try_join!(
        lister(client, cursor_before.as_ref(), id_tx),
        fetcher(client, id_rx, meta_tx),
        classifier_stage(config, meta_rx, decision_tx),
        applier(decision_rx),
    )?;

    db.finish_run(run_id, &cursor_after.to_string(), counts)
        .await?;

    Ok(RunSummary {
        run_id,
        kind,
        dry_run: options.dry_run,
        labelled: counts.labelled,
        filters_changed: counts.filters_changed,
        unknown: counts.unknown,
    })
}

async fn lister<C: EmailClient>(
    client: &C,
    cursor: Option<&C::Cursor>,
    ids: mpsc::Sender<Vec<MessageId>>,
) -> Result<C::Cursor, RunError> {
    let changes = client.changes_since(cursor).await?;
    for batch in changes.ids.chunks(BATCH_SIZE) {
        ids.send(batch.to_vec())
            .await
            .map_err(|_| RunError::StageStopped("fetcher"))?;
    }
    Ok(changes.cursor)
}

async fn fetcher<C: EmailClient>(
    client: &C,
    mut ids: mpsc::Receiver<Vec<MessageId>>,
    metas: mpsc::Sender<MessageMeta>,
) -> Result<(), RunError> {
    while let Some(batch) = ids.recv().await {
        for meta in client.metadata(&batch).await? {
            metas
                .send(meta)
                .await
                .map_err(|_| RunError::StageStopped("classifier"))?;
        }
    }
    Ok(())
}

async fn classifier_stage(
    config: &Config,
    mut metas: mpsc::Receiver<MessageMeta>,
    decisions: mpsc::Sender<Decision>,
) -> Result<(), RunError> {
    while let Some(meta) = metas.recv().await {
        let decision = classifier::classify(config, &meta);
        decisions
            .send(decision)
            .await
            .map_err(|_| RunError::StageStopped("applier"))?;
    }
    Ok(())
}

/// Skeleton: nothing is ever decided, so there is nothing to apply; it only
/// counts Unknowns. Label changes arrive with the first real Decisions.
async fn applier(mut decisions: mpsc::Receiver<Decision>) -> Result<RunCounts, RunError> {
    let mut counts = RunCounts::default();
    while let Some(decision) = decisions.recv().await {
        if decision.is_unknown() {
            counts.unknown += 1;
        }
    }
    Ok(counts)
}
