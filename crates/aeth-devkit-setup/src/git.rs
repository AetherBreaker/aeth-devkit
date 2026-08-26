//! Committing the changes `devkit setup-project` made, when the project is git-tracked.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};

pub use aeth_devkit_core::git::is_git_tracked;

use crate::changes::Changes;

pub const COMMIT_SUBJECT: &str = "Standardize project configuration with devkit";

/// Env files carry secrets: never auto-commit them, even if the repo happens to track one.
fn is_env_file(rel: &str) -> bool {
  let name = rel.rsplit('/').next().unwrap_or(rel);
  name == ".env" || name.ends_with(".env")
}

/// Files from `changes` that should be committed: never env files, and not gitignored.
fn trackable(root: &Path, changes: &Changes) -> Result<Vec<String>> {
  let mut out = Vec::new();
  for f in &changes.files {
    let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
    if is_env_file(&rel) {
      continue;
    }
    let ignored = Command::new("git")
      .current_dir(root)
      .args(["check-ignore", "-q", "--", &rel])
      .status()
      .context("running git check-ignore")?
      .success();
    if !ignored {
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
