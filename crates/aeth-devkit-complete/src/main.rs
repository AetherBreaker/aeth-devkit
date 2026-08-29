use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  let args = aeth_devkit_complete::Args::parse();
  aeth_devkit_complete::run_real(&args)
}
