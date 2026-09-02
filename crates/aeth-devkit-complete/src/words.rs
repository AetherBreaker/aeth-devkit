//! Turn bash's `COMP_LINE` / `COMP_POINT` pair into words plus a cursor index.
//!
//! Only bash needs this. PowerShell hands the completer `$commandAst`, which its own parser
//! already produced, so re-parsing raw text there would mean discarding an authoritative
//! answer and deriving a worse one. See the design doc's "Cursor semantics".

/// A command line split around the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Split {
  /// argv-style words parsed from the text left of the cursor.
  pub words: Vec<String>,
  /// Index of the word being completed. Equals `words.len()` when a fresh word is starting.
  pub cword: usize,
  /// The partially typed word: what candidates must start with.
  pub prefix: String,
}

/// Round `point` down to the nearest character boundary at or before it, and never past the
/// end of `line`.
///
/// `COMP_POINT` comes from the shell and is a byte count, so on a line containing any
/// multi-byte character it can land mid-character. `str::split_at` panics on such an index,
/// and a panicking completer would print a backtrace over the user's prompt.
fn clamp_to_boundary(line: &str, point: usize) -> usize {
  // `min` first: an offset past the end is common (the shell counts the whole line while we
  // may have been handed a shorter one) and is not an error.
  let mut p = point.min(line.len());
  // `is_char_boundary` is the cheap check the standard library uses internally; walking back
  // at most three bytes is enough because UTF-8 sequences are at most four bytes long.
  while p > 0 && !line.is_char_boundary(p) {
    p -= 1;
  }
  p
}

/// Split the text left of the cursor into words and say which one is being completed.
///
/// Everything to the right of the cursor is discarded deliberately: it is not part of what
/// the user is completing, and ignoring it also sidesteps a half-typed quote further along
/// the line.
pub fn split_line(line: &str, point: usize) -> Split {
  let point = clamp_to_boundary(line, point);
  // `split_at` returns a tuple of two borrowed halves; we only want the left one, and `_`
  // discards the right without binding it.
  let (left, _) = line.split_at(point);

  // A cursor sitting directly after whitespace means a brand-new word with nothing typed.
  // An empty line is the same case: word zero, no prefix.
  let starting_new_word = left.is_empty() || left.ends_with(char::is_whitespace);

  // `shell_words::split` implements POSIX word splitting, including quote handling, so we
  // do not hand-roll it. It returns `Err` on an unterminated quote — which is *normal* while
  // someone is typing one — so retry with each quote character appended before giving up.
  // `or_else` runs only on the error path, and each closure returns the same Result type.
  let mut parsed = shell_words::split(left)
    .or_else(|_| shell_words::split(&format!("{left}\"")))
    .or_else(|_| shell_words::split(&format!("{left}'")))
    // Last resort: naive whitespace splitting. Worse than POSIX rules, but a completer that
    // offers slightly wrong candidates beats one that returns nothing at all.
    .unwrap_or_else(|_| left.split_whitespace().map(str::to_string).collect());

  if starting_new_word {
    // Nothing typed for the new word, so its index is one past everything parsed so far.
    let cword = parsed.len();
    Split {
      words: parsed,
      cword,
      prefix: String::new(),
    }
  } else {
    // The last parsed token *is* the partially typed word. It stays in `words` so that
    // positional counting downstream sees the same shape in both branches, and `cword`
    // points at it.
    let prefix = parsed.last().cloned().unwrap_or_default();
    // `saturating_sub` rather than `- 1`: on an empty parse this would underflow, and a
    // usize underflow panics in debug and wraps to a huge number in release.
    let cword = parsed.len().saturating_sub(1);
    if parsed.is_empty() {
      parsed.push(String::new());
    }
    Split {
      words: parsed,
      cword,
      prefix,
    }
  }
}
