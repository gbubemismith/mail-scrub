//! In-memory [`EmailClient`] for tests and for running the CLI offline.
//!
//! Seed it with `add_*`/`seed_*`, hand it to a Run, then assert on
//! [`FakeClient::mutations`], [`FakeClient::labels_of`] and
//! [`FakeClient::filters`]. Seeding is not recorded as a mutation.

use std::collections::HashMap;
use std::fmt;
use std::num::ParseIntError;
use std::str::FromStr;
use std::sync::Mutex;

use super::{Changes, ClientError, EmailClient, Filter, Label, LabelId, MessageId, MessageMeta};

/// Position in the fake's history: the sequence number of the newest message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FakeCursor(pub u64);

impl fmt::Display for FakeCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for FakeCursor {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(FakeCursor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCriteria(pub String);

/// A write the client was asked to make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mutation {
    CreateLabel {
        name: String,
    },
    SetLabels {
        message_id: MessageId,
        add: Vec<LabelId>,
        remove: Vec<LabelId>,
    },
    CreateFilter {
        criteria: FakeCriteria,
        add_label_id: LabelId,
    },
    DeleteFilter {
        id: String,
    },
}

#[derive(Default)]
struct State {
    /// Messages in insertion order, each with the sequence number it arrived at.
    messages: Vec<(u64, MessageMeta)>,
    bodies: HashMap<MessageId, String>,
    labels: Vec<Label>,
    filters: Vec<Filter<FakeCriteria>>,
    mutations: Vec<Mutation>,
    next_seq: u64,
    next_label: u64,
    next_filter: u64,
}

#[derive(Default)]
pub struct FakeClient {
    state: Mutex<State>,
}

impl FakeClient {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("fake client state poisoned")
    }

    /// Seed a message. It becomes visible to `changes_since` cursors taken
    /// before this call.
    pub fn add_message(&self, meta: MessageMeta) {
        let mut state = self.state();
        state.next_seq += 1;
        let seq = state.next_seq;
        state.messages.push((seq, meta));
    }

    pub fn set_body(&self, id: &str, body: &str) {
        self.state().bodies.insert(id.to_owned(), body.to_owned());
    }

    /// Seed an existing label, returning its id.
    pub fn seed_label(&self, name: &str) -> LabelId {
        let mut state = self.state();
        let label = new_label(&mut state, name);
        label.id
    }

    /// Seed an existing filter, returning its id.
    pub fn seed_filter(&self, criteria: FakeCriteria, add_label_ids: &[LabelId]) -> String {
        let mut state = self.state();
        state.next_filter += 1;
        let id = format!("filter_{}", state.next_filter);
        state.filters.push(Filter {
            id: id.clone(),
            criteria,
            add_label_ids: add_label_ids.to_vec(),
            remove_label_ids: Vec::new(),
        });
        id
    }

    /// Every write made through the trait, in order.
    pub fn mutations(&self) -> Vec<Mutation> {
        self.state().mutations.clone()
    }

    pub fn labels_of(&self, id: &str) -> Option<Vec<LabelId>> {
        let state = self.state();
        state
            .messages
            .iter()
            .find(|(_, m)| m.id == id)
            .map(|(_, m)| m.label_ids.clone())
    }

    pub fn filters(&self) -> Vec<Filter<FakeCriteria>> {
        self.state().filters.clone()
    }

    pub fn labels(&self) -> Vec<Label> {
        self.state().labels.clone()
    }
}

fn new_label(state: &mut State, name: &str) -> Label {
    state.next_label += 1;
    let label = Label {
        id: format!("Label_{}", state.next_label),
        name: name.to_owned(),
    };
    state.labels.push(label.clone());
    label
}

impl EmailClient for FakeClient {
    type Cursor = FakeCursor;
    type FilterCriteria = FakeCriteria;

    async fn changes_since(
        &self,
        cursor: Option<&FakeCursor>,
    ) -> Result<Changes<FakeCursor>, ClientError> {
        let state = self.state();
        let after = cursor.map_or(0, |c| c.0);
        let ids = state
            .messages
            .iter()
            .filter(|(seq, _)| *seq > after)
            .map(|(_, m)| m.id.clone())
            .collect();
        Ok(Changes {
            ids,
            cursor: FakeCursor(state.next_seq),
        })
    }

    async fn metadata(&self, ids: &[MessageId]) -> Result<Vec<MessageMeta>, ClientError> {
        let state = self.state();
        ids.iter()
            .map(|id| {
                state
                    .messages
                    .iter()
                    .find(|(_, m)| &m.id == id)
                    .map(|(_, m)| m.clone())
                    .ok_or_else(|| ClientError::NotFound(id.clone()))
            })
            .collect()
    }

    async fn body(&self, id: &str) -> Result<String, ClientError> {
        let state = self.state();
        if !state.messages.iter().any(|(_, m)| m.id == id) {
            return Err(ClientError::NotFound(id.to_owned()));
        }
        Ok(state.bodies.get(id).cloned().unwrap_or_default())
    }

    async fn set_labels(
        &self,
        id: &str,
        add: &[LabelId],
        remove: &[LabelId],
    ) -> Result<(), ClientError> {
        let mut state = self.state();
        let message = state
            .messages
            .iter_mut()
            .map(|(_, m)| m)
            .find(|m| m.id == id)
            .ok_or_else(|| ClientError::NotFound(id.to_owned()))?;
        message.label_ids.retain(|l| !remove.contains(l));
        for label in add {
            if !message.label_ids.contains(label) {
                message.label_ids.push(label.clone());
            }
        }
        state.mutations.push(Mutation::SetLabels {
            message_id: id.to_owned(),
            add: add.to_vec(),
            remove: remove.to_vec(),
        });
        Ok(())
    }

    async fn list_filters(&self) -> Result<Vec<Filter<FakeCriteria>>, ClientError> {
        Ok(self.state().filters.clone())
    }

    async fn create_filter(
        &self,
        criteria: FakeCriteria,
        add_label_id: &str,
    ) -> Result<Filter<FakeCriteria>, ClientError> {
        let mut state = self.state();
        state.next_filter += 1;
        let filter = Filter {
            id: format!("filter_{}", state.next_filter),
            criteria: criteria.clone(),
            add_label_ids: vec![add_label_id.to_owned()],
            remove_label_ids: Vec::new(),
        };
        state.filters.push(filter.clone());
        state.mutations.push(Mutation::CreateFilter {
            criteria,
            add_label_id: add_label_id.to_owned(),
        });
        Ok(filter)
    }

    async fn delete_filter(&self, id: &str) -> Result<(), ClientError> {
        let mut state = self.state();
        let before = state.filters.len();
        state.filters.retain(|f| f.id != id);
        if state.filters.len() == before {
            return Err(ClientError::NotFound(id.to_owned()));
        }
        state
            .mutations
            .push(Mutation::DeleteFilter { id: id.to_owned() });
        Ok(())
    }

    async fn list_labels(&self) -> Result<Vec<Label>, ClientError> {
        Ok(self.state().labels.clone())
    }

    async fn create_label(&self, name: &str) -> Result<Label, ClientError> {
        let mut state = self.state();
        let label = new_label(&mut state, name);
        state.mutations.push(Mutation::CreateLabel {
            name: name.to_owned(),
        });
        Ok(label)
    }
}
