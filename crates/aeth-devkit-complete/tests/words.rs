//! Splitting a bash command line and locating the cursor within it.
//!
//! bash hands the completer `COMP_LINE` (raw text) and `COMP_POINT` (a byte offset). Every
//! layer above wants words plus an index, and this is the only place that conversion lives.

use aeth_devkit_complete::words::split_line;

#[test]
fn splits_a_plain_line_and_locates_the_cursor_at_the_end() {
  let s = split_line("poe build", 9);
  assert_eq!(s.words, ["poe", "build"]);
  assert_eq!(s.cword, 1);
  assert_eq!(s.prefix, "build");
}

#[test]
fn a_cursor_on_trailing_space_starts_a_fresh_word() {
  let s = split_line("poe ", 4);
  assert_eq!(s.words, ["poe"]);
  // One past the last parsed word: nothing has been typed for it yet.
  assert_eq!(s.cword, 1);
  assert_eq!(s.prefix, "");
}

#[test]
fn a_cursor_mid_word_filters_on_the_left_portion_only() {
  // "poe bu|ild" — the design says text to the right of the cursor is ignored.
  let s = split_line("poe build", 6);
  assert_eq!(s.cword, 1);
  assert_eq!(s.prefix, "bu");
}

#[test]
fn keeps_a_quoted_path_as_one_word() {
  let s = split_line("poe run \"my file.txt\"", 21);
  assert_eq!(s.words, ["poe", "run", "my file.txt"]);
  assert_eq!(s.cword, 2);
}

#[test]
fn an_unclosed_quote_does_not_panic() {
  // shell-words rejects this outright; we retry with the quote closed, because a user
  // mid-way through typing a quoted path means the closed form.
  let s = split_line("poe run \"my fi", 14);
  assert_eq!(s.words[0], "poe");
  assert_eq!(s.cword, 2);
  assert_eq!(s.prefix, "my fi");
}

#[test]
fn inline_equals_stays_in_one_word() {
  // The whole reason for taking the raw line: bash's own COMP_WORDS splits this into
  // three tokens, which is what the old script spent ~20 lines gluing back together.
  let s = split_line("poe -C=../other build", 21);
  assert_eq!(s.words, ["poe", "-C=../other", "build"]);
}

#[test]
fn an_offset_past_the_end_is_clamped_rather_than_panicking() {
  // COMP_POINT arrives from the shell and is not to be trusted with a raw slice.
  let s = split_line("poe", 999);
  assert_eq!(s.words, ["poe"]);
}

#[test]
fn an_offset_inside_a_multibyte_character_is_clamped_to_a_boundary() {
  // Slicing a String at a non-boundary byte panics. In "poe café" the 'é' occupies bytes
  // 7 and 8, so offset 8 is genuinely mid-character; it must clamp back to 7.
  let s = split_line("poe café", 8);
  assert_eq!(s.words, ["poe", "caf"]);
  assert_eq!(s.prefix, "caf");
}

#[test]
fn an_empty_line_yields_no_words() {
  let s = split_line("", 0);
  assert!(s.words.is_empty());
  assert_eq!(s.cword, 0);
  assert_eq!(s.prefix, "");
}
