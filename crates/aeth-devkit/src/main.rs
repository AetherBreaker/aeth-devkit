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
  /// Shell-completion data for poe tasks (fast replacement for poe's `_list_tasks`).
  Complete(aeth_devkit_complete::Args),
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let result = match &cli.command {
    Command::SetupProject(args) => aeth_devkit_setup::cli::run(args),
    Command::Lock(args) => aeth_devkit_lock::run_real(args),
    Command::Release(args) => aeth_devkit_release::run_real(args),
    Command::Complete(args) => Ok(aeth_devkit_complete::run_real(args)),
  };
  match result {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
