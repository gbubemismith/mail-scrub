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
    #[error("cannot write config {}: {source}", path.display())]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot edit config {}: `gmail` is not a table", .0.display())]
    GmailNotATable(PathBuf),
    #[error("cannot edit config {}: {source}", path.display())]
    Edit {
        path: PathBuf,
        source: toml_edit::TomlError,
    },
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

/// Record the Gmail client ID, creating the file (owner-only, since the
/// Overrides in it reveal who emails the user) if it does not exist and
/// leaving everything else in an existing file as the user wrote it.
///
/// # Errors
///
/// Returns an error if the file cannot be read, parsed or written.
pub fn set_client_id(path: &Path, client_id: &str) -> Result<(), ConfigError> {
    let write_err = |source| ConfigError::Write {
        path: path.to_owned(),
        source,
    };
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };
    let mut doc: toml_edit::DocumentMut = existing.parse().map_err(|source| ConfigError::Edit {
        path: path.to_owned(),
        source,
    })?;
    doc.entry("gmail")
        .or_insert_with(toml_edit::table)
        .as_table_like_mut()
        .ok_or_else(|| ConfigError::GmailNotATable(path.to_owned()))?
        .insert("client_id", toml_edit::value(client_id));

    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(write_err)?;
    }
    write_owner_only(path, &doc.to_string()).map_err(write_err)
}

/// Write with mode 0600 when creating; an existing file keeps its mode.
fn write_owner_only(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(text.as_bytes())
}

/// Whether group or others can read the file. Always `false` where the
/// platform has no such permission bits.
pub fn is_readable_by_others(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.permissions().mode() & 0o044 != 0)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
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
