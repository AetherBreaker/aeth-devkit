//! Command-line surface of `devkit setup-project`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

/// Standardize a project's configuration from the templates shipped with aeth-devkit.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-setup", version, about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Directory containing the templates (defaults to the installed aeth_devkit package).
  #[arg(long)]
  pub templates_dir: Option<PathBuf>,

  /// Print the changes that would be made without writing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// Like --dry-run, but exit non-zero if anything would change.
  #[arg(long)]
  pub check: bool,

  /// Do not commit the changes. By default, when the project is git-tracked, the changed
  /// files (never env files) are committed with a standard message.
  #[arg(long)]
  pub no_commit: bool,
}

/// Exit codes: 0 ok, 1 `--check` found drift, 3 applied but commit failed. Errors bubble up
/// for the caller to print (exit 2).
pub fn run(args: &Args) -> Result<ExitCode> {
  let dry_run = args.dry_run || args.check;
  let templates = crate::templates::locate(args.templates_dir.as_deref())?;
  let mut changes = crate::run(&args.root, &templates, dry_run)?;
  let root = crate::context::strip_verbatim(args.root.canonicalize().unwrap_or(args.root.clone()));
  if !dry_run {
    match crate::format::format_pyproject(&root, &crate::format::SystemRunner, &mut changes)? {
      crate::format::Outcome::Formatted(_) => {}
      crate::format::Outcome::Unavailable => println!("note: tombi not found; skipping pyproject.toml formatting."),
      crate::format::Outcome::Failed { code } => {
        eprintln!("warning: tombi format exited with {code:?}; pyproject.toml left unformatted.");
      }
    }
  }
  for note in &changes.notes {
    println!("note: {note}");
  }
  if changes.is_empty() {
    println!("Nothing to do — project already matches the templates.");
    return Ok(ExitCode::SUCCESS);
  }
  let header = if dry_run { "Would change:" } else { "Changed:" };
  println!("{header}\n{}", changes.report(&root));
  if args.check {
    return Ok(ExitCode::from(1));
  }
  if !dry_run && !args.no_commit && crate::git::is_git_tracked(&root) {
    match crate::git::commit_changes(&root, &changes) {
      Ok(Some(hash)) => println!("Committed as {hash}."),
      Ok(None) => println!("Nothing to commit (only gitignored or env files changed)."),
      Err(e) => {
        eprintln!("warning: changes applied but not committed: {e:#}");
        return Ok(ExitCode::from(3));
      }
    }
  }
  Ok(ExitCode::SUCCESS)
}
