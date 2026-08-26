//! `sft-setup` — standardize an SFT project's configuration from the templates shipped
//! with `poe_tasks`. See docs/specs/2026-08-26-setup-project-design.md.

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "sft-setup", version, about)]
struct Cli {
  /// Print the changes that would be made without writing anything.
  #[arg(long)]
  dry_run: bool,

  /// Like --dry-run, but exit non-zero if anything would change.
  #[arg(long)]
  check: bool,
}

fn main() {
  let cli = Cli::parse();
  println!("sft-setup stub (dry_run={}, check={})", cli.dry_run, cli.check);
}
