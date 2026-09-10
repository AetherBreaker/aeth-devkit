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
  /// Standardize the project's configuration from the devkit-templates package in its environment.
  SetupProject(aeth_devkit_setup::cli::Args),
  /// Bump the aeth-devkit pin, run `uv sync`, and commit uv.lock.
  Lock(aeth_devkit_lock::Args),
  /// Bump version, commit, tag, push and create the GitHub release, then wait for the
  /// release workflow to build, attach and publish the artefacts.
  Release(aeth_devkit_release::Args),
  /// Pin the docker compose file to a released version of this project.
  DockerPin(aeth_devkit_pin::Args),
  /// Release (always waiting for the workflow; `--no-wait` is refused), then pin the docker
  /// compose file to the freshly released version.
  ReleaseAndPin(aeth_devkit_release::Args),
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
    Command::SetupProject(args) => aeth_devkit_setup::cli::run_reject_headless(args),
    Command::Lock(args) => aeth_devkit_lock::run_real(args),
    Command::Release(args) => aeth_devkit_release::run_real(args),
    Command::DockerPin(args) => aeth_devkit_pin::run_real(args),
    Command::ReleaseAndPin(args) => release_and_pin(args),
  };
  // Last thing printed, so it is what the user sees; runs even after a failure, since an
  // outdated devkit may be the reason for it.
  aeth_devkit_core::update::nag(env!("CARGO_PKG_VERSION"));
  match result {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
