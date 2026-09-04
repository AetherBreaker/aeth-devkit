//! Asking the user a question on the terminal, with a scripted stand-in for tests.
//!
//! `devkit release` needs a human for "dirty tree, continue?" and "artefacts exist,
//! remove them?"; `devkit setup-project` needs one before replacing a Docker file. The
//! prompt is a trait so tests feed canned answers and assert which questions were asked.

use std::cell::RefCell;
// `VecDeque` is a double-ended queue: cheap `pop_front`, which is what "next scripted
// answer" needs. A `Vec` would have to shift every element on each pop.
use std::collections::VecDeque;
// Traits for `read_line` (BufRead) and `flush` (Write); imported anonymously for methods.
use std::io::{BufRead as _, Write as _};

use anyhow::{Context as _, Result, bail};

/// Something that can ask a question and return the user's (trimmed) answer.
pub trait Prompt {
  fn ask(&self, question: &str) -> Result<String>;
}

/// Reads answers from standard input.
pub struct StdinPrompt;

impl Prompt for StdinPrompt {
  fn ask(&self, question: &str) -> Result<String> {
    // Prompts go to stderr so stdout stays clean for machine-readable output, and so the
    // question shows even when stdout is redirected. `flush` forces the text out before we
    // block waiting for input (stderr is usually unbuffered, but be explicit).
    eprint!("{question} ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    // `lock()` takes the stdin handle once for the whole read instead of per byte.
    std::io::stdin().lock().read_line(&mut line).context("reading answer from stdin")?;
    Ok(line.trim().to_string())
  }
}

/// Answers from a queue and records every question; for tests.
pub struct ScriptedPrompt {
  pub answers: RefCell<VecDeque<String>>,
  pub asked: RefCell<Vec<String>>,
}

impl ScriptedPrompt {
  pub fn new(answers: &[&str]) -> Self {
    Self {
      // `collect()` can build a `VecDeque` directly from an iterator of `String`s.
      answers: RefCell::new(answers.iter().map(|s| s.to_string()).collect()),
      asked: RefCell::new(Vec::new()),
    }
  }
}

impl Prompt for ScriptedPrompt {
  fn ask(&self, question: &str) -> Result<String> {
    self.asked.borrow_mut().push(question.to_string());
    match self.answers.borrow_mut().pop_front() {
      Some(a) => Ok(a),
      // Running out of answers is a test bug worth failing loudly on, not a silent "no".
      None => bail!("no scripted answer for {question:?}"),
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn scripted_prompt_answers_in_order_then_errors() {
    let p = ScriptedPrompt::new(&["force", "no"]);
    assert_eq!(p.ask("a?").unwrap(), "force");
    assert_eq!(p.ask("b?").unwrap(), "no");
    assert!(p.ask("c?").is_err());
    assert_eq!(*p.asked.borrow(), vec!["a?", "b?", "c?"]);
  }
}
