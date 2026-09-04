//! End-to-end: the Docker step against fixture projects, with scripted consent.

use std::fs;
use std::path::{Path, PathBuf};

use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_core::prompt::ScriptedPrompt;
use aeth_devkit_setup::changes::Changes;
use aeth_devkit_setup::docker::{Deps, Mode};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("docker")
}

fn templates() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/aeth_devkit/templates")
}

fn write(root: &Path, rel: &str, content: &str) {
  let p = root.join(rel);
  fs::create_dir_all(p.parent().unwrap()).unwrap();
  fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
  fs::read_to_string(root.join(rel)).unwrap()
}

/// A git-tracked project with an origin, `services`, and an aeth-ext dependency.
fn project(services: &[&str], origin: &str) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  let list = services.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ");
  write(
    root,
    "pyproject.toml",
    &format!(
      "[project]\n  name = \"demo-app\"\n  version = \"1.2.3\"\n  dependencies = [\"aeth-ext>=8\"]\n\n[tool.docker]\n  services = [{list}]\n"
    ),
  );
  write(root, "src/demo_app/__init__.py", "");
  aeth_devkit_core::git::init_test_repo(root);
  let out = std::process::Command::new("git")
    .current_dir(root)
    .args(["remote", "add", "origin", origin])
    .output()
    .unwrap();
  assert!(out.status.success(), "git remote add: {}", String::from_utf8_lossy(&out.stderr));
  dir
}

fn run(root: &Path, mode: Mode, answers: &[&str], dry_run: bool) -> (Changes, ScriptedPrompt, RecordingRunner) {
  let prompt = ScriptedPrompt::new(answers);
  let runner = RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\nv1.0.0\n");
  let changes = {
    let deps = Deps {
      runner: &runner,
      prompt: &prompt,
      reviewer: None,
      mode,
    };
    aeth_devkit_setup::run_with(root, &templates(), dry_run, &deps).unwrap()
  };
  (changes, prompt, runner)
}

#[test]
fn fresh_project_gets_dockerfile_and_compose_then_is_idempotent() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  let (changes, prompt, runner) = run(root, Mode::Ask, &[], false);
  assert!(prompt.asked.borrow().is_empty(), "creation never prompts");
  let df = read(root, "docker/Dockerfile");
  assert!(
    df.contains(&format!(
      "/v{}/devkit-container-x86_64-unknown-linux-musl",
      env!("CARGO_PKG_VERSION")
    )),
    "{df}"
  );
  assert!(df.contains("mv /tmp/repo/src /app/src"), "{df}");
  assert!(!df.contains("gosu"), "{df}");
  let compose = read(root, "docker/compose.yaml");
  assert!(compose.contains("  demo-app:\n    container_name: demo-app\n"), "{compose}");
  assert!(compose.contains("GIT_REPO: https://github.com/O/Demo.git"), "{compose}");
  assert!(compose.contains("GIT_TAG: v1.1.0"), "latest stable tag, remote spelling: {compose}");
  assert!(compose.contains("source: /data/demo_app_files"), "{compose}");
  assert!(
    compose.contains("ALERTS_EMAIL=info@sweetfiretobacco.com"),
    "aeth-ext dependency: {compose}"
  );
  assert!(compose.ends_with("networks:\n  coolify:\n    external: true\n"), "{compose}");
  assert_eq!(runner.calls_for("gh").len(), 1);
  assert!(changes.files.iter().any(|f| f.path.ends_with("compose.yaml") && f.created));

  let (again, _, runner) = run(root, Mode::Ask, &[], false);
  assert!(again.is_empty(), "{}", again.report(root));
  assert!(runner.calls_for("gh").is_empty(), "a routine run never resolves the tag");
}

#[test]
fn without_aeth_ext_the_alerts_block_is_absent() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(root, "pyproject.toml", &read(root, "pyproject.toml").replace("\"aeth-ext>=8\"", ""));
  run(root, Mode::Ask, &[], false);
  assert!(!read(root, "docker/compose.yaml").contains("ALERTS_EMAIL"));
}

#[test]
fn dockerfile_drift_is_replaced_only_with_consent() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  let good = read(root, "docker/Dockerfile");
  write(root, "docker/Dockerfile", &good.replace("PYTHONOPTIMIZE=1", "PYTHONOPTIMIZE=2"));

  let (changes, prompt, _) = run(root, Mode::Ask, &[""], false);
  assert_eq!(
    prompt.asked.borrow()[0],
    "Replace docker/Dockerfile? [replace / replace all / anything else keeps it]:"
  );
  assert!(changes.is_empty(), "kept: {}", changes.report(root));
  assert!(read(root, "docker/Dockerfile").contains("PYTHONOPTIMIZE=2"));

  let (changes, _, _) = run(root, Mode::Ask, &["replace"], false);
  assert!(changes.files.iter().any(|f| f.path.ends_with("Dockerfile")));
  assert_eq!(read(root, "docker/Dockerfile"), good);

  // CRLF-only drift is not drift.
  write(root, "docker/Dockerfile", &good.replace('\n', "\r\n"));
  let (changes, prompt, _) = run(root, Mode::Ask, &[], false);
  assert!(changes.is_empty() && prompt.asked.borrow().is_empty());
}

#[test]
fn replace_all_covers_the_compose_edits_too() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  write(root, "docker/Dockerfile", "FROM scratch\n");
  write(
    root,
    "docker/compose.yaml",
    &read(root, "docker/compose.yaml").replace("interval: 30s", "interval: 99s"),
  );
  let (changes, prompt, _) = run(root, Mode::Ask, &["replace all"], false);
  assert_eq!(prompt.asked.borrow().len(), 1);
  assert_eq!(changes.files.len(), 2, "{}", changes.report(root));
  assert!(read(root, "docker/compose.yaml").contains("interval: 30s"));
}

#[test]
fn non_interactive_keeps_everything_and_says_so() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  write(root, "docker/Dockerfile", "FROM scratch\n");
  let (changes, _, _) = run(root, Mode::KeepAll, &[], false);
  assert!(changes.is_empty());
  assert!(changes.notes.iter().any(|n| n.contains("--replace-docker")), "{:?}", changes.notes);
  assert_eq!(read(root, "docker/Dockerfile"), "FROM scratch\n");
  // --replace-docker without a terminal applies files.
  let (changes, _, _) = run(root, Mode::ReplaceAll, &[], false);
  assert!(!changes.is_empty());
}

#[test]
fn dry_run_records_docker_drift_without_writing_or_asking() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  write(root, "docker/Dockerfile", "FROM scratch\n");
  let (changes, prompt, _) = run(root, Mode::DryRun, &[], true);
  assert!(prompt.asked.borrow().is_empty());
  assert!(
    changes.files.iter().any(|f| f.path.ends_with("Dockerfile")),
    "--check must see Docker drift"
  );
  assert_eq!(read(root, "docker/Dockerfile"), "FROM scratch\n");
}

#[test]
fn imap_fixture_with_injected_drift_gets_exactly_the_standard_edits() {
  let dir = project(
    &["imap-report-collector"],
    "https://github.com/AetherBreaker/IMAPReportCollector.git",
  );
  let root = dir.path();
  let original = fs::read_to_string(fixtures().join("compose-imap.yaml")).unwrap();
  let drifted = original
    .replace("      dockerfile: docker/Dockerfile\n", "      dockerfile: Dockerfile\n")
    .replace("      interval: 30s\n", "")
    .replace("      - ALERTS_EMAIL=info@sweetfiretobacco.com\n", "");
  assert_ne!(drifted, original, "fixture shape changed; update the injected drift");
  write(root, "docker/compose.yaml", &drifted);
  let (changes, _, _) = run(root, Mode::ReplaceAll, &[], false);
  let out = read(root, "docker/compose.yaml");
  assert!(out.contains("dockerfile: docker/Dockerfile"), "{out}");
  assert!(out.contains("interval: 30s"), "{out}");
  assert!(out.contains("- ALERTS_EMAIL=info@sweetfiretobacco.com"), "{out}");
  // Everything the standard does not name survives byte for byte.
  for kept in [
    "# Email to Watch",
    "WATCH_POLLING_TIMEOUT_SEC=600",
    "    # expose:\n",
    "restart: no\n",
    "GIT_TAG: v3.0.4",
  ] {
    assert!(out.contains(kept), "{kept} lost: {out}");
  }
  let details: Vec<&str> = changes
    .files
    .iter()
    .filter(|f| f.path.ends_with("compose.yaml"))
    .flat_map(|f| f.details.iter().map(String::as_str))
    .collect();
  assert_eq!(details.len(), 3, "{details:?}");
  let (again, _, _) = run(root, Mode::ReplaceAll, &[], false);
  assert!(again.is_empty(), "{}", again.report(root));
}

#[test]
fn aeth_ext_fixture_is_already_compliant() {
  let dir = project(&["central-log-server"], "git@github.com:AetherBreaker/aeth_ext.git");
  let root = dir.path();
  write(
    root,
    "pyproject.toml",
    &read(root, "pyproject.toml").replace("demo-app", "aeth-ext"),
  );
  write(
    root,
    "docker/compose.yaml",
    &fs::read_to_string(fixtures().join("compose-aeth-ext.yaml")).unwrap(),
  );
  let (changes, _, _) = run(root, Mode::ReplaceAll, &[], false);
  let compose_changed = changes.files.iter().any(|f| f.path.ends_with("compose.yaml"));
  assert!(!compose_changed, "{}", changes.report(root));
}

#[test]
fn a_missing_service_is_its_own_diff_and_sidecars_are_untouched() {
  let dir = project(&["demo-app", "worker"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(
    root,
    "docker/compose.yaml",
    "services:\n  wireguard:\n    image: wg\n  demo-app:\n    container_name: demo-app\n",
  );
  // demo-app edits: replace; worker add: keep; top level: replace.
  let (_, prompt, _) = run(root, Mode::Ask, &["replace", "", "replace"], false);
  let asked = prompt.asked.borrow().clone();
  assert_eq!(
    asked[0],
    "Apply the demo-app edits to docker/compose.yaml? [replace / replace all / anything else keeps it]:"
  );
  assert_eq!(
    asked[1],
    "Add service worker to docker/compose.yaml? [replace / replace all / anything else keeps it]:"
  );
  assert!(asked[2].starts_with("Apply the top-level edits"), "{asked:?}");
  let out = read(root, "docker/compose.yaml");
  assert!(!out.contains("  worker:"), "kept: {out}");
  assert!(out.contains("  wireguard:\n    image: wg\n"), "sidecar untouched: {out}");
  assert!(out.contains("  demo-app:\n    container_name: demo-app\n    build:\n"), "{out}");

  let (_, prompt, _) = run(root, Mode::Ask, &["replace"], false);
  assert_eq!(prompt.asked.borrow().len(), 1, "only the add remains: {:?}", prompt.asked.borrow());
  let out = read(root, "docker/compose.yaml");
  assert!(out.contains("\n  worker:\n    container_name: worker\n"), "{out}");
  assert!(out.contains("GIT_TAG: v1.1.0"), "{out}");
}

#[test]
fn replace_all_and_replace_docker_add_missing_services() {
  let dir = project(&["demo-app", "worker"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(
    root,
    "docker/compose.yaml",
    "services:\n  demo-app:\n    container_name: demo-app\n",
  );
  let (_, prompt, _) = run(root, Mode::Ask, &["replace all"], false);
  assert_eq!(prompt.asked.borrow().len(), 1);
  assert!(read(root, "docker/compose.yaml").contains("\n  worker:\n"));

  write(
    root,
    "docker/compose.yaml",
    "services:\n  demo-app:\n    container_name: demo-app\n",
  );
  let (changes, _, _) = run(root, Mode::ReplaceAll, &[], false);
  assert!(
    read(root, "docker/compose.yaml").contains("\n  worker:\n"),
    "{}",
    changes.report(root)
  );
}

#[test]
fn a_partial_answer_from_the_reviewer_writes_the_assembled_text() {
  use aeth_devkit_setup::vscode::protocol::{Response, ScriptedReviewer};
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  let good = read(root, "docker/Dockerfile");
  // Two edits more than six lines apart, so `similar` reports two hunks.
  write(
    root,
    "docker/Dockerfile",
    &(good.replace("PYTHONOPTIMIZE=1", "PYTHONOPTIMIZE=2") + "# trailing\n"),
  );
  let prompt = ScriptedPrompt::new(&[]);
  let runner = RecordingRunner::new(0);
  let reviewer = ScriptedReviewer::new(vec![Response::Partial { accepted: vec![0] }]);
  let deps = Deps {
    runner: &runner,
    prompt: &prompt,
    reviewer: Some(&reviewer),
    mode: Mode::Ask,
  };
  let changes = aeth_devkit_setup::run_with(root, &templates(), false, &deps).unwrap();
  let out = read(root, "docker/Dockerfile");
  assert!(out.contains("PYTHONOPTIMIZE=1") && out.ends_with("# trailing\n"), "{out}");
  assert!(
    changes.files.iter().any(|f| f.details.iter().any(|d| d.contains("1 of 2 hunks"))),
    "{}",
    changes.report(root)
  );
  assert!(prompt.asked.borrow().is_empty());
  assert_eq!(*reviewer.reviewed.borrow(), vec!["docker/Dockerfile"]);
}

#[test]
fn a_byte_order_mark_is_not_drift_and_does_not_hide_services() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  let good = read(root, "docker/Dockerfile");
  write(root, "docker/Dockerfile", &format!("\u{feff}{good}"));
  let compose = read(root, "docker/compose.yaml");
  write(
    root,
    "docker/compose.yaml",
    &format!("\u{feff}{}", compose.replace("interval: 30s", "interval: 99s")),
  );
  let (changes, prompt, _) = run(root, Mode::Ask, &["replace"], false);
  assert_eq!(
    prompt.asked.borrow().len(),
    1,
    "only the compose edit asks: {:?}",
    prompt.asked.borrow()
  );
  assert!(
    !changes.files.iter().any(|f| f.path.ends_with("Dockerfile")),
    "{}",
    changes.report(root)
  );
  let out = read(root, "docker/compose.yaml");
  assert!(out.starts_with("services:") && out.contains("interval: 30s"), "{out}");
}

#[test]
fn stray_entrypoint_files_are_reported_not_deleted() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(root, "docker/entrypoint.sh", "#!/bin/sh\n");
  write(root, "docker/scripts/get_readme.py", "");
  let (changes, _, _) = run(root, Mode::Ask, &[], false);
  assert!(
    changes.notes.iter().any(|n| n.contains("docker/entrypoint.sh and docker/scripts")),
    "{:?}",
    changes.notes
  );
  assert!(root.join("docker/entrypoint.sh").is_file());
}
