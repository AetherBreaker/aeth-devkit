use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  let args = aeth_devkit_hooks::Args::parse();
  aeth_devkit_hooks::run_real(&args)
}
