//! Post-merge formatting of `pyproject.toml` with tombi, when it is installed.

use std::path::Path;

use anyhow::Result;

pub use aeth_devkit_core::process::{Runner, SystemRunner};

use crate::changes::Changes;

/// What the formatting step did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
  /// tombi ran; `true` when it changed the file.
  Formatted(bool),
  /// tombi is not installed (not on `PATH`); the step was skipped.
  Unavailable,
  /// tombi ran but exited non-zero (a TOML error, most likely). The file is left as merged.
  Failed { code: Option<i32> },
}

/// Run `tombi format pyproject.toml` in `root`. When the formatter changes the file, the
/// change is added to `changes` so it is reported and committed with the rest. Never
/// returns an error for a missing or failing tombi: formatting is best-effort.
pub fn format_pyproject(root: &Path, runner: &dyn Runner, changes: &mut Changes) -> Result<Outcome> {
  let path = root.join("pyproject.toml");
  let before = std::fs::read_to_string(&path)?;
  let code = match runner.run_inherit("tombi", &["format".into(), "--quiet".into(), "pyproject.toml".into()], root) {
    Ok(code) => code,
    Err(e) if is_not_found(&e) => return Ok(Outcome::Unavailable),
    Err(e) => return Err(e),
  };
  if code != Some(0) {
    return Ok(Outcome::Failed { code });
  }
  let after = std::fs::read_to_string(&path)?;
  let changed = after != before;
  if changed {
    changes.note(&path, "formatted with tombi");
  }
  Ok(Outcome::Formatted(changed))
}

fn is_not_found(e: &anyhow::Error) -> bool {
  e.chain()
    .filter_map(|c| c.downcast_ref::<std::io::Error>())
    .any(|io| io.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(test)]
mod tests {
  use std::path::PathBuf;

  use aeth_devkit_core::process::{CapturedOutput, RecordingRunner};

  use super::*;

  /// Fails every spawn the way `Command::status` does when the program is not on PATH.
  struct MissingRunner;
  impl Runner for MissingRunner {
    fn run_inherit_env(&self, program: &str, _: &[String], _: &Path, _: &[(&str, &str)]) -> Result<Option<i32>> {
      use anyhow::Context as _;
      Err(std::io::Error::from(std::io::ErrorKind::NotFound)).with_context(|| format!("running {program}"))
    }
    // A missing program fails the same way whether or not output is captured, so this
    // simply reuses `run_inherit` and discards its (never produced) exit code.
    fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
      self.run_inherit(program, args, cwd).map(|_| CapturedOutput::default())
    }
  }

  /// Rewrites pyproject.toml on "run" and reports the given exit code.
  struct RewritingRunner {
    content: &'static str,
    exit_code: i32,
  }
  impl Runner for RewritingRunner {
    fn run_inherit_env(&self, _: &str, _: &[String], cwd: &Path, _: &[(&str, &str)]) -> Result<Option<i32>> {
      std::fs::write(cwd.join("pyproject.toml"), self.content)?;
      Ok(Some(self.exit_code))
    }
    // Same side effect, with the exit code wrapped in a `CapturedOutput` and no text.
    fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
      let code = self.run_inherit(program, args, cwd)?;
      Ok(CapturedOutput {
        code,
        ..Default::default()
      })
    }
  }

  fn project(content: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("pyproject.toml"), content).unwrap();
    (dir, root)
  }

  #[test]
  fn invokes_tombi_format_on_pyproject_in_root() {
    let (_d, root) = project("[project]\nname = \"x\"\n");
    let runner = RecordingRunner::new(0);
    let mut changes = Changes::new(false);
    let out = format_pyproject(&root, &runner, &mut changes).unwrap();
    assert_eq!(out, Outcome::Formatted(false));
    let calls = runner.calls.borrow();
    assert_eq!(calls[0].program, "tombi");
    assert_eq!(calls[0].args, vec!["format", "--quiet", "pyproject.toml"]);
    assert_eq!(calls[0].cwd, root);
    assert!(changes.is_empty(), "unchanged file must not be recorded");
  }

  #[test]
  fn records_change_when_formatter_rewrites_file() {
    let (_d, root) = project("[project]\nname=\"x\"\n");
    let runner = RewritingRunner {
      content: "[project]\nname = \"x\"\n",
      exit_code: 0,
    };
    let mut changes = Changes::new(false);
    let out = format_pyproject(&root, &runner, &mut changes).unwrap();
    assert_eq!(out, Outcome::Formatted(true));
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].path, root.join("pyproject.toml"));
    assert!(!changes.files[0].created);
    assert_eq!(changes.files[0].details, vec!["formatted with tombi"]);
  }

  #[test]
  fn appends_detail_to_existing_pyproject_entry() {
    let (_d, root) = project("[project]\nname=\"x\"\n");
    let path = root.join("pyproject.toml");
    let mut changes = Changes::new(false);
    changes.note(&path, "merged template");
    let runner = RewritingRunner {
      content: "[project]\nname = \"x\"\n",
      exit_code: 0,
    };
    format_pyproject(&root, &runner, &mut changes).unwrap();
    assert_eq!(changes.files.len(), 1);
    assert_eq!(changes.files[0].details, vec!["merged template", "formatted with tombi"]);
  }

  #[test]
  fn missing_tombi_is_skipped_not_an_error() {
    let (_d, root) = project("[project]\n");
    let mut changes = Changes::new(false);
    let out = format_pyproject(&root, &MissingRunner, &mut changes).unwrap();
    assert_eq!(out, Outcome::Unavailable);
    assert!(changes.is_empty());
  }

  #[test]
  fn nonzero_exit_is_reported_and_file_not_recorded() {
    let (_d, root) = project("[project]\n");
    let runner = RewritingRunner {
      content: "[project]\n",
      exit_code: 1,
    };
    let mut changes = Changes::new(false);
    let out = format_pyproject(&root, &runner, &mut changes).unwrap();
    assert_eq!(out, Outcome::Failed { code: Some(1) });
    assert!(changes.is_empty());
  }

  #[test]
  fn real_tombi_sorts_and_formats_when_installed() {
    let (_d, root) = project("[project]\nname=\"x\"\nversion=\"1\"\n");
    let mut changes = Changes::new(false);
    match format_pyproject(&root, &SystemRunner, &mut changes).unwrap() {
      Outcome::Unavailable => eprintln!("tombi not installed; skipping"),
      Outcome::Formatted(true) => {
        let s = std::fs::read_to_string(root.join("pyproject.toml")).unwrap();
        assert!(s.contains("name = \"x\""), "{s}");
        assert_eq!(changes.files.len(), 1);
      }
      other => panic!("unexpected: {other:?}"),
    }
  }
}
