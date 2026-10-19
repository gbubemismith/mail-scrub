//! [`EmailClient`] for Gmail over its REST API.
//!
//! Requests share one semaphore so a Run stays under Gmail's per-user quota
//! however many callers fan out, and rate-limit responses are retried with
//! backoff rather than failing the Run.

use std::fmt;
use std::future::Future;
use std::num::ParseIntError;
use std::str::FromStr;
use std::time::Duration;

use futures_util::future::try_join_all;
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::sync::Semaphore;
use tokio::time::Instant;

use super::{Changes, ClientError, EmailClient, Filter, Label, LabelId, MessageId, MessageMeta};
use crate::auth::{AuthError, Session};

const BASE_URL: &str = "https://gmail.googleapis.com";
/// Gmail allows roughly ten concurrent requests per user, and `messages.get`
/// costs 5 of the 250 quota units available each second.
const MAX_IN_FLIGHT: usize = 8;
const PAGE_SIZE: usize = 500;
/// Request starts are spaced this far apart: 40 a second is 200 of the 250
/// units, even if every request were a `messages.get`.
const REQUEST_INTERVAL: Duration = Duration::from_millis(25);
const MAX_ATTEMPTS: u32 = 8;
const MAX_BACKOFF: Duration = Duration::from_secs(32);

/// Position in Gmail's change history: the mailbox's `historyId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GmailCursor(pub u64);

impl fmt::Display for GmailCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for GmailCursor {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(GmailCursor)
    }
}

/// The Gmail search query a filter matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GmailCriteria {
    pub query: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GmailError {
    #[error("cannot reach Gmail: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Gmail returned {status}: {body}")]
    Status { status: u16, body: String },
    #[error("unexpected response from Gmail: {0}")]
    Malformed(String),
}

impl From<GmailError> for ClientError {
    fn from(e: GmailError) -> Self {
        ClientError::Other(Box::new(e))
    }
}

impl From<reqwest::Error> for ClientError {
    fn from(e: reqwest::Error) -> Self {
        GmailError::Http(e).into()
    }
}

fn auth_failure(e: AuthError) -> ClientError {
    match e {
        AuthError::RefreshExpired => ClientError::AuthExpired,
        other => ClientError::Other(Box::new(other)),
    }
}

pub struct GmailClient {
    http: reqwest::Client,
    session: Session,
    base_url: String,
    in_flight: Semaphore,
    /// When the next request may start.
    next_slot: tokio::sync::Mutex<Instant>,
    request_interval: Duration,
    retry_delay: Duration,
}

impl GmailClient {
    pub fn new(session: Session) -> Self {
        Self {
            http: reqwest::Client::new(),
            session,
            base_url: BASE_URL.to_owned(),
            in_flight: Semaphore::new(MAX_IN_FLIGHT),
            next_slot: tokio::sync::Mutex::new(Instant::now()),
            request_interval: REQUEST_INTERVAL,
            retry_delay: Duration::from_secs(1),
        }
    }

    /// Talk to another server, such as a test double.
    pub fn with_base_url(mut self, base_url: &str) -> Self {
        base_url
            .trim_end_matches('/')
            .clone_into(&mut self.base_url);
        self
    }

    /// Override the timing: the gap between request starts, and the first
    /// backoff after a rate-limit response (which doubles each retry).
    pub fn with_timing(mut self, request_interval: Duration, retry_delay: Duration) -> Self {
        self.request_interval = request_interval;
        self.retry_delay = retry_delay;
        self
    }

    /// Wait for this request's slot, so the rate stays under quota however
    /// many callers fan out.
    async fn pace(&self) {
        let slot = {
            let mut next = self.next_slot.lock().await;
            let slot = (*next).max(Instant::now());
            *next = slot + self.request_interval;
            slot
        };
        tokio::time::sleep_until(slot).await;
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        self.http
            .request(method, format!("{}/gmail/v1/users/me{path}", self.base_url))
    }

    /// Send the request built by `build`, retrying rate limits and server
    /// errors, and renewing the access token once if Gmail rejects it.
    async fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Response, ClientError> {
        let mut renewed = false;
        for attempt in 0..MAX_ATTEMPTS {
            self.pace().await;
            let token = self.session.access_token().await.map_err(auth_failure)?;
            let response = {
                // Never closed, so acquiring cannot fail; the permit is held
                // only while the request is in flight, not while backing off.
                let _permit = self.in_flight.acquire().await;
                build().bearer_auth(token).send().await?
            };
            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            if status == StatusCode::UNAUTHORIZED && !renewed {
                renewed = true;
                self.session.renew().await.map_err(auth_failure)?;
                continue;
            }
            let wait = retry_after(&response)
                .unwrap_or_else(|| (self.retry_delay * 2_u32.pow(attempt)).min(MAX_BACKOFF));
            let body = response.text().await?;
            if is_retryable(status, &body) && attempt + 1 < MAX_ATTEMPTS {
                tokio::time::sleep(wait).await;
                continue;
            }
            if status == StatusCode::NOT_FOUND {
                return Err(ClientError::NotFound(body));
            }
            return Err(GmailError::Status {
                status: status.as_u16(),
                body,
            }
            .into());
        }
        // The last attempt always returns above.
        Err(GmailError::Malformed("retry loop ended without a result".to_owned()).into())
    }

    async fn json<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> RequestBuilder,
    ) -> Result<T, ClientError> {
        let response = self.send(build).await?;
        Ok(response.json().await?)
    }

    async fn message_metadata(&self, id: &str) -> Result<Option<MessageMeta>, ClientError> {
        let result: Result<RawMessage, ClientError> = self
            .json(|| {
                self.request(Method::GET, &format!("/messages/{id}"))
                    .query(&[
                        ("format", "metadata"),
                        ("metadataHeaders", "From"),
                        ("metadataHeaders", "Subject"),
                        (
                            "fields",
                            "id,threadId,labelIds,internalDate,payload/headers",
                        ),
                    ])
            })
            .await;
        match result {
            Ok(raw) => Ok(Some(raw.into_meta())),
            // Deleted between listing and fetching: nothing left to label.
            Err(ClientError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Rate limits arrive as 429, or as 403 with a rate-limit reason; 5xx are
/// Google's own hiccups.
fn is_retryable(status: StatusCode, body: &str) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status.is_server_error()
        || (status == StatusCode::FORBIDDEN && body.contains("ateLimitExceeded"))
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()
        .map(Duration::from_secs)
}

#[derive(Deserialize)]
struct Profile {
    #[serde(rename = "historyId")]
    history_id: String,
}

#[derive(Deserialize)]
struct MessageList {
    #[serde(default)]
    messages: Vec<IdOnly>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct IdOnly {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMessage {
    id: String,
    #[serde(default)]
    thread_id: String,
    #[serde(default)]
    label_ids: Vec<String>,
    internal_date: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(default)]
    headers: Vec<Header>,
}

#[derive(Deserialize)]
struct Header {
    name: String,
    value: String,
}

impl RawMessage {
    fn into_meta(self) -> MessageMeta {
        let headers = self.payload.map(|p| p.headers).unwrap_or_default();
        let header = |name: &str| {
            headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case(name))
                .map(|h| h.value.as_str())
        };
        MessageMeta {
            sender_address: header("From").map(bare_address).unwrap_or_default(),
            subject: header("Subject").map(str::to_owned),
            internal_date: self
                .internal_date
                .and_then(|ms| ms.parse::<i64>().ok())
                .map_or(0, |ms| ms / 1000),
            id: self.id,
            thread_id: self.thread_id,
            label_ids: self.label_ids,
        }
    }
}

/// `Chase <billing@email.chase.com>` → `billing@email.chase.com`. The last
/// angle-bracket pair wins so a display name containing `<` cannot confuse it.
pub fn bare_address(from: &str) -> String {
    let inner = match (from.rfind('<'), from.rfind('>')) {
        (Some(open), Some(close)) if open < close => &from[open + 1..close],
        _ => from,
    };
    inner.trim().to_owned()
}

#[derive(Deserialize)]
struct LabelList {
    #[serde(default)]
    labels: Vec<RawLabel>,
}

#[derive(Deserialize)]
struct RawLabel {
    id: String,
    name: String,
}

impl From<RawLabel> for Label {
    fn from(l: RawLabel) -> Self {
        Label {
            id: l.id,
            name: l.name,
        }
    }
}

impl EmailClient for GmailClient {
    type Cursor = GmailCursor;
    type FilterCriteria = GmailCriteria;

    /// Always walks the whole mailbox, whatever `cursor` says. The returned
    /// cursor is the history id from before the walk starts, so anything
    /// that arrives during it is still ahead of the cursor.
    async fn changes_since(
        &self,
        _cursor: Option<&GmailCursor>,
        limit: Option<usize>,
    ) -> Result<Changes<GmailCursor>, ClientError> {
        let profile: Profile = self.json(|| self.request(Method::GET, "/profile")).await?;
        let cursor = profile
            .history_id
            .parse()
            .map_err(|_| GmailError::Malformed(format!("history id {:?}", profile.history_id)))?;

        // Spam and Trash are left out (`includeSpamTrash` defaults to false):
        // nothing there is worth labelling. Gmail lists newest first.
        let limit = limit.unwrap_or(usize::MAX);
        let mut ids = Vec::new();
        let mut page_token: Option<String> = None;
        while ids.len() < limit {
            let page_size = PAGE_SIZE.min(limit - ids.len()).to_string();
            let page: MessageList = self
                .json(|| {
                    let mut request = self.request(Method::GET, "/messages").query(&[
                        ("maxResults", page_size.as_str()),
                        ("fields", "messages/id,nextPageToken"),
                    ]);
                    if let Some(token) = &page_token {
                        request = request.query(&[("pageToken", token)]);
                    }
                    request
                })
                .await?;
            ids.extend(page.messages.into_iter().map(|m| m.id));
            match page.next_page_token {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }
        ids.truncate(limit);
        Ok(Changes {
            ids,
            cursor: GmailCursor(cursor),
        })
    }

    async fn metadata(&self, ids: &[MessageId]) -> Result<Vec<MessageMeta>, ClientError> {
        let fetched = try_join_all(ids.iter().map(|id| self.message_metadata(id))).await?;
        Ok(fetched.into_iter().flatten().collect())
    }

    fn body(&self, _id: &str) -> impl Future<Output = Result<String, ClientError>> + Send {
        std::future::ready(Err(ClientError::Unsupported))
    }

    async fn set_labels(
        &self,
        id: &str,
        add: &[LabelId],
        remove: &[LabelId],
    ) -> Result<(), ClientError> {
        let body = serde_json::json!({"addLabelIds": add, "removeLabelIds": remove});
        self.send(|| {
            self.request(Method::POST, &format!("/messages/{id}/modify"))
                .json(&body)
        })
        .await?;
        Ok(())
    }

    fn list_filters(
        &self,
    ) -> impl Future<Output = Result<Vec<Filter<GmailCriteria>>, ClientError>> + Send {
        std::future::ready(Err(ClientError::Unsupported))
    }

    fn create_filter(
        &self,
        _criteria: GmailCriteria,
        _add_label_id: &str,
    ) -> impl Future<Output = Result<Filter<GmailCriteria>, ClientError>> + Send {
        std::future::ready(Err(ClientError::Unsupported))
    }

    fn delete_filter(&self, _id: &str) -> impl Future<Output = Result<(), ClientError>> + Send {
        std::future::ready(Err(ClientError::Unsupported))
    }

    async fn list_labels(&self) -> Result<Vec<Label>, ClientError> {
        let list: LabelList = self.json(|| self.request(Method::GET, "/labels")).await?;
        Ok(list.labels.into_iter().map(Label::from).collect())
    }

    async fn create_label(&self, name: &str) -> Result<Label, ClientError> {
        let body = serde_json::json!({
            "name": name,
            "labelListVisibility": "labelShow",
            "messageListVisibility": "show",
        });
        let label: RawLabel = self
            .json(|| self.request(Method::POST, "/labels").json(&body))
            .await?;
        Ok(label.into())
    }
}
