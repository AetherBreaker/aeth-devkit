//! PreToolUse hooks: pure decisions on the payload, no processes spawned.

use std::path::Path;

use serde_json::{Value, json};

/// Files Claude must never edit directly. `.env` holds live credentials; `uv.lock` is owned
/// by uv and hand-edits corrupt it. Matched on the basename so nested copies are covered too.
const PROTECTED_NAMES: [&str; 2] = [".env", "uv.lock"];

/// Subcommands that change the project's dependencies. `uv lock` is included because it
/// rewrites `uv.lock`, which is the same decision by another route.
const BANNED_UV_SUBCOMMANDS: [&str; 3] = ["add", "remove", "lock"];

/// Words that may sit in front of the real program without changing which program runs.
/// `sudo` is deliberately absent: it takes its own flags, and nothing here should be
/// encouraging a sudo'd `uv add` in the first place.
const WRAPPERS: [&str; 5] = ["time", "env", "command", "builtin", "exec"];

/// Shells that run their `-c` argument as a command line of their own.
const SHELLS: [&str; 5] = ["bash", "sh", "zsh", "dash", "ksh"];

/// uv's global options that consume the *following* token, so it is not the subcommand.
/// A flag written as `--project=x` carries its value already and needs no entry here.
const UV_VALUE_FLAGS: [&str; 8] = [
  "--directory",
  "--project",
  "--python",
  "--with",
  "--index",
  "--default-index",
  "--config-file",
  "--cache-dir",
];

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

/// Normalize a basename the way the filesystem would before comparing it to the protected
/// list.
///
/// Windows resolves `.ENV`, `.env `, and `.env.` all to the same file as `.env`, so a guard
/// that compares raw bytes is bypassed by a capital letter or a trailing dot — and the write
/// still lands on the credentials. Normalizing costs a false positive on a genuinely
/// distinct `.ENV` on Linux, which is the safe direction for a guard to be wrong in.
fn normalized_name(file_path: &str) -> Option<String> {
  // `Path::file_name` returns `Option<&OsStr>`; `to_str` narrows to `Option<&str>`. The `?`
  // on an `Option` inside a function returning `Option` early-returns `None` — exactly the
  // "no file name, no opinion" behaviour we want, without an `if let` ladder.
  let name = Path::new(file_path).file_name()?.to_str()?;
  Some(name.trim_end_matches(['.', ' ']).to_ascii_lowercase())
}

/// `pre-edit-protect`: deny edits to protected files.
pub fn edit_protect(file_path: &str) -> Option<Value> {
  let name = normalized_name(file_path)?;
  if !PROTECTED_NAMES.contains(&name.as_str()) {
    return None;
  }
  Some(deny(format!(
    "{name} is protected. .env holds live credentials and uv.lock is managed by uv — ask the user to make this change directly."
  )))
}

/// `pre-bash-protect-deps`: deny `uv add|remove|lock` — dependency changes are the user's
/// call, not Claude's.
///
/// This works on the *tokens* of the command line rather than on its raw text. A regex over
/// raw text gets this wrong in both directions: it misses `(uv add x)`, `uv --directory . add
/// x` and `env FOO=1 uv add x`, while denying `git commit -m "wip; uv add later"` because the
/// `;` inside a quoted string looks like a separator. Splitting into commands first, with
/// quotes respected, fixes both halves at once.
pub fn bash_protect_deps(command: &str) -> Option<Value> {
  if command.trim().is_empty() {
    return None;
  }
  if !split_commands(command).iter().any(|tokens| runs_banned_uv(tokens)) {
    return None;
  }
  Some(deny(format!(
    "Dependency changes (uv add/remove/lock) must be run by the user, not Claude. Ask them to run: {}",
    command.trim()
  )))
}

/// Split a command line into the individual commands it runs, each as a token list.
///
/// This is deliberately not a shell parser. It tracks quoting so that separators inside a
/// string are inert, and treats everything that can *start a new command* — `;`, `&`, `|`,
/// newline, and the grouping characters `( ) { }` — as a boundary. Anything subtler than
/// that (here-docs, process substitution) degrades to "more boundaries than a shell would
/// find", which splits a command into fragments and can only ever cause a miss, never a
/// false deny on unrelated text.
fn split_commands(command: &str) -> Vec<Vec<String>> {
  let mut commands: Vec<Vec<String>> = Vec::new();
  let mut tokens: Vec<String> = Vec::new();
  let mut current = String::new();
  // `None` outside a quoted run; `Some(q)` while inside one opened by `q`.
  let mut quote: Option<char> = None;
  let mut chars = command.chars().peekable();

  // Close off the token being built, then the command being built.
  // Double braces make each macro expand to a *block expression*, so it is legal in the
  // match arms below; a bare statement body would be a syntax error there.
  macro_rules! end_token {
    () => {{
      if !current.is_empty() {
        tokens.push(std::mem::take(&mut current));
      }
    }};
  }
  macro_rules! end_command {
    () => {{
      end_token!();
      if !tokens.is_empty() {
        commands.push(std::mem::take(&mut tokens));
      }
    }};
  }

  while let Some(c) = chars.next() {
    match quote {
      // Inside single quotes nothing is special but the closing quote; inside double quotes
      // a backslash still escapes the next character.
      Some(q) => {
        if c == '\\' && q == '"' {
          if let Some(next) = chars.next() {
            current.push(next);
          }
        } else if c == q {
          quote = None;
        } else {
          current.push(c);
        }
      }
      None => match c {
        '\'' | '"' => quote = Some(c),
        // A backslash outside quotes escapes one character, including a newline.
        '\\' => {
          if let Some(next) = chars.next() {
            current.push(next);
          }
        }
        // `$(` opens a command substitution: the `$` is dropped and `(` starts a command.
        '$' if chars.peek() == Some(&'(') => {
          chars.next();
          end_command!();
        }
        ';' | '&' | '|' | '\n' | '(' | ')' | '{' | '}' | '`' => end_command!(),
        c if c.is_whitespace() => end_token!(),
        _ => current.push(c),
      },
    }
  }
  end_command!();
  commands
}

/// Whether one command's tokens run a banned `uv` subcommand.
fn runs_banned_uv(tokens: &[String]) -> bool {
  // Skip anything in front of the program that does not change which program runs:
  // `FOO=1 uv add x`, `env uv add x`, `time uv add x`, and shell keywords like `then`/`do`
  // that a split leaves at the head of the fragment.
  let mut rest = tokens;
  while let Some((first, tail)) = rest.split_first() {
    let bare = program_name(first);
    // `FOO=1` — an assignment prefix, recognised by an `=` before any `/`.
    let is_assignment = first.split_once('=').is_some_and(|(k, _)| !k.is_empty() && !k.contains('/'));
    if is_assignment || WRAPPERS.contains(&bare.as_str()) || matches!(bare.as_str(), "then" | "do" | "else" | "!") {
      rest = tail;
      continue;
    }
    break;
  }

  let Some((program, args)) = rest.split_first() else {
    return false;
  };
  let program = program_name(program);

  // `bash -c "<command line>"` runs its argument as a command line of its own, so recurse
  // into it rather than treating it as an opaque string.
  if SHELLS.contains(&program.as_str()) {
    if let Some(script) = args.iter().position(|a| a == "-c").and_then(|i| args.get(i + 1)) {
      return split_commands(script).iter().any(|t| runs_banned_uv(t));
    }
    return false;
  }

  if program != "uv" {
    return false;
  }

  // Walk uv's own global options to find the subcommand. A value-taking flag consumes the
  // token after it, which is how `uv --directory . add requests` hides `add` from a naive
  // "second token" check.
  let mut it = args.iter();
  while let Some(arg) = it.next() {
    if UV_VALUE_FLAGS.contains(&arg.as_str()) {
      it.next();
      continue;
    }
    if arg.starts_with('-') {
      continue;
    }
    return BANNED_UV_SUBCOMMANDS.contains(&arg.as_str());
  }
  false
}

/// The bare program name for a token: the last path segment, minus a Windows `.exe`, folded
/// to lower case. `./.venv/Scripts/uv.exe` and `/usr/bin/uv` are both `uv`.
fn program_name(token: &str) -> String {
  let base = token.rsplit(['/', '\\']).next().unwrap_or(token);
  base.strip_suffix(".exe").unwrap_or(base).to_ascii_lowercase()
}
