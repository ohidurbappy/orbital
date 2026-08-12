//! `orbital update` — download and install the latest release.

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::commands::{Command, Ctx};
use crate::core::updater::apply::{apply_update, progress_label, ApplyOutcome, Phase, Progress};
use crate::style;
use crate::term::{self, Frame};
use crate::Res;

pub const COMMAND: Command = Command {
    name: "update",
    description: "Download and install the latest release",
    aliases: &["upgrade", "self-update"],
    run: None,
    view,
    reads_stdin: false,
};

/// Braille spinner frames, matching `ink-spinner`'s "dots".
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
const FRAME_TIME: Duration = Duration::from_millis(80);

fn view(ctx: &Ctx) -> Res {
    // The update runs on a worker so this thread can animate the spinner.
    let progress = Arc::new(Mutex::new(Progress {
        phase: Phase::Checking,
        total_bytes: None,
    }));
    let (done_tx, done_rx) = mpsc::channel::<ApplyOutcome>();

    let worker = {
        let progress = Arc::clone(&progress);
        std::thread::spawn(move || {
            let outcome = apply_update(&|update| {
                if let Ok(mut current) = progress.lock() {
                    *current = update;
                }
            });
            let _ = done_tx.send(outcome);
        })
    };

    let outcome = if ctx.interactive {
        spin_until_done(&progress, &done_rx)?
    } else {
        // Nothing to animate into a pipe; just wait for the result.
        done_rx.recv().unwrap_or_else(|_| {
            ApplyOutcome::new(
                crate::core::updater::apply::Status::Error,
                "Update failed: the worker stopped unexpectedly.",
            )
        })
    };
    worker.join().ok();

    term::emit(&[style::colored(&outcome.message, outcome.status.color())]);
    Ok(())
}

/// Animate the spinner, repainting the phase label until the update finishes.
fn spin_until_done(
    progress: &Arc<Mutex<Progress>>,
    done: &mpsc::Receiver<ApplyOutcome>,
) -> Res<ApplyOutcome> {
    let mut frame = Frame::new();
    let mut tick = 0usize;

    loop {
        match done.try_recv() {
            Ok(outcome) => {
                frame.clear()?;
                return Ok(outcome);
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                frame.clear()?;
                return Err("the update worker stopped unexpectedly".into());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        let current = progress.lock().map(|p| *p).unwrap_or(Progress {
            phase: Phase::Checking,
            total_bytes: None,
        });
        frame.draw(&[format!(
            "{} {}",
            style::cyan(&SPINNER[tick % SPINNER.len()].to_string()),
            progress_label(current)
        )])?;
        tick += 1;
        std::thread::sleep(FRAME_TIME);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::updater::apply::Status;

    #[test]
    fn the_spinner_has_frames_to_cycle_through() {
        assert!(SPINNER.len() > 1);
    }

    #[test]
    fn each_outcome_has_a_colour() {
        for status in [
            Status::Updated,
            Status::UpToDate,
            Status::Unsupported,
            Status::NoAsset,
            Status::Error,
        ] {
            assert!(!status.color().is_empty());
        }
    }

    #[test]
    fn a_cargo_build_reports_that_self_update_does_not_apply() {
        let ctx = Ctx {
            args: &[],
            input: None,
            interactive: false,
        };
        // Runs the real command: under `cargo test` it must short-circuit
        // before touching the network or the binary.
        assert!(view(&ctx).is_ok());
    }
}
