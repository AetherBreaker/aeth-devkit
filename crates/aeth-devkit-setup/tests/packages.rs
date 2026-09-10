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

fn fixtures_root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn fixtures() -> PathBuf {
  fixtures_root().join("docker")
}

fn lock_with(container: &str) -> String {
  format!(
    "version = 1\n\n[[package]]\nname = \"aeth-devkit\"\nversion = \"{RUNNING_DEVKIT}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-claude-hooks\"\nversion = \"1.0.0\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-poe-complete\"\nversion = \"1.0.0\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-container\"\nversion = \"{container}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n"
  )
}

/// `lock_with` plus the templates package, as the bootstrap's recorded lock leaves it.
fn lock_with_templates(templates: &str) -> String {
  format!(
    "{}\n[[package]]\nname = \"devkit-templates\"\nversion = \"{templates}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n",
    lock_with("1.4.0")
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

const DOCKER_PYPROJECT: &str = "[project]\n  name = \"p\"\n  dependencies = [\"devkit-container\"]\n\n[tool.docker]\n  services = [\"p\"]\n\n[dependency-groups]\n  dev = [\"devkit-claude-hooks>=1.0.0\", \"devkit-poe-complete>=1.0.0\"]\n\n[tool.uv.sources]\n  devkit-container = [{ index = \"SFTPyPI\" }]\n  devkit-claude-hooks = [{ index = \"SFTPyPI\" }]\n  devkit-poe-complete = [{ index = \"SFTPyPI\" }]\n\n[[tool.uv.index]]\n  name = \"SFTPyPI\"\n  url = \"https://idx/+simple\"\n  explicit = true\n";

/// The venv with the fixture copy of the container package at `version`, or empty.
fn venv(version: Option<&str>) -> StubVenv {
  let mut map = HashMap::new();
  for name in ["devkit_claude_hooks", "devkit_poe_complete"] {
    map.insert(
      name.to_string(),
      Installed {
        dir: fixtures(),
        version: "1.0.0".into(),
      },
    );
  }
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
  packages::advance(&ctx, &deps, dry_run, &packages::active(&ctx), latest, &mut changes)?;
  Ok(changes)
}

/// `venv(None)` plus the templates package at `version`, its files being the snapshot under
/// `tests/fixtures/templates` (the package directory is `tests/fixtures`).
fn venv_with_templates(version: &str) -> StubVenv {
  let mut v = venv(None);
  v.0.insert(
    "devkit_templates".to_string(),
    Installed {
      dir: fixtures_root(),
      version: version.into(),
    },
  );
  v
}

fn ensure(
  root: &Path,
  runner: &RecordingRunner,
  index: &StubIndexClient,
  venv: &StubVenv,
  dry_run: bool,
) -> anyhow::Result<(PathBuf, aeth_devkit_setup::changes::Changes)> {
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
  let dir = packages::ensure_templates(&ctx, &deps, dry_run, &mut changes)?;
  Ok((dir, changes))
}

fn latest() -> Vec<String> {
  vec![
    "devkit-claude-hooks".into(),
    "devkit-poe-complete".into(),
    "devkit-container".into(),
  ]
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
      "devkit-claude-hooks",
      "--upgrade-package",
      "devkit-poe-complete",
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
  // The stub index answers 1.4.0 for every package, so the two packages locked at 1.0.0 get
  // the throttle warning; the subject of this test must not.
  assert!(
    changes.warnings.iter().all(|w| !w.contains("devkit-container")),
    "{:?}",
    changes.warnings
  );
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
  assert!(py.contains("\"devkit-container\"") && !py.contains("devkit-container>="), "{py}");
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

const PLAIN_PYPROJECT: &str = "[project]\n  name = \"p\"\n\n[dependency-groups]\n  dev = [\"devkit-claude-hooks\", \"devkit-poe-complete\"]\n\n[tool.uv.sources]\n  devkit-claude-hooks = [{ index = \"SFTPyPI\" }]\n  devkit-poe-complete = [{ index = \"SFTPyPI\" }]\n\n[[tool.uv.index]]\n  name = \"SFTPyPI\"\n  url = \"https://idx/+simple\"\n  explicit = true\n";

#[test]
fn a_project_without_docker_still_gets_the_hooks_and_completion() {
  // The venv already holds both packages at the locked version, so the step locks under the
  // devkit constraint, writes the floors, re-locks and has nothing to sync; an empty venv
  // would end in the sync re-check that
  // `a_lagging_venv_is_synced_and_a_sync_that_changes_nothing_is_an_error` covers.
  let lock = lock_with("1.4.0").replace("devkit-container", "unrelated");
  let dir = project(PLAIN_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(dir.path()).unwrap();
  // The bootstrap's package is not the merge's: `active` never lists it.
  assert!(packages::active(&ctx).iter().all(|p| p.name != packages::TEMPLATES.name));
  let changes = advance(dir.path(), &runner, &index, &venv(None), &latest(), false).unwrap();
  let calls = runner.calls_for("uv");
  assert_eq!(
    calls,
    vec![
      vec![
        "lock".to_string(),
        "--upgrade-package".into(),
        "devkit-claude-hooks".into(),
        "--upgrade-package".into(),
        "devkit-poe-complete".into(),
        "--upgrade-package".into(),
        format!("aeth-devkit=={RUNNING_DEVKIT}"),
      ],
      vec!["lock".to_string()],
    ],
    "the upgrade lock, the re-lock after the floors, no sync: the venv already matches"
  );
  let py = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
  assert!(
    py.contains("\"devkit-claude-hooks>=1.0.0\"") && py.contains("\"devkit-poe-complete>=1.0.0\""),
    "{py}"
  );
  assert!(changes.warnings.is_empty(), "{:?}", changes.warnings);
}

#[test]
fn the_bootstrap_adds_the_bare_requirement_and_source_and_returns_the_venv_templates_dir() {
  let dir = project(PLAIN_PYPROJECT, Some(&lock_with_templates("1.0.0")));
  let root = dir.path();
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient {
    versions: vec!["1.0.0".into()],
  };
  let (out, changes) = ensure(root, &runner, &index, &venv_with_templates("1.0.0"), false).unwrap();
  assert_eq!(out, fixtures_root().join("templates"));
  let py = fs::read_to_string(root.join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-templates>=1.0.0\""), "{py}");
  assert!(py.contains("devkit-templates = [{ index = \"SFTPyPI\" }]"), "{py}");
  let calls = runner.calls_for("uv");
  assert_eq!(
    calls[0],
    vec![
      "lock".to_string(),
      "--upgrade-package".into(),
      "devkit-templates".into(),
      "--upgrade-package".into(),
      format!("aeth-devkit=={RUNNING_DEVKIT}"),
    ],
    "{calls:?}"
  );
  assert_eq!(calls[1], vec!["lock".to_string()], "the re-lock after the floor");
  assert!(calls.iter().all(|c| c[0] != "sync"), "the stub venv already holds 1.0.0");
  let pyproject_entries: Vec<_> = changes.files.iter().filter(|f| f.path.ends_with("pyproject.toml")).collect();
  assert_eq!(pyproject_entries.len(), 1, "{changes:?}");
  let details = &pyproject_entries[0].details;
  assert!(details.iter().any(|d| d.contains("added \"devkit-templates\"")), "{details:?}");
  assert!(details.iter().any(|d| d.contains("pinned devkit-templates>=1.0.0")), "{details:?}");
}

#[test]
fn a_dry_run_without_the_templates_package_is_an_error_naming_the_remedy() {
  let dir = project(PLAIN_PYPROJECT, None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let err = ensure(dir.path(), &runner, &index, &venv(None), true).unwrap_err().to_string();
  assert!(err.contains("a plain run adds it"), "{err}");
  assert!(runner.calls_for("uv").is_empty());
  assert!(
    !fs::read_to_string(dir.path().join("pyproject.toml"))
      .unwrap()
      .contains("devkit-templates")
  );
  // Listed but not installed: the same refusal (a dry run never syncs).
  let listed = PLAIN_PYPROJECT.replace("\"devkit-poe-complete\"]", "\"devkit-poe-complete\", \"devkit-templates>=1.0.0\"]");
  let dir = project(&listed, Some(&lock_with_templates("1.0.0")));
  let err = ensure(dir.path(), &runner, &index, &venv(None), true).unwrap_err().to_string();
  assert!(err.contains("a plain run adds it"), "{err}");
  // Listed and installed: a dry run passes through and reads the venv.
  let (out, changes) = ensure(dir.path(), &runner, &index, &venv_with_templates("1.0.0"), true).unwrap();
  assert_eq!(out, fixtures_root().join("templates"));
  assert!(changes.files.is_empty() && runner.calls_for("uv").is_empty(), "{changes:?}");
}

#[test]
fn the_templates_repository_never_bootstraps_itself() {
  let dir = project(&PLAIN_PYPROJECT.replace("name = \"p\"", "name = \"devkit-templates\""), None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let err = ensure(dir.path(), &runner, &index, &venv(None), false).unwrap_err().to_string();
  assert!(err.contains("templates-dir"), "{err}");
  assert!(runner.calls_for("uv").is_empty());
}

#[test]
fn the_bootstrap_creates_the_tables_it_needs_without_empty_headers() {
  let dir = project(
    "[project]\n  name = \"p\"\n\n[tool.ruff]\n  fix = true\n",
    Some(&lock_with_templates("1.0.0")),
  );
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  ensure(dir.path(), &runner, &index, &venv_with_templates("1.0.0"), false).unwrap();
  let py = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
  let doc: toml_edit::DocumentMut = py.parse().unwrap();
  assert_eq!(doc["dependency-groups"]["dev"][0].as_str(), Some("devkit-templates>=1.0.0"), "{py}");
  assert_eq!(
    doc["tool"]["uv"]["sources"]["devkit-templates"][0]["index"].as_str(),
    Some("SFTPyPI"),
    "{py}"
  );
  assert!(!py.contains("[tool]\n") && !py.contains("[tool.uv]\n"), "no empty headers: {py}");
  assert!(py.contains("[tool.ruff]\n  fix = true\n"), "{py}");
}

#[test]
fn a_stale_lock_is_one_problem_on_a_dry_run_and_stops_a_plain_run_before_any_write() {
  let stale = lock_with_templates("1.0.0").replace(&format!("version = \"{RUNNING_DEVKIT}\""), "version = \"0.0.1\"");
  let listed = PLAIN_PYPROJECT.replace("\"devkit-poe-complete\"]", "\"devkit-poe-complete\", \"devkit-templates>=1.0.0\"]");
  let dir = project(&listed, Some(&stale));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  // The bootstrap and its `advance` both ask; one report.
  let (_, changes) = ensure(dir.path(), &runner, &index, &venv_with_templates("1.0.0"), true).unwrap();
  assert_eq!(changes.problems.len(), 1, "{:?}", changes.problems);
  assert!(runner.calls_for("uv").is_empty());
  let dir = project(PLAIN_PYPROJECT, Some(&stale));
  let err = ensure(dir.path(), &runner, &index, &venv(None), false).unwrap_err().to_string();
  assert!(err.contains("uv.lock pins aeth-devkit 0.0.1"), "{err}");
  let py = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
  assert!(!py.contains("devkit-templates"), "nothing written before the refusal: {py}");
  assert!(runner.calls_for("uv").is_empty());
}
