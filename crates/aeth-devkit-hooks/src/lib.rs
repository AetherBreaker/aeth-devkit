//! `devkit hook <name>` — Claude Code hooks, ported from the per-repo Python scripts.
//! See docs/specs/2026-08-28-agent-config-design.md §A.
//!
//! Every hook has the same shape: Claude pipes a JSON payload describing the event to stdin,
//! and the hook may print one JSON object to stdout telling Claude what to do. Printing
//! nothing means "no opinion". The binary must exit 0 no matter what, because a non-zero
//! exit is shown as a hook *error* in every session — so every failure path here degrades to
//! silence rather than to an error.

mod pre;
mod stop;

use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
// `Deserialize` is the derive that lets serde build a struct straight from JSON.
use serde::Deserialize;

use aeth_devkit_core::process::Runner;

/// Which hook to run. `ValueEnum` lets clap parse the kebab-case name (`pre-edit-protect`)
/// directly into the variant, so the CLI and the enum can never drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Hook {
  PreEditProtect,
  PreBashProtectDeps,
  StopRuff,
  StopPyright,
  StopClean,
}

/// Run one Claude Code hook: JSON payload on stdin, decision JSON on stdout.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-hook", version, about)]
pub struct Args {
  /// Hook to run.
  #[arg(value_enum)]
  pub hook: Hook,
}

/// The slice of Claude's hook payload we care about. Every field is `#[serde(default)]` so a
/// payload missing a key (a different tool, an older Claude) deserializes to empty strings
/// instead of failing — "missing" and "irrelevant" are the same thing to a hook.
#[derive(Debug, Deserialize, Default)]
pub struct Payload {
  #[serde(default)]
  pub tool_input: ToolInput,
}

#[derive(Debug, Deserialize, Default)]
pub struct ToolInput {
  #[serde(default)]
  pub file_path: String,
  #[serde(default)]
  pub command: String,
}

/// Decide what to tell Claude. `None` means "say nothing" (allow / no findings).
///
/// The `project_dir` and `runner` are only used by the Stop hooks; passing them in (rather
/// than reading the environment and spawning processes inside) is what makes the whole
/// crate testable without ruff or pyright installed.
pub fn run(hook: Hook, payload: &str, project_dir: &Path, runner: &dyn Runner) -> Option<String> {
  // Unparseable stdin → silence, per the module doc. `unwrap_or_default` turns the `Err`
  // into an all-empty `Payload`, which every hook below treats as "nothing to see".
  let payload: Payload = serde_json::from_str(payload).unwrap_or_default();
  let decision = match hook {
    Hook::PreEditProtect => pre::edit_protect(&payload.tool_input.file_path),
    Hook::PreBashProtectDeps => pre::bash_protect_deps(&payload.tool_input.command),
    Hook::StopRuff => stop::check(project_dir, &stop::RUFF, runner),
    Hook::StopPyright => stop::check(project_dir, &stop::PYRIGHT, runner),
    Hook::StopClean => stop::check(project_dir, &stop::CLEAN, runner),
  };
  // `Value::to_string()` renders compact JSON — one line, which is what Claude expects.
  decision.map(|v| v.to_string())
}

/// Production entry: never exits non-zero, because a failing hook surfaces as an error in
/// every Claude session.
pub fn run_real(args: &Args) -> ExitCode {
  // Read all of stdin. `read_to_string` needs the `Read` trait in scope; an unreadable stdin
  // leaves `payload` empty, which `run` treats as "nothing to decide".
  let mut payload = String::new();
  let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut payload);
  // Claude sets `CLAUDE_PROJECT_DIR` for every hook; the cwd fallback covers running a hook
  // by hand. `env::var` fails on a missing *or* non-UTF-8 value — either way, fall back.
  let project_dir = std::env::var("CLAUDE_PROJECT_DIR")
    .map(std::path::PathBuf::from)
    .or_else(|_| std::env::current_dir())
    .unwrap_or_else(|_| std::path::PathBuf::from("."));
  if let Some(decision) = run(args.hook, &payload, &project_dir, &aeth_devkit_core::process::SystemRunner) {
    println!("{decision}");
  }
  ExitCode::SUCCESS
}
