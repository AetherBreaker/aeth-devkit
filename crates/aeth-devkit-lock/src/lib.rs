//! `devkit lock` — bump a dependency pin to the latest stable release on its index, run
//! `uv sync`, and commit `uv.lock` (and the pin change).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::{git, pyproject, version};

pub const COMMIT_SUBJECT: &str = "Update uv.lock";
pub const DEFAULT_PACKAGE: &str = "aeth-devkit";
const PUBLIC_INDEX: &str = "https://pypi.org/simple/";

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

  /// Extra arguments forwarded verbatim to `uv sync` (after `--`).
  #[arg(last = true)]
  pub uv_args: Vec<String>,
}

/// Exit codes: 0 ok, 3 synced but commit failed, `uv sync`'s own code when it fails.
/// Errors bubble up for the caller to print (exit 2).
pub fn run(args: &Args, index: &dyn IndexClient, runner: &dyn Runner) -> Result<ExitCode> {
  let root = args
    .root
    .canonicalize()
    .with_context(|| format!("resolving {}", args.root.display()))?;
  let root = strip_verbatim(root);
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

  let mut uv_args = vec!["sync".to_string()];
  uv_args.extend(args.uv_args.iter().cloned());
  if args.dry_run {
    println!("Would run: uv {}", uv_args.join(" "));
    return Ok(ExitCode::SUCCESS);
  }
  match runner.run_inherit("uv", &uv_args, &root)? {
    Some(0) => {}
    Some(code) => return Ok(ExitCode::from(code.clamp(1, 255) as u8)),
    None => bail!("uv sync was terminated by a signal"),
  }

  if args.no_commit {
    return Ok(ExitCode::SUCCESS);
  }
  if !git::is_git_tracked(&root) {
    println!("Not a git repository; skipping commit");
    return Ok(ExitCode::SUCCESS);
  }
  let paths = ["uv.lock", "pyproject.toml"];
  if !git::is_dirty(&root, &paths)? {
    println!("uv.lock is up to date; nothing to commit");
    return Ok(ExitCode::SUCCESS);
  }
  let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
  match git::commit_paths(&root, &owned, COMMIT_SUBJECT) {
    Ok(hash) => {
      println!("Committed as {hash}.");
      Ok(ExitCode::SUCCESS)
    }
    Err(e) => {
      eprintln!("warning: synced but not committed: {e:#}");
      Ok(ExitCode::from(3))
    }
  }
}

/// `run` with the real HTTP index client and process runner.
pub fn run_real(args: &Args) -> Result<ExitCode> {
  run(
    args,
    &aeth_devkit_core::index::HttpIndexClient,
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
      "{pkg} requirement \"{}\" is not a simple >=/==/~= pin; skipping pin update (latest is {latest})",
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

/// `\\?\D:\foo` → `D:\foo` (Windows canonicalize adds the verbatim prefix).
fn strip_verbatim(p: PathBuf) -> PathBuf {
  let s = p.to_string_lossy();
  match s.strip_prefix(r"\\?\") {
    Some(rest) => PathBuf::from(rest),
    None => p,
  }
}
