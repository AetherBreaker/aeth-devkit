//! `devkit release` end to end: real git in a temp repo, `uv`/`gh`/remote-git scripted,
//! devpi stubbed.
//!
//! The pattern: build a `World` (temp repo + recorders), run the command, then assert on
//! two things — the external calls that were made (via `RecordingRunner::calls_for`) and
//! the state of the repository afterwards. For rollback tests, "state afterwards" must be
//! byte-for-byte what it was before.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use aeth_devkit_core::devpi::StubDevpiClient;
use aeth_devkit_core::git;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_release::prompt::ScriptedPrompt;
use aeth_devkit_release::{Args, Deps, run};

const PYPROJECT: &str = "[project]\nname = \"demo\"\nversion = \"1.0.0\"\n\n[[tool.uv.index]]\nname = \"Private\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";
const CARGO: &str = "[workspace]\n  members = []\n\n[workspace.package]\n  version = \"1.0.0\"\n";

/// A temp repo plus every injectable collaborator, owned together so their lifetimes line
/// up: `deps()` borrows from `self`, and the borrow checker guarantees nothing is used after
/// the `World` is dropped.
struct World {
  dir: tempfile::TempDir,
  runner: RecordingRunner,
  devpi: StubDevpiClient,
  prompt: ScriptedPrompt,
  flag: AtomicBool,
}

/// Fake environment: only the two credential variables exist.
fn env(key: &str) -> Option<String> {
  match key {
    "UV_INDEX_PRIVATE_USERNAME" => Some("u".into()),
    "UV_INDEX_PRIVATE_PASSWORD" => Some("p".into()),
    _ => None,
  }
}

impl World {
  fn new(answers: &[&str]) -> Self {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("pyproject.toml"), PYPROJECT).unwrap();
    std::fs::write(root.join("uv.lock"), "version = 1\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), CARGO).unwrap();
    // `dist/` is ignored, as in a real project; otherwise the old wheel would make the
    // tree look dirty and the run would stop at the dirty-tree prompt.
    std::fs::write(
      root.join(".gitignore"),
      "dist/
",
    )
    .unwrap();
    std::fs::create_dir(root.join("dist")).unwrap();
    std::fs::write(root.join("dist/demo-0.9.0-py3-none-any.whl"), "old").unwrap();
    git::init_test_repo(root);
    git::commit_paths(
      root,
      &["pyproject.toml".into(), "uv.lock".into(), "Cargo.toml".into(), ".gitignore".into()],
      "init",
    )
    .unwrap();
    let runner = RecordingRunner::new(0);
    // Remote-flavoured git answers: an upstream exists, we are on main, not behind, and
    // the remote has no tag for the target version.
    runner.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
    runner.script("git", &["rev-parse", "--abbrev-ref", "HEAD"], 0, "main\n");
    runner.script("git", &["rev-list", "--count"], 0, "0\n");
    runner.script("git", &["ls-remote"], 0, "");
    // No GitHub release exists (view fails); create succeeds and prints a URL.
    runner.script("gh", &["release", "view"], 1, "");
    runner.script("gh", &["release", "create"], 0, "https://github.com/o/demo/releases/tag/v1.0.1\n");
    // Matching is newest-registration-wins, so the broad `uv version` answer goes first and
    // the more specific `--bump … --dry-run` answer overrides it for that call.
    runner.script("uv", &["version"], 0, "demo 1.0.0\n");
    runner.script("uv", &["version", "--bump", "patch", "--dry-run"], 0, "demo 1.0.0 => 1.0.1\n");
    Self {
      dir,
      runner,
      devpi: StubDevpiClient::new(false),
      prompt: ScriptedPrompt::new(answers),
      flag: AtomicBool::new(false),
    }
  }

  fn root(&self) -> &Path {
    self.dir.path()
  }

  /// `Deps<'_>`: the returned struct borrows from `self` for as long as it is used.
  fn deps(&self) -> Deps<'_> {
    Deps {
      runner: &self.runner,
      devpi: &self.devpi,
      prompt: &self.prompt,
      env: &env,
      interrupted: &self.flag,
    }
  }

  fn args(&self, words: &[&str]) -> Args {
    Args {
      root: self.root().to_path_buf(),
      force: false,
      dry_run: false,
      index: None,
      words: words.iter().map(|s| s.to_string()).collect(),
    }
  }

  /// Everything a rollback must restore, as one comparable value.
  fn state(&self) -> (String, Option<String>, String, String, Vec<PathBuf>) {
    let r = self.root();
    (
      git::head_sha(r).unwrap(),
      git::tag_target(r, "v1.0.1").unwrap(),
      std::fs::read_to_string(r.join("pyproject.toml")).unwrap(),
      std::fs::read_to_string(r.join("Cargo.toml")).unwrap(),
      aeth_devkit_release::snapshot::dist_artifacts(r).unwrap(),
    )
  }
}

fn ok(c: ExitCode) -> bool {
  c == ExitCode::SUCCESS
}

/// Does a recorded argument list start with `prefix`? Safe on short lists, unlike slicing.
fn starts(c: &[String], prefix: &[&str]) -> bool {
  c.len() >= prefix.len() && c.iter().zip(prefix).all(|(a, b)| a == b)
}

#[test]
fn bump_mode_happy_path() {
  let w = World::new(&[]);
  let before = git::head_sha(w.root()).unwrap();
  assert!(ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  let uv = w.runner.calls_for("uv");
  assert!(uv.contains(&vec!["version".to_string(), "--bump".into(), "patch".into()]));
  assert!(uv.contains(&vec!["lock".to_string()]));
  assert!(uv.contains(&vec!["build".to_string()]));
  assert!(
    uv.iter()
      .any(|c| c.starts_with(&["publish".to_string(), "--index".into(), "Private".into()]))
  );
  // Cargo.lock does not exist in the fixture, so `cargo update` must not run; Cargo.toml
  // itself is still rewritten.
  assert!(w.runner.calls_for("cargo").is_empty());
  let git_calls = w.runner.calls_for("git");
  assert!(git_calls.contains(&vec!["push".to_string(), "origin".into(), "main".into(), "v1.0.1".into()]));
  let gh = w.runner.calls_for("gh");
  let create = gh.iter().find(|c| starts(c, &["release", "create"])).unwrap();
  assert!(create.contains(&"--generate-notes".to_string()));
  let (head, tag, _py, cargo, _dist) = w.state();
  assert_ne!(head, before);
  assert_eq!(tag.as_deref(), Some(git::short_head(w.root()).unwrap().as_str()));
  assert!(cargo.contains("version = \"1.0.1\""));
  assert!(git::status_porcelain(w.root()).unwrap().is_empty());
}

#[test]
fn no_bump_mode_pushes_only_the_tag() {
  let w = World::new(&[]);
  let before = git::head_sha(w.root()).unwrap();
  assert!(ok(run(&w.args(&[]), &w.deps()).unwrap()));
  assert_eq!(git::head_sha(w.root()).unwrap(), before);
  assert!(
    w.runner
      .calls_for("git")
      .contains(&vec!["push".to_string(), "origin".into(), "v1.0.0".into()])
  );
  assert!(!w.runner.calls_for("uv").contains(&vec!["lock".to_string()]));
}

#[test]
fn notes_are_forwarded() {
  let w = World::new(&[]);
  assert!(ok(run(&w.args(&["patch", "first", "patch", "release"]), &w.deps()).unwrap()));
  let gh = w.runner.calls_for("gh");
  let create = gh.iter().find(|c| starts(c, &["release", "create"])).unwrap();
  let i = create.iter().position(|a| a == "--notes").unwrap();
  assert_eq!(create[i + 1], "first patch release");
}

/// Run a bump release with one scripted failure and assert the repo is fully restored.
fn rollback_case(fail_program: &str, fail_args: &[&str]) -> World {
  let w = World::new(&[]);
  let before = w.state();
  w.runner.script(fail_program, fail_args, 1, "");
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(
    w.state(),
    before,
    "repo state must be restored after {fail_program} {fail_args:?} fails"
  );
  w
}

#[test]
fn build_failure_rolls_back_files_only() {
  let w = rollback_case("uv", &["build"]);
  assert!(w.runner.calls_for("git").iter().all(|c| c[0] != "push"));
  assert!(w.devpi.calls.borrow().iter().all(|c| !c.starts_with("DELETE")));
}

#[test]
fn publish_failure_resets_commit_and_tag() {
  let w = rollback_case("uv", &["publish"]);
  assert!(w.runner.calls_for("git").iter().all(|c| c[0] != "push"));
}

#[test]
fn push_failure_deletes_devpi_version() {
  let w = rollback_case("git", &["push", "origin", "main"]);
  assert_eq!(*w.devpi.calls.borrow().last().unwrap(), "DELETE https://x/user/internal/demo/1.0.1");
  assert!(w.runner.calls_for("gh").iter().all(|c| !starts(c, &["release", "delete"])));
}

#[test]
fn github_failure_unwinds_everything_with_lease() {
  let w = rollback_case("gh", &["release", "create"]);
  let git_calls = w.runner.calls_for("git");
  assert!(
    git_calls
      .iter()
      .any(|c| c[0] == "push" && c[1].starts_with("--force-with-lease=main:"))
  );
  assert!(git_calls.contains(&vec!["push".to_string(), "origin".into(), "--delete".into(), "v1.0.1".into()]));
  // The GitHub release was never created, so nothing tries to delete it.
  assert!(w.runner.calls_for("gh").iter().all(|c| !starts(c, &["release", "delete"])));
}

#[test]
fn leaked_local_tag_is_reported_and_removed_on_force() {
  let w = World::new(&["force"]);
  git::create_annotated_tag(w.root(), "v1.0.1", "leak").unwrap();
  assert!(ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.prompt.asked.borrow().len(), 1);
  assert!(w.prompt.asked.borrow()[0].contains("Remove these"));
  // The leaked tag was deleted and a fresh one now points at the new bump commit.
  assert_eq!(
    git::tag_target(w.root(), "v1.0.1").unwrap().as_deref(),
    Some(git::short_head(w.root()).unwrap().as_str())
  );
}

#[test]
fn leaked_artefacts_abort_without_force() {
  let w = World::new(&["no"]);
  w.devpi.exists.set(true);
  let before = w.state();
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert!(w.devpi.calls.borrow().iter().all(|c| c.starts_with("GET")));
}

#[test]
fn dirty_tree_prompt() {
  let w = World::new(&["nope"]);
  std::fs::write(w.root().join("scratch.txt"), "x").unwrap();
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert!(w.prompt.asked.borrow()[0].contains("dirty tree"));

  // With `--force` the prompt is skipped entirely.
  let w2 = World::new(&[]);
  std::fs::write(w2.root().join("scratch.txt"), "x").unwrap();
  let mut a = w2.args(&["patch"]);
  a.force = true;
  assert!(ok(run(&a, &w2.deps()).unwrap()));
  assert!(w2.prompt.asked.borrow().is_empty());
}

#[test]
fn dry_run_changes_nothing() {
  let w = World::new(&[]);
  let before = w.state();
  let mut a = w.args(&["patch"]);
  a.dry_run = true;
  assert!(ok(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] == "version" || c[0] == "--version"));
  assert!(
    w.runner
      .calls_for("gh")
      .iter()
      .all(|c| c[0] == "--version" || starts(c, &["release", "view"]))
  );
}

#[test]
fn cargo_mismatch_is_refused_before_anything() {
  let w = World::new(&[]);
  std::fs::write(w.root().join("Cargo.toml"), CARGO.replace("1.0.0", "0.9.9")).unwrap();
  let e = run(&w.args(&["patch"]), &w.deps()).unwrap_err().to_string();
  assert!(e.contains("0.9.9"), "{e}");
}
