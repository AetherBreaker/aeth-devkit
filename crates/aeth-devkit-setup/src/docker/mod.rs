//! Docker standardisation: templated `docker/` files replaced whole and the compose file
//! edited in place — each only with the user's consent, given per file or once for all.

pub mod compose_rules;
pub mod scaffold;
pub mod static_files;

use std::cell::Cell;
use std::path::Path;

use anyhow::Result;

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::prompt::Prompt;

use crate::changes::Changes;
use crate::context::ProjectContext;

/// How consent questions are answered for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  /// A terminal is attached: ask per file.
  Ask,
  /// `--replace-docker`, or the user answered `replace all`.
  ReplaceAll,
  /// No terminal and no flag: show diffs, change nothing.
  KeepAll,
  /// `--dry-run` / `--check`: every intended edit is recorded, nothing is asked or written.
  DryRun,
}

/// The injectable collaborators, in the style of the release and pin crates.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub prompt: &'a dyn Prompt,
  pub mode: Mode,
  /// Whether a human can answer the add-service question (stdin is a terminal and this
  /// is not a dry run). `replace all` never adds a service: a typo in pyproject must not
  /// grow the compose file without someone reading the service name.
  pub interactive: bool,
}

/// Consent state for one run. `Cell` because `replace all` upgrades the mode through a
/// shared reference: the same `&Consent` is handed to every step.
pub struct Consent<'a> {
  prompt: &'a dyn Prompt,
  mode: Cell<Mode>,
  interactive: bool,
  declined_silently: Cell<bool>,
}

impl<'a> Consent<'a> {
  pub fn new(prompt: &'a dyn Prompt, mode: Mode, interactive: bool) -> Self {
    Self {
      prompt,
      mode: Cell::new(mode),
      interactive,
      declined_silently: Cell::new(false),
    }
  }

  /// Whether a change whose diff was just printed may be applied.
  pub fn replace(&self, question: &str) -> Result<bool> {
    match self.mode.get() {
      Mode::ReplaceAll | Mode::DryRun => Ok(true),
      Mode::KeepAll => {
        self.declined_silently.set(true);
        Ok(false)
      }
      Mode::Ask => match self.prompt.ask(question)?.as_str() {
        "replace" => Ok(true),
        "replace all" => {
          self.mode.set(Mode::ReplaceAll);
          Ok(true)
        }
        _ => Ok(false),
      },
    }
  }

  /// Whether a listed-but-absent service may be scaffolded into the compose file.
  pub fn add(&self, question: &str) -> Result<bool> {
    if self.mode.get() == Mode::DryRun {
      return Ok(true); // an intended edit, shown like every other
    }
    if !self.interactive {
      return Ok(false);
    }
    Ok(self.prompt.ask(question)? == "add")
  }

  /// A change was kept only because nobody could be asked.
  pub fn kept_silently(&self) -> bool {
    self.declined_silently.get()
  }
}

/// Everything Docker: static files first, then the compose file (Task 7), then advisories.
pub fn apply(ctx: &ProjectContext, templates_dir: &Path, deps: &Deps, changes: &mut Changes) -> Result<()> {
  let consent = Consent::new(deps.prompt, deps.mode, deps.interactive);
  static_files::apply(ctx, templates_dir, &consent, changes)?;
  Ok(())
}

#[cfg(test)]
mod consent_tests {
  use super::*;
  use aeth_devkit_core::prompt::ScriptedPrompt;

  #[test]
  fn replace_all_sticks_for_the_rest_of_the_run() {
    let p = ScriptedPrompt::new(&["replace all"]);
    let c = Consent::new(&p, Mode::Ask, true);
    assert!(c.replace("a?").unwrap());
    assert!(c.replace("b?").unwrap(), "no second question");
    assert_eq!(p.asked.borrow().len(), 1);
  }

  #[test]
  fn anything_but_the_keywords_keeps() {
    let p = ScriptedPrompt::new(&["replace", "y", "", "add", "no"]);
    let c = Consent::new(&p, Mode::Ask, true);
    assert!(c.replace("a?").unwrap());
    assert!(!c.replace("b?").unwrap());
    assert!(!c.replace("c?").unwrap());
    assert!(c.add("d?").unwrap());
    assert!(!c.add("e?").unwrap());
  }

  #[test]
  fn dry_run_and_keep_all_never_ask() {
    let p = ScriptedPrompt::new(&[]);
    let dry = Consent::new(&p, Mode::DryRun, false);
    assert!(dry.replace("a?").unwrap() && dry.add("b?").unwrap());
    let keep = Consent::new(&p, Mode::KeepAll, false);
    assert!(!keep.replace("a?").unwrap() && !keep.add("b?").unwrap());
    assert!(keep.kept_silently());
    // --replace-docker without a terminal: files replaced, services never added.
    let all = Consent::new(&p, Mode::ReplaceAll, false);
    assert!(all.replace("a?").unwrap());
    assert!(!all.add("b?").unwrap());
    assert!(p.asked.borrow().is_empty());
  }
}
