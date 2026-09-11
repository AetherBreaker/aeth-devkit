//! Markdown files with a devkit-managed block (`AGENTS.md`).
//!
//! TOML tables and JSON keys have identity, so merging them can tell "template-owned" from
//! "project-owned" for free. Markdown has none, so we manufacture the boundary: everything
//! between `<!-- devkit:begin -->` and `<!-- devkit:end -->` is devkit's and is replaced
//! wholesale; everything outside is the project's and is never touched.

use anyhow::{Result, bail};

pub const BEGIN: &str = "<!-- devkit:begin -->";
pub const END: &str = "<!-- devkit:end -->";

/// Replace the managed block in `original` with `block`, or append it when absent.
/// `original: None` creates the file. Errors on a `begin` marker with no `end`.
pub fn merge_managed_block(original: Option<&str>, block: &str, log: &mut Vec<String>) -> Result<String> {
  // Work in LF internally and restore CRLF at the end if the file used it, so a
  // Windows-authored AGENTS.md doesn't come back with mixed endings. `is_some_and` is the
  // `Option` idiom for "present *and* satisfies this predicate".
  let crlf = original.is_some_and(|o| o.contains("\r\n"));
  let original = original.map(|o| o.replace("\r\n", "\n"));

  // The block always ends in exactly one newline before the end marker, whatever the
  // template file's trailing whitespace happened to be.
  let wrapped = format!("{BEGIN}\n{}\n{END}\n", block.trim_end_matches('\n'));

  // `as_deref` turns `Option<String>` into `Option<&str>` so we borrow rather than move.
  let merged = match original.as_deref() {
    None => wrapped,
    Some(text) => match text.find(BEGIN) {
      // No block yet: append after the project's own text, separated by a blank line.
      // Appending (not prepending) keeps the project's title at the top where readers
      // expect it.
      None => {
        log.push("added devkit-managed block".into());
        if text.trim().is_empty() {
          wrapped
        } else {
          format!("{}\n\n{wrapped}", text.trim_end_matches('\n'))
        }
      }
      Some(start) => {
        // Search for the end marker only *after* the begin marker. `text[start..].find`
        // returns an offset relative to the slice, so add `start` back to get a document
        // offset. `let … else` binds on the `Some` path and must diverge on the `None` one.
        let Some(rel_end) = text[start..].find(END) else {
          // A begin with no end means someone edited inside the block and lost the marker.
          // Guessing where the block ends could destroy their text, so refuse.
          bail!("{END} marker missing after {BEGIN}; restore it (or remove both) and re-run")
        };
        let mut after = start + rel_end + END.len();
        // Swallow the newline that followed the old end marker; `wrapped` brings its own.
        if text[after..].starts_with('\n') {
          after += 1;
        }
        let merged = format!("{}{wrapped}{}", &text[..start], &text[after..]);
        if merged != text {
          log.push("updated devkit-managed block".into());
        }
        merged
      }
    },
  };
  Ok(if crlf { merged.replace('\n', "\r\n") } else { merged })
}

#[cfg(test)]
mod tests {
  use super::*;
  const BLOCK: &str = "## Rule\n\nDo the thing.\n";

  fn wrapped(body: &str) -> String {
    format!("{BEGIN}\n{body}{END}\n")
  }

  #[test]
  fn creates_file_from_block() {
    let mut log = vec![];
    let out = merge_managed_block(None, BLOCK, &mut log).unwrap();
    assert_eq!(out, wrapped(BLOCK));
  }

  #[test]
  fn appends_when_no_markers() {
    let mut log = vec![];
    let out = merge_managed_block(Some("# My Project\n\nIntro.\n"), BLOCK, &mut log).unwrap();
    assert_eq!(out, format!("# My Project\n\nIntro.\n\n{}", wrapped(BLOCK)));
    assert!(log.iter().any(|l| l.contains("added")), "{log:?}");
  }

  #[test]
  fn replaces_between_markers_and_preserves_outside() {
    let orig = format!("# Title\n\n{}\n## Project notes\n\nkeep me\n", wrapped("old stuff\n"));
    let mut log = vec![];
    let out = merge_managed_block(Some(&orig), BLOCK, &mut log).unwrap();
    assert_eq!(out, format!("# Title\n\n{}\n## Project notes\n\nkeep me\n", wrapped(BLOCK)));
    assert!(log.iter().any(|l| l.contains("updated")), "{log:?}");
  }

  #[test]
  fn second_merge_is_a_no_op() {
    let mut log = vec![];
    let once = merge_managed_block(Some("# T\n"), BLOCK, &mut log).unwrap();
    let mut log2 = vec![];
    let twice = merge_managed_block(Some(&once), BLOCK, &mut log2).unwrap();
    assert_eq!(once, twice);
    assert!(log2.is_empty(), "{log2:?}");
  }

  #[test]
  fn unterminated_block_is_an_error() {
    let mut log = vec![];
    let err = merge_managed_block(Some(&format!("{BEGIN}\nhalf\n")), BLOCK, &mut log).unwrap_err();
    assert!(err.to_string().contains(END), "{err}");
  }

  #[test]
  fn preserves_crlf() {
    let orig = format!("# T\r\n\r\n{BEGIN}\r\nold\r\n{END}\r\n");
    let mut log = vec![];
    let out = merge_managed_block(Some(&orig), BLOCK, &mut log).unwrap();
    assert_eq!(out, format!("# T\r\n\r\n{BEGIN}\r\n## Rule\r\n\r\nDo the thing.\r\n{END}\r\n"));
  }
}
