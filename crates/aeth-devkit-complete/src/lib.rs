//! `devkit complete` — fast shell completion for poe tasks.
//! See docs/specs/2026-08-28-poe-completion-design.md.
//!
//! poe's own completion costs ~200 ms per Tab press: it starts Python, imports the
//! poethepoet framework, and — with `include_script` — spawns a second process through
//! `uv run` to obtain the task table. This crate answers the same two questions
//! (`which tasks?`, `which args for this task?`) from Rust, with the task table resolved
//! natively for TOML and cached for `include_script`.

pub mod cache;
pub mod format;
pub mod resolve;
pub mod scripts;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use aeth_devkit_core::process::SystemRunner;

/// Shell-completion data for poe tasks, served from Rust instead of a Python process.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-complete", version, about)]
pub struct Args {
  /// Ignore and rewrite the completion cache.
  #[arg(long, global = true)]
  pub no_cache: bool,

  #[command(subcommand)]
  pub command: Command,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
  /// Print task names on one line (replaces `poe _list_tasks`).
  Tasks {
    /// Project directory (defaults to the current directory; an empty string means the same).
    dir: Option<String>,
  },
  /// Print a task's arguments, tab-separated (replaces `poe _describe_task_args`).
  Args {
    task: String,
    /// Project directory (defaults to the current directory; an empty string means the same).
    dir: Option<String>,
  },
  /// Print a shell completion script that registers for the `poe` command.
  Script {
    #[arg(long, conflicts_with = "bash")]
    powershell: bool,
    #[arg(long)]
    bash: bool,
  },
}

/// What to print for `args`, given a fully resolved directory. Separated from the I/O so it
/// can be tested without a shell.
pub fn output(command: &Command, no_cache: bool) -> String {
  match command {
    Command::Tasks { dir } => {
      let root = project_dir(dir.as_deref());
      cache::resolve_cached(&root, &SystemRunner, no_cache)
        .map(|r| format::list_tasks(&r.tasks))
        .unwrap_or_default()
    }
    Command::Args { task, dir } => {
      let root = project_dir(dir.as_deref());
      cache::resolve_cached(&root, &SystemRunner, no_cache)
        .ok()
        .and_then(|r| r.tasks.into_iter().find(|t| &t.name == task))
        .map(|t| format::describe_task_args(&t))
        .unwrap_or_default()
    }
    Command::Script { bash, .. } => if *bash { scripts::BASH } else { scripts::POWERSHELL }.to_string(),
  }
}

/// The bash completion script always passes `"$target_path"`, which is the empty string when
/// no `-C` was given; treat that, and a missing argument, as "here". (`dir` is a `String`
/// rather than a `PathBuf` because clap's `PathBuf` parser rejects an empty value outright.)
fn project_dir(dir: Option<&str>) -> PathBuf {
  match dir {
    Some(d) if !d.is_empty() => PathBuf::from(d),
    _ => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
  }
}

/// Production entry: never exits non-zero — a completer that errors breaks the shell.
pub fn run_real(args: &Args) -> ExitCode {
  print!("{}", output(&args.command, args.no_cache));
  ExitCode::SUCCESS
}
