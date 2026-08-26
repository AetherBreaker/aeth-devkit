//! Thin wrappers over the `git` CLI used by every command.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

fn git(root: &Path) -> Command {
  let mut c = Command::new("git");
  c.current_dir(root);
  c
}

/// True when `root` is inside a git checkout.
pub fn is_git_tracked(root: &Path) -> bool {
  git(root)
    .args(["rev-parse", "--is-inside-work-tree"])
    .output()
    .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

/// True when any of `paths` has uncommitted changes (tracked and modified/staged, or
/// untracked but present on disk).
pub fn is_dirty(root: &Path, paths: &[&str]) -> Result<bool> {
  let status = git(root)
    .args(["diff", "--quiet", "HEAD", "--"])
    .args(paths)
    .status()
    .context("running git diff")?;
  // `git diff --quiet HEAD` exits 1 on differences; on a repo without commits it errors
  // (128), which we treat as "dirty" if any path exists.
  if status.code() == Some(1) {
    return Ok(true);
  }
  let out = git(root)
    .args(["ls-files", "--others", "--exclude-standard", "--"])
    .args(paths)
    .output()
    .context("running git ls-files")?;
  if !String::from_utf8_lossy(&out.stdout).trim().is_empty() {
    return Ok(true);
  }
  if status.code() == Some(128) {
    return Ok(paths.iter().any(|p| root.join(p).exists()));
  }
  Ok(false)
}

/// Stage exactly `paths` and commit only those paths (anything else the user staged is
/// left alone). Returns the short hash of the new commit.
pub fn commit_paths(root: &Path, paths: &[String], message: &str) -> Result<String> {
  let status = git(root).arg("add").arg("--").args(paths).status().context("running git add")?;
  if !status.success() {
    bail!("git add failed");
  }
  let out = git(root)
    .args(["commit", "--quiet", "-m", message, "--"])
    .args(paths)
    .output()
    .context("running git commit")?;
  if !out.status.success() {
    bail!("git commit failed: {}", String::from_utf8_lossy(&out.stderr).trim());
  }
  short_head(root)
}

pub fn short_head(root: &Path) -> Result<String> {
  let out = git(root).args(["rev-parse", "--short", "HEAD"]).output().context("reading HEAD")?;
  Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git init` plus the identity config needed to commit, for tests.
#[cfg(any(test, feature = "test-util"))]
pub fn init_test_repo(root: &Path) {
  for args in [
    &["init", "-q", "-b", "main"][..],
    &["config", "user.email", "test@example.com"],
    &["config", "user.name", "Test"],
    &["config", "commit.gpgsign", "false"],
  ] {
    assert!(git(root).args(args).status().unwrap().success(), "git {args:?}");
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(root: &Path, rel: &str, s: &str) {
    std::fs::write(root.join(rel), s).unwrap();
  }

  #[test]
  fn tracked_and_dirty_and_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(!is_git_tracked(root));
    init_test_repo(root);
    assert!(is_git_tracked(root));

    write(root, "a.txt", "1\n");
    assert!(is_dirty(root, &["a.txt"]).unwrap(), "untracked file counts as dirty");
    let hash = commit_paths(root, &["a.txt".into()], "first").unwrap();
    assert_eq!(hash, short_head(root).unwrap());
    assert!(!is_dirty(root, &["a.txt"]).unwrap());

    write(root, "a.txt", "2\n");
    assert!(is_dirty(root, &["a.txt"]).unwrap());
    assert!(!is_dirty(root, &["missing.txt"]).unwrap());
  }

  #[test]
  fn commit_only_listed_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    write(root, "a.txt", "a\n");
    write(root, "b.txt", "b\n");
    let _ = git(root).args(["add", "b.txt"]).status().unwrap();
    commit_paths(root, &["a.txt".into()], "only a").unwrap();
    let out = git(root).args(["show", "--name-only", "--format=", "HEAD"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "a.txt");
  }
}
