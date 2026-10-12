//! `mailscrub run` end to end through the real binary.

use std::process::Command;

use mail_scrub::db::{Db, RunKind};

fn mailscrub() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mailscrub"))
}

#[tokio::test]
async fn run_completes_against_an_empty_mailbox_and_records_one_run() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("mailscrub.toml");
    let db_path = dir.path().join("mailscrub.db");
    std::fs::write(&config, "[overrides.sender]\n").unwrap();

    let output = mailscrub()
        .arg("--config")
        .arg(&config)
        .arg("--db")
        .arg(&db_path)
        .arg("run")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("0 labelled, 0 filters changed, 0 unknown"),
        "{stdout}"
    );
    let runs = Db::open(&db_path).await.unwrap().runs().await.unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].kind, RunKind::Full);
}

#[test]
fn the_other_subcommands_exist_but_are_not_implemented_yet() {
    for args in [
        &["setup"][..],
        &["review"],
        &["refresh", "chase.com"],
        &["status"],
    ] {
        let output = mailscrub().args(args).output().unwrap();

        assert!(
            !output.status.success(),
            "{args:?} should not pretend to work"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("not implemented"), "{args:?}: {stderr}");
    }
}

#[test]
fn an_explicit_config_path_that_does_not_exist_is_an_error() {
    let dir = tempfile::tempdir().unwrap();

    let output = mailscrub()
        .arg("--config")
        .arg(dir.path().join("nope.toml"))
        .arg("--db")
        .arg(dir.path().join("mailscrub.db"))
        .arg("run")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("nope.toml"));
}
