//! The primary seam: a whole Run against the in-memory fake, no network.

use mail_scrub::classifier::{Signal, Source};
use mail_scrub::client::MessageMeta;
use mail_scrub::client::fake::{FakeClient, Mutation};
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

fn config_with_overrides(toml: &str) -> Config {
    toml.parse().unwrap()
}

/// Label names currently on a message, so tests don't depend on fake label ids.
fn label_names(client: &FakeClient, id: &str) -> Vec<String> {
    let labels = client.labels();
    let mut names: Vec<String> = client
        .labels_of(id)
        .unwrap()
        .iter()
        .map(|label_id| {
            labels
                .iter()
                .find(|l| &l.id == label_id)
                .map_or_else(|| label_id.clone(), |l| l.name.clone())
        })
        .collect();
    names.sort();
    names
}

const SHIPPING_OVERRIDES: &str = r#"
[overrides.sender]
"amazon.com" = "Shopping"

[[overrides.type]]
address = "ship-confirm@amazon.com"
type = "Shipping"
"#;

fn amazon_shipping(id: &str) -> MessageMeta {
    MessageMeta {
        sender_address: "ship-confirm@amazon.com".to_owned(),
        ..message(id)
    }
}

#[tokio::test]
async fn a_message_matching_both_overrides_gets_one_from_and_one_type_label() {
    let client = FakeClient::new();
    client.add_message(amazon_shipping("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);

    let summary = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert_eq!(
        label_names(&client, "m1"),
        ["From/Shopping", "INBOX", "Type/Shipping"]
    );
    assert_eq!(summary.labelled, 1);
    assert_eq!(summary.unknown, 0);
}

#[tokio::test]
async fn decisions_record_the_signal_and_source_that_fired() {
    let client = FakeClient::new();
    client.add_message(amazon_shipping("m1"));
    client.add_message(message("m2"));
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);

    let summary = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    let decisions = db.decisions().await.unwrap();
    let known = &decisions[0];
    assert_eq!(known.decision.message_id, "m1");
    assert_eq!(
        known.decision.sender_category,
        Some(("Shopping".to_owned(), Source::Override))
    );
    let email_type = known.decision.email_type.as_ref().unwrap();
    assert_eq!(email_type.email_type, "Shipping");
    assert_eq!(email_type.signal, Signal::Address);
    assert_eq!(email_type.signal_key, "ship-confirm@amazon.com");
    assert_eq!(known.run_id, summary.run_id);
    assert!(known.applied);

    let unknown = &decisions[1];
    assert_eq!(unknown.decision.message_id, "m2");
    assert_eq!(unknown.decision.sender_category, None);
    assert_eq!(unknown.decision.email_type, None);
}

#[tokio::test]
async fn a_domain_override_covers_subdomains_and_is_case_insensitive() {
    let client = FakeClient::new();
    client.add_message(MessageMeta {
        sender_address: "Orders@Email.Amazon.com".to_owned(),
        ..message("m1")
    });
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(
        r#"
[overrides.sender]
"amazon.com" = "Shopping"

[[overrides.type]]
address = "orders@email.amazon.com"
type = "Receipt"
"#,
    );

    run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert_eq!(
        label_names(&client, "m1"),
        ["From/Shopping", "INBOX", "Type/Receipt"]
    );
}

#[tokio::test]
async fn a_message_known_on_one_dimension_only_is_left_untouched() {
    let client = FakeClient::new();
    client.add_message(MessageMeta {
        sender_address: "someone@amazon.com".to_owned(),
        ..message("m1")
    });
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);

    let summary = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert_eq!(summary.unknown, 1);
    assert!(client.mutations().is_empty());
    let stored = &db.decisions().await.unwrap()[0].decision;
    assert_eq!(
        stored.sender_category,
        Some(("Shopping".to_owned(), Source::Override)),
        "the known half is still recorded"
    );
    assert_eq!(stored.email_type, None);
}

#[tokio::test]
async fn owned_labels_are_created_only_when_absent() {
    let client = FakeClient::new();
    client.seed_label("From/Shopping");
    client.add_message(amazon_shipping("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);

    run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    let created = client
        .mutations()
        .iter()
        .filter(|m| matches!(m, Mutation::CreateLabel { .. }))
        .count();
    assert_eq!(created, 1, "only the missing Type/ label is created");
    assert_eq!(
        label_names(&client, "m1"),
        ["From/Shopping", "INBOX", "Type/Shipping"]
    );
}

#[tokio::test]
async fn a_stale_owned_label_is_replaced_and_labels_outside_the_prefixes_are_kept() {
    let client = FakeClient::new();
    let stale = client.seed_label("From/Finance");
    let other_type = client.seed_label("Type/Promotion");
    let personal = client.seed_label("Projects/Kitchen");
    client.add_message(MessageMeta {
        label_ids: vec!["INBOX".to_owned(), stale, other_type, personal],
        ..amazon_shipping("m1")
    });
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);

    run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert_eq!(
        label_names(&client, "m1"),
        [
            "From/Shopping",
            "INBOX",
            "Projects/Kitchen",
            "Type/Shipping"
        ]
    );
}

#[tokio::test]
async fn a_message_already_labelled_correctly_is_not_written_again() {
    let client = FakeClient::new();
    let from = client.seed_label("From/Shopping");
    let kind = client.seed_label("Type/Shipping");
    client.add_message(MessageMeta {
        label_ids: vec![from, kind],
        ..amazon_shipping("m1")
    });
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);

    let summary = run::execute(&client, &db, &config, RunOptions::default())
        .await
        .unwrap();

    assert!(client.mutations().is_empty());
    assert_eq!(summary.labelled, 0);
    assert_eq!(summary.unknown, 0);
}

#[tokio::test]
async fn a_dry_run_plans_every_add_and_remove_and_writes_nothing_to_the_mailbox() {
    let client = FakeClient::new();
    let stale = client.seed_label("From/Finance");
    client.add_message(MessageMeta {
        label_ids: vec!["INBOX".to_owned(), stale],
        ..amazon_shipping("m1")
    });
    let db = Db::open_in_memory().await.unwrap();
    let config = config_with_overrides(SHIPPING_OVERRIDES);
    let options = RunOptions {
        dry_run: true,
        ..RunOptions::default()
    };

    let summary = run::execute(&client, &db, &config, options).await.unwrap();

    assert!(client.mutations().is_empty(), "no labels created or set");
    assert_eq!(label_names(&client, "m1"), ["From/Finance", "INBOX"]);
    let plan: Vec<String> = summary
        .planned
        .iter()
        .flat_map(|change| {
            change
                .to_string()
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        plan,
        [
            "m1: add From/Shopping (source override)",
            "m1: add Type/Shipping (signal address ship-confirm@amazon.com)",
            "m1: remove From/Finance",
        ]
    );
    let record = &db.decisions().await.unwrap()[0];
    assert!(!record.applied, "a dry-run Decision is marked unapplied");
}

#[tokio::test]
async fn editing_the_overrides_and_running_again_changes_the_result() {
    let client = FakeClient::new();
    client.add_message(amazon_shipping("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let full = RunOptions {
        full: true,
        ..RunOptions::default()
    };
    run::execute(
        &client,
        &db,
        &config_with_overrides(SHIPPING_OVERRIDES),
        full,
    )
    .await
    .unwrap();

    let edited = config_with_overrides(
        r#"
[overrides.sender]
"amazon.com" = "Finance"

[[overrides.type]]
address = "ship-confirm@amazon.com"
type = "Receipt"
"#,
    );
    run::execute(&client, &db, &edited, full).await.unwrap();

    assert_eq!(
        label_names(&client, "m1"),
        ["From/Finance", "INBOX", "Type/Receipt"]
    );
}

#[tokio::test]
async fn the_first_run_is_a_dry_run_unless_apply_is_asked_for() {
    let db = Db::open_in_memory().await.unwrap();

    assert!(run::resolve_dry_run(&db, false, false).await.unwrap());
    assert!(run::resolve_dry_run(&db, true, false).await.unwrap());
    assert!(!run::resolve_dry_run(&db, false, true).await.unwrap());
}

#[tokio::test]
async fn after_a_finished_run_plain_runs_apply_and_an_explicit_dry_run_still_previews() {
    let client = FakeClient::new();
    client.add_message(message("m1"));
    let db = Db::open_in_memory().await.unwrap();
    let config = Config::default();

    let dry = RunOptions {
        dry_run: true,
        ..RunOptions::default()
    };
    run::execute(&client, &db, &config, dry).await.unwrap();

    assert!(
        !run::resolve_dry_run(&db, false, false).await.unwrap(),
        "the preview has been seen; the next plain Run applies"
    );
    assert!(run::resolve_dry_run(&db, true, false).await.unwrap());
}
