//! The outdated-version nag reaches stderr from ordinary commands and never from the
//! completion data path. Drives the real `devkit` binary with a fresh cache file so no
//! network is touched.

use std::path::Path;
use std::process::Command;

const DEVKIT: &str = env!("CARGO_BIN_EXE_devkit");

/// A project pinning aeth-devkit from a named index, plus a fresh cache claiming 99.0.0 is out.
fn project_with_stale_devkit() -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(
    dir.path().join("pyproject.toml"),
    r#"[project]
name = "demo"
version = "0.1.0"
dependencies = ["aeth-devkit>=7.1.0"]

[tool.uv.sources]
aeth-devkit = { index = "SFTPyPI" }

[[tool.uv.index]]
name = "SFTPyPI"
url = "https://example.invalid/+simple"
explicit = true
"#,
  )
  .unwrap();
  let now = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap()
    .as_secs();
  std::fs::write(
    dir.path().join("update-check.json"),
    format!(r#"{{"checked_at":{now},"latest":"99.0.0"}}"#),
  )
  .unwrap();
  dir
}

fn devkit(root: &Path, args: &[&str]) -> std::process::Output {
  Command::new(DEVKIT)
    .args(args)
    .current_dir(root)
    .env("DEVKIT_UPDATE_CACHE", root.join("update-check.json"))
    // `complete install` insists that `devkit` be on PATH; the test binary's own dir suffices.
    .env("PATH", Path::new(DEVKIT).parent().unwrap())
    .env("HOME", root)
    .env("USERPROFILE", root)
    .env_remove("DEVKIT_NO_UPDATE_CHECK")
    .output()
    .unwrap()
}

#[test]
fn an_ordinary_command_nags_on_stderr() {
  let proj = project_with_stale_devkit();
  let out = devkit(proj.path(), &["complete", "install", "--bash", "--dry-run"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(out.status.success(), "{stderr}");
  assert!(stderr.contains("99.0.0 available"), "stderr was: {stderr}");
  assert!(stderr.contains("uv tool upgrade aeth-devkit"), "{stderr}");
}

#[test]
fn the_completion_data_path_never_nags() {
  let proj = project_with_stale_devkit();
  for args in [
    vec!["complete", "tasks"],
    vec!["complete", "args", "x"],
    vec!["complete", "script", "--bash"],
  ] {
    let out = devkit(proj.path(), &args);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("outdated"), "{args:?} must stay silent: {stderr}");
  }
}

#[test]
fn the_env_var_disables_the_nag() {
  let proj = project_with_stale_devkit();
  let out = Command::new(DEVKIT)
    .args(["complete", "install", "--bash", "--dry-run"])
    .current_dir(proj.path())
    .env("DEVKIT_UPDATE_CACHE", proj.path().join("update-check.json"))
    .env("PATH", Path::new(DEVKIT).parent().unwrap())
    .env("HOME", proj.path())
    .env("USERPROFILE", proj.path())
    .env("DEVKIT_NO_UPDATE_CHECK", "1")
    .output()
    .unwrap();
  assert!(!String::from_utf8_lossy(&out.stderr).contains("outdated"));
}
