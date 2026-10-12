use std::path::PathBuf;

use anyhow::{Context, bail};
use clap::{Parser, Subcommand};
use mail_scrub::client::fake::FakeClient;
use mail_scrub::config::Config;
use mail_scrub::db::Db;
use mail_scrub::run::{self, RunOptions};

#[derive(Parser)]
#[command(
    name = "mailscrub",
    version,
    about = "Label Gmail by who sent it and what it is"
)]
struct Cli {
    /// Config file [default: $XDG_CONFIG_HOME/mailscrub/mailscrub.toml]
    #[arg(long, global = true)]
    config: Option<PathBuf>,

    /// Database file [default: $XDG_DATA_HOME/mailscrub/mailscrub.db]
    #[arg(long, global = true)]
    db: Option<PathBuf>,

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { dry_run, full } => {
            let config = load_config(cli.config)?;
            let db_path = match cli.db {
                Some(path) => path,
                None => data_dir()?.join("mailscrub.db"),
            };
            let db = Db::open(&db_path).await.context("cannot open database")?;
            // The Gmail client arrives in MS-03; until then a Run sees an empty fake mailbox.
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
    match explicit {
        Some(path) => Ok(Config::load(&path)?),
        None => {
            let path = config_dir()?.join("mailscrub.toml");
            if path.exists() {
                Ok(Config::load(&path)?)
            } else {
                Ok(Config::default())
            }
        }
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
