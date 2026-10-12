#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![warn(clippy::pedantic)]

use clap::Parser;
use mail_scrub::cli::{self, Cli};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    cli::run(Cli::parse()).await
}
