//! Hunk table for a proposed change and reassembly from the hunks the user accepted.
//! Both texts must be LF-normalised (`static_files::normalize_newlines`): only then does
//! `similar` split lines exactly like `split_inclusive('\n')`, and these ranges are the
//! contract with the VS Code extension, which indexes the same two texts.

use serde::{Deserialize, Serialize};
use similar::{DiffOp, TextDiff};

/// One changed region as 0-based `[start, end)` line ranges in each text. Context lines
/// are excluded, so `proposed[0]` is the line the extension puts the hunk's lens on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
  pub current: [usize; 2],
  pub proposed: [usize; 2],
}

/// The hunks of the unified diff the terminal prints: three lines of context decide which
/// nearby changes merge into one hunk, exactly as in `static_files::unified_diff`.
pub fn hunks(current: &str, proposed: &str) -> Vec<Hunk> {
  let diff = TextDiff::from_lines(current, proposed);
  diff
    .grouped_ops(3)
    .iter()
    .filter_map(|group| {
      // A group is context + changes + context; only the change ops bound the hunk.
      let mut changed = group.iter().filter(|op| !matches!(op, DiffOp::Equal { .. }));
      let first = changed.next()?;
      let last = changed.next_back().unwrap_or(first);
      Some(Hunk {
        current: [first.old_range().start, last.old_range().end],
        proposed: [first.new_range().start, last.new_range().end],
      })
    })
    .collect()
}

/// `proposed` with every hunk not in `accepted` reverted to the current text. `Err` when
/// an index is out of range (a malformed response from the extension).
pub fn assemble(current: &str, proposed: &str, hunks: &[Hunk], accepted: &[usize]) -> anyhow::Result<String> {
  if let Some(bad) = accepted.iter().find(|i| **i >= hunks.len()) {
    anyhow::bail!("accepted hunk {bad} does not exist ({} hunks)", hunks.len());
  }
  // `split_inclusive` keeps each line's `\n`, so joining the slices back is lossless,
  // including a missing final newline.
  let cur: Vec<&str> = current.split_inclusive('\n').collect();
  let new: Vec<&str> = proposed.split_inclusive('\n').collect();
  let mut out = String::new();
  let mut cursor = 0;
  for (i, h) in hunks.iter().enumerate() {
    out.extend(cur[cursor..h.current[0]].iter().copied());
    let side = if accepted.contains(&i) {
      &new[h.proposed[0]..h.proposed[1]]
    } else {
      &cur[h.current[0]..h.current[1]]
    };
    out.extend(side.iter().copied());
    cursor = h.current[1];
  }
  out.extend(cur[cursor..].iter().copied());
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  const CUR: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
  const NEW: &str = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n";

  #[test]
  fn hunks_exclude_context_and_split_far_apart_changes() {
    assert_eq!(
      hunks(CUR, NEW),
      vec![
        Hunk {
          current: [1, 2],
          proposed: [1, 2]
        },
        Hunk {
          current: [9, 10],
          proposed: [9, 11]
        },
      ]
    );
  }

  #[test]
  fn hunks_for_pure_insert_delete_and_no_change() {
    assert_eq!(
      hunks("a\nb\n", "a\nx\nb\n"),
      vec![Hunk {
        current: [1, 1],
        proposed: [1, 2]
      }]
    );
    assert_eq!(
      hunks("a\nx\nb\n", "a\nb\n"),
      vec![Hunk {
        current: [1, 2],
        proposed: [1, 1]
      }]
    );
    assert!(hunks("a\n", "a\n").is_empty());
  }

  #[test]
  fn assemble_reverts_rejected_hunks() {
    let h = hunks(CUR, NEW);
    assert_eq!(assemble(CUR, NEW, &h, &[0, 1]).unwrap(), NEW);
    assert_eq!(assemble(CUR, NEW, &h, &[]).unwrap(), CUR);
    assert_eq!(assemble(CUR, NEW, &h, &[1]).unwrap(), "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n");
    assert_eq!(assemble(CUR, NEW, &h, &[0]).unwrap(), "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\n");
    assert!(assemble(CUR, NEW, &h, &[2]).is_err());
  }

  #[test]
  fn assemble_keeps_a_missing_final_newline() {
    let (cur, new) = ("a\nb", "a\nc");
    let h = hunks(cur, new);
    assert_eq!(assemble(cur, new, &h, &[0]).unwrap(), "a\nc");
    assert_eq!(assemble(cur, new, &h, &[]).unwrap(), "a\nb");
  }
}
