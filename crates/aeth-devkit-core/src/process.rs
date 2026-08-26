//! Running external commands, with a recording implementation for tests.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

/// One recorded external command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
  pub program: String,
  pub args: Vec<String>,
  pub cwd: PathBuf,
}

pub trait Runner {
  /// Run `program args` in `cwd` with inherited stdio. Returns the exit code, or `None`
  /// when the process was terminated by a signal.
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>>;
}

/// Executes commands for real.
pub struct SystemRunner;

impl Runner for SystemRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    let status = Command::new(program)
      .args(args)
      .current_dir(cwd)
      .status()
      .with_context(|| format!("running {program}"))?;
    Ok(status.code())
  }
}

/// Records every call and returns a fixed exit code; for tests.
pub struct RecordingRunner {
  pub calls: RefCell<Vec<Invocation>>,
  pub exit_code: i32,
}

impl RecordingRunner {
  pub fn new(exit_code: i32) -> Self {
    Self {
      calls: RefCell::new(Vec::new()),
      exit_code,
    }
  }
}

impl Runner for RecordingRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    self.calls.borrow_mut().push(Invocation {
      program: program.to_string(),
      args: args.to_vec(),
      cwd: cwd.to_path_buf(),
    });
    Ok(Some(self.exit_code))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn recording_runner_records_and_returns_code() {
    let r = RecordingRunner::new(3);
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
  fn system_runner_runs_git_version() {
    let code = SystemRunner.run_inherit("git", &["--version".into()], Path::new(".")).unwrap();
    assert_eq!(code, Some(0));
  }
}
