//! The package step (spec 4.0) against scratch projects, with the `uv` calls recorded
//! rather than run: the lock is pre-written as uv would have left it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_core::prompt::ScriptedPrompt;
use aeth_devkit_setup::docker::{Deps as DockerDeps, Mode};
use aeth_devkit_setup::packages::{self, Installed, RUNNING_DEVKIT, StubVenv};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("docker")
}

fn lock_with(container: &str) -> String {
  format!(
    "version = 1\n\n[[package]]\nname = \"aeth-devkit\"\nversion = \"{RUNNING_DEVKIT}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-container\"\nversion = \"{container}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n"
  )
}

fn project(pyproject: &str, lock: Option<&str>) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
  if let Some(l) = lock {
    fs::write(dir.path().join("uv.lock"), l).unwrap();
  }
  dir
}

const DOCKER_PYPROJECT: &str = "[project]\n  name = \"p\"\n  dependencies = [\"devkit-container\"]\n\n[tool.docker]\n  services = [\"p\"]\n\n[tool.uv.sources]\n  devkit-container = [{ index = \"SFTPyPI\" }]\n\n[[tool.uv.index]]\n  name = \"SFTPyPI\"\n  url = \"https://idx/+simple\"\n  explicit = true\n";

/// The venv with the fixture copy of the container package at `version`, or empty.
fn venv(version: Option<&str>) -> StubVenv {
  let mut map = HashMap::new();
  if let Some(v) = version {
    map.insert(
      "devkit_container".to_string(),
      Installed {
        dir: fixtures(),
        version: v.into(),
      },
    );
  }
  StubVenv(map)
}

fn advance(
  root: &Path,
  runner: &RecordingRunner,
  index: &StubIndexClient,
  venv: &StubVenv,
  latest: &[String],
  dry_run: bool,
) -> anyhow::Result<aeth_devkit_setup::changes::Changes> {
  let prompt = ScriptedPrompt::new(&[]);
  let deps = aeth_devkit_setup::Deps {
    docker: DockerDeps {
      runner,
      prompt: &prompt,
      reviewer: None,
      mode: Mode::Ask,
    },
    index,
    venv,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root)?;
  let mut changes = aeth_devkit_setup::changes::Changes::new(dry_run);
  packages::advance(&ctx, &deps, dry_run, latest, &mut changes)?;
  Ok(changes)
}

fn latest() -> Vec<String> {
  vec!["devkit-container".to_string()]
}

#[test]
fn a_latest_package_is_locked_under_the_devkit_constraint_and_its_floor_written() {
  // The lock is pre-written as uv leaves it after upgrading to 1.4.0; the bare
  // `devkit-container` requirement is what the merge writes for a `{latest}` entry.
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let root = dir.path();
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient {
    versions: vec!["1.0.0".into(), "1.4.0".into()],
  };
  let changes = advance(root, &runner, &index, &venv(Some("1.4.0")), &latest(), false).unwrap();
  let calls = runner.calls_for("uv");
  // The running devkit rides along as a pinned `--upgrade-package`: uv treats the specifier
  // as a hard constraint for this resolution, and `uv lock` has no constraints flag.
  assert_eq!(
    calls[0],
    [
      "lock",
      "--upgrade-package",
      "devkit-container",
      "--upgrade-package",
      &format!("aeth-devkit=={RUNNING_DEVKIT}"),
    ],
    "{calls:?}"
  );
  // The floor is a requirement the lock's metadata records, so writing it is followed by a
  // plain re-lock; the venv already holds 1.4.0 and the lock did not move, so no sync.
  assert_eq!(calls[1], ["lock"], "{calls:?}");
  assert_eq!(calls.len(), 2, "{calls:?}");
  let py = fs::read_to_string(root.join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-container>=1.4.0\""), "{py}");
  assert!(changes.warnings.is_empty(), "{:?}", changes.warnings);
  assert!(changes.files.iter().any(|f| f.path.ends_with("pyproject.toml")));
  assert!(changes.managed.iter().any(|p| p.ends_with("uv.lock")), "{:?}", changes.managed);
  assert!(!changes.venv_synced);
}

#[test]
fn a_floor_already_written_needs_no_relock() {
  let dir = project(
    &DOCKER_PYPROJECT.replace("\"devkit-container\"", "\"devkit-container>=1.4.0\""),
    Some(&lock_with("1.4.0")),
  );
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  advance(dir.path(), &runner, &index, &venv(Some("1.4.0")), &latest(), false).unwrap();
  assert_eq!(runner.calls_for("uv").len(), 1);
}

#[test]
fn a_compatible_release_clause_and_an_odd_spelling_are_handled() {
  // `~=` carries the user's ceiling: moving its version would narrow it, so it stays. A
  // bare requirement is bare however it is spelled.
  for (spec, expect, relocks) in [
    ("devkit-container~=1.2", "\"devkit-container~=1.2\"", 1),
    ("Devkit_Container", "\"devkit-container>=1.4.0\"", 2),
  ] {
    let dir = project(
      &DOCKER_PYPROJECT.replace("\"devkit-container\"", &format!("\"{spec}\"")),
      Some(&lock_with("1.4.0")),
    );
    let runner = RecordingRunner::new(0);
    let index = StubIndexClient { versions: vec![] };
    let changes = advance(dir.path(), &runner, &index, &venv(Some("1.4.0")), &latest(), false).unwrap();
    let py = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
    assert!(py.contains(expect), "{spec}: {py}");
    assert_eq!(runner.calls_for("uv").len(), relocks, "{spec}");
    assert_eq!(
      changes.notes.iter().any(|n| n.contains("left as written")),
      relocks == 1,
      "{spec}: {:?}",
      changes.notes
    );
  }
}

#[test]
fn a_package_locked_from_a_path_gets_no_floor() {
  let lock = lock_with("1.5.0.dev0").replace(
    "version = \"1.5.0.dev0\"\nsource = { registry = \"https://idx/+simple\" }",
    "version = \"1.5.0.dev0\"\nsource = { editable = \"../devkit-container\" }",
  );
  let dir = project(DOCKER_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, &venv(Some("1.5.0.dev0")), &latest(), false).unwrap();
  let py = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-container\"") && !py.contains(">="), "{py}");
  assert!(changes.notes.iter().any(|n| n.contains("left as written")), "{:?}", changes.notes);
  assert_eq!(runner.calls_for("uv").len(), 1, "no floor, no relock");
}

#[test]
fn a_dry_run_reports_a_lock_on_another_devkit_as_a_problem() {
  let lock = lock_with("1.4.0").replace(&format!("version = \"{RUNNING_DEVKIT}\""), "version = \"0.0.1\"");
  let dir = project(DOCKER_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, &venv(Some("1.4.0")), &latest(), true).unwrap();
  assert!(
    changes.problems.iter().any(|p| p.contains("0.0.1") && p.contains(RUNNING_DEVKIT)),
    "{:?}",
    changes.problems
  );
  assert!(runner.calls_for("uv").is_empty());
}

#[test]
fn a_throttled_latest_warns_when_the_index_is_ahead() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient {
    versions: vec!["1.4.0".into(), "2.0.0".into()],
  };
  let changes = advance(dir.path(), &runner, &index, &venv(Some("1.4.0")), &latest(), false).unwrap();
  assert!(
    changes.warnings.iter().any(|w| w.contains("2.0.0") && w.contains("devkit lock")),
    "{:?}",
    changes.warnings
  );
}

#[test]
fn an_unsatisfiable_floor_stops_with_the_remedy() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  runner.script_err(
    "uv",
    &["lock"],
    1,
    "  × No solution found when resolving dependencies:\n  ╰─▶ Because devkit-container>=2 depends on aeth-devkit>=12 ...",
  );
  let index = StubIndexClient { versions: vec![] };
  let err = advance(dir.path(), &runner, &index, &venv(None), &[], false)
    .unwrap_err()
    .to_string();
  assert!(err.contains("devkit lock") && err.contains(RUNNING_DEVKIT), "{err}");
  assert!(runner.calls_for("uv").iter().all(|c| c[0] != "sync"), "no sync after a failed lock");
}

#[test]
fn a_failed_refresh_is_an_error_before_adoption_and_a_warning_after() {
  let index = StubIndexClient { versions: vec![] };
  let failing = || {
    let runner = RecordingRunner::new(0);
    runner.script_err(
      "uv",
      &["lock"],
      2,
      "error: Failed to fetch: `https://idx/+simple/devkit-container/`",
    );
    runner
  };
  // Not locked yet: nothing to fall back on.
  let dir = project(DOCKER_PYPROJECT, None);
  let err = advance(dir.path(), &failing(), &index, &venv(None), &latest(), false)
    .unwrap_err()
    .to_string();
  assert!(err.contains("uv lock failed") && err.contains("Failed to fetch"), "{err}");
  // Locked and installed: the run goes on with 1.4.0 and says why it did not move.
  let floored = DOCKER_PYPROJECT.replace("\"devkit-container\"", "\"devkit-container>=1.4.0\"");
  let dir = project(&floored, Some(&lock_with("1.4.0")));
  let runner = failing();
  let changes = advance(dir.path(), &runner, &index, &venv(Some("1.4.0")), &latest(), false).unwrap();
  assert!(
    changes
      .warnings
      .iter()
      .any(|w| w.contains("uv lock failed") && w.contains("Failed to fetch")),
    "{:?}",
    changes.warnings
  );
  assert!(runner.calls_for("uv").iter().all(|c| c[0] != "sync"), "nothing to sync");
}

#[test]
fn a_lock_on_another_devkit_stops_before_locking() {
  let lock = lock_with("1.4.0").replace(&format!("version = \"{RUNNING_DEVKIT}\""), "version = \"0.0.1\"");
  let dir = project(DOCKER_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let err = advance(dir.path(), &runner, &index, &venv(None), &[], false)
    .unwrap_err()
    .to_string();
  assert!(err.contains("uv sync") && err.contains("0.0.1"), "{err}");
  assert!(runner.calls_for("uv").is_empty());
}

#[test]
fn a_dry_run_only_notes_a_missing_package() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, &venv(None), &latest(), true).unwrap();
  assert!(runner.calls_for("uv").is_empty(), "dry run runs nothing");
  assert!(
    changes
      .notes
      .iter()
      .any(|n| n.contains("devkit-container") && n.contains("plain run")),
    "{:?}",
    changes.notes
  );
}

#[test]
fn a_lagging_venv_is_synced_and_a_sync_that_changes_nothing_is_an_error() {
  // The recorded sync cannot install anything, so the re-check after it is what fails:
  // the message names the environment as the thing to look at.
  let index = StubIndexClient { versions: vec![] };
  for installed in [None, Some("1.3.0")] {
    let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
    let runner = RecordingRunner::new(0);
    let err = advance(dir.path(), &runner, &index, &venv(installed), &[], false)
      .unwrap_err()
      .to_string();
    let calls = runner.calls_for("uv");
    assert_eq!(
      calls.last().map(|c| c.as_slice()),
      Some(&["sync".to_string(), "--frozen".to_string()][..]),
      "{installed:?}: {calls:?}"
    );
    assert!(
      err.contains("devkit-container 1.4.0") && err.contains("UV_PROJECT_ENVIRONMENT"),
      "{err}"
    );
  }
}

#[test]
fn a_package_missing_from_the_lock_after_locking_is_an_error() {
  let dir = project(
    DOCKER_PYPROJECT,
    Some(&lock_with("1.4.0").replace("devkit-container", "something-else")),
  );
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let err = advance(dir.path(), &runner, &index, &venv(None), &[], false)
    .unwrap_err()
    .to_string();
  assert!(
    err.contains("not in uv.lock after locking") && err.contains("devkit-container"),
    "{err}"
  );
}

#[test]
fn nothing_happens_without_docker() {
  let dir = project("[project]\n  name = \"p\"\n", None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, &venv(None), &[], false).unwrap();
  assert!(runner.calls_for("uv").is_empty() && changes.notes.is_empty());
}
