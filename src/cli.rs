//! The command-line front end. `main` parses a [`Cli`] and hands it to [`run`].

use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};

use crate::client::fake::FakeClient;
use crate::config::Config;
use crate::db::Db;
use crate::run::{self, RunOptions};

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
        /// Print what would change without writing anything
        #[arg(long)]
        dry_run: bool,
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
/// Returns an error if the config or database cannot be opened, the Run
/// fails, or the command is not implemented yet.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    match cli.command {
        Command::Run { dry_run, full } => {
            let config = load_config(cli.config)?;
            let db = Db::open(&data_dir()?.join("mailscrub.db"))
                .await
                .context("cannot open database")?;
            eprintln!("note: using the in-memory fake client; Gmail is not wired up yet");
            let summary = run::execute(
                &FakeClient::new(),
                &db,
                &config,
                RunOptions { dry_run, full },
            )
            .await?;
            println!("{summary}");
            Ok(())
        }
        Command::Setup => not_implemented("setup"),
        Command::Review => not_implemented("review"),
        Command::Refresh { .. } => not_implemented("refresh"),
        Command::Status => not_implemented("status"),
    }
}

fn not_implemented(command: &str) -> anyhow::Result<()> {
    bail!("`mailscrub {command}` is not implemented yet")
}

/// An explicit `--config` must exist; the default location may be absent, in
/// which case the built-in defaults apply.
fn load_config(explicit: Option<PathBuf>) -> anyhow::Result<Config> {
    if let Some(path) = explicit {
        return Ok(Config::load(&path)?);
    }
    let path = config_dir()?.join("mailscrub.toml");
    if path.exists() {
        Ok(Config::load(&path)?)
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

fn xdg_dir(var: &str, home_relative: &str) -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os(var).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir).join("mailscrub"));
    }
    let home = std::env::var_os("HOME").context("neither $XDG_* nor $HOME is set")?;
    Ok(PathBuf::from(home).join(home_relative).join("mailscrub"))
}
