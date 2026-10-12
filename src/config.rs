//! The hand-edited TOML file: Taxonomy, Overrides, Source settings.
//!
//! Every field has a default, so an empty file is a valid config. Unknown keys
//! are rejected so a typo like `[overides.sender]` fails loudly instead of
//! silently doing nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid config: {0}")]
    Parse(#[from] toml::de::Error),
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub gmail: GmailConfig,
    pub sources: SourcesConfig,
    pub review: ReviewConfig,
    pub taxonomy: Taxonomy,
    pub overrides: Overrides,
    /// Source category → Taxonomy entry, per Source (`[map.ut1]`).
    pub map: BTreeMap<String, BTreeMap<String, String>>,
}

impl Config {
    /// Read and parse the config file at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a valid config.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_owned(),
            source,
        })?;
        text.parse()
    }
}

impl std::str::FromStr for Config {
    type Err = ConfigError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Ok(toml::from_str(text)?)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct GmailConfig {
    pub client_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SourcesConfig {
    /// Empty disables the Cloudflare Source.
    pub cloudflare_account_id: String,
    pub cloudflare_token_env: String,
    pub ut1_path: String,
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            cloudflare_account_id: String::new(),
            cloudflare_token_env: "MAILSCRUB_CF_TOKEN".to_owned(),
            ut1_path: "~/.local/share/mailscrub/ut1".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReviewConfig {
    pub max_questions: u32,
    pub promote_after_pins: u32,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            max_questions: 20,
            promote_after_pins: 2,
        }
    }
}

/// The Sender categories and Email types the tool may assign.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Taxonomy {
    #[serde(rename = "sender")]
    pub sender_categories: Vec<String>,
    #[serde(rename = "type")]
    pub email_types: Vec<String>,
}

impl Default for Taxonomy {
    fn default() -> Self {
        let own = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect();
        Self {
            sender_categories: own(&[
                "Finance",
                "Shopping",
                "Travel",
                "Software",
                "Social",
                "News",
                "Health",
                "Government",
                "Utilities",
                "Education",
                "Work",
                "People",
            ]),
            email_types: own(&[
                "Receipt",
                "Statement",
                "Shipping",
                "Security",
                "Account",
                "Promotion",
                "Newsletter",
                "Notification",
                "Calendar",
                "Conversation",
            ]),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Overrides {
    /// Sender domain → Sender category.
    pub sender: BTreeMap<String, String>,
    #[serde(rename = "type")]
    pub email_types: Vec<TypeOverride>,
}

/// An Email-type Override, keyed by sender address or subject pattern.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypeOverride {
    pub address: Option<String>,
    pub subject: Option<String>,
    #[serde(rename = "type")]
    pub email_type: String,
}
