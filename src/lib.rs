//! Mail Scrub: classifies Gmail messages from metadata and applies labels.
//!
//! All provider access goes through [`client::EmailClient`]; a Run is wired up
//! in [`run`]; [`cli`] is the command-line front end.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![warn(clippy::pedantic)]

pub mod classifier;
pub mod cli;
pub mod client;
pub mod config;
pub mod db;
pub mod run;
