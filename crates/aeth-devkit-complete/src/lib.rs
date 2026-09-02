//! `devkit complete` — fast shell completion for poe tasks.
//!
//! poe's own completion costs ~200 ms per Tab press: it starts Python, imports the
//! poethepoet framework, and — with `include_script` — spawns a second process through
//! `uv run` to obtain the task table. This crate answers the same two questions
//! (`which tasks?`, `which args for this task?`) from Rust, with the task table resolved
//! natively for TOML and cached for `include_script`.

pub mod cache;
pub mod engine;
pub mod format;
pub mod install;
pub mod resolve;
pub mod scripts;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
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
  /// Install the completion into your shell profile(s), replacing poe's own registration.
  #[command(group = clap::ArgGroup::new("shell").required(true).multiple(true))]
  Install {
    /// Add a line to $PROFILE (and remove poe's `_powershell_completion` line).
    #[arg(long, group = "shell")]
    powershell: bool,
    /// Write ~/bash_completion.d/poe.bash (Git Bash) and
    /// ~/.local/share/bash-completion/completions/poe (bash-completion package).
    #[arg(long, group = "shell")]
    bash: bool,
    /// Report what would change without writing.
    #[arg(long)]
    dry_run: bool,
  },
}

/// What to print for the data and script subcommands. Separated from the I/O so it can be
/// tested without a shell. (`install` has side effects and goes through [`run_install`].)
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
    Command::Install { .. } => String::new(),
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

/// The bash completion files, in the order they are written. Both hold the same script;
/// see the module doc in [`install`] for why there are two.
fn bash_targets(home: &std::path::Path) -> Vec<PathBuf> {
  vec![
    home.join("bash_completion.d").join("poe.bash"),
    home
      .join(".local")
      .join("share")
      .join("bash-completion")
      .join("completions")
      .join("poe"),
  ]
}

/// `install`: print every change, and the warning or refusal from the PATH preflight.
pub fn run_install(powershell: bool, bash: bool, dry_run: bool) -> Result<()> {
  let path_var = std::env::var("PATH").unwrap_or_default();
  if let Some(warning) = install::preflight(&path_var)? {
    eprintln!("{warning}");
  }
  let mut log = Vec::new();
  if powershell {
    let profile = install::powershell_profile(&SystemRunner)?;
    log.extend(install::install_powershell(&profile, dry_run)?);
  }
  if bash {
    let home = std::env::var_os("HOME")
      .or_else(|| std::env::var_os("USERPROFILE"))
      .map(PathBuf::from)
      .ok_or_else(|| anyhow::anyhow!("neither HOME nor USERPROFILE is set"))?;
    log.extend(install::install_bash(&bash_targets(&home), scripts::BASH, dry_run)?);
  }
  if log.is_empty() {
    println!("Nothing to do — completion is already installed.");
  } else {
    println!("{}", if dry_run { "Would change:" } else { "Changed:" });
    for l in &log {
      println!("  - {l}");
    }
    if !dry_run {
      println!("Open a new shell for it to take effect. Re-run this after upgrading devkit if the script changes.");
    }
  }
  Ok(())
}

/// Production entry. The data/script subcommands never exit non-zero — a completer that
/// errors breaks the shell — but `install` is an ordinary command and may.
pub fn run_real(args: &Args) -> ExitCode {
  match &args.command {
    Command::Install { powershell, bash, dry_run } => match run_install(*powershell, *bash, *dry_run) {
      Ok(()) => ExitCode::SUCCESS,
      Err(e) => {
        eprintln!("error: {e:#}");
        ExitCode::from(1)
      }
    },
    other => {
      print!("{}", output(other, args.no_cache));
      ExitCode::SUCCESS
    }
  }
}
