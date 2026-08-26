//! End-to-end: apply the real templates to fixture projects, check outcomes and idempotency.

use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn templates() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("python")
    .join("poe_tasks")
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

  let changes = sft_setup::run(root, &templates(), false).unwrap();
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
      "PYTHONPYCACHEPREFIX=\"{}\\.cache\\pycache\"",
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
  let again = sft_setup::run(root, &templates(), false).unwrap();
  assert!(again.is_empty(), "second run should be a no-op, got:\n{}", again.report(root));
}

#[test]
fn dry_run_writes_nothing() {
  let dir = make_project();
  let root = dir.path();
  let before = read(root, "pyproject.toml");
  let changes = sft_setup::run(root, &templates(), true).unwrap();
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
  let changes = sft_setup::run(root, &templates(), false).unwrap();
  let gi = read(root, ".gitignore");
  assert!(!gi.contains("project-specific"), "{gi}");
  assert!(gi.starts_with("# Byte-compiled"));
  let py = read(root, "pyproject.toml");
  assert!(py.contains("[tool.mypy]"), "{py}");
  assert!(py.contains("source_pkgs = [\"demo_app\"]"), "{py}");
  assert!(root.join(".vscode/launch.json").is_file());
  assert!(!root.join(".dockerignore").exists(), "no docker setup → no .dockerignore");
  assert!(!changes.is_empty());
  assert!(sft_setup::run(root, &templates(), false).unwrap().is_empty());
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
  sft_setup::run(root, &templates(), false).unwrap();

  let py = read(root, "pyproject.toml");
  assert!(py.contains("src       = [\"./python\"]"), "{py}");
  assert!(py.contains("root = \"python\", extraPaths = [\"python\"]"), "{py}");
  assert!(py.contains("source_pkgs = [\"mixed_tool\"]"), "{py}");
  let launch = read(root, ".vscode/launch.json");
  assert!(launch.contains("\"PYTHONPATH\": \"${workspaceFolder}/python\""), "{launch}");
  let ext = read(root, ".vscode/extensions.json");
  assert!(ext.contains("rust-lang.rust-analyzer"), "{ext}");
  let settings = read(root, ".vscode/settings.json");
  assert!(settings.contains("\"[rust]\""), "{settings}");
  let gi = read(root, ".gitignore");
  assert!(gi.contains("target/"), "{gi}");
  assert!(gi.contains("secrets/"), "{gi}");
  assert!(sft_setup::run(root, &templates(), false).unwrap().is_empty());
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
  sft_setup::run(root, &templates(), false).unwrap();
  assert!(read(root, "pyproject.toml").contains("src       = [\"./src\"]"));
  assert!(!read(root, ".vscode/extensions.json").contains("rust-analyzer"));
  assert!(!read(root, ".vscode/settings.json").contains("[rust]"));
}
