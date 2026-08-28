//! Read-only checks that run before anything is mutated.
//!
//! The principle: every reason a release *could* fail late should be caught here, while
//! failing costs nothing. Tools missing, branch behind its upstream, Cargo/pyproject
//! versions drifted, dirty tree, artefacts already published — all are decided before the
//! first file is touched, so rollback is needed only for surprises.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::devpi::DeleteOutcome;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::{cargo_toml, git};

use crate::Deps;
use crate::config::Config;
use crate::prompt::{Prompt, confirm_force};
use crate::report::Existing;

/// What `gh release view` prints on stderr for a missing release. Any *other* non-zero
/// exit (auth, network, wrong repo) is a real error, not "absent".
pub const GH_NOT_FOUND: &str = "release not found";

/// The version being released, and the one currently in `pyproject.toml`.
/// In no-bump mode the two are equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
  pub current: String,
  pub new: String,
}

/// `&["a", "b"]` → `vec!["a".to_string(), "b".to_string()]`, for the `Runner` API.
fn s(args: &[&str]) -> Vec<String> {
  args.iter().map(|a| a.to_string()).collect()
}

/// `git`, `uv`, and `gh` must all answer `--version`.
pub fn check_tools(runner: &dyn Runner, root: &Path) -> Result<()> {
  // `filter` keeps the tools that *fail*; the closure maps a run error or a non-zero exit
  // to `false` via `map(…).unwrap_or(false)`, so any kind of trouble counts as missing.
  let missing: Vec<&str> = ["git", "uv", "gh"]
    .into_iter()
    .filter(|tool| {
      !runner
        .run_capture(tool, &s(&["--version"]), root)
        .map(|o| o.success())
        .unwrap_or(false)
    })
    .collect();
  if missing.is_empty() {
    Ok(())
  } else {
    bail!("required tools not found on PATH: {}", missing.join(", "))
  }
}

/// The branch must be `main`, its upstream must be `origin/main` (the ref the release
/// will push), and it must not be behind it. Returns the branch name.
///
/// `main` is hard-coded rather than configurable: `undo` already pushes to
/// `refs/heads/main`, so a release from any other branch would be un-rescindable anyway.
///
/// The branch name is read through the runner (not `git::current_branch`) so a test can
/// script it without a real upstream; everything here is a remote-flavoured question.
pub fn check_branch(runner: &dyn Runner, root: &Path) -> Result<String> {
  let Some(up) = git::upstream(runner, root)? else {
    bail!("the current branch has no upstream; push it first (git push -u origin <branch>)");
  };
  let branch = runner.run_capture("git", &s(&["rev-parse", "--abbrev-ref", "HEAD"]), root)?;
  let branch = branch.stdout.trim().to_string();
  // Checked before the fetch: a wrong branch is a local fact, no point paying for network.
  if branch != "main" {
    bail!("releases are cut from main, but the current branch is {branch}; switch to main first");
  }
  // The fetch below refreshes `origin`, and the release pushes `origin/main`; if `@{u}`
  // named anything else the behind-count would be answering a different question.
  if up != "origin/main" {
    bail!("main must track origin/main to release, but its upstream is {up}; fix with git branch -u origin/main");
  }
  git::fetch(runner, root)?;
  let behind = git::behind_count(runner, root)?;
  if behind > 0 {
    bail!("{branch} is {behind} commit(s) behind {up}; pull or rebase first");
  }
  Ok(branch)
}

/// Parse `uv version` output: `name old => new` (with `--bump … --dry-run`) or `name ver`.
pub fn parse_uv_version(stdout: &str) -> Result<Target> {
  // `lines().next()` is the first line, or `None` for empty input → "".
  let line = stdout.lines().next().unwrap_or("").trim();
  let words: Vec<&str> = line.split_whitespace().collect();
  // Slice patterns with a literal in the middle: `"=>"` must match exactly.
  match words.as_slice() {
    [_, current, "=>", new] => Ok(Target {
      current: current.to_string(),
      new: new.to_string(),
    }),
    [_, current] => Ok(Target {
      current: current.to_string(),
      new: current.to_string(),
    }),
    _ => bail!("could not parse `uv version` output: {line:?}"),
  }
}

/// Ask uv what the version would become (`--dry-run` when bumping, so nothing changes).
pub fn target_version(runner: &dyn Runner, root: &Path, bumps: &[String]) -> Result<Target> {
  let mut args = vec!["version".to_string()];
  for b in bumps {
    args.push("--bump".into());
    args.push(b.clone());
  }
  if !bumps.is_empty() {
    args.push("--dry-run".into());
  }
  let out = runner.run_capture("uv", &args, root)?;
  if !out.success() {
    bail!("uv version failed: {}", out.stderr.trim());
  }
  parse_uv_version(&out.stdout)
}

/// If `Cargo.toml` exists and declares a version, it must equal the current Python version.
/// (This is what catches the drift the old `sed` silently caused.)
pub fn check_cargo_version(root: &Path, current: &str) -> Result<()> {
  let path = root.join("Cargo.toml");
  if !path.is_file() {
    return Ok(());
  }
  let doc: DocumentMut = std::fs::read_to_string(&path)
    .context("reading Cargo.toml")?
    .parse()
    .context("parsing Cargo.toml")?;
  match cargo_toml::read_version(&doc) {
    // A guard compares the bound value with `current`.
    Some(v) if v == current => Ok(()),
    Some(v) => bail!("Cargo.toml version {v} does not match pyproject.toml version {current}; fix Cargo.toml first"),
    None => Ok(()),
  }
}

/// Refuse if any release-managed file is mid merge-conflict. Unlike a merely dirty tree
/// (which the release works around and `--force` may wave through), unmerged index entries
/// have no automatic resolution — three competing versions of the file exist and only the
/// user knows which is right — so this is a hard error, not a prompt.
///
/// The check itself lives in `git::index_entries`, which fails on unmerged rows because it
/// cannot represent them; calling it here surfaces that error before anything is mutated
/// instead of mid-release in step 2.
pub fn check_unmerged(root: &Path) -> Result<()> {
  let paths: Vec<String> = crate::snapshot::TRACKED.iter().map(|p| p.to_string()).collect();
  git::index_entries(root, &paths).map(|_| ())
}

/// Show uncommitted changes and require `force` (typed or flagged) to continue.
///
/// Edits to the files the release itself rewrites are fine too: step 5 builds the bump
/// commit from clean `HEAD` content and re-applies the user's edits around it, so they are
/// neither swept into the commit nor lost (see `steps::commit_release_edits`).
pub fn confirm_dirty_tree(root: &Path, force: bool, prompt: &dyn Prompt) -> Result<()> {
  if git::status_porcelain(root)?.is_empty() {
    return Ok(());
  }
  eprintln!(
    "WARNING: You have uncommitted changes (edits to the release-managed files are kept out of the bump commit):\n{}",
    git::status_short(root)?
  );
  if force {
    eprintln!("WARNING: Proceeding anyway (--force).");
    return Ok(());
  }
  if confirm_force(prompt, false, "Continue with a dirty tree? Type 'force' to continue:")? {
    Ok(())
  } else {
    bail!("aborted: working tree is dirty")
  }
}

/// Probe every place `v<version>` could already exist.
pub fn probe(deps: &Deps, root: &Path, cfg: &Config, version: &str) -> Result<Existing> {
  let tag = format!("v{version}");
  // `--jq .url` makes gh print just the URL. Only a confirmed "release not found" is
  // absence; anything else non-zero would make the report lie, so it is an error.
  let gh = deps
    .runner
    .run_capture("gh", &s(&["release", "view", &tag, "--json", "url", "--jq", ".url"]), root)?;
  let github = if gh.success() {
    Some(gh.stdout.trim().to_string())
  } else if gh.stderr.contains(GH_NOT_FOUND) {
    None
  } else {
    bail!("gh release view {tag} failed: {}", gh.stderr.trim());
  };
  Ok(Existing {
    local_tag: git::tag_target(root, &tag)?,
    remote_tag: git::remote_tag_exists(deps.runner, root, &tag)?,
    github,
    devpi: deps.devpi.exists(&cfg.devpi_url(version), &cfg.username, &cfg.password)?,
  })
}

/// Remove whatever `probe` found, GitHub first (its `--cleanup-tag` also drops the remote
/// tag), then remote tag, devpi, local tag. Under `dry_run` only prints what it would do.
/// Commits are never rewound here — that is `rescind-release`'s job.
pub fn remove_existing(deps: &Deps, root: &Path, cfg: &Config, version: &str, ex: &Existing, dry_run: bool) -> Result<()> {
  let tag = format!("v{version}");
  let verb = if dry_run { "Would remove" } else { "Removing" };
  // This is the one destructive thing pre-flight does, so it honours Ctrl-C the same way
  // the forward steps do: checked before each deletion.
  deps.check_interrupt()?;
  if ex.github.is_some() {
    println!("  -> {verb} GitHub release {tag} (and its remote tag)");
    if !dry_run {
      let out = deps
        .runner
        .run_capture("gh", &s(&["release", "delete", &tag, "--yes", "--cleanup-tag"]), root)?;
      if !out.success() {
        bail!("gh release delete failed: {}", out.stderr.trim());
      }
    }
  }
  deps.check_interrupt()?;
  if ex.remote_tag {
    println!("  -> {verb} remote tag {tag}");
    if !dry_run {
      git::delete_remote_tag(deps.runner, root, &tag)?;
    }
  }
  deps.check_interrupt()?;
  if ex.devpi {
    println!("  -> {verb} {}=={version} from {}", cfg.package, cfg.index_name);
    if !dry_run {
      // Both outcomes are fine: the goal is "not there afterwards". Matching exhaustively
      // (rather than `let _ =`) means a future third variant would be a compile error here.
      match deps.devpi.delete(&cfg.devpi_url(version), &cfg.username, &cfg.password)? {
        DeleteOutcome::Deleted | DeleteOutcome::NotFound => {}
      }
    }
  }
  deps.check_interrupt()?;
  if ex.local_tag.is_some() {
    println!("  -> {verb} local tag {tag}");
    if !dry_run {
      git::delete_tag(root, &tag)?;
    }
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;

  #[test]
  fn parses_uv_version_output() {
    let t = parse_uv_version("aeth-devkit 7.0.2 => 7.0.3\n").unwrap();
    assert_eq!((t.current.as_str(), t.new.as_str()), ("7.0.2", "7.0.3"));
    let t = parse_uv_version("aeth-devkit 7.0.2\n").unwrap();
    assert_eq!((t.current.as_str(), t.new.as_str()), ("7.0.2", "7.0.2"));
    assert!(parse_uv_version("").is_err());
  }

  #[test]
  fn target_version_builds_bump_flags() {
    let r = RecordingRunner::new(0);
    r.script("uv", &["version"], 0, "demo 1.0.0 => 2.0.0a1\n");
    let t = target_version(&r, Path::new("."), &["major".into(), "alpha".into()]).unwrap();
    assert_eq!(t.new, "2.0.0a1");
    assert_eq!(
      r.calls_for("uv")[0],
      vec!["version", "--bump", "major", "--bump", "alpha", "--dry-run"]
    );
    let r2 = RecordingRunner::new(0);
    r2.script("uv", &["version"], 0, "demo 1.0.0\n");
    target_version(&r2, Path::new("."), &[]).unwrap();
    assert_eq!(r2.calls_for("uv")[0], vec!["version"]);
  }

  #[test]
  fn branch_check_requires_upstream_and_not_behind() {
    let r = RecordingRunner::new(0);
    r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 1, "");
    assert!(check_branch(&r, Path::new(".")).unwrap_err().to_string().contains("no upstream"));
    let r = RecordingRunner::new(0);
    r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
    r.script("git", &["rev-parse", "--abbrev-ref", "HEAD"], 0, "main\n");
    r.script("git", &["rev-list", "--count"], 0, "3\n");
    assert!(check_branch(&r, Path::new(".")).unwrap_err().to_string().contains("behind"));
  }

  #[test]
  fn branch_check_requires_origin_main_upstream() {
    let r = RecordingRunner::new(0);
    r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "upstream/main\n");
    r.script("git", &["rev-parse", "--abbrev-ref", "HEAD"], 0, "main\n");
    let err = check_branch(&r, Path::new(".")).unwrap_err().to_string();
    assert!(err.contains("upstream is upstream/main"), "{err}");
    assert!(r.calls_for("git").iter().all(|c| c[0] != "fetch"));
  }

  #[test]
  fn branch_check_requires_main() {
    let r = RecordingRunner::new(0);
    r.script(
      "git",
      &["rev-parse", "--abbrev-ref", "@{u}"],
      0,
      "origin/feature
",
    );
    r.script(
      "git",
      &["rev-parse", "--abbrev-ref", "HEAD"],
      0,
      "feature
",
    );
    let err = check_branch(&r, Path::new(".")).unwrap_err().to_string();
    assert!(err.contains("current branch is feature"), "{err}");
    // Refused before any fetch: the only git calls are the two rev-parses.
    assert_eq!(r.calls_for("git").len(), 2);
  }

  #[test]
  fn cargo_version_must_match() {
    let dir = tempfile::tempdir().unwrap();
    assert!(check_cargo_version(dir.path(), "1.0.0").is_ok());
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.9.0\"\n").unwrap();
    let e = check_cargo_version(dir.path(), "1.0.0").unwrap_err().to_string();
    assert!(e.contains("0.9.0") && e.contains("1.0.0"), "{e}");
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"1.0.0\"\n").unwrap();
    assert!(check_cargo_version(dir.path(), "1.0.0").is_ok());
  }
}
