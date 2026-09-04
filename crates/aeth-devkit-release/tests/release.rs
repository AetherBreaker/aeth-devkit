//! `devkit release` end to end: real git in a temp repo, `uv`/`gh`/remote-git scripted,
//! devpi stubbed.
//!
//! The pattern: build a `World` (temp repo + recorders), run the command, then assert on
//! two things — the external calls that were made (via `RecordingRunner::calls_for`) and
//! the state of the repository afterwards. For rollback tests, "state afterwards" must be
//! byte-for-byte what it was before.

use std::cell::Cell;
use std::ops::Deref;
use std::path::Path;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use aeth_devkit_core::devpi::{DeleteOutcome, DevpiClient, StubDevpiClient};
use aeth_devkit_core::git;
use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::{CapturedOutput, RecordingRunner, Runner};
use aeth_devkit_release::prompt::ScriptedPrompt;
use aeth_devkit_release::{Args, Deps, run};

const PYPROJECT: &str = "[project]\nname = \"demo\"\nversion = \"1.0.0\"\n\n[[tool.uv.index]]\nname = \"Private\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";
const CARGO: &str = "[workspace]\n  members = []\n\n[workspace.package]\n  version = \"1.0.0\"\n";
const WORKFLOW: &str = ".github/workflows/release.yml";
/// What setup-project would render for the fixture: the publish step names its index.
const WORKFLOW_TEXT: &str = "name: Release\n  run: uv publish --index Private dist/*\n";

/// A temp repo plus every injectable collaborator, owned together so their lifetimes line
/// up: `deps()` borrows from `self`, and the borrow checker guarantees nothing is used after
/// the `World` is dropped.
struct World {
  dir: tempfile::TempDir,
  runner: SeqRunner,
  devpi: SeqDevpi,
  index: StubIndexClient,
  prompt: ScriptedPrompt,
  flag: Rc<AtomicBool>,
}

/// A `RecordingRunner` whose `gh run list --workflow …` answers empty until `gh release
/// create` has been called: the release's run cannot exist before the release does, and
/// step 8 must be seen to wait for a *new* run rather than adopt one scripted up front.
/// `Deref` keeps `w.runner.script(…)` / `calls_for(…)` working on the inner recorder.
struct SeqRunner {
  inner: RecordingRunner,
  released: Rc<Cell<bool>>,
  /// How many run-state queries still answer `in_progress` before the scripted answer.
  live_polls: Cell<usize>,
  /// Raised when `gh release create` runs, if `interrupt_on_create`: a Ctrl-C that lands
  /// while the release is being created.
  interrupted: Rc<AtomicBool>,
  interrupt_on_create: Cell<bool>,
  /// Cleared by a test that scripts runs which exist *before* the release (an earlier
  /// release of the tag), which the pre-flight must see.
  gate_runs: Cell<bool>,
}

/// The private index as the release sees it: `exists` is the stub's own answer until the
/// GitHub release has been created, after which the workflow "has published" and the
/// version is there — unless `publishes` is cleared to model a run that uploaded nothing.
/// The simple index (`StubIndexClient`) is not consulted for an index target at all.
struct SeqDevpi {
  inner: StubDevpiClient,
  released: Rc<Cell<bool>>,
  publishes: Cell<bool>,
}

impl Deref for SeqDevpi {
  type Target = StubDevpiClient;
  fn deref(&self) -> &StubDevpiClient {
    &self.inner
  }
}

impl DevpiClient for SeqDevpi {
  fn exists(&self, url: &str, username: &str, password: &str) -> anyhow::Result<bool> {
    let own = self.inner.exists(url, username, password)?;
    Ok(own || (self.released.get() && self.publishes.get()))
  }
  fn delete(&self, url: &str, username: &str, password: &str) -> anyhow::Result<DeleteOutcome> {
    self.inner.delete(url, username, password)
  }
}

impl Deref for SeqRunner {
  type Target = RecordingRunner;
  fn deref(&self) -> &RecordingRunner {
    &self.inner
  }
}

impl Runner for SeqRunner {
  fn run_inherit_env(&self, program: &str, args: &[String], cwd: &Path, env: &[(&str, &str)]) -> anyhow::Result<Option<i32>> {
    self.inner.run_inherit_env(program, args, cwd, env)
  }
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> anyhow::Result<CapturedOutput> {
    if program == "gh" && starts(args, &["release", "create"]) {
      self.released.set(true);
      if self.interrupt_on_create.get() {
        self.interrupted.store(true, Ordering::SeqCst);
      }
    }
    if program == "gh" && starts(args, &["run", "view", "123456", "--json", "status,conclusion"]) && self.live_polls.get() > 0 {
      self.live_polls.set(self.live_polls.get() - 1);
      self.inner.run_capture(program, args, cwd)?; // recorded
      return Ok(CapturedOutput {
        code: Some(0),
        stdout: "in_progress \n".into(),
        ..Default::default()
      });
    }
    if program == "gh" && starts(args, &["run", "list", "--workflow"]) && self.gate_runs.get() && !self.released.get() {
      self.inner.run_capture(program, args, cwd)?; // recorded, answered empty
      return Ok(CapturedOutput {
        code: Some(0),
        ..Default::default()
      });
    }
    self.inner.run_capture(program, args, cwd)
  }
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
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(root.join(WORKFLOW), WORKFLOW_TEXT).unwrap();
    git::init_test_repo(root);
    git::commit_paths(
      root,
      &["pyproject.toml".into(), "uv.lock".into(), "Cargo.toml".into(), WORKFLOW.into()],
      "init",
    )
    .unwrap();
    // The remote's view of main, as a fetch would have left it: everything is pushed.
    git_out(root, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let runner = RecordingRunner::new(0);
    // Remote-flavoured git answers: an upstream exists, we are on main, not behind, and
    // the remote has no tag for the target version.
    runner.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
    runner.script("git", &["rev-parse", "--abbrev-ref", "HEAD"], 0, "main\n");
    runner.script("git", &["rev-list", "--count"], 0, "0\n");
    runner.script("git", &["ls-remote"], 0, "");
    // The pre-flight probe (`release view <tag> --json …`) finds no release for either
    // candidate tag; the post-workflow existence check (`release view <tag>`, no `--json`)
    // finds the one step 7 created. Registered broad-first: newest registration wins.
    runner.script("gh", &["release", "view"], 0, "https://github.com/o/demo/releases/tag/v1.0.1\n");
    runner.script_err("gh", &["release", "view", "v1.0.1", "--json"], 1, "release not found");
    runner.script_err("gh", &["release", "view", "v1.0.0", "--json"], 1, "release not found");
    runner.script("gh", &["release", "create"], 0, "https://github.com/o/demo/releases/tag/v1.0.1\n");
    // The id the rollback deletes the release by.
    runner.script("gh", &["release", "view", "v1.0.1", "--json", "databaseId"], 0, "42\n");
    // The release workflow: one run exists at once, `gh run watch` succeeds (default exit 0).
    runner.script("gh", &["run", "list", "--workflow"], 0, "123456 completed\n");
    runner.script("gh", &["run", "view"], 0, "https://github.com/o/demo/actions/runs/123456\n");
    // Read only after `gh run watch` fails: the run has concluded, nothing to cancel.
    runner.script(
      "gh",
      &["run", "view", "123456", "--json", "status,conclusion"],
      0,
      "completed failure\n",
    );
    // Matching is newest-registration-wins, so the broad `uv version` answer goes first and
    // the more specific `--bump … --dry-run` answer overrides it for that call.
    runner.script("uv", &["version"], 0, "demo 1.0.0\n");
    runner.script("uv", &["version", "--bump", "patch", "--dry-run"], 0, "demo 1.0.0 => 1.0.1\n");
    let released = Rc::new(Cell::new(false));
    let flag = Rc::new(AtomicBool::new(false));
    Self {
      dir,
      runner: SeqRunner {
        inner: runner,
        released: Rc::clone(&released),
        live_polls: Cell::new(0),
        interrupted: Rc::clone(&flag),
        interrupt_on_create: Cell::new(false),
        gate_runs: Cell::new(true),
      },
      devpi: SeqDevpi {
        inner: StubDevpiClient::new(false),
        released,
        publishes: Cell::new(true),
      },
      // Only a PyPI target reads the simple index; the fixture publishes to `Private`.
      index: StubIndexClient { versions: vec![] },
      prompt: ScriptedPrompt::new(answers),
      flag,
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
      index: &self.index,
      prompt: &self.prompt,
      env: &env,
      interrupted: &self.flag,
      sleep: &|_| {},
    }
  }

  fn args(&self, words: &[&str]) -> Args {
    Args {
      root: self.root().to_path_buf(),
      force: false,
      dry_run: false,
      index: None,
      no_wait: false,
      words: words.iter().map(|s| s.to_string()).collect(),
    }
  }

  /// Everything a rollback must restore, as one comparable value: `HEAD`, the tag, every
  /// managed file (absent ones as `None`), and the whole index (`git ls-files -s`, so
  /// staged blobs and modes are covered).
  fn state(&self) -> State {
    let r = self.root();
    let read = |rel: &str| std::fs::read_to_string(r.join(rel)).ok();
    State {
      head: git::head_sha(r).unwrap(),
      tag: git::tag_target(r, "v1.0.1").unwrap(),
      pyproject: read("pyproject.toml"),
      uv_lock: read("uv.lock"),
      cargo_toml: read("Cargo.toml"),
      cargo_lock: read("Cargo.lock"),
      index: git_out(r, &["ls-files", "-s"]),
    }
  }
}

/// See [`World::state`]. `PartialEq` + `Debug` so `assert_eq!` can compare and print it.
#[derive(Debug, PartialEq, Eq)]
struct State {
  head: String,
  tag: Option<String>,
  pyproject: Option<String>,
  uv_lock: Option<String>,
  cargo_toml: Option<String>,
  cargo_lock: Option<String>,
  index: String,
}

fn ok(c: ExitCode) -> bool {
  c == ExitCode::SUCCESS
}

/// Whether a recorded `gh` call deletes the v1.0.1 release: by the id read after creation
/// (the fixture answers 42), or by tag when that lookup was scripted to fail.
fn deletes_release(c: &[String]) -> bool {
  c[..] == ["api", "-X", "DELETE", "repos/{owner}/{repo}/releases/42"] || starts(c, &["release", "delete", "v1.0.1"])
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
  // Nothing is built or published here; the workflow does that.
  assert!(uv.iter().all(|c| c[0] != "build" && c[0] != "publish"));
  // Cargo.lock does not exist in the fixture, so `cargo update` must not run; Cargo.toml
  // itself is still rewritten.
  assert!(w.runner.calls_for("cargo").is_empty());
  let git_calls = w.runner.calls_for("git");
  assert!(git_calls.contains(&vec![
    "push".to_string(),
    "--atomic".into(),
    "origin".into(),
    "main".into(),
    "v1.0.1".into()
  ]));
  let gh = w.runner.calls_for("gh");
  let create = gh.iter().find(|c| starts(c, &["release", "create"])).unwrap();
  assert!(create.contains(&"--generate-notes".to_string()));
  // No files on the command line: `gh release create <tag> --title <tag> --generate-notes`.
  assert_eq!(create.len(), 6, "{create:?}");
  // Step 8: the runs for the tag were listed before the release was created (so an old
  // run of a re-released tag is never mistaken for the new one), then the new run watched.
  let listed_at = gh.iter().position(|c| starts(c, &["run", "list", "--workflow"])).unwrap();
  let created_at = gh.iter().position(|c| starts(c, &["release", "create"])).unwrap();
  assert!(listed_at < created_at, "{gh:?}");
  assert!(gh.iter().any(|c| c == &["run", "watch", "123456", "--exit-status"]), "{gh:?}");
  let st = w.state();
  assert_ne!(st.head, before);
  assert_eq!(st.tag.as_deref(), Some(git::short_head(w.root()).unwrap().as_str()));
  assert!(st.cargo_toml.unwrap().contains("version = \"1.0.1\""));
  assert!(git::status_porcelain(w.root()).unwrap().is_empty());
}

#[test]
fn run_outcome_reports_released_version() {
  use aeth_devkit_release::{Outcome, run_outcome};
  let w = World::new(&[]);
  let outcome = run_outcome(&w.args(&["patch"]), &w.deps()).unwrap();
  assert_eq!(outcome, Outcome::Released { version: "1.0.1".into() });
  // A dry run reports DryRun, never Released.
  let w = World::new(&[]);
  let mut args = w.args(&["patch"]);
  args.dry_run = true;
  assert_eq!(run_outcome(&args, &w.deps()).unwrap(), Outcome::DryRun);
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
      .contains(&vec!["push".to_string(), "--atomic".into(), "origin".into(), "v1.0.0".into()])
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
fn push_failure_resets_commit_and_tag() {
  let w = rollback_case("git", &["push", "--atomic", "origin", "main"]);
  assert!(w.runner.calls_for("gh").iter().all(|c| !deletes_release(c)));
}

#[test]
fn failed_workflow_run_rolls_back_the_whole_release() {
  let w = rollback_case("gh", &["run", "watch"]);
  let gh = w.runner.calls_for("gh");
  assert!(gh.iter().any(|c| deletes_release(c)), "{gh:?}");
  assert!(gh.iter().all(|c| !starts(c, &["run", "cancel"])), "{gh:?}");
}

#[test]
fn a_run_still_going_when_its_watcher_dies_is_cancelled_before_the_rollback() {
  let w = World::new(&[]);
  w.runner.script("gh", &["run", "watch"], 130, "");
  // Live for two polls after the cancel, then the scripted "completed failure".
  w.runner.live_polls.set(2);
  let before = w.state();
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  let gh = w.runner.calls_for("gh");
  let cancel = gh.iter().position(|c| c[..] == ["run", "cancel", "123456"]).unwrap();
  let delete = gh.iter().position(|c| deletes_release(c)).unwrap();
  assert!(cancel < delete, "{gh:?}");
}

#[test]
fn an_interrupt_during_release_creation_cancels_the_run_then_rolls_back() {
  let w = World::new(&[]);
  w.runner.interrupt_on_create.set(true);
  w.runner.live_polls.set(1); // in progress when found, then the scripted "completed failure"
  let before = w.state();
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  let gh = w.runner.calls_for("gh");
  assert!(gh.iter().all(|c| !starts(c, &["run", "watch"])), "{gh:?}");
  let cancel = gh.iter().position(|c| c[..] == ["run", "cancel", "123456"]).unwrap();
  let delete = gh.iter().position(|c| deletes_release(c)).unwrap();
  assert!(cancel < delete, "{gh:?}");
}

#[test]
fn a_run_that_never_settles_is_left_in_place_not_rolled_back() {
  let w = World::new(&[]);
  w.runner.script("gh", &["run", "watch"], 1, "");
  // Never reports completed: the settle window is spent (sleeps are no-ops here). The
  // release, tag and bump commit all stay, because the run may still upload to them.
  w.runner
    .script("gh", &["run", "view", "123456", "--json", "status,conclusion"], 0, "in_progress \n");
  let before = w.state();
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  let after = w.state();
  assert_ne!(after.head, before.head);
  assert!(after.tag.is_some());
  let gh = w.runner.calls_for("gh");
  assert!(gh.iter().any(|c| c[..] == ["run", "cancel", "123456"]), "{gh:?}");
  assert!(gh.iter().all(|c| !deletes_release(c)), "{gh:?}");
  assert!(
    w.runner
      .calls_for("git")
      .iter()
      .all(|c| !starts(c, &["push", "--force-with-lease"]))
  );
}

#[test]
fn a_workflow_for_the_wrong_publish_target_is_refused_before_anything() {
  let w = World::new(&[]);
  let root = w.root();
  write_file(
    root,
    WORKFLOW,
    "name: Release\n  run: uv publish --trusted-publishing always dist/*\n",
  );
  git::commit_paths(root, &[WORKFLOW.into()], "stale wf").unwrap();
  let before = w.state();
  let err = run(&w.args(&["patch"]), &w.deps()).unwrap_err().to_string();
  assert!(err.contains("does not publish to Private"), "{err}");
  assert_eq!(w.state(), before);
}

#[test]
fn an_active_run_of_an_earlier_release_is_refused_before_anything() {
  let w = World::new(&[]);
  w.runner.gate_runs.set(false);
  w.runner.script("gh", &["run", "list", "--workflow"], 0, "777 queued\n");
  let before = w.state();
  let err = run(&w.args(&["patch"]), &w.deps()).unwrap_err();
  assert!(err.to_string().contains("777"), "{err}");
  assert_eq!(w.state(), before);
  let gh = w.runner.calls_for("gh");
  assert!(gh.iter().all(|c| !starts(c, &["release", "create"])), "{gh:?}");
}

#[test]
fn published_version_missing_after_a_green_run_rolls_back() {
  let w = World::new(&[]);
  w.devpi.publishes.set(false);
  let before = w.state();
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("gh").iter().any(|c| deletes_release(c)));
}

#[test]
fn no_wait_returns_once_the_release_exists() {
  use aeth_devkit_release::{Outcome, run_outcome};
  let w = World::new(&[]);
  let mut a = w.args(&["patch"]);
  a.no_wait = true;
  assert_eq!(run_outcome(&a, &w.deps()).unwrap(), Outcome::Released { version: "1.0.1".into() });
  let gh = w.runner.calls_for("gh");
  assert!(gh.iter().all(|c| !starts(c, &["run", "watch"])), "{gh:?}");
  // The runs are listed once, by the pre-flight active-run check; never after the release
  // exists, since nothing waits for its run.
  let created_at = gh.iter().position(|c| starts(c, &["release", "create"])).unwrap();
  let listings: Vec<usize> = gh
    .iter()
    .enumerate()
    .filter(|(_, c)| starts(c, &["run", "list", "--workflow"]))
    .map(|(i, _)| i)
    .collect();
  // Pre-flight and the re-check before creation, none after.
  assert!(listings.len() == 2 && listings.iter().all(|&i| i < created_at), "{gh:?}");
}

#[test]
fn missing_workflow_file_is_refused_before_anything() {
  let w = World::new(&[]);
  let root = w.root();
  assert!(git_out(root, &["rm", "-q", WORKFLOW]).is_empty());
  git_out(root, &["commit", "-q", "-m", "drop workflow"]);
  let before = w.state();
  let err = run(&w.args(&["patch"]), &w.deps()).unwrap_err().to_string();
  assert!(err.contains("release.yml is not committed"), "{err}");
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] == "--version"));
}

#[test]
fn tag_only_release_needs_the_workflow_on_origin_main() {
  let w = World::new(&[]);
  let root = w.root();
  // A workflow edit committed locally but not pushed: main is ahead of origin/main.
  write_file(root, WORKFLOW, &format!("{WORKFLOW_TEXT}edited: true\n"));
  git::commit_paths(root, &[WORKFLOW.into()], "wf").unwrap();
  let before = w.state();
  let err = run(&w.args(&[]), &w.deps()).unwrap_err().to_string();
  assert!(err.contains("origin/main") && err.contains("push main first"), "{err}");
  assert_eq!(w.state(), before);
  // A bump release pushes main with the tag, so the same tree releases fine.
  assert!(ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
}

#[test]
fn github_failure_unwinds_everything_with_lease() {
  let w = World::new(&[]);
  // `create` fails, and the follow-up `view` confirms nothing was created.
  w.runner.script_err("gh", &["release", "view"], 1, "release not found");
  let before = w.state();
  w.runner.script("gh", &["release", "create"], 1, "");
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  let git_calls = w.runner.calls_for("git");
  assert!(
    git_calls
      .iter()
      .any(|c| c[0] == "push" && c[1].starts_with("--force-with-lease=main:"))
  );
  // The remote tag is deleted with a lease on the tag object this run created, so only
  // *our* tag can be the one removed.
  assert!(git_calls.iter().any(|c| c.len() == 4
    && c[0] == "push"
    && c[1].starts_with("--force-with-lease=refs/tags/v1.0.1:")
    && c[3] == ":refs/tags/v1.0.1"));
  // The GitHub release was never created, so nothing tries to delete it.
  assert!(w.runner.calls_for("gh").iter().all(|c| !deletes_release(c)));
}

#[test]
fn partial_github_release_is_deleted_on_rollback() {
  let w = World::new(&[]);
  let before = w.state();
  // `create` fails (say, an asset upload), but `view` afterwards finds the release.
  w.runner.script("gh", &["release", "create"], 1, "");
  w.runner
    .script("gh", &["release", "view"], 0, "https://github.com/o/demo/releases/tag/v1.0.1\n");
  let mut a = w.args(&["patch"]);
  a.force = true; // the pre-flight probe sees that same `view` answer
  assert!(!ok(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  let gh = w.runner.calls_for("gh");
  let create_at = gh.iter().position(|c| starts(c, &["release", "create"])).unwrap();
  assert!(
    gh[create_at..].iter().any(|c| deletes_release(c)),
    "the half-created release must be deleted during rollback: {gh:?}"
  );
}

#[test]
fn rollback_keeps_unrelated_staged_changes() {
  let w = World::new(&[]);
  let root = w.root();
  // The user has an unrelated file staged and accepts the dirty tree with `--force`.
  std::fs::write(root.join("notes.md"), "wip\n").unwrap();
  assert!(
    std::process::Command::new("git")
      .args(["add", "notes.md"])
      .current_dir(root)
      .status()
      .unwrap()
      .success()
  );
  let before = w.state();
  w.runner.script("gh", &["release", "create"], 1, "");
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(!ok(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  let staged = std::process::Command::new("git")
    .args(["diff", "--cached", "--name-only"])
    .current_dir(root)
    .output()
    .unwrap();
  assert_eq!(String::from_utf8(staged.stdout).unwrap().trim(), "notes.md");
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
fn gh_view_errors_other_than_not_found_abort_preflight() {
  let w = World::new(&[]);
  let before = w.state();
  w.runner.script_err("gh", &["release", "view"], 1, "HTTP 401: Bad credentials");
  let err = run(&w.args(&["patch"]), &w.deps()).unwrap_err().to_string();
  assert!(err.contains("gh release view"), "{err}");
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] != "lock"));
}

fn write_file(root: &Path, rel: &str, content: &str) {
  std::fs::write(root.join(rel), content).unwrap();
}

/// `git <args>` in the repo, stdout trimmed. For the assertions that need raw git.
fn git_out(root: &Path, args: &[&str]) -> String {
  let out = std::process::Command::new("git").args(args).current_dir(root).output().unwrap();
  String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// The user has an unstaged note at the top of Cargo.toml and a staged edit in uv.lock,
/// and runs with `--force`. Only the release's own change (the Cargo.toml version, since
/// `uv` is scripted) may land in the bump commit; both edits must survive around it.
fn dirty_release_files_world() -> World {
  let w = World::new(&[]);
  let root = w.root();
  std::fs::write(root.join("Cargo.toml"), format!("# user note\n{CARGO}")).unwrap();
  std::fs::write(root.join("uv.lock"), "version = 1\n# staged by user\n").unwrap();
  assert!(git_out(root, &["add", "uv.lock"]).is_empty());
  w
}

#[test]
fn user_edits_to_release_files_stay_out_of_the_bump_commit() {
  let w = dirty_release_files_world();
  let root = w.root();
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(ok(run(&a, &w.deps()).unwrap()));
  // The commit: bumped version, no user note, uv.lock untouched.
  let committed_cargo = git_out(root, &["show", "HEAD:Cargo.toml"]);
  assert!(
    committed_cargo.contains("version = \"1.0.1\"") && !committed_cargo.contains("user note"),
    "{committed_cargo}"
  );
  assert_eq!(git_out(root, &["show", "HEAD:uv.lock"]), "version = 1");
  // The working tree: the user's note *and* the bump.
  let cargo = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
  assert!(
    cargo.starts_with("# user note\n") && cargo.contains("version = \"1.0.1\""),
    "{cargo}"
  );
  // `git status` shows exactly what the user had before: Cargo.toml unstaged, uv.lock staged.
  assert_eq!(git_out(root, &["diff", "--cached", "--name-only"]), "uv.lock");
  assert_eq!(git_out(root, &["diff", "--name-only"]), "Cargo.toml");
  assert_eq!(
    std::fs::read_to_string(root.join("uv.lock")).unwrap(),
    "version = 1\n# staged by user\n"
  );
}

#[test]
fn rollback_restores_user_edits_and_index_exactly() {
  let w = dirty_release_files_world();
  let root = w.root();
  let before = w.state();
  let staged_before = git_out(root, &["ls-files", "-s"]);
  w.runner.script("gh", &["release", "create"], 1, "");
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(!ok(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert_eq!(git_out(root, &["ls-files", "-s"]), staged_before, "index must be byte-identical");
  assert_eq!(
    std::fs::read_to_string(root.join("uv.lock")).unwrap(),
    "version = 1\n# staged by user\n"
  );
}

#[test]
fn deleted_lockfile_is_regenerated_for_the_commit_but_stays_deleted_for_the_user() {
  let w = World::new(&[]);
  let root = w.root();
  // The user deleted uv.lock (unstaged) and accepts the dirty tree with --force.
  std::fs::remove_file(root.join("uv.lock")).unwrap();
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(ok(run(&a, &w.deps()).unwrap()));
  // The commit still carries uv.lock (regenerated from HEAD by the scripted `uv lock`)…
  assert_eq!(git_out(root, &["show", "HEAD:uv.lock"]), "version = 1");
  // …while the user's working tree shows the same unstaged deletion it showed before.
  assert!(!root.join("uv.lock").exists());
  assert_eq!(git_out(root, &["diff", "--name-only"]), "uv.lock");
  assert_eq!(git_out(root, &["diff", "--cached", "--name-only"]), "");
}

#[test]
fn deleted_lockfile_rollback_is_exact() {
  let w = World::new(&[]);
  std::fs::remove_file(w.root().join("uv.lock")).unwrap();
  let before = w.state();
  w.runner.script("gh", &["release", "create"], 1, "");
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(!ok(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
}

#[test]
fn untracked_lockfile_stays_untracked_and_out_of_the_commit() {
  let w = World::new(&[]);
  let root = w.root();
  // A Cargo.lock the user created but never committed (the fixture's HEAD has none). The
  // untracked file makes the tree dirty; --force accepts that.
  std::fs::write(root.join("Cargo.lock"), "# the user's lockfile\n").unwrap();
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(ok(run(&a, &w.deps()).unwrap()));
  // The bump commit must not adopt the user's file — releasing must never be the thing
  // that starts tracking a path the user chose not to commit.
  assert_eq!(git_out(root, &["ls-tree", "HEAD", "--", "Cargo.lock"]), "");
  // Afterwards the file is right where it was: on disk and untracked.
  assert_eq!(git_out(root, &["status", "--porcelain", "--", "Cargo.lock"]), "?? Cargo.lock");
}

#[test]
fn untracked_lockfile_rollback_is_exact() {
  let w = World::new(&[]);
  std::fs::write(w.root().join("Cargo.lock"), "# the user's lockfile\n").unwrap();
  let before = w.state();
  w.runner.script("gh", &["release", "create"], 1, "");
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(!ok(run(&a, &w.deps()).unwrap()));
  // `state` covers the Cargo.lock bytes and the full index listing, so this also proves
  // the file is byte-identical and still untracked.
  assert_eq!(w.state(), before);
}

#[test]
fn staged_chmod_stays_out_of_the_bump_commit() {
  let w = World::new(&[]);
  let root = w.root();
  // The user staged an executable bit on pyproject.toml (content unchanged). `--chmod=+x`
  // records mode 100755 in the index regardless of filesystem support, so this works on
  // Windows too.
  assert!(git_out(root, &["update-index", "--chmod=+x", "pyproject.toml"]).is_empty());
  let mut a = w.args(&["patch"]);
  a.force = true; // the staged mode change is a dirty tree
  assert!(ok(run(&a, &w.deps()).unwrap()));
  // The commit keeps HEAD's mode: the pending chmod is the user's change, not the bump's…
  assert!(git_out(root, &["ls-tree", "HEAD", "--", "pyproject.toml"]).starts_with("100644"));
  // …and it is still staged for them afterwards.
  assert!(git_out(root, &["ls-files", "-s", "--", "pyproject.toml"]).starts_with("100755"));
}

#[test]
fn unmerged_managed_file_is_refused_before_anything() {
  use std::io::Write as _;
  let w = World::new(&[]);
  let root = w.root();
  // Manufacture a merge conflict in uv.lock: stage-1/2/3 rows via `update-index
  // --index-info` are what a real conflicted merge leaves in the index.
  let sha = git_out(root, &["hash-object", "-w", "uv.lock"]);
  let rows = format!("100644 {sha} 1\tuv.lock\n100644 {sha} 2\tuv.lock\n100644 {sha} 3\tuv.lock\n");
  let mut child = std::process::Command::new("git")
    .args(["update-index", "--index-info"])
    .current_dir(root)
    .stdin(std::process::Stdio::piped())
    .spawn()
    .unwrap();
  child.stdin.take().unwrap().write_all(rows.as_bytes()).unwrap();
  assert!(child.wait().unwrap().success());
  let before = w.state();
  // A hard pre-flight error (exit 2), not a prompt: --force must not wave a conflict
  // through, because neither the release nor the rollback can represent three stages.
  let mut a = w.args(&["patch"]);
  a.force = true;
  let err = run(&a, &w.deps()).unwrap_err().to_string();
  assert!(err.contains("unmerged"), "{err}");
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] != "lock"));
}

#[test]
fn ambiguous_push_failure_leaves_foreign_refs_alone() {
  // The push fails, and a same-named tag exists on the remote — but its object id ("abc")
  // is not the tag object this run created: it belongs to a concurrent publisher, and the
  // rollback must leave it. (The same script answers the pre-flight probe, so the run
  // first reports the "leaked" tag and removes it with consent — hence the "force".)
  let w = World::new(&["force"]);
  let before = w.state();
  w.runner.script("git", &["push", "--atomic"], 1, "");
  w.runner.script(
    "git",
    &["ls-remote", "--tags", "origin", "refs/tags/v1.0.1"],
    0,
    "abc\trefs/tags/v1.0.1\n",
  );
  assert!(!ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  let git_calls = w.runner.calls_for("git");
  // Exactly one delete: pre-flight's consented removal (plain `--delete`). The rollback
  // journals no tag compensation, because the probed object id is not ours.
  assert_eq!(
    git_calls
      .iter()
      .filter(|c| starts(c, &["push", "origin", "--delete", "v1.0.1"]))
      .count(),
    1
  );
  // No leased deletes and no branch rewind either: the branch probe (the broad
  // `ls-remote` script answers "") saw no bump sha on the remote.
  assert!(git_calls.iter().all(|c| !c.iter().any(|arg| arg.starts_with("--force-with-lease"))));
}

#[test]
fn uncommitted_config_edit_is_refused() {
  let w = World::new(&[]);
  let root = w.root();
  // The user edited [project].name and did not commit. The release would build from the
  // committed name but publish (and roll back) under the edited one, so it must refuse —
  // even under --force.
  std::fs::write(root.join("pyproject.toml"), PYPROJECT.replace("\"demo\"", "\"other\"")).unwrap();
  let before = w.state();
  let mut a = w.args(&["patch"]);
  a.force = true;
  let err = run(&a, &w.deps()).unwrap_err().to_string();
  assert!(err.contains("project name"), "{err}");
  assert_eq!(w.state(), before);
  // Refused before pre-flight ever asked uv anything beyond the tool check.
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] == "--version"));
}

#[test]
fn overlapping_user_edit_is_an_error_and_rolls_back() {
  let w = World::new(&[]);
  let root = w.root();
  // The user changed the very line the bump rewrites (same version value, so the
  // Cargo/pyproject pre-flight check still passes; a trailing comment on that line).
  std::fs::write(root.join("Cargo.toml"), CARGO.replace("\"1.0.0\"", "\"1.0.0\" # pinned")).unwrap();
  let before = w.state();
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(!ok(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("git").iter().all(|c| c[0] != "push"));
}

#[test]
fn crlf_checkout_is_not_an_overlapping_edit() {
  let w = World::new(&[]);
  let root = w.root();
  // A Windows checkout: `core.autocrlf=true`, so every managed file sits on disk as CRLF
  // while HEAD holds LF. `git status` is clean, and the release must agree — a raw byte
  // comparison would see an edit on every line, and the merge-back would then refuse the
  // bump as an overlapping edit of the version line.
  // Checked out by git, as on a real machine, so the index stat data matches the CRLF
  // files (a hand-written CRLF file is stat-dirty and `status` reports it modified).
  assert!(git_out(root, &["config", "core.autocrlf", "true"]).is_empty());
  for rel in ["Cargo.toml", "pyproject.toml", "uv.lock"] {
    std::fs::remove_file(root.join(rel)).unwrap();
  }
  assert!(git_out(root, &["checkout", "--", "Cargo.toml", "pyproject.toml", "uv.lock"]).is_empty());
  assert!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap().contains("\r\n"));
  assert_eq!(git_out(root, &["status", "--porcelain"]), "");
  // No `--force`: the tree really is clean, so no dirty-tree prompt may appear.
  assert!(ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  // The commit holds LF (repository form) with the bump…
  let committed = git_out(root, &["show", "HEAD:Cargo.toml"]);
  assert!(
    committed.contains("version = \"1.0.1\"") && !committed.contains('\r'),
    "{committed:?}"
  );
  // …the working copy keeps its CRLF endings, and the tree is clean afterwards.
  let on_disk = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
  assert!(on_disk.contains("version = \"1.0.1\"\r\n"), "{on_disk:?}");
  assert_eq!(git_out(root, &["status", "--porcelain"]), "");
}

#[test]
fn crlf_checkout_with_a_user_edit_merges_cleanly() {
  let w = World::new(&[]);
  let root = w.root();
  assert!(git_out(root, &["config", "core.autocrlf", "true"]).is_empty());
  // The user's note sits far from the version line; only line endings differ elsewhere.
  std::fs::write(root.join("Cargo.toml"), format!("# user note\n{CARGO}").replace('\n', "\r\n")).unwrap();
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(ok(run(&a, &w.deps()).unwrap()));
  let committed = git_out(root, &["show", "HEAD:Cargo.toml"]);
  assert!(
    committed.contains("version = \"1.0.1\"") && !committed.contains("user note"),
    "{committed}"
  );
  let on_disk = std::fs::read(root.join("Cargo.toml")).unwrap();
  assert!(
    on_disk.starts_with(b"# user note\r\n") && on_disk.windows(19).any(|w| w == b"version = \"1.0.1\"\r\n"),
    "{:?}",
    String::from_utf8_lossy(&on_disk)
  );
  // `git status` shows exactly what the user had before: Cargo.toml unstaged, nothing staged.
  assert_eq!(git_out(root, &["diff", "--name-only"]), "Cargo.toml");
  assert_eq!(git_out(root, &["diff", "--cached", "--name-only"]), "");
}

#[test]
fn interrupt_before_cleanup_deletes_nothing() {
  let w = World::new(&[]);
  w.devpi.exists.set(true);
  w.flag.store(true, std::sync::atomic::Ordering::SeqCst);
  let mut a = w.args(&["patch"]);
  a.force = true;
  let err = run(&a, &w.deps()).unwrap_err().to_string();
  assert!(err.contains("interrupted"), "{err}");
  assert!(w.devpi.calls.borrow().iter().all(|c| c.starts_with("GET")));
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
      .all(|c| c[0] == "--version" || starts(c, &["release", "view"]) || starts(c, &["run", "list"]))
  );
}

#[test]
fn cargo_mismatch_is_refused_before_anything() {
  let w = World::new(&[]);
  std::fs::write(w.root().join("Cargo.toml"), CARGO.replace("1.0.0", "0.9.9")).unwrap();
  let e = run(&w.args(&["patch"]), &w.deps()).unwrap_err().to_string();
  assert!(e.contains("0.9.9"), "{e}");
}
