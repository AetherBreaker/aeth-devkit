//! The release-specific prompt helper; the trait and its implementations live in core so
//! `setup-project` can share them.

use anyhow::Result;

// Re-exported so every `crate::prompt::…` path in this crate and its tests keeps working.
pub use aeth_devkit_core::prompt::{Prompt, ScriptedPrompt, StdinPrompt};

/// `true` when `force` is already set (the `--force` flag) or the user types exactly
/// `force`. Anything else — including `y`, `yes`, an empty line — is a refusal.
pub fn confirm_force(prompt: &dyn Prompt, force: bool, question: &str) -> Result<bool> {
  if force {
    return Ok(true);
  }
  Ok(prompt.ask(question)? == "force")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn confirm_force_requires_the_word_force() {
    assert!(confirm_force(&ScriptedPrompt::new(&["force"]), false, "q").unwrap());
    assert!(!confirm_force(&ScriptedPrompt::new(&["y"]), false, "q").unwrap());
    // With `force == true` the prompt must not even be consulted.
    let p = ScriptedPrompt::new(&[]);
    assert!(confirm_force(&p, true, "q").unwrap());
    assert!(p.asked.borrow().is_empty());
  }
}
