//! Gmail credentials: the OAuth consent flow, the OS keychain, and keeping an
//! access token fresh.
//!
//! The client secret, refresh token and access token live together in one
//! keychain entry; the config file only ever holds the client ID.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::console::Console;

/// `gmail.modify` for labels, `gmail.settings.basic` for filters.
pub const SCOPES: [&str; 2] = [
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/gmail.settings.basic",
];

const KEYCHAIN_SERVICE: &str = "mailscrub";
const KEYCHAIN_ENTRY: &str = "gmail";
/// Refresh this long before expiry so a request never goes out with a token
/// that dies in flight.
const EXPIRY_MARGIN_SECS: i64 = 60;
/// How long to wait for the user to finish in the browser.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(300);
/// How long a connection to the redirect listener may sit silent.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Shown whenever the refresh token no longer works.
pub const EXPIRED_MESSAGE: &str = "Gmail authorization has expired or been revoked; run `mailscrub run` from a terminal to sign in again";

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no Gmail credentials found; run `mailscrub setup`")]
    NotSetUp,
    #[error("{EXPIRED_MESSAGE}")]
    RefreshExpired,
    #[error("consent was not granted: {0}")]
    ConsentDenied(String),
    #[error("timed out waiting for consent in the browser")]
    ConsentTimedOut,
    #[error("Google rejected the token request: {0}")]
    Token(String),
    #[error("cannot reach Google: {0}")]
    Http(#[from] reqwest::Error),
    #[error("keychain: {0}")]
    Store(String),
    #[error("consent listener: {0}")]
    Io(#[from] std::io::Error),
}

/// Where Google's OAuth endpoints live; a value so tests can point elsewhere.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub auth_url: String,
    pub token_url: String,
}

impl Endpoints {
    pub fn google() -> Self {
        Self {
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
            token_url: "https://oauth2.googleapis.com/token".to_owned(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub token: String,
    /// Seconds since the Unix epoch.
    pub expires_at: i64,
}

/// Everything secret about the Gmail account. Deliberately not `Debug`.
#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub client_secret: String,
    pub refresh_token: String,
    pub access_token: Option<AccessToken>,
}

pub trait SecretStore: Send + Sync {
    /// # Errors
    ///
    /// Returns an error if the store cannot be read or holds something unreadable.
    fn load(&self) -> Result<Option<Credentials>, AuthError>;

    /// # Errors
    ///
    /// Returns an error if the store cannot be written.
    fn save(&self, credentials: &Credentials) -> Result<(), AuthError>;
}

/// The OS keychain: Keychain Services, Credential Manager or Secret Service.
pub struct Keychain;

impl Keychain {
    fn entry() -> Result<keyring::Entry, AuthError> {
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ENTRY)
            .map_err(|e| AuthError::Store(e.to_string()))
    }
}

impl SecretStore for Keychain {
    fn load(&self) -> Result<Option<Credentials>, AuthError> {
        match Self::entry()?.get_password() {
            Ok(json) => serde_json::from_str(&json)
                .map(Some)
                .map_err(|e| AuthError::Store(format!("unreadable keychain entry: {e}"))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AuthError::Store(e.to_string())),
        }
    }

    fn save(&self, credentials: &Credentials) -> Result<(), AuthError> {
        let json = serde_json::to_string(credentials)
            .map_err(|e| AuthError::Store(format!("cannot encode credentials: {e}")))?;
        Self::entry()?
            .set_password(&json)
            .map_err(|e| AuthError::Store(e.to_string()))
    }
}

/// A store that forgets everything when dropped, for tests.
#[derive(Default)]
pub struct MemoryStore {
    credentials: Mutex<Option<Credentials>>,
}

impl SecretStore for MemoryStore {
    fn load(&self) -> Result<Option<Credentials>, AuthError> {
        Ok(self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone())
    }

    fn save(&self, credentials: &Credentials) -> Result<(), AuthError> {
        *self
            .credentials
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(credentials.clone());
        Ok(())
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs().cast_signed())
}

fn random_url_safe(bytes: usize) -> Result<String, AuthError> {
    let mut buf = vec![0_u8; bytes];
    getrandom::fill(&mut buf).map_err(|e| AuthError::Store(format!("no randomness: {e}")))?;
    Ok(URL_SAFE_NO_PAD.encode(buf))
}

async fn token_request(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    form: &[(&str, &str)],
) -> Result<TokenResponse, AuthError> {
    let response = http.post(&endpoints.token_url).form(form).send().await?;
    if response.status().is_success() {
        return Ok(response.json().await?);
    }
    let text = response.text().await?;
    match serde_json::from_str::<TokenError>(&text) {
        Ok(e) if e.error == "invalid_grant" => Err(AuthError::RefreshExpired),
        Ok(e) => Err(AuthError::Token(match e.error_description {
            Some(description) => format!("{}: {description}", e.error),
            None => e.error,
        })),
        Err(_) => Err(AuthError::Token(text)),
    }
}

fn access_from(response: &TokenResponse) -> AccessToken {
    AccessToken {
        token: response.access_token.clone(),
        expires_at: now() + response.expires_in,
    }
}

/// Run the browser consent flow (authorization code with PKCE on a loopback
/// redirect) and return the resulting credentials.
///
/// # Errors
///
/// Returns an error if the user denies consent, does not finish in time, or
/// Google rejects the code exchange.
pub async fn consent(
    http: &reqwest::Client,
    endpoints: &Endpoints,
    client_id: &str,
    client_secret: &str,
    console: &dyn Console,
) -> Result<Credentials, AuthError> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect_uri = format!("http://127.0.0.1:{}", listener.local_addr()?.port());
    let verifier = random_url_safe(32)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let state = random_url_safe(16)?;

    let url = Url::parse_with_params(
        &endpoints.auth_url,
        [
            ("client_id", client_id),
            ("redirect_uri", &redirect_uri),
            ("response_type", "code"),
            ("scope", &SCOPES.join(" ")),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            // Without these Google may omit the refresh token on a re-consent.
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map_err(|e| AuthError::Token(format!("bad authorization URL: {e}")))?;
    console.say(&format!(
        "Opening your browser to grant access. If nothing opens, visit:\n{url}"
    ));
    console.open(url.as_str());

    let code = tokio::time::timeout(CONSENT_TIMEOUT, wait_for_redirect(&listener, &state))
        .await
        .map_err(|_| AuthError::ConsentTimedOut)??;

    let response = token_request(
        http,
        endpoints,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("redirect_uri", &redirect_uri),
            ("code_verifier", &verifier),
        ],
    )
    .await
    .map_err(|e| match e {
        // `invalid_grant` means an expired refresh token elsewhere; here it is a bad code.
        AuthError::RefreshExpired => AuthError::Token(
            "the authorization code was rejected (expired or already used); try again".to_owned(),
        ),
        other => other,
    })?;
    let refresh_token = response.refresh_token.clone().ok_or_else(|| {
        AuthError::Token(
            "Google returned no refresh token; revoke the app's access and retry".to_owned(),
        )
    })?;
    Ok(Credentials {
        client_secret: client_secret.to_owned(),
        refresh_token,
        access_token: Some(access_from(&response)),
    })
}

/// Serve the loopback redirect until a request carries the outcome of consent.
/// Each connection is handled on its own task, because browsers open
/// speculative connections that never send anything.
async fn wait_for_redirect(listener: &TcpListener, state: &str) -> Result<String, AuthError> {
    let (outcomes, mut decided) = mpsc::unbounded_channel();
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                tokio::spawn(serve_redirect(stream, state.to_owned(), outcomes.clone()));
            }
            Some(outcome) = decided.recv() => return outcome,
        }
    }
}

/// Answer one connection. Anything that is not the consent outcome (a favicon
/// fetch, a port scan) gets a 404 and is not reported.
async fn serve_redirect(
    mut stream: TcpStream,
    state: String,
    outcomes: mpsc::UnboundedSender<Result<String, AuthError>>,
) {
    let mut request_line = String::new();
    let read = tokio::time::timeout(
        REQUEST_TIMEOUT,
        BufReader::new(&mut stream).read_line(&mut request_line),
    )
    .await;
    if !matches!(read, Ok(Ok(_))) {
        return;
    }
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    let params: Vec<(String, String)> = Url::parse(&format!("http://127.0.0.1{target}"))
        .map(|u| u.query_pairs().into_owned().collect())
        .unwrap_or_default();
    let param = |name: &str| {
        params
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };

    let outcome = match (param("code"), param("error")) {
        (_, Some(error)) => Some(Err(AuthError::ConsentDenied(error.to_owned()))),
        (Some(code), None) if param("state") == Some(state.as_str()) => Some(Ok(code.to_owned())),
        (Some(_), None) => Some(Err(AuthError::ConsentDenied(
            "the redirect did not match this sign-in attempt".to_owned(),
        ))),
        (None, None) => None,
    };
    let (status, body) = match &outcome {
        Some(Ok(_)) => (
            "200 OK",
            "Mail Scrub is authorised. You can close this tab.",
        ),
        Some(Err(_)) => (
            "400 Bad Request",
            "Authorisation failed. Return to the terminal.",
        ),
        None => ("404 Not Found", ""),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // The browser may already have gone away; the outcome is what matters.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    if let Some(outcome) = outcome {
        // The receiver is gone only once consent has already been decided.
        let _ = outcomes.send(outcome);
    }
}

/// Valid Gmail credentials for the length of a Run.
pub struct Session {
    http: reqwest::Client,
    endpoints: Endpoints,
    client_id: String,
    store: Arc<dyn SecretStore>,
    // Held across a refresh so concurrent requests refresh once, not each.
    credentials: tokio::sync::Mutex<Credentials>,
}

impl Session {
    /// A token good for at least the next minute, refreshing if needed.
    ///
    /// # Errors
    ///
    /// Returns [`AuthError::RefreshExpired`] if Google no longer honours the
    /// refresh token, or another error if the refresh fails.
    pub async fn access_token(&self) -> Result<String, AuthError> {
        let mut credentials = self.credentials.lock().await;
        if let Some(token) = &credentials.access_token
            && token.expires_at > now() + EXPIRY_MARGIN_SECS
        {
            return Ok(token.token.clone());
        }
        self.refresh(&mut credentials).await
    }

    /// Replace the credentials after a fresh consent.
    async fn adopt(&self, credentials: Credentials) {
        *self.credentials.lock().await = credentials;
    }

    /// Discard the cached token and get a new one, for when the API says the
    /// token we hold was rejected.
    ///
    /// # Errors
    ///
    /// As [`Session::access_token`].
    pub async fn renew(&self) -> Result<String, AuthError> {
        let mut credentials = self.credentials.lock().await;
        self.refresh(&mut credentials).await
    }

    async fn refresh(&self, credentials: &mut Credentials) -> Result<String, AuthError> {
        let response = token_request(
            &self.http,
            &self.endpoints,
            &[
                ("grant_type", "refresh_token"),
                ("refresh_token", &credentials.refresh_token),
                ("client_id", &self.client_id),
                ("client_secret", &credentials.client_secret),
            ],
        )
        .await?;
        let access = access_from(&response);
        let token = access.token.clone();
        credentials.access_token = Some(access);
        if let Some(rotated) = response.refresh_token {
            credentials.refresh_token = rotated;
        }
        self.store.save(credentials)?;
        Ok(token)
    }
}

/// Load the stored credentials and make sure they work. If the refresh token
/// has expired, sign in again in the browser when `interactive`, otherwise
/// fail so a scheduled Run exits with a clear message instead of hanging.
///
/// # Errors
///
/// Returns [`AuthError::NotSetUp`] with nothing stored,
/// [`AuthError::RefreshExpired`] when re-consent is needed but impossible, or
/// the error from a failed refresh or consent.
pub async fn open_session(
    store: Arc<dyn SecretStore>,
    endpoints: Endpoints,
    client_id: &str,
    interactive: bool,
    console: &dyn Console,
) -> Result<Session, AuthError> {
    let credentials = store.load()?.ok_or(AuthError::NotSetUp)?;
    let http = reqwest::Client::new();
    let session = Session {
        http: http.clone(),
        endpoints: endpoints.clone(),
        client_id: client_id.to_owned(),
        store: store.clone(),
        credentials: tokio::sync::Mutex::new(credentials.clone()),
    };
    match session.access_token().await {
        Ok(_) => Ok(session),
        Err(AuthError::RefreshExpired) if interactive => {
            console.say("Gmail authorization has expired. Signing in again.");
            let renewed = consent(
                &http,
                &endpoints,
                client_id,
                &credentials.client_secret,
                console,
            )
            .await?;
            store.save(&renewed)?;
            session.adopt(renewed).await;
            Ok(session)
        }
        Err(e) => Err(e),
    }
}
