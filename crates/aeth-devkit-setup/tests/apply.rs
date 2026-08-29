//! End-to-end: apply the real templates to fixture projects, check outcomes and idempotency.

use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn templates() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("..")
    .join("python")
    .join("aeth_devkit")
    .join("templates")
}

fn write(root: &Path, rel: &str, content: &str) {
  let p = root.join(rel);
  fs::create_dir_all(p.parent().unwrap()).unwrap();
  fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
  fs::read_to_string(root.join(rel)).unwrap()
}

/// A project resembling IMAPReportCollector with aeth_ext's VS Code files.
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
  write(root, "docker/Dockerfile", "FROM scratch\n");
  dir
}

#[test]
fn applies_and_is_idempotent() {
  let dir = make_project();
  let root = dir.path();

  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
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
  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(again.is_empty(), "second run should be a no-op, got:\n{}", again.report(root));
}

#[test]
fn dry_run_writes_nothing() {
  let dir = make_project();
  let root = dir.path();
  let before = read(root, "pyproject.toml");
  let changes = aeth_devkit_setup::run(root, &templates(), true).unwrap();
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
  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let gi = read(root, ".gitignore");
  assert!(!gi.contains("project-specific"), "{gi}");
  assert!(gi.starts_with("# Byte-compiled"));
  let py = read(root, "pyproject.toml");
  assert!(py.contains("[tool.mypy]"), "{py}");
  assert!(py.contains("source_pkgs = [\"demo_app\"]"), "{py}");
  assert!(root.join(".vscode/launch.json").is_file());
  assert!(!root.join(".dockerignore").exists(), "no docker setup → no .dockerignore");
  assert!(!changes.is_empty());
  assert!(aeth_devkit_setup::run(root, &templates(), false).unwrap().is_empty());
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
  aeth_devkit_setup::run(root, &templates(), false).unwrap();

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
  assert!(aeth_devkit_setup::run(root, &templates(), false).unwrap().is_empty());
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
  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(read(root, "pyproject.toml").contains("src       = [\"./src\", \"../*/src\", \"../*/python\"]"));
  assert!(!read(root, ".vscode/extensions.json").contains("rust-analyzer"));
  assert!(!read(root, ".vscode/settings.json").contains("[rust]"));
}

#[test]
fn commits_only_changed_trackable_files_in_a_git_repo() {
  let dir = make_project();
  let root = dir.path();
  let git = |args: &[&str]| {
    let out = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
  };
  git(&["init", "-q"]);
  git(&["config", "user.email", "t@t"]);
  git(&["config", "user.name", "t"]);
  git(&["add", "-A"]);
  git(&["commit", "-q", "-m", "init"]);
  // Something the user staged but that setup-project must not sweep into its commit.
  write(root, "unrelated.txt", "x\n");
  git(&["add", "unrelated.txt"]);

  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(aeth_devkit_setup::git::is_git_tracked(root));
  let hash = aeth_devkit_setup::git::commit_changes(root, &changes).unwrap();
  assert!(hash.is_some());

  let subject = git(&["log", "-1", "--format=%s"]);
  assert_eq!(subject, aeth_devkit_setup::git::COMMIT_SUBJECT);
  let committed = git(&["show", "--name-only", "--format=", "HEAD"]);
  assert!(committed.contains("pyproject.toml"), "{committed}");
  assert!(committed.contains(".vscode/settings.json"), "{committed}");
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
  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(aeth_devkit_setup::git::commit_changes(root, &again).unwrap().is_none());
}

#[test]
fn replaces_legacy_poe_tasks_include_script() {
  let dir = make_project();
  let root = dir.path();
  let py = read(root, "pyproject.toml").replace("aeth_devkit:tasks", "poe_tasks:tasks");
  assert!(py.contains("poe_tasks:tasks"), "fixture should start with the legacy include");
  write(root, "pyproject.toml", &py);

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
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

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
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

  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
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

  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let report = changes.report(root);
  for rel in [".claude/settings.json", ".mcp.json"] {
    assert!(report.contains(&format!("{rel}: created")), "{report}");
  }
  assert_eq!(read(root, ".claude/CLAUDE.md"), "my own claude notes\n");
  assert_eq!(read(root, ".github/workflows/claude.yml"), "name: mine\n");

  let settings: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.json")).unwrap();
  let cmd = settings["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
  assert!(cmd.ends_with(" hook stop-ruff"), "{cmd}");
  assert!(cmd.starts_with("uv run devkit"), "no venv in fixture → uv fallback: {cmd}");
  assert!(settings.get("enabledPlugins").is_none());
  assert!(settings["env"]["PYTHONPYCACHEPREFIX"].as_str().unwrap().contains(".cache"));

  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(again.is_empty(), "second run must be a no-op:\n{}", again.report(root));
}

#[test]
fn claude_md_and_workflow_are_created_when_missing_and_devkit_bin_prefers_the_venv() {
  let dir = make_project();
  let root = dir.path();
  write(root, ".venv/Scripts/devkit.exe", "");

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(read(root, ".claude/CLAUDE.md").starts_with("@../AGENTS.md\n"));
  assert!(read(root, ".github/workflows/claude.yml").contains("claude-code-action@v1"));
  let settings: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.json")).unwrap();
  let cmd = settings["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
  assert_eq!(cmd, "\"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit.exe\" hook pre-edit-protect");
}

#[test]
fn pyproject_gets_sister_src_globs_future_annotations_ban_and_google_docstrings() {
  let dir = make_project();
  let root = dir.path();
  aeth_devkit_setup::run(root, &templates(), false).unwrap();
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
  fs::remove_dir_all(root.join("docker")).unwrap();
  write(root, "pyproject.toml", &strip_tool_docker(&read(root, "pyproject.toml")));
  // A stray empty `docker/` dir must not count either.
  fs::create_dir_all(root.join("docker")).unwrap();
  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(!read(root, "pyproject.toml").contains("[tool.docker]"));
  assert!(!root.join(".dockerignore").exists());
}

fn git_init(root: &Path) {
  for args in [&["init", "-q"][..], &["config", "user.email", "t@t"], &["config", "user.name", "t"]] {
    let out = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
  }
}

#[test]
fn gitignored_managed_files_get_negation_lines_appended() {
  let dir = make_project();
  let root = dir.path();
  git_init(root);
  let gi = read(root, ".gitignore");
  write(root, ".gitignore", &format!("{gi}\n# project rule\n*.json\n"));

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let gi = read(root, ".gitignore");
  assert!(gi.contains("\n!.claude/settings.json\n"), "{gi}");
  assert!(gi.contains("\n!.mcp.json\n"), "{gi}");
  assert!(gi.contains("\n!.vscode/settings.json\n"), "{gi}");
  assert!(!gi.contains("!.env"), ".env must stay ignored: {gi}");
  let ignored = std::process::Command::new("git")
    .current_dir(root)
    .args(["check-ignore", "-q", ".mcp.json"])
    .status()
    .unwrap();
  assert!(!ignored.success(), ".mcp.json must no longer be ignored");

  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(again.is_empty(), "second run must be a no-op:\n{}", again.report(root));
}

#[test]
fn gitignored_parent_directory_is_un_ignored_before_the_file() {
  let dir = make_project();
  let root = dir.path();
  git_init(root);
  let gi = read(root, ".gitignore");
  write(root, ".gitignore", &format!("{gi}
# project rule
.claude/
"));

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let gi = read(root, ".gitignore");
  let dir_at = gi.find("
!.claude/
").unwrap_or_else(|| panic!("missing !.claude/ in:
{gi}"));
  let file_at = gi.find("
!.claude/settings.json
").unwrap_or_else(|| panic!("missing file rule in:
{gi}"));
  assert!(dir_at < file_at, "directory negation must precede the file negation:
{gi}");
  let ignored = std::process::Command::new("git")
    .current_dir(root)
    .args(["check-ignore", "-q", ".claude/settings.json"])
    .status()
    .unwrap();
  assert!(!ignored.success(), ".claude/settings.json must no longer be ignored");

  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(again.is_empty(), "second run must be a no-op:
{}", again.report(root));
}

#[test]
fn obsolete_artifacts_are_reported_not_removed() {
  let dir = make_project();
  let root = dir.path();
  fs::remove_dir_all(root.join("docker")).unwrap();
  write(root, ".github/copilot-instructions.md", "old\n");
  assert!(
    read(root, "pyproject.toml").contains("[tool.docker]"),
    "fixture is expected to carry the table"
  );

  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(
    changes.notes.iter().any(|n| n.contains("copilot-instructions.md")),
    "{:?}",
    changes.notes
  );
  assert!(changes.notes.iter().any(|n| n.contains("[tool.docker]")), "{:?}", changes.notes);
  assert_eq!(read(root, ".github/copilot-instructions.md"), "old\n");
  assert!(read(root, "pyproject.toml").contains("[tool.docker]"));
}
