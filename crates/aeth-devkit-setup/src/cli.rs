//! Command-line surface of `devkit setup-project`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use clap::Parser;

/// Standardize a project's configuration from the devkit-templates package in its
/// environment.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-setup", version, about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Render this directory instead of the environment's devkit-templates package
  /// (DEVKIT_TEMPLATES and [tool.devkit].templates-dir also set it, in that order of
  /// precedence).
  #[arg(long)]
  pub templates_dir: Option<PathBuf>,

  /// Print the changes that would be made without writing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// Do not commit the changes. By default, when the project is git-tracked, the changed
  /// files (never env files) are committed with a standard message.
  #[arg(long)]
  pub no_commit: bool,

  /// Accept every proposed change without asking (Docker files and compose services
  /// included); the run then needs no standard input.
  #[arg(short = 'y', long)]
  pub yes: bool,

  /// Use the VS Code diff for Docker consent even when TERM_PROGRAM is not "vscode".
  #[arg(long, conflicts_with = "no_vscode")]
  pub vscode: bool,

  /// Never open VS Code; always use the terminal prompt.
  #[arg(long)]
  pub no_vscode: bool,
}

/// [`run`], refused first when there is no standard input to answer a prompt from (see
/// `prompt::stdin_present`) unless nothing will be asked: `--yes`, `--dry-run`.
/// A pipe counts as input: its lines answer the prompts, and running dry mid-way cancels
/// the run. A launch with no input at all would otherwise block on a question nobody can
/// answer, so it fails fast instead. Tests call [`run`] directly, which has no such check.
pub fn run_reject_headless(args: &Args) -> Result<ExitCode> {
  if !(args.yes || args.dry_run) && !aeth_devkit_core::prompt::stdin_present() {
    bail!("setup-project has no standard input to answer its prompts from; pass -y/--yes to accept every change, or use --dry-run");
  }
  run(args)
}

/// The `services` half of the refusal (`gates_for` with HEAD's pyproject is the other): a
/// committing run merges into HEAD's pyproject but takes the Docker switch from the working
/// copy; a `[tool.docker].services` that differs between the two would produce a commit the
/// switch does not describe, or a duplicate key once the edit is replayed. Not automated
/// away: the commit is the user's to make.
fn refuse_uncommitted_services(root: &Path) -> Result<()> {
  let head = match aeth_devkit_core::git::head_blob(root, "pyproject.toml")? {
    Some(bytes) => {
      let doc: toml_edit::DocumentMut = String::from_utf8_lossy(&bytes).parse().context("parsing HEAD's pyproject.toml")?;
      crate::context::services_key(&doc)?
    }
    None => None,
  };
  let text = std::fs::read_to_string(root.join("pyproject.toml")).context("reading pyproject.toml")?;
  let worktree = crate::context::services_key(&text.parse::<toml_edit::DocumentMut>().context("parsing pyproject.toml")?)?;
  if head != worktree {
    bail!("pyproject.toml's [tool.docker].services is not committed; commit that change, then rerun setup-project");
  }
  Ok(())
}

/// Exit codes: 0 ok; 1 finished with `error:` findings (drift in a managed file the run
/// could not edit; everything else was written and committed); 3 commit failed (the
/// template changes were rolled back). Errors bubble up for the caller to print (exit 2).
pub fn run(args: &Args) -> Result<ExitCode> {
  let dry_run = args.dry_run;
  let root = crate::context::strip_verbatim(args.root.canonicalize().unwrap_or(args.root.clone()));
  // `IsTerminal` is how std asks "is a human here?": VS Code is only worth opening when
  // one is, not when a pipe is scripting the answers.
  let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
  let runner = aeth_devkit_core::process::SystemRunner;

  // Discovered before staging (see `run_with`), and with the override before the VS Code
  // step: a wrong --templates-dir is refused before an extension is installed or a reload
  // asked for.
  let ctx = crate::context::ProjectContext::discover(&root)?;
  let templates_override = crate::templates::override_dir(args.templates_dir.as_deref(), &ctx)?;

  // First, so the guards inside `prepare` (the extension download, argv.json) count.
  if !dry_run && let Err(e) = crate::interrupt::install() {
    println!("note: {e:#}; a Ctrl-C will not wait for a write in progress to finish.");
  }
  // VS Code is consulted only where a human could answer in the terminal anyway: never
  // without a tty (hooks and CI), never when --yes has already answered. It runs before
  // staging so a "reload and rerun" stop touches nothing.
  let vs = if args.no_vscode || !tty || args.yes {
    None
  } else {
    let opts = crate::vscode::Options::from_env(args.vscode, !dry_run, &root);
    match crate::vscode::prepare(&opts, &runner, &crate::vscode::install::HttpFetch) {
      crate::vscode::Prepared::Inert => None,
      crate::vscode::Prepared::Unavailable(why) => {
        println!("note: {why}; using the terminal prompt.");
        None
      }
      crate::vscode::Prepared::ReloadNeeded => {
        // A refusal like the headless one: no project file was touched (the extension
        // itself was installed), so exit 2 says so to a wrapper.
        bail!(
          "the devkit VS Code extension was updated; reload the VS Code window, then run setup-project again (or pass --no-vscode)"
        )
      }
      crate::vscode::Prepared::Ready(vs) => Some(vs),
    }
  };
  // Printed now, not with the run's notes: one of them ("restart VS Code once") is
  // emitted only on the run that grants argv.json, and a failure later would lose it.
  for note in vs.iter().flat_map(|v| &v.notes) {
    println!("note: {note}");
  }
  // Handed in for a dry run too: never consulted there (`decide` answers first), but its
  // presence is what makes the run keep previews for the review at the end.
  let reviewer = vs.as_ref().map(|v| crate::vscode::session::VsCodeReviewer::new(v, &runner));

  // When committing, the committable managed files are merged against their `HEAD`
  // content, so the commit carries only this run's changes and the user's uncommitted
  // edits are replayed back on top afterwards (see `aeth_devkit_core::commit`).
  let committing = !dry_run && !args.no_commit && crate::git::is_git_tracked(&root);
  if committing {
    refuse_uncommitted_services(&root)?;
    // Every gate the run will evaluate must agree between HEAD and the working copy (spec
    // 2.5). Before the first run installs the templates package there is nothing to sweep
    // but the container's templates; the `services` check above still applies.
    let head = aeth_devkit_core::git::head_blob(&root, "pyproject.toml")?
      .map(|b| String::from_utf8_lossy(&b).into_owned())
      .unwrap_or_default();
    let venv = crate::packages::SystemVenv;
    let templates_dir = templates_override
      .clone()
      .or_else(|| crate::packages::Venv::installed(&venv, &root, &crate::packages::TEMPLATES).map(|i| i.dir.join("templates")));
    crate::gates_for(&ctx, templates_dir.as_deref(), &venv, Some(&head))?;
  }
  let mut bases = if committing {
    let _w = crate::interrupt::Writing::begin();
    Some(crate::git::stage_bases(&root)?)
  } else {
    None
  };

  // Apply the templates (plus tombi), putting the user's files back on any failure.
  let apply = |changes: &mut Option<crate::changes::Changes>| -> Result<()> {
    let index = aeth_devkit_core::index::HttpIndexClient::with_timeout(std::time::Duration::from_secs(30));
    let deps = crate::Deps {
      docker: crate::docker::Deps {
        runner: &runner,
        prompt: &aeth_devkit_core::prompt::StdinPrompt,
        reviewer: reviewer.as_ref().map(|r| r as &dyn crate::vscode::protocol::Reviewer),
        mode: match (dry_run, args.yes) {
          (true, _) => crate::docker::Mode::DryRun,
          (false, true) => crate::docker::Mode::Yes,
          (false, false) => crate::docker::Mode::Ask,
        },
      },
      index: &index,
      venv: &crate::packages::SystemVenv,
    };
    let mut c = crate::run_with(&ctx, templates_override.as_deref(), dry_run, &deps)?;
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
      let _w = crate::interrupt::Writing::begin();
      aeth_devkit_core::commit::restore_worktree(&root, bases)?;
    }
    return Err(e);
  }
  // `expect` documents the invariant: `apply` only returns `Ok` after setting it.
  let changes = changes.expect("apply sets changes on success");

  for note in &changes.notes {
    println!("note: {note}");
  }
  for warning in &changes.warnings {
    eprintln!("warning: {warning}");
  }
  for error in &changes.errors {
    eprintln!("error: {error}");
  }
  // An error is a finding on the repo, not something to write: the run finishes, and the
  // exit code carries the finding (a commit failure below still wins with 3).
  let exit = if changes.errors.is_empty() {
    ExitCode::SUCCESS
  } else {
    ExitCode::from(1)
  };
  if changes.is_empty() {
    // No file differs from its merge base; undo the staging so the user's uncommitted
    // edits to managed files are back in place.
    if let Some(bases) = &bases {
      let _w = crate::interrupt::Writing::begin();
      aeth_devkit_core::commit::unstage_clean_base(&root, bases)?;
      crate::packages::resync_after_replay(&root, &runner, bases, &changes);
    }
    if changes.errors.is_empty() {
      println!("Nothing to do — project already matches the templates.");
    } else {
      println!("Nothing to write; the error(s) above need a hand edit.");
    }
    return Ok(exit);
  }
  let header = if dry_run { "Would change:" } else { "Changed:" };
  println!("{header}\n{}", changes.report(&root));
  if dry_run
    && let Some(vs) = &vs
    && let Err(e) = crate::vscode::session::open_review(vs, &runner, &root, &changes.previews, crate::vscode::session::ACK_TIMEOUT)
  {
    println!("note: could not open the review in VS Code: {e:#}");
  }
  if let Some(bases) = &mut bases {
    let _w = crate::interrupt::Writing::begin();
    let committed = crate::git::commit_changes(&root, &changes, bases);
    // Committed and replayed, or rolled back: either way the user's uv.lock is back on
    // disk, and the venv follows it.
    crate::packages::resync_after_replay(&root, &runner, bases, &changes);
    match committed {
      Ok(Some(hash)) => println!("Committed as {hash}."),
      Ok(None) => println!("Nothing to commit (only gitignored or env files changed)."),
      Err(e) => {
        eprintln!("warning: not committed; the template changes were rolled back: {e:#}");
        return Ok(ExitCode::from(3));
      }
    }
  }
  Ok(exit)
}
