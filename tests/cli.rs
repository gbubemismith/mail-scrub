//! The real binary's command line.

use std::process::Command;

fn mailscrub() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mailscrub"))
}

#[test]
fn run_without_a_client_id_points_at_setup() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("mailscrub.toml");
    std::fs::write(&config, "[overrides.sender]\n").unwrap();

    let output = mailscrub()
        .arg("--config")
        .arg(&config)
        .env("XDG_DATA_HOME", dir.path())
        .arg("run")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("mailscrub setup"), "{stderr}");
}

#[test]
fn dry_run_and_apply_contradict_each_other() {
    let output = mailscrub()
        .args(["run", "--dry-run", "--apply"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used with"));
}

#[test]
fn the_other_subcommands_exist_but_are_not_implemented_yet() {
    for args in [&["review"][..], &["refresh", "chase.com"], &["status"]] {
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
        .env("XDG_DATA_HOME", dir.path())
        .arg("run")
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("nope.toml"));
}
