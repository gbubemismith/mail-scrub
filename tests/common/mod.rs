//! Shared test doubles for the auth, setup and Gmail tests.

#![allow(dead_code, reason = "each test binary uses a different subset")]

use std::collections::VecDeque;
use std::io;
use std::sync::Mutex;

use mail_scrub::console::Console;
use reqwest::Url;

/// A console that answers from a script and plays the browser: when asked to
/// open a consent URL it follows the redirect back to the loopback listener.
#[derive(Default)]
pub struct ScriptedConsole {
    opened: Mutex<Vec<String>>,
    said: Mutex<Vec<String>>,
    answers: Mutex<VecDeque<String>>,
    secrets: Mutex<VecDeque<String>>,
    /// The `code` the "browser" returns; `None` simulates the user denying consent.
    pub grant_code: Option<String>,
    /// Open a connection to the redirect port that never sends anything, as
    /// browsers do speculatively, before the real redirect.
    pub preconnect_silently: bool,
}

impl ScriptedConsole {
    pub fn granting(code: &str) -> Self {
        Self {
            grant_code: Some(code.to_owned()),
            ..Self::default()
        }
    }

    pub fn preconnecting_silently(mut self) -> Self {
        self.preconnect_silently = true;
        self
    }

    pub fn answering(self, answer: &str) -> Self {
        self.answers.lock().unwrap().push_back(answer.to_owned());
        self
    }

    pub fn answering_secret(self, secret: &str) -> Self {
        self.secrets.lock().unwrap().push_back(secret.to_owned());
        self
    }

    pub fn opened(&self) -> Vec<String> {
        self.opened.lock().unwrap().clone()
    }

    pub fn said(&self) -> String {
        self.said.lock().unwrap().join("\n")
    }
}

impl Console for ScriptedConsole {
    fn open(&self, url: &str) {
        self.opened.lock().unwrap().push(url.to_owned());
        let Ok(url) = Url::parse(url) else { return };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.into_owned())
        };
        let (Some(redirect), Some(state)) = (param("redirect_uri"), param("state")) else {
            return;
        };
        let mut back = Url::parse(&redirect).unwrap();
        if self.preconnect_silently {
            let addr = format!("127.0.0.1:{}", back.port().unwrap());
            let silent = std::net::TcpStream::connect(addr).unwrap();
            // Held open for the rest of the process, i.e. the test.
            std::mem::forget(silent);
        }
        match &self.grant_code {
            Some(code) => back.query_pairs_mut().append_pair("code", code),
            None => back.query_pairs_mut().append_pair("error", "access_denied"),
        };
        back.query_pairs_mut().append_pair("state", &state);
        tokio::spawn(async move { reqwest::get(back).await });
    }

    fn say(&self, text: &str) {
        self.said.lock().unwrap().push(text.to_owned());
    }

    fn ask(&self, _prompt: &str) -> io::Result<String> {
        Ok(self.answers.lock().unwrap().pop_front().unwrap_or_default())
    }

    fn ask_secret(&self, _prompt: &str) -> io::Result<String> {
        Ok(self.secrets.lock().unwrap().pop_front().unwrap_or_default())
    }
}
