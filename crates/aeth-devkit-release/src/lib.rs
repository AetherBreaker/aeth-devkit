//! `devkit release` — bump, build, tag, publish, and create a GitHub release, rolling back
//! on failure.
//!
//! The crate is split by responsibility so each piece can be read (and tested) alone:
//!
//! - [`args`]      — the positional bump/notes heuristic (pure);
//! - [`config`]    — which index, which credentials, which package;
//! - [`prompt`]    — the typed-`force` confirmations;
//! - [`preflight`] — read-only checks before anything is touched;
//! - [`report`]    — the "what already exists" table;
//! - [`snapshot`]  — byte-exact file backups for rollback;
//! - [`steps`]     — the nine forward steps;
//! - [`undo`]      — the journal that reverses them.
//!
//! This file holds the CLI definition, the [`Deps`] bundle of injectable collaborators,
//! and [`run`], which strings the modules together.

pub mod args;
pub mod config;
pub mod preflight;
pub mod prompt;
pub mod report;
pub mod snapshot;
pub mod steps;
pub mod undo;

use std::path::PathBuf;
use std::process::ExitCode;
// `AtomicBool` can be read and written from any thread without a lock — exactly what a
// Ctrl-C handler (which runs on its own thread) needs to signal the main thread.
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::devpi::DevpiClient;
use aeth_devkit_core::paths::strip_verbatim;
use aeth_devkit_core::process::Runner;

use crate::prompt::{Prompt, confirm_force};

/// Bump version, commit, tag, build, publish to the index, and create a GitHub release.
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

  /// `[[tool.uv.index]]` to publish to (default: the one with a publish-url).
  #[arg(long)]
  pub index: Option<String>,

  /// Bump types (major minor patch stable alpha beta rc post dev) followed by optional
  /// multi-word notes.
  ///
  /// `trailing_var_arg` + `allow_hyphen_values`: once the first positional is seen, every
  /// remaining word (even `-f`) lands here, and `args::parse_positionals` sorts them out.
  #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
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
  pub prompt: &'a dyn Prompt,
  /// Environment lookup, injected so tests never mutate the real process environment.
  pub env: &'a dyn Fn(&str) -> Option<String>,
  /// Set by the Ctrl-C handler; checked between steps.
  pub interrupted: &'a AtomicBool,
}

/// Exit codes: 0 released; 1 aborted at a prompt or failed and rolled back. Errors from
/// pre-flight bubble up as `Err` (the dispatcher prints them and exits 2).
pub fn run(args: &Args, deps: &Deps) -> Result<ExitCode> {
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
  let target = preflight::target_version(deps.runner, &root, &parsed.bumps)?;
  if parsed.bumps.is_empty() {
    println!("Releasing {} {} (no bump)", cfg.package, target.new);
  } else {
    println!("Releasing {} {} -> {}", cfg.package, target.current, target.new);
  }
  preflight::check_cargo_version(&root, &target.current)?;
  // A declined prompt is a normal exit (1), not an error (2): the user chose to stop.
  if let Err(e) = preflight::confirm_dirty_tree(&root, force, deps.prompt) {
    eprintln!("{e:#}");
    return Ok(ExitCode::from(1));
  }

  let existing = preflight::probe(deps, &root, &cfg, &target.new)?;
  if existing.any() {
    print!("{}", report::render(&target.new, &existing, &cfg.package, &cfg.index_name));
    if !args.dry_run && !confirm_force(deps.prompt, force, "Remove these and continue? Type 'force' to continue:")? {
      eprintln!("aborted: artefacts for v{} already exist", target.new);
      return Ok(ExitCode::from(1));
    }
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
  };
  if args.dry_run {
    print!("{}", steps::describe(&plan));
    return Ok(ExitCode::SUCCESS);
  }

  // --- Execute, and unwind the journal on the first error. ---
  let mut journal = Vec::new();
  match steps::execute(&plan, deps, &mut journal) {
    Ok(url) => {
      println!("Released {} {}\n{url}", cfg.package, target.new);
      Ok(ExitCode::SUCCESS)
    }
    Err(e) => {
      eprintln!("\nERROR: Release failed: {e:#}\nRolling back...");
      let failures = undo::unwind(journal, deps, &root, &cfg);
      if failures.is_empty() {
        eprintln!("\nRollback complete.");
      } else {
        eprintln!("\n{}", undo::render_failures(&failures));
      }
      Ok(ExitCode::from(1))
    }
  }
}

/// Process-wide interrupt flag. A `static` lives for the whole program, which is what the
/// Ctrl-C handler (a `'static` closure) needs to reference.
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// [`run`] with the real collaborators.
pub fn run_real(args: &Args) -> Result<ExitCode> {
  // The handler only flips the flag. Child processes (`uv`, `git`, `gh`) receive the same
  // console interrupt and exit non-zero, which surfaces as an ordinary step error and
  // triggers rollback; the flag covers interrupts that land between steps. The handler stays
  // installed, so a second Ctrl-C during rollback does not kill us mid-unwind.
  ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).context("installing Ctrl-C handler")?;
  let env = |key: &str| std::env::var(key).ok();
  run(
    args,
    &Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      devpi: &aeth_devkit_core::devpi::HttpDevpiClient,
      prompt: &prompt::StdinPrompt,
      env: &env,
      interrupted: &INTERRUPTED,
    },
  )
}
