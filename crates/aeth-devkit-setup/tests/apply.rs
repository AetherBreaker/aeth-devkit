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
  for rel in [".claude/settings.json", ".claude/settings.local.json", ".mcp.json"] {
    assert!(report.contains(&format!("{rel}: created")), "{report}");
  }
  assert_eq!(read(root, ".claude/CLAUDE.md"), "my own claude notes\n");
  assert_eq!(read(root, ".github/workflows/claude.yml"), "name: mine\n");

  let shared: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.json")).unwrap();
  let local: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.local.json")).unwrap();
  let cmd = local["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
  assert!(cmd.ends_with(" hook stop-ruff"), "{cmd}");
  assert!(cmd.starts_with("uv run devkit"), "no venv in fixture → uv fallback: {cmd}");
  assert!(local["env"]["PYTHONPYCACHEPREFIX"].as_str().unwrap().contains(".cache"));
  // Nothing machine-specific may reach the committed half.
  assert!(shared.get("hooks").is_none(), "hooks belong in the local half: {shared}");
  assert!(shared.get("env").is_none(), "env belongs in the local half: {shared}");
  assert_eq!(shared["enabledMcpjsonServers"], serde_json::json!(["github", "context7"]));

  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(again.is_empty(), "second run must be a no-op:\n{}", again.report(root));
}

#[test]
fn the_committed_settings_carry_no_absolute_or_os_specific_path() {
  // This is the whole point of the split: `settings.json` is shared, so a path from the
  // machine that ran setup would break every teammate whose clone lives elsewhere or who
  // runs a different OS.
  let dir = make_project();
  let root = dir.path();
  write(root, ".venv/Scripts/devkit.exe", "");
  aeth_devkit_setup::run(root, &templates(), false).unwrap();

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
  assert!(local.contains(".venv/Scripts/devkit.exe"), "{local}");
  assert!(read(root, ".gitignore").contains(".claude/settings.local.json"));
}

#[test]
fn claude_md_and_workflow_are_created_when_missing_and_devkit_bin_prefers_the_venv() {
  let dir = make_project();
  let root = dir.path();
  write(root, ".venv/Scripts/devkit.exe", "");

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(read(root, ".claude/CLAUDE.md").starts_with("@../AGENTS.md\n"));
  assert!(read(root, ".github/workflows/claude.yml").contains("claude-code-action@v1"));
  let local: serde_json::Value = serde_json::from_str(&read(root, ".claude/settings.local.json")).unwrap();
  let cmd = local["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
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

  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
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

  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
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
  aeth_devkit_setup::run(root, &templates(), false).unwrap();

  // Now the project tightens its own .gitignore, after everything is already in place.
  let gi = read(root, ".gitignore");
  write(root, ".gitignore", &format!("{gi}\n.claude/\n"));
  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();

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
  let dry = aeth_devkit_setup::run(a.path(), &templates(), true).unwrap();
  assert_eq!(snapshot(a.path()), before, "--dry-run must not write anything");

  let real = aeth_devkit_setup::run(b.path(), &templates(), false).unwrap();

  fn rels(c: &aeth_devkit_setup::changes::Changes, root: &Path) -> Vec<String> {
    let mut v: Vec<String> = c
      .files
      .iter()
      .map(|f| f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/"))
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
