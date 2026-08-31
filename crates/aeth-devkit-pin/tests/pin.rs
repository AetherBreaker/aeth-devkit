//! End-to-end tests for `devkit docker-pin` against real temp repositories, with the
//! network side (gh, remote git) scripted through `RecordingRunner` and `StubIndexClient`.

use std::path::{Path, PathBuf};
use std::process::Command;

use aeth_devkit_core::git::init_test_repo;
use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_pin::{Args, Deps, run};

const PYPROJECT: &str = "[project]\nname = \"my-package\"\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/internal/\"\n";
const COMPOSE: &str = "services:\n  app:\n    build:\n      args:\n        PACKAGE_NAME: my_package\n        PACKAGE_VERSION: 1.0.0\n";

fn git(root: &Path, args: &[&str]) {
  assert!(
    Command::new("git").current_dir(root).args(args).status().unwrap().success(),
    "git {args:?}"
  );
}

/// A repo with pyproject + committed compose file; origin points at GitHub so the
/// completeness preflight includes tags and the release.
fn fixture() -> (tempfile::TempDir, PathBuf) {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path().to_path_buf();
  init_test_repo(&root);
  std::fs::write(root.join("pyproject.toml"), PYPROJECT).unwrap();
  std::fs::write(root.join("compose.yaml"), COMPOSE).unwrap();
  git(&root, &["add", "."]);
  git(&root, &["commit", "-q", "-m", "init"]);
  git(&root, &["remote", "add", "origin", "https://github.com/o/r.git"]);
  (dir, root)
}

fn args(root: &Path) -> Args {
  Args {
    root: root.to_path_buf(),
    version: None,
    dry_run: false,
    no_commit: false,
    no_push: false,
    compose_file: None,
  }
}

/// A runner scripted for a complete 2.0.0 release and quiet remote git.
fn happy_runner() -> RecordingRunner {
  let r = RecordingRunner::new(0);
  r.script("gh", &["api"], 0, "v2.0.0\nv1.0.0\n");
  r.script("gh", &["release", "view"], 0, "url\n");
  r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
  r.script("git", &["rev-list", "--count"], 0, "0\n");
  r
}

fn pushed(r: &RecordingRunner) -> bool {
  r.calls_for("git").iter().any(|c| c.first().is_some_and(|a| a == "push"))
}

#[test]
fn pins_latest_commits_and_pushes() {
  let (_d, root) = fixture();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["1.0.0".into(), "2.0.0".into()],
  };
  run(&args(&root), &Deps { runner: &r, index: &idx }).unwrap();
  let text = std::fs::read_to_string(root.join("compose.yaml")).unwrap();
  assert!(text.contains("PACKAGE_VERSION: 2.0.0"), "{text}");
  // Committed (tree clean) and pushed.
  let out = Command::new("git")
    .current_dir(&root)
    .args(["status", "--porcelain"])
    .output()
    .unwrap();
  assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");
  let msg = Command::new("git")
    .current_dir(&root)
    .args(["log", "-1", "--format=%s"])
    .output()
    .unwrap();
  assert_eq!(String::from_utf8_lossy(&msg.stdout).trim(), "chore: pin my-package to 2.0.0");
  assert!(pushed(&r));
}

#[test]
fn already_pinned_is_a_quiet_noop() {
  let (_d, root) = fixture();
  let r = happy_runner();
  r.script("gh", &["api"], 0, "v1.0.0\n");
  let idx = StubIndexClient {
    versions: vec!["1.0.0".into()],
  };
  run(&args(&root), &Deps { runner: &r, index: &idx }).unwrap();
  assert!(!pushed(&r));
  assert_eq!(std::fs::read_to_string(root.join("compose.yaml")).unwrap(), COMPOSE);
}

#[test]
fn dry_run_touches_nothing() {
  let (_d, root) = fixture();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.dry_run = true;
  run(&a, &Deps { runner: &r, index: &idx }).unwrap();
  assert_eq!(std::fs::read_to_string(root.join("compose.yaml")).unwrap(), COMPOSE);
  assert!(!pushed(&r));
}

#[test]
fn behind_origin_fails_before_editing() {
  let (_d, root) = fixture();
  let r = happy_runner();
  r.script("git", &["rev-list", "--count"], 0, "2\n");
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let err = run(&args(&root), &Deps { runner: &r, index: &idx }).unwrap_err().to_string();
  assert!(err.contains("behind origin"), "{err}");
  assert_eq!(std::fs::read_to_string(root.join("compose.yaml")).unwrap(), COMPOSE);
}

#[test]
fn dirty_compose_commits_pin_on_head_and_keeps_user_edits() {
  let (_d, root) = fixture();
  // The user added an unrelated line, far enough from the pin line to merge cleanly.
  let dirty = format!("# EXTRA_ARG note from the user\n{COMPOSE}");
  std::fs::write(root.join("compose.yaml"), &dirty).unwrap();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  run(&args(&root), &Deps { runner: &r, index: &idx }).unwrap();
  // HEAD has the pin but not the user's edit; the worktree has both.
  let head = Command::new("git")
    .current_dir(&root)
    .args(["show", "HEAD:compose.yaml"])
    .output()
    .unwrap();
  let head = String::from_utf8_lossy(&head.stdout);
  assert!(head.contains("PACKAGE_VERSION: 2.0.0") && !head.contains("EXTRA_ARG"), "{head}");
  let tree = std::fs::read_to_string(root.join("compose.yaml")).unwrap();
  assert!(tree.contains("PACKAGE_VERSION: 2.0.0") && tree.contains("EXTRA_ARG"), "{tree}");
}

#[test]
fn conflicting_dirty_edit_aborts_before_committing() {
  let (_d, root) = fixture();
  // The user edited the very line the pin wants to change.
  std::fs::write(root.join("compose.yaml"), COMPOSE.replace("1.0.0", "9.9.9")).unwrap();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let err = run(&args(&root), &Deps { runner: &r, index: &idx }).unwrap_err().to_string();
  assert!(err.contains("overlap"), "{err}");
  // Nothing was committed.
  let log = Command::new("git").current_dir(&root).args(["log", "--oneline"]).output().unwrap();
  assert_eq!(String::from_utf8_lossy(&log.stdout).lines().count(), 1);
}

#[test]
fn no_commit_edits_worktree_only() {
  let (_d, root) = fixture();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_commit = true;
  run(&a, &Deps { runner: &r, index: &idx }).unwrap();
  assert!(std::fs::read_to_string(root.join("compose.yaml")).unwrap().contains("2.0.0"));
  let log = Command::new("git").current_dir(&root).args(["log", "--oneline"]).output().unwrap();
  assert_eq!(String::from_utf8_lossy(&log.stdout).lines().count(), 1, "no new commit");
  assert!(!pushed(&r));
}

#[test]
fn no_push_commits_locally_only() {
  let (_d, root) = fixture();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &Deps { runner: &r, index: &idx }).unwrap();
  let log = Command::new("git").current_dir(&root).args(["log", "--oneline"]).output().unwrap();
  assert_eq!(String::from_utf8_lossy(&log.stdout).lines().count(), 2, "pin commit exists");
  assert!(!pushed(&r));
}

#[test]
fn explicit_version_flows_through() {
  let (_d, root) = fixture();
  let r = happy_runner();
  r.script("gh", &["api"], 0, "v2.0.0\nv1.0.0\nv1.5.0\n");
  let idx = StubIndexClient {
    versions: vec!["1.0.0".into(), "1.5.0".into(), "2.0.0".into()],
  };
  let mut a = args(&root);
  a.version = Some("1.5.0".into());
  run(&a, &Deps { runner: &r, index: &idx }).unwrap();
  assert!(
    std::fs::read_to_string(root.join("compose.yaml"))
      .unwrap()
      .contains("PACKAGE_VERSION: 1.5.0")
  );
}
