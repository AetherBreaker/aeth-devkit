//! Committing the changes `devkit setup-project` made, when the project is git-tracked.

use std::path::Path;
use std::process::Command;

use anyhow::Result;

pub use aeth_devkit_core::git::is_git_tracked;

use crate::changes::Changes;

pub const COMMIT_SUBJECT: &str = "Standardize project configuration with devkit";

/// Env files carry secrets: never auto-commit them, and never un-ignore them, even if the
/// repo happens to track one. A `launch.json` `envFile` may name any of the three spellings
/// in the wild, so all three count: `.env` itself, the `.env.<suffix>` family
/// (`.env.local`, `.env.production`), and the `<name>.env` form (`prod.env`). Missing one
/// is not cosmetic — step 13 would rewrite the project's own `.gitignore` to expose it.
pub fn is_env_file(rel: &str) -> bool {
  let name = rel.rsplit('/').next().unwrap_or(rel);
  name == ".env" || name.ends_with(".env") || name.starts_with(".env.")
}

/// Managed files that are *supposed* to stay out of git, so neither the commit nor the
/// "this is gitignored" warning should ever consider them: env files carry secrets, and
/// `.claude/settings.local.json` holds absolute paths and this machine's venv layout.
pub fn is_intentionally_local(rel: &str) -> bool {
  is_env_file(rel) || rel == ".claude/settings.local.json"
}

/// Whether git would ignore `rel` (a root-relative, `/`-separated path). The file need
/// not exist: `check-ignore` matches patterns, so this also answers for a dry run.
pub fn is_ignored(root: &Path, rel: &str) -> bool {
  Command::new("git")
    .current_dir(root)
    .args(["check-ignore", "-q", "--", rel])
    .status()
    .is_ok_and(|s| s.success())
}

/// Files from `changes` that should be committed: never env files, never anything outside
/// the repository, and not gitignored.
fn trackable(root: &Path, changes: &Changes) -> Result<Vec<String>> {
  let mut out = Vec::new();
  for f in &changes.files {
    // A path that does not sit under `root` cannot be committed to this repo — a
    // `launch.json` naming `${workspaceFolder}/../shared.env` produces one. Passing it on
    // makes `git check-ignore` exit 128 ("outside repository"), which reads as "not
    // ignored", and then `git add` refuses the whole invocation, so every run ends
    // "applied but not committed".
    let Ok(rel) = f.path.strip_prefix(root) else { continue };
    let rel = rel.to_string_lossy().replace('\\', "/");
    if is_intentionally_local(&rel) {
      continue;
    }
    if !is_ignored(root, &rel) {
      out.push(rel);
    }
  }
  Ok(out)
}

/// Stage exactly the changed, non-ignored files and commit them. Returns the short hash,
/// or `None` when nothing trackable changed.
pub fn commit_changes(root: &Path, changes: &Changes) -> Result<Option<String>> {
  let files = trackable(root, changes)?;
  if files.is_empty() {
    return Ok(None);
  }
  let mut body = String::new();
  for f in &changes.files {
    let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
    if files.contains(&rel) {
      body.push_str(&format!("- {rel}: {}\n", if f.created { "created" } else { "updated" }));
    }
  }
  let message = format!("{COMMIT_SUBJECT}\n\n{body}");
  aeth_devkit_core::git::commit_paths(root, &files, &message).map(Some)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn env_file_covers_every_spelling_a_launch_json_can_name() {
    for rel in [
      ".env",
      ".env.local",
      ".env.production",
      ".env.dev.local",
      "prod.env",
      "sub/dir/.env.local",
    ] {
      assert!(is_env_file(rel), "{rel} must be treated as an env file");
    }
  }

  #[test]
  fn env_file_does_not_over_match() {
    for rel in ["settings.json", ".environment", "env", ".envrc"] {
      assert!(!is_env_file(rel), "{rel} must not be treated as an env file");
    }
  }

  #[test]
  fn env_file_fails_closed_on_the_dot_env_prefix() {
    // `.env.md` is a doc, not secrets, but the predicate only ever *excludes* a file from
    // being committed or un-ignored. Over-matching costs nothing (devkit manages no such
    // file); under-matching writes a secret into git, so the prefix rule stays broad.
    assert!(is_env_file(".env.md"));
  }
}
