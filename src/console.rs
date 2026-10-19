//! The human at the other end of `setup` and re-consent: a browser to open and
//! a terminal to talk to. A trait so the flows can be driven without either.

use std::io::{self, BufRead, Write};

pub trait Console: Send + Sync {
    /// Open `url` in the user's browser. Best effort: callers also print the
    /// URL, so a failure here is not an error.
    fn open(&self, url: &str);

    fn say(&self, text: &str);

    /// Ask one line of input.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal cannot be read.
    fn ask(&self, prompt: &str) -> io::Result<String>;

    /// Like [`Console::ask`], but the answer is not echoed.
    ///
    /// # Errors
    ///
    /// Returns an error if the terminal cannot be read.
    fn ask_secret(&self, prompt: &str) -> io::Result<String>;
}

pub struct Terminal;

impl Console for Terminal {
    fn open(&self, url: &str) {
        // Ignored: the URL is on screen as well.
        let _ = open::that(url);
    }

    fn say(&self, text: &str) {
        println!("{text}");
    }

    fn ask(&self, prompt: &str) -> io::Result<String> {
        print!("{prompt} ");
        io::stdout().flush()?;
        let mut line = String::new();
        io::stdin().lock().read_line(&mut line)?;
        Ok(line.trim().to_owned())
    }

    fn ask_secret(&self, prompt: &str) -> io::Result<String> {
        Ok(rpassword::prompt_password(format!("{prompt} "))?
            .trim()
            .to_owned())
    }
}
