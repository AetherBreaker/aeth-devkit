//! The forward steps of a release. Each pushes its undo onto the journal on success.
//!
//! Ordering is the point of this module. Every purely local step (snapshot, bump, lock,
//! commit, tag) happens first and the remote-git/GitHub steps last, so the further a run
//! gets, the more it has already proven, and the expensive compensations are reserved for
//! the rare late failure. Building and publishing are not steps at all any more: the
//! GitHub release created in step 7 triggers the release workflow, which owns them.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::commit::{self, TrackedBase};
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
  pub no_wait: bool,
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
  s += "  1. snapshot pyproject.toml, uv.lock, Cargo.toml, Cargo.lock\n";
  if plan.bumping() {
    s += &format!(
      "  2. uv version --bump {} ; update Cargo.toml ; cargo update --workspace\n",
      plan.bumps.join(" --bump ")
    );
    s += "  3. uv lock\n";
    s += &format!("  4. git commit \"Bump version to {}\"\n", plan.target.new);
  }
  s += &format!("  5. git tag -a {tag}\n");
  s += &format!(
    "  6. git push origin {}{tag}\n",
    if plan.bumping() {
      format!("{} ", plan.branch)
    } else {
      String::new()
    }
  );
  s += &format!(
    "  7. gh release create {tag} ({})\n",
    plan.notes.map_or("--generate-notes".to_string(), |n| format!("--notes {n:?}"))
  );
  if plan.no_wait {
    s += "  8. (skipped: --no-wait) wait for the release workflow\n";
  } else {
    s += &format!(
      "  8. wait for the release workflow, then verify {}=={} on {}\n",
      plan.cfg.package,
      plan.target.new,
      plan.cfg.target.label()
    );
  }
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

/// Commit the release's edits and only those, then put the user's edits back — the shared
/// quiet-commit machinery (`aeth_devkit_core::commit`), plus the release's journaling: the
/// `ResetCommit` undo is pushed the moment the commit exists, *before* the index and
/// working tree are touched, so a failure in that last stretch still rolls the commit back.
fn commit_release_edits(root: &Path, bases: &[TrackedBase], message: &str, pre_sha: &str, journal: &mut Vec<Undo>) -> Result<String> {
  commit::commit_on_clean_base(root, bases, message, "the version bump", &mut |sha| {
    journal.push(Undo::ResetCommit {
      bump_sha: sha.to_string(),
      pre_sha: pre_sha.to_string(),
      index: bases.iter().map(|b| b.index.clone()).collect(),
    });
  })
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
  println!("[1/8] Snapshotting files...");
  journal.push(Undo::RestoreFiles(snapshot::take(root)?));

  // Recorded before any commit so the rollback knows where the branch was.
  let pre_sha = git::head_sha(root)?;
  // `Some(sha)` once the bump commit exists; `None` in no-bump mode.
  let mut bump_sha: Option<String> = None;
  // Only in bump mode: no-bump releases never touch these files.
  let mut bases = Vec::new();
  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[2/8] Bumping version to {new}...");
    bases = commit::stage_clean_base(root, &TRACKED)?;
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
    println!("[3/8] uv lock...");
    run_ok(deps, root, "uv", &["lock"])?;
  }

  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[4/8] Committing...");
    // A file the tools created during the run (say, a first `Cargo.lock`) has no base;
    // add one so it is committed as-is.
    commit::absorb_created(root, &TRACKED, &mut bases);
    // The sha is returned by the commit itself (which also journals its own undo), so the
    // push step can reuse it and no fallible call sits between mutating the remote and
    // journaling that undo either.
    let sha = commit_release_edits(root, &bases, &format!("Bump version to {new}"), &pre_sha, journal)?;
    bump_sha = Some(sha);
  }

  check_interrupt(deps)?;
  println!("[5/8] Tagging {tag}...");
  git::create_annotated_tag(root, &tag, &format!("Version {new}"))?;
  journal.push(Undo::DeleteLocalTag(tag.clone()));
  // The tag object's identity, recorded while it is unambiguously ours. Every remote-tag
  // compensation carries it, so a rollback can only ever delete *this* tag — a same-named
  // tag some concurrent publisher pushed in the meantime fails the lease and is left alone.
  let tag_sha = git::tag_object_sha(root, &tag)?;

  check_interrupt(deps)?;
  println!("[6/8] Pushing...");
  // Shared by both failure paths below: after a failed push, did *our* tag land? Only a
  // remote tag whose object id equals `tag_sha` is ours — a same-named tag with another id
  // is a concurrent publisher's, and rollback must leave it. A probe error means we cannot
  // tell; assume ours (the lease on the delete still protects a foreign tag).
  let tag_landed = |deps: &Deps| {
    git::remote_tag_sha(deps.runner, root, &tag)
      .map(|sha| sha.as_deref() == Some(tag_sha.as_str()))
      .unwrap_or(true)
  };
  if plan.bumping() {
    // Set in step 5, which always runs in bump mode; `expect` documents that invariant.
    let bump = bump_sha.expect("bump mode commits before pushing");
    // One `--atomic` push for both refs: the server takes both or neither, so a rejected
    // ref cannot leave the branch moved with no undo for it.
    if let Err(e) = git::push_refs(deps.runner, root, &[plan.branch, &tag]) {
      // A failed push is not proof the remote stayed put: the server can apply both refs
      // and the client lose the connection before hearing so. Probe each ref and journal
      // its undo only for what actually landed *and is ours*; both compensations are
      // guarded by leases, so even an assume-the-worst probe cannot destroy foreign refs.
      if tag_landed(deps) {
        journal.push(Undo::DeleteRemoteTag {
          tag: tag.clone(),
          expected: tag_sha.clone(),
        });
      }
      // The branch "landed" only if the remote now points at our bump commit; any other
      // sha is someone else's work, which the rollback must not rewind.
      let landed = git::remote_branch_sha(deps.runner, root, plan.branch)
        .map(|sha| sha.as_deref() == Some(bump.as_str()))
        .unwrap_or(true);
      if landed {
        journal.push(Undo::ForcePushBranch {
          branch: plan.branch.to_string(),
          bump_sha: bump,
          pre_sha,
        });
      }
      return Err(e);
    }
    journal.push(Undo::DeleteRemoteTag {
      tag: tag.clone(),
      expected: tag_sha.clone(),
    });
    journal.push(Undo::ForcePushBranch {
      branch: plan.branch.to_string(),
      bump_sha: bump,
      pre_sha,
    });
  } else {
    if let Err(e) = git::push_refs(deps.runner, root, &[&tag]) {
      // Same ambiguity as above, tag only.
      if tag_landed(deps) {
        journal.push(Undo::DeleteRemoteTag {
          tag: tag.clone(),
          expected: tag_sha.clone(),
        });
      }
      return Err(e);
    }
    journal.push(Undo::DeleteRemoteTag {
      tag: tag.clone(),
      expected: tag_sha.clone(),
    });
  }

  check_interrupt(deps)?;
  // Runs of an earlier release of this tag (removed in pre-flight) still exist, and
  // `gh run list` would hand back the newest of them before the new one starts; step 8
  // waits for a run that is not in this list.
  let known = if plan.no_wait {
    Vec::new()
  } else {
    crate::ci::list_runs(deps.runner, root, &tag)?
  };
  println!("[7/8] Creating GitHub release {tag}...");
  // No files: the release workflow attaches the artefacts it builds.
  let mut args: Vec<String> = vec!["release".into(), "create".into(), tag.clone(), "--title".into(), tag.clone()];
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
  journal.push(Undo::DeleteGithubRelease(tag.clone()));
  let release_url = out.stdout.trim().to_string();

  check_interrupt(deps)?;
  if plan.no_wait {
    println!(
      "[8/8] Not waiting for the release workflow (--no-wait): {}",
      crate::ci::actions_url(root).unwrap_or_else(|| "see the repository's Actions tab".into())
    );
    return Ok(release_url);
  }
  // A failed or missing run is a failed release: the journal is unwound like any other
  // late failure (release deleted, tag and branch rewound under their leases). A tag with
  // no artefacts is exactly the state the completeness check exists to reject, so it is
  // better rolled back than left for a later `devkit release` to trip over.
  println!("[8/8] Waiting for the release workflow...");
  let run_url = crate::ci::wait_for_run(deps, root, &tag, &known)?;
  crate::ci::verify_published(deps, root, plan.cfg, new)?;
  println!("  workflow succeeded: {run_url}");
  Ok(release_url)
}
