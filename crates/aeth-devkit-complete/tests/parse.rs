//! Scanning a poe command line for its global options, target directory and task.
//!
//! Every case here was previously implemented twice — once in the bash script and once in
//! the PowerShell one — which is exactly how the two drifted.

use aeth_devkit_complete::parse::{GLOBAL_OPTIONS, OPTIONS_WITH_VALUES, exclusions, parse};

/// Build owned words from a display line.
fn w(line: &str) -> Vec<String> {
  line.split(' ').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

// ---- task location -----------------------------------------------------------------------

#[test]
fn finds_the_task_as_the_first_non_option_word() {
  let p = parse(&w("poe build --release"), 2);
  assert_eq!(p.task.as_deref(), Some("build"));
  assert_eq!(p.task_position, Some(1));
}

#[test]
fn no_task_yet_when_only_options_have_been_typed() {
  let p = parse(&w("poe --verbose"), 2);
  assert_eq!(p.task, None);
  assert_eq!(p.task_position, None);
}

#[test]
fn a_global_options_value_is_not_mistaken_for_the_task() {
  // "uv" is the value of -e, not a task name.
  let p = parse(&w("poe -e uv build"), 3);
  assert_eq!(p.task.as_deref(), Some("build"));
  assert_eq!(p.task_position, Some(3));
}

#[test]
fn an_inline_equals_option_does_not_swallow_the_next_word() {
  // `-e=uv` carries its own value, so "build" is still the task.
  let p = parse(&w("poe -e=uv build"), 2);
  assert_eq!(p.task.as_deref(), Some("build"));
  assert_eq!(p.task_position, Some(2));
}

// ---- target directory --------------------------------------------------------------------

#[test]
fn extracts_target_dir_from_a_separate_argument() {
  let p = parse(&w("poe -C ../other build"), 3);
  assert_eq!(p.target_dir.as_deref(), Some("../other"));
  assert_eq!(p.task.as_deref(), Some("build"));
  assert_eq!(p.task_position, Some(3));
}

#[test]
fn extracts_target_dir_from_inline_equals() {
  let p = parse(&w("poe --directory=../other build"), 2);
  assert_eq!(p.target_dir.as_deref(), Some("../other"));
  assert_eq!(p.task_position, Some(2));
}

#[test]
fn accepts_the_root_spelling_for_target_dir() {
  // poe's own scripts honour --root here even though it is absent from the option list.
  let p = parse(&w("poe --root ../other build"), 3);
  assert_eq!(p.target_dir.as_deref(), Some("../other"));
}

#[test]
fn a_dangling_dash_c_with_no_value_yields_no_target_dir() {
  // "poe -C" with the cursor on the value: nothing to extract yet, and no panic.
  let p = parse(&w("poe -C"), 2);
  assert_eq!(p.target_dir, None);
  assert_eq!(p.task, None);
}

#[test]
fn options_after_the_task_do_not_set_the_target_dir() {
  // -C is a global; once the task is found the scan stops treating words as globals.
  let p = parse(&w("poe build -C ../other"), 4);
  assert_eq!(p.target_dir, None);
  assert_eq!(p.task.as_deref(), Some("build"));
}

// ---- pass-through separator --------------------------------------------------------------

#[test]
fn detects_the_pass_through_separator_before_the_cursor() {
  let p = parse(&w("poe run -- somefile"), 3);
  assert!(p.after_separator);
}

#[test]
fn a_separator_after_the_cursor_does_not_count() {
  // Cursor is on "x"; the -- further right is not yet relevant.
  let p = parse(&w("poe run x -- y"), 2);
  assert!(!p.after_separator);
}

#[test]
fn a_separator_before_any_task_is_not_a_pass_through() {
  let p = parse(&w("poe --"), 2);
  assert!(!p.after_separator);
}

// ---- constants ---------------------------------------------------------------------------

#[test]
fn global_options_match_poes_own_list() {
  // Ported verbatim from the generated scripts; drift here is a behaviour change.
  assert_eq!(GLOBAL_OPTIONS.len(), 17);
  assert!(GLOBAL_OPTIONS.contains(&"--no-ansi"));
  assert!(GLOBAL_OPTIONS.contains(&"--executor-opt"));
}

#[test]
fn options_with_values_match_poes_own_list() {
  assert_eq!(OPTIONS_WITH_VALUES.len(), 8);
  assert!(OPTIONS_WITH_VALUES.contains(&"--executor"));
  // A boolean global must NOT be here, or the scan would eat the following word.
  assert!(!OPTIONS_WITH_VALUES.contains(&"--verbose"));
}

#[test]
fn verbose_and_quiet_suppress_each_other() {
  assert!(exclusions("-v").contains(&"--quiet"));
  assert!(exclusions("--quiet").contains(&"-v"));
  // An option with no exclusions returns an empty slice rather than panicking.
  assert!(exclusions("--executor-opt").is_empty());
}
