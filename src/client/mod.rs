//! The provider boundary. Everything the tool does to a mailbox goes through
//! [`EmailClient`]; Gmail and the in-memory [`fake::FakeClient`] implement it.

pub mod fake;
pub mod gmail;

use std::fmt;
use std::future::Future;
use std::str::FromStr;

pub type MessageId = String;
pub type LabelId = String;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The provider cannot do this, e.g. it has no filters.
    #[error("operation not supported by this provider")]
    Unsupported,
    #[error("not found: {0}")]
    NotFound(String),
    /// The provider no longer honours our credentials; the user must sign in again.
    #[error("{}", crate::auth::EXPIRED_MESSAGE)]
    AuthExpired,
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync>),
}

/// The metadata a Run classifies on. Never includes the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageMeta {
    pub id: MessageId,
    pub thread_id: String,
    /// Bare address from the From header, e.g. `billing@email.chase.com`.
    pub sender_address: String,
    pub subject: Option<String>,
    /// Seconds since the Unix epoch.
    pub internal_date: i64,
    pub label_ids: Vec<LabelId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Label {
    pub id: LabelId,
    pub name: String,
}

/// A provider filter. `C` is the provider's own criteria type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter<C> {
    pub id: String,
    pub criteria: C,
    pub add_label_ids: Vec<LabelId>,
    pub remove_label_ids: Vec<LabelId>,
}

/// Messages to visit, and the cursor to store for the next Run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changes<C> {
    pub ids: Vec<MessageId>,
    pub cursor: C,
}

pub trait EmailClient: Send + Sync {
    /// Opaque position in the provider's change history. Round-trips through
    /// text so it can be stored in the Run log.
    type Cursor: Clone
        + Send
        + Sync
        + fmt::Display
        + FromStr<Err: std::error::Error + Send + Sync + 'static>;

    /// Opaque, provider-specific filter criteria.
    type FilterCriteria: Clone + Send + Sync + fmt::Debug + PartialEq;

    /// Ids of messages new or changed since `cursor`; all messages when `None`.
    fn changes_since(
        &self,
        cursor: Option<&Self::Cursor>,
    ) -> impl Future<Output = Result<Changes<Self::Cursor>, ClientError>> + Send;

    /// Metadata for a batch of ids.
    fn metadata(
        &self,
        ids: &[MessageId],
    ) -> impl Future<Output = Result<Vec<MessageMeta>, ClientError>> + Send;

    /// The body of one message. Callers parse it and discard it.
    fn body(&self, id: &str) -> impl Future<Output = Result<String, ClientError>> + Send;

    fn set_labels(
        &self,
        id: &str,
        add: &[LabelId],
        remove: &[LabelId],
    ) -> impl Future<Output = Result<(), ClientError>> + Send;

    fn list_filters(
        &self,
    ) -> impl Future<Output = Result<Vec<Filter<Self::FilterCriteria>>, ClientError>> + Send;

    /// Create a filter that adds `add_label_id` to matching mail. May return
    /// [`ClientError::Unsupported`], in which case the Run skips filters.
    fn create_filter(
        &self,
        criteria: Self::FilterCriteria,
        add_label_id: &str,
    ) -> impl Future<Output = Result<Filter<Self::FilterCriteria>, ClientError>> + Send;

    fn delete_filter(&self, id: &str) -> impl Future<Output = Result<(), ClientError>> + Send;

    fn list_labels(&self) -> impl Future<Output = Result<Vec<Label>, ClientError>> + Send;

    fn create_label(&self, name: &str) -> impl Future<Output = Result<Label, ClientError>> + Send;
}
