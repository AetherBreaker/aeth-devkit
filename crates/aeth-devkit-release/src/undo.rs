//! The undo journal: one entry per completed forward step, unwound in reverse on failure.
//!
//! This replaces the shell script's pile of `COMMITTED=true` / `PUSHED=true` flags. Each
//! forward step that succeeds pushes an [`Undo`] describing exactly how to reverse it; if a
//! later step fails, the journal is walked backwards. Because the entries are an enum with
//! data, the compiler guarantees every kind of undo is handled in `apply` (and in the two
//! text renderers) — a new variant that is not matched is a compile error, not a silent
//! gap in the rollback.

use std::path::Path;

use anyhow::{Result, bail};

use aeth_devkit_core::git::{self, IndexEntry};

use crate::Deps;
use crate::snapshot::Snapshot;

/// One reversible action, with the data needed to reverse it.
pub enum Undo {
  /// Copy the snapshot back (covers `uv version`, the Cargo edit, `uv lock`, `uv build`).
  RestoreFiles(Snapshot),
  /// Drop the bump commit — but only if `HEAD` is still that commit — and put the index
  /// entries of the release-managed files back to exactly what they were before the run
  /// (`index`), staged edits included. Nothing else in the index is touched.
  ResetCommit {
    bump_sha: String,
    pre_sha: String,
    index: Vec<IndexEntry>,
  },
  DeleteLocalTag(String),
  /// Delete `refs/tags/<tag>` on `origin`, guarded by a lease on `expected` (the tag
  /// object this release created) so a concurrently replaced tag is refused, not deleted.
  DeleteRemoteTag {
    tag: String,
    expected: String,
  },
  /// Rewind `origin/<branch>` to `pre_sha`, guarded by a lease on `bump_sha`.
  ForcePushBranch {
    branch: String,
    bump_sha: String,
    pre_sha: String,
  },
  DeleteGithubRelease(String),
}

/// A compensating step that did not work, with the command the user can run by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
  pub what: String,
  pub manual: String,
  pub error: String,
}

impl Undo {
  /// Human-readable description for the progress log.
  pub fn describe(&self) -> String {
    // `match self` with `Undo::Variant(..)` / `{ .. }` patterns; `..` ignores the payload.
    match self {
      Undo::RestoreFiles(_) => "Restoring pyproject.toml, uv.lock, Cargo.toml, Cargo.lock".into(),
      Undo::ResetCommit { .. } => "Resetting the version-bump commit".into(),
      Undo::DeleteLocalTag(t) => format!("Deleting local tag {t}"),
      Undo::DeleteRemoteTag { tag, .. } => format!("Deleting remote tag {tag}"),
      Undo::ForcePushBranch { branch, .. } => format!("Force-pushing the pre-release {branch} to origin"),
      Undo::DeleteGithubRelease(t) => format!("Deleting GitHub release {t}"),
    }
  }

  /// The shell command a user can paste if this undo fails.
  pub fn manual_command(&self) -> String {
    match self {
      // `git checkout` would restore `HEAD`, not the pre-run tree (which may have held
      // accepted dirty changes, and `dist/` is not tracked at all). The snapshot directory
      // is the real pre-run state; `unwind` keeps it on disk when this undo fails, and the
      // snapshot renders the exact delete-then-copy sequence `restore` would have done.
      Undo::RestoreFiles(snap) => snap.manual_restore_command(),
      Undo::ResetCommit { pre_sha, index, .. } => {
        let mut s = format!("git reset --soft {pre_sha}");
        for e in index {
          match &e.staged {
            Some((mode, sha)) => s += &format!(" && git update-index --add --cacheinfo {mode},{sha},{}", e.path),
            None => s += &format!(" && git update-index --force-remove -- {}", e.path),
          }
        }
        s
      }
      Undo::DeleteLocalTag(t) => format!("git tag -d {t}"),
      // The same lease `apply` uses, so even the hand-run command cannot delete a tag
      // that is no longer the one this release created.
      Undo::DeleteRemoteTag { tag, expected } => {
        format!("git push --force-with-lease=refs/tags/{tag}:{expected} origin :refs/tags/{tag}")
      }
      Undo::ForcePushBranch { branch, bump_sha, pre_sha } => {
        format!("git push --force-with-lease={branch}:{bump_sha} origin {pre_sha}:refs/heads/{branch}")
      }
      Undo::DeleteGithubRelease(t) => format!("gh release delete {t} --yes"),
    }
  }

  /// Perform the compensating action.
  fn apply(&self, deps: &Deps, root: &Path) -> Result<()> {
    match self {
      Undo::RestoreFiles(snap) => snap.restore(root),
      Undo::ResetCommit { bump_sha, pre_sha, index } => {
        // The guard that the old `git reset HEAD~1` lacked: refuse to reset if `HEAD` is
        // not the commit we made. `&head[..7]` slices the first seven characters for the
        // message (SHAs are ASCII, so byte slicing is safe).
        let head = git::head_sha(root)?;
        if head != *bump_sha {
          bail!("HEAD is {} but the bump commit was {}; not resetting", &head[..7], &bump_sha[..7]);
        }
        git::reset_soft_to(root, pre_sha)?;
        for e in index {
          git::set_index_entry(root, e)?;
        }
        Ok(())
      }
      Undo::DeleteLocalTag(t) => git::delete_tag(root, t),
      Undo::DeleteRemoteTag { tag, expected } => git::delete_remote_tag_leased(deps.runner, root, tag, expected),
      Undo::ForcePushBranch { branch, bump_sha, pre_sha } => git::force_push_with_lease(deps.runner, root, branch, bump_sha, pre_sha),
      // No `--cleanup-tag`: that would delete the remote tag with no lease, ahead of the
      // `DeleteRemoteTag` entry whose lease is what keeps a tag another publisher pushed
      // during the (now minutes-long) CI wait out of reach.
      Undo::DeleteGithubRelease(t) => {
        let args: Vec<String> = ["release", "delete", t, "--yes"].iter().map(|s| s.to_string()).collect();
        let out = deps.runner.run_capture("gh", &args, root)?;
        if out.success() {
          Ok(())
        } else {
          bail!("gh release delete failed: {}", out.stderr.trim())
        }
      }
    }
  }
}

/// Walk the journal backwards, attempting every entry even if earlier ones fail, and
/// return the failures. Taking `journal` by value (`Vec<Undo>`) consumes it: after
/// unwinding there is nothing left to accidentally unwind twice.
pub fn unwind(journal: Vec<Undo>, deps: &Deps, root: &Path) -> Vec<Failure> {
  let mut failures = Vec::new();
  // `into_iter().rev()` yields owned entries last-to-first.
  for undo in journal.into_iter().rev() {
    eprintln!("  -> {}...", undo.describe());
    if let Err(e) = undo.apply(deps, root) {
      // `{e:#}` prints the error with its full context chain on one line.
      eprintln!("     WARNING: {e:#}");
      failures.push(Failure {
        what: undo.describe(),
        manual: undo.manual_command(),
        error: format!("{e:#}"),
      });
      // A failed file restore must not take the only copy of the pre-run state with it:
      // keep the snapshot directory (its path is in `manual`) instead of letting the
      // `TempDir` delete it on drop.
      if let Undo::RestoreFiles(snap) = undo {
        let kept = snap.keep();
        eprintln!("     pre-run snapshot kept at {}", kept.display());
      }
    }
  }
  failures
}

/// The block printed when some compensating steps failed.
pub fn render_failures(failures: &[Failure]) -> String {
  let mut s = String::from("Manual cleanup required:\n");
  for f in failures {
    s += &format!("  {}: {}\n    {}\n", f.what, f.error, f.manual);
  }
  s
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::AtomicBool;

  use aeth_devkit_core::devpi::StubDevpiClient;
  use aeth_devkit_core::index::StubIndexClient;
  use aeth_devkit_core::process::RecordingRunner;

  use crate::prompt::ScriptedPrompt;

  #[test]
  fn unwinds_in_reverse_and_keeps_going_after_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    std::fs::write(root.join("pyproject.toml"), "v=1\n").unwrap();
    git::commit_paths(root, &["pyproject.toml".into()], "init").unwrap();
    let pre = git::head_sha(root).unwrap();
    let snap = crate::snapshot::take(root).unwrap();
    // Recorded *before* the bump commit, as `steps::execute` does.
    let pre_index = git::index_entries(root, &["pyproject.toml".into()]).unwrap();
    std::fs::write(root.join("pyproject.toml"), "v=2\n").unwrap();
    git::commit_paths(root, &["pyproject.toml".into()], "bump").unwrap();
    let bump = git::head_sha(root).unwrap();
    git::create_annotated_tag(root, "v2", "Version 2").unwrap();
    let tag_sha = git::tag_object_sha(root, "v2").unwrap();

    let runner = RecordingRunner::new(0);
    // The GitHub delete is scripted to fail: it must be reported but not stop the rest.
    runner.script("gh", &["release", "delete"], 1, "");
    let devpi = StubDevpiClient::new(true);
    let prompt = ScriptedPrompt::new(&[]);
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let deps = Deps {
      runner: &runner,
      devpi: &devpi,
      index: &index,
      prompt: &prompt,
      env: &|_| None,
      interrupted: &flag,
    };

    let journal = vec![
      Undo::RestoreFiles(snap),
      Undo::ResetCommit {
        bump_sha: bump.clone(),
        pre_sha: pre.clone(),
        index: pre_index,
      },
      Undo::DeleteLocalTag("v2".into()),
      Undo::DeleteRemoteTag {
        tag: "v2".into(),
        expected: tag_sha.clone(),
      },
      Undo::ForcePushBranch {
        branch: "main".into(),
        bump_sha: bump.clone(),
        pre_sha: pre.clone(),
      },
      Undo::DeleteGithubRelease("v2".into()),
    ];
    let failures = unwind(journal, &deps, root);
    assert_eq!(failures.len(), 1);
    assert!(failures[0].what.contains("GitHub release"));
    assert_eq!(failures[0].manual, "gh release delete v2 --yes");
    let git_calls = runner.calls_for("git");
    assert_eq!(
      git_calls[0],
      vec![
        "push",
        &format!("--force-with-lease=main:{bump}"),
        "origin",
        &format!("{pre}:refs/heads/main")
      ]
    );
    assert_eq!(
      git_calls[1],
      vec![
        "push".to_string(),
        format!("--force-with-lease=refs/tags/v2:{tag_sha}"),
        "origin".into(),
        ":refs/tags/v2".into()
      ]
    );
    assert_eq!(git::tag_target(root, "v2").unwrap(), None);
    assert_eq!(git::head_sha(root).unwrap(), pre);
    assert_eq!(std::fs::read_to_string(root.join("pyproject.toml")).unwrap(), "v=1\n");
    assert!(render_failures(&failures).contains("Manual cleanup required"));
  }

  #[test]
  fn reset_refuses_when_head_moved() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    std::fs::write(root.join("a"), "1").unwrap();
    git::commit_paths(root, &["a".into()], "one").unwrap();
    let pre = git::head_sha(root).unwrap();
    std::fs::write(root.join("a"), "2").unwrap();
    git::commit_paths(root, &["a".into()], "two").unwrap();
    let bump = git::head_sha(root).unwrap();
    std::fs::write(root.join("a"), "3").unwrap();
    git::commit_paths(root, &["a".into()], "three").unwrap();
    let moved = git::head_sha(root).unwrap();
    let runner = RecordingRunner::new(0);
    let devpi = StubDevpiClient::new(false);
    let prompt = ScriptedPrompt::new(&[]);
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let deps = Deps {
      runner: &runner,
      devpi: &devpi,
      index: &index,
      prompt: &prompt,
      env: &|_| None,
      interrupted: &flag,
    };
    let failures = unwind(
      vec![Undo::ResetCommit {
        bump_sha: bump,
        pre_sha: pre,
        index: Vec::new(),
      }],
      &deps,
      root,
    );
    assert_eq!(failures.len(), 1);
    assert!(failures[0].error.contains("HEAD"));
    assert_eq!(git::head_sha(root).unwrap(), moved);
  }
}
