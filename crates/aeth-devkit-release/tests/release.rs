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

use aeth_devkit_core::devpi::{DeleteOutcome, DevpiClient, StubDevpiClient};
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
    runner.script_err("gh", &["release", "view"], 1, "release not found");
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

  /// Everything a rollback must restore, as one comparable value: `HEAD`, the tag, every
  /// managed file (absent ones as `None`), the artefacts, whether `dist/` exists at all,
  /// and the whole index (`git ls-files -s`, so staged blobs and modes are covered).
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
      dist: aeth_devkit_release::snapshot::dist_artifacts(r).unwrap(),
      dist_dir_exists: r.join("dist").is_dir(),
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
  dist: Vec<PathBuf>,
  dist_dir_exists: bool,
  index: String,
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
  // Exactly this and nothing more on argv: credentials travel in the child's environment,
  // never where an error message could echo them.
  assert!(uv.contains(&vec!["publish".to_string(), "--index".into(), "Private".into()]));
  assert!(uv.iter().flatten().all(|a| a != "--password" && a != "p"));
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
  let st = w.state();
  assert_ne!(st.head, before);
  assert_eq!(st.tag.as_deref(), Some(git::short_head(w.root()).unwrap().as_str()));
  assert!(st.cargo_toml.unwrap().contains("version = \"1.0.1\""));
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
  let w = rollback_case("git", &["push", "--atomic", "origin", "main"]);
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
  // The remote tag is deleted with a lease on the tag object this run created, so only
  // *our* tag can be the one removed.
  assert!(git_calls.iter().any(|c| c.len() == 4
    && c[0] == "push"
    && c[1].starts_with("--force-with-lease=refs/tags/v1.0.1:")
    && c[3] == ":refs/tags/v1.0.1"));
  // The GitHub release was never created, so nothing tries to delete it.
  assert!(w.runner.calls_for("gh").iter().all(|c| !starts(c, &["release", "delete"])));
}

/// A devpi where the version is *always* present — models the partial upload where the
/// wheel landed before the sdist failed, so `uv publish` exits non-zero yet the index
/// holds a release. The stub's flip-on-delete would hide that case.
struct StickyDevpi {
  calls: std::cell::RefCell<Vec<String>>,
}

impl DevpiClient for StickyDevpi {
  fn exists(&self, url: &str, _u: &str, _p: &str) -> anyhow::Result<bool> {
    self.calls.borrow_mut().push(format!("GET {url}"));
    Ok(true)
  }
  fn delete(&self, url: &str, _u: &str, _p: &str) -> anyhow::Result<DeleteOutcome> {
    self.calls.borrow_mut().push(format!("DELETE {url}"));
    Ok(DeleteOutcome::Deleted)
  }
}

#[test]
fn partial_publish_is_deleted_on_rollback() {
  let w = World::new(&[]);
  let devpi = StickyDevpi {
    calls: std::cell::RefCell::new(Vec::new()),
  };
  let deps = Deps {
    runner: &w.runner,
    devpi: &devpi,
    prompt: &w.prompt,
    env: &env,
    interrupted: &w.flag,
  };
  let before = w.state();
  w.runner.script("uv", &["publish"], 1, "");
  // `force`: the pre-flight probe also sees the sticky version and would otherwise prompt.
  let mut a = w.args(&["patch"]);
  a.force = true;
  assert!(!ok(run(&a, &deps).unwrap()));
  assert_eq!(w.state(), before);
  let url = "https://x/user/internal/demo/1.0.1";
  // Pre-flight probe + removal, then the post-failure probe and the rollback delete.
  assert_eq!(
    *devpi.calls.borrow(),
    vec![
      format!("GET {url}"),
      format!("DELETE {url}"),
      format!("GET {url}"),
      format!("DELETE {url}")
    ]
  );
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
    gh[create_at..].iter().any(|c| starts(c, &["release", "delete", "v1.0.1"])),
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
  w.runner.script("uv", &["publish"], 1, "");
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
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] != "build"));
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
  w.runner.script("uv", &["publish"], 1, "");
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
  w.runner.script("uv", &["publish"], 1, "");
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
  w.runner.script("uv", &["publish"], 1, "");
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
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] != "build"));
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

/// A devpi holding this version with a file this run never built — a concurrent
/// publisher's release. `exists` answers `false` for the pre-flight probe, modelling the
/// concurrent upload landing *between* pre-flight and our failed publish.
struct ForeignDevpi {
  calls: std::cell::RefCell<Vec<String>>,
}

impl DevpiClient for ForeignDevpi {
  fn exists(&self, url: &str, _u: &str, _p: &str) -> anyhow::Result<bool> {
    self.calls.borrow_mut().push(format!("GET {url}"));
    Ok(false)
  }
  fn delete(&self, url: &str, _u: &str, _p: &str) -> anyhow::Result<DeleteOutcome> {
    self.calls.borrow_mut().push(format!("DELETE {url}"));
    Ok(DeleteOutcome::Deleted)
  }
  fn files(&self, _url: &str, _u: &str, _p: &str) -> anyhow::Result<Option<Vec<(String, String)>>> {
    Ok(Some(vec![("demo-1.0.1-py3-none-any.whl".into(), "https://x/f.whl".into())]))
  }
  fn fetch(&self, _href: &str, _u: &str, _p: &str) -> anyhow::Result<Vec<u8>> {
    Ok(b"someone else's wheel".to_vec())
  }
}

#[test]
fn foreign_devpi_version_is_not_deleted_on_rollback() {
  let w = World::new(&[]);
  let devpi = ForeignDevpi {
    calls: std::cell::RefCell::new(Vec::new()),
  };
  let deps = Deps {
    runner: &w.runner,
    devpi: &devpi,
    prompt: &w.prompt,
    env: &env,
    interrupted: &w.flag,
  };
  let before = w.state();
  w.runner.script("uv", &["publish"], 1, "");
  assert!(!ok(run(&w.args(&["patch"]), &deps).unwrap()));
  assert_eq!(w.state(), before);
  // Everything local was rolled back, but the foreign index version was left alone.
  assert!(
    devpi.calls.borrow().iter().all(|c| !c.starts_with("DELETE")),
    "{:?}",
    devpi.calls.borrow()
  );
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
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] != "publish"));
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

#[test]
fn publish_hands_uv_the_credential_variables_it_actually_reads() {
  // uv keeps two separate credential sets: `UV_INDEX_<NAME>_*` authenticate *reads* from an
  // index during resolution, while `uv publish` looks for `UV_PUBLISH_USERNAME` /
  // `_PASSWORD`. Supplying only the first pair leaves uv with no publish credentials, so it
  // prompts on the terminal at step 7 of 9 — after the commit and tag are already made.
  let w = World::new(&[]);
  assert!(ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));

  let calls = w.runner.calls.borrow();
  let publish = calls
    .iter()
    .find(|c| c.program == "uv" && c.args.first().is_some_and(|a| a == "publish"))
    .expect("uv publish must run");
  let env: std::collections::HashMap<&str, &str> = publish.env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
  assert_eq!(env.get("UV_PUBLISH_USERNAME"), Some(&"u"), "{:?}", publish.env);
  assert_eq!(env.get("UV_PUBLISH_PASSWORD"), Some(&"p"), "{:?}", publish.env);

  // And still nowhere near argv, which is what `run_ok`'s error message interpolates.
  assert!(!publish.args.iter().any(|a| a == "p" || a == "u"));
}

#[test]
fn no_other_step_is_handed_the_publish_credentials() {
  // The password should reach exactly one child process. Anything else inheriting it widens
  // the blast radius of a compromised tool for no benefit.
  let w = World::new(&[]);
  assert!(ok(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  let calls = w.runner.calls.borrow();
  let with_creds: Vec<&str> = calls
    .iter()
    .filter(|c| c.env.iter().any(|(k, _)| k == "UV_PUBLISH_PASSWORD"))
    .map(|c| c.program.as_str())
    .collect();
  assert_eq!(with_creds, ["uv"], "only `uv publish` may carry them");
}
