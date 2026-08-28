//! Running external commands, with a recording implementation for tests.
//!
//! Every `devkit` command shells out to tools such as `uv`, `git`, and `gh`. Rather than
//! calling `std::process::Command` directly from the business logic, the commands talk to
//! a small [`Runner`] *trait*. In production the trait is implemented by [`SystemRunner`],
//! which really spawns processes; in tests it is implemented by [`RecordingRunner`], which
//! only remembers what it was asked to run and hands back scripted answers. This is
//! "dependency injection" done the Rust way: the caller receives `&dyn Runner` and never
//! knows (or cares) which implementation is behind it.

// `Cell` and `RefCell` give us *interior mutability*: a way to mutate data through a shared
// `&self` reference. Rust's normal rule is "many readers XOR one writer", enforced at compile
// time. `Cell<T>` (for `Copy` types) and `RefCell<T>` (for anything) move that check to run
// time so a method taking `&self` can still update bookkeeping. We want that here because
// the `Runner` trait methods take `&self` — a runner is shared read-only by callers — yet the
// recording implementation must append to its call log.
use std::cell::{Cell, RefCell};
// `Path` is the borrowed, unsized view of a filesystem path (like `str`); `PathBuf` is the
// owned, growable version (like `String`). Functions take `&Path`, structs store `PathBuf`.
use std::path::{Path, PathBuf};
// The standard library's process spawner.
use std::process::Command;

// `anyhow::Result<T>` is shorthand for `Result<T, anyhow::Error>`: a convenient catch-all
// error type for applications. `Context` is a trait that adds `.context("…")` /
// `.with_context(|| …)` to `Result`s so errors carry a human-readable trail of what we were
// doing. `as _` imports the trait for its methods without bringing its name into scope.
use anyhow::{Context as _, Result};

/// One recorded external command.
///
/// `#[derive(...)]` asks the compiler to generate trait implementations for us:
/// `Debug` (so `{:?}` prints it), `Clone` (deep copy), and `PartialEq`/`Eq` (so tests can
/// `assert_eq!` two invocations).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
  pub program: String,
  pub args: Vec<String>,
  pub cwd: PathBuf,
}

/// What a finished process left behind when we captured its output instead of letting it
/// print to the terminal.
///
/// `code` is `Option<i32>` because on Unix a process killed by a signal has *no* exit code;
/// `None` models that honestly rather than inventing a number. `Default` lets us write
/// `CapturedOutput { code: Some(0), ..Default::default() }` to fill the rest with empties.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapturedOutput {
  pub code: Option<i32>,
  pub stdout: String,
  pub stderr: String,
}

impl CapturedOutput {
  /// True only for a clean exit code of zero. Comparing an `Option` with `Some(0)` handles
  /// the `None` (signal) case as "not success" with no extra branching.
  pub fn success(&self) -> bool {
    self.code == Some(0)
  }
}

/// Something that can run a program for us.
///
/// A *trait* is Rust's interface: a set of method signatures a type promises to provide.
/// Callers hold `&dyn Runner` — a "trait object" — which is a fat pointer (data pointer +
/// vtable pointer) that dispatches to whichever concrete type is behind it at run time.
pub trait Runner {
  /// Run `program args` in `cwd` with inherited stdio (the child's output goes straight to
  /// the user's terminal). Returns the exit code, or `None` when the process was terminated
  /// by a signal. Use this for long-running tools whose progress the user wants to see.
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>>;

  /// Run `program args` in `cwd` and capture stdout/stderr instead of showing them. Use
  /// this when we need to *parse* the output (e.g. `uv version`, `git rev-parse`).
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput>;
}

/// Executes commands for real.
///
/// A unit struct (no fields) is a perfectly good type to hang a trait implementation on;
/// it costs nothing at run time.
pub struct SystemRunner;

impl Runner for SystemRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    // Builder pattern: each call returns `&mut Command` so we can chain. `.status()` spawns
    // the child, waits for it, and returns its `ExitStatus`. The `?` operator unwraps the
    // `Ok` value or *returns early* with the error — after `with_context` has wrapped it
    // with a message saying which program we were trying to run. The closure `|| …` is
    // only evaluated on the error path, so the happy path pays nothing for the message.
    let status = Command::new(program)
      .args(args)
      .current_dir(cwd)
      .status()
      .with_context(|| format!("running {program}"))?;
    // `status.code()` is already an `Option<i32>` for exactly the signal reason above.
    Ok(status.code())
  }

  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    // `.output()` is like `.status()` but also collects the child's stdout and stderr as
    // raw bytes (`Vec<u8>`), because a process may print non-UTF-8.
    let out = Command::new(program)
      .args(args)
      .current_dir(cwd)
      .output()
      .with_context(|| format!("running {program}"))?;
    Ok(CapturedOutput {
      code: out.status.code(),
      // `from_utf8_lossy` never fails: invalid bytes become U+FFFD. It returns a `Cow<str>`
      // (borrowed if already valid, owned if it had to fix anything); `into_owned` forces a
      // `String` either way so the struct owns its text.
      stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
      stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
  }
}

/// A canned answer the [`RecordingRunner`] gives for calls matching `program` whose
/// arguments start with `arg_prefix`.
#[derive(Debug, Clone)]
pub struct Script {
  pub program: String,
  pub arg_prefix: Vec<String>,
  pub code: i32,
  pub stdout: String,
  pub stderr: String,
}

/// Records every call and answers from scripts; for tests.
///
/// Matching rules: the *most recently registered* [`Script`] whose `program` equals the
/// call's program and whose `arg_prefix` is a prefix of the call's arguments wins, so a test
/// can register broad defaults first and override one call later; scripts are never
/// consumed, so one script answers any number of calls. Unmatched calls succeed/fail with `exit_code` and
/// produce no output. [`fail_at`](Self::fail_at) overrides everything for one specific call.
pub struct RecordingRunner {
  /// Public so tests can inspect the raw log; wrapped in `RefCell` so `&self` methods can
  /// push to it (see the note on interior mutability at the top of the file).
  pub calls: RefCell<Vec<Invocation>>,
  /// Exit code for calls no script matches.
  pub exit_code: i32,
  // Private fields: tests use the `script` / `fail_at` methods rather than poking these.
  scripts: RefCell<Vec<Script>>,
  // `Cell` (not `RefCell`) because `Option<usize>` is `Copy`: we can `get`/`set` it whole.
  fail_at: Cell<Option<usize>>,
}

impl RecordingRunner {
  pub fn new(exit_code: i32) -> Self {
    Self {
      calls: RefCell::new(Vec::new()),
      exit_code,
      scripts: RefCell::new(Vec::new()),
      fail_at: Cell::new(None),
    }
  }

  /// Register a canned answer. Returns `&Self` so calls can be chained:
  /// `r.script(…).script(…)`.
  pub fn script(&self, program: &str, arg_prefix: &[&str], code: i32, stdout: &str) -> &Self {
    // `borrow_mut()` takes the RefCell's run-time write lock; it panics if anything else
    // currently holds a borrow, which is why we keep the borrow short (one statement).
    self.scripts.borrow_mut().push(Script {
      program: program.to_string(),
      // Turn a slice of `&str` into a `Vec<String>`: iterate, convert each, collect.
      arg_prefix: arg_prefix.iter().map(|s| s.to_string()).collect(),
      code,
      stdout: stdout.to_string(),
      stderr: String::new(),
    });
    self
  }

  /// Like [`script`](Self::script) but with a scripted stderr too, for callers that decide
  /// what a non-zero exit *means* by reading it (e.g. `gh`'s "release not found").
  pub fn script_err(&self, program: &str, arg_prefix: &[&str], code: i32, stderr: &str) -> &Self {
    self.scripts.borrow_mut().push(Script {
      program: program.to_string(),
      arg_prefix: arg_prefix.iter().map(|s| s.to_string()).collect(),
      code,
      stdout: String::new(),
      stderr: stderr.to_string(),
    });
    self
  }

  /// Make the `nth_call` (1-based, counting both `run_inherit` and `run_capture`) fail with
  /// exit code 1, regardless of scripts. Lets a test inject a failure at a precise step.
  pub fn fail_at(&self, nth_call: usize) {
    self.fail_at.set(Some(nth_call));
  }

  /// The argument lists of every recorded call to `program`, in order.
  pub fn calls_for(&self, program: &str) -> Vec<Vec<String>> {
    // `borrow()` is the read lock. `filter` keeps matching programs, `map` extracts the
    // args (cloned, because we are returning owned data past the borrow's lifetime).
    self
      .calls
      .borrow()
      .iter()
      .filter(|c| c.program == program)
      .map(|c| c.args.clone())
      .collect()
  }

  /// Shared implementation for both trait methods: log the call, then decide the answer.
  fn record(&self, program: &str, args: &[String], cwd: &Path) -> CapturedOutput {
    self.calls.borrow_mut().push(Invocation {
      program: program.to_string(),
      args: args.to_vec(),
      cwd: cwd.to_path_buf(),
    });
    // The call we just pushed is call number `len` (1-based).
    let n = self.calls.borrow().len();
    if self.fail_at.get() == Some(n) {
      return CapturedOutput {
        code: Some(1),
        stdout: String::new(),
        stderr: "scripted failure".into(),
      };
    }
    let scripts = self.scripts.borrow();
    // `.rev()` walks newest-first so later registrations override earlier ones; `find`
    // returns the first script satisfying the closure; `starts_with` on slices checks that
    // `args` begins with every element of `arg_prefix`, in order.
    match scripts
      .iter()
      .rev()
      .find(|s| s.program == program && args.starts_with(&s.arg_prefix))
    {
      Some(s) => CapturedOutput {
        code: Some(s.code),
        stdout: s.stdout.clone(),
        stderr: s.stderr.clone(),
      },
      // Struct-update syntax: take `code` from here, everything else from `Default`.
      None => CapturedOutput {
        code: Some(self.exit_code),
        ..Default::default()
      },
    }
  }
}

impl Runner for RecordingRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    Ok(self.record(program, args, cwd).code)
  }

  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    Ok(self.record(program, args, cwd))
  }
}

// `#[cfg(test)]` compiles this module only for `cargo test`, so test code never bloats the
// shipped binary. `use super::*` pulls in everything from the enclosing module.
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn recording_runner_records_and_returns_code() {
    let r = RecordingRunner::new(3);
    // `.into()` converts `&str` → `String` via the `From`/`Into` traits; the target type is
    // inferred from the `&[String]` parameter.
    let code = r
      .run_inherit("uv", &["sync".into(), "--upgrade".into()], Path::new("/proj"))
      .unwrap();
    assert_eq!(code, Some(3));
    let calls = r.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "uv");
    assert_eq!(calls[0].args, vec!["sync", "--upgrade"]);
    assert_eq!(calls[0].cwd, PathBuf::from("/proj"));
  }

  #[test]
  fn recording_runner_scripts_by_program_and_arg_prefix() {
    let r = RecordingRunner::new(0);
    r.script("uv", &["version"], 0, "demo 1.0.0 => 1.0.1\n");
    // A longer argument list still matches because only the prefix has to agree.
    let out = r
      .run_capture("uv", &["version".into(), "--bump".into(), "patch".into()], Path::new("."))
      .unwrap();
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout, "demo 1.0.0 => 1.0.1\n");
    assert!(out.success());
    // No script for `uv build`: falls back to the default exit code and empty output.
    let other = r.run_capture("uv", &["build".into()], Path::new(".")).unwrap();
    assert_eq!(other.stdout, "");
    assert_eq!(r.calls_for("uv").len(), 2);
    assert_eq!(r.calls_for("uv")[1], vec!["build"]);
  }

  #[test]
  fn recording_runner_fail_at_fails_exactly_that_call() {
    let r = RecordingRunner::new(0);
    r.fail_at(2);
    assert_eq!(r.run_inherit("a", &[], Path::new(".")).unwrap(), Some(0));
    assert_eq!(r.run_inherit("b", &[], Path::new(".")).unwrap(), Some(1));
    let c = r.run_capture("c", &[], Path::new(".")).unwrap();
    assert_eq!(c.code, Some(0));
  }

  #[test]
  fn system_runner_runs_a_real_process() {
    // `CARGO` is set by cargo for every test run, so this needs nothing beyond the toolchain.
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let code = SystemRunner.run_inherit(&cargo, &["--version".into()], Path::new(".")).unwrap();
    assert_eq!(code, Some(0));
  }

  #[test]
  fn system_runner_reports_nonzero_exit() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let code = SystemRunner
      .run_inherit(&cargo, &["--definitely-not-a-flag".into()], Path::new("."))
      .unwrap();
    assert_ne!(code, Some(0));
  }

  #[test]
  fn system_runner_captures_stdout() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let out = SystemRunner.run_capture(&cargo, &["--version".into()], Path::new(".")).unwrap();
    assert!(out.success());
    assert!(out.stdout.starts_with("cargo "));
  }
}
