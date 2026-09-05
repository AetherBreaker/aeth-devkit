//! Committing the changes `devkit setup-project` made, when the project is git-tracked.
//!
//! The commit is quiet (see `aeth_devkit_core::commit`): the committable managed files are
//! reset to their `HEAD` content before the templates are applied, the commit carries only
//! this command's changes, and the user's uncommitted edits are replayed back on top
//! afterwards — or, if they overlap the template changes, the run is rejected and rolled
//! back.

use std::path::Path;
use std::process::Command;

use anyhow::Result;

use aeth_devkit_core::commit::{self, TrackedBase};
pub use aeth_devkit_core::git::is_git_tracked;

use crate::changes::Changes;

pub const COMMIT_SUBJECT: &str = "Standardize project configuration with devkit";

/// Every committable file `setup-project` can write. Static except the Docker files: the
/// templated ones are `static_files::TARGETS`, and the compose file is wherever
/// docker-pin's discovery finds it (or `docker/compose.yaml` when it will be created). Env
/// files and `settings.local.json` are intentionally local and never committed, so they
/// are not listed. Staging a path the project lacks is a no-op.
pub fn committable(root: &Path) -> Vec<String> {
  let mut out: Vec<String> = [
    "pyproject.toml",
    ".vscode/settings.json",
    ".vscode/extensions.json",
    ".vscode/launch.json",
    ".vscode/tasks.json",
    ".gitignore",
    ".gitattributes",
    ".dockerignore",
    "AGENTS.md",
    ".claude/CLAUDE.md",
    ".claude/settings.json",
    ".github/workflows/claude.yml",
    ".github/workflows/release.yml",
    ".mcp.json",
  ]
  .iter()
  .map(|s| s.to_string())
  .collect();
  out.extend(crate::docker::static_files::TARGETS.iter().map(|t| format!("docker/{t}")));
  let compose = aeth_devkit_core::compose::find_compose_file(root)
    .ok()
    .flatten()
    .and_then(|p| p.strip_prefix(root).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
    .unwrap_or_else(|| "docker/compose.yaml".into());
  out.push(compose);
  out
}

/// Reset the committable managed files to `HEAD` (capturing the user's uncommitted state)
/// so the template merges run on clean input. Gitignored files are excluded: they are never
/// committed, so they are merged in place like the env files.
pub fn stage_bases(root: &Path) -> Result<Vec<TrackedBase>> {
  let all = committable(root);
  let paths: Vec<&str> = all.iter().map(String::as_str).filter(|rel| !is_ignored(root, rel)).collect();
  commit::stage_clean_base(root, &paths)
}

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

/// `path` named relative to `root`, or `None` when it does not live under it.
///
/// `run` works from a *canonicalized* root, so every recorded path is in that spelling —
/// which need not be the spelling a caller passes here. On Windows the same directory can
/// arrive as an 8.3 short name, with a `\\?\` prefix, or with different drive-letter case,
/// and a plain `strip_prefix` then fails on paths that are genuinely inside the repo. Trying
/// the caller's spelling first and the canonical one second keeps the "outside the
/// repository" answer meaning only that.
fn relative_to(root: &Path, path: &Path) -> Option<String> {
  let rel = path.strip_prefix(root).ok().or_else(|| {
    let canon = aeth_devkit_core::paths::strip_verbatim(std::fs::canonicalize(root).ok()?);
    path.strip_prefix(&canon).ok()
  })?;
  Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Files from `changes` that should be committed: never env files, never anything outside
/// the repository, and not gitignored.
fn trackable(root: &Path, changes: &Changes) -> Result<Vec<String>> {
  let mut out = Vec::new();
  for f in &changes.files {
    // A path that really does not sit under `root` cannot be committed to this repo — a
    // `launch.json` naming `${workspaceFolder}/../shared.env` produces one. Passing it on
    // makes `git check-ignore` exit 128 ("outside repository"), which reads as "not
    // ignored", and then `git add` refuses the whole invocation, so every run ends
    // "applied but not committed".
    let Some(rel) = relative_to(root, &f.path) else { continue };
    if is_intentionally_local(&rel) {
      continue;
    }
    if !is_ignored(root, &rel) {
      out.push(rel);
    }
  }
  Ok(out)
}

/// Commit exactly the changed, non-ignored files through the quiet-commit machinery, then
/// replay the user's uncommitted edits on top. Returns the short hash, or `None` when
/// nothing committable changed (the user's originals are back where `HEAD` copies were
/// staged). On error — most often uncommitted edits overlapping the template changes —
/// the working tree, index, and branch are restored to their pre-run state.
pub fn commit_changes(root: &Path, changes: &Changes, bases: &mut Vec<TrackedBase>) -> Result<Option<String>> {
  let files = trackable(root, changes)?;
  let mut body = String::new();
  for f in &changes.files {
    let rel = relative_to(root, &f.path).unwrap_or_default();
    if files.contains(&rel) {
      body.push_str(&format!("- {rel}: {}\n", if f.created { "created" } else { "updated" }));
    }
  }
  let message = format!("{COMMIT_SUBJECT}\n\n{body}");
  // A committable file the run created from scratch gets an "existed nowhere" base so it
  // is committed as-is; a gitignored one stays out, like it stayed out of the staging.
  let all = committable(root);
  let created: Vec<&str> = all.iter().map(String::as_str).filter(|rel| !is_ignored(root, rel)).collect();
  commit::absorb_created(root, &created, bases);
  commit::commit_or_rollback(root, bases, &message, "the template changes")
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
