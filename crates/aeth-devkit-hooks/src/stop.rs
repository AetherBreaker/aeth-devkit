//! Stop hooks: run a checker and hand its complaints back to Claude as context.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use aeth_devkit_core::process::Runner;

/// Claude reads `additionalContext` as if it were part of the conversation; anything
/// past this many characters is noise it would have to scroll through anyway.
const MAX_CHARS: usize = 4000;

/// Windows caps a command line near 32 KiB. A branch touching enough files to approach that
/// is one where a whole-project run is the more useful answer anyway.
const MAX_ARG_CHARS: usize = 30_000;

/// A checker to run: the console-script name, its arguments, and the label used in the
/// report. `&'static` because every value is a literal baked into the binary.
pub struct Tool {
  pub name: &'static str,
  pub args: &'static [&'static str],
  pub label: &'static str,
  /// Whether to narrow this tool to the branch's changed files (see [`scope`]).
  pub scoped: bool,
}

pub const RUFF: Tool = Tool {
  name: "ruff",
  // `--unfixable F401` keeps ruff sorting/formatting imports (and applying other safe
  // autofixes) but leaves unused-import removal as a *reported* violation instead of
  // silently deleting the import -- an import added in one edit and used in a later one
  // shouldn't get stripped in between.
  //
  // No trailing path: `check` appends either the scoped file list or `.`.
  args: &["check", "--fix", "--unfixable", "F401"],
  label: "ruff check",
  scoped: true,
};

pub const PYRIGHT: Tool = Tool {
  name: "pyright",
  args: &[],
  label: "pyright (project-wide)",
  // Type errors are non-local: changing a signature here breaks a caller somewhere else,
  // possibly outside the branch diff. Narrowing pyright would hide exactly the regressions
  // it exists to catch, so it always runs whole-project.
  scoped: false,
};

pub const CLEAN: Tool = Tool {
  name: "poe",
  args: &["clean"],
  label: "poe clean",
  // Deletes generated files; it has no per-file output to narrow.
  scoped: false,
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

/// The Python files this branch has touched, or `None` for "no branch scope — whole project".
///
/// `Some(vec![])` is meaningfully different from `None`: it means the branch is real but has
/// changed nothing, so the caller should skip the check rather than widen it.
///
/// On `main`/`master` the answer is deliberately `None`. That is where cross-project lint
/// cleanup happens, so the whole-project run is the useful one there.
pub fn scope(project_dir: &Path, runner: &dyn Runner) -> Option<Vec<String>> {
  // A tiny closure over the runner: yields captured stdout only for a clean exit, so
  // "git is missing", "not a repo", and "that ref does not exist" all collapse to `None`.
  //
  // `-c core.quotePath=false` matters more than it looks: the setting defaults to *on*, and
  // with it git wraps a non-ASCII path in quotes and octal-escapes the bytes, so `cafe.py`
  // with an accented `e` arrives as a quoted, escaped string that no longer ends in `.py`.
  // Such a file fails the suffix test below and vanishes from the scope; a branch touching
  // only non-ASCII paths would lint nothing while the hook stayed silent. Setting it here
  // ignores whatever the user has configured.
  let git = |args: &[&str]| -> Option<String> {
    let mut full: Vec<String> = vec!["-c".to_string(), "core.quotePath=false".to_string()];
    full.extend(args.iter().map(|s| s.to_string()));
    let out = runner.run_capture("git", &full, project_dir).ok()?;
    out.success().then_some(out.stdout)
  };

  let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"])?.trim().to_string();
  // `HEAD` as the branch name means detached: there is no branch whose scope we could take.
  if matches!(branch.as_str(), "main" | "master" | "HEAD") {
    return None;
  }

  // Prefer the remote's view of the default branch: a stale local `main` would otherwise
  // widen the diff to include everything merged since it was last pulled.
  let base_rev = ["origin/main", "origin/master", "main", "master"]
    .into_iter()
    .find_map(|candidate| git(&["merge-base", "HEAD", candidate]))
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())?;

  // Every source below names paths from the *repo root*, which is the directory the checker
  // runs in only when Claude was opened at the repo root. Resolving through the root first
  // is what lets the three sources be compared to each other and handed to ruff safely.
  let toplevel = git(&["rev-parse", "--show-toplevel"])
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .map(PathBuf::from)
    .unwrap_or_else(|| project_dir.to_path_buf());
  // Canonicalize both sides so the `strip_prefix` below compares like with like: on Windows
  // that also settles the verbatim `\\?\` prefix and 8.3 short names, which would never match otherwise.
  let base = std::fs::canonicalize(project_dir).unwrap_or_else(|_| project_dir.to_path_buf());

  let mut files: Vec<String> = Vec::new();
  // `--diff-filter=d` drops deletions: ruff errors out when handed a path that is gone.
  let three_dot = format!("{base_rev}...HEAD");
  let sources: [Vec<&str>; 3] = [
    // Committed work on this branch, measured from where it diverged.
    vec!["diff", "--name-only", "--diff-filter=d", &three_dot],
    // Staged and unstaged edits not yet committed.
    vec!["diff", "--name-only", "--diff-filter=d", "HEAD"],
    // Brand-new files git is not tracking yet. `--full-name` makes these repo-root-relative
    // like the two diffs; without it they come back relative to the cwd and the three
    // sources silently disagree about what the same path means.
    vec!["ls-files", "--others", "--exclude-standard", "--full-name"],
  ];
  for args in sources {
    let Some(out) = git(&args) else { continue };
    for line in out.lines() {
      let path = line.trim();
      // Ruff only reads Python; feeding it a README wastes a process and risks an error.
      if !(path.ends_with(".py") || path.ends_with(".pyi")) {
        continue;
      }
      let abs = toplevel.join(path);
      // `--diff-filter=d` only drops files deleted *in the commits being compared*. A file
      // added earlier on the branch and then removed from the worktree without committing is
      // still listed by the commit-to-commit diff, and handing ruff a path that is gone earns
      // an `E902 ... (os error 2)` that Claude is then asked to fix -- on every turn, with no
      // fix available, until the deletion is committed.
      if !abs.is_file() {
        continue;
      }
      // Name the file the way the directory ruff runs in names it, and drop anything outside
      // that directory: a path ruff cannot resolve is worse than a file left unchecked.
      let abs = std::fs::canonicalize(&abs).unwrap_or(abs);
      let Ok(rel) = abs.strip_prefix(&base) else { continue };
      let rel = rel.to_string_lossy().replace('\\', "/");
      // The three sources overlap: a file can be committed on the branch and since re-edited.
      if !files.contains(&rel) {
        files.push(rel);
      }
    }
  }

  // Too many paths to pass safely: let the caller fall back to a whole-project run.
  if files.iter().map(|f| f.len() + 1).sum::<usize>() > MAX_ARG_CHARS {
    return None;
  }
  Some(files)
}

/// Run `tool` in `project_dir` and, if it failed *and* said something, report that.
pub fn check(project_dir: &Path, tool: &Tool, runner: &dyn Runner) -> Option<Value> {
  let (program, mut args) = resolve(project_dir, tool);
  args.extend(tool.args.iter().map(|s| s.to_string()));

  // The label tells Claude how much was actually examined, so a clean report is not
  // mistaken for "the whole project is clean" when only the branch diff was checked.
  let mut label = tool.label.to_string();
  if tool.scoped {
    match scope(project_dir, runner) {
      // A real branch that has touched no Python: there is nothing to check.
      Some(files) if files.is_empty() => return None,
      Some(files) => {
        label = format!("{label} (branch diff, {} file(s))", files.len());
        args.extend(files);
      }
      None => {
        label = format!("{label} (project-wide)");
        args.push(".".to_string());
      }
    }
  }

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
      "additionalContext": format!("{label} reported issues:\n{text}"),
    }
  }))
}
