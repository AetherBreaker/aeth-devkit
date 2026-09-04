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

  /// Answer `replace all` to every Docker prompt up front (also applies Docker files when
  /// stdin is not a terminal).
  #[arg(long)]
  pub replace_docker: bool,
}

/// Exit codes: 0 ok, 1 `--check` found drift, 3 commit failed (the template changes were
/// rolled back). Errors bubble up for the caller to print (exit 2).
pub fn run(args: &Args) -> Result<ExitCode> {
  let dry_run = args.dry_run || args.check;
  let templates = crate::templates::locate(args.templates_dir.as_deref())?;
  let root = crate::context::strip_verbatim(args.root.canonicalize().unwrap_or(args.root.clone()));

  // When committing, the committable managed files are merged against their `HEAD`
  // content, so the commit carries only this run's changes and the user's uncommitted
  // edits are replayed back on top afterwards (see `aeth_devkit_core::commit`).
  let committing = !dry_run && !args.no_commit && crate::git::is_git_tracked(&root);
  let mut bases = if committing { Some(crate::git::stage_bases(&root)?) } else { None };

  // Apply the templates (plus tombi), putting the user's files back on any failure.
  let apply = |changes: &mut Option<crate::changes::Changes>| -> Result<()> {
    // `IsTerminal` is how std asks "is a human here?": prompts only make sense on a tty.
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let deps = crate::docker::Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      prompt: &aeth_devkit_core::prompt::StdinPrompt,
      mode: match (dry_run, args.replace_docker, tty) {
        (true, _, _) => crate::docker::Mode::DryRun,
        (false, true, _) => crate::docker::Mode::ReplaceAll,
        (false, false, true) => crate::docker::Mode::Ask,
        (false, false, false) => crate::docker::Mode::KeepAll,
      },
      interactive: tty && !dry_run,
    };
    let mut c = crate::run_with(&root, &templates, dry_run, &deps)?;
    if !dry_run {
      match crate::format::format_pyproject(&root, &crate::format::SystemRunner, &mut c)? {
        crate::format::Outcome::Formatted(_) => {}
        crate::format::Outcome::Unavailable => println!("note: tombi not found; skipping pyproject.toml formatting."),
        crate::format::Outcome::Failed { code } => {
          eprintln!("warning: tombi format exited with {code:?}; pyproject.toml left unformatted.");
        }
      }
    }
    *changes = Some(c);
    Ok(())
  };
  let mut changes = None;
  if let Err(e) = apply(&mut changes) {
    if let Some(bases) = &bases {
      aeth_devkit_core::commit::restore_worktree(&root, bases)?;
    }
    return Err(e);
  }
  // `expect` documents the invariant: `apply` only returns `Ok` after setting it.
  let changes = changes.expect("apply sets changes on success");

  for note in &changes.notes {
    println!("note: {note}");
  }
  if changes.is_empty() {
    // No file differs from its merge base; undo the staging so the user's uncommitted
    // edits to managed files are back in place.
    if let Some(bases) = &bases {
      aeth_devkit_core::commit::unstage_clean_base(&root, bases)?;
    }
    println!("Nothing to do — project already matches the templates.");
    return Ok(ExitCode::SUCCESS);
  }
  let header = if dry_run { "Would change:" } else { "Changed:" };
  println!("{header}\n{}", changes.report(&root));
  if args.check {
    return Ok(ExitCode::from(1));
  }
  if let Some(bases) = &mut bases {
    match crate::git::commit_changes(&root, &changes, bases) {
      Ok(Some(hash)) => println!("Committed as {hash}."),
      Ok(None) => println!("Nothing to commit (only gitignored or env files changed)."),
      Err(e) => {
        eprintln!("warning: not committed; the template changes were rolled back: {e:#}");
        return Ok(ExitCode::from(3));
      }
    }
  }
  Ok(ExitCode::SUCCESS)
}
