//! Getting and keeping Gmail credentials, against a mock Google.

mod common;

use std::sync::Arc;

use common::ScriptedConsole;
use mail_scrub::auth::{
    self, AccessToken, AuthError, Credentials, Endpoints, MemoryStore, SecretStore,
};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn endpoints(server: &MockServer) -> Endpoints {
    Endpoints {
        auth_url: "https://accounts.example.test/auth".to_owned(),
        token_url: format!("{}/token", server.uri()),
    }
}

fn token_response(access: &str, refresh: Option<&str>) -> ResponseTemplate {
    let mut body = serde_json::json!({"access_token": access, "expires_in": 3600});
    if let Some(refresh) = refresh {
        body["refresh_token"] = refresh.into();
    }
    ResponseTemplate::new(200).set_body_json(body)
}

fn expired_refresh() -> ResponseTemplate {
    ResponseTemplate::new(400).set_body_json(serde_json::json!({
        "error": "invalid_grant",
        "error_description": "Token has been expired or revoked."
    }))
}

fn stored(refresh: &str, access: Option<AccessToken>) -> Arc<MemoryStore> {
    let store = MemoryStore::default();
    store
        .save(&Credentials {
            client_secret: "shh".to_owned(),
            refresh_token: refresh.to_owned(),
            access_token: access,
        })
        .unwrap();
    Arc::new(store)
}

#[tokio::test]
async fn consent_exchanges_the_code_for_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("code=the-code"))
        .and(body_string_contains("client_secret=shh"))
        .and(body_string_contains("code_verifier="))
        .respond_with(token_response("access-1", Some("refresh-1")))
        .mount(&server)
        .await;
    let console = ScriptedConsole::granting("the-code");

    let creds = auth::consent(
        &reqwest::Client::new(),
        &endpoints(&server),
        "id",
        "shh",
        &console,
    )
    .await
    .unwrap();

    assert_eq!(creds.refresh_token, "refresh-1");
    assert_eq!(creds.access_token.unwrap().token, "access-1");
    assert_eq!(creds.client_secret, "shh");
    let opened = console.opened();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].starts_with("https://accounts.example.test/auth?"));
    assert!(opened[0].contains("gmail.modify"));
    assert!(opened[0].contains("gmail.settings.basic"));
}

#[tokio::test]
async fn denied_consent_is_an_error() {
    let server = MockServer::start().await;
    let console = ScriptedConsole::default();

    let err = auth::consent(
        &reqwest::Client::new(),
        &endpoints(&server),
        "id",
        "shh",
        &console,
    )
    .await
    .err()
    .unwrap();

    assert!(matches!(err, AuthError::ConsentDenied(_)), "{err}");
}

#[tokio::test]
async fn a_stale_access_token_is_refreshed_and_saved_to_the_store() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=refresh-1"))
        .respond_with(token_response("access-2", None))
        .mount(&server)
        .await;
    let store = stored("refresh-1", None);

    let session = auth::open_session(
        store.clone(),
        endpoints(&server),
        "id",
        false,
        &ScriptedConsole::default(),
    )
    .await
    .unwrap();

    assert_eq!(session.access_token().await.unwrap(), "access-2");
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.access_token.unwrap().token, "access-2");
    assert_eq!(saved.refresh_token, "refresh-1");
}

#[tokio::test]
async fn an_expired_refresh_token_exits_clearly_when_not_interactive() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(expired_refresh())
        .mount(&server)
        .await;
    let console = ScriptedConsole::granting("never-used");

    let err = auth::open_session(
        stored("old", None),
        endpoints(&server),
        "id",
        false,
        &console,
    )
    .await
    .err()
    .unwrap();

    assert!(matches!(err, AuthError::RefreshExpired), "{err}");
    assert!(console.opened().is_empty(), "no browser without a terminal");
}

#[tokio::test]
async fn an_expired_refresh_token_reopens_the_browser_when_interactive() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(expired_refresh())
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(token_response("access-new", Some("refresh-new")))
        .mount(&server)
        .await;
    let console = ScriptedConsole::granting("code-2");
    let store = stored("old", None);

    let session = auth::open_session(store.clone(), endpoints(&server), "id", true, &console)
        .await
        .unwrap();

    assert_eq!(console.opened().len(), 1);
    assert_eq!(session.access_token().await.unwrap(), "access-new");
    let saved = store.load().unwrap().unwrap();
    assert_eq!(saved.refresh_token, "refresh-new");
    assert_eq!(saved.client_secret, "shh", "the client secret is kept");
}

#[tokio::test]
async fn a_silent_connection_to_the_redirect_port_does_not_block_consent() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(token_response("access-1", Some("refresh-1")))
        .mount(&server)
        .await;
    let console = ScriptedConsole::granting("the-code").preconnecting_silently();

    let creds = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        auth::consent(
            &reqwest::Client::new(),
            &endpoints(&server),
            "id",
            "shh",
            &console,
        ),
    )
    .await
    .expect("consent was stuck behind the silent connection")
    .unwrap();

    assert_eq!(creds.refresh_token, "refresh-1");
}
