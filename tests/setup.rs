//! `mailscrub setup` driven through a scripted console and a mock Google.

mod common;

use std::path::{Path, PathBuf};

use common::ScriptedConsole;
use mail_scrub::auth::{Endpoints, MemoryStore, SecretStore};
use mail_scrub::config::Config;
use mail_scrub::setup::{self, SetupError};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn google(server: &MockServer) -> Endpoints {
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-1",
            "refresh_token": "refresh-1",
            "expires_in": 3600
        })))
        .mount(server)
        .await;
    Endpoints {
        auth_url: "https://accounts.example.test/auth".to_owned(),
        token_url: format!("{}/token", server.uri()),
    }
}

/// Writes a client file in the shape Google's console downloads for a Desktop app.
fn write_desktop_client(dir: &Path, file: &str, project_id: &str, client_id: &str) -> PathBuf {
    let path = dir.join(file);
    std::fs::write(
        &path,
        format!(
            r#"{{"installed":{{"client_id":"{client_id}","project_id":"{project_id}",
"client_secret":"GOCSPX-secret","auth_uri":"https://accounts.google.com/o/oauth2/auth",
"token_uri":"https://oauth2.googleapis.com/token","redirect_uris":["http://localhost"]}}}}"#
        ),
    )
    .unwrap();
    path
}

/// The whole flow with the client file accepted at its default.
fn happy_console() -> ScriptedConsole {
    ScriptedConsole::granting("code-1")
        .answering("") // step 1 done
        .answering("my-proj")
        .answering("") // step 2 done
        .answering("") // step 3 done
        .answering("") // use the found file
}

#[tokio::test]
async fn setup_opens_three_deep_linked_pages_then_runs_consent_with_the_file_client() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    write_desktop_client(
        downloads.path(),
        "client_secret_1.json",
        "my-proj",
        "my-client-id.apps.googleusercontent.com",
    );
    let console = happy_console();

    setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        downloads.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await
    .unwrap();

    let opened = console.opened();
    let (consent, pages) = opened.split_last().unwrap();
    assert_eq!(pages.len(), 3, "{pages:?}");
    assert!(
        pages
            .iter()
            .all(|url| url.starts_with("https://console.cloud.google.com/")),
        "{pages:?}"
    );
    assert!(pages[1].contains("project=my-proj"), "{pages:?}");
    assert!(pages[2].contains("project=my-proj"), "{pages:?}");
    assert!(consent.starts_with("https://accounts.example.test/auth?"));
    assert!(consent.contains("client_id=my-client-id.apps.googleusercontent.com"));
}

#[tokio::test]
async fn setup_stores_secrets_in_the_keychain_and_only_the_client_id_in_the_toml() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    write_desktop_client(
        downloads.path(),
        "client_secret_1.json",
        "my-proj",
        "my-client-id.apps.googleusercontent.com",
    );
    let config_path = dir.path().join("mailscrub.toml");
    let store = MemoryStore::default();

    setup::run(
        &happy_console(),
        &config_path,
        downloads.path(),
        &store,
        &endpoints,
    )
    .await
    .unwrap();

    let creds = store.load().unwrap().unwrap();
    assert_eq!(creds.client_secret, "GOCSPX-secret");
    assert_eq!(creds.refresh_token, "refresh-1");
    assert_eq!(creds.access_token.unwrap().token, "access-1");
    let toml = std::fs::read_to_string(&config_path).unwrap();
    assert_eq!(
        Config::load(&config_path)
            .unwrap()
            .gmail
            .client_id
            .as_deref(),
        Some("my-client-id.apps.googleusercontent.com")
    );
    for secret in ["GOCSPX-secret", "refresh-1", "access-1"] {
        assert!(
            !toml.contains(secret),
            "{secret} leaked into the config: {toml}"
        );
    }
}

#[tokio::test]
async fn setup_picks_the_download_for_the_entered_project() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    write_desktop_client(
        downloads.path(),
        "client_secret_mine.json",
        "my-proj",
        "mine.apps.googleusercontent.com",
    );
    // Written last, so it is the newest file in the directory.
    write_desktop_client(
        downloads.path(),
        "client_secret_other.json",
        "other-proj",
        "other.apps.googleusercontent.com",
    );
    let console = happy_console();

    setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        downloads.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await
    .unwrap();

    let opened = console.opened();
    let consent = opened.last().unwrap();
    assert!(consent.contains("client_id=mine.apps.googleusercontent.com"));
}

#[tokio::test]
async fn setup_uses_the_path_the_user_gives_when_nothing_is_found() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let empty_downloads = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let client = write_desktop_client(
        elsewhere.path(),
        "my-client.json",
        "my-proj",
        "typed.apps.googleusercontent.com",
    );
    let console = ScriptedConsole::granting("code-1")
        .answering("")
        .answering("my-proj")
        .answering("")
        .answering("")
        .answering(client.to_str().unwrap());

    setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        empty_downloads.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await
    .unwrap();

    let opened = console.opened();
    let consent = opened.last().unwrap();
    assert!(consent.contains("client_id=typed.apps.googleusercontent.com"));
}

#[tokio::test]
async fn setup_rejects_a_web_client_and_stores_nothing() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let web = downloads.path().join("client_secret_web.json");
    std::fs::write(
        &web,
        r#"{"web":{"client_id":"web.apps.googleusercontent.com","project_id":"my-proj",
"client_secret":"GOCSPX-web","redirect_uris":["http://localhost"]}}"#,
    )
    .unwrap();
    let config_path = dir.path().join("mailscrub.toml");
    let store = MemoryStore::default();
    let console = ScriptedConsole::granting("code-1")
        .answering("")
        .answering("my-proj")
        .answering("")
        .answering("")
        .answering(web.to_str().unwrap());

    let error = setup::run(&console, &config_path, downloads.path(), &store, &endpoints)
        .await
        .unwrap_err();

    assert!(matches!(error, SetupError::WebClient { .. }), "{error}");
    assert!(store.load().unwrap().is_none());
    assert!(!config_path.exists());
}

#[tokio::test]
async fn setup_names_a_client_file_it_cannot_parse() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let broken = downloads.path().join("client_secret_broken.json");
    std::fs::write(&broken, r#"{"installed":{"client_id":"x"}}"#).unwrap();
    let console = ScriptedConsole::granting("code-1")
        .answering("")
        .answering("my-proj")
        .answering("")
        .answering("")
        .answering(broken.to_str().unwrap());

    let error = setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        downloads.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await
    .unwrap_err();

    assert!(
        matches!(&error, SetupError::ClientFileInvalid { path, .. } if *path == broken),
        "{error}"
    );
}

#[tokio::test]
async fn setup_trusts_a_chosen_file_from_a_different_project_and_warns() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let client = write_desktop_client(
        elsewhere.path(),
        "my-client.json",
        "other-proj",
        "other.apps.googleusercontent.com",
    );
    let console = ScriptedConsole::granting("code-1")
        .answering("")
        .answering("my-proj")
        .answering("")
        .answering("")
        .answering(client.to_str().unwrap());

    setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        downloads.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await
    .unwrap();

    let opened = console.opened();
    assert!(
        opened
            .last()
            .unwrap()
            .contains("client_id=other.apps.googleusercontent.com")
    );
    assert!(console.said().contains("other-proj"), "{}", console.said());
}

#[tokio::test]
async fn setup_asks_again_after_an_invalid_project_id() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    write_desktop_client(
        downloads.path(),
        "client_secret_1.json",
        "my-proj",
        "my-client-id.apps.googleusercontent.com",
    );
    let console = ScriptedConsole::granting("code-1")
        .answering("")
        .answering("My Project")
        .answering("my-proj")
        .answering("")
        .answering("")
        .answering("");

    setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        downloads.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await
    .unwrap();

    let opened = console.opened();
    assert!(opened[1].contains("project=my-proj"), "{opened:?}");
}

#[tokio::test]
async fn setup_refuses_an_empty_project_id() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let console = ScriptedConsole::granting("code-1")
        .answering("")
        .answering("");

    let result = setup::run(
        &console,
        &dir.path().join("mailscrub.toml"),
        dir.path(),
        &MemoryStore::default(),
        &endpoints,
    )
    .await;

    assert!(matches!(result, Err(SetupError::Empty(_))), "{result:?}");
}

#[tokio::test]
async fn failed_consent_leaves_nothing_stored() {
    let server = MockServer::start().await;
    let endpoints = google(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let downloads = tempfile::tempdir().unwrap();
    write_desktop_client(
        downloads.path(),
        "client_secret_1.json",
        "my-proj",
        "my-client-id.apps.googleusercontent.com",
    );
    let config_path = dir.path().join("mailscrub.toml");
    let store = MemoryStore::default();
    let denied = ScriptedConsole::default()
        .answering("")
        .answering("my-proj")
        .answering("")
        .answering("")
        .answering("");

    let result = setup::run(&denied, &config_path, downloads.path(), &store, &endpoints).await;

    assert!(result.is_err());
    assert!(store.load().unwrap().is_none());
    assert!(!config_path.exists());
}
