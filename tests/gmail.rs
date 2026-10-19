//! The Gmail client against a mock Gmail API.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::ScriptedConsole;
use mail_scrub::auth::{
    self, AccessToken, Credentials, Endpoints, MemoryStore, SecretStore, Session,
};
use mail_scrub::client::gmail::{GmailClient, bare_address};
use mail_scrub::client::{ClientError, EmailClient};
use serde_json::json;
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn client(server: &MockServer) -> GmailClient {
    let store = MemoryStore::default();
    store
        .save(&Credentials {
            client_secret: "shh".to_owned(),
            refresh_token: "refresh".to_owned(),
            access_token: Some(AccessToken {
                token: "access".to_owned(),
                expires_at: i64::MAX,
            }),
        })
        .unwrap();
    let endpoints = Endpoints {
        auth_url: "unused".to_owned(),
        token_url: format!("{}/token", server.uri()),
    };
    let session: Session = auth::open_session(
        Arc::new(store),
        endpoints,
        "id",
        false,
        &ScriptedConsole::default(),
    )
    .await
    .unwrap();
    GmailClient::new(session)
        .with_base_url(&server.uri())
        .with_timing(Duration::ZERO, Duration::from_millis(1))
}

fn message_json(id: &str, from: &str, subject: Option<&str>) -> serde_json::Value {
    let mut headers = vec![json!({"name": "From", "value": from})];
    if let Some(subject) = subject {
        headers.push(json!({"name": "Subject", "value": subject}));
    }
    json!({
        "id": id,
        "threadId": format!("t-{id}"),
        "labelIds": ["INBOX", "Label_7"],
        "internalDate": "1700000000000",
        "payload": {"headers": headers}
    })
}

#[tokio::test]
async fn a_full_listing_follows_every_page_and_returns_the_history_id_cursor() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/profile"))
        .and(header("authorization", "Bearer access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"historyId": "4242"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .and(query_param("pageToken", "page-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [{"id": "m3", "threadId": "t3"}]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [{"id": "m1", "threadId": "t1"}, {"id": "m2", "threadId": "t2"}],
            "nextPageToken": "page-2"
        })))
        .mount(&server)
        .await;

    let changes = client(&server)
        .await
        .changes_since(None, None)
        .await
        .unwrap();

    assert_eq!(changes.ids, ["m1", "m2", "m3"]);
    assert_eq!(changes.cursor.to_string(), "4242");
}

#[tokio::test]
async fn a_limited_listing_asks_for_only_what_it_needs_and_stops_paging() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/profile"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"historyId": "4242"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .and(query_param("pageToken", "page-2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"messages": []})))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages"))
        .and(query_param("maxResults", "2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "messages": [{"id": "m1", "threadId": "t1"}, {"id": "m2", "threadId": "t2"}],
            "nextPageToken": "page-2"
        })))
        .mount(&server)
        .await;

    let changes = client(&server)
        .await
        .changes_since(None, Some(2))
        .await
        .unwrap();

    assert_eq!(changes.ids, ["m1", "m2"]);
}

#[test]
fn the_sender_address_is_taken_from_the_last_angle_brackets() {
    let cases = [
        ("Chase <billing@email.chase.com>", "billing@email.chase.com"),
        ("\"Doe, <J>\" <j@example.com>", "j@example.com"),
        ("bare@example.com", "bare@example.com"),
        ("  padded@example.com  ", "padded@example.com"),
        ("Name < spaced@example.com >", "spaced@example.com"),
        ("", ""),
    ];

    for (from, expected) in cases {
        assert_eq!(bare_address(from), expected, "{from:?}");
    }
}

#[tokio::test]
async fn metadata_reads_the_sender_and_subject_from_the_headers() {
    let server = MockServer::start().await;
    for (id, from, subject) in [
        ("m1", "Chase <billing@email.chase.com>", Some("Statement")),
        ("m2", "bare@example.com", None),
    ] {
        Mock::given(method("GET"))
            .and(path(format!("/gmail/v1/users/me/messages/{id}")))
            .and(query_param("format", "metadata"))
            .respond_with(ResponseTemplate::new(200).set_body_json(message_json(id, from, subject)))
            .mount(&server)
            .await;
    }
    let ids = ["m1", "m2"].map(String::from);

    let metas = client(&server).await.metadata(&ids).await.unwrap();

    let read: Vec<(&str, Option<&str>)> = metas
        .iter()
        .map(|m| (m.sender_address.as_str(), m.subject.as_deref()))
        .collect();
    assert_eq!(
        read,
        [
            ("billing@email.chase.com", Some("Statement")),
            ("bare@example.com", None),
        ]
    );
}

#[tokio::test]
async fn a_message_deleted_since_listing_is_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages/gone"))
        .respond_with(ResponseTemplate::new(404).set_body_json(json!({"error": {"code": 404}})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages/here"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(message_json("here", "a@b.com", None)),
        )
        .mount(&server)
        .await;
    let ids = ["gone", "here"].map(String::from);

    let metas = client(&server).await.metadata(&ids).await.unwrap();

    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].id, "here");
}

#[tokio::test]
async fn rate_limited_requests_are_retried_until_they_succeed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages/m1"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
        .up_to_n_times(2)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages/m1"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({
            "error": {"errors": [{"reason": "userRateLimitExceeded"}]}
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/messages/m1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(message_json("m1", "a@b.com", None)))
        .mount(&server)
        .await;
    let ids = vec!["m1".to_owned()];

    let metas = client(&server).await.metadata(&ids).await.unwrap();

    assert_eq!(metas.len(), 1);
}

#[tokio::test]
async fn a_rejected_access_token_is_renewed_once_and_the_request_repeated() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "renewed", "expires_in": 3600})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/labels"))
        .and(header("authorization", "Bearer renewed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"labels": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/labels"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let labels = client(&server).await.list_labels().await.unwrap();

    assert!(labels.is_empty());
}

#[tokio::test]
async fn a_revoked_refresh_token_surfaces_as_auth_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant"})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/gmail/v1/users/me/labels"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = client(&server).await.list_labels().await.unwrap_err();

    assert!(matches!(err, ClientError::AuthExpired), "{err}");
}
