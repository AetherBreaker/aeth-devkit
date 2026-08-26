use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  let args = aeth_devkit_setup::cli::Args::parse();
  match aeth_devkit_setup::cli::run(&args) {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
