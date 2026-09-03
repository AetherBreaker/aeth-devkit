//! `devkit` — project maintenance commands. Each subcommand lives in its own crate; this
//! binary only parses and dispatches.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "devkit", version, about = "Project maintenance commands")]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
  /// Standardize the project's configuration from the shipped templates.
  SetupProject(aeth_devkit_setup::cli::Args),
  /// Bump the aeth-devkit pin, run `uv sync`, and commit uv.lock.
  Lock(aeth_devkit_lock::Args),
  /// Bump version, build, tag, publish to the index, and create a GitHub release.
  Release(aeth_devkit_release::Args),
  /// Pin the docker compose file to a released version of this project.
  DockerPin(aeth_devkit_pin::Args),
  /// Release, then pin the docker compose file to the freshly released version.
  ReleaseAndPin(aeth_devkit_release::Args),
  /// Shell-completion data for poe tasks (fast replacement for poe's `_list_tasks`).
  Complete(aeth_devkit_complete::Args),
  /// Run a Claude Code hook (payload on stdin, decision on stdout). Always exits 0.
  Hook(aeth_devkit_hooks::Args),
}

impl Command {
  /// Whether to append the outdated-devkit nag after this command. The completion data and
  /// script subcommands run on every Tab press and their output must stay pure and fast, so
  /// only `complete install` — an ordinary, interactive command — gets it. Hooks are the
  /// same story: Claude runs them on every tool call, nobody is watching their stderr, and
  /// the nag's once-a-day index fetch would put a network timeout in that path.
  fn wants_update_check(&self) -> bool {
    match self {
      Command::Complete(args) => matches!(args.command, aeth_devkit_complete::Command::Install { .. }),
      Command::Hook(_) => false,
      _ => true,
    }
  }
}

/// `devkit release` then `devkit docker-pin --version <released>`, in-process. The pin step
/// only runs after a completed release: a dry run stays dry, and an aborted or rolled-back
/// release must not move the pin.
fn release_and_pin(args: &aeth_devkit_release::Args) -> anyhow::Result<ExitCode> {
  use aeth_devkit_release::Outcome;
  // The pin's completeness preflight needs the artefacts the workflow publishes.
  if args.no_wait {
    anyhow::bail!("release-and-pin waits for the release workflow so it can pin the result; drop --no-wait");
  }
  match aeth_devkit_release::run_outcome_real(args)? {
    Outcome::Aborted => Ok(ExitCode::from(1)),
    Outcome::DryRun => {
      println!("Dry run: skipping docker pin.");
      Ok(ExitCode::SUCCESS)
    }
    Outcome::Released { version } => {
      println!();
      aeth_devkit_pin::run_real(&aeth_devkit_pin::Args {
        root: args.root.clone(),
        version: Some(version),
        dry_run: false,
        no_commit: false,
        no_push: false,
        compose_file: None,
      })
    }
  }
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let result = match &cli.command {
    Command::SetupProject(args) => aeth_devkit_setup::cli::run(args),
    Command::Lock(args) => aeth_devkit_lock::run_real(args),
    Command::Release(args) => aeth_devkit_release::run_real(args),
    Command::DockerPin(args) => aeth_devkit_pin::run_real(args),
    Command::ReleaseAndPin(args) => release_and_pin(args),
    Command::Complete(args) => Ok(aeth_devkit_complete::run_real(args)),
    Command::Hook(args) => Ok(aeth_devkit_hooks::run_real(args)),
  };
  // Last thing printed, so it is what the user sees; runs even after a failure, since an
  // outdated devkit may be the reason for it.
  if cli.command.wants_update_check() {
    aeth_devkit_core::update::nag(env!("CARGO_PKG_VERSION"));
  }
  match result {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
