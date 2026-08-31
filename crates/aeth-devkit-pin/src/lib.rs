//! `devkit docker-pin` — pin the compose file's version for the services that build this
//! project, then commit and push the change.

pub mod resolve;

use std::path::PathBuf;

use clap::Parser;

use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::process::Runner;

/// Pin the docker compose file to a released version of this project.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-docker-pin", about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Pin to this exact version (with or without a leading `v`; pre-releases allowed).
  /// Default: the latest stable version released everywhere.
  #[arg(long, short = 'V')]
  pub version: Option<String>,

  /// Resolve and report without changing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// Edit the compose file but do not commit (implies --no-push).
  #[arg(long)]
  pub no_commit: bool,

  /// Commit locally but do not push.
  #[arg(long)]
  pub no_push: bool,

  /// Compose file to edit (default: auto-discovered from the repo root).
  #[arg(long, short = 'c')]
  pub compose_file: Option<PathBuf>,
}

/// The injectable collaborators, mirroring the release crate's pattern: production passes
/// real ones (see `run_real`), tests pass recorders and stubs.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub index: &'a dyn IndexClient,
}
