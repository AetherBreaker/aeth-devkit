//! Render a [`Directive`] as the line-oriented text the shims parse.
//!
//! Shape is a directive header followed by zero or more tab-separated item lines:
//!
//! ```text
//! items
//! build<TAB>build<TAB>Compile the crate<TAB>command
//! ```
//!
//! The header carries no version. The shim version is negotiated on the *request* side, and
//! stamping it here too would imply the response format changes whenever the shim text is
//! edited cosmetically, which it does not.

use crate::engine::{Directive, Item};

/// Directive header for a list of candidates.
const HEADER_ITEMS: &str = "items";
/// Directive header asking the shell to complete directory names itself.
const HEADER_DIRS: &str = "dirs";
/// Directive header asking the shell to complete file names itself.
const HEADER_FILES: &str = "files";

/// Replace every character that would forge a column or a record with a space.
///
/// Task names, help text and choices all come from a project's `pyproject.toml`, so they are
/// untrusted as far as this format is concerned: a tab in a help string would shift every
/// later column, and a newline would fabricate an entire extra candidate.
///
/// Returns `String` rather than `Cow<str>` because the caller is building an owned line
/// anyway; the copy that a `Cow` would sometimes avoid is one the caller would then make.
fn sanitize(field: &str) -> String {
  field
    .chars()
    // `matches!` compares against a pattern rather than a value, which reads better than
    // three chained `==` and compiles to the same jump table.
    .map(|c| if matches!(c, '\t' | '\n' | '\r') { ' ' } else { c })
    .collect()
}

/// One item as its four tab-separated columns, newline-terminated.
fn render_item(item: &Item) -> String {
  format!(
    "{}\t{}\t{}\t{}\n",
    sanitize(&item.value),
    sanitize(&item.display),
    sanitize(&item.tooltip),
    item.kind.as_wire(),
  )
}

/// Render a directive for transmission to a shim.
pub fn render(directive: &Directive) -> String {
  match directive {
    // The sentinels are a header and nothing else: there are no candidates to send, only
    // an instruction for the shell to do the work.
    Directive::Dirs => format!("{HEADER_DIRS}\n"),
    Directive::Files => format!("{HEADER_FILES}\n"),
    Directive::Items(items) => {
      // `String::from` then `push_str` in a loop, rather than `map().collect()`, so the
      // header and the body share one allocation that grows as needed.
      let mut out = String::from(HEADER_ITEMS);
      out.push('\n');
      for item in items {
        out.push_str(&render_item(item));
      }
      out
    }
  }
}
