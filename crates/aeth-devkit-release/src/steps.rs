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
  run_ok_env(deps, root, program, args, &[])
}

/// Like [`run_ok`] but with extra variables set on the child.
///
/// Note what the error message does *not* contain: `env`. Only `args` is interpolated, so a
/// secret passed this way cannot reach a log, a terminal, or an `anyhow` chain.
fn run_ok_env(deps: &Deps, root: &Path, program: &str, args: &[&str], env: &[(&str, &str)]) -> Result<()> {
  let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
  match deps.runner.run_inherit_env(program, &owned, root, env)? {
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

/// After a failed `uv publish`: is the version now on the index *ours*?
///
/// `Ok(None)` — the version does not exist (nothing landed, nothing to compensate).
/// `Ok(Some(true))` — every stored release file is byte-identical to one of this run's
/// `dist/` artifacts (a partial or complete upload by us); an empty file list also counts,
/// as it carries no evidence against ownership.
/// `Ok(Some(false))` — the index holds a file we did not build: a concurrent publisher's
/// release, which a rollback must not delete.
fn devpi_version_is_ours(deps: &Deps, root: &Path, url: &str, cfg: &Config) -> Result<Option<bool>> {
  let Some(files) = deps.devpi.files(url, &cfg.username, &cfg.password)? else {
    return Ok(None);
  };
  // Our artifacts, matched by file name first (cheap), then by content (decisive).
  let local = snapshot::dist_artifacts(root)?;
  for (name, href) in &files {
    // `file_name()` is an `OsStr`; `to_string_lossy` makes it comparable to the index's name.
    let ours = local.iter().find(|p| p.file_name().is_some_and(|f| f.to_string_lossy() == *name));
    let Some(ours) = ours else {
      return Ok(Some(false)); // a file we never built cannot be our upload
    };
    let local_bytes = std::fs::read(ours).with_context(|| format!("reading {}", ours.display()))?;
    if deps.devpi.fetch(href, &cfg.username, &cfg.password)? != local_bytes {
      return Ok(Some(false));
    }
  }
  Ok(Some(true))
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
    // add one so it is committed as-is.
    commit::absorb_created(root, &TRACKED, &mut bases);
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
  // The tag object's identity, recorded while it is unambiguously ours. Every remote-tag
  // compensation carries it, so a rollback can only ever delete *this* tag — a same-named
  // tag some concurrent publisher pushed in the meantime fails the lease and is left alone.
  let tag_sha = git::tag_object_sha(root, &tag)?;

  check_interrupt(deps)?;
  println!("[7/9] Publishing to {}...", plan.cfg.index_name);
  // Credentials go in the child's *environment*, never on the command line — argv would put
  // the password in every process listing and in `run_ok`'s error text.
  //
  // The variable names matter, and are not the ones `config::resolve` reads. uv keeps two
  // separate sets: `UV_INDEX_<NAME>_USERNAME` / `_PASSWORD` authenticate *reads* from an
  // index during resolution, while `uv publish` looks for `UV_PUBLISH_USERNAME` /
  // `_PASSWORD`. Passing only the first pair left uv with no publish credentials, so it
  // stopped and prompted on the terminal at step 7 of 9 — after the commit and tag were
  // already made — and only reached the index pair as a fallback once the prompt came back
  // empty. Handing it the names it actually documents removes the prompt.
  let devpi_url = plan.cfg.devpi_url(new);
  let publish_env = [
    ("UV_PUBLISH_USERNAME", plan.cfg.username.as_str()),
    ("UV_PUBLISH_PASSWORD", plan.cfg.password.as_str()),
  ];
  if let Err(e) = run_ok_env(deps, root, "uv", &["publish", "--index", &plan.cfg.index_name], &publish_env) {
    // A non-zero exit is not proof nothing landed: the wheel can upload before the sdist
    // fails. But existence alone is not proof it was *us*, either — a concurrent release
    // of the same version could have won the race after pre-flight, and deleting theirs
    // would be worse than leaving ours. So compare the stored files byte-for-byte with
    // this run's `dist/` artifacts, and only compensate what is provably ours. If the
    // probe itself errors we cannot tell, so assume the worst (a partial upload by us is
    // far likelier than a same-second concurrent publisher).
    match devpi_version_is_ours(deps, root, &devpi_url, plan.cfg) {
      Ok(None) => {}
      Ok(Some(true)) | Err(_) => journal.push(Undo::DeleteDevpi {
        url: devpi_url,
        index_name: plan.cfg.index_name.clone(),
      }),
      Ok(Some(false)) => {
        eprintln!("WARNING: {devpi_url} exists but holds files that are not this run's artifacts; leaving it in place");
      }
    }
    return Err(e);
  }
  journal.push(Undo::DeleteDevpi {
    url: devpi_url,
    index_name: plan.cfg.index_name.clone(),
  });

  check_interrupt(deps)?;
  println!("[8/9] Pushing...");
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

#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::AtomicBool;

  use aeth_devkit_core::devpi::{DeleteOutcome, DevpiClient};
  use aeth_devkit_core::process::RecordingRunner;

  use crate::prompt::ScriptedPrompt;

  /// A devpi whose `files`/`fetch` answers are set directly, for ownership-check tests.
  struct ScriptedFiles {
    files: Option<Vec<(String, String)>>,
    fetch: Vec<u8>,
  }

  impl DevpiClient for ScriptedFiles {
    fn exists(&self, _url: &str, _u: &str, _p: &str) -> Result<bool> {
      Ok(self.files.is_some())
    }
    fn delete(&self, _url: &str, _u: &str, _p: &str) -> Result<DeleteOutcome> {
      Ok(DeleteOutcome::Deleted)
    }
    fn files(&self, _url: &str, _u: &str, _p: &str) -> Result<Option<Vec<(String, String)>>> {
      Ok(self.files.clone())
    }
    fn fetch(&self, _href: &str, _u: &str, _p: &str) -> Result<Vec<u8>> {
      Ok(self.fetch.clone())
    }
  }

  #[test]
  fn devpi_ownership_is_decided_by_file_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir(root.join("dist")).unwrap();
    std::fs::write(root.join("dist/demo-1.0.1-py3-none-any.whl"), b"ours").unwrap();
    let runner = RecordingRunner::new(0);
    let prompt = ScriptedPrompt::new(&[]);
    let flag = AtomicBool::new(false);
    let cfg = Config {
      package: "demo".into(),
      index_name: "I".into(),
      publish_url: "https://x/".into(),
      username: "u".into(),
      password: "p".into(),
    };
    // One closure builds the `Deps` around each scripted client and runs the check, so
    // every case below reads as "given these remote files, the verdict is …".
    let check = |files: Option<Vec<(String, String)>>, fetch: &[u8]| {
      let client = ScriptedFiles {
        files,
        fetch: fetch.to_vec(),
      };
      let deps = Deps {
        runner: &runner,
        devpi: &client,
        prompt: &prompt,
        env: &|_| None,
        interrupted: &flag,
      };
      devpi_version_is_ours(&deps, root, "https://x/demo/1.0.1", &cfg).unwrap()
    };
    let whl = |bytes_url: &str| Some(vec![("demo-1.0.1-py3-none-any.whl".to_string(), bytes_url.to_string())]);
    // Version absent → nothing landed.
    assert_eq!(check(None, b""), None);
    // Same file name, same bytes → ours (a partial upload to compensate).
    assert_eq!(check(whl("h"), b"ours"), Some(true));
    // Same file name, different bytes → a concurrent publisher's release.
    assert_eq!(check(whl("h"), b"theirs"), Some(false));
    // A file this run never built → foreign, no matter its content.
    assert_eq!(check(Some(vec![("other-9.9.9.tar.gz".into(), "h".into())]), b""), Some(false));
    // Version exists but stores no files → no evidence against ownership.
    assert_eq!(check(Some(vec![]), b""), Some(true));
  }
}
