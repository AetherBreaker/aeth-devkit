use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Standardize a project's configuration from the templates shipped with aeth-devkit.
#[derive(Parser, Debug)]
#[command(name = "devkit-setup", version, about)]
struct Cli {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  root: PathBuf,

  /// Directory containing the templates (defaults to the installed poe_tasks package).
  #[arg(long)]
  templates_dir: Option<PathBuf>,

  /// Print the changes that would be made without writing anything.
  #[arg(long)]
  dry_run: bool,

  /// Like --dry-run, but exit non-zero if anything would change.
  #[arg(long)]
  check: bool,

  /// Do not commit the changes. By default, when the project is git-tracked, the changed
  /// files (never env files) are committed with a standard message.
  #[arg(long)]
  no_commit: bool,
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let dry_run = cli.dry_run || cli.check;
  let result = aeth_devkit_setup::templates::locate(cli.templates_dir.as_deref())
    .and_then(|templates| aeth_devkit_setup::run(&cli.root, &templates, dry_run).map(|c| (templates, c)));
  match result {
    Ok((_, changes)) => {
      let root = aeth_devkit_setup::context::strip_verbatim(cli.root.canonicalize().unwrap_or(cli.root.clone()));
      if changes.is_empty() {
        println!("Nothing to do — project already matches the templates.");
        return ExitCode::SUCCESS;
      }
      let header = if dry_run { "Would change:" } else { "Changed:" };
      println!("{header}\n{}", changes.report(&root));
      if cli.check {
        return ExitCode::from(1);
      }
      if !dry_run && !cli.no_commit && aeth_devkit_setup::git::is_git_tracked(&root) {
        match aeth_devkit_setup::git::commit_changes(&root, &changes) {
          Ok(Some(hash)) => println!("Committed as {hash}."),
          Ok(None) => println!("Nothing to commit (only gitignored or env files changed)."),
          Err(e) => {
            eprintln!("warning: changes applied but not committed: {e:#}");
            return ExitCode::from(3);
          }
        }
      }
      ExitCode::SUCCESS
    }
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
