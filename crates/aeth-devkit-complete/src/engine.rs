//! The completion engine: one pure function answering every Tab press, for every shell.
//!
//! The two generated shell scripts used to carry this logic twice over, once in bash and
//! once in PowerShell, which is how they drifted. Here it exists once and the shells keep
//! only a shim. Purity is deliberate: nothing in this module touches stdout, argv or the
//! environment, so a test is `(words, cursor) -> Directive` with no process involved.

use std::path::PathBuf;

use aeth_devkit_core::process::Runner;

use crate::{cache, parse};

/// Which shell asked. The engine is shell-agnostic in its logic; this exists only because
/// PowerShell renders [`ItemKind`] and bash discards it, and because the two hand their
/// command line over in different shapes before reaching this module.
///
/// `Copy` because it is two bytes at most — passing it by value is cheaper than a reference,
/// and `derive` gives us that for a fieldless enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
  Bash,
  PowerShell,
}

/// How a completion should be presented. bash ignores this entirely (its completion model
/// is a flat list of words); PowerShell maps it onto `CompletionResultType`, which is what
/// produces the icon and grouping in its popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
  /// A poe task — the thing you are actually trying to run.
  Command,
  /// An option name such as `--verbose` or `--mode`.
  Param,
  /// A value for an option or positional, e.g. one of a `choices` list.
  Value,
}

impl ItemKind {
  /// The token written into the wire format's fourth column. Kept next to the enum so the
  /// spelling can never drift from the shims that parse it.
  pub fn as_wire(self) -> &'static str {
    match self {
      ItemKind::Command => "command",
      ItemKind::Param => "param",
      ItemKind::Value => "value",
    }
  }
}

/// One completion candidate.
///
/// `value` and `display` differ only for inline `--opt=value` completion, where the whole
/// word must be replaced (`--mode=fast`) but the user should see just the part they are
/// choosing (`fast`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
  /// Text inserted into the command line.
  pub value: String,
  /// Text shown in the completion popup.
  pub display: String,
  /// Longer help shown alongside, where the shell supports it.
  pub tooltip: String,
  pub kind: ItemKind,
}

impl Item {
  /// The common case: the candidate shows exactly what it inserts.
  ///
  /// Takes `&str` rather than `String` because every call site has a borrowed name and would
  /// otherwise have to allocate before calling; the allocation happens once, here.
  pub(crate) fn plain(value: &str, tooltip: &str, kind: ItemKind) -> Self {
    Self {
      value: value.to_string(),
      display: value.to_string(),
      tooltip: tooltip.to_string(),
      kind,
    }
  }
}

/// What the shim should do with the answer.
///
/// An enum rather than a list-plus-flags because the cases are genuinely exclusive: either
/// the engine has candidates, or it is declining and handing path completion back to the
/// shell. Encoding that as a type means no caller can accidentally read items that are not
/// there — the compiler makes them handle the sentinel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
  /// Concrete candidates, already filtered by the request's prefix.
  Items(Vec<Item>),
  /// "Complete directories here" — the shell does it, keeping its own quoting rules.
  Dirs,
  /// "Complete files here", likewise.
  Files,
}

/// Everything the engine needs to answer one Tab press.
///
/// Both shells normalise into this before calling: bash by splitting its raw line, PowerShell
/// by forwarding the elements its own parser already produced.
#[derive(Debug, Clone)]
pub struct Request {
  pub shell: Shell,
  /// argv-style words, where `words[0]` is always the command itself (`poe`).
  pub words: Vec<String>,
  /// Index of the word being completed. Equals `words.len()` when the user has typed a
  /// space and is starting a fresh word, so every read of this must be bounds-checked.
  pub cword: usize,
  /// The text of that word to the left of the cursor: what candidates must start with.
  pub prefix: String,
  /// Project directory to resolve tasks from.
  pub root: PathBuf,
}

/// The word immediately left of the cursor, if there is one.
///
/// `checked_sub` returns `None` rather than underflowing when the cursor is on word 0, and
/// `and_then` chains that into the lookup so the whole thing is one expression with no
/// manual bounds check.
fn previous_word(req: &Request) -> Option<&str> {
  req.cword.checked_sub(1).and_then(|i| req.words.get(i)).map(String::as_str)
}

/// Whether the cursor is positioned to supply a value for one of `opts`.
///
/// Two shapes count: the previous word is one of the options (`-C |`), or the word being
/// typed is the inline form (`-C=sr|`).
fn supplying_value_for(req: &Request, opts: &[&str]) -> bool {
  if let Some((name, _)) = req.prefix.split_once('=') {
    return opts.contains(&name);
  }
  previous_word(req).is_some_and(|w| opts.contains(&w))
}

/// Candidates for `-e` / `--executor`, in both the separate and inline spellings.
///
/// Returns `None` when the cursor is not on an executor value at all, which lets the caller
/// distinguish "not my branch" from "my branch, no matches".
fn executor_items(req: &Request) -> Option<Vec<Item>> {
  const EXEC_OPTS: &[&str] = &["-e", "--executor"];

  // Inline form: the whole word must be replaced, so each candidate carries the option name.
  if let Some((name, typed)) = req.prefix.split_once('=') {
    if !EXEC_OPTS.contains(&name) {
      return None;
    }
    return Some(
      parse::EXECUTORS
        .iter()
        .filter(|e| e.starts_with(typed))
        .map(|e| Item {
          value: format!("{name}={e}"),
          display: (*e).to_string(),
          tooltip: format!("Executor: {e}"),
          kind: ItemKind::Value,
        })
        .collect(),
    );
  }

  if !previous_word(req).is_some_and(|w| EXEC_OPTS.contains(&w)) {
    return None;
  }
  Some(
    parse::EXECUTORS
      .iter()
      .filter(|e| e.starts_with(&req.prefix))
      .map(|e| Item::plain(e, &format!("Executor: {e}"), ItemKind::Value))
      .collect(),
  )
}

/// poe's own options, minus any made redundant by one already on the line.
fn global_items(req: &Request) -> Vec<Item> {
  // Gather every spelling suppressed by something already typed. A `Vec` is fine over a
  // `HashSet`: the list is 17 entries, so a linear scan beats hashing.
  let mut excluded: Vec<&str> = Vec::new();
  for (i, word) in req.words.iter().enumerate() {
    // Skip the word being completed — it is a half-typed `-`, not a decision.
    if i == req.cword {
      continue;
    }
    // Match on the base name so `-e=uv` counts as `-e` having been used.
    let base = word.split_once('=').map_or(word.as_str(), |(n, _)| n);
    excluded.extend_from_slice(parse::exclusions(base));
  }

  parse::GLOBAL_OPTIONS
    .iter()
    .filter(|opt| opt.starts_with(&req.prefix) && !excluded.contains(*opt))
    .map(|opt| Item::plain(opt, &format!("Global option: {opt}"), ItemKind::Param))
    .collect()
}

/// Answer one completion request.
///
/// `runner` and `no_cache` are threaded through to [`cache::resolve_cached`] rather than
/// read from globals, which is what lets tests substitute a recording runner and bypass the
/// on-disk cache.
///
/// Branch order mirrors poe's own scripts, and it matters: the separator wins over
/// everything, values win over names, and task arguments are only reached once a task is
/// actually on the line.
pub fn complete(req: &Request, runner: &dyn Runner, no_cache: bool) -> Directive {
  let parsed = parse::parse(&req.words, req.cword);

  // Past `--`, every remaining word is handed to the task itself, so only paths make sense.
  if parsed.after_separator {
    return Directive::Files;
  }

  // `-C <cursor>` and `-C=<cursor>` both want directories. The engine declines and the
  // shell enumerates them, keeping its own quoting and trailing-separator behaviour.
  if supplying_value_for(req, parse::DIRECTORY_OPTIONS) {
    return Directive::Dirs;
  }

  if let Some(items) = executor_items(req) {
    return Directive::Items(items);
  }

  // `is_none_or` reads as "no task yet, or the cursor has not passed it" — with nothing
  // typed the cursor is necessarily still in global-option territory, which is the `None`
  // arm's `true`.
  let before_task = parsed.task_position.is_none_or(|pos| req.cword <= pos);

  if before_task {
    if req.prefix.starts_with('-') {
      return Directive::Items(global_items(req));
    }

    // A completer must never fail loudly, so an unresolvable project simply has nothing to
    // offer rather than surfacing an error over the user's prompt.
    let Ok(resolved) = cache::resolve_cached(&req.root, runner, no_cache) else {
      return Directive::Items(Vec::new());
    };
    let items = resolved
      .tasks
      .iter()
      .filter(|t| t.name.starts_with(&req.prefix))
      .map(|t| Item::plain(&t.name, &format!("Task: {}", t.name), ItemKind::Command))
      .collect();
    return Directive::Items(items);
  }

  // Past the task: its own arguments. Filled in by the next task.
  Directive::Items(Vec::new())
}
