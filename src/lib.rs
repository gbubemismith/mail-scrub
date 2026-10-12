//! Mail Scrub: classifies Gmail messages from metadata and applies labels.
//!
//! All provider access goes through [`client::EmailClient`]; a Run is wired up
//! in [`run`].

pub mod classifier;
pub mod client;
pub mod config;
pub mod db;
pub mod run;
