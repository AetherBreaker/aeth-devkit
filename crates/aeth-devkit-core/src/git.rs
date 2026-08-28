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

/// `git reset --soft <rev>`: move `HEAD` only; index and working tree stay as they are.
pub fn reset_soft_to(root: &Path, rev: &str) -> Result<()> {
  expect_ok(capture(root, &["reset", "--soft", "--quiet", rev])?, "git reset --soft").map(|_| ())
}

// ---------------------------------------------------------------------------------------
// Blob-level plumbing. These let a caller build a commit from exact file contents without
// going through `git add`, and put the index back exactly as it was. Bytes, not `String`s:
// lockfiles and TOML are text, but nothing here should depend on that.
// ---------------------------------------------------------------------------------------

/// What the index holds for one path: `(mode, blob sha)`, or `None` when the path is not
/// staged at all (untracked, or never existed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
  pub path: String,
  pub staged: Option<(String, String)>,
}

/// Run `git args` and return raw stdout bytes; errors carry stderr.
fn capture_bytes(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
  let out = git(root)
    .args(args)
    .output()
    .with_context(|| format!("running git {}", args.join(" ")))?;
  if !out.status.success() {
    bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
  }
  Ok(out.stdout)
}

/// The index entries for `paths`, in the order given. `git ls-files -s` prints
/// `<mode> <sha> <stage>\t<path>` for staged paths only, so anything it does not mention
/// is `None`. A path mid merge-conflict is an error: it has *three* rows (stages 1/2/3
/// instead of one stage-0 row), a shape `IndexEntry` cannot represent — silently taking
/// one row would build the release from the merge ancestor and lose the conflict state
/// on restore.
pub fn index_entries(root: &Path, paths: &[String]) -> Result<Vec<IndexEntry>> {
  let mut args = vec!["ls-files", "-s", "--"];
  args.extend(paths.iter().map(String::as_str));
  let listing = expect_ok(capture(root, &args)?, "git ls-files -s")?;
  // First pass: refuse unmerged rows. Only the requested paths are listed, so any row
  // whose third field is not "0" is a managed file in conflict. `nth(2)` consumes the
  // iterator up to the stage field (mode, sha, then stage).
  for line in listing.lines() {
    // `let … else` destructures or skips: a line without a tab is not an entry row.
    let Some((meta, path)) = line.split_once('\t') else { continue };
    if meta.split_whitespace().nth(2) != Some("0") {
      bail!("{path} has unmerged index entries (a merge conflict in progress); resolve and commit the conflict first");
    }
  }
  Ok(
    paths
      .iter()
      .map(|p| {
        // Find the line for this path: split each line at the tab, compare the tail.
        let staged = listing.lines().find_map(|line| {
          let (meta, path) = line.split_once('\t')?;
          if path != p {
            return None;
          }
          let mut fields = meta.split_whitespace();
          Some((fields.next()?.to_string(), fields.next()?.to_string()))
        });
        IndexEntry { path: p.clone(), staged }
      })
      .collect(),
  )
}

/// The bytes of `path` as committed in `HEAD`, or `None` if `HEAD` has no such file.
pub fn head_blob(root: &Path, path: &str) -> Result<Option<Vec<u8>>> {
  let spec = format!("HEAD:{path}");
  // `rev-parse -q --verify` exits non-zero (quietly) when the path is not in the tree.
  let out = capture(root, &["rev-parse", "-q", "--verify", &spec])?;
  if !out.success() {
    return Ok(None);
  }
  blob_bytes(root, out.stdout.trim()).map(Some)
}

/// The file mode (e.g. `100644`, `100755`) of `path` as committed in `HEAD`, or `None`
/// when `HEAD` has no such file. Modes live in *tree* objects, not blobs, so this asks
/// `ls-tree` (which prints `<mode> <type> <sha>\t<path>`) rather than `cat-file`.
pub fn head_mode(root: &Path, path: &str) -> Result<Option<String>> {
  // `ls-tree` exits 0 with empty output for a path HEAD does not have.
  let listing = expect_ok(capture(root, &["ls-tree", "HEAD", "--", path])?, "git ls-tree")?;
  // First line → text before the first space. `and_then` chains the two `Option`s.
  Ok(
    listing
      .lines()
      .next()
      .and_then(|line| line.split_whitespace().next())
      .map(str::to_string),
  )
}

/// The bytes of an object by sha.
pub fn blob_bytes(root: &Path, sha: &str) -> Result<Vec<u8>> {
  capture_bytes(root, &["cat-file", "blob", sha])
}

/// Store `bytes` as a blob in the object database and return its sha.
pub fn hash_object(root: &Path, bytes: &[u8]) -> Result<String> {
  use std::io::Write as _;
  use std::process::Stdio;
  let mut child = git(root)
    .args(["hash-object", "-w", "--stdin"])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .context("running git hash-object")?;
  // `take()` moves the pipe out of the child so it is closed (EOF) when this block ends;
  // git will not finish reading until it sees that EOF.
  {
    let mut stdin = child.stdin.take().context("opening git hash-object stdin")?;
    stdin.write_all(bytes).context("writing to git hash-object")?;
  }
  let out = child.wait_with_output().context("waiting for git hash-object")?;
  if !out.status.success() {
    bail!("git hash-object failed: {}", String::from_utf8_lossy(&out.stderr).trim());
  }
  Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Three-way merge of file contents: the changes from `base` to `other`, applied on top of
/// `current`. `Ok(None)` means the two sides touched overlapping lines and git could not
/// combine them. Runs `git merge-file -p` on three temporary files.
pub fn merge_file(root: &Path, current: &[u8], base: &[u8], other: &[u8]) -> Result<Option<Vec<u8>>> {
  let dir = tempfile::tempdir().context("creating merge scratch dir")?;
  let (c, b, o) = (dir.path().join("current"), dir.path().join("base"), dir.path().join("other"));
  std::fs::write(&c, current).context("writing merge input")?;
  std::fs::write(&b, base).context("writing merge input")?;
  std::fs::write(&o, other).context("writing merge input")?;
  let out = git(root)
    .arg("merge-file")
    .arg("-p")
    .args([&c, &b, &o])
    .output()
    .context("running git merge-file")?;
  // Exit status is the number of conflicts (0 = clean, capped at 127); 255 is an error.
  match out.status.code() {
    Some(0) => Ok(Some(out.stdout)),
    Some(n) if (1..=127).contains(&n) => Ok(None),
    _ => bail!("git merge-file failed: {}", String::from_utf8_lossy(&out.stderr).trim()),
  }
}

/// Put one index entry back: stage `(mode, sha)` for `path`, or drop the entry entirely.
pub fn set_index_entry(root: &Path, entry: &IndexEntry) -> Result<()> {
  let args: Vec<String> = match &entry.staged {
    Some((mode, sha)) => vec![
      "update-index".into(),
      "--add".into(),
      "--cacheinfo".into(),
      format!("{mode},{sha},{}", entry.path),
    ],
    None => vec!["update-index".into(), "--force-remove".into(), "--".into(), entry.path.clone()],
  };
  let args: Vec<&str> = args.iter().map(String::as_str).collect();
  expect_ok(capture(root, &args)?, "git update-index").map(|_| ())
}

/// Create a commit on top of `HEAD` whose tree is `HEAD`'s tree with `files` swapped in,
/// and advance the current branch to it — without touching the user's index or working
/// tree. Each entry stages `(mode, sha)` at its path, or removes the path when `None`.
///
/// The trick is `GIT_INDEX_FILE`: git builds commits from *an* index, and pointing that
/// variable at a scratch file gives us a private one. `read-tree HEAD` fills it, the
/// `update-index` calls edit it, `write-tree` turns it into a tree object, `commit-tree`
/// wraps that in a commit, and `update-ref` moves the branch. Returns the full sha.
pub fn commit_files_on_head(root: &Path, files: &[IndexEntry], message: &str) -> Result<String> {
  let scratch = tempfile::tempdir().context("creating scratch index dir")?;
  let index = scratch.path().join("index");
  // A closure that runs git with the scratch index; `&[&str]` args like `capture`.
  let run = |args: &[&str]| -> Result<String> {
    let out = git(root)
      .env("GIT_INDEX_FILE", &index)
      .args(args)
      .output()
      .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
      bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
  };
  // `HEAD` is resolved to a sha exactly once. Passing the literal string `HEAD` to each
  // command below would resolve it at three different moments; if another process advanced
  // the branch mid-way, the commit could get the wrong parent and `update-ref` would then
  // overwrite the concurrent commit. With one captured sha the tree, the parent, and the
  // compare-and-swap below all agree on the same starting point.
  let head = run(&["rev-parse", "HEAD"])?;
  run(&["read-tree", &head])?;
  for f in files {
    match &f.staged {
      Some((mode, sha)) => {
        let info = format!("{mode},{sha},{}", f.path);
        run(&["update-index", "--add", "--cacheinfo", &info])?;
      }
      None => {
        run(&["update-index", "--force-remove", "--", &f.path])?;
      }
    }
  }
  let tree = run(&["write-tree"])?;
  let commit = run(&["commit-tree", &tree, "-p", &head, "-m", message])?;
  // The trailing `&head` is `update-ref`'s *old-value* argument: git moves the ref only if
  // it still equals that sha, and fails otherwise. So a branch some other process moved
  // during this function makes us abort cleanly instead of silently discarding their commit.
  run(&["update-ref", "-m", message, "HEAD", &commit, &head])?;
  Ok(commit)
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

/// The sha `origin` has for `refs/heads/<branch>`, or `None` when the remote has no such
/// branch. Used after a failed push to learn whether the branch actually moved: a push
/// whose response was lost on the wire reports failure even though the refs landed.
pub fn remote_branch_sha(runner: &dyn Runner, root: &Path, branch: &str) -> Result<Option<String>> {
  let refname = format!("refs/heads/{branch}");
  let s = expect_ok(remote(runner, root, &["ls-remote", "origin", &refname])?, "git ls-remote")?;
  // Output is `<sha>\t<refname>` or empty; the sha is everything before the first
  // whitespace, and `map` only runs when there was a first token at all.
  Ok(s.split_whitespace().next().map(str::to_string))
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
    assert_eq!(head_mode(root, "a.txt").unwrap().as_deref(), Some("100644"));
    assert_eq!(head_mode(root, "missing.txt").unwrap(), None);
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
  fn index_entries_reject_unmerged_paths() {
    use std::io::Write as _;
    use std::process::Stdio;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    write(root, "a.txt", "base\n");
    write(root, "b.txt", "fine\n");
    commit_paths(root, &["a.txt".into(), "b.txt".into()], "first").unwrap();
    // Manufacture a merge conflict without a real merge: `update-index --index-info`
    // writes raw index rows, and stage-1/2/3 entries (ancestor/ours/theirs) for one path
    // are exactly what `git merge` leaves behind for a conflicted file.
    let (b, o, t) = (
      hash_object(root, b"base\n").unwrap(),
      hash_object(root, b"ours\n").unwrap(),
      hash_object(root, b"theirs\n").unwrap(),
    );
    let rows = format!("100644 {b} 1\ta.txt\n100644 {o} 2\ta.txt\n100644 {t} 3\ta.txt\n");
    let mut child = git(root)
      .args(["update-index", "--index-info"])
      .stdin(Stdio::piped())
      .spawn()
      .unwrap();
    child.stdin.take().unwrap().write_all(rows.as_bytes()).unwrap();
    assert!(child.wait().unwrap().success());
    // The conflicted path is refused with a message naming it…
    let err = index_entries(root, &["a.txt".into()]).unwrap_err().to_string();
    assert!(err.contains("a.txt") && err.contains("unmerged"), "{err}");
    // …and a clean path on its own still parses normally.
    let clean = index_entries(root, &["b.txt".into()]).unwrap();
    assert!(clean[0].staged.is_some());
  }

  #[test]
  fn merge_file_applies_delta_or_reports_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    let base = b"a\nb\nc\nd\ne\n";
    // The user changed line 1; the release changed line 5. Disjoint → merges cleanly.
    let merged = merge_file(root, b"A\nb\nc\nd\ne\n", base, b"a\nb\nc\nd\nE\n").unwrap();
    assert_eq!(merged.as_deref(), Some(&b"A\nb\nc\nd\nE\n"[..]));
    // Both sides changed line 5 → conflict → `None`.
    assert_eq!(merge_file(root, b"a\nb\nc\nd\nX\n", base, b"a\nb\nc\nd\nE\n").unwrap(), None);
  }

  #[test]
  fn commit_files_on_head_leaves_index_and_worktree_alone() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    write(root, "a.txt", "1\n");
    write(root, "other.txt", "x\n");
    commit_paths(root, &["a.txt".into(), "other.txt".into()], "first").unwrap();
    let first = head_sha(root).unwrap();
    // User state: `other.txt` staged, `a.txt` edited but unstaged.
    write(root, "other.txt", "y\n");
    assert!(git(root).args(["add", "other.txt"]).status().unwrap().success());
    write(root, "a.txt", "user\n");
    let before = index_entries(root, &["a.txt".into(), "other.txt".into(), "new.txt".into()]).unwrap();
    assert!(before[0].staged.is_some() && before[1].staged.is_some() && before[2].staged.is_none());

    // Commit "bumped" content for a.txt plus a brand-new file, straight from bytes.
    let bumped = hash_object(root, b"bumped\n").unwrap();
    let fresh = hash_object(root, b"new\n").unwrap();
    let sha = commit_files_on_head(
      root,
      &[
        IndexEntry {
          path: "a.txt".into(),
          staged: Some(("100644".into(), bumped)),
        },
        IndexEntry {
          path: "new.txt".into(),
          staged: Some(("100644".into(), fresh)),
        },
      ],
      "bump",
    )
    .unwrap();
    assert_eq!(head_sha(root).unwrap(), sha);
    assert_eq!(head_blob(root, "a.txt").unwrap().as_deref(), Some(&b"bumped\n"[..]));
    assert_eq!(head_blob(root, "new.txt").unwrap().as_deref(), Some(&b"new\n"[..]));
    assert_eq!(head_blob(root, "missing.txt").unwrap(), None);
    // The real index and working tree are exactly as the user left them.
    assert_eq!(
      index_entries(root, &["a.txt".into(), "other.txt".into(), "new.txt".into()]).unwrap(),
      before
    );
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "user\n");
    assert!(!root.join("new.txt").exists());

    // Undo: soft reset + put the recorded entries back → index identical to `before`.
    reset_soft_to(root, &first).unwrap();
    for e in &before {
      set_index_entry(root, e).unwrap();
    }
    assert_eq!(head_sha(root).unwrap(), first);
    let staged = String::from_utf8(git(root).args(["diff", "--cached", "--name-only"]).output().unwrap().stdout).unwrap();
    assert_eq!(staged.trim(), "other.txt");
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
    // The broad `ls-remote` script answers the branch probe too (its output's first token
    // is the sha); a later, more specific empty script models a branch the remote lacks.
    assert_eq!(remote_branch_sha(&r, root, "main").unwrap().as_deref(), Some("abc"));
    r.script("git", &["ls-remote", "origin", "refs/heads/gone"], 0, "");
    assert_eq!(remote_branch_sha(&r, root, "gone").unwrap(), None);

    let failing = RecordingRunner::new(1);
    assert_eq!(upstream(&failing, root).unwrap(), None);
    assert!(push_refs(&failing, root, &["main"]).is_err());
  }
}
