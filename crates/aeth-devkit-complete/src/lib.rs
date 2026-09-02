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
pub mod parse;
pub mod resolve;
pub mod scripts;
pub mod wire;
pub mod words;

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
  /// Answer one completion request from a shell shim. Not meant to be typed by hand.
  Query {
    /// Which shim is asking, which decides how the command line was handed over.
    #[arg(long, value_enum)]
    shell: engine::Shell,
    /// The shim's own version, so a drifted shim can be detected and repaired.
    #[arg(long)]
    shim_version: u32,
    /// bash: the raw `COMP_LINE`.
    #[arg(long)]
    line: Option<String>,
    /// bash: `COMP_POINT`, a byte offset into `--line`.
    #[arg(long)]
    point: Option<usize>,
    /// PowerShell: index of the element the cursor is on.
    #[arg(long)]
    cword: Option<usize>,
    /// PowerShell: its own `$wordToComplete`, authoritative for the prefix.
    #[arg(long)]
    word_to_complete: Option<String>,
    /// PowerShell: the parsed command elements, after `--`.
    ///
    /// `allow_hyphen_values` is essential: these routinely start with `-` (`poe -C ../x`)
    /// and clap would otherwise try to parse them as flags of its own.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    words: Vec<String>,
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
    Command::Query {
      shell,
      line,
      point,
      cword,
      word_to_complete,
      words,
      ..
    } => {
      let req = build_request(*shell, line.as_deref(), *point, *cword, word_to_complete.as_deref(), words);
      wire::render(&engine::complete(&req, &SystemRunner, no_cache))
    }
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

/// Assemble an [`engine::Request`] from whichever shape the shim sent.
///
/// The two shells are deliberately asymmetric: bash hands over raw text because its own
/// word splitting breaks on `=`, while PowerShell hands over elements its parser already
/// produced, which are authoritative and would only be degraded by re-parsing.
fn build_request(
  shell: engine::Shell,
  line: Option<&str>,
  point: Option<usize>,
  cword: Option<usize>,
  word_to_complete: Option<&str>,
  words: &[String],
) -> engine::Request {
  let (words, cword, prefix) = match shell {
    engine::Shell::Bash => {
      // `unwrap_or_default` covers a shim that somehow omitted the pair: an empty line
      // completes nothing, which is the correct degenerate answer.
      let split = words::split_line(line.unwrap_or_default(), point.unwrap_or(0));
      (split.words, split.cword, split.prefix)
    }
    engine::Shell::PowerShell => {
      // A missing `--cword` means "past the last element", i.e. a fresh word.
      let cword = cword.unwrap_or(words.len());
      (words.to_vec(), cword, word_to_complete.unwrap_or_default().to_string())
    }
  };

  // `-C ../other` retargets the whole request; without it the process cwd is the project.
  let cwd = project_dir(None);
  let root = match parse::parse(&words, cword).target_dir.as_deref() {
    // `Path::join` replaces the base outright when the argument is absolute, so this one
    // expression handles both relative and absolute `-C` values.
    Some(dir) if !dir.is_empty() => cwd.join(dir),
    _ => cwd,
  };

  engine::Request { shell, words, cword, prefix, root }
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
