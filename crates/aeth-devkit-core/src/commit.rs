//! Quiet commits: a command's file edits, committed on a clean `HEAD` base, without
//! disturbing the user's uncommitted work.
//!
//! Every devkit command that auto-commits follows the same convention. The files the
//! command manages are reset to their `HEAD` content before the tools run, so the tools
//! edit clean input; the commit is built from that output through a scratch index (the
//! user's index is never `git add`ed); and the user's uncommitted edits are then replayed
//! on top with a three-way merge, so the tree afterwards looks like "what the user had,
//! plus the command's changes" and `git status` shows exactly the edits they had before.
//! Edits that overlap the command's changes cannot be combined and are an error — the
//! caller rolls back instead of committing a mixture.
//!
//! `devkit release` drives the pieces directly (its rollback runs through an undo
//! journal); `devkit lock` and `devkit setup-project` use [`commit_or_rollback`], which
//! wraps the whole commit-and-replay in a self-contained rollback.

use std::path::Path;

use anyhow::{Context as _, Result, anyhow};

use crate::git::{self, IndexEntry};

/// Pre-run state of one managed file, captured before any tool touches it, so the commit
/// can be built from clean content and the user's edits put back afterwards.
pub struct TrackedBase {
  pub path: String,
  /// The committed bytes (`None` if `HEAD` has no such file).
  pub head: Option<Vec<u8>>,
  /// The committed file mode (`100644`/`100755`), `None` alongside `head`. The commit
  /// keeps *this* mode, not the index's: a staged `chmod` is the user's pending change and
  /// must stay in their index, not be folded into the command's commit.
  pub head_mode: Option<String>,
  /// The working-tree bytes before the run in repository form (clean filters applied) —
  /// the user's version, edits and all — or `None` when the file was absent (deleted by
  /// the user, or never created yet).
  pub worktree: Option<Vec<u8>>,
  /// Whether those bytes differ from the raw file on disk (a CRLF checkout, say); every
  /// write back to the working tree smudges iff this is set, so the file keeps its form.
  pub filtered: bool,
  /// The index entry before the run, and the bytes it points at (if any).
  pub index: IndexEntry,
  pub index_bytes: Option<Vec<u8>>,
}

impl TrackedBase {
  /// A base for a file that existed nowhere before the run — not on disk, not in `HEAD`,
  /// not in the index. A file the tools then create is committed as-is.
  pub fn absent(path: &str) -> Self {
    TrackedBase {
      path: path.to_string(),
      head: None,
      head_mode: None,
      worktree: None,
      filtered: false,
      index: IndexEntry {
        path: path.to_string(),
        staged: None,
      },
      index_bytes: None,
    }
  }
}

/// Capture every managed file that exists on disk, in `HEAD`, or in the index, and where
/// the working copy differs from `HEAD` (edited *or deleted*) put the `HEAD` version on
/// disk so the tools operate on clean input.
///
/// The caller restores the working tree on failure ([`restore_worktree`]); on success
/// [`commit_on_clean_base`] merges the user's edits back on top of the tools' output.
pub fn stage_clean_base(root: &Path, paths: &[&str]) -> Result<Vec<TrackedBase>> {
  let paths: Vec<String> = paths.iter().map(|p| p.to_string()).collect();
  let entries = git::index_entries(root, &paths)?;
  let mut bases = Vec::with_capacity(paths.len());
  // `zip` pairs each path with its index entry; they were produced in the same order.
  for (path, index) in paths.into_iter().zip(entries) {
    // Repository-form bytes (clean filters applied), never a raw `fs::read`: on a
    // `core.autocrlf=true` checkout the raw file is CRLF while every blob is LF, and
    // comparing those would call a clean tree "edited on every line" — then the merge-back
    // in `commit_on_clean_base` would report every line as an overlapping edit.
    let (worktree, filtered) = match git::worktree_blob(root, &path)? {
      Some(w) => (Some(w.bytes), w.filtered),
      None => (None, false),
    };
    let head = git::head_blob(root, &path)?;
    let head_mode = git::head_mode(root, &path)?;
    if worktree.is_none() && head.is_none() && index.staged.is_none() {
      continue; // the project simply does not have this file
    }
    // `as_ref()` turns `&Option<(String, String)>` into `Option<&(String, String)>` so the
    // closure can borrow the sha without moving it out of the entry.
    let index_bytes = match index.staged.as_ref() {
      Some((_, sha)) => Some(git::blob_bytes(root, sha)?),
      None => None,
    };
    // A let-chain (edition 2024): bind `h` *and* test it in one condition. A deleted file
    // (`worktree == None`) counts as "differs", and gets the HEAD copy back for the tools.
    if let Some(h) = &head
      && worktree.as_deref() != Some(h.as_slice())
    {
      git::write_worktree(root, &path, h, filtered).with_context(|| format!("resetting {path} to HEAD"))?;
    }
    bases.push(TrackedBase {
      path,
      head,
      head_mode,
      worktree,
      filtered,
      index,
      index_bytes,
    });
  }
  Ok(bases)
}

/// After the tools ran: add an [`TrackedBase::absent`] base for every managed file that is
/// now on disk but had no base (it existed nowhere before the run), so a file the tools
/// created is committed as-is instead of being invisible to the commit.
pub fn absorb_created(root: &Path, paths: &[&str], bases: &mut Vec<TrackedBase>) {
  for p in paths {
    if root.join(p).is_file() && !bases.iter().any(|b| b.path == *p) {
      bases.push(TrackedBase::absent(p));
    }
  }
}

/// Whether the tools actually changed anything: does any managed file's current content
/// differ from what the commit's base (`HEAD`) holds? Files the commit machinery would
/// skip anyway — a pre-existing untracked file, a file the user had deleted — do not count.
pub fn changed_vs_head(root: &Path, bases: &[TrackedBase]) -> Result<bool> {
  for b in bases {
    let current = git::worktree_blob(root, &b.path)?.map(|w| w.bytes);
    match (&b.head, &current) {
      (Some(h), Some(c)) if c != h => return Ok(true),
      // Created by this run (nothing pre-existed): a change the commit would include.
      (None, Some(_)) if b.worktree.is_none() && b.index.staged.is_none() => return Ok(true),
      _ => {}
    }
  }
  Ok(false)
}

/// Put the working tree back the way [`stage_clean_base`] found it: each base's pre-run
/// bytes rewritten (smudged iff the file was filtered), and files that did not exist
/// before — deleted by the user, or created by the tools — removed again. This is the
/// *rollback* restore: it also reverts tool edits to pre-existing untracked files.
pub fn restore_worktree(root: &Path, bases: &[TrackedBase]) -> Result<()> {
  for b in bases {
    restore_one(root, b)?;
  }
  Ok(())
}

/// Undo only the staging: put the user's copies back where [`stage_clean_base`] wrote a
/// `HEAD` copy over them. Files `HEAD` does not have — a pre-existing untracked file the
/// tools edited in place (never committed, so the edit is the intended outcome) — are left
/// as the tools left them. This is the restore for a run that changed nothing committable.
pub fn unstage_clean_base(root: &Path, bases: &[TrackedBase]) -> Result<()> {
  for b in bases {
    if b.head.is_some() {
      restore_one(root, b)?;
    }
  }
  Ok(())
}

fn restore_one(root: &Path, b: &TrackedBase) -> Result<()> {
  match &b.worktree {
    Some(bytes) => git::write_worktree(root, &b.path, bytes, b.filtered).with_context(|| format!("restoring {}", b.path)),
    None => {
      let file = root.join(&b.path);
      if file.exists() {
        std::fs::remove_file(&file).with_context(|| format!("removing {}", b.path))?;
      }
      Ok(())
    }
  }
}

/// Commit the tools' edits and only those, then put the user's edits back.
///
/// For each managed file the tools produced `bumped` from the clean base `head`. The commit
/// gets `bumped` verbatim (built through a scratch index, so the real index is never
/// `git add`ed). Then the same base→bumped delta is replayed onto the user's working-tree
/// version and onto their staged version with a three-way merge, so after this the tree
/// looks like "what the user had, plus the change", and `git status` shows exactly the
/// edits they had before. Overlapping edits (the user changed the same lines) cannot be
/// combined, and are an error — the caller rolls back. `change` names the command's edit
/// in that message ("the version bump", "the pin update").
///
/// `on_commit` fires with the new sha the moment the commit exists, *before* the index and
/// working tree are touched, so the caller can arm its rollback first — a failure in that
/// last stretch must still reset the commit.
pub fn commit_on_clean_base(
  root: &Path,
  bases: &[TrackedBase],
  message: &str,
  change: &str,
  on_commit: &mut dyn FnMut(&str),
) -> Result<String> {
  let mut to_commit = Vec::new();
  let mut new_index = Vec::new();
  // `Option<Vec<u8>>` per path: `None` means "the user had deleted it; delete it again".
  // The `bool` is the file's `filtered` flag: smudge on the way back out iff set.
  let mut new_worktree: Vec<(String, Option<Vec<u8>>, bool)> = Vec::new();
  for b in bases {
    // Repository form again (see `stage_clean_base`), so `bumped` is comparable with
    // `head` and the user's copies whatever line endings the tools wrote.
    let Some(bumped) = git::worktree_blob(root, &b.path)?.map(|w| w.bytes) else {
      // Nothing regenerated it (a deleted file the tools did not need): the commit keeps
      // HEAD's version and the user's deletion stands.
      continue;
    };
    // Two very different situations hide behind "HEAD has no such file".
    let Some(head) = &b.head else {
      if b.worktree.is_some() || b.index.staged.is_some() {
        // The file predates the run (untracked, or staged as new): its bytes are the
        // *user's* content, which the tools edited in place, and committing it would
        // sweep their work into the commit and silently start tracking it. Leave it out
        // of the commit entirely: the working tree already holds "theirs plus the
        // change", and the real index — never touched by the scratch-index commit —
        // still holds exactly what they had staged.
        continue;
      }
      // Created *by this run* (a first `Cargo.lock`, say): nothing pre-existed, so
      // commit it as-is and stage the same entry — without one, the committed path would
      // show up in `git status` as a staged deletion.
      let staged = Some(("100644".to_string(), git::hash_object(root, &bumped)?));
      to_commit.push(IndexEntry {
        path: b.path.clone(),
        staged: staged.clone(),
      });
      new_index.push(IndexEntry {
        path: b.path.clone(),
        staged,
      });
      continue;
    };
    // The commit's mode comes from `HEAD` (captured with the blob in `stage_clean_base`);
    // a mode the user *staged* stays theirs, restored with their index entry below.
    let head_mode = b.head_mode.clone().unwrap_or_else(|| "100644".to_string());
    to_commit.push(IndexEntry {
      path: b.path.clone(),
      staged: Some((head_mode, git::hash_object(root, &bumped)?)),
    });
    let replay = |onto: &[u8], what: &str| -> Result<Vec<u8>> {
      if onto == head.as_slice() {
        return Ok(bumped.clone());
      }
      git::merge_file(root, onto, head, &bumped)?.with_context(|| {
        format!(
          "your uncommitted {what} edits to {} overlap {change}; commit or stash them and rerun",
          b.path
        )
      })
    };
    // The user's working copy: replay the change onto it, or keep it deleted.
    let worktree = match &b.worktree {
      Some(bytes) => Some(replay(bytes, "working-tree")?),
      None => None,
    };
    // The user's staged copy: replay onto it; a staged deletion (HEAD has the file, the
    // index does not) stays a deletion. The rebuilt entry keeps the *index's* mode — this
    // is where a staged `chmod` survives, while the commit above kept HEAD's mode.
    let index = match (&b.index_bytes, b.index.staged.as_ref()) {
      // Both are `Some` together (`index_bytes` was read from the staged sha); matching
      // the pair lets the compiler hand us the mode without an `unwrap`.
      (Some(bytes), Some((staged_mode, _))) => Some((staged_mode.clone(), git::hash_object(root, &replay(bytes, "staged")?)?)),
      _ => None,
    };
    new_index.push(IndexEntry {
      path: b.path.clone(),
      staged: index,
    });
    new_worktree.push((b.path.clone(), worktree, b.filtered));
  }
  // All merges succeeded before anything is mutated, so a conflict leaves no commit behind.
  let sha = git::commit_files_on_head(root, &to_commit, message)?;
  on_commit(&sha);
  for e in &new_index {
    git::set_index_entry(root, e)?;
  }
  for (path, bytes, filtered) in &new_worktree {
    match bytes {
      // Smudged iff the user's copy was, so the file keeps the line endings it had.
      Some(b) => git::write_worktree(root, path, b, *filtered).with_context(|| format!("re-applying edits to {path}"))?,
      None => std::fs::remove_file(root.join(path)).with_context(|| format!("re-deleting {path}"))?,
    }
  }
  Ok(sha)
}

/// The whole quiet commit for commands without an undo journal: commit the tools' edits on
/// the clean base and replay the user's uncommitted work, or put everything back.
///
/// `Ok(Some(short hash))` — committed. `Ok(None)` — no managed file differs from `HEAD`
/// (nothing to commit); the user's originals are back where `HEAD` copies were staged.
/// `Err` — the commit could not be made (most often because the user's uncommitted edits
/// overlap `change`); the working tree, index, and branch are back exactly as they were
/// before [`stage_clean_base`].
pub fn commit_or_rollback(root: &Path, bases: &[TrackedBase], message: &str, change: &str) -> Result<Option<String>> {
  if !changed_vs_head(root, bases)? {
    unstage_clean_base(root, bases)?;
    return Ok(None);
  }
  let pre_sha = git::head_sha(root)?;
  // Set the moment the commit object exists, so the rollback below knows whether there is
  // a commit to reset (a merge conflict fails before one is ever created).
  let mut committed: Option<String> = None;
  match commit_on_clean_base(root, bases, message, change, &mut |sha| committed = Some(sha.to_string())) {
    Ok(_) => Ok(Some(git::short_head(root)?)),
    Err(e) => {
      let rollback = || -> Result<()> {
        if let Some(sha) = &committed
          && git::head_sha(root)? == *sha
        {
          git::reset_soft_to(root, &pre_sha)?;
          for b in bases {
            git::set_index_entry(root, &b.index)?;
          }
        }
        restore_worktree(root, bases)
      };
      match rollback() {
        Ok(()) => Err(e),
        Err(r) => Err(anyhow!("{e:#}; additionally, rolling the working tree back failed: {r:#}")),
      }
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(root: &Path, rel: &str, s: &str) {
    std::fs::write(root.join(rel), s).unwrap();
  }

  fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap()
  }

  /// A repo with `managed.txt` committed and the user holding an unrelated uncommitted
  /// edit to it (a changed first line; the tools will change the last line).
  fn repo_with_user_edit() -> (tempfile::TempDir, &'static str) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    write(root, "managed.txt", "a\nb\nc\n");
    git::commit_paths(root, &["managed.txt".into()], "init").unwrap();
    write(root, "managed.txt", "USER\nb\nc\n");
    (dir, "managed.txt")
  }

  #[test]
  fn stage_reset_commit_and_replay_keep_user_edits_uncommitted() {
    let (dir, path) = repo_with_user_edit();
    let root = dir.path();
    let bases = stage_clean_base(root, &[path]).unwrap();
    // The tools see the HEAD content, not the user's edit.
    assert_eq!(read(root, path), "a\nb\nc\n");
    assert!(!changed_vs_head(root, &bases).unwrap());
    write(root, path, "a\nb\nTOOL\n");
    assert!(changed_vs_head(root, &bases).unwrap());

    let hash = commit_or_rollback(root, &bases, "tool commit", "the change").unwrap();
    assert_eq!(hash.as_deref(), Some(git::short_head(root).unwrap().as_str()));
    // The commit holds base + tool edit only; the tree holds user + tool.
    assert_eq!(git::head_blob(root, path).unwrap().as_deref(), Some(&b"a\nb\nTOOL\n"[..]));
    assert_eq!(read(root, path), "USER\nb\nTOOL\n");
    assert!(
      git::status_porcelain(root).unwrap().contains(path),
      "the user edit stays uncommitted"
    );
  }

  #[test]
  fn no_tool_change_restores_the_user_edit_and_commits_nothing() {
    let (dir, path) = repo_with_user_edit();
    let root = dir.path();
    let first = git::head_sha(root).unwrap();
    let bases = stage_clean_base(root, &[path]).unwrap();
    let hash = commit_or_rollback(root, &bases, "tool commit", "the change").unwrap();
    assert_eq!(hash, None);
    assert_eq!(git::head_sha(root).unwrap(), first);
    assert_eq!(read(root, path), "USER\nb\nc\n");
  }

  #[test]
  fn overlapping_user_edit_is_rejected_and_rolled_back() {
    let (dir, path) = repo_with_user_edit();
    let root = dir.path();
    let first = git::head_sha(root).unwrap();
    let bases = stage_clean_base(root, &[path]).unwrap();
    // The tools change the same line the user changed: unmergeable.
    write(root, path, "TOOL\nb\nc\n");
    let err = commit_or_rollback(root, &bases, "tool commit", "the change")
      .unwrap_err()
      .to_string();
    assert!(err.contains("overlap the change"), "{err}");
    assert_eq!(git::head_sha(root).unwrap(), first, "no commit may survive a conflict");
    assert_eq!(read(root, path), "USER\nb\nc\n", "the user's edit is back");
  }

  #[test]
  fn created_and_untracked_files_are_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    write(root, "a.txt", "1\n");
    git::commit_paths(root, &["a.txt".into()], "init").unwrap();
    // `untracked.txt` predates the run; `fresh.txt` does not exist yet.
    write(root, "untracked.txt", "mine\n");
    let mut bases = stage_clean_base(root, &["a.txt", "untracked.txt", "fresh.txt"]).unwrap();
    assert_eq!(bases.len(), 2, "a file that exists nowhere gets no base");
    // The tools edit the untracked file and create the fresh one.
    write(root, "untracked.txt", "mine\ntool\n");
    write(root, "fresh.txt", "made\n");
    absorb_created(root, &["a.txt", "untracked.txt", "fresh.txt"], &mut bases);
    assert_eq!(bases.len(), 3);
    commit_or_rollback(root, &bases, "tool commit", "the change").unwrap();
    // The pre-existing untracked file stays the user's; the created one is committed.
    assert_eq!(git::head_blob(root, "untracked.txt").unwrap(), None);
    assert_eq!(git::head_blob(root, "fresh.txt").unwrap().as_deref(), Some(&b"made\n"[..]));
    assert_eq!(read(root, "untracked.txt"), "mine\ntool\n");
  }

  #[test]
  fn restore_worktree_removes_what_did_not_exist() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    write(root, "a.txt", "1\n");
    git::commit_paths(root, &["a.txt".into()], "init").unwrap();
    write(root, "a.txt", "user\n");
    let mut bases = stage_clean_base(root, &["a.txt", "made.txt"]).unwrap();
    write(root, "a.txt", "tool\n");
    write(root, "made.txt", "tool\n");
    absorb_created(root, &["a.txt", "made.txt"], &mut bases);
    restore_worktree(root, &bases).unwrap();
    assert_eq!(read(root, "a.txt"), "user\n");
    assert!(!root.join("made.txt").exists());
  }
}
