//! The completion engine: one pure function answering every Tab press, for every shell.
//!
//! The two generated shell scripts used to carry this logic twice over, once in bash and
//! once in PowerShell, which is how they drifted. Here it exists once and the shells keep
//! only a shim. Purity is deliberate: nothing in this module touches stdout, argv or the
//! environment, so a test is `(words, cursor) -> Directive` with no process involved.

use std::path::PathBuf;

use aeth_devkit_core::process::Runner;

use crate::cache;

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

/// Answer one completion request.
///
/// `runner` and `no_cache` are threaded through to [`cache::resolve_cached`] rather than
/// read from globals, which is what lets tests substitute a recording runner and bypass the
/// on-disk cache.
pub fn complete(req: &Request, runner: &dyn Runner, no_cache: bool) -> Directive {
  // `let ... else` binds on success and diverges on failure, which reads better than a
  // `match` when the failure arm is a single early return. A completer must never surface an
  // error — an unresolvable project simply has nothing to offer.
  let Ok(resolved) = cache::resolve_cached(&req.root, runner, no_cache) else {
    return Directive::Items(Vec::new());
  };

  // Iterator chain rather than a loop: `filter` keeps only names the user's prefix matches,
  // `map` turns each into an `Item`, and `collect` decides the target type from the binding.
  let items = resolved
    .tasks
    .iter()
    .filter(|t| t.name.starts_with(&req.prefix))
    .map(|t| Item::plain(&t.name, &format!("Task: {}", t.name), ItemKind::Command))
    .collect();

  Directive::Items(items)
}
