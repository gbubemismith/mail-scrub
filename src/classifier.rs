//! The Classifier: a synchronous, pure function from metadata to a [`Decision`].
//!
//! It is the only place that will know the Taxonomy and the Signal order, and
//! it never does I/O.

use std::str::FromStr;

use crate::client::{MessageId, MessageMeta};
use crate::config::Config;

/// Where a Sender category came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Override,
}

impl Source {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Override => "override",
        }
    }
}

impl FromStr for Source {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "override" => Ok(Source::Override),
            other => Err(format!("unknown source {other:?}")),
        }
    }
}

/// Which check decided an Email type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Address,
}

impl Signal {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Signal::Address => "address",
        }
    }
}

impl FromStr for Signal {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "address" => Ok(Signal::Address),
            other => Err(format!("unknown signal {other:?}")),
        }
    }
}

/// The Email type and the Signal that decided it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDecision {
    pub email_type: String,
    pub signal: Signal,
    /// The address or pattern that fired.
    pub signal_key: String,
}

/// The tool's classification of one message. `None` on a dimension = Unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub message_id: MessageId,
    /// The Sender category and the Source that produced it.
    pub sender_category: Option<(String, Source)>,
    pub email_type: Option<TypeDecision>,
}

impl Decision {
    /// Unknown on either dimension: such a message is left untouched.
    #[must_use]
    pub fn is_unknown(&self) -> bool {
        self.sender_category.is_none() || self.email_type.is_none()
    }
}

/// The host part of an address, lowercased: `Billing@Email.Chase.com` → `email.chase.com`.
#[must_use]
pub fn sender_host(address: &str) -> String {
    address
        .rsplit_once('@')
        .map_or(address, |(_, host)| host)
        .to_ascii_lowercase()
}

/// Classify one message from its metadata.
#[must_use]
pub fn classify(config: &Config, meta: &MessageMeta) -> Decision {
    Decision {
        message_id: meta.id.clone(),
        sender_category: sender_category_override(config, &sender_host(&meta.sender_address))
            .map(|category| (category, Source::Override)),
        email_type: type_override(config, &meta.sender_address),
    }
}

/// An Override for the host or the nearest parent domain that has one, so an
/// Override on `chase.com` covers `email.chase.com`.
fn sender_category_override(config: &Config, host: &str) -> Option<String> {
    let mut domain = host;
    loop {
        if let Some(category) = config.overrides.sender.get(domain) {
            return Some(category.clone());
        }
        domain = domain.split_once('.')?.1;
    }
}

fn type_override(config: &Config, address: &str) -> Option<TypeDecision> {
    config
        .overrides
        .email_types
        .iter()
        .find(|o| {
            o.address
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(address))
        })
        .map(|o| TypeDecision {
            email_type: o.email_type.clone(),
            signal: Signal::Address,
            signal_key: address.to_ascii_lowercase(),
        })
}
