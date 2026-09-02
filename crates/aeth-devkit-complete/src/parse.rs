//! Scan a poe command line: where is the task, which directory was targeted, are we past
//! the `--` separator.
//!
//! Every constant and rule here is ported verbatim from the completion scripts poe itself
//! generates, so behaviour is identical and only the implementation language changed.

/// poe's global options, in the order they should be offered.
///
/// A slice rather than a fixed-size array: `[&str; N]` forces the length into the type, and
/// getting `N` wrong is a compile error you fix by editing a number rather than by thinking
/// about the list. `&[&str]` lets the list be the single source of truth.
pub const GLOBAL_OPTIONS: &[&str] = &[
  "-h",
  "--help",
  "--version",
  "-v",
  "--verbose",
  "-q",
  "--quiet",
  "-d",
  "--dry-run",
  "-C",
  "--directory",
  "-e",
  "--executor",
  "-X",
  "--executor-opt",
  "--ansi",
  "--no-ansi",
];

/// Globals that consume the following word, so a left-to-right scan must skip that word or
/// it would mistake the value for the task name.
pub const OPTIONS_WITH_VALUES: &[&str] = &["-h", "--help", "-C", "--directory", "-e", "--executor", "-X", "--executor-opt"];

/// Spellings that name the project directory. `--root` is honoured by poe's own scripts
/// here even though it is absent from [`GLOBAL_OPTIONS`]; that asymmetry is poe's, kept.
pub const DIRECTORY_OPTIONS: &[&str] = &["-C", "--directory", "--root"];

/// Valid `-e` / `--executor` values, offered as choices.
pub const EXECUTORS: &[&str] = &["auto", "poetry", "simple", "uv", "virtualenv"];

/// Which spellings become redundant once `opt` is present.
///
/// Returns `&'static [&'static str]` — a borrow of data baked into the binary, so there is
/// no allocation and no lifetime tied to the caller. The empty slice is the "nothing is
/// excluded" case, which keeps every call site free of `Option` handling.
pub fn exclusions(opt: &str) -> &'static [&'static str] {
  match opt {
    "-h" | "--help" => &["--help", "-h"],
    "--version" => &["--version"],
    "-v" | "--verbose" => &["--quiet", "-q"],
    "-q" | "--quiet" => &["--verbose", "-v"],
    "-d" | "--dry-run" => &["--dry-run", "-d"],
    "-C" | "--directory" => &["--directory", "-C"],
    "-e" | "--executor" => &["--executor", "-e"],
    "--ansi" | "--no-ansi" => &["--ansi", "--no-ansi"],
    // `-X` / `--executor-opt` may be repeated, so nothing is suppressed.
    _ => &[],
  }
}

/// What a scan of the command line found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
  /// Value of `-C` / `--directory` / `--root`, if one was given before the task.
  pub target_dir: Option<String>,
  /// The task name, i.e. the first word that is neither an option nor an option's value.
  pub task: Option<String>,
  /// Index of that task within `words`.
  pub task_position: Option<usize>,
  /// Whether a `--` appears after the task and before the cursor, meaning everything from
  /// here on is passed through to the task rather than interpreted by poe.
  pub after_separator: bool,
}

/// Split `-C=../x` into its option name and inline value.
///
/// Returns `(name, Some(value))` for the inline form and `(whole, None)` otherwise.
///
/// The returned strs borrow from `word` rather than copying. That lifetime relationship is
/// written out nowhere because elision infers it: with exactly one input reference, Rust
/// gives every output reference that same lifetime, which is precisely what is wanted here.
fn split_inline(word: &str) -> (&str, Option<&str>) {
  match word.split_once('=') {
    Some((name, value)) => (name, Some(value)),
    None => (word, None),
  }
}

/// Scan `words` (argv-style, `words[0]` is the command) with the cursor on index `cword`.
pub fn parse(words: &[String], cword: usize) -> Parsed {
  let mut out = Parsed::default();

  // A manual index rather than `for (i, w) in ...`: an option that takes a value needs to
  // advance the cursor by two, and a `for` loop's variable cannot be nudged from inside.
  let mut i = 1;
  while i < words.len() {
    let word = words[i].as_str();

    // Globals only precede the task, so the scan stops as soon as one is found.
    if out.task.is_some() {
      break;
    }

    if word.starts_with('-') && word != "--" {
      let (name, inline) = split_inline(word);

      if DIRECTORY_OPTIONS.contains(&name) {
        match inline {
          // `-C=../x`: the value is right here.
          Some(value) => out.target_dir = Some(value.to_string()),
          // `-C ../x`: the value is the next word, if there is one. A dangling `-C` at the
          // end of the line (the user is about to type the directory) leaves this None.
          None => {
            if let Some(next) = words.get(i + 1) {
              out.target_dir = Some(next.clone());
              i += 1;
            }
          }
        }
      } else if inline.is_none() && OPTIONS_WITH_VALUES.contains(&name) && i + 1 < words.len() {
        // Skip the value so it is not mistaken for the task. Only when there is no inline
        // `=`, since that form carries its own value.
        i += 1;
      }
    } else if word != "--" {
      // The first word that is not an option, and not the separator, is the task.
      out.task = Some(word.to_string());
      out.task_position = Some(i);
    }

    i += 1;
  }

  // The separator only matters after a task, and only if the user's cursor is past it —
  // a `--` further to the right has not taken effect yet.
  if let Some(pos) = out.task_position {
    out.after_separator = words
      .iter()
      .enumerate()
      .skip(pos + 1)
      .any(|(j, word)| word == "--" && j < cword);
  }

  out
}
