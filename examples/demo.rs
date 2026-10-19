//! Run one Run against a seeded in-memory mailbox and show the labels.
//!
//!     cargo run --example demo -- examples/demo.toml [--dry-run]
//!
//! Edit the TOML and run again to see the labels change.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![warn(clippy::pedantic)]

use std::path::PathBuf;

use anyhow::{Context, bail};
use mail_scrub::client::MessageMeta;
use mail_scrub::client::fake::FakeClient;
use mail_scrub::config::Config;
use mail_scrub::db::Db;
use mail_scrub::run::{self, RunOptions};
use tokio::sync::mpsc;

fn message(id: &str, from: &str, subject: &str, labels: Vec<String>) -> MessageMeta {
    MessageMeta {
        id: id.to_owned(),
        thread_id: format!("thread-{id}"),
        sender_address: from.to_owned(),
        subject: Some(subject.to_owned()),
        internal_date: 1_700_000_000,
        label_ids: labels,
    }
}

fn seed() -> FakeClient {
    let client = FakeClient::new();
    // An earlier, wrong label the Run should replace, plus one it must leave alone.
    let stale = client.seed_label("From/Finance");
    let personal = client.seed_label("Projects/Kitchen");
    let inbox = "INBOX".to_owned();
    client.add_message(message(
        "amazon-1",
        "ship-confirm@amazon.com",
        "Your package has shipped",
        vec![inbox.clone(), stale, personal],
    ));
    client.add_message(message(
        "chase-1",
        "no-reply@alertsp.chase.com",
        "Sign-in from a new device",
        vec![inbox.clone()],
    ));
    client.add_message(message(
        "amazon-2",
        "deals@amazon.com",
        "Today's deals",
        vec![inbox.clone()],
    ));
    client.add_message(message(
        "friend-1",
        "sam@example.org",
        "Lunch?",
        vec![inbox],
    ));
    client
}

fn print_labels(client: &FakeClient, heading: &str) {
    println!("{heading}");
    let labels = client.labels();
    for id in ["amazon-1", "chase-1", "amazon-2", "friend-1"] {
        let names: Vec<String> = client
            .labels_of(id)
            .unwrap_or_default()
            .iter()
            .map(|label_id| {
                labels
                    .iter()
                    .find(|l| &l.id == label_id)
                    .map_or_else(|| label_id.clone(), |l| l.name.clone())
            })
            .collect();
        println!("  {id:<10} {}", names.join(", "));
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut path = None;
    let mut dry_run = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            _ if path.is_none() => path = Some(PathBuf::from(arg)),
            _ => bail!("usage: demo <config.toml> [--dry-run]"),
        }
    }
    let path = path.context("usage: demo <config.toml> [--dry-run]")?;
    let config = Config::load(&path)?;

    let client = seed();
    let db = Db::open_in_memory().await?;
    print_labels(&client, "Before:");

    let summary = run::execute(
        &client,
        &db,
        &config,
        RunOptions {
            dry_run,
            ..RunOptions::default()
        },
        mpsc::unbounded_channel().0,
    )
    .await?;

    // A dry run stores no Decisions, so this section is empty for one.
    println!("\nDecisions:");
    for record in db.decisions().await? {
        let d = &record.decision;
        let from = d.sender_category.as_ref().map_or_else(
            || "Unknown".to_owned(),
            |(c, s)| format!("{c} ({})", s.as_str()),
        );
        let kind = d.email_type.as_ref().map_or_else(
            || "Unknown".to_owned(),
            |t| format!("{} ({} {})", t.email_type, t.signal.as_str(), t.signal_key),
        );
        println!("  {:<10} From: {from}; Type: {kind}", d.message_id);
    }

    if dry_run {
        println!("\nPlanned changes:");
        for change in &summary.planned {
            println!("{change}");
        }
    }
    print_labels(&client, "\nAfter:");
    println!("\n{summary}");
    Ok(())
}
