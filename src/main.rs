use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;

/// Standardize an SFT project's configuration from the templates shipped with poe_tasks.
#[derive(Parser, Debug)]
#[command(name = "sft-setup", version, about)]
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

  /// Do not commit the changes (by default they are committed when the project is
  /// inside a git work tree; only the files sft-setup changed are staged).
  #[arg(long)]
  no_commit: bool,
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let dry_run = cli.dry_run || cli.check;
  let result = sft_setup::templates::locate(cli.templates_dir.as_deref())
    .and_then(|templates| sft_setup::run(&cli.root, &templates, dry_run).map(|c| (templates, c)));
  match result {
    Ok((_, changes)) => {
      let root = sft_setup::context::strip_verbatim(cli.root.canonicalize().unwrap_or(cli.root.clone()));
      if changes.is_empty() {
        println!("Nothing to do — project already matches the templates.");
        return ExitCode::SUCCESS;
      }
      let header = if dry_run { "Would change:" } else { "Changed:" };
      println!("{header}\n{}", changes.report(&root));
      if cli.check {
        return ExitCode::from(1);
      }
      if !dry_run && !cli.no_commit && sft_setup::git::is_git_tracked(&root) {
        match sft_setup::git::commit_changes(&root, &changes) {
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
