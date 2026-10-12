//! The primary seam: a whole Run against the in-memory fake, no network.

use mail_scrub::client::MessageMeta;
use mail_scrub::client::fake::FakeClient;
use mail_scrub::config::Config;
use mail_scrub::db::{Db, RunKind};
use mail_scrub::run::{self, RunOptions};

#[tokio::test]
async fn run_on_an_empty_mailbox_changes_nothing_and_records_one_run() {
    let client = FakeClient::new();
    let db = Db::open_in_memory().await.unwrap();

    let summary = run::execute(&client, &db, &Config::default(), RunOptions::default())
        .await
        .unwrap();

    assert!(client.mutations().is_empty());
    assert_eq!(summary.labelled, 0);
    assert_eq!(summary.filters_changed, 0);
    assert_eq!(summary.unknown, 0);
    let runs = db.runs().await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, summary.run_id);
}

fn message(id: &str) -> MessageMeta {
    MessageMeta {
        id: id.to_owned(),
        thread_id: format!("thread-{id}"),
        sender_address: "someone@example.com".to_owned(),
        subject: Some("Hello".to_owned()),
        internal_date: 1_700_000_000,
        label_ids: vec!["INBOX".to_owned()],
    }
}

#[tokio::test]
async fn messages_that_cannot_be_decided_are_counted_unknown_and_left_untouched() {
    let client = FakeClient::new();
    client.add_message(message("m1"));
    client.add_message(message("m2"));
    let db = Db::open_in_memory().await.unwrap();

    let summary = run::execute(&client, &db, &Config::default(), RunOptions::default())
        .await
        .unwrap();

    assert_eq!(summary.unknown, 2);
    assert_eq!(summary.labelled, 0);
    assert!(client.mutations().is_empty());
    assert_eq!(client.labels_of("m1").unwrap(), ["INBOX"]);
    assert_eq!(db.runs().await.unwrap()[0].counts.unknown, 2);
}

#[tokio::test]
async fn a_second_run_resumes_from_the_stored_cursor() {
    let client = FakeClient::new();
    client.add_message(message("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let config = Config::default();

    let first = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();
    client.add_message(message("m2"));
    let second = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert_eq!(first.kind, RunKind::Full);
    assert_eq!(second.kind, RunKind::Incremental);
    assert_eq!(second.unknown, 1, "only the new message is visited");
    let runs = db.runs().await.unwrap();
    assert_eq!(runs.len(), 2);
    assert!(runs[0].cursor_after.is_some());
    assert_eq!(runs[0].cursor_before, None);
    assert_eq!(runs[1].cursor_before, runs[0].cursor_after);
    assert_ne!(
        runs[1].cursor_after, runs[0].cursor_after,
        "the cursor moved past the new message"
    );
}

#[tokio::test]
async fn a_full_run_ignores_the_stored_cursor() {
    let client = FakeClient::new();
    client.add_message(message("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let config = Config::default();
    run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    let options = RunOptions {
        full: true,
        ..RunOptions::default()
    };
    let again = run::execute(&client, &db, &config, options).await.unwrap();

    assert_eq!(again.kind, RunKind::Full);
    assert_eq!(again.unknown, 1);
}

#[tokio::test]
async fn a_dry_run_does_not_advance_the_cursor() {
    let client = FakeClient::new();
    client.add_message(message("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let config = Config::default();

    let dry = RunOptions {
        dry_run: true,
        ..RunOptions::default()
    };
    let preview = run::execute(&client, &db, &config, dry).await.unwrap();
    let real = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert!(preview.dry_run);
    assert_eq!(
        real.kind,
        RunKind::Full,
        "a dry run is not a baseline for the next Run"
    );
    assert_eq!(real.unknown, 1);
    assert!(db.runs().await.unwrap()[0].dry_run);
}
