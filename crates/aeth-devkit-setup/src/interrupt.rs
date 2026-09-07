//! Ctrl-C during a wet run. Installing a handler replaces the default kill, so the handler
//! must choose: while [`crate::vscode::session::wait_for`] polls VS Code it only flags the
//! interrupt (the question goes back to the terminal); during a file write it defers the
//! exit until the write is done, so no file is ever torn; anywhere else it exits at once,
//! as an unhandled Ctrl-C would. State held only in memory (the user's edits to managed
//! files while HEAD content sits on disk, see `commit::stage_clean_base`) is lost then, by
//! decision: a rerun re-standardises, but a half-written file needs a hand.
//!
//! A guard covers only devkit's own writes: the console delivers the Ctrl-C to every
//! child too, so git's lockfile-and-rename atomicity is what protects a git phase, and
//! `tombi` rewriting pyproject.toml (see `format`) is on its own.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};

use anyhow::{Context as _, Result};

/// True only inside `wait_for`.
pub(crate) static WAITING: AtomicBool = AtomicBool::new(false);
pub(crate) static INTERRUPTED: AtomicBool = AtomicBool::new(false);
static WRITES: AtomicUsize = AtomicUsize::new(0);
static EXIT_PENDING: AtomicBool = AtomicBool::new(false);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Action {
  Flag,
  Defer,
  Exit,
}

/// The handler's choice for a given state; pure, so the test never touches the flags.
pub(crate) fn action(waiting: bool, writes: usize) -> Action {
  if waiting {
    Action::Flag
  } else if writes > 0 {
    Action::Defer
  } else {
    Action::Exit
  }
}

/// `EXIT_PENDING` is set before `WRITES` is read, and [`Writing::begin`] increments
/// before it reads the flag, so whichever side comes second sees the other: a write that
/// starts as the exit is decided never begins.
pub(crate) fn on_ctrl_c() -> Action {
  if WAITING.load(SeqCst) {
    INTERRUPTED.store(true, SeqCst);
    return Action::Flag;
  }
  EXIT_PENDING.store(true, SeqCst);
  action(false, WRITES.load(SeqCst))
}

/// Install the handler; an earlier call having done so already is fine.
pub fn install() -> Result<()> {
  match ctrlc::set_handler(|| {
    if on_ctrl_c() == Action::Exit {
      std::process::exit(130);
    }
  }) {
    Err(ctrlc::Error::MultipleHandlers) => Ok(()),
    r => r.context("installing Ctrl-C handler"),
  }
}

/// Held for the duration of a write (one file, or one git phase that rewrites several).
/// A Ctrl-C that arrives meanwhile exits when the guard drops.
pub struct Writing(());

impl Writing {
  pub fn begin() -> Self {
    // Only the outermost guard refuses to start: an inner one is part of a write that
    // is already under way and must finish.
    if WRITES.fetch_add(1, SeqCst) == 0 && EXIT_PENDING.load(SeqCst) {
      std::process::exit(130);
    }
    Writing(())
  }
}

impl Drop for Writing {
  fn drop(&mut self) {
    if WRITES.fetch_sub(1, SeqCst) == 1 && EXIT_PENDING.load(SeqCst) {
      std::process::exit(130);
    }
  }
}

#[cfg(test)]
pub(crate) mod tests {
  use super::*;
  use std::sync::Mutex;

  // Process-wide statics; the session tests share them and take the same lock.
  pub(crate) static SERIAL: Mutex<()> = Mutex::new(());

  fn reset() {
    WAITING.store(false, SeqCst);
    INTERRUPTED.store(false, SeqCst);
    EXIT_PENDING.store(false, SeqCst);
  }

  #[test]
  fn waiting_flags_writing_defers_otherwise_exits() {
    let _g = SERIAL.lock().unwrap();
    reset();
    WAITING.store(true, SeqCst);
    assert_eq!(on_ctrl_c(), Action::Flag);
    assert!(INTERRUPTED.swap(false, SeqCst) && !EXIT_PENDING.load(SeqCst));
    reset();
    // Never through `on_ctrl_c`: a set `EXIT_PENDING` makes any guard in another test
    // thread exit the whole test binary.
    assert_eq!(action(false, 1), Action::Defer);
    assert_eq!(action(false, 0), Action::Exit);
  }

  #[test]
  fn a_guard_counts_the_write_and_a_clean_drop_leaves_no_exit_behind() {
    let _g = SERIAL.lock().unwrap();
    reset();
    {
      let _w = Writing::begin();
      let _inner = Writing::begin();
      assert_eq!(WRITES.load(SeqCst), 2);
    }
    assert_eq!(WRITES.load(SeqCst), 0);
    assert!(!EXIT_PENDING.load(SeqCst));
  }
}
