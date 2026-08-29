//! `devkit complete` — fast shell completion for poe tasks.
//! See docs/specs/2026-08-28-poe-completion-design.md.

pub mod cache;
pub mod format;
pub mod resolve;

use std::process::ExitCode;

use clap::Parser;

/// Shell-completion data for poe tasks, served from Rust instead of a Python process.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-complete", version, about)]
pub struct Args {}

/// Production entry: never exits non-zero — a completer that errors breaks the shell.
pub fn run_real(args: &Args) -> ExitCode {
  let _ = args;
  ExitCode::SUCCESS
}
