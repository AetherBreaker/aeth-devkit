//! The positional-argument heuristic inherited from `release.sh`.
//!
//! `poe release` forwards its free arguments untouched, so the command receives one flat
//! list of words and must decide which are bump types and which are release notes:
//!
//! - leading words that are bump types (`major`, `patch`, …) are bumps;
//! - everything from the first non-bump word on is the *tail*;
//! - a tail of 2+ words (or one word containing a space, i.e. a shell-quoted string) is the
//!   notes text;
//! - a tail of exactly one bare word is almost certainly a typo'd bump type, so it is an
//!   error rather than a one-word release note.
//!
//! This lives in its own module as a pure function (no I/O) so the rules can be unit-tested
//! exhaustively without involving clap or the rest of the command.

use anyhow::{Result, bail};

/// The keywords `uv version --bump` accepts, in the order the help text lists them.
pub const BUMP_TYPES: [&str; 9] = ["major", "minor", "patch", "stable", "alpha", "beta", "rc", "post", "dev"];

/// `true` if `word` is one of [`BUMP_TYPES`].
pub fn is_bump_type(word: &str) -> bool {
  // `contains` on an array of `&str` compares by value; `&word` makes the types line up
  // (`&&str` vs the array's element type `&str` — `contains` takes a reference to an element).
  BUMP_TYPES.contains(&word)
}

/// The outcome of parsing the free arguments.
///
/// `Default` gives us an all-empty value to fill in incrementally; `notes` is an `Option`
/// because "no notes" is a real state distinct from empty text.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
  pub bumps: Vec<String>,
  pub notes: Option<String>,
  pub force: bool,
}

/// Apply the heuristic described in the module docs.
pub fn parse_positionals(words: &[String]) -> Result<Parsed> {
  let mut parsed = Parsed::default();
  // Borrowing `&str` slices out of `words` rather than cloning: `tail` cannot outlive
  // `words`, and the borrow checker verifies that for us.
  let mut tail: Vec<&str> = Vec::new();
  for word in words {
    // `match` on the string, with *guards* (`if …`) to add conditions to a pattern. Arms are
    // tried top to bottom; the first that matches wins.
    match word.as_str() {
      "--force" | "-f" => parsed.force = true,
      // A bump type only counts while the tail is still empty — once notes have started,
      // a word like "patch" inside them is just part of the sentence.
      w if is_bump_type(w) && tail.is_empty() => parsed.bumps.push(w.to_string()),
      w => tail.push(w),
    }
  }
  // Slice patterns on the tail decide what it means.
  parsed.notes = match tail.as_slice() {
    [] => None,
    // One argument that contains a space: the shell kept the user's quotes, so it is notes.
    [one] if one.contains(' ') => Some(one.to_string()),
    // One bare word: treat as a typo. `bail!` returns the error from the whole function.
    [one] => bail!(
      "'{one}' is not a valid bump type.\n       Valid bump types: {}\n       (Notes must be multiple words — single-word notes are not supported.)",
      BUMP_TYPES.join(", ")
    ),
    // Two or more words: join them back together with single spaces.
    many => Some(many.join(" ")),
  };
  Ok(parsed)
}

#[cfg(test)]
mod tests {
  use super::*;

  /// Tiny helper: `&["a", "b"]` → `vec!["a".to_string(), "b".to_string()]`.
  fn w(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
  }

  #[test]
  fn bumps_only() {
    let p = parse_positionals(&w(&["major", "alpha"])).unwrap();
    assert_eq!(p.bumps, vec!["major", "alpha"]);
    assert_eq!(p.notes, None);
    assert!(!p.force);
  }

  #[test]
  fn nothing_means_no_bump() {
    let p = parse_positionals(&[]).unwrap();
    assert!(p.bumps.is_empty() && p.notes.is_none());
  }

  #[test]
  fn strips_force_flags() {
    let p = parse_positionals(&w(&["--force", "patch", "-f"])).unwrap();
    assert!(p.force);
    assert_eq!(p.bumps, vec!["patch"]);
  }

  #[test]
  fn multi_word_tail_is_notes() {
    // "minor" appears again inside the notes and must not be taken as a second bump.
    let p = parse_positionals(&w(&["minor", "first", "minor", "release"])).unwrap();
    assert_eq!(p.bumps, vec!["minor"]);
    assert_eq!(p.notes.as_deref(), Some("first minor release"));
  }

  #[test]
  fn single_spaced_arg_is_notes() {
    let p = parse_positionals(&w(&["publish notes"])).unwrap();
    assert!(p.bumps.is_empty());
    assert_eq!(p.notes.as_deref(), Some("publish notes"));
  }

  #[test]
  fn single_word_tail_is_an_error() {
    let e = parse_positionals(&w(&["patch", "typo"])).unwrap_err().to_string();
    assert!(e.contains("'typo' is not a valid bump type"), "{e}");
    assert!(e.contains("major, minor, patch, stable, alpha, beta, rc, post, dev"));
    assert!(e.contains("multiple words"));
  }
}
