//! End-to-end: apply the template snapshot under `tests/fixtures/templates` to fixture
//! projects, check outcomes and idempotency. The templates live in devkit-templates; the
//! engine's tests are hermetic.

use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn templates() -> PathBuf {
  fixtures().join("templates")
}

fn write(root: &Path, rel: &str, content: &str) {
  let p = root.join(rel);
  fs::create_dir_all(p.parent().unwrap()).unwrap();
  fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
  fs::read_to_string(root.join(rel)).unwrap()
}

/// `run_with` accepting every proposal, with the Docker step's `gh` tag lookup answered
/// locally (the fixture lists a service, so a first run would otherwise hit GitHub, and
/// fail without a token in CI), no index answers, and the fixture copy of the container
/// package's template standing in for the venv. Renders the snapshot as an override; see
/// `run_from_venv` for the bootstrap path.
fn run(root: &Path, dry_run: bool) -> anyhow::Result<aeth_devkit_setup::changes::Changes> {
  run_via(root, dry_run, Some(&templates()))
}

/// `run` through the environment: no override, so the run bootstraps `devkit-templates`
/// (the stub venv holds it at 1.0.0 with the snapshot as its `templates/`).
fn run_from_venv(root: &Path, dry_run: bool) -> anyhow::Result<aeth_devkit_setup::changes::Changes> {
  run_via(root, dry_run, None)
}

fn run_via(root: &Path, dry_run: bool, templates_override: Option<&Path>) -> anyhow::Result<aeth_devkit_setup::changes::Changes> {
  let runner = aeth_devkit_core::process::RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\n");
  let index = aeth_devkit_core::index::StubIndexClient { versions: vec![] };
  let mut map = std::collections::HashMap::new();
  for (name, version, dir) in [
    ("devkit_container", "1.4.0", fixtures().join("docker")),
    ("devkit_claude_hooks", "1.0.0", fixtures().join("docker")),
    ("devkit_poe_complete", "1.0.0", fixtures().join("docker")),
    ("devkit_templates", "1.0.0", fixtures()),
  ] {
    map.insert(
      name.to_string(),
      aeth_devkit_setup::packages::Installed {
        dir,
        version: version.into(),
      },
    );
  }
  let venv = aeth_devkit_setup::packages::StubVenv(map);
  let deps = aeth_devkit_setup::Deps {
    docker: aeth_devkit_setup::docker::Deps {
      runner: &runner,
      prompt: &aeth_devkit_core::prompt::ScriptedPrompt::new(&[]),
      reviewer: None,
      mode: if dry_run {
        aeth_devkit_setup::docker::Mode::DryRun
      } else {
        aeth_devkit_setup::docker::Mode::Yes
      },
    },
    index: &index,
    venv: &venv,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root)?;
  aeth_devkit_setup::run_with(&ctx, templates_override, dry_run, &deps)
}

fn make_project() -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  let fx = fixtures();
  fs::copy(fx.join("pyproject.fixture.toml"), root.join("pyproject.toml")).unwrap();
  for f in ["launch.json", "tasks.json", "settings.json"] {
    write(
      root,
      &format!(".vscode/{f}"),
      &fs::read_to_string(fx.join("vscode").join(f)).unwrap(),
    );
  }
  write(root, ".gitignore", &fs::read_to_string(fx.join("gitignore-custom")).unwrap());
  write(root, ".env", &fs::read_to_string(fx.join("env")).unwrap());
  write(root, "src/imap_report_collector/__init__.py", "");
  write(root, "uv.lock", &devkit_lock());
  dir
}

/// [`make_project`] with no Docker service: the binary's runs below go through the real
/// environment, which holds no `devkit-container`, and a Docker project without it is refused
/// (the plain run installs it; a dry run cannot). These tests are about other things.
fn make_project_without_docker() -> tempfile::TempDir {
  let dir = make_project();
  let py = read(dir.path(), "pyproject.toml").replace("services    = [\"imap-report-collector\"]", "services    = []");
  write(dir.path(), "pyproject.toml", &py);
  dir
}

/// A lock as uv leaves it with every devkit package (the templates included) at the version
/// the stub venv holds, so the package step's recorded `uv lock` has nothing to change and
/// the sync is skipped.
fn devkit_lock() -> String {
  format!(
    "version = 1\n\n[[package]]\nname = \"aeth-devkit\"\nversion = \"{}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-claude-hooks\"\nversion = \"1.0.0\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-poe-complete\"\nversion = \"1.0.0\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-container\"\nversion = \"1.4.0\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-templates\"\nversion = \"1.0.0\"\nsource = {{ registry = \"https://idx/+simple\" }}\n",
    aeth_devkit_setup::packages::RUNNING_DEVKIT
  )
}

#[test]
fn applies_and_is_idempotent() {
  let dir = make_project();
  let root = dir.path();

  let changes = run(root, false).unwrap();
  let py = read(root, "pyproject.toml");
  assert!(!py.contains("{latest}") && !py.contains("setup-project:"), "{py}");
  let changed: Vec<String> = changes
    .files
    .iter()
    .map(|f| f.path.file_name().unwrap().to_string_lossy().to_string())
    .collect();
  for expected in [
    "pyproject.toml",
    "settings.json",
    "extensions.json",
    "tasks.json",
    ".env",
    "testing.env",
    ".gitignore",
    ".gitattributes",
    ".dockerignore",
  ] {
    assert!(
      changed.contains(&expected.to_string()),
      "expected {expected} to change; changed = {changed:?}"
    );
  }
  // aeth_ext's launch.json already carries envFile + the env keys, so it must be left byte-identical.
  assert!(
    !changed.contains(&"launch.json".to_string()),
    "compliant launch.json must not be rewritten"
  );
  assert_eq!(
    read(root, ".vscode/launch.json"),
    fs::read_to_string(fixtures().join("vscode/launch.json")).unwrap()
  );

  // pyproject: cache dirs, inlined config, extends removed, package placeholder, no mypy
  let py = read(root, "pyproject.toml");
  assert!(py.contains("cache_dir    = \".cache/pytest\""), "{py}");
  assert!(py.contains("[tool.coverage.html]"), "{py}");
  assert!(!py.contains("extends = \"../pyproject.toml\""), "{py}");
  assert!(!py.contains("extend    = \"../pyproject.toml\""), "{py}");
  assert!(
    py.replace(' ', "").contains("known-first-party=[\"imap_report_collector\"]"),
    "{py}"
  );
  assert!(!py.contains("[tool.mypy]"), "{py}");
  assert!(py.contains("\"poe-tasks>=4.0.0\""), "poe-tasks pin must be untouched: {py}");
  assert!(py.contains("[tool.docker]"), "project-only sections must survive: {py}");
  assert!(py.contains("required_persisted_dirs = [\"persisted_data\"]"), "{py}");
  assert!(
    changes.notes.iter().any(|n| n.contains("chown_paths and mkdirs")),
    "{:?}",
    changes.notes
  );
  assert!(py.contains("[[tool.uv.index]]"), "{py}");

  // .env: in-place replacement, secrets untouched
  let env = read(root, ".env");
  assert!(env.starts_with("SECRET=\"abc\"\n"), "{env}");
  assert!(
    env.contains(&format!(
      "PYTHONPYCACHEPREFIX=\"{}/.cache/pycache\"",
      root.canonicalize().unwrap().to_string_lossy().trim_start_matches(r"\\?\")
    )),
    "{env}"
  );
  assert!(env.contains("OTHER=1"));
  assert!(
    root.join("testing.env").is_file(),
    "envFile referenced by launch.json must be created"
  );

  // launch.json: every debugpy config patched, others untouched, header kept
  let launch = read(root, ".vscode/launch.json");
  assert!(launch.contains("// Use IntelliSense"), "{launch}");
  assert_eq!(
    launch
      .matches("\"PYTHONPYCACHEPREFIX\": \"${workspaceFolder}/.cache/pycache\"")
      .count(),
    3,
    "{launch}"
  );
  assert!(launch.contains("\"type\": \"PowerShell\""));
  assert!(launch.contains("\"compounds\""));

  // settings.json: merged, pre-existing keys kept
  let settings = read(root, ".vscode/settings.json");
  assert!(settings.contains("\"python.testing.pytestArgs\""));
  assert!(settings.contains("terminal.integrated.env.windows"));

  // gitignore: template first, custom tail kept once
  let gi = read(root, ".gitignore");
  assert!(gi.starts_with("# Byte-compiled"), "{gi}");
  assert!(
    gi.contains("# ---- project-specific ----\n# project stuff\nsecrets/\n*.db\n"),
    "{gi}"
  );
  assert_eq!(gi.matches("persisted_data/").count(), 1, "{gi}");

  assert!(read(root, ".gitattributes").contains("* text=auto eol=lf"));
  assert!(read(root, ".dockerignore").contains(".cache/"));

  // Second run: nothing changes.
  let again = run(root, false).unwrap();
  assert!(again.is_empty(), "second run should be a no-op, got:\n{}", again.report(root));
}

#[test]
fn dry_run_writes_nothing() {
  let dir = make_project();
  let root = dir.path();
  let before = read(root, "pyproject.toml");
  let changes = run(root, true).unwrap();
  assert!(!changes.is_empty());
  assert_eq!(read(root, "pyproject.toml"), before);
  assert!(!root.join(".vscode/extensions.json").exists());
}

#[test]
fn uv_init_gitignore_is_replaced_and_mypy_is_conditional() {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  write(
    root,
    "pyproject.toml",
    "[project]\n  name = \"demo-app\"\n  version = \"0.1.0\"\n  dependencies = []\n\n[dependency-groups]\n  dev = [\"mypy>=1\"]\n",
  );
  write(root, ".gitignore", &fs::read_to_string(fixtures().join("gitignore-uv")).unwrap());
  write(root, "uv.lock", &devkit_lock());
  let changes = run(root, false).unwrap();
  let gi = read(root, ".gitignore");
  assert!(!gi.contains("project-specific"), "{gi}");
  assert!(gi.starts_with("# Byte-compiled"));
  let py = read(root, "pyproject.toml");
  assert!(py.contains("[tool.mypy]"), "{py}");
  assert!(py.contains("source_pkgs = [\"demo_app\"]"), "{py}");
  assert!(root.join(".vscode/launch.json").is_file());
  assert!(!root.join(".dockerignore").exists(), "no docker setup → no .dockerignore");
  assert!(!changes.is_empty());
  assert!(run(root, false).unwrap().is_empty());
}

#[test]
fn mixed_rust_python_project_uses_python_dir_and_rust_overlays() {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  write(
    root,
    "pyproject.toml",
    "[project]\n  name = \"mixed-tool\"\n  version = \"0.1.0\"\n  dependencies = []\n",
  );
  write(root, "Cargo.toml", "[package]\nname = \"mixed-tool\"\nversion = \"0.1.0\"\n");
  write(root, "src/main.rs", "fn main() {}\n");
  write(root, "python/mixed_tool/__init__.py", "");
  write(root, ".gitignore", "# custom\nsecrets/\n");
  write(root, "uv.lock", &devkit_lock());
  run(root, false).unwrap();

  let py = read(root, "pyproject.toml");
  assert!(py.contains("src       = [\"./python\", \"../*/src\", \"../*/python\"]"), "{py}");
  assert!(py.contains("root = \"python\", extraPaths = [\"python\"]"), "{py}");
  assert!(py.contains("source_pkgs = [\"mixed_tool\"]"), "{py}");
  let launch = read(root, ".vscode/launch.json");
  assert!(launch.contains("\"PYTHONPATH\": \"${workspaceFolder}/python\""), "{launch}");
  let ext = read(root, ".vscode/extensions.json");
  assert!(ext.contains("rust-lang.rust-analyzer"), "{ext}");
  let settings = read(root, ".vscode/settings.json");
  assert!(settings.contains("\"[rust]\""), "{settings}");
  let gi = read(root, ".gitignore");
  assert!(gi.contains("*.pdb"), "rust overlay must be merged: {gi}");
  assert!(gi.contains("secrets/"), "{gi}");
  // The container package has its own repository and release workflow, so no rendered
  // release workflow mentions it.
  let wf = read(root, ".github/workflows/release.yml");
  assert!(!wf.contains("container"), "{wf}");
  assert!(wf.contains("targets: ${{ matrix.target }}\n"), "{wf}");
  assert!(wf.contains("name: Wheel (${{ matrix.target }})"), "{wf}");
  assert!(!wf.contains("setup-project:"), "markers must not leak: {wf}");
  assert!(
    wf.contains("dist/*.whl dist/*.tar.gz"),
    "publish must not feed binaries to uv: {wf}"
  );
  assert!(run(root, false).unwrap().is_empty());
}

#[test]
fn plain_python_project_gets_no_rust_overlays() {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  write(
    root,
    "pyproject.toml",
    "[project]\n  name = \"plain\"\n  version = \"0.1.0\"\n  dependencies = []\n",
  );
  write(root, "src/plain/__init__.py", "");
  write(root, "uv.lock", &devkit_lock());
  run(root, false).unwrap();
  assert!(read(root, "pyproject.toml").contains("src       = [\"./src\", \"../*/src\", \"../*/python\"]"));
  assert!(!read(root, ".vscode/extensions.json").contains("rust-analyzer"));
  assert!(!read(root, ".vscode/settings.json").contains("[rust]"));
}

#[test]
fn commits_only_changed_trackable_files_in_a_git_repo() {
  let dir = make_project();
  let root = dir.path();
  let git = |args: &[&str]| git(root, args);
  git(&["init", "-q"]);
  git(&["config", "user.email", "t@t"]);
  git(&["config", "user.name", "t"]);
  git(&["add", "-A"]);
  git(&["commit", "-q", "-m", "init"]);
  // Something the user staged but that setup-project must not sweep into its commit.
  write(root, "unrelated.txt", "x\n");
  git(&["add", "unrelated.txt"]);

  let mut bases = aeth_devkit_setup::git::stage_bases(root).unwrap();
  let changes = run(root, false).unwrap();
  assert!(aeth_devkit_setup::git::is_git_tracked(root));
  let hash = aeth_devkit_setup::git::commit_changes(root, &changes, &mut bases).unwrap();
  assert!(hash.is_some());

  let subject = git(&["log", "-1", "--format=%s"]);
  assert_eq!(subject, aeth_devkit_setup::git::COMMIT_SUBJECT);
  let committed = git(&["show", "--name-only", "--format=", "HEAD"]);
  assert!(committed.contains("pyproject.toml"), "{committed}");
  assert!(committed.contains(".vscode/settings.json"), "{committed}");
  // Installed by this run; `devkit release` refuses an uncommitted workflow.
  assert!(committed.contains(".github/workflows/release.yml"), "{committed}");
  assert!(committed.contains("docker/Dockerfile"), "{committed}");
  assert!(committed.contains("docker/compose.yaml"), "{committed}");
  assert!(
    !committed.contains(".env"),
    ".env is gitignored and must not be committed: {committed}"
  );
  assert!(
    !committed.contains("unrelated.txt"),
    "pre-staged user file must be left alone: {committed}"
  );
  assert_eq!(
    git(&["diff", "--cached", "--name-only"]),
    "unrelated.txt",
    "user's staged file must remain staged"
  );

  // Nothing to commit on a second run.
  let mut bases = aeth_devkit_setup::git::stage_bases(root).unwrap();
  let again = run(root, false).unwrap();
  assert!(aeth_devkit_setup::git::commit_changes(root, &again, &mut bases).unwrap().is_none());
}

#[test]
fn uncommitted_edits_to_managed_files_stay_out_of_the_commit() {
  let dir = make_project();
  let root = dir.path();
  let git = |args: &[&str]| git(root, args);
  git_init(root);
  git(&["add", "-A"]);
  git(&["commit", "-q", "-m", "init"]);
  // An uncommitted user edit to a managed file, on a line the templates leave alone.
  let user_rule = "user-scratch-dir/";
  write(root, ".gitignore", &format!("{}{user_rule}\n", read(root, ".gitignore")));

  let mut bases = aeth_devkit_setup::git::stage_bases(root).unwrap();
  let changes = run(root, false).unwrap();
  let hash = aeth_devkit_setup::git::commit_changes(root, &changes, &mut bases).unwrap();
  assert!(hash.is_some());

  // The commit was built from HEAD + templates: the user's rule is not in it…
  let committed = git(&["show", "HEAD:.gitignore"]);
  assert!(!committed.contains(user_rule), "{committed}");
  // …but it survives in the working tree, still uncommitted.
  assert!(read(root, ".gitignore").contains(user_rule));
  let status = git(&["status", "--porcelain", ".gitignore"]);
  assert!(status.contains(".gitignore"), "the user edit must stay uncommitted: {status:?}");
}

#[test]
fn an_uncommitted_services_change_cancels_a_committing_run() {
  // The committing run merges into HEAD's pyproject but takes the switch from the working
  // copy, so a `services` that differs between the two is refused before anything is
  // staged: the user commits it and reruns. Three shapes: the key added to an existing
  // table, the whole table added (at the end and mid-file), and the value changed.
  let fixture = read(make_project().path(), "pyproject.toml");
  let services_line = "  services    = [\"imap-report-collector\"]\n";
  assert!(fixture.contains(services_line), "{fixture}");
  let no_key = fixture.replace(services_line, "");
  let no_table = strip_tool_docker(&fixture);
  let table = "[tool.docker]\n  services = [\"imap-report-collector\"]\n";
  for (head, edited) in [
    (no_key.clone(), fixture.clone()),
    (no_table.clone(), format!("{no_table}\n{table}")),
    (
      no_table.clone(),
      no_table.replacen("[tool.pytest", &format!("{table}\n[tool.pytest"), 1),
    ),
    (
      fixture.replace("imap-report-collector\"]", "imap-report-collector\", \"worker\"]"),
      fixture.clone(),
    ),
    (fixture.clone(), fixture.replace(services_line, "  services    = []\n")),
  ] {
    assert_ne!(head, edited);
    let dir = make_project();
    let root = dir.path();
    write(root, "pyproject.toml", &head);
    git_init(root);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    write(root, "pyproject.toml", &edited);
    let err = aeth_devkit_setup::cli::run(&aeth_devkit_setup::cli::Args {
      root: root.to_path_buf(),
      templates_dir: Some(templates()),
      dry_run: false,
      no_commit: false,
      yes: false,
      vscode: false,
      no_vscode: true,
    })
    .unwrap_err()
    .to_string();
    assert!(err.contains("services is not committed"), "{err}");
    assert_eq!(read(root, "pyproject.toml"), edited, "nothing touched");
    assert!(!root.join("docker/Dockerfile").exists());
    assert_eq!(git(root, &["rev-list", "--count", "HEAD"]), "1");
    // Committed, the same run goes through and the Docker step runs (or not) as the
    // committed switch says.
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "services"]);
    let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
    let mut bases = aeth_devkit_setup::git::stage_bases(root).unwrap();
    let changes = run(root, false).unwrap();
    assert!(
      aeth_devkit_setup::git::commit_changes(root, &changes, &mut bases)
        .unwrap()
        .is_some()
    );
    assert_eq!(root.join("docker/Dockerfile").is_file(), ctx.has_docker);
  }
}

/// A venv where `devkit_container` arrives with the run's `uv sync`, as it does for a
/// project adopting or upgrading the package: absent before, `container` after.
struct LateContainer<'a> {
  base: aeth_devkit_setup::packages::StubVenv,
  runner: &'a aeth_devkit_core::process::RecordingRunner,
  container: aeth_devkit_setup::packages::Installed,
}

impl aeth_devkit_setup::packages::Venv for LateContainer<'_> {
  fn installed(
    &self,
    root: &Path,
    package: &aeth_devkit_setup::packages::DevkitPackage,
  ) -> Option<aeth_devkit_setup::packages::Installed> {
    if package.import_name != "devkit_container" {
      return self.base.installed(root, package);
    }
    let synced = self
      .runner
      .calls_for("uv")
      .iter()
      .any(|args| args.first().map(String::as_str) == Some("sync"));
    synced.then(|| self.container.clone())
  }
}

#[test]
fn a_container_package_installed_by_the_run_has_its_gates_swept_before_the_docker_step() {
  // The package step installs devkit-container mid-run; the Dockerfile it brings carries a
  // gate the templates package never mentions, and the Docker step renders it anyway.
  let dir = make_project();
  let root = dir.path();
  let site = tempfile::tempdir().unwrap();
  for entry in fs::read_dir(fixtures().join("docker")).unwrap() {
    let entry = entry.unwrap();
    fs::copy(entry.path(), site.path().join(entry.file_name())).unwrap();
  }
  let dockerfile = site.path().join("template.Dockerfile");
  let text = fs::read_to_string(&dockerfile).unwrap();
  fs::write(
    &dockerfile,
    format!("{text}# !if rust:\nRUN cargo --version\n# !end\nRUN echo gated-rendered\n"),
  )
  .unwrap();
  let runner = aeth_devkit_core::process::RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\n");
  let index = aeth_devkit_core::index::StubIndexClient { versions: vec![] };
  let mut map = std::collections::HashMap::new();
  for (name, version, dir) in [
    ("devkit_claude_hooks", "1.0.0", fixtures().join("docker")),
    ("devkit_poe_complete", "1.0.0", fixtures().join("docker")),
    ("devkit_templates", "1.0.0", fixtures()),
  ] {
    map.insert(
      name.to_string(),
      aeth_devkit_setup::packages::Installed {
        dir,
        version: version.into(),
      },
    );
  }
  let venv = LateContainer {
    base: aeth_devkit_setup::packages::StubVenv(map),
    runner: &runner,
    container: aeth_devkit_setup::packages::Installed {
      dir: site.path().to_path_buf(),
      version: "1.4.0".into(),
    },
  };
  let deps = aeth_devkit_setup::Deps {
    docker: aeth_devkit_setup::docker::Deps {
      runner: &runner,
      prompt: &aeth_devkit_core::prompt::ScriptedPrompt::new(&[]),
      reviewer: None,
      mode: aeth_devkit_setup::docker::Mode::Yes,
    },
    index: &index,
    venv: &venv,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
  assert!(ctx.has_docker, "the fixture project lists a service");
  aeth_devkit_setup::run_with(&ctx, Some(&templates()), false, &deps).unwrap();
  let rendered = read(root, "docker/Dockerfile");
  assert!(rendered.contains("RUN echo gated-rendered"), "{rendered}");
  assert!(!rendered.contains("cargo --version") && !rendered.contains("!if"), "{rendered}");
}

#[test]
fn a_pyproject_edit_that_flips_a_gate_cancels_a_committing_run() {
  // `[tool.mypy]` in the template is gated on `dep("mypy")`; adding mypy to the working copy
  // without committing it flips that gate, which the run refuses before staging anything.
  let dir = make_project();
  let root = dir.path();
  let committed = read(root, "pyproject.toml");
  git_init(root);
  git(root, &["add", "-A"]);
  git(root, &["commit", "-q", "-m", "init"]);
  let edited = committed.replacen(
    "[dependency-groups]\n  dev = [",
    "[dependency-groups]\n  dev = [\n    \"mypy>=1\",",
    1,
  );
  assert_ne!(committed, edited, "the fixture's dev group must be where this expects it");
  write(root, "pyproject.toml", &edited);
  let err = aeth_devkit_setup::cli::run(&aeth_devkit_setup::cli::Args {
    root: root.to_path_buf(),
    templates_dir: Some(templates()),
    dry_run: false,
    no_commit: false,
    yes: false,
    vscode: false,
    no_vscode: true,
  })
  .unwrap_err()
  .to_string();
  assert!(err.contains("not committed") && err.contains("dep(\"mypy\")"), "{err}");
  assert_eq!(read(root, "pyproject.toml"), edited, "nothing touched");
  assert_eq!(git(root, &["rev-list", "--count", "HEAD"]), "1");
}

#[test]
fn tool_docker_is_seeded_only_where_docker_files_exist() {
  // Docker files but no table: both keys are seeded, `services` stays empty, nothing
  // Docker-specific runs, and the reminder to list the service fires.
  let dir = make_project();
  let root = dir.path();
  write(root, "pyproject.toml", &strip_tool_docker(&read(root, "pyproject.toml")));
  write(root, "docker/Dockerfile", "FROM scratch\n");
  let changes = run(root, false).unwrap();
  let py = read(root, "pyproject.toml");
  let doc: toml_edit::DocumentMut = py.parse().unwrap();
  assert!(doc["tool"]["docker"]["services"].as_array().is_some_and(|a| a.is_empty()), "{py}");
  assert!(doc["tool"]["docker"].get("required_persisted_dirs").is_some(), "{py}");
  assert!(!py.contains("setup-project:"), "markers must not leak: {py}");
  assert_eq!(
    read(root, "docker/Dockerfile"),
    "FROM scratch\n",
    "not managed until a service is listed"
  );
  assert!(!root.join("docker/compose.yaml").exists());
  assert!(!root.join(".dockerignore").exists());
  assert!(
    changes.notes.iter().any(|n| n.contains("no service is listed")),
    "{:?}",
    changes.notes
  );
  assert!(run(root, false).unwrap().is_empty());
}

#[test]
fn a_run_without_standard_input_is_refused_unless_nothing_will_be_asked() {
  let super_run = run;
  // Only a real process can lack stdin, so this goes through the binary with the null
  // device (no answer can ever come from it). A plain run and `--no-commit` are refused
  // before touching anything (exit 2, the error exit); `-y` gets past the gate, shown
  // here on a root with no pyproject so the run fails on that instead, hermetically.
  let dir = make_project_without_docker();
  let root = dir.path();
  let before = read(root, "pyproject.toml");
  let exe = env!("CARGO_BIN_EXE_devkit-setup");
  let run = |root: &Path, flags: &[&str]| {
    let out = std::process::Command::new(exe)
      .arg("--root")
      .arg(root)
      .arg("--templates-dir")
      .arg(templates())
      .args(flags)
      .stdin(std::process::Stdio::null())
      .output()
      .unwrap();
    (out.status.code(), String::from_utf8_lossy(&out.stderr).into_owned())
  };
  for flags in [&[][..], &["--no-commit"]] {
    let (code, err) = run(root, flags);
    assert_eq!(code, Some(2), "{flags:?}: {err}");
    assert!(err.contains("no standard input") && err.contains("-y/--yes"), "{flags:?}: {err}");
  }
  assert_eq!(read(root, "pyproject.toml"), before, "nothing touched");
  let empty = tempfile::tempdir().unwrap();
  let (code, err) = run(empty.path(), &["-y"]);
  assert_eq!(code, Some(2), "{err}");
  assert!(err.contains("pyproject.toml") && !err.contains("no standard input"), "{err}");
  // A pipe is input: the same empty root fails the same way, past the gate.
  let out = std::process::Command::new(exe)
    .arg("--root")
    .arg(empty.path())
    .arg("--templates-dir")
    .arg(templates())
    .stdin(std::process::Stdio::piped())
    .output()
    .unwrap();
  let err = String::from_utf8_lossy(&out.stderr);
  assert!(err.contains("pyproject.toml") && !err.contains("no standard input"), "{err}");
  // Set up once (in-process, no network), then the headless dry run is accepted: exit 0
  // clean, and exit 0 with the drift reported after a managed file is deleted.
  super_run(root, false).unwrap();
  assert_eq!(run(root, &["--dry-run"]).0, Some(0));
  fs::remove_file(root.join(".gitignore")).unwrap();
  let out = std::process::Command::new(exe)
    .arg("--root")
    .arg(root)
    .arg("--templates-dir")
    .arg(templates())
    .arg("--dry-run")
    .stdin(std::process::Stdio::null())
    .output()
    .unwrap();
  let stdout = String::from_utf8_lossy(&out.stdout);
  assert_eq!(out.status.code(), Some(0), "{stdout}");
  assert!(
    stdout.contains("Would change:") && stdout.contains(".gitignore"),
    "drift is reported: {stdout}"
  );
  assert!(!root.join(".gitignore").exists(), "a dry run writes nothing");
}

#[test]
fn the_unlisted_services_warning_can_be_silenced() {
  let dir = make_project();
  let root = dir.path();
  let py = strip_tool_docker(&read(root, "pyproject.toml"));
  write(root, "docker/Dockerfile", "FROM scratch\n");
  // The warning and the key that quiets it name each other.
  write(root, "pyproject.toml", &py);
  let changes = run(root, false).unwrap();
  let warning = changes.notes.iter().find(|n| n.contains("no service is listed")).unwrap();
  assert!(warning.contains("silence_unlisted_services_warning = true"), "{warning}");
  // Set (with the seeded keys kept), the warning is gone and nothing else changes: the key
  // is not a switch, so the Docker step still stays off.
  write(
    root,
    "pyproject.toml",
    &read(root, "pyproject.toml").replace("[tool.docker]\n", "[tool.docker]\n  silence_unlisted_services_warning = true\n"),
  );
  let changes = run(root, false).unwrap();
  assert!(
    !changes.notes.iter().any(|n| n.contains("no service is listed")),
    "{:?}",
    changes.notes
  );
  assert!(changes.is_empty(), "{changes:?}");
  assert_eq!(read(root, "docker/Dockerfile"), "FROM scratch\n");
  assert!(!root.join(".dockerignore").exists());
  let err = {
    write(
      root,
      "pyproject.toml",
      &read(root, "pyproject.toml").replace("warning = true", "warning = \"yes\""),
    );
    run(root, false).unwrap_err().to_string()
  };
  assert!(err.contains("must be a boolean"), "{err}");
}

#[test]
fn an_unsupported_compose_shape_is_an_error_on_every_run() {
  // A listed service is a declared intent to have the compose file managed, so a shape
  // the engine cannot edit is an `error:`: the rest of the run still writes, and (see the
  // next test) the exit code says the project is not clean.
  let dir = make_project();
  let root = dir.path();
  run(root, false).unwrap();
  // An include-only aggregator is a supported layout: a warning, no error.
  write(root, "docker/compose.yaml", "include:\n  - path: other.yaml\n");
  let changes = run(root, true).unwrap();
  assert!(changes.errors.is_empty(), "{:?}", changes.errors);
  assert_eq!(changes.warnings.len(), 1, "{:?}", changes.warnings);
  // A shape the user could reformat is the error, on this run and the next.
  write(root, "docker/compose.yaml", "services: {imap-report-collector: {image: x}}\n");
  for _ in 0..2 {
    let changes = run(root, true).unwrap();
    assert!(changes.is_empty(), "no drift, only an error: {changes:?}");
    assert_eq!(changes.errors.len(), 1, "{:?}", changes.errors);
  }
}

#[test]
fn a_recorded_error_exits_1_and_a_clean_dry_run_0() {
  // The exit code carries a finding the run recorded rather than wrote (an `error:`), here
  // the stale-lock one a dry run records instead of stopping.
  let dir = make_project_without_docker();
  let root = dir.path();
  let args = aeth_devkit_setup::cli::Args {
    root: root.to_path_buf(),
    templates_dir: Some(templates()),
    dry_run: true,
    no_commit: true,
    yes: false,
    vscode: false,
    no_vscode: true,
  };
  run(root, false).unwrap();
  assert_eq!(aeth_devkit_setup::cli::run(&args).unwrap(), std::process::ExitCode::SUCCESS);
  let lock = read(root, "uv.lock");
  let running = format!("version = \"{}\"", aeth_devkit_setup::packages::RUNNING_DEVKIT);
  assert!(lock.contains(&running), "{lock}");
  write(root, "uv.lock", &lock.replacen(&running, "version = \"0.0.1\"", 1));
  assert_eq!(aeth_devkit_setup::cli::run(&args).unwrap(), std::process::ExitCode::from(1));
}

#[test]
fn replaces_legacy_poe_tasks_include_script() {
  let dir = make_project();
  let root = dir.path();
  let py = read(root, "pyproject.toml").replace("aeth_devkit:tasks", "poe_tasks:tasks");
  assert!(py.contains("poe_tasks:tasks"), "fixture should start with the legacy include");
  write(root, "pyproject.toml", &py);

  run(root, false).unwrap();
  let out = read(root, "pyproject.toml");
  assert!(!out.contains("poe_tasks:tasks"), "{out}");
  let code: String = out.lines().map(|l| l.split('#').next().unwrap_or("")).collect();
  assert_eq!(code.matches("aeth_devkit:tasks").count(), 1, "{out}");
  assert!(out.contains("include_script = [{ script"), "no stray space after '[': {out}");
}

#[test]
fn agents_md_gets_a_managed_block_and_keeps_project_text() {
  let dir = make_project();
  let root = dir.path();
  write(root, "AGENTS.md", "# My Project\n\nProject-specific notes.\n");

  run(root, false).unwrap();
  let agents = read(root, "AGENTS.md");
  assert!(agents.starts_with("# My Project\n\nProject-specific notes.\n"), "{agents}");
  assert!(
    agents.contains("<!-- devkit:begin -->") && agents.contains("<!-- devkit:end -->"),
    "{agents}"
  );
  assert!(agents.contains("## Environment"), "{agents}");
  assert!(!agents.contains("if-dep"), "markers must not leak: {agents}");
  let has_aeth_ext = read(root, "pyproject.toml").contains("aeth-ext");
  assert_eq!(agents.contains("## Pydantic Dataclass Conventions"), has_aeth_ext, "{agents}");

  let again = run(root, false).unwrap();
  assert!(
    !again.files.iter().any(|f| f.path.ends_with("AGENTS.md")),
    "second run must not touch AGENTS.md: {}",
    again.report(root)
  );
}

#[test]
fn claude_config_files_are_created_and_create_if_missing_ones_are_never_rewritten() {
  let dir = make_project();
  let root = dir.path();
  write(root, ".claude/CLAUDE.md", "my own claude notes\n");
  write(root, ".github/workflows/claude.yml", "name: mine\n");

  let changes = run(root, false).unwrap();
  let report = changes.report(root);
  for rel in [".claude/settings.json", ".claude/settings.local.json", ".mcp.json"] {
    assert!(report.contains(&format!("{rel}: created")), "{report}");
  }
  assert_eq!(read(root, ".claude/CLAUDE.md"), "my own claude notes\n");
  assert_eq!(read(root, ".github/workflows/claude.yml"), "name: mine\n");

  let shared: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.json")).unwrap();
  let local: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.local.json")).unwrap();
  let cmd = local["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
  assert!(cmd.ends_with("devkit-hook stop-ruff"), "{cmd}");
  assert!(cmd.starts_with("uv run devkit-hook"), "no venv in fixture → uv fallback: {cmd}");
  assert!(local["env"]["PYTHONPYCACHEPREFIX"].as_str().unwrap().contains(".cache"));
  // Nothing machine-specific may reach the committed half.
  assert!(shared.get("hooks").is_none(), "hooks belong in the local half: {shared}");
  assert!(shared.get("env").is_none(), "env belongs in the local half: {shared}");
  assert_eq!(shared["enabledMcpjsonServers"], serde_json::json!(["github", "context7"]));

  let again = run(root, false).unwrap();
  assert!(again.is_empty(), "second run must be a no-op:\n{}", again.report(root));
}

#[test]
fn the_committed_settings_carry_no_absolute_or_os_specific_path() {
  // This is the whole point of the split: `settings.json` is shared, so a path from the
  // machine that ran setup would break every teammate whose clone lives elsewhere or who
  // runs a different OS.
  let dir = make_project();
  let root = dir.path();
  write(root, ".venv/Scripts/devkit-hook.exe", "");
  run(root, false).unwrap();

  let shared = read(root, ".claude/settings.json");
  let root_str = root.to_string_lossy().replace('\\', "/");
  assert!(!shared.contains(&root_str), "absolute path leaked into the shared file:\n{shared}");
  for os_specific in [".venv/Scripts", ".venv/bin", ".exe"] {
    assert!(
      !shared.contains(os_specific),
      "{os_specific} leaked into the shared file:\n{shared}"
    );
  }
  // The local half is where those belong, and it is ignored by the shipped gitignore.
  let local = read(root, ".claude/settings.local.json");
  assert!(local.contains(".venv/Scripts/devkit-hook.exe"), "{local}");
  assert!(read(root, ".gitignore").contains(".claude/settings.local.json"));
}

#[test]
fn claude_md_and_workflow_are_created_when_missing_and_hook_bin_prefers_the_venv() {
  let dir = make_project();
  let root = dir.path();
  write(root, ".venv/Scripts/devkit-hook.exe", "");

  run(root, false).unwrap();
  assert!(read(root, ".claude/CLAUDE.md").starts_with("@../AGENTS.md\n"));
  assert!(read(root, ".github/workflows/claude.yml").contains("claude-code-action@v1"));
  let local: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.local.json")).unwrap();
  let cmd = local["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
  assert_eq!(cmd, "\"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit-hook.exe\" stop-ruff");
}

#[test]
fn pyproject_gets_sister_src_globs_future_annotations_ban_and_google_docstrings() {
  let dir = make_project();
  let root = dir.path();
  run(root, false).unwrap();
  let py = read(root, "pyproject.toml");
  let doc: toml_edit::DocumentMut = py.parse().unwrap();
  let ruff = &doc["tool"]["ruff"];
  let src: Vec<&str> = ruff["src"].as_array().unwrap().iter().filter_map(|v| v.as_str()).collect();
  assert!(src.contains(&"../*/src") && src.contains(&"../*/python"), "{src:?}");
  let select: Vec<&str> = ruff["lint"]["extend-select"]
    .as_array()
    .unwrap()
    .iter()
    .filter_map(|v| v.as_str())
    .collect();
  assert!(select.contains(&"TID") && select.contains(&"D"), "{select:?}");
  let msg = ruff["lint"]["flake8-tidy-imports"]["banned-api"]["__future__.annotations"]["msg"]
    .as_str()
    .unwrap();
  assert!(msg.contains("PEP 649"), "{msg}");
  assert_eq!(ruff["lint"]["pydocstyle"]["convention"].as_str(), Some("google"));
}

/// The fixture pyproject with its `[tool.docker]` table removed.
fn strip_tool_docker(py: &str) -> String {
  let mut doc: toml_edit::DocumentMut = py.parse().unwrap();
  doc["tool"].as_table_mut().unwrap().remove("docker");
  doc.to_string()
}

#[test]
fn docker_less_project_gets_no_tool_docker_and_no_dockerignore() {
  let dir = make_project();
  let root = dir.path();
  write(root, "pyproject.toml", &strip_tool_docker(&read(root, "pyproject.toml")));
  run(root, false).unwrap();
  assert!(!read(root, "pyproject.toml").contains("[tool.docker]"));
  assert!(!root.join(".dockerignore").exists());
}

fn git(root: &Path, args: &[&str]) -> String {
  let out = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
  assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
  String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_init(root: &Path) {
  for args in [&["init", "-q"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
    git(root, args);
  }
}

#[test]
fn obsolete_artifacts_are_reported_not_removed() {
  let dir = make_project();
  let root = dir.path();
  write(root, ".github/copilot-instructions.md", "old\n");
  assert!(
    read(root, "pyproject.toml").contains("[tool.docker]"),
    "fixture is expected to carry the table"
  );

  let changes = run(root, false).unwrap();
  assert!(
    changes.notes.iter().any(|n| n.contains("copilot-instructions.md")),
    "{:?}",
    changes.notes
  );
  assert!(changes.notes.iter().any(|n| n.contains("[tool.docker]")), "{:?}", changes.notes);
  assert_eq!(read(root, ".github/copilot-instructions.md"), "old\n");
  assert!(read(root, "pyproject.toml").contains("[tool.docker]"));
}

#[test]
fn a_gitignored_managed_file_is_reported_and_the_gitignore_is_left_alone() {
  // devkit used to append `!` negations here. Reversing a rule the project chose is the
  // user's call, and doing it correctly under a directory rule means un-ignoring the whole
  // directory — so it now describes the situation instead of editing the file.
  let dir = make_project();
  let root = dir.path();
  git_init(root);
  let before = read(root, ".gitignore");
  write(root, ".gitignore", &format!("{before}\n# project rule\n*.json\n"));

  let changes = run(root, false).unwrap();
  // Step 6 still merges the shipped template into .gitignore — that is the project opting
  // in. What must never appear is a negation reversing a rule the project wrote itself.
  let gi = read(root, ".gitignore");
  for negation in ["!.mcp.json", "!.claude/", "!.vscode/settings.json", "!.env"] {
    assert!(
      !gi.contains(negation),
      "devkit must not un-ignore on the user's behalf: {negation}\n{gi}"
    );
  }
  assert!(
    changes.notes.iter().any(|n| n.contains(".mcp.json") && n.contains("gitignored")),
    "{:?}",
    changes.notes
  );
  // Files that are meant to be ignored are never warned about.
  for quiet in [".env", "settings.local.json"] {
    assert!(!changes.notes.iter().any(|n| n.contains(quiet)), "{quiet}: {:?}", changes.notes);
  }
}

#[test]
fn an_ignored_parent_directory_is_named_as_the_cause() {
  // The fix differs: a `!<file>` line does nothing while a parent directory is ignored,
  // because git never descends into one. Saying so is the whole value of the warning.
  let dir = make_project();
  let root = dir.path();
  git_init(root);
  let gi = read(root, ".gitignore");
  write(root, ".gitignore", &format!("{gi}\n# project rule\n.claude/\n"));

  let changes = run(root, false).unwrap();
  let note = changes
    .notes
    .iter()
    .find(|n| n.contains(".claude/settings.json"))
    .unwrap_or_else(|| panic!("no note for the ignored file: {:?}", changes.notes));
  assert!(note.contains(".claude/"), "must name the directory: {note}");
  assert!(note.contains("does not look inside"), "must explain why: {note}");
}

#[test]
fn a_gitignore_tightened_after_setup_is_still_reported() {
  // The check used to run over the files this run *changed*. Once setup has succeeded a
  // later run changes nothing, so a project that tightens its .gitignore afterwards would
  // never hear about it again.
  let dir = make_project();
  let root = dir.path();
  git_init(root);
  run(root, false).unwrap();

  // Now the project tightens its own .gitignore, after everything is already in place.
  let gi = read(root, ".gitignore");
  write(root, ".gitignore", &format!("{gi}\n.claude/\n"));
  let changes = run(root, false).unwrap();

  assert!(
    changes.notes.iter().any(|n| n.contains(".claude/settings.json")),
    "a later run must still warn: {:?}",
    changes.notes
  );
}

#[test]
fn dry_run_reports_exactly_what_a_real_run_writes() {
  // Two identical projects: one inspected, one applied. The change sets must agree, and the
  // dry run must leave the tree byte-for-byte as it found it.
  let a = make_project();
  let b = make_project();
  git_init(a.path());
  git_init(b.path());

  fn snapshot(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
      for e in fs::read_dir(&d).unwrap().flatten() {
        let p = e.path();
        if p.file_name().is_some_and(|n| n == ".git") {
          continue;
        }
        if p.is_dir() {
          stack.push(p);
        } else {
          let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
          out.push((rel, fs::read(&p).unwrap()));
        }
      }
    }
    out.sort();
    out
  }

  let before = snapshot(a.path());
  let dry = run(a.path(), true).unwrap();
  assert_eq!(snapshot(a.path()), before, "--dry-run must not write anything");

  let real = run(b.path(), false).unwrap();

  fn rels(c: &aeth_devkit_setup::changes::Changes, root: &Path) -> Vec<String> {
    // `run` records paths under the *canonicalized* root, which on Windows need not be the
    // spelling `tempdir()` handed us — so strip against the canonical form, or the fallback
    // silently compares absolute paths that can never match between two temp dirs.
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let root = aeth_devkit_core::paths::strip_verbatim(root);
    let mut v: Vec<String> = c
      .files
      .iter()
      .map(|f| {
        f.path
          .strip_prefix(&root)
          .unwrap_or_else(|_| panic!("recorded path outside the project root: {}", f.path.display()))
          .to_string_lossy()
          .replace('\\', "/")
      })
      .collect();
    v.sort();
    v
  }
  assert_eq!(
    rels(&dry, a.path()),
    rels(&real, b.path()),
    "dry run and apply must agree on the file list"
  );
}

#[test]
fn commit_works_when_the_caller_spells_the_root_differently() {
  // `run` records every path under the *canonicalized* root, which need not match the
  // spelling a caller passes to `commit_changes`. On Windows CI that difference is real
  // (8.3 short names, drive-letter case, a `\?\` prefix); here it is reproduced with a
  // redundant `sub/..` segment, which `strip_prefix` will not normalise but
  // `canonicalize` will. A literal comparison drops every file and commits nothing.
  let dir = make_project();
  let root = dir.path();
  git_init(root);
  let out = std::process::Command::new("git")
    .current_dir(root)
    .args(["add", "-A"])
    .output()
    .unwrap();
  assert!(out.status.success());
  let out = std::process::Command::new("git")
    .current_dir(root)
    .args(["commit", "-q", "-m", "init"])
    .output()
    .unwrap();
  assert!(out.status.success());

  let odd_root = root.join("src").join("..");
  let mut bases = aeth_devkit_setup::git::stage_bases(&odd_root).unwrap();
  let changes = run(root, false).unwrap();
  let hash = aeth_devkit_setup::git::commit_changes(&odd_root, &changes, &mut bases).unwrap();
  assert!(hash.is_some(), "a differently-spelled root must still commit the managed files");
}

#[test]
fn release_workflow_is_installed_and_replaced_on_drift() {
  let dir = make_project();
  let root = dir.path();
  let changes = run(root, false).unwrap();
  let wf = read(root, ".github/workflows/release.yml");
  // The fixture is a pure-Python project with one publish index (SFTPyPI).
  assert!(wf.contains("uv publish --index SFTPyPI dist/*"), "{wf}");
  assert!(wf.contains("secrets.UV_INDEX_SFTPYPI_USERNAME"), "{wf}");
  assert!(!wf.contains("trusted-publishing") && !wf.contains("id-token"), "{wf}");
  assert!(!wf.contains("setup-project:"), "markers leaked:\n{wf}");
  assert!(!wf.contains("maturin"), "pure-Python project got the Rust variant:\n{wf}");
  assert!(
    changes
      .notes
      .iter()
      .any(|n| n.contains("UV_INDEX_SFTPYPI_USERNAME") && n.contains("UV_INDEX_SFTPYPI_PASSWORD")),
    "{:?}",
    changes.notes
  );

  // Devkit-owned: a hand edit is put back, and it counts as a change a dry run reports.
  write(root, ".github/workflows/release.yml", "name: mine\n");
  let changes = run(root, true).unwrap();
  assert!(changes.files.iter().any(|f| f.path.ends_with("release.yml")), "{:?}", changes.files);
  assert_eq!(
    read(root, ".github/workflows/release.yml"),
    "name: mine\n",
    "dry run must not write"
  );
  run(root, false).unwrap();
  assert_eq!(read(root, ".github/workflows/release.yml"), wf);
  // The secrets note is for the first install only.
  let again = run(root, false).unwrap();
  assert!(again.notes.iter().all(|n| !n.contains("UV_INDEX_")), "{:?}", again.notes);

  // Replacing a workflow the project wrote itself introduces the credential requirement
  // just like an install into an empty project, so the note is printed again.
  write(root, ".github/workflows/release.yml", "name: mine\n");
  let replaced = run(root, false).unwrap();
  assert!(
    replaced.notes.iter().any(|n| n.contains("UV_INDEX_SFTPYPI_USERNAME")),
    "{:?}",
    replaced.notes
  );
}

#[test]
fn release_workflow_uses_pypi_when_no_index_publishes() {
  let dir = make_project();
  let root = dir.path();
  let py = read(root, "pyproject.toml").replace("publish-url", "x-publish-url");
  write(root, "pyproject.toml", &py);
  let changes = run(root, false).unwrap();
  let wf = read(root, ".github/workflows/release.yml");
  assert!(wf.contains("uv publish --trusted-publishing always dist/*"), "{wf}");
  assert!(wf.contains("id-token: write"), "{wf}");
  assert!(!wf.contains("uv publish --index") && !wf.contains("setup-project:"), "{wf}");
  assert!(
    changes
      .notes
      .iter()
      .any(|n| n.contains("trusted publisher") && n.contains("release.yml")),
    "{:?}",
    changes.notes
  );
}

#[test]
fn rust_projects_get_the_maturin_matrix_workflow() {
  let dir = make_project();
  let root = dir.path();
  write(root, "Cargo.toml", "[package]\nname = \"x\"\nversion = \"0.1.0\"\n");
  run(root, false).unwrap();
  let wf = read(root, ".github/workflows/release.yml");
  assert!(wf.contains("PyO3/maturin-action@v1"), "{wf}");
  assert!(
    wf.contains("x86_64-pc-windows-msvc") && wf.contains("x86_64-unknown-linux-gnu"),
    "{wf}"
  );
  assert!(wf.contains("needs: [build, sdist]"), "{wf}");
  assert!(
    wf.contains("uv publish --index SFTPyPI dist/*") && !wf.contains("setup-project:"),
    "{wf}"
  );
}

#[test]
fn a_committing_run_resyncs_the_venv_to_the_lock_the_user_gets_back() {
  // The run locks and syncs against HEAD's copy of uv.lock; a lock the user had edited but
  // not committed comes back after the replay, and the venv must follow it rather than
  // the copy the run worked on. With a clean lock the one sync is the right one.
  let devkit_only = format!(
    "version = 1\n\n[[package]]\nname = \"aeth-devkit\"\nversion = \"{}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n",
    aeth_devkit_setup::packages::RUNNING_DEVKIT
  );
  let users_extra = "[[package]]\nname = \"requests\"\nversion = \"2.32.0\"\nsource = { registry = \"https://idx/+simple\" }\n\n";
  for (user_edit, syncs) in [(false, 1), (true, 2)] {
    let dir = make_project();
    let root = dir.path();
    // HEAD's lock predates the container; the recorded `uv lock` adds it, as uv would.
    let with_container = read(root, "uv.lock");
    write(root, "uv.lock", &devkit_only);
    git_init(root);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    if user_edit {
      // Their package sits above devkit's block, so the replay merges cleanly.
      write(
        root,
        "uv.lock",
        &devkit_only.replace("[[package]]", &format!("{users_extra}[[package]]")),
      );
    }
    let runner = aeth_devkit_core::process::RecordingRunner::new(0);
    runner.script("gh", &["api"], 0, "v1.1.0\n");
    let lock_after = with_container.clone();
    runner.script_with_effect("uv", &["lock"], 0, move |cwd| {
      std::fs::write(cwd.join("uv.lock"), &lock_after).unwrap()
    });
    let index = aeth_devkit_core::index::StubIndexClient { versions: vec![] };
    let mut map = std::collections::HashMap::new();
    for (name, version) in [
      ("devkit_container", "1.4.0"),
      ("devkit_claude_hooks", "1.0.0"),
      ("devkit_poe_complete", "1.0.0"),
    ] {
      map.insert(
        name.to_string(),
        aeth_devkit_setup::packages::Installed {
          dir: fixtures().join("docker"),
          version: version.into(),
        },
      );
    }
    let venv = aeth_devkit_setup::packages::StubVenv(map);
    let deps = aeth_devkit_setup::Deps {
      docker: aeth_devkit_setup::docker::Deps {
        runner: &runner,
        prompt: &aeth_devkit_core::prompt::ScriptedPrompt::new(&[]),
        reviewer: None,
        mode: aeth_devkit_setup::docker::Mode::Yes,
      },
      index: &index,
      venv: &venv,
    };
    let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
    let mut bases = aeth_devkit_setup::git::stage_bases(root).unwrap();
    let changes = aeth_devkit_setup::run_with(&ctx, Some(&templates()), false, &deps).unwrap();
    assert!(changes.venv_synced, "the lock moved, so the run synced");
    let committed = aeth_devkit_setup::git::commit_changes(root, &changes, &mut bases);
    aeth_devkit_setup::packages::resync_after_replay(root, &runner, &bases, &changes);
    assert!(committed.unwrap().is_some());
    let lock = read(root, "uv.lock");
    assert_eq!(lock.contains("name = \"requests\""), user_edit, "{lock}");
    assert!(lock.contains("name = \"devkit-container\""), "{lock}");
    let sync_calls = runner.calls_for("uv").iter().filter(|c| c[0] == "sync").count();
    assert_eq!(sync_calls, syncs, "user_edit={user_edit}: {:?}", runner.calls_for("uv"));
  }
}

#[test]
fn a_project_that_opts_out_keeps_its_own_release_workflow() {
  // The extension repository releases a vsix from a workflow of its own at the same path;
  // with the switch off, setup-project neither replaces it nor announces publishing secrets.
  let dir = make_project();
  let root = dir.path();
  let own = "name: Release\non:\n  push:\n    tags: [\"v*\"]\n";
  write(root, ".github/workflows/release.yml", own);
  let py = read(root, "pyproject.toml");
  write(
    root,
    "pyproject.toml",
    &format!("{py}\n[tool.devkit]\n  release-workflow = false\n"),
  );
  let changes = run(root, false).unwrap();
  assert_eq!(read(root, ".github/workflows/release.yml"), own);
  assert!(!changes.notes.iter().any(|n| n.contains("release workflow")), "{:?}", changes.notes);
  assert!(!changes.managed.iter().any(|p| p.ends_with("release.yml")), "{:?}", changes.managed);
  // The pyproject merge must carry the switch through, or the second run would replace the file.
  assert!(read(root, "pyproject.toml").contains("release-workflow = false"));
  let again = run(root, false).unwrap();
  assert!(
    again.is_empty(),
    "second run should be a no-op, got:
{}",
    again.report(root)
  );
  assert_eq!(read(root, ".github/workflows/release.yml"), own);
}

#[test]
fn the_venv_path_bootstraps_the_templates_package_and_renders_the_same_files() {
  let via_venv = make_project();
  let via_override = make_project();
  let changes = run_from_venv(via_venv.path(), false).unwrap();
  let reference = run(via_override.path(), false).unwrap();
  let py = read(via_venv.path(), "pyproject.toml");
  assert!(py.contains("\"devkit-templates>=1.0.0\""), "{py}");
  assert!(py.contains("devkit-templates = [{ index = \"SFTPyPI\" }]"), "{py}");
  let entries = changes.files.iter().filter(|f| f.path.ends_with("pyproject.toml")).count();
  assert_eq!(entries, 1, "one pyproject.toml entry, the bootstrap's details merged into it");
  // Every other rendered file is what the override renders.
  // The records carry the discovered root (canonical: the runner's TEMP is an 8.3 name).
  let reference_root = aeth_devkit_setup::context::strip_verbatim(via_override.path().canonicalize().unwrap());
  let rendered: Vec<String> = reference
    .files
    .iter()
    .map(|f| f.path.strip_prefix(&reference_root).unwrap().to_string_lossy().replace('\\', "/"))
    .collect();
  assert!(rendered.len() > 5, "{rendered:?}");
  // The root is substituted into a few files (`{project_root}`), in either slash form.
  let normalized = |root: &Path, rel: &str| {
    let prefix = aeth_devkit_setup::context::strip_verbatim(root.canonicalize().unwrap())
      .display()
      .to_string();
    // Plain, forward-slashed (`.env`) and JSON-escaped (the Claude settings) spellings.
    let json = prefix.replace('\\', "\\\\");
    read(root, rel)
      .replace(&json, "<root>")
      .replace(&prefix, "<root>")
      .replace(&prefix.replace('\\', "/"), "<root>")
  };
  for rel in rendered.iter().filter(|r| *r != "pyproject.toml") {
    assert_eq!(normalized(via_venv.path(), rel), normalized(via_override.path(), rel), "{rel}");
  }
  // The floor line and the source line aside, the pyproject is byte for byte the same
  // (tombi lays the file out at the end of a real run; the harness does not run it).
  let floor = "\n    \"devkit-templates>=1.0.0\",";
  let source = "\ndevkit-templates = [{ index = \"SFTPyPI\" }]";
  assert!(py.contains(floor) && py.contains(source), "{py}");
  let reference_py = read(via_override.path(), "pyproject.toml");
  assert_eq!(py.replacen(floor, "", 1).replacen(source, "", 1), reference_py);
  let again = run_from_venv(via_venv.path(), false).unwrap();
  assert!(again.is_empty(), "idempotent: {}", again.report(via_venv.path()));
}

#[test]
fn the_env_override_renders_without_a_venv_and_must_be_a_directory() {
  let dir = make_project_without_docker();
  let root = dir.path();
  run(root, false).unwrap();
  fs::remove_file(root.join(".gitignore")).unwrap();
  let exe = env!("CARGO_BIN_EXE_devkit-setup");
  let out = std::process::Command::new(exe)
    .arg("--root")
    .arg(root)
    .args(["--dry-run", "--no-vscode"])
    .env("DEVKIT_TEMPLATES", templates())
    .stdin(std::process::Stdio::null())
    .output()
    .unwrap();
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
  assert!(stdout.contains("Would change:") && stdout.contains(".gitignore"), "{stdout}");
  assert!(!root.join(".gitignore").exists(), "a dry run writes nothing");
  let out = std::process::Command::new(exe)
    .arg("--root")
    .arg(root)
    .args(["--dry-run", "--no-vscode"])
    .env("DEVKIT_TEMPLATES", root.join("pyproject.toml"))
    .stdin(std::process::Stdio::null())
    .output()
    .unwrap();
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert_eq!(out.status.code(), Some(2), "{stderr}");
  assert!(
    stderr.contains("DEVKIT_TEMPLATES") && stderr.contains("not a directory"),
    "{stderr}"
  );
}

#[test]
fn a_committing_run_bootstraps_the_templates_package_once_and_replays_the_users_edit() {
  let dir = make_project();
  let root = dir.path();
  // HEAD's lock predates the package; the recorded `uv lock` adds it, as uv would.
  let with_templates = read(root, "uv.lock");
  let templates_entry =
    "\n[[package]]\nname = \"devkit-templates\"\nversion = \"1.0.0\"\nsource = { registry = \"https://idx/+simple\" }\n";
  assert!(with_templates.contains(templates_entry));
  write(root, "uv.lock", &with_templates.replace(templates_entry, ""));
  git_init(root);
  git(root, &["add", "-A"]);
  git(root, &["commit", "-q", "-m", "init"]);
  // An unrelated uncommitted edit, at the top where nothing the run writes lands.
  let head_py = read(root, "pyproject.toml");
  write(root, "pyproject.toml", &format!("# the user's note\n{head_py}"));
  let runner = aeth_devkit_core::process::RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\n");
  runner.script_with_effect("uv", &["lock"], 0, move |cwd| {
    std::fs::write(cwd.join("uv.lock"), &with_templates).unwrap()
  });
  let index = aeth_devkit_core::index::StubIndexClient { versions: vec![] };
  let mut map = std::collections::HashMap::new();
  for (name, version, dir) in [
    ("devkit_container", "1.4.0", fixtures().join("docker")),
    ("devkit_claude_hooks", "1.0.0", fixtures().join("docker")),
    ("devkit_poe_complete", "1.0.0", fixtures().join("docker")),
    ("devkit_templates", "1.0.0", fixtures()),
  ] {
    map.insert(
      name.to_string(),
      aeth_devkit_setup::packages::Installed {
        dir,
        version: version.into(),
      },
    );
  }
  let venv = aeth_devkit_setup::packages::StubVenv(map);
  let deps = aeth_devkit_setup::Deps {
    docker: aeth_devkit_setup::docker::Deps {
      runner: &runner,
      prompt: &aeth_devkit_core::prompt::ScriptedPrompt::new(&[]),
      reviewer: None,
      mode: aeth_devkit_setup::docker::Mode::Yes,
    },
    index: &index,
    venv: &venv,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
  let mut bases = aeth_devkit_setup::git::stage_bases(root).unwrap();
  let changes = aeth_devkit_setup::run_with(&ctx, None, false, &deps).unwrap();
  let committed = aeth_devkit_setup::git::commit_changes(root, &changes, &mut bases);
  aeth_devkit_setup::packages::resync_after_replay(root, &runner, &bases, &changes);
  assert!(committed.unwrap().is_some());
  assert_eq!(changes.files.iter().filter(|f| f.path.ends_with("pyproject.toml")).count(), 1);
  let head = git(root, &["show", "HEAD:pyproject.toml"]);
  assert_eq!(head.matches("\"devkit-templates>=1.0.0\"").count(), 1, "{head}");
  assert_eq!(head.matches("devkit-templates = [{ index = \"SFTPyPI\" }]").count(), 1, "{head}");
  assert!(!head.contains("the user's note"), "the edit stays out of the commit");
  assert!(git(root, &["show", "HEAD:uv.lock"]).contains("name = \"devkit-templates\""));
  let py = read(root, "pyproject.toml");
  assert!(py.starts_with("# the user's note\n"), "the edit is back: {py}");
  assert_eq!(py.matches("\"devkit-templates>=1.0.0\"").count(), 1, "{py}");
  // The edit is back as an unstaged change, and the lock went into the commit.
  let status = git(root, &["status", "--short"]);
  assert!(status.contains("M pyproject.toml") && !status.contains("uv.lock"), "{status}");
}

#[test]
fn the_pyproject_setting_renders_without_a_flag_or_the_env_var() {
  let dir = make_project_without_docker();
  let root = dir.path();
  run(root, false).unwrap();
  // Absolute, so a join with the root yields it as is; forward slashes need no escaping.
  let tpl = templates().to_string_lossy().replace('\\', "/");
  let py = read(root, "pyproject.toml");
  write(root, "pyproject.toml", &format!("{py}\n[tool.devkit]\ntemplates-dir = \"{tpl}\"\n"));
  fs::remove_file(root.join(".gitignore")).unwrap();
  let exe = env!("CARGO_BIN_EXE_devkit-setup");
  let out = std::process::Command::new(exe)
    .arg("--root")
    .arg(root)
    .args(["--dry-run", "--no-vscode"])
    .env_remove("DEVKIT_TEMPLATES")
    .stdin(std::process::Stdio::null())
    .output()
    .unwrap();
  let stdout = String::from_utf8_lossy(&out.stdout);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
  assert!(stdout.contains("Would change:") && stdout.contains(".gitignore"), "{stdout}");
  // The report lists a changed file as `<path>: <verb>`; notes may mention the file too.
  assert!(
    !stdout.lines().any(|l| l.starts_with("pyproject.toml")),
    "the setting is the project's, left alone: {stdout}"
  );
}
