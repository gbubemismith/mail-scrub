//! One Run: Lister → Fetcher → Classifier → Applier over bounded channels.
//!
//! Only the Lister, Fetcher and Applier touch the [`EmailClient`]. The stages
//! are futures joined on the caller's task, so a failure in any stage drops
//! the others and the Run ends with that error.

use std::fmt;

use tokio::sync::mpsc;

use crate::apply::{Applied, Applier, LabelChange};
use crate::classifier::{self, Decision};
use crate::client::{ClientError, EmailClient, MessageId, MessageMeta};
use crate::config::Config;
use crate::db::{Db, DbError, RunKind};
use crate::progress::{Progress, ProgressSender, report};

/// Ids per metadata request.
const BATCH_SIZE: usize = 50;
/// Bounded so a slow stage pushes back on the one before it.
const CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Default)]
pub struct RunOptions {
    pub dry_run: bool,
    /// Ignore the stored cursor and walk the whole mailbox.
    pub full: bool,
    /// Visit only this many of the newest messages: a sample, so it records
    /// no cursor and does not count as the first-Run preview.
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: i64,
    pub kind: RunKind,
    pub dry_run: bool,
    pub labelled: u64,
    pub filters_changed: u64,
    pub unknown: u64,
    /// The label changes a dry run would make; empty otherwise.
    pub planned: Vec<LabelChange>,
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

/// Whether a Run should be a dry run. An explicit `apply` or `dry_run` wins;
/// otherwise only the first Run, before any Run has finished, previews so the
/// user sees the planned changes before anything is written. A preview counts:
/// once seen, the next plain Run applies, which is what a scheduled job needs.
///
/// # Errors
///
/// Returns an error if the database cannot be read.
pub async fn resolve_dry_run(db: &Db, dry_run: bool, apply: bool) -> Result<bool, DbError> {
    if dry_run {
        return Ok(true);
    }
    if apply {
        return Ok(false);
    }
    Ok(db.runs().await?.iter().all(|r| r.cursor_after.is_none()))
}

/// Perform one Run against `client`, recording it in `db`. Progress goes to
/// `progress`, which is dropped when the Run ends, closing the channel.
///
/// # Errors
///
/// Returns an error if the client or database fails, or the stored cursor is
/// not valid for this provider. A Run that fails part way leaves its row
/// without a `cursor_after`.
pub async fn execute<C: EmailClient>(
    client: &C,
    db: &Db,
    config: &Config,
    options: RunOptions,
    progress: ProgressSender,
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

    let applier = Applier::new(client, db, run_id, options.dry_run).await?;
    let (cursor_after, (), (), Applied { counts, planned }) = tokio::try_join!(
        lister(
            client,
            cursor_before.as_ref(),
            options.limit,
            id_tx,
            progress.clone()
        ),
        fetcher(client, id_rx, meta_tx),
        classifier_stage(config, meta_rx, decision_tx),
        applier_stage(applier, decision_rx, progress),
    )?;

    // A sample skipped older mail, so its cursor is no place to resume from.
    let cursor_after = options.limit.is_none().then(|| cursor_after.to_string());
    db.finish_run(run_id, cursor_after.as_deref(), counts)
        .await?;

    Ok(RunSummary {
        run_id,
        kind,
        dry_run: options.dry_run,
        labelled: counts.labelled,
        filters_changed: counts.filters_changed,
        unknown: counts.unknown,
        planned,
    })
}

async fn lister<C: EmailClient>(
    client: &C,
    cursor: Option<&C::Cursor>,
    limit: Option<usize>,
    ids: mpsc::Sender<Vec<MessageId>>,
    progress: ProgressSender,
) -> Result<C::Cursor, RunError> {
    report(&progress, Progress::Listing);
    let changes = client.changes_since(cursor, limit).await?;
    report(&progress, Progress::Listed(changes.ids.len()));
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
    decisions: mpsc::Sender<(MessageMeta, Decision)>,
) -> Result<(), RunError> {
    while let Some(meta) = metas.recv().await {
        let decision = classifier::classify(config, &meta);
        decisions
            .send((meta, decision))
            .await
            .map_err(|_| RunError::StageStopped("applier"))?;
    }
    Ok(())
}

async fn applier_stage<C: EmailClient>(
    mut applier: Applier<'_, C>,
    mut decisions: mpsc::Receiver<(MessageMeta, Decision)>,
    progress: ProgressSender,
) -> Result<Applied, RunError> {
    let mut done = 0;
    while let Some((meta, decision)) = decisions.recv().await {
        applier.apply(&meta, &decision).await?;
        done += 1;
        report(&progress, Progress::Processed(done));
    }
    Ok(applier.finish())
}
