//! `devkit release` — bump, tag, push, create a GitHub release, and wait for the release
//! workflow to build and publish, rolling back on failure.
//!
//! The crate is split by responsibility so each piece can be read (and tested) alone:
//!
//! - [`args`]      — the positional bump/notes heuristic (pure);
//! - [`ci`]        — waiting for the release workflow and verifying its output;
//! - [`config`]    — which index, which credentials, which package;
//! - [`prompt`]    — the typed-`force` confirmations;
//! - [`preflight`] — read-only checks before anything is touched;
//! - [`report`]    — the "what already exists" table;
//! - [`snapshot`]  — byte-exact file backups for rollback;
//! - [`steps`]     — the forward steps;
//! - [`undo`]      — the journal that reverses them.
//!
//! This file holds the CLI definition, the [`Deps`] bundle of injectable collaborators,
//! and [`run`], which strings the modules together.

pub mod args;
pub mod ci;
pub mod config;
pub mod preflight;
pub mod prompt;
pub mod repaint;
pub mod report;
pub mod snapshot;
pub mod steps;
pub mod undo;
pub mod watch;

use std::path::PathBuf;
use std::process::ExitCode;
// `AtomicBool` can be read and written from any thread without a lock — exactly what a
// Ctrl-C handler (which runs on its own thread) needs to signal the main thread.
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::devpi::DevpiClient;
use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::paths::strip_verbatim;
use aeth_devkit_core::process::Runner;

use crate::config::PublishTarget;
use crate::prompt::{Prompt, confirm_force};

/// Bump version, commit, tag, push, create the GitHub release, and wait for the release
/// workflow to build and publish.
///
/// `#[derive(Parser)]` makes clap generate the argument parser from the struct: each field
/// becomes a flag or positional, and the doc comments become `--help` text.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-release", version, about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Skip the confirmation prompts (dirty tree, existing artefacts) as if `force` were typed.
  #[arg(long, short = 'f')]
  pub force: bool,

  /// Run every check and print the plan without changing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// The `[[tool.uv.index]]` the workflow publishes to; it must be the sole one with a publish-url
  /// (default: that index, or PyPI when there is none).
  #[arg(long)]
  pub index: Option<String>,

  /// Create the GitHub release and return without waiting for the release workflow.
  #[arg(long)]
  pub no_wait: bool,

  /// Bump types (major minor patch stable alpha beta rc post dev) followed by optional
  /// multi-word notes.
  // Only `///` doc comments become `--help` text; this `//` note is for readers of the code.
  // A plain `Vec<String>` positional: clap keeps parsing `--force` / `--dry-run` / `-f`
  // wherever they appear (before or after the words), which matters because `poe release`
  // forwards the whole command line verbatim through `$POE_EXTRA_ARGS`. Everything that is
  // not a known flag lands here, and `args::parse_positionals` sorts bumps from notes.
  pub words: Vec<String>,
}

/// The injectable collaborators. Production passes real ones (see [`run_real`]); tests pass
/// recorders and stubs. Bundling them in one struct keeps `run`'s signature stable as the
/// list grows.
///
/// Every field is a borrowed trait object (`&'a dyn Trait`), so `Deps` is cheap to build and
/// pass around, and the concrete types are chosen by the caller.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub devpi: &'a dyn DevpiClient,
  /// Reads simple-index pages: the PyPI existence probe, and the post-CI completeness check
  /// on whichever target the workflow published to.
  pub index: &'a dyn IndexClient,
  pub prompt: &'a dyn Prompt,
  /// Environment lookup, injected so tests never mutate the real process environment.
  pub env: &'a dyn Fn(&str) -> Option<String>,
  /// Set by the Ctrl-C handler; checked between steps.
  pub interrupted: &'a AtomicBool,
  /// How step 8 waits between polls; tests pass a recorder instead of `thread::sleep`.
  pub sleep: &'a dyn Fn(std::time::Duration),
}

impl Deps<'_> {
  /// Abort if Ctrl-C was pressed. `SeqCst` is the strongest (and simplest to reason
  /// about) memory ordering; the cost is irrelevant at this frequency.
  pub fn check_interrupt(&self) -> Result<()> {
    if self.interrupted.load(Ordering::SeqCst) {
      anyhow::bail!("interrupted")
    } else {
      Ok(())
    }
  }
}

/// What a release run amounted to, for callers that compose further steps on top
/// (`devkit release-and-pin` pins the compose file only after `Released`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
  /// The release completed and (unless `--no-wait`) the workflow published it; `version` is
  /// the released version (PEP 440, no `v`).
  Released { version: String },
  /// `--dry-run`: the plan was printed, nothing changed.
  DryRun,
  /// Declined at a prompt, or failed and rolled back (exit 1 either way).
  Aborted,
}

impl Outcome {
  fn exit_code(&self) -> ExitCode {
    match self {
      Outcome::Aborted => ExitCode::from(1),
      _ => ExitCode::SUCCESS,
    }
  }
}

/// Exit codes: 0 released (or dry run); 1 aborted at a prompt or failed and rolled back.
/// Errors from pre-flight bubble up as `Err` (the dispatcher prints them and exits 2).
pub fn run(args: &Args, deps: &Deps) -> Result<ExitCode> {
  Ok(run_outcome(args, deps)?.exit_code())
}

/// [`run`], but reporting *what happened* instead of collapsing it to an exit code.
pub fn run_outcome(args: &Args, deps: &Deps) -> Result<Outcome> {
  let parsed = args::parse_positionals(&args.words)?;
  // `--force` may arrive as a clap flag or buried in the positionals; either counts.
  let force = args.force || parsed.force;
  let root = strip_verbatim(
    args
      .root
      .canonicalize()
      .with_context(|| format!("resolving {}", args.root.display()))?,
  );

  let pyproject_path = root.join("pyproject.toml");
  let doc: DocumentMut = std::fs::read_to_string(&pyproject_path)
    .with_context(|| format!("{} not found", pyproject_path.display()))?
    .parse()
    .context("parsing pyproject.toml")?;
  let cfg = config::resolve(&doc, args.index.as_deref(), deps.env)?;

  // --- Pre-flight: nothing below this line mutates anything until `steps::execute`. ---
  preflight::check_tools(deps.runner, &root)?;
  let branch = preflight::check_branch(deps.runner, &root)?;
  // After `check_branch`, which fetched: the tag-only case compares against `origin/main`.
  preflight::check_workflow_committed(&root, parsed.bumps.is_empty(), &cfg)?;
  // `cfg` above came from the *worktree* pyproject.toml; a release builds from `HEAD`'s
  // copy, so the two must agree on everything release-critical (hard error, exit 2).
  preflight::check_config_committed(&root, &cfg, args.index.as_deref(), deps.env)?;
  let target = preflight::target_version(deps.runner, &root, &parsed.bumps)?;
  ci::check_no_active_run(deps.runner, &root, &format!("v{}", target.new))?;
  if parsed.bumps.is_empty() {
    println!("Releasing {} {} (no bump)", cfg.package, target.new);
  } else {
    println!("Releasing {} {} -> {}", cfg.package, target.current, target.new);
  }
  preflight::check_cargo_version(&root, &target.current)?;
  // Hard error (exit 2), deliberately *before* the dirty-tree prompt: `--force` may accept
  // a dirty tree, but an unresolved merge conflict in a managed file has no safe automatic
  // handling (see `preflight::check_unmerged`).
  preflight::check_unmerged(&root)?;
  // A declined prompt is a normal exit (1), not an error (2): the user chose to stop.
  if let Err(e) = preflight::confirm_dirty_tree(&root, force, deps.prompt) {
    eprintln!("{e:#}");
    return Ok(Outcome::Aborted);
  }

  let existing = preflight::probe(deps, &root, &cfg, &target.new)?;
  if existing.any() {
    print!("{}", report::render(&target.new, &existing, &cfg.package, cfg.target.label()));
    // PyPI files are immutable: nothing here can make room for the version, so the only
    // way forward is a different version number.
    if existing.index && cfg.target == PublishTarget::Pypi {
      eprintln!(
        "aborted: {}=={} is already on PyPI and PyPI releases cannot be removed; bump to a new version",
        cfg.package, target.new
      );
      return Ok(Outcome::Aborted);
    }
    if !args.dry_run && !confirm_force(deps.prompt, force, "Remove these and continue? Type 'force' to continue:")? {
      eprintln!("aborted: artefacts for v{} already exist", target.new);
      return Ok(Outcome::Aborted);
    }
    // The probe above may have taken a while; do not start deleting after a Ctrl-C.
    deps.check_interrupt()?;
    preflight::remove_existing(deps, &root, &cfg, &target.new, &existing, args.dry_run)?;
  } else {
    println!("No existing artefacts for v{}.", target.new);
  }

  let plan = steps::Plan {
    root: &root,
    cfg: &cfg,
    target: &target,
    bumps: &parsed.bumps,
    // `as_deref()` turns `Option<String>` into `Option<&str>` without moving.
    notes: parsed.notes.as_deref(),
    branch: &branch,
    no_wait: args.no_wait,
  };
  if args.dry_run {
    print!("{}", steps::describe(&plan));
    return Ok(Outcome::DryRun);
  }

  // --- Execute, and unwind the journal on the first error. ---
  let mut journal = Vec::new();
  match steps::execute(&plan, deps, &mut journal) {
    Ok(url) => {
      println!("Released {} {}\n{url}", cfg.package, target.new);
      Ok(Outcome::Released {
        version: target.new.clone(),
      })
    }
    // A run whose state is unknown may still publish: unwinding under it would delete the
    // release and tag it is about to upload to, and could never un-publish. Leave every
    // step in place and hand the user the same commands the unwind would have run.
    Err(e) if e.downcast_ref::<ci::Unsettled>().is_some() => {
      // PyPI cannot take a version back, so a rerun would only hit the immutable-version
      // abort; a private index is cleaned up by the next run's probe.
      let then = if cfg.target == PublishTarget::Pypi {
        "if the run published, bump to a new version (PyPI files cannot be removed); otherwise"
      } else {
        "either run `devkit release` again for this version — it detects what exists and offers to remove it — or"
      };
      eprintln!(
        "\nERROR: Release failed: {e:#}\nNOT rolling back: the run may still publish. When it has stopped (see Actions), \
         {then} undo by hand, in this order:"
      );
      for undo in journal.into_iter().rev() {
        eprintln!("  {}", undo.manual_command());
        // The restore command names the snapshot directory, which the `TempDir` would
        // delete on drop — before the user has waited for the run to stop.
        if let undo::Undo::RestoreFiles(snap) = undo {
          eprintln!("    (pre-run snapshot kept at {})", snap.keep().display());
        }
      }
      Ok(Outcome::Aborted)
    }
    Err(e) => {
      eprintln!("\nERROR: Release failed: {e:#}\nRolling back...");
      let failures = undo::unwind(journal, deps, &root);
      if failures.is_empty() {
        eprintln!("\nRollback complete.");
      } else {
        eprintln!("\n{}", undo::render_failures(&failures));
      }
      Ok(Outcome::Aborted)
    }
  }
}

/// Process-wide interrupt flag. A `static` lives for the whole program, which is what the
/// Ctrl-C handler (a `'static` closure) needs to reference.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// [`run`] with the real collaborators.
pub fn run_real(args: &Args) -> Result<ExitCode> {
  Ok(run_outcome_real(args)?.exit_code())
}

/// [`run_outcome`] with the real collaborators.
pub fn run_outcome_real(args: &Args) -> Result<Outcome> {
  // The handler only flips the flag. Child processes (`uv`, `git`, `gh`) receive the same
  // console interrupt and exit non-zero, which surfaces as an ordinary step error and
  // triggers rollback; the flag covers interrupts that land between steps. The handler stays
  // installed, so a second Ctrl-C during rollback does not kill us mid-unwind.
  ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).context("installing Ctrl-C handler")?;
  let env = |key: &str| std::env::var(key).ok();
  let index = aeth_devkit_core::index::HttpIndexClient::with_timeout(std::time::Duration::from_secs(30));
  run_outcome(
    args,
    &Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      devpi: &aeth_devkit_core::devpi::HttpDevpiClient,
      index: &index,
      prompt: &prompt::StdinPrompt,
      env: &env,
      interrupted: &INTERRUPTED,
      sleep: &|d| std::thread::sleep(d),
    },
  )
}

#[cfg(test)]
mod cli_tests {
  use super::*;

  #[test]
  fn flags_parse_anywhere_on_the_line() {
    // `try_parse_from` takes the full argv including the program name, and returns a
    // `Result` instead of exiting the process like `parse()` would.
    let a = Args::try_parse_from(["devkit-release", "patch", "--dry-run", "-f", "first patch release"]).unwrap();
    assert!(a.force && a.dry_run);
    assert_eq!(a.words, vec!["patch", "first patch release"]);
    let a = Args::try_parse_from(["devkit-release", "--index", "Other", "major", "alpha"]).unwrap();
    assert_eq!(a.index.as_deref(), Some("Other"));
    assert_eq!(a.words, vec!["major", "alpha"]);
    let a = Args::try_parse_from(["devkit-release", "patch", "--no-wait"]).unwrap();
    assert!(a.no_wait);
  }
}
