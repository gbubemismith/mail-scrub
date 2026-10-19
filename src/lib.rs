//! Mail Scrub: classifies Gmail messages from metadata and applies labels.
//!
//! All provider access goes through [`client::EmailClient`]; a Run is wired up
//! in [`run`]; [`auth`] and [`setup`] get and keep Gmail credentials; [`cli`]
//! is the command-line front end.

#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![warn(clippy::pedantic)]
#![allow(
    clippy::must_use_candidate,
    clippy::return_self_not_must_use,
    reason = "not worth an attribute on every pure function"
)]

pub mod apply;
pub mod auth;
pub mod classifier;
pub mod cli;
pub mod client;
pub mod config;
pub mod console;
pub mod db;
pub mod progress;
pub mod run;
pub mod setup;
