//! Bake the poe task table into `_tasks_generated.py` before this crate compiles.
//!
//! Tasks are authored in `python/aeth_devkit/_tasks_source.py` against poethepoet_tasks'
//! `TaskCollection`, whose import costs ~24 ms — a price poe pays on every task run. The
//! table is computed once here and written out as a plain dict, which
//! `aeth_devkit/__init__.py` re-exports (~0.8 ms). None of these tasks are dynamic, so the
//! bake is faithful.
//!
//! Doing it in a build script rather than a PEP 517 shim is what keeps
//! `build-backend = "maturin"`: maturin builds this crate by invoking cargo, and cargo runs
//! build scripts first, so `cargo build`, `uv build`, `uv sync` and `pip install` all
//! regenerate on the way past. Nothing to remember before a release, and because the whole
//! generator lives in this file, the crate carries it — an sdist needs no extra packaging.
//!
//! Every failure degrades to a warning rather than breaking the build: `_tasks_generated.py`
//! is committed and shipped, so the worst case is building against a table that is merely
//! current-as-of-commit. `tests/` is what keeps that copy honest.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Stands in for the absolute scripts directory inside the generated file.
///
/// `_script_path()` interpolates absolute paths into task commands. Freezing those would ship
/// the *building* machine's paths to every consumer, where they do not exist — so they are
/// swapped for this marker and resolved from `__file__` when the generated module is imported.
const PLACEHOLDER: &str = "@@AETH_DEVKIT_SCRIPTS@@";

/// The generator, run as `python -c <this> <path to _tasks_source.py>`.
///
/// Kept in Python because the task table *is* the return value of a Python program; there is
/// no static declaration to read. `pprint` emits a valid Python literal and `sort_dicts=False`
/// preserves definition order, which poe's task listing depends on.
const GENERATOR: &str = r#"
import importlib.util
import pprint
import sys

spec = importlib.util.spec_from_file_location("_aeth_tasks_source", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

# A .generate() generator is evaluated per call, so baking its output would freeze a snapshot
# rather than mirror it. Nothing registers one today; fail loudly if that changes.
assert not module.tasks._task_generators, (
    "_tasks_source.py registers a .generate() task generator, whose output is dynamic and "
    "cannot be baked into a static table."
)

data = module.tasks()
# Dropped here and re-derived at import: it must name the generated file, not this source.
data.pop("config_path", None)

# _script_path() normalizes to forward slashes, so match that form.
scripts_dir = module._SCRIPTS_DIR.replace("\\", "/")


def placehold(value):
    """Swap the scripts directory for the marker, at any nesting depth.

    Done on the data rather than on the formatted text because pprint breaks long strings
    into adjacent literals, which can split a path down the middle and defeat a plain
    replace on the output.
    """
    if isinstance(value, str):
        return value.replace(scripts_dir, "@@AETH_DEVKIT_SCRIPTS@@")
    if isinstance(value, dict):
        return {k: placehold(v) for k, v in value.items()}
    if isinstance(value, list):
        return [placehold(v) for v in value]
    return value


print(pprint.pformat(placehold(data), width=110, sort_dicts=False))
"#;

/// The generated module, with `__RAW_BODY__` swapped for the literal the generator printed.
///
/// A plain template rather than `format!` so the Python braces below need no escaping.
const TEMPLATE: &str = r#""""The poe task table, baked from `_tasks_source.py` by `crates/aeth-devkit/build.rs`.

Do not edit by hand, and do not import poethepoet_tasks from here: this file exists so that
importing `aeth_devkit` costs ~0.8 ms rather than ~24 ms, which poe pays on every task run.
Edit `_tasks_source.py` and build.
"""

import os

# Absolute script paths cannot be baked: this package installs into each consuming project's
# own site-packages, so they are resolved from __file__ at import instead.
_SCRIPTS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scripts").replace("\\", "/")
_PLACEHOLDER = "@@AETH_DEVKIT_SCRIPTS@@"

_RAW = __RAW_BODY__


# Annotated as `object` rather than via typing: importing typing would cost ~5 ms, most of
# what this file exists to save.
def _resolve(value: object) -> object:
  """Substitute the real scripts directory into every string, at any nesting depth."""
  if isinstance(value, str):
    return value.replace(_PLACEHOLDER, _SCRIPTS_DIR)
  if isinstance(value, dict):
    return {k: _resolve(v) for k, v in value.items()}
  if isinstance(value, list):
    return [_resolve(v) for v in value]
  return value


def tasks(include_tags: object = (), exclude_tags: object = ()) -> dict:
  """The task table poe loads via `include_script = "aeth_devkit:tasks"`.

  `include_tags`/`exclude_tags` are accepted for signature parity with TaskCollection but
  ignored: no task here declares tags, and poe itself has no notion of them — filtering was
  always the callable's own job. Add it here if a consumer ever needs it.
  """
  resolved = _resolve(_RAW)
  # Narrowing for the type checker; _RAW is a dict literal, so this always holds.
  assert isinstance(resolved, dict)
  return {**resolved, "config_path": __file__}
"#;

fn main() {
  // `crates/aeth-devkit` -> `crates` -> repo root. Cargo guarantees CARGO_MANIFEST_DIR.
  let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"));
  let Some(root) = manifest_dir.parent().and_then(Path::parent) else {
    println!("cargo:warning=cannot locate the repo root from {}", manifest_dir.display());
    return;
  };

  let package = root.join("python").join("aeth_devkit");
  let source = package.join("_tasks_source.py");
  let out = package.join("_tasks_generated.py");

  // Without this, cargo caches the build script's first run and edits to the task definitions
  // would never reach the generated file. The *output* is deliberately not listed: watching
  // what this script writes would rebuild on every build, forever.
  println!("cargo:rerun-if-changed={}", source.display());

  // An sdist built before the source module was packaged still has a baked table; there is
  // simply nothing to refresh it from.
  if !source.is_file() {
    return;
  }

  let Some(python) = find_python(root) else {
    println!("cargo:warning=no Python found to regenerate the poe task table; using the committed one");
    return;
  };

  // stdin is closed rather than inherited: a build script has no console to prompt at, and an
  // interpreter that blocked on a read would hang the build indefinitely instead of failing.
  let output = match Command::new(&python)
    .arg("-c")
    .arg(GENERATOR)
    .arg(&source)
    .stdin(Stdio::null())
    .output()
  {
    Ok(output) => output,
    Err(err) => {
      println!("cargo:warning=could not run {}: {err}", python.display());
      return;
    }
  };
  if !output.status.success() {
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!(
      "cargo:warning=generating the poe task table failed: {}",
      stderr.trim().replace('\n', "; ")
    );
    return;
  }

  let body = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
  let body = body.trim();

  // The substitution happening in Python means it uses the exact string `_script_path` built;
  // verifying here catches the case where that stopped being true and real paths would ship.
  let scripts_dir = package.join("scripts").to_string_lossy().replace('\\', "/");
  if body.contains(&scripts_dir) {
    println!("cargo:warning=an absolute script path survived placeholding; keeping the committed table");
    return;
  }
  if !body.contains(PLACEHOLDER) {
    println!("cargo:warning=no task referenced the scripts directory; has _script_path changed?");
    return;
  }

  // Normalized rather than inherited: .gitattributes pins the repo to LF, and letting the
  // output follow this file's own line endings would rewrite every line of the generated
  // file whenever build.rs was checked out differently.
  let rendered = TEMPLATE.replace("__RAW_BODY__", body).replace("\r\n", "\n");

  // SAFETY-CRITICAL, do not remove: writing only on an actual content change is what keeps
  // this script from feeding a rebuild loop. It writes into the source tree, which editors
  // and rust-analyzer watch; an unconditional write would bump the mtime on every build, a
  // watcher would see the change and trigger another check, and round it goes. Because the
  // output is a pure function of `_tasks_source.py`, the steady state here is zero writes.
  // It also keeps `aeth-devkit-complete`'s completion cache — keyed on mtime — valid.
  //
  // Compared as bytes: reading as a String would hide a line-ending difference that `write`
  // would then bake in, making every build look like a change.
  if std::fs::read(&out).is_ok_and(|existing| existing == rendered.as_bytes()) {
    return;
  }

  // Written via a temporary and renamed into place so a reader never sees a half-written
  // module. rust-analyzer runs `cargo check` on its own schedule, so two build scripts can
  // be in here at once; `rename` replaces atomically on both Windows and Unix, which turns a
  // concurrent overwrite into "last writer wins" rather than a truncated file.
  let temp = out.with_extension("py.tmp");
  if let Err(err) = std::fs::write(&temp, rendered) {
    println!("cargo:warning=could not write {}: {err}", temp.display());
    return;
  }
  if let Err(err) = std::fs::rename(&temp, &out) {
    println!("cargo:warning=could not replace {}: {err}", out.display());
    let _ = std::fs::remove_file(&temp);
  }
}

/// The interpreter to run the generator with, most specific first.
///
/// The generator imports poethepoet_tasks, so the interpreter has to be one that has it.
fn find_python(root: &Path) -> Option<PathBuf> {
  let mut candidates = Vec::new();

  // A PEP 517 build runs in an isolated venv holding exactly `[build-system] requires` —
  // which lists poethepoet-tasks for this reason. maturin passes that environment down to
  // cargo as VIRTUAL_ENV, so it is the only interpreter guaranteed to have the dependency
  // during a packaging build.
  if let Ok(venv) = std::env::var("VIRTUAL_ENV") {
    candidates.push(PathBuf::from(venv));
  }
  // A plain `cargo build`/`cargo test` has no build venv; the project's own has the dev deps.
  candidates.push(root.join(".venv"));

  for venv in candidates {
    // Joined component-by-component so the result keeps native separators throughout.
    for exe in [venv.join("Scripts").join("python.exe"), venv.join("bin").join("python")] {
      if exe.is_file() {
        return Some(exe);
      }
    }
  }

  // Last resort: whatever `python` resolves to. May well lack poethepoet_tasks, in which case
  // the generator fails and the caller degrades to the committed table.
  Some(PathBuf::from("python"))
}
