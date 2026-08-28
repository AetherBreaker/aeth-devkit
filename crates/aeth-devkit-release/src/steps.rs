//! The nine forward steps of a release. Each pushes its undo onto the journal on success.
//!
//! Ordering is the point of this module. The old script pushed to the remote *before*
//! building, so a plain build failure ended in a force-push. Here every purely local step
//! (snapshot, bump, lock, build, commit, tag) happens first, the index publish next, and
//! the two remote-git/GitHub steps last — so the further a run gets, the more it has
//! already proven, and the expensive compensations are reserved for the rare late failure.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::git::IndexEntry;
use aeth_devkit_core::{cargo_toml, git};

use crate::Deps;
use crate::config::Config;
use crate::preflight::Target;
use crate::snapshot::{self, TRACKED};
use crate::undo::Undo;

/// Everything `execute` needs, gathered by pre-flight. All fields are borrows (`&'a …`):
/// the plan does not own its inputs, it just points at values that outlive it, and the
/// lifetime `'a` says so.
pub struct Plan<'a> {
  pub root: &'a Path,
  pub cfg: &'a Config,
  pub target: &'a Target,
  pub bumps: &'a [String],
  pub notes: Option<&'a str>,
  pub branch: &'a str,
}

// `impl Plan<'_>`: the anonymous lifetime says "whatever lifetime the plan has", since the
// methods do not need to name it.
impl Plan<'_> {
  fn tag(&self) -> String {
    format!("v{}", self.target.new)
  }

  fn bumping(&self) -> bool {
    !self.bumps.is_empty()
  }
}

/// The numbered plan printed by `--dry-run`.
pub fn describe(plan: &Plan) -> String {
  let tag = plan.tag();
  let mut s = String::from("Plan:\n");
  s += "  1. snapshot pyproject.toml, uv.lock, Cargo.toml, Cargo.lock, dist/\n";
  if plan.bumping() {
    s += &format!(
      "  2. uv version --bump {} ; update Cargo.toml ; cargo update --workspace\n",
      plan.bumps.join(" --bump ")
    );
    s += "  3. uv lock\n";
  }
  s += "  4. uv build\n";
  if plan.bumping() {
    s += &format!("  5. git commit \"Bump version to {}\"\n", plan.target.new);
  }
  s += &format!("  6. git tag -a {tag}\n");
  s += &format!("  7. uv publish --index {}\n", plan.cfg.index_name);
  s += &format!(
    "  8. git push origin {}{tag}\n",
    if plan.bumping() {
      format!("{} ", plan.branch)
    } else {
      String::new()
    }
  );
  s += &format!(
    "  9. gh release create {tag} ({})\n",
    plan.notes.map_or("--generate-notes".to_string(), |n| format!("--notes {n:?}"))
  );
  s
}

/// Abort between steps if Ctrl-C was pressed (see [`Deps::check_interrupt`]).
fn check_interrupt(deps: &Deps) -> Result<()> {
  deps.check_interrupt()
}

/// Run a tool with inherited stdio (the user sees its output) and require exit code 0.
fn run_ok(deps: &Deps, root: &Path, program: &str, args: &[&str]) -> Result<()> {
  let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
  match deps.runner.run_inherit(program, &owned, root)? {
    Some(0) => Ok(()),
    Some(code) => bail!("{program} {} exited with {code}", args.join(" ")),
    None => bail!("{program} was terminated by a signal"),
  }
}

/// Pre-run state of one release-managed file, captured before any tool touches it, so the
/// bump commit can be built from clean content and the user's edits put back afterwards.
struct TrackedBase {
  path: String,
  /// The committed bytes (`None` if `HEAD` has no such file).
  head: Option<Vec<u8>>,
  /// The working-tree bytes before the run — the user's version, edits and all — or
  /// `None` when the file was absent (deleted by the user, or never created yet).
  worktree: Option<Vec<u8>>,
  /// The index entry before the run, and the bytes it points at (if any).
  index: IndexEntry,
  index_bytes: Option<Vec<u8>>,
}

/// Capture every release-managed file that exists on disk, in `HEAD`, or in the index, and
/// where the working copy differs from `HEAD` (edited *or deleted*) put the `HEAD` version
/// on disk so `uv version` / `uv lock` / the Cargo edit operate on clean input.
///
/// The working tree is restored by the snapshot on failure; on success step 5 merges the
/// user's edits back on top of the bumped content (`commit_release_edits`).
fn stage_clean_base(root: &Path) -> Result<Vec<TrackedBase>> {
  let paths: Vec<String> = TRACKED.iter().map(|p| p.to_string()).collect();
  let entries = git::index_entries(root, &paths)?;
  let mut bases = Vec::with_capacity(paths.len());
  // `zip` pairs each path with its index entry; they were produced in the same order.
  for (path, index) in paths.into_iter().zip(entries) {
    let file = root.join(&path);
    let worktree = if file.is_file() {
      Some(std::fs::read(&file).with_context(|| format!("reading {path}"))?)
    } else {
      None
    };
    let head = git::head_blob(root, &path)?;
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
      std::fs::write(&file, h).with_context(|| format!("resetting {path} to HEAD for the bump"))?;
    }
    bases.push(TrackedBase {
      path,
      head,
      worktree,
      index,
      index_bytes,
    });
  }
  Ok(bases)
}

/// Commit the release's edits and only those, then put the user's edits back.
///
/// For each managed file the tools produced `bumped` from the clean base `head`. The commit
/// gets `bumped` verbatim (built through a scratch index, so the real index is never
/// `git add`ed). Then the same base→bumped delta is replayed onto the user's working-tree
/// version and onto their staged version with a three-way merge, so after this the tree
/// looks like "what the user had, plus the bump", and `git status` shows exactly the edits
/// they had before. Overlapping edits (say, the user changed the version line) cannot be
/// combined, and are an error — the caller rolls back.
///
/// The `ResetCommit` undo is pushed onto `journal` the moment the commit exists, *before*
/// the index and working tree are touched, so a failure in that last stretch still rolls
/// the commit back.
fn commit_release_edits(root: &Path, bases: &[TrackedBase], message: &str, pre_sha: &str, journal: &mut Vec<Undo>) -> Result<String> {
  let mut to_commit = Vec::new();
  let mut new_index = Vec::new();
  // `Option<Vec<u8>>` per path: `None` means "the user had deleted it; delete it again".
  let mut new_worktree: Vec<(String, Option<Vec<u8>>)> = Vec::new();
  for b in bases {
    let file = root.join(&b.path);
    if !file.is_file() {
      // Nothing regenerated it (a deleted lockfile the tools did not need): the commit
      // keeps HEAD's version and the user's deletion stands.
      continue;
    }
    let bumped = std::fs::read(&file).with_context(|| format!("reading {}", b.path))?;
    // Mode: keep the index's if it had one, else a plain file.
    let mode = b.index.staged.as_ref().map_or("100644", |(m, _)| m.as_str()).to_string();
    to_commit.push(IndexEntry {
      path: b.path.clone(),
      staged: Some((mode.clone(), git::hash_object(root, &bumped)?)),
    });
    // With no committed version there was no clean base to diverge from: the tools ran on
    // the user's bytes directly, so `bumped` already *is* "theirs plus the bump".
    let Some(head) = &b.head else {
      new_index.push(IndexEntry {
        path: b.path.clone(),
        staged: Some((mode, git::hash_object(root, &bumped)?)),
      });
      continue;
    };
    let replay = |onto: &[u8], what: &str| -> Result<Vec<u8>> {
      if onto == head.as_slice() {
        return Ok(bumped.clone());
      }
      git::merge_file(root, onto, head, &bumped)?.with_context(|| {
        format!(
          "your uncommitted {what} edits to {} overlap the version bump; commit or stash them and rerun",
          b.path
        )
      })
    };
    // The user's working copy: replay the bump onto it, or keep it deleted.
    let worktree = match &b.worktree {
      Some(bytes) => Some(replay(bytes, "working-tree")?),
      None => None,
    };
    // The user's staged copy: replay onto it; a staged deletion (HEAD has the file, the
    // index does not) stays a deletion.
    let index = match &b.index_bytes {
      Some(bytes) => Some((mode, git::hash_object(root, &replay(bytes, "staged")?)?)),
      None => None,
    };
    new_index.push(IndexEntry {
      path: b.path.clone(),
      staged: index,
    });
    new_worktree.push((b.path.clone(), worktree));
  }
  // All merges succeeded before anything is mutated, so a conflict leaves no commit behind.
  let sha = git::commit_files_on_head(root, &to_commit, message)?;
  journal.push(Undo::ResetCommit {
    bump_sha: sha.clone(),
    pre_sha: pre_sha.to_string(),
    index: bases.iter().map(|b| b.index.clone()).collect(),
  });
  for e in &new_index {
    git::set_index_entry(root, e)?;
  }
  for (path, bytes) in &new_worktree {
    let file = root.join(path);
    match bytes {
      Some(b) => std::fs::write(&file, b).with_context(|| format!("re-applying edits to {path}"))?,
      None => std::fs::remove_file(&file).with_context(|| format!("re-deleting {path}"))?,
    }
  }
  Ok(sha)
}

/// Rewrite `Cargo.toml`'s version if the file exists and has one. Returns whether it did.
fn set_cargo_version(root: &Path, version: &str) -> Result<bool> {
  let path = root.join("Cargo.toml");
  if !path.is_file() {
    return Ok(false);
  }
  let text = std::fs::read_to_string(&path).context("reading Cargo.toml")?;
  let mut doc: DocumentMut = text.parse().context("parsing Cargo.toml")?;
  if !cargo_toml::set_version(&mut doc, version) {
    return Ok(false);
  }
  std::fs::write(&path, doc.to_string()).context("writing Cargo.toml")?;
  Ok(true)
}

/// Run the release. On success returns the GitHub release URL. On error the caller unwinds
/// `journal`, which by then holds an undo for every step that completed.
///
/// `journal: &mut Vec<Undo>` — a mutable borrow — is how the caller keeps ownership of the
/// journal while this function appends to it. If `execute` owned the journal, a `?` early
/// return would drop it and the caller would have nothing to unwind.
pub fn execute(plan: &Plan, deps: &Deps, journal: &mut Vec<Undo>) -> Result<String> {
  let root = plan.root;
  let tag = plan.tag();
  let new = &plan.target.new;

  check_interrupt(deps)?;
  println!("[1/9] Snapshotting files...");
  journal.push(Undo::RestoreFiles(snapshot::take(root)?));

  // Recorded before any commit so the rollback knows where the branch was.
  let pre_sha = git::head_sha(root)?;
  // `Some(sha)` once the bump commit exists; `None` in no-bump mode.
  let mut bump_sha: Option<String> = None;
  // Only in bump mode: no-bump releases never touch these files.
  let mut bases = Vec::new();
  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[2/9] Bumping version to {new}...");
    bases = stage_clean_base(root)?;
    let mut args = vec!["version"];
    for b in plan.bumps {
      args.push("--bump");
      args.push(b);
    }
    run_ok(deps, root, "uv", &args)?;
    // Keep the Rust side in step. `cargo update --workspace` refreshes Cargo.lock's entries
    // for the workspace crates only (no dependency upgrades). Its failure is a real error,
    // unlike the old script's `|| true`.
    if set_cargo_version(root, new)? && root.join("Cargo.lock").is_file() {
      run_ok(deps, root, "cargo", &["update", "--workspace", "--quiet"])?;
    }
    check_interrupt(deps)?;
    println!("[3/9] uv lock...");
    run_ok(deps, root, "uv", &["lock"])?;
  }

  check_interrupt(deps)?;
  println!("[4/9] Building...");
  snapshot::clear_dist(root)?;
  run_ok(deps, root, "uv", &["build"])?;

  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[5/9] Committing...");
    // A file the tools created during the run (say, a first `Cargo.lock`) has no base;
    // add it as "no HEAD, no index, no prior working copy" so it is committed as-is.
    for p in TRACKED {
      if root.join(p).is_file() && !bases.iter().any(|b| b.path == p) {
        bases.push(TrackedBase {
          path: p.to_string(),
          head: None,
          worktree: None,
          index: IndexEntry {
            path: p.to_string(),
            staged: None,
          },
          index_bytes: None,
        });
      }
    }
    // The sha is returned by the commit itself (which also journals its own undo), so the
    // push step can reuse it and no fallible call sits between mutating the remote and
    // journaling that undo either.
    let sha = commit_release_edits(root, &bases, &format!("Bump version to {new}"), &pre_sha, journal)?;
    bump_sha = Some(sha);
  }

  check_interrupt(deps)?;
  println!("[6/9] Tagging {tag}...");
  git::create_annotated_tag(root, &tag, &format!("Version {new}"))?;
  journal.push(Undo::DeleteLocalTag(tag.clone()));

  check_interrupt(deps)?;
  println!("[7/9] Publishing to {}...", plan.cfg.index_name);
  // Credentials are deliberately *not* on the command line: uv reads
  // `UV_INDEX_<NAME>_USERNAME` / `_PASSWORD` from the environment it inherits (the same
  // variables `config::resolve` required), and argv would leak the password into process
  // listings and into `run_ok`'s error text.
  let devpi_url = plan.cfg.devpi_url(new);
  if let Err(e) = run_ok(deps, root, "uv", &["publish", "--index", &plan.cfg.index_name]) {
    // A non-zero exit is not proof nothing landed: the wheel can upload before the sdist
    // fails. Probe, and queue the delete if anything is there. If the probe itself errors
    // we cannot tell, so assume the worst — `delete` treats "not found" as success.
    let landed = deps
      .devpi
      .exists(&devpi_url, &plan.cfg.username, &plan.cfg.password)
      .unwrap_or(true);
    if landed {
      journal.push(Undo::DeleteDevpi {
        url: devpi_url,
        index_name: plan.cfg.index_name.clone(),
      });
    }
    return Err(e);
  }
  journal.push(Undo::DeleteDevpi {
    url: devpi_url,
    index_name: plan.cfg.index_name.clone(),
  });

  check_interrupt(deps)?;
  println!("[8/9] Pushing...");
  if plan.bumping() {
    // One `--atomic` push for both refs: the server takes both or neither, so a rejected
    // ref cannot leave the branch moved with no undo for it.
    git::push_refs(deps.runner, root, &[plan.branch, &tag])?;
    journal.push(Undo::DeleteRemoteTag(tag.clone()));
    journal.push(Undo::ForcePushBranch {
      branch: plan.branch.to_string(),
      // Set in step 5, which always runs in bump mode; `expect` documents that invariant.
      bump_sha: bump_sha.expect("bump mode commits before pushing"),
      pre_sha,
    });
  } else {
    git::push_refs(deps.runner, root, &[&tag])?;
    journal.push(Undo::DeleteRemoteTag(tag.clone()));
  }

  check_interrupt(deps)?;
  println!("[9/9] Creating GitHub release {tag}...");
  // No shell here, so `dist/*` is expanded by us, not by bash.
  let artifacts = snapshot::dist_artifacts(root)?;
  let mut args: Vec<String> = vec!["release".into(), "create".into(), tag.clone()];
  args.extend(artifacts.iter().map(|p| p.to_string_lossy().into_owned()));
  args.extend(["--title".to_string(), tag.clone()]);
  match plan.notes {
    Some(n) => args.extend(["--notes".to_string(), n.to_string()]),
    None => args.push("--generate-notes".into()),
  }
  // Captured (not inherited) because gh prints the release URL, which we return.
  let out = deps.runner.run_capture("gh", &args, root)?;
  if !out.success() {
    // `gh release create` can create the release and then fail uploading an asset. Probe
    // before giving up so a half-made release is still deleted by the rollback. If the
    // probe itself errors, assume the worst and queue the delete anyway.
    let view: Vec<String> = ["release", "view", &tag].iter().map(|s| s.to_string()).collect();
    let created = match deps.runner.run_capture("gh", &view, root) {
      Ok(o) if o.success() => true,
      Ok(o) if o.stderr.contains(crate::preflight::GH_NOT_FOUND) => false,
      _ => true,
    };
    if created {
      journal.push(Undo::DeleteGithubRelease(tag));
    }
    bail!("gh release create failed: {}{}", out.stdout.trim(), out.stderr.trim());
  }
  journal.push(Undo::DeleteGithubRelease(tag));
  Ok(out.stdout.trim().to_string())
}
