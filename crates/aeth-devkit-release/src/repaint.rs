//! Drawing each refresh of a progress view over the previous one, in the terminal's
//! *normal* screen buffer.
//!
//! `gh run watch` renders into the alternate screen buffer, which is one screen-sized grid
//! with no scrollback by construction: a run taller than the terminal loses its top rows
//! with no way to scroll back to them. Piping its stdout takes the terminal away from it,
//! and it then prints the whole run as plain text once per refresh instead. [`Repaint`]
//! turns that stream back into a live view by erasing the previous block before the next
//! one lands — in the normal buffer, so everything printed before the watch started stays
//! in the scrollback where the user left it.

use std::io::Write;

/// The terminal's (columns, rows), or `None` when stdout is not a terminal.
pub type Size = fn() -> Option<(usize, usize)>;

/// Ask the real terminal, naming stdout explicitly: a size taken from stderr would have us
/// write escape sequences into a redirected stdout.
pub fn stdout_size() -> Option<(usize, usize)> {
  terminal_size::terminal_size_of(std::io::stdout()).map(|(w, h)| (w.0 as usize, h.0 as usize))
}

pub struct Repaint {
  size: Size,
  /// Rows the current block has printed. Reset to 0 when a block is appended rather than
  /// painted over, which is what tells the next block there is nothing to erase.
  painted: usize,
}

impl Repaint {
  pub fn new(size: Size) -> Self {
    Self { size, painted: 0 }
  }

  /// Print one line of the watched output. `starts_block` marks the first line of a refresh,
  /// which is where the previous refresh is erased.
  ///
  /// Errors are swallowed: a broken stdout must not turn a release that is mid-flight into
  /// a rollback, and there is nowhere left to report it to anyway.
  pub fn line(&mut self, out: &mut dyn Write, text: &str, starts_block: bool) {
    let size = (self.size)();
    if starts_block {
      // Only what is still on screen can be erased: the cursor cannot travel above the top
      // row, so a block that was taller than the terminal was scrolled through and is now
      // partly in the scrollback. Leave it there and append the next one — which is the
      // behaviour the alternate screen cannot offer at all.
      if let Some((_, rows)) = size
        && self.painted > 0
        && self.painted < rows
      {
        // Up `painted` rows, then erase from the cursor to the end of the screen.
        let _ = write!(out, "\x1b[{}A\x1b[J", self.painted);
      }
      self.painted = 0;
    }
    match size {
      // Truncated so one line is one row: a wrapped line would make `painted` undercount
      // and leave a stale row behind on the next erase.
      Some((cols, _)) => {
        let _ = writeln!(out, "{}", truncate(text, cols));
      }
      None => {
        let _ = writeln!(out, "{text}");
      }
    }
    self.painted += 1;
    // Line-buffered would be enough on a terminal, but stdout is block-buffered when it is
    // not one, and a watch that shows nothing until it ends is not a watch.
    let _ = out.flush();
  }
}

/// Terminal columns `text` occupies.
///
/// A rough East-Asian-wide/emoji test rather than a unicode-width dependency: it only has to
/// hold for what GitHub puts in job and step names, where the one realistic double-width
/// character is an emoji someone typed into a step name.
pub fn columns(text: &str) -> usize {
  text.chars().map(char_columns).sum()
}

fn char_columns(c: char) -> usize {
  1 + usize::from(matches!(
    c as u32,
    0x1100..=0x115F | 0x2E80..=0xA4CF | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F | 0xFF00..=0xFF60 | 0x1F300..=0x1FAFF
  ))
}

/// `text` cut to at most `cols` terminal columns.
pub fn truncate(text: &str, cols: usize) -> &str {
  let mut used = 0;
  for (i, c) in text.char_indices() {
    let w = char_columns(c);
    if used + w > cols {
      return &text[..i];
    }
    used += w;
  }
  text
}

#[cfg(test)]
mod tests {
  use super::*;

  fn small() -> Option<(usize, usize)> {
    Some((20, 5))
  }
  fn none() -> Option<(usize, usize)> {
    None
  }

  fn paint(size: Size, blocks: &[&[&str]]) -> String {
    let mut r = Repaint::new(size);
    let mut out: Vec<u8> = Vec::new();
    for block in blocks {
      for (i, line) in block.iter().enumerate() {
        r.line(&mut out, line, i == 0);
      }
    }
    String::from_utf8(out).unwrap()
  }

  #[test]
  fn a_block_that_fits_is_erased_before_the_next_one() {
    let out = paint(small, &[&["a", "b"], &["c", "d"]]);
    assert_eq!(out, "a\nb\n\x1b[2A\x1b[Jc\nd\n");
  }

  #[test]
  fn a_block_taller_than_the_terminal_is_left_in_the_scrollback() {
    // Five rows, five lines: the first scrolled off the top, so there is no cursor movement
    // that could erase the block. The next block is appended instead of painted over.
    let out = paint(small, &[&["1", "2", "3", "4", "5"], &["next"]]);
    assert_eq!(out, "1\n2\n3\n4\n5\nnext\n");
  }

  #[test]
  fn nothing_is_erased_before_the_first_block() {
    assert_eq!(paint(small, &[&["only"]]), "only\n");
  }

  #[test]
  fn a_non_terminal_stdout_gets_plain_appended_lines() {
    let out = paint(none, &[&["a"], &["b"]]);
    assert_eq!(out, "a\nb\n");
  }

  #[test]
  fn lines_are_cut_to_the_terminal_width() {
    assert_eq!(truncate("0123456789", 4), "0123");
    assert_eq!(truncate("short", 20), "short");
    // The emoji is two columns, so only three of the four fit.
    assert_eq!(truncate("🧪🧪🧪🧪", 7), "🧪🧪🧪");
  }
}
