//! The package step (spec 4.0) against scratch projects, with the `uv` calls recorded
//! rather than run: the lock is pre-written as uv would have left it.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_core::prompt::ScriptedPrompt;
use aeth_devkit_setup::docker::{Deps as DockerDeps, Mode};
use aeth_devkit_setup::packages::{self, RUNNING_DEVKIT, StubPackageDirs};

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

fn advance(
  root: &Path,
  runner: &RecordingRunner,
  index: &StubIndexClient,
  installed: bool,
  dry_run: bool,
) -> aeth_devkit_setup::changes::Changes {
  let prompt = ScriptedPrompt::new(&[]);
  let mut map = HashMap::new();
  if installed {
    map.insert("devkit_container".to_string(), fixtures());
  }
  let dirs = StubPackageDirs(map);
  let deps = aeth_devkit_setup::Deps {
    docker: DockerDeps {
      runner,
      prompt: &prompt,
      reviewer: None,
      mode: Mode::Ask,
    },
    index,
    packages: &dirs,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
  let mut changes = aeth_devkit_setup::changes::Changes::new(dry_run);
  packages::advance(&ctx, &deps, dry_run, &["devkit-container".to_string()], &mut changes).unwrap();
  changes
}

/// `advance` with no `{latest}` names and nothing installed, for the error paths.
fn advance_err(root: &Path, runner: &RecordingRunner) -> String {
  let prompt = ScriptedPrompt::new(&[]);
  let index = StubIndexClient { versions: vec![] };
  let dirs = StubPackageDirs::default();
  let deps = aeth_devkit_setup::Deps {
    docker: DockerDeps {
      runner,
      prompt: &prompt,
      reviewer: None,
      mode: Mode::Ask,
    },
    index: &index,
    packages: &dirs,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
  let mut changes = aeth_devkit_setup::changes::Changes::new(false);
  packages::advance(&ctx, &deps, false, &[], &mut changes).unwrap_err().to_string()
}

#[test]
fn a_latest_package_is_locked_under_the_devkit_constraint_and_its_floor_written() {
  // The recording runner does not rewrite files, so the lock is pre-written as uv would
  // have left it after upgrading to 1.4.0; the bare `devkit-container` requirement in the
  // pyproject is what the merge writes for a `{latest}` entry.
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let root = dir.path();
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient {
    versions: vec!["1.0.0".into(), "1.4.0".into()],
  };
  let changes = advance(root, &runner, &index, true, false);
  let lock_call = &runner.calls_for("uv")[0];
  assert_eq!(&lock_call[..3], &["lock", "--upgrade-package", "devkit-container"], "{lock_call:?}");
  let constraints = lock_call
    .iter()
    .position(|a| a == "--constraints")
    .map(|i| PathBuf::from(&lock_call[i + 1]))
    .expect("a constraints file");
  assert_eq!(constraints, root.join(".cache").join("devkit-constraints.txt"));
  assert_eq!(
    fs::read_to_string(&constraints).unwrap().trim(),
    format!("aeth-devkit=={RUNNING_DEVKIT}")
  );
  let py = fs::read_to_string(root.join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-container>=1.4.0\""), "{py}");
  assert!(changes.warnings.is_empty(), "{:?}", changes.warnings);
  assert!(changes.files.iter().any(|f| f.path.ends_with("pyproject.toml")));
}

#[test]
fn a_throttled_latest_warns_when_the_index_is_ahead() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient {
    versions: vec!["1.4.0".into(), "2.0.0".into()],
  };
  let changes = advance(dir.path(), &runner, &index, true, false);
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
  let err = advance_err(dir.path(), &runner);
  assert!(err.contains("devkit lock") && err.contains(RUNNING_DEVKIT), "{err}");
  assert!(runner.calls_for("uv").iter().all(|c| c[0] != "sync"), "no sync after a failed lock");
}

#[test]
fn a_lock_on_another_devkit_stops_before_locking() {
  let lock = lock_with("1.4.0").replace(&format!("version = \"{RUNNING_DEVKIT}\""), "version = \"0.0.1\"");
  let dir = project(DOCKER_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let err = advance_err(dir.path(), &runner);
  assert!(err.contains("uv sync") && err.contains("0.0.1"), "{err}");
  assert!(runner.calls_for("uv").is_empty());
}

#[test]
fn a_missing_package_is_synced_and_a_dry_run_only_notes() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient {
    versions: vec!["1.4.0".into()],
  };
  let changes = advance(dir.path(), &runner, &index, false, true);
  assert!(runner.calls_for("uv").is_empty(), "dry run runs nothing");
  assert!(
    changes
      .notes
      .iter()
      .any(|n| n.contains("devkit-container") && n.contains("plain run")),
    "{:?}",
    changes.notes
  );
  let runner = RecordingRunner::new(0);
  advance(dir.path(), &runner, &index, false, false);
  let calls = runner.calls_for("uv");
  assert_eq!(
    calls.last().map(|c| c.as_slice()),
    Some(&["sync".to_string(), "--frozen".to_string()][..]),
    "{calls:?}"
  );
}

#[test]
fn nothing_happens_without_docker() {
  let dir = project("[project]\n  name = \"p\"\n", None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, false, false);
  assert!(runner.calls_for("uv").is_empty() && changes.notes.is_empty());
}
