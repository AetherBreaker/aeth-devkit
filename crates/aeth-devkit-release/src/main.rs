//! Dev binary: `cargo run -p aeth-devkit-release -- …` builds and links only this command
//! while iterating. The shipped `devkit` binary wraps the same `Args` and `run_real`.

use std::process::ExitCode;

// `Parser` is imported for its `parse()` method only, so `as _` keeps the name out of scope.
use clap::Parser as _;

fn main() -> ExitCode {
  let args = aeth_devkit_release::Args::parse();
  match aeth_devkit_release::run_real(&args) {
    Ok(code) => code,
    Err(e) => {
      // `{e:#}` prints the whole context chain: "parsing pyproject.toml: expected …".
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
