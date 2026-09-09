//! End-to-end tests for `devkit docker-pin` against real temp repositories, with the
//! network side (gh, remote git) scripted through `RecordingRunner` and `StubIndexClient`.

use std::path::{Path, PathBuf};
use std::process::Command;

use aeth_devkit_core::git::init_test_repo;
use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_pin::{Args, Deps, run};
use aeth_devkit_setup::packages::{Installed, StubVenv};

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

fn deps<'a>(runner: &'a RecordingRunner, index: &'a StubIndexClient, venv: &'a StubVenv) -> Deps<'a> {
  Deps { runner, index, venv }
}

/// No container package anywhere: the tests without Docker services never ask.
static NO_VENV: std::sync::LazyLock<StubVenv> = std::sync::LazyLock::new(StubVenv::default);

#[test]
fn pins_latest_commits_and_pushes() {
  let (_d, root) = fixture();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["1.0.0".into(), "2.0.0".into()],
  };
  run(&args(&root), &deps(&r, &idx, &NO_VENV)).unwrap();
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
  run(&args(&root), &deps(&r, &idx, &NO_VENV)).unwrap();
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
  run(&a, &deps(&r, &idx, &NO_VENV)).unwrap();
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
  let err = run(&args(&root), &deps(&r, &idx, &NO_VENV)).unwrap_err().to_string();
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
  run(&args(&root), &deps(&r, &idx, &NO_VENV)).unwrap();
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
fn dirty_crlf_checkout_merges_cleanly_and_stays_crlf() {
  let (_d, root) = fixture();
  // A Windows checkout (`core.autocrlf=true`): CRLF on disk, LF in HEAD. With an unrelated
  // user edit on top, the merge-back must compare in repository form, or the pinned line
  // reads as an overlapping edit.
  git(&root, &["config", "core.autocrlf", "true"]);
  let dirty = format!("# EXTRA_ARG note from the user\n{COMPOSE}").replace('\n', "\r\n");
  std::fs::write(root.join("compose.yaml"), &dirty).unwrap();
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  run(&args(&root), &deps(&r, &idx, &NO_VENV)).unwrap();
  let head = Command::new("git")
    .current_dir(&root)
    .args(["show", "HEAD:compose.yaml"])
    .output()
    .unwrap();
  let head = String::from_utf8_lossy(&head.stdout);
  assert!(
    head.contains("PACKAGE_VERSION: 2.0.0") && !head.contains("EXTRA_ARG") && !head.contains('\r'),
    "{head:?}"
  );
  let tree = std::fs::read_to_string(root.join("compose.yaml")).unwrap();
  assert!(
    tree.contains("PACKAGE_VERSION: 2.0.0\r\n") && tree.starts_with("# EXTRA_ARG"),
    "{tree:?}"
  );
  assert!(!tree.replace("\r\n", "").contains('\n'), "mixed endings: {tree:?}");
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
  let err = run(&args(&root), &deps(&r, &idx, &NO_VENV)).unwrap_err().to_string();
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
  run(&a, &deps(&r, &idx, &NO_VENV)).unwrap();
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
  run(&a, &deps(&r, &idx, &NO_VENV)).unwrap();
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
  run(&a, &deps(&r, &idx, &NO_VENV)).unwrap();
  assert!(
    std::fs::read_to_string(root.join("compose.yaml"))
      .unwrap()
      .contains("PACKAGE_VERSION: 1.5.0")
  );
}

const DOCKER_PYPROJECT: &str = "[project]\nname = \"my-package\"\n\n[tool.docker]\nservices = [\"app\"]\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/internal/\"\n";

/// `fixture()` made a Docker project: `services` set, a committed Dockerfile, and a lock
/// naming devkit-container when `locked` is given.
fn docker_fixture(dockerfile: &str, locked: Option<&str>) -> (tempfile::TempDir, PathBuf) {
  let (dir, root) = fixture();
  std::fs::write(root.join("pyproject.toml"), DOCKER_PYPROJECT).unwrap();
  std::fs::create_dir_all(root.join("docker")).unwrap();
  std::fs::write(root.join("docker/Dockerfile"), dockerfile).unwrap();
  if let Some(v) = locked {
    std::fs::write(
      root.join("uv.lock"),
      format!(
        "version = 1\n\n[[package]]\nname = \"devkit-container\"\nversion = \"{v}\"\nsource = {{ registry = \"https://x/+simple\" }}\n"
      ),
    )
    .unwrap();
  }
  git(&root, &["add", "."]);
  git(&root, &["commit", "-q", "-m", "docker"]);
  (dir, root)
}

/// A venv holding devkit_container at `version` with `template` as its Dockerfile.
fn installed(version: &str, template: &str) -> (tempfile::TempDir, StubVenv) {
  let site = tempfile::tempdir().unwrap();
  let pkg = site.path().join("devkit_container");
  std::fs::create_dir_all(&pkg).unwrap();
  std::fs::write(pkg.join("template.Dockerfile"), template).unwrap();
  let mut map = std::collections::HashMap::new();
  map.insert(
    "devkit_container".to_string(),
    Installed {
      dir: pkg,
      version: version.into(),
    },
  );
  (site, StubVenv(map))
}

fn subjects(root: &Path) -> Vec<String> {
  let out = Command::new("git").current_dir(root).args(["log", "--format=%s"]).output().unwrap();
  String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

#[test]
fn a_drifted_dockerfile_is_replaced_and_committed_before_the_pin() {
  let (_d, root) = docker_fixture("FROM old\n", Some("1.4.0"));
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &deps(&r, &idx, &venv)).unwrap();
  assert_eq!(std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(), "FROM new src\n");
  let log = subjects(&root);
  assert_eq!(log[0], "chore: pin my-package to 2.0.0", "{log:?}");
  assert_eq!(log[1], "chore(docker): refresh Dockerfile from devkit-container 1.4.0", "{log:?}");
  assert!(r.calls_for("uv").is_empty(), "installed 1.4.0 matches the lock: no sync");
}

#[test]
fn a_stale_venv_is_synced_before_the_dockerfile_is_compared() {
  let (_d, root) = docker_fixture("FROM new src\n", Some("1.4.0"));
  let (_site, venv) = installed("1.3.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_push = true;
  // The recorded sync installs nothing, so the venv still lags afterwards: the run stops
  // rather than render 1.3.0's template under 1.4.0's name.
  let err = run(&a, &deps(&r, &idx, &venv)).unwrap_err().to_string();
  assert_eq!(r.calls_for("uv")[0], vec!["sync", "--frozen"]);
  assert!(err.contains("1.4.0") && err.contains("uv sync"), "{err}");
  assert!(!subjects(&root).iter().any(|s| s.contains("refresh Dockerfile")), "{err}");
}

#[test]
fn without_the_package_in_the_lock_the_refresh_is_skipped() {
  let (_d, root) = docker_fixture("FROM old\n", None);
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &deps(&r, &idx, &venv)).unwrap();
  assert_eq!(std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(), "FROM old\n");
  assert!(r.calls_for("uv").is_empty());
  assert_eq!(subjects(&root)[0], "chore: pin my-package to 2.0.0");
}

#[test]
fn an_already_pinned_project_still_gets_the_refresh_committed_and_pushed() {
  let (_d, root) = docker_fixture("FROM old\n", Some("1.4.0"));
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  r.script("gh", &["api"], 0, "v1.0.0\n");
  let idx = StubIndexClient {
    versions: vec!["1.0.0".into()],
  };
  run(&args(&root), &deps(&r, &idx, &venv)).unwrap();
  let log = subjects(&root);
  assert_eq!(log[0], "chore(docker): refresh Dockerfile from devkit-container 1.4.0", "{log:?}");
  assert!(!log.iter().any(|s| s.starts_with("chore: pin")), "{log:?}");
  assert!(pushed(&r), "the refresh commit must reach origin even with nothing to pin");
}

#[test]
fn no_commit_keeps_an_uncommitted_dockerfile_edit_on_top_of_the_refresh() {
  let (_d, root) = docker_fixture("FROM old\nRUN keep-me\n", Some("1.4.0"));
  // A user edit at the end, uncommitted; the refresh changes the first line only.
  std::fs::write(root.join("docker/Dockerfile"), "FROM old\nRUN keep-me\nRUN user-edit\n").unwrap();
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\nRUN keep-me\n");
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_commit = true;
  run(&a, &deps(&r, &idx, &venv)).unwrap();
  assert_eq!(
    std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(),
    "FROM new src\nRUN keep-me\nRUN user-edit\n"
  );
  assert_eq!(subjects(&root)[0], "docker", "nothing committed");
}

#[test]
fn an_overlapping_compose_edit_aborts_before_the_dockerfile_refresh_is_committed() {
  let (_d, root) = docker_fixture("FROM old\n", Some("1.4.0"));
  // The user edited the very line the pin wants to change, while the Dockerfile has drifted.
  std::fs::write(root.join("compose.yaml"), COMPOSE.replace("1.0.0", "9.9.9")).unwrap();
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let err = run(&args(&root), &deps(&r, &idx, &venv)).unwrap_err().to_string();
  assert!(err.contains("overlap"), "{err}");
  // Neither commit was made and the Dockerfile was not touched: the two land together or not at all.
  assert_eq!(subjects(&root), ["docker", "init"]);
  assert_eq!(std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(), "FROM old\n");
  assert!(!pushed(&r));
}

#[test]
fn an_untracked_dockerfile_is_refused_unless_it_already_matches() {
  // No base to merge against: a never-committed Dockerfile with content of its own is
  // refused, one that already equals the render is simply not drift.
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\n");
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  for (content, ok) in [("FROM mine\n", false), ("FROM new src\n", true)] {
    let (_d, root) = docker_fixture("FROM old\n", Some("1.4.0"));
    git(&root, &["rm", "-q", "--cached", "docker/Dockerfile"]);
    git(&root, &["commit", "-q", "-m", "untrack"]);
    std::fs::write(root.join("docker/Dockerfile"), content).unwrap();
    let r = happy_runner();
    let mut a = args(&root);
    a.no_push = true;
    let result = run(&a, &deps(&r, &idx, &venv));
    assert_eq!(result.is_ok(), ok, "{content:?}: {result:?}");
    if !ok {
      let err = result.unwrap_err().to_string();
      assert!(err.contains("not committed yet"), "{err}");
    }
    assert_eq!(std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(), content);
    assert!(!subjects(&root).iter().any(|s| s.contains("refresh")), "{:?}", subjects(&root));
  }
}

#[test]
fn a_project_without_services_is_pinned_without_the_setup_context() {
  // Two publish indexes is a configuration setup-project refuses; the pin resolves across
  // both and never consulted that context before the Dockerfile refresh existed.
  let (_d, root) = fixture();
  let two =
    format!("{PYPROJECT}\n[[tool.uv.index]]\nname = \"Mirror\"\nurl = \"https://y/+simple\"\npublish-url = \"https://y/internal/\"\n");
  std::fs::write(root.join("pyproject.toml"), two).unwrap();
  git(&root, &["commit", "-q", "-am", "two indexes"]);
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &deps(&r, &idx, &NO_VENV)).unwrap();
  assert_eq!(subjects(&root)[0], "chore: pin my-package to 2.0.0");
}

#[test]
fn a_crlf_dockerfile_is_refreshed_in_its_own_endings() {
  let (_d, root) = docker_fixture("FROM old\r\nRUN keep-me\r\n", Some("1.4.0"));
  let (_site, venv) = installed("1.4.0", "FROM new {python_dir}\nRUN keep-me\n");
  let r = happy_runner();
  let idx = StubIndexClient {
    versions: vec!["2.0.0".into()],
  };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &deps(&r, &idx, &venv)).unwrap();
  assert_eq!(
    std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(),
    "FROM new src\r\nRUN keep-me\r\n"
  );
}
