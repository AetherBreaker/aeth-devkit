//! `devkit lock` — bump a dependency pin to the latest stable release on its index, run
//! `uv sync`, and commit `uv.lock` (and the pin change).
//!
//! The commit is quiet (see `aeth_devkit_core::commit`): the pin update and sync run
//! against the files as committed in `HEAD`, the commit carries only this command's
//! changes, and the user's uncommitted edits are replayed back on top afterwards.
//! Uncommitted edits that overlap the pin update make the commit impossible; the run is
//! rejected and everything is rolled back.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::commit;
use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::paths::strip_verbatim;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::{git, pyproject, version};

pub const COMMIT_SUBJECT: &str = "Update uv.lock";
pub const DEFAULT_PACKAGE: &str = "aeth-devkit";
const PUBLIC_INDEX: &str = "https://pypi.org/simple/";
/// The files this command may rewrite — and the only files its commit may contain.
const MANAGED: [&str; 2] = ["pyproject.toml", "uv.lock"];

/// Bump a dependency pin to the latest stable release, `uv sync`, and commit uv.lock.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-lock", version, about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Dependency pin(s) to bump; repeatable. Defaults to aeth-devkit.
  #[arg(long = "package", short = 'p')]
  pub package: Vec<String>,

  /// Do not commit uv.lock / pyproject.toml after syncing.
  #[arg(long)]
  pub no_commit: bool,

  /// Report what would change without writing, syncing, or committing.
  #[arg(long)]
  pub dry_run: bool,

  /// Extra arguments forwarded to `uv sync` (after `--`), appended to the default
  /// `--upgrade --all-extras` (forwarding a default again is harmless — it is dropped).
  #[arg(last = true)]
  pub uv_args: Vec<String>,
}

/// Exit codes: 0 ok, 3 commit failed (the pin update and lock changes were rolled back),
/// `uv sync`'s own code when it fails. Errors bubble up for the caller to print (exit 2).
pub fn run(args: &Args, index: &dyn IndexClient, runner: &dyn Runner) -> Result<ExitCode> {
  let root = args
    .root
    .canonicalize()
    .with_context(|| format!("resolving {}", args.root.display()))?;
  let root = strip_verbatim(root);

  // When committing, the pin update and sync run against `pyproject.toml`/`uv.lock` as
  // committed in `HEAD`, so the commit carries only this command's changes and the user's
  // uncommitted edits are replayed back on top afterwards. Deliberately no branch check:
  // unlike `release`, updating the pin is safe on any branch.
  let tracked = git::is_git_tracked(&root);
  let committing = !args.dry_run && !args.no_commit && tracked;
  let mut bases = if committing {
    Some(commit::stage_clean_base(&root, &MANAGED)?)
  } else {
    None
  };

  // The pin edit and `uv sync`, with any failure putting the user's files back first.
  match bump_and_sync(args, &root, index, runner) {
    Ok(None) => {}
    Ok(Some(code)) => {
      // A `uv sync` failure (dry runs never stage): restore before propagating its code.
      if let Some(bases) = &bases {
        commit::restore_worktree(&root, bases)?;
      }
      return Ok(code);
    }
    Err(e) => {
      if let Some(bases) = &bases {
        commit::restore_worktree(&root, bases)?;
      }
      return Err(e);
    }
  }

  let Some(bases) = &mut bases else {
    if !args.dry_run && !args.no_commit && !tracked {
      println!("Not a git repository; skipping commit");
    }
    return Ok(ExitCode::SUCCESS);
  };
  commit::absorb_created(&root, &MANAGED, bases);
  match commit::commit_or_rollback(&root, bases, COMMIT_SUBJECT, "the pin update") {
    Ok(Some(hash)) => {
      println!("Committed as {hash}.");
      Ok(ExitCode::SUCCESS)
    }
    Ok(None) => {
      println!("uv.lock is up to date; nothing to commit");
      Ok(ExitCode::SUCCESS)
    }
    Err(e) => {
      eprintln!("warning: not committed; the pin update and sync were rolled back: {e:#}");
      Ok(ExitCode::from(3))
    }
  }
}

/// Bump the requested pins in `pyproject.toml` on disk and run `uv sync`. `Ok(Some(code))`
/// is an early exit (a dry run, or uv's own failure code); `Ok(None)` means synced.
fn bump_and_sync(args: &Args, root: &Path, index: &dyn IndexClient, runner: &dyn Runner) -> Result<Option<ExitCode>> {
  let pyproject_path = root.join("pyproject.toml");
  let original = std::fs::read_to_string(&pyproject_path).with_context(|| format!("{} not found", pyproject_path.display()))?;
  let mut doc: DocumentMut = original.parse().context("parsing pyproject.toml")?;

  let targets: Vec<&str> = if args.package.is_empty() {
    vec![DEFAULT_PACKAGE]
  } else {
    args.package.iter().map(String::as_str).collect()
  };
  for pkg in targets {
    bump_pin(&mut doc, pkg, index)?;
  }

  let updated = doc.to_string();
  if updated != original {
    if args.dry_run {
      println!("Would write pyproject.toml");
    } else {
      std::fs::write(&pyproject_path, &updated).context("writing pyproject.toml")?;
    }
  }

  // Upgrading within the pins and syncing every extra is what a lock run is for, so both
  // are on by default; forwarded args come after. A forwarded copy of a default (`poe
  // lock -U`, say) is dropped rather than passed twice.
  let mut uv_args: Vec<String> = ["sync", "--upgrade", "--all-extras"].iter().map(|s| s.to_string()).collect();
  for a in &args.uv_args {
    if a == "-U" || a == "--upgrade" || a == "--all-extras" {
      continue;
    }
    uv_args.push(a.clone());
  }
  if args.dry_run {
    println!("Would run: uv {}", uv_args.join(" "));
    return Ok(Some(ExitCode::SUCCESS));
  }
  match runner.run_inherit("uv", &uv_args, root)? {
    Some(0) => Ok(None),
    Some(code) => Ok(Some(ExitCode::from(code.clamp(1, 255) as u8))),
    None => bail!("uv sync was terminated by a signal"),
  }
}

/// `run` with the real HTTP index client and process runner.
pub fn run_real(args: &Args) -> Result<ExitCode> {
  run(
    args,
    &aeth_devkit_core::index::HttpIndexClient::default(),
    &aeth_devkit_core::process::SystemRunner,
  )
}

/// Rewrite `pkg`'s requirement in `doc` to the latest stable version on its index.
fn bump_pin(doc: &mut DocumentMut, pkg: &str, index: &dyn IndexClient) -> Result<()> {
  let Some(req) = pyproject::find_requirement(doc, pkg) else {
    println!("No {pkg} pin found in pyproject.toml; skipping pin update");
    return Ok(());
  };
  let simple_url = pyproject::index_url_for(doc, pkg).unwrap_or_else(|| PUBLIC_INDEX.to_string());
  println!("Querying {simple_url} for latest stable {pkg} version...");
  let versions = index.versions(&simple_url, pkg)?;
  let latest = version::latest_stable(versions.iter().map(String::as_str))
    .with_context(|| format!("No stable release versions found for {pkg} on {simple_url}"))?;
  let Some(new_spec) = pyproject::set_requirement_version(&req.spec, &latest) else {
    println!(
      "{pkg} requirement \"{}\" is neither a simple >=/==/~= pin nor a one-major >=A,<B range; skipping pin update (latest is {latest})",
      req.spec
    );
    return Ok(());
  };
  if new_spec == req.spec {
    println!("{pkg} pin already at {latest}");
  } else {
    pyproject::replace_requirement(doc, &req, &new_spec);
    println!("Updated {pkg} pin to {latest}");
  }
  Ok(())
}
