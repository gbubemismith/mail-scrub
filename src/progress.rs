//! How far a Run has got: the events a Run sends, and drawing them on a
//! terminal.

use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// The client has started listing which messages to visit.
    Listing,
    /// This many messages will be visited.
    Listed(usize),
    /// This many messages have been decided and applied, or planned on a dry
    /// run. Can finish short of the listed total when a message vanishes
    /// mid-Run.
    Processed(usize),
}

/// Unbounded so a slow or stalled terminal never pushes back on the Run.
pub type ProgressSender = mpsc::UnboundedSender<Progress>;

pub(crate) fn report(progress: &ProgressSender, event: Progress) {
    // A closed receiver means nobody is watching, which must not fail the Run.
    let _ = progress.send(event);
}

/// Draw progress on one stderr line, redrawn in place, and end the line once
/// the Run closes the channel. Stderr so stdout stays the plan and summary.
pub async fn print_terminal(mut events: mpsc::UnboundedReceiver<Progress>) {
    let mut total = 0;
    let mut drawn = false;
    while let Some(event) = events.recv().await {
        let text = match event {
            Progress::Listing => "Listing messages…".to_owned(),
            Progress::Listed(n) => {
                total = n;
                format!("Found {n} messages")
            }
            Progress::Processed(done) => format!("Processed {done}/{total} messages"),
        };
        // Clear first: the new text can be shorter than the old.
        eprint!("\r\x1b[2K{text}");
        drawn = true;
    }
    if drawn {
        eprintln!();
    }
}
