//! The Applier: turns a [`Decision`] into label changes, only ever under the
//! Owned prefixes, and records the Decision.

use std::fmt;

use crate::classifier::Decision;
use crate::client::{EmailClient, Label, LabelId, MessageId, MessageMeta};
use crate::db::{Db, RunCounts};
use crate::run::RunError;

/// Owned prefixes: the tool may create, add and remove labels under these and
/// nothing else.
pub const SENDER_PREFIX: &str = "From/";
pub const TYPE_PREFIX: &str = "Type/";

fn is_owned(name: &str) -> bool {
    name.starts_with(SENDER_PREFIX) || name.starts_with(TYPE_PREFIX)
}

/// A label to add and the Source or Signal that asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedLabel {
    pub name: String,
    pub because: String,
}

/// The label changes one message needs, by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelChange {
    pub message_id: MessageId,
    pub add: Vec<PlannedLabel>,
    pub remove: Vec<String>,
}

impl fmt::Display for LabelChange {
    /// One line per add or remove.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let adds = self
            .add
            .iter()
            .map(|l| format!("{}: add {} ({})", self.message_id, l.name, l.because));
        let removes = self
            .remove
            .iter()
            .map(|name| format!("{}: remove {name}", self.message_id));
        let lines: Vec<String> = adds.chain(removes).collect();
        f.write_str(&lines.join("\n"))
    }
}

/// What the Applier reports at the end of a Run.
#[derive(Debug, Default)]
pub struct Applied {
    pub counts: RunCounts,
    /// Filled only on a dry run: what would have been written.
    pub planned: Vec<LabelChange>,
}

pub struct Applier<'a, C> {
    client: &'a C,
    db: &'a Db,
    run_id: i64,
    dry_run: bool,
    labels: Vec<Label>,
    outcome: Applied,
}

impl<'a, C: EmailClient> Applier<'a, C> {
    /// # Errors
    ///
    /// Returns an error if the client cannot list its labels.
    pub async fn new(
        client: &'a C,
        db: &'a Db,
        run_id: i64,
        dry_run: bool,
    ) -> Result<Self, RunError> {
        Ok(Self {
            client,
            db,
            run_id,
            dry_run,
            labels: client.list_labels().await?,
            outcome: Applied::default(),
        })
    }

    /// Apply one Decision, then record it. Unknowns change nothing. A dry run
    /// only plans: the stored Decision must keep matching the labels actually
    /// in the mailbox, because Pins are detected against it.
    ///
    /// # Errors
    ///
    /// Returns an error if the client or database fails.
    pub async fn apply(&mut self, meta: &MessageMeta, decision: &Decision) -> Result<(), RunError> {
        if decision.is_unknown() {
            self.outcome.counts.unknown += 1;
        } else if let Some(change) = self.plan(meta, decision) {
            self.outcome.counts.labelled += 1;
            if self.dry_run {
                self.outcome.planned.push(change);
            } else {
                self.write(&change).await?;
            }
        }
        if !self.dry_run {
            self.db.record_decision(self.run_id, decision).await?;
        }
        Ok(())
    }

    #[must_use]
    pub fn finish(self) -> Applied {
        self.outcome
    }

    /// The change that makes the message's owned labels match the Decision,
    /// or `None` if they already do.
    fn plan(&self, meta: &MessageMeta, decision: &Decision) -> Option<LabelChange> {
        let (Some((category, source)), Some(email_type)) =
            (&decision.sender_category, &decision.email_type)
        else {
            return None;
        };
        let wanted = [
            PlannedLabel {
                name: format!("{SENDER_PREFIX}{category}"),
                because: format!("source {}", source.as_str()),
            },
            PlannedLabel {
                name: format!("{TYPE_PREFIX}{}", email_type.email_type),
                because: format!(
                    "signal {} {}",
                    email_type.signal.as_str(),
                    email_type.signal_key
                ),
            },
        ];
        let current: Vec<&str> = meta
            .label_ids
            .iter()
            .filter_map(|id| self.labels.iter().find(|l| &l.id == id))
            .map(|l| l.name.as_str())
            .filter(|name| is_owned(name))
            .collect();

        let add: Vec<PlannedLabel> = wanted
            .iter()
            .filter(|w| !current.contains(&w.name.as_str()))
            .cloned()
            .collect();
        let remove: Vec<String> = current
            .iter()
            .filter(|name| !wanted.iter().any(|w| w.name == **name))
            .map(|name| (*name).to_owned())
            .collect();
        if add.is_empty() && remove.is_empty() {
            return None;
        }
        Some(LabelChange {
            message_id: meta.id.clone(),
            add,
            remove,
        })
    }

    async fn write(&mut self, change: &LabelChange) -> Result<(), RunError> {
        let mut add_ids = Vec::new();
        for label in &change.add {
            add_ids.push(self.label_id(&label.name).await?);
        }
        let remove_ids: Vec<LabelId> = change
            .remove
            .iter()
            .filter_map(|name| self.labels.iter().find(|l| &l.name == name))
            .map(|l| l.id.clone())
            .collect();
        self.client
            .set_labels(&change.message_id, &add_ids, &remove_ids)
            .await?;
        Ok(())
    }

    /// The id of an owned label, created on the client if it does not exist.
    async fn label_id(&mut self, name: &str) -> Result<LabelId, RunError> {
        if let Some(label) = self.labels.iter().find(|l| l.name == name) {
            return Ok(label.id.clone());
        }
        let label = self.client.create_label(name).await?;
        let id = label.id.clone();
        self.labels.push(label);
        Ok(id)
    }
}
