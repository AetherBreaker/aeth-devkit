//! Stop hooks: run a project-wide checker and hand its complaints back to Claude as context.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use aeth_devkit_core::process::Runner;

/// Claude reads `additionalContext` as if it were part of the conversation; anything
/// past this many characters is noise it would have to scroll through anyway.
const MAX_CHARS: usize = 4000;

/// A checker to run: the console-script name, its arguments, and the label used in the
/// report. `&'static` because every value is a literal baked into the binary.
pub struct Tool {
  pub name: &'static str,
  pub args: &'static [&'static str],
  pub label: &'static str,
}

pub const RUFF: Tool = Tool {
  name: "ruff",
  // `--unfixable F401` keeps ruff sorting/formatting imports (and applying other safe
  // autofixes) but leaves unused-import removal as a *reported* violation instead of
  // silently deleting the import — an import added in one edit and used in a later one
  // shouldn't get stripped in between.
  args: &["check", "--fix", "--unfixable", "F401", "."],
  label: "ruff check (project-wide)",
};

pub const PYRIGHT: Tool = Tool {
  name: "pyright",
  args: &[],
  label: "pyright (project-wide)",
};

pub const CLEAN: Tool = Tool {
  name: "poe",
  args: &["clean"],
  label: "poe clean",
};

/// Where to find `tool`: the venv's own console script if it exists, else `uv run`.
///
/// `uv run` spends ~140 ms checking the environment is in sync before it starts the tool.
/// That is the right default for a human at a shell, but a hook runs after *every* turn, so
/// we go straight to the binary when the venv has one. Returns the program to spawn and the
/// arguments to put in front of the tool's own.
fn resolve(project_dir: &Path, tool: &Tool) -> (String, Vec<String>) {
  let venv = project_dir.join(".venv");
  // Windows venvs put console scripts in `Scripts/` with an `.exe` suffix; Unix in `bin/`.
  // Checking both means the same binary behaves on either platform without `cfg!` gates.
  let candidates: [PathBuf; 2] = [
    venv.join("Scripts").join(format!("{}.exe", tool.name)),
    venv.join("bin").join(tool.name),
  ];
  // `into_iter` on an array (edition 2021+) yields owned `PathBuf`s; `find` hands back the
  // first that exists; `map` turns it into the `(program, no prefix)` pair.
  if let Some(exe) = candidates.into_iter().find(|p| p.is_file()) {
    return (exe.to_string_lossy().into_owned(), Vec::new());
  }
  ("uv".to_string(), vec!["run".to_string(), tool.name.to_string()])
}

/// Run `tool` in `project_dir` and, if it failed *and* said something, report that.
pub fn check(project_dir: &Path, tool: &Tool, runner: &dyn Runner) -> Option<Value> {
  let (program, mut args) = resolve(project_dir, tool);
  args.extend(tool.args.iter().map(|s| s.to_string()));
  // `.ok()?`: a spawn error (tool not installed) becomes `None` — silence, not a hook error.
  let out = runner.run_capture(&program, &args, project_dir).ok()?;
  if out.success() {
    return None;
  }
  // Python's `(stdout + stderr).strip()`, then `[:4000]` — which slices by *character*.
  // `str::chars().take(n)` is the Rust equivalent; byte-slicing `&s[..4000]` would panic if
  // byte 4000 fell inside a multibyte character (an `é` in a path, say).
  let text: String = format!("{}{}", out.stdout, out.stderr).trim().chars().take(MAX_CHARS).collect();
  if text.is_empty() {
    return None;
  }
  Some(json!({
    "hookSpecificOutput": {
      "hookEventName": "Stop",
      "additionalContext": format!("{} reported issues:\n{text}", tool.label),
    }
  }))
}
