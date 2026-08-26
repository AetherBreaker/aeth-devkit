use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  let args = aeth_devkit_lock::Args::parse();
  match aeth_devkit_lock::run_real(&args) {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
