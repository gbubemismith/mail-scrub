//! The command-line front end. `main` parses a [`Cli`] and hands it to [`run`].

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

use crate::auth::{self, Endpoints, Keychain};
use crate::client::gmail::GmailClient;
use crate::config::{self, Config};
use crate::console::Terminal;
use crate::db::Db;
use crate::run::{self, RunOptions};
use crate::setup;

#[derive(Parser)]
#[command(
    name = "mailscrub",
    version,
    about = "Label Gmail by who sent it and what it is"
)]
pub struct Cli {
    /// Config file (default: mailscrub/mailscrub.toml in the XDG config dir)
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a Google Cloud OAuth client and store its token
    Setup,
    /// Classify and label mail
    Run {
        /// Print what would change without writing anything. The first Run does
        /// this by default.
        #[arg(long)]
        dry_run: bool,
        /// Write labels even on the first Run
        #[arg(long, conflicts_with = "dry_run")]
        apply: bool,
        /// Walk the whole mailbox instead of resuming from the last Run
        #[arg(long)]
        full: bool,
    },
    /// Answer outstanding questions without a full Run
    Review,
    /// Force a re-lookup of one Sender domain
    Refresh { domain: String },
    /// Show the cursor, last Run and counts of Overrides, Pins and Unknowns
    Status,
}

/// Execute a parsed command line.
///
/// # Errors
///
/// Returns an error if the config or database cannot be opened, Gmail
/// credentials are missing, the Run fails, or the command is not implemented
/// yet.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Run {
            dry_run,
            apply,
            full,
        } => run_command(cli.config, dry_run, apply, full).await,
        Command::Setup => {
            let path = config_path(cli.config)?;
            let downloads = downloads_dir()?;
            setup::run(
                &Terminal,
                &path,
                &downloads,
                &Keychain,
                &Endpoints::google(),
            )
            .await?;
            Ok(())
        }
        Command::Review => not_implemented("review"),
        Command::Refresh { .. } => not_implemented("refresh"),
        Command::Status => not_implemented("status"),
    }
}

async fn run_command(
    explicit_config: Option<PathBuf>,
    dry_run: bool,
    apply: bool,
    full: bool,
) -> anyhow::Result<()> {
    let explicit = explicit_config.is_some();
    let path = config_path(explicit_config)?;
    let config = load_config(&path, explicit)?;
    if config::is_readable_by_others(&path) {
        eprintln!(
            "warning: {} can be read by other users; run `chmod 600` on it",
            path.display()
        );
    }
    let client_id = config
        .gmail
        .client_id
        .as_deref()
        .context("no Gmail client ID in the config; run `mailscrub setup`")?;

    let db = Db::open(&data_dir()?.join("mailscrub.db"))
        .await
        .context("cannot open database")?;
    let session = auth::open_session(
        Arc::new(Keychain),
        Endpoints::google(),
        client_id,
        std::io::stdin().is_terminal(),
        &Terminal,
    )
    .await?;

    let previewing = run::resolve_dry_run(&db, dry_run, apply).await?;
    if previewing && !dry_run {
        eprintln!("note: first Run, previewing only; pass --apply to write labels");
    }
    let dry_run = previewing;
    let summary = run::execute(
        &GmailClient::new(session),
        &db,
        &config,
        RunOptions { dry_run, full },
    )
    .await?;
    for change in &summary.planned {
        println!("{change}");
    }
    println!("{summary}");
    Ok(())
}

fn not_implemented(command: &str) -> anyhow::Result<()> {
    bail!("`mailscrub {command}` is not implemented yet")
}

fn config_path(explicit: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    match explicit {
        Some(path) => Ok(path),
        None => Ok(config_dir()?.join("mailscrub.toml")),
    }
}

/// An explicit `--config` must exist; the default location may be absent, in
/// which case the built-in defaults apply.
fn load_config(path: &Path, explicit: bool) -> anyhow::Result<Config> {
    if explicit || path.exists() {
        Ok(Config::load(path)?)
    } else {
        Ok(Config::default())
    }
}

fn config_dir() -> anyhow::Result<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

fn data_dir() -> anyhow::Result<PathBuf> {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

fn downloads_dir() -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").context("$HOME is not set")?;
    Ok(PathBuf::from(home).join("Downloads"))
}

fn xdg_dir(var: &str, home_relative: &str) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join("mailscrub"));
    }
    let home = std::env::var_os("HOME").context("neither $XDG_* nor $HOME is set")?;
    Ok(PathBuf::from(home).join(home_relative).join("mailscrub"))
}
