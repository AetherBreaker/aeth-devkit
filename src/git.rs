//! Committing the changes `sft-setup` made, when the project is git-tracked.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::changes::Changes;

pub const COMMIT_SUBJECT: &str = "Standardize project configuration with sft-setup";

fn git(root: &Path) -> Command {
  let mut c = Command::new("git");
  c.current_dir(root);
  c
}

/// True when `root` is inside a git checkout (i.e. the project is git-tracked).
pub fn is_git_tracked(root: &Path) -> bool {
  git(root)
    .args(["rev-parse", "--is-inside-work-tree"])
    .output()
    .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

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
    let ignored = git(root)
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

/// Stage exactly the changed, non-ignored files and commit them (only those paths, so
/// anything the user had staged beforehand is left alone). Returns the short hash, or
/// `None` when nothing trackable changed.
pub fn commit_changes(root: &Path, changes: &Changes) -> Result<Option<String>> {
  let files = trackable(root, changes)?;
  if files.is_empty() {
    return Ok(None);
  }
  let status = git(root).arg("add").arg("--").args(&files).status().context("running git add")?;
  if !status.success() {
    bail!("git add failed");
  }
  let mut body = String::new();
  for f in &changes.files {
    let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
    if files.contains(&rel) {
      body.push_str(&format!("- {rel}: {}\n", if f.created { "created" } else { "updated" }));
    }
  }
  let message = format!("{COMMIT_SUBJECT}\n\n{body}");
  let out = git(root)
    .args(["commit", "--quiet", "-m", &message, "--"])
    .args(&files)
    .output()
    .context("running git commit")?;
  if !out.status.success() {
    bail!("git commit failed: {}", String::from_utf8_lossy(&out.stderr).trim());
  }
  let hash = git(root).args(["rev-parse", "--short", "HEAD"]).output().context("reading HEAD")?;
  Ok(Some(String::from_utf8_lossy(&hash.stdout).trim().to_string()))
}
