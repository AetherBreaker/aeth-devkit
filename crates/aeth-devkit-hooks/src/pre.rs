//! PreToolUse hooks: pure decisions on the payload, no processes spawned.

use std::path::Path;

use serde_json::{Value, json};

/// Files Claude must never edit directly. `.env` holds live credentials; `uv.lock` is owned
/// by uv and hand-edits corrupt it. Matched on the basename so nested copies are covered too.
const PROTECTED_NAMES: [&str; 2] = [".env", "uv.lock"];

/// Build the JSON Claude understands as "refuse this tool call, and here is why".
fn deny(reason: String) -> Value {
  json!({
    "hookSpecificOutput": {
      "hookEventName": "PreToolUse",
      "permissionDecision": "deny",
      "permissionDecisionReason": reason,
    }
  })
}

/// `pre-edit-protect`: deny edits to protected files.
pub fn edit_protect(file_path: &str) -> Option<Value> {
  // `Path::file_name` returns `Option<&OsStr>`; `to_str` narrows to `Option<&str>`. The `?`
  // on an `Option` inside a function returning `Option` early-returns `None` — exactly the
  // "no file name, no opinion" behaviour we want, without an `if let` ladder.
  let name = Path::new(file_path).file_name()?.to_str()?;
  if !PROTECTED_NAMES.contains(&name) {
    return None;
  }
  Some(deny(format!(
    "{name} is protected. .env holds live credentials and uv.lock is managed by uv — ask the user to make this change directly."
  )))
}

/// `pre-bash-protect-deps`: deny `uv add|remove|lock` — dependency changes are the user's
/// call, not Claude's.
pub fn bash_protect_deps(command: &str) -> Option<Value> {
  // `std::sync::LazyLock` compiles the regex once, on first use, and every later call reuses
  // it. A hook runs once per process, so this is mostly about keeping the pattern next to
  // the code that owns it rather than about speed.
  //
  // The pattern: the banned verb must be at the start of a command, or right after a shell
  // separator (`;`, `&`, `|`, newline) or the `then` keyword — so `cd x && uv lock` is
  // caught, but `echo uv-add` is not (the trailing `\b` needs a word boundary after `add`).
  static BANNED: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?:^|[;&|\n]|\bthen\b)\s*uv\s+(?:add|remove|lock)\b").unwrap());
  if command.is_empty() || !BANNED.is_match(command) {
    return None;
  }
  Some(deny(format!(
    "Dependency changes (uv add/remove/lock) must be run by the user, not Claude. Ask them to run: {}",
    command.trim()
  )))
}
