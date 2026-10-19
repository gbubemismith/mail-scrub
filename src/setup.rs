//! `mailscrub setup`: walk the user through creating their own Google Cloud
//! project and desktop OAuth client, then run consent and store the result.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::auth::{self, AuthError, Endpoints, SecretStore};
use crate::config::{self, ConfigError};
use crate::console::Console;

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot read your answer: {0}")]
    Console(#[from] std::io::Error),
    #[error("the {0} cannot be empty")]
    Empty(&'static str),
    #[error("cannot read {}: {source}", path.display())]
    ClientFileRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{} is not a usable OAuth client file: {reason}", path.display())]
    ClientFileInvalid { path: PathBuf, reason: String },
    #[error(
        "{} is a Web client. Create an OAuth client of type \"Desktop app\" instead and download that one.",
        path.display()
    )]
    WebClient { path: PathBuf },
}

const ENABLE_GMAIL_URL: &str =
    "https://console.cloud.google.com/flows/enableapi?apiid=gmail.googleapis.com";
const TOTAL_STEPS: usize = 3;

/// The OAuth client Google lets you download for a Desktop app.
#[derive(Debug, Deserialize)]
struct ClientFile {
    installed: Option<InstalledClient>,
    web: Option<serde::de::IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct InstalledClient {
    client_id: String,
    client_secret: String,
    project_id: String,
}

/// Run the whole guided setup. Nothing is stored unless consent succeeds.
///
/// `downloads` is where the browser saves the client JSON; it is searched
/// for the file so the user does not have to type a path.
///
/// # Errors
///
/// Returns an error if the terminal cannot be read, an answer is empty, the
/// client file is unreadable or not a Desktop client, consent fails, or the
/// keychain or config file cannot be written.
pub async fn run(
    console: &dyn Console,
    config_path: &Path,
    downloads: &Path,
    store: &dyn SecretStore,
    endpoints: &Endpoints,
) -> Result<(), SetupError> {
    console.say(
        "Mail Scrub uses your own Google Cloud project, so no one else holds access to your mail.\n\
         I will open each page you need. Follow the instruction, then come back here.",
    );

    step(
        console,
        1,
        "Create a project and enable the Gmail API.",
        ENABLE_GMAIL_URL,
    )?;
    let project_id = ask_project_id(console)?;

    step(
        console,
        2,
        "Click Get started: enter an app name and your email, choose External.\n\
         Then open Branding: set a home page and privacy policy link (any https URL, \
         e.g. your GitHub profile), add its domain under Authorized domains, leave the \
         logo empty, and Save.\n\
         Then open Audience in the left menu and click Publish app.",
        &format!("https://console.cloud.google.com/auth/overview/create?project={project_id}"),
    )?;
    step(
        console,
        3,
        "Create an OAuth client of type \"Desktop app\", then click Download JSON.",
        &format!("https://console.cloud.google.com/auth/clients/create?project={project_id}"),
    )?;

    let (client_path, client) = ask_client_file(console, downloads, &project_id)?;
    console.say(&format!("Using {}", client_path.display()));
    if client.project_id != project_id {
        console.say(&format!(
            "warning: that file is for project {}, not {project_id}. Using the file.",
            client.project_id
        ));
    }

    console.say(
        "\nGoogle will warn that it hasn't verified this app, because the app is yours. \
         Click Advanced, then \"Go to\" followed by the app name you chose.",
    );
    let credentials = auth::consent(
        &reqwest::Client::new(),
        endpoints,
        &client.client_id,
        &client.client_secret,
        console,
    )
    .await?;
    store.save(&credentials)?;
    config::set_client_id(config_path, &client.client_id)?;

    console.say(&format!(
        "\nDone. Tokens and the client secret are in your keychain; the client ID is in {}.\n\
         You can delete the downloaded {} now.",
        config_path.display(),
        client_path.display()
    ));
    Ok(())
}

fn step(
    console: &dyn Console,
    number: usize,
    instructions: &str,
    url: &str,
) -> Result<(), SetupError> {
    console.say(&format!(
        "\nStep {number} of {TOTAL_STEPS}: {instructions}\n{url}"
    ));
    console.open(url);
    console.ask("Press Enter when done.")?;
    Ok(())
}

fn ask_project_id(console: &dyn Console) -> Result<String, SetupError> {
    loop {
        let answer = non_empty(
            &console.ask("Project ID (in the page URL or the project picker):")?,
            "project ID",
        )?;
        match project_id_problem(&answer) {
            None => return Ok(answer),
            Some(problem) => console.say(&format!(
                "\"{answer}\" is not a project ID: it {problem}. Use the ID, not the project name."
            )),
        }
    }
}

/// Google Cloud's documented project ID rules.
fn project_id_problem(id: &str) -> Option<&'static str> {
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        Some("may contain only lowercase letters, digits and hyphens")
    } else if !id.starts_with(|c: char| c.is_ascii_lowercase()) {
        Some("must start with a lowercase letter")
    } else if id.ends_with('-') {
        Some("must not end with a hyphen")
    } else if !(6..=30).contains(&id.len()) {
        Some("must be 6 to 30 characters long")
    } else {
        None
    }
}

fn ask_client_file(
    console: &dyn Console,
    downloads: &Path,
    project_id: &str,
) -> Result<(PathBuf, InstalledClient), SetupError> {
    let found = find_client_file(downloads, project_id);
    let answer = match &found {
        Some(path) => console.ask(&format!(
            "Path to the downloaded client JSON (Enter to use {}):",
            path.display()
        ))?,
        None => console.ask("Path to the downloaded client JSON:")?,
    };
    let answer = answer.trim();
    let path = match (answer.is_empty(), found) {
        (true, Some(found)) => found,
        (true, None) => return Err(SetupError::Empty("client file path")),
        (false, _) => PathBuf::from(answer),
    };
    let client = read_client_file(&path)?;
    Ok((path, client))
}

/// The newest `client_secret_*.json` in `downloads` for `project_id`.
/// Matching on project rather than taking the newest file rules out stale
/// downloads from earlier attempts.
fn find_client_file(downloads: &Path, project_id: &str) -> Option<PathBuf> {
    std::fs::read_dir(downloads)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            let named = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("client_secret_"));
            let json = path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
            named && json
        })
        .filter(|path| read_client_file(path).is_ok_and(|c| c.project_id == project_id))
        .max_by_key(|path| {
            let modified = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (modified, path.clone())
        })
}

fn read_client_file(path: &Path) -> Result<InstalledClient, SetupError> {
    let text = std::fs::read_to_string(path).map_err(|source| SetupError::ClientFileRead {
        path: path.to_owned(),
        source,
    })?;
    let file: ClientFile =
        serde_json::from_str(&text).map_err(|e| SetupError::ClientFileInvalid {
            path: path.to_owned(),
            reason: e.to_string(),
        })?;
    match (file.installed, file.web) {
        (Some(client), _) => Ok(client),
        (None, Some(_)) => Err(SetupError::WebClient {
            path: path.to_owned(),
        }),
        (None, None) => Err(SetupError::ClientFileInvalid {
            path: path.to_owned(),
            reason: "no \"installed\" section".to_owned(),
        }),
    }
}

fn non_empty(answer: &str, what: &'static str) -> Result<String, SetupError> {
    let answer = answer.trim();
    if answer.is_empty() {
        Err(SetupError::Empty(what))
    } else {
        Ok(answer.to_owned())
    }
}
