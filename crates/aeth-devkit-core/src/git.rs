//! Thin wrappers over the `git` CLI used by every command.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::process::{CapturedOutput, Runner};

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

// ---------------------------------------------------------------------------------------
// Local helpers: these only touch the repository on disk, so they call `git` directly.
// ---------------------------------------------------------------------------------------

/// Run `git args` in `root`, capturing output. Shared plumbing for the local helpers.
fn capture(root: &Path, args: &[&str]) -> Result<CapturedOutput> {
  let out = git(root)
    .args(args)
    .output()
    .with_context(|| format!("running git {}", args.join(" ")))?;
  Ok(CapturedOutput {
    code: out.status.code(),
    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
  })
}

/// Turn a captured result into `Ok(trimmed stdout)` or an error carrying git's stderr.
/// Taking `CapturedOutput` by value (not `&`) is fine: the caller has no further use for it,
/// and it lets us move `stdout` out without cloning.
fn expect_ok(out: CapturedOutput, what: &str) -> Result<String> {
  if out.success() {
    Ok(out.stdout.trim_end().to_string())
  } else {
    bail!("{what} failed: {}", out.stderr.trim())
  }
}

/// The full 40-character SHA of `HEAD`.
pub fn head_sha(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["rev-parse", "HEAD"])?, "git rev-parse HEAD")
}

/// The checked-out branch name (`HEAD` when detached).
pub fn current_branch(root: &Path) -> Result<String> {
  expect_ok(
    capture(root, &["rev-parse", "--abbrev-ref", "HEAD"])?,
    "git rev-parse --abbrev-ref HEAD",
  )
}

/// Machine-readable status; empty when the tree is clean.
pub fn status_porcelain(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["status", "--porcelain"])?, "git status")
}

/// Human-readable short status, for showing the user what is dirty.
pub fn status_short(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["status", "--short"])?, "git status")
}

/// The short SHA of the commit `tag` points at, or `None` when the tag does not exist.
/// `^{commit}` peels an annotated tag object down to its commit.
pub fn tag_target(root: &Path, tag: &str) -> Result<Option<String>> {
  let rev = format!("refs/tags/{tag}^{{commit}}");
  let out = capture(root, &["rev-parse", "--short", "--verify", "--quiet", &rev])?;
  // `bool::then` builds `Some(closure())` on true and `None` on false — exactly the
  // "exists → value" mapping we want for a missing-tag probe.
  Ok(out.success().then(|| out.stdout.trim().to_string()))
}

/// `git tag -a <tag> -m <message>`.
pub fn create_annotated_tag(root: &Path, tag: &str, message: &str) -> Result<()> {
  // `.map(|_| ())` discards the (empty) stdout so the function returns `Result<()>`.
  expect_ok(capture(root, &["tag", "-a", tag, "-m", message])?, "git tag").map(|_| ())
}

/// `git tag -d <tag>`; an error if the tag does not exist.
pub fn delete_tag(root: &Path, tag: &str) -> Result<()> {
  expect_ok(capture(root, &["tag", "-d", tag])?, "git tag -d").map(|_| ())
}

/// `git reset --mixed <rev>`: move `HEAD` and the index, keep the working tree.
pub fn reset_mixed_to(root: &Path, rev: &str) -> Result<()> {
  expect_ok(capture(root, &["reset", "--mixed", "--quiet", rev])?, "git reset").map(|_| ())
}

/// Drop a commit while leaving the rest of the index alone: `git reset --soft <rev>` moves
/// `HEAD` without touching the index or working tree, then `git reset <rev> -- <paths>`
/// puts *only* the listed index entries back to what `rev` has. Anything else the user had
/// staged before the run stays staged — which a plain `--mixed` reset would silently
/// unstage. `paths` are the files the dropped commit touched.
pub fn reset_commit_keeping_index(root: &Path, rev: &str, paths: &[String]) -> Result<()> {
  expect_ok(capture(root, &["reset", "--soft", "--quiet", rev])?, "git reset --soft")?;
  if paths.is_empty() {
    return Ok(());
  }
  // Build `["reset", "--quiet", rev, "--", p1, p2, …]` as `&str`s for `capture`.
  let mut args = vec!["reset", "--quiet", rev, "--"];
  args.extend(paths.iter().map(String::as_str));
  expect_ok(capture(root, &args)?, "git reset -- <paths>").map(|_| ())
}

// ---------------------------------------------------------------------------------------
// Remote helpers: these may talk to `origin`, so they go through the injected `Runner` and
// tests can script them instead of needing a network.
// ---------------------------------------------------------------------------------------

/// Run `git args` through `runner`. The `&[&str]` → `Vec<String>` conversion exists only
/// because the `Runner` trait takes owned strings (so recorded calls own their data).
fn remote(runner: &dyn Runner, root: &Path, args: &[&str]) -> Result<CapturedOutput> {
  let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
  runner.run_capture("git", &owned, root)
}

/// `git fetch --quiet origin`.
pub fn fetch(runner: &dyn Runner, root: &Path) -> Result<()> {
  expect_ok(remote(runner, root, &["fetch", "--quiet", "origin"])?, "git fetch").map(|_| ())
}

/// The upstream branch (e.g. `origin/main`), or `None` when the branch has none.
pub fn upstream(runner: &dyn Runner, root: &Path) -> Result<Option<String>> {
  let out = remote(runner, root, &["rev-parse", "--abbrev-ref", "@{u}"])?;
  Ok(out.success().then(|| out.stdout.trim().to_string()))
}

/// How many commits the upstream has that `HEAD` does not.
pub fn behind_count(runner: &dyn Runner, root: &Path) -> Result<u32> {
  let s = expect_ok(remote(runner, root, &["rev-list", "--count", "HEAD..@{u}"])?, "git rev-list")?;
  // `str::parse` infers the target type (`u32`) from the function's return type.
  s.trim().parse().with_context(|| format!("parsing rev-list count {s:?}"))
}

/// Whether `origin` has `refs/tags/<tag>`. `ls-remote` prints one line per matching ref.
pub fn remote_tag_exists(runner: &dyn Runner, root: &Path, tag: &str) -> Result<bool> {
  let refname = format!("refs/tags/{tag}");
  let s = expect_ok(remote(runner, root, &["ls-remote", "--tags", "origin", &refname])?, "git ls-remote")?;
  Ok(!s.trim().is_empty())
}

/// `git push --atomic origin <refs…>` — one push, and `--atomic` makes the server update
/// all of the refs or none of them. Without it a multi-ref push is *not* all-or-nothing:
/// the branch could land while the tag is rejected, and a failure would leave the remote
/// half-done with no undo queued for the branch.
pub fn push_refs(runner: &dyn Runner, root: &Path, refs: &[&str]) -> Result<()> {
  let mut args = vec!["push", "--atomic", "origin"];
  args.extend_from_slice(refs);
  expect_ok(remote(runner, root, &args)?, "git push").map(|_| ())
}

/// `git push origin --delete <tag>`. Treats "already gone" as success, because a preceding
/// `gh release delete --cleanup-tag` may have removed it for us.
pub fn delete_remote_tag(runner: &dyn Runner, root: &Path, tag: &str) -> Result<()> {
  let out = remote(runner, root, &["push", "origin", "--delete", tag])?;
  if out.success() || out.stderr.contains("remote ref does not exist") {
    Ok(())
  } else {
    bail!("git push --delete {tag} failed: {}", out.stderr.trim())
  }
}

/// Rewind `origin/<branch>` to `new_sha`, but only if the remote is still at `expected_sha`.
/// `--force-with-lease` is the safety catch: if someone else pushed in the meantime the
/// remote is no longer what we expect and git refuses instead of clobbering their work.
pub fn force_push_with_lease(runner: &dyn Runner, root: &Path, branch: &str, expected_sha: &str, new_sha: &str) -> Result<()> {
  let lease = format!("--force-with-lease={branch}:{expected_sha}");
  let refspec = format!("{new_sha}:refs/heads/{branch}");
  expect_ok(
    remote(runner, root, &["push", &lease, "origin", &refspec])?,
    "git push --force-with-lease",
  )
  .map(|_| ())
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
  #[test]
  fn local_helpers_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    write(root, "a.txt", "1\n");
    commit_paths(root, &["a.txt".into()], "first").unwrap();
    let first = head_sha(root).unwrap();
    assert_eq!(first.len(), 40);
    assert_eq!(current_branch(root).unwrap(), "main");
    assert_eq!(status_porcelain(root).unwrap(), "");
    write(root, "b.txt", "x\n");
    assert!(status_porcelain(root).unwrap().contains("b.txt"));
    assert!(status_short(root).unwrap().contains("b.txt"));
    std::fs::remove_file(root.join("b.txt")).unwrap();

    assert_eq!(tag_target(root, "v1.0.0").unwrap(), None);
    create_annotated_tag(root, "v1.0.0", "Version 1.0.0").unwrap();
    assert_eq!(
      tag_target(root, "v1.0.0").unwrap().as_deref(),
      Some(short_head(root).unwrap().as_str())
    );
    delete_tag(root, "v1.0.0").unwrap();
    assert_eq!(tag_target(root, "v1.0.0").unwrap(), None);
    assert!(delete_tag(root, "v1.0.0").is_err());

    write(root, "a.txt", "2\n");
    commit_paths(root, &["a.txt".into()], "second").unwrap();
    reset_mixed_to(root, &first).unwrap();
    assert_eq!(head_sha(root).unwrap(), first);
    // Mixed reset keeps the working tree: the file still has the newer content.
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "2\n");
  }

  #[test]
  fn reset_keeping_index_preserves_unrelated_staged_changes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    write(root, "a.txt", "1\n");
    write(root, "other.txt", "x\n");
    commit_paths(root, &["a.txt".into(), "other.txt".into()], "first").unwrap();
    let first = head_sha(root).unwrap();
    // The user stages an unrelated change, then a "bump" commit touches only a.txt.
    write(root, "other.txt", "y\n");
    assert!(git(root).args(["add", "other.txt"]).status().unwrap().success());
    write(root, "a.txt", "2\n");
    commit_paths(root, &["a.txt".into()], "bump").unwrap();

    reset_commit_keeping_index(root, &first, &["a.txt".into()]).unwrap();
    assert_eq!(head_sha(root).unwrap(), first);
    // `a.txt` is no longer staged (index matches `first`), but `other.txt` still is.
    let staged = String::from_utf8(git(root).args(["diff", "--cached", "--name-only"]).output().unwrap().stdout).unwrap();
    assert_eq!(staged.trim(), "other.txt");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "2\n");
  }

  #[test]
  fn remote_helpers_go_through_runner() {
    use crate::process::RecordingRunner;
    let r = RecordingRunner::new(0);
    let root = Path::new(".");
    r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
    r.script("git", &["rev-list", "--count"], 0, "2\n");
    r.script("git", &["ls-remote"], 0, "abc\trefs/tags/v1.0.0\n");
    fetch(&r, root).unwrap();
    assert_eq!(upstream(&r, root).unwrap().as_deref(), Some("origin/main"));
    assert_eq!(behind_count(&r, root).unwrap(), 2);
    assert!(remote_tag_exists(&r, root, "v1.0.0").unwrap());
    push_refs(&r, root, &["main", "v1.0.0"]).unwrap();
    delete_remote_tag(&r, root, "v1.0.0").unwrap();
    force_push_with_lease(&r, root, "main", "aaa", "bbb").unwrap();
    let git = r.calls_for("git");
    assert_eq!(git[0], vec!["fetch", "--quiet", "origin"]);
    assert_eq!(git[4], vec!["push", "--atomic", "origin", "main", "v1.0.0"]);
    assert_eq!(git[5], vec!["push", "origin", "--delete", "v1.0.0"]);
    assert_eq!(git[6], vec!["push", "--force-with-lease=main:aaa", "origin", "bbb:refs/heads/main"]);

    let failing = RecordingRunner::new(1);
    assert_eq!(upstream(&failing, root).unwrap(), None);
    assert!(push_refs(&failing, root, &["main"]).is_err());
  }
}
