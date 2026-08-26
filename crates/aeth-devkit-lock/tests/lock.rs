//! `devkit lock` end to end against a temp git repo, with the index and uv stubbed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use aeth_devkit_core::git;
use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_lock::{Args, COMMIT_SUBJECT, run};

const PYPROJECT: &str = r#"[project]
  name = "demo"
  dependencies = ["requests>=2"]

[dependency-groups]
  dev = [
    "aeth-devkit>=6.0.2",
  ]

[tool.uv.sources]
  aeth-devkit = { index = "Private" }

[[tool.uv.index]]
  name = "Private"
  url  = "https://pypi.example.com/+simple"
"#;

fn project(with_git: bool) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(dir.path().join("pyproject.toml"), PYPROJECT).unwrap();
  std::fs::write(dir.path().join("uv.lock"), "version = 1\n").unwrap();
  if with_git {
    git::init_test_repo(dir.path());
    git::commit_paths(dir.path(), &["pyproject.toml".into(), "uv.lock".into()], "init").unwrap();
  }
  dir
}

fn args(root: &Path) -> Args {
  Args {
    root: root.to_path_buf(),
    package: vec![],
    no_commit: false,
    dry_run: false,
    uv_args: vec![],
  }
}

fn read(root: &Path, rel: &str) -> String {
  std::fs::read_to_string(root.join(rel)).unwrap()
}

/// Canonical form without the Windows `\\?\` prefix, matching what `run` passes to uv.
fn canon(root: &Path) -> PathBuf {
  let c = root.canonicalize().unwrap();
  let s = c.to_string_lossy();
  match s.strip_prefix(r"\\?\") {
    Some(rest) => PathBuf::from(rest),
    None => c,
  }
}

fn last_subject(root: &Path) -> String {
  let out = std::process::Command::new("git")
    .current_dir(root)
    .args(["log", "-1", "--format=%s"])
    .output()
    .unwrap();
  String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn bumps_pin_syncs_and_commits() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["6.0.2".into(), "7.1.0".into(), "8.0.0a1".into()],
  };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.uv_args = vec!["--upgrade".into()];

  let code = run(&a, &index, &runner).unwrap();
  assert_eq!(code, ExitCode::SUCCESS);
  assert!(read(root, "pyproject.toml").contains("\"aeth-devkit>=7.1.0\","));

  let calls = runner.calls.borrow();
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0].program, "uv");
  assert_eq!(calls[0].args, vec!["sync", "--upgrade"]);
  assert_eq!(calls[0].cwd, canon(root));

  assert_eq!(last_subject(root), COMMIT_SUBJECT);
  assert!(!git::is_dirty(root, &["pyproject.toml", "uv.lock"]).unwrap());
}

#[test]
fn already_current_pin_and_clean_lock_commits_nothing() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["6.0.2".into()],
  };
  let runner = RecordingRunner::new(0);
  run(&args(root), &index, &runner).unwrap();
  assert_eq!(last_subject(root), "init");
  assert_eq!(runner.calls.borrow().len(), 1, "uv sync still runs");
}

#[test]
fn missing_pin_is_skipped_but_sync_runs() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["9.9.9".into()],
  };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.package = vec!["not-there".into()];
  run(&a, &index, &runner).unwrap();
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=6.0.2"));
  assert_eq!(runner.calls.borrow().len(), 1);
}

#[test]
fn uv_failure_propagates_exit_code_and_skips_commit() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["7.0.0".into()],
  };
  let runner = RecordingRunner::new(7);
  let code = run(&args(root), &index, &runner).unwrap();
  assert_eq!(code, ExitCode::from(7));
  assert_eq!(last_subject(root), "init");
}

#[test]
fn dry_run_changes_nothing() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["7.0.0".into()],
  };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.dry_run = true;
  run(&a, &index, &runner).unwrap();
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=6.0.2"));
  assert!(runner.calls.borrow().is_empty());
  assert_eq!(last_subject(root), "init");
}

#[test]
fn no_commit_leaves_changes_in_tree() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["7.0.0".into()],
  };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.no_commit = true;
  run(&a, &index, &runner).unwrap();
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=7.0.0"));
  assert_eq!(last_subject(root), "init");
  assert!(git::is_dirty(root, &["pyproject.toml"]).unwrap());
}

#[test]
fn not_a_git_repo_skips_commit() {
  let dir = project(false);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["7.0.0".into()],
  };
  let runner = RecordingRunner::new(0);
  let code = run(&args(root), &index, &runner).unwrap();
  assert_eq!(code, ExitCode::SUCCESS);
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=7.0.0"));
}

#[test]
fn no_stable_version_is_an_error() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient {
    versions: vec!["7.0.0a1".into()],
  };
  let runner = RecordingRunner::new(0);
  let err = run(&args(root), &index, &runner).unwrap_err().to_string();
  assert!(err.contains("No stable release versions found for aeth-devkit"), "{err}");
  assert!(runner.calls.borrow().is_empty());
}
