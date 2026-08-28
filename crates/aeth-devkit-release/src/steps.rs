//! The nine forward steps of a release. Each pushes its undo onto the journal on success.
//!
//! Ordering is the point of this module. The old script pushed to the remote *before*
//! building, so a plain build failure ended in a force-push. Here every purely local step
//! (snapshot, bump, lock, build, commit, tag) happens first, the index publish next, and
//! the two remote-git/GitHub steps last — so the further a run gets, the more it has
//! already proven, and the expensive compensations are reserved for the rare late failure.

use std::path::Path;
// `Ordering` here is the memory-ordering enum for atomics, not the comparison one.
use std::sync::atomic::Ordering;

use anyhow::{Context as _, Result, bail};
use toml_edit::DocumentMut;

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

/// Abort between steps if Ctrl-C was pressed. `SeqCst` is the strongest (and simplest to
/// reason about) memory ordering; the cost is irrelevant at this frequency.
fn check_interrupt(deps: &Deps) -> Result<()> {
  if deps.interrupted.load(Ordering::SeqCst) {
    bail!("interrupted")
  } else {
    Ok(())
  }
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
  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[2/9] Bumping version to {new}...");
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
    // Only the tracked files that exist, and only them — `commit_paths` stages and commits
    // exactly these, so anything else the user had staged stays out of the bump commit.
    let paths: Vec<String> = TRACKED.iter().filter(|p| root.join(p).is_file()).map(|p| p.to_string()).collect();
    git::commit_paths(root, &paths, &format!("Bump version to {new}"))?;
    // Read once, here, while nothing remote has happened yet. The push step reuses it so
    // no fallible call sits between mutating the remote and journaling its undo.
    let sha = git::head_sha(root)?;
    journal.push(Undo::ResetCommit {
      bump_sha: sha.clone(),
      pre_sha: pre_sha.clone(),
      paths,
    });
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
      journal.push(Undo::DeleteDevpi { url: devpi_url });
    }
    return Err(e);
  }
  journal.push(Undo::DeleteDevpi { url: devpi_url });

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
    let created = deps.runner.run_capture("gh", &view, root).map(|o| o.success()).unwrap_or(true);
    if created {
      journal.push(Undo::DeleteGithubRelease(tag));
    }
    bail!("gh release create failed: {}{}", out.stdout.trim(), out.stderr.trim());
  }
  journal.push(Undo::DeleteGithubRelease(tag));
  Ok(out.stdout.trim().to_string())
}
