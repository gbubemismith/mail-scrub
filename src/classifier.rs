//! The Classifier: a synchronous, pure function from metadata to a [`Decision`].
//!
//! It is the only place that will know the Taxonomy and the Signal order, and
//! it never does I/O.

use crate::client::{MessageId, MessageMeta};
use crate::config::Config;

/// The tool's classification of one message. `None` on a dimension = Unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub message_id: MessageId,
    pub sender_category: Option<String>,
    pub email_type: Option<String>,
}

impl Decision {
    /// Unknown on either dimension: such a message is left untouched.
    pub fn is_unknown(&self) -> bool {
        self.sender_category.is_none() || self.email_type.is_none()
    }
}

/// Skeleton: no Source or Signal exists yet, so everything is Unknown.
pub fn classify(_config: &Config, meta: &MessageMeta) -> Decision {
    Decision {
        message_id: meta.id.clone(),
        sender_category: None,
        email_type: None,
    }
}
