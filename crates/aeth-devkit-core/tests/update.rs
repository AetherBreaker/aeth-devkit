//! `update`: nag when a newer devkit is published. Pure logic over an injected index client
//! and clock; the cache is a JSON file the tests point at a temp dir.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::update::{DAY_SECS, Stored, check, message};

/// A stub that counts how often it is asked, so the tests can prove the cache prevents fetches.
struct Counting {
  versions: Vec<String>,
  fail: bool,
  calls: Cell<u32>,
}

impl Counting {
  fn new(versions: &[&str]) -> Self {
    Self {
      versions: versions.iter().map(|s| s.to_string()).collect(),
      fail: false,
      calls: Cell::new(0),
    }
  }
}

impl IndexClient for Counting {
  fn versions(&self, _simple_url: &str, _package: &str) -> anyhow::Result<Vec<String>> {
    self.calls.set(self.calls.get() + 1);
    if self.fail {
      anyhow::bail!("index unreachable");
    }
    Ok(self.versions.clone())
  }
}

/// A project dir whose pyproject pins aeth-devkit from a named index.
fn project() -> tempfile::TempDir {
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
url = "https://example.test/+simple"
explicit = true
"#,
  )
  .unwrap();
  dir
}

fn tool_exe() -> PathBuf {
  PathBuf::from(if cfg!(windows) {
    r"C:\Users\me\.local\bin\devkit.exe"
  } else {
    "/home/me/.local/bin/devkit"
  })
}

fn venv_exe() -> PathBuf {
  PathBuf::from(if cfg!(windows) {
    r"D:\proj\.venv\Scripts\devkit.exe"
  } else {
    "/home/me/proj/.venv/bin/devkit"
  })
}

// ---- message wording ----------------------------------------------------------------------

#[test]
fn message_names_both_versions_and_the_tool_upgrade_for_a_global_install() {
  let m = message("7.1.0", "7.2.0", &tool_exe()).expect("newer version must produce a message");
  assert!(m.contains("7.1.0") && m.contains("7.2.0"), "{m}");
  assert!(m.contains("uv tool upgrade aeth-devkit"), "{m}");
}

#[test]
fn message_suggests_devkit_lock_when_running_from_a_venv() {
  let m = message("7.1.0", "7.2.0", &venv_exe()).unwrap();
  assert!(m.contains("devkit lock"), "{m}");
  assert!(!m.contains("uv tool upgrade"), "{m}");
}

#[test]
fn message_is_none_when_current_is_equal_or_newer_or_unparseable() {
  assert_eq!(message("7.2.0", "7.2.0", &tool_exe()), None);
  assert_eq!(message("7.3.0", "7.2.0", &tool_exe()), None);
  assert_eq!(message("junk", "7.2.0", &tool_exe()), None);
  assert_eq!(message("7.2.0", "junk", &tool_exe()), None);
}

// ---- check: fetch, cache, nag -------------------------------------------------------------

fn cache_in(dir: &Path) -> PathBuf {
  dir.join("cache").join("update-check.json")
}

#[test]
fn first_run_fetches_writes_the_cache_and_nags() {
  let proj = project();
  let cache = cache_in(proj.path());
  let index = Counting::new(&["7.0.0", "7.2.0", "7.3.0a1"]);
  let msg = check(proj.path(), &index, &cache, 1_000, "7.1.0", &tool_exe());
  assert_eq!(index.calls.get(), 1);
  assert!(msg.is_some_and(|m| m.contains("7.2.0")), "prerelease 7.3.0a1 must not be offered");
  let stored: Stored = serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
  assert_eq!(stored.checked_at, 1_000);
  assert_eq!(stored.latest.as_deref(), Some("7.2.0"));
}

#[test]
fn a_fresh_cache_is_used_without_fetching_and_still_nags_every_run() {
  let proj = project();
  let cache = cache_in(proj.path());
  let index = Counting::new(&["7.2.0"]);
  check(proj.path(), &index, &cache, 1_000, "7.1.0", &tool_exe());
  let again = check(proj.path(), &index, &cache, 1_000 + DAY_SECS - 1, "7.1.0", &tool_exe());
  assert_eq!(index.calls.get(), 1, "second run within a day must not hit the index");
  assert!(again.is_some(), "the nag repeats on every run until upgraded");
}

#[test]
fn a_stale_cache_is_refreshed() {
  let proj = project();
  let cache = cache_in(proj.path());
  let index = Counting::new(&["7.2.0"]);
  check(proj.path(), &index, &cache, 1_000, "7.1.0", &tool_exe());
  check(proj.path(), &index, &cache, 1_000 + DAY_SECS, "7.1.0", &tool_exe());
  assert_eq!(index.calls.get(), 2);
  let stored: Stored = serde_json::from_str(&std::fs::read_to_string(&cache).unwrap()).unwrap();
  assert_eq!(stored.checked_at, 1_000 + DAY_SECS);
}

#[test]
fn nothing_when_up_to_date() {
  let proj = project();
  let index = Counting::new(&["7.1.0"]);
  assert_eq!(
    check(proj.path(), &index, &cache_in(proj.path()), 1_000, "7.1.0", &tool_exe()),
    None
  );
}

#[test]
fn a_failed_fetch_is_silent_and_leaves_the_cache_alone() {
  let proj = project();
  let cache = cache_in(proj.path());
  let good = Counting::new(&["7.2.0"]);
  check(proj.path(), &good, &cache, 1_000, "7.1.0", &tool_exe());
  let before = std::fs::read_to_string(&cache).unwrap();
  let mut bad = Counting::new(&[]);
  bad.fail = true;
  // Stale cache → tries to fetch → fails → falls back to the cached answer, keeps nagging.
  let msg = check(proj.path(), &bad, &cache, 1_000 + DAY_SECS, "7.1.0", &tool_exe());
  assert_eq!(bad.calls.get(), 1);
  assert!(msg.is_some(), "cached knowledge of 7.2.0 still applies");
  assert_eq!(
    std::fs::read_to_string(&cache).unwrap(),
    before,
    "a failed fetch must not rewrite the cache"
  );
}

#[test]
fn no_pyproject_or_no_index_means_no_check_at_all() {
  let empty = tempfile::tempdir().unwrap();
  let index = Counting::new(&["9.9.9"]);
  assert_eq!(
    check(empty.path(), &index, &cache_in(empty.path()), 1_000, "7.1.0", &tool_exe()),
    None
  );
  std::fs::write(empty.path().join("pyproject.toml"), "[project]\nname = \"x\"\nversion = \"0\"\n").unwrap();
  assert_eq!(
    check(empty.path(), &index, &cache_in(empty.path()), 1_000, "7.1.0", &tool_exe()),
    None
  );
  assert_eq!(index.calls.get(), 0);
}

#[test]
fn a_corrupt_cache_is_treated_as_missing() {
  let proj = project();
  let cache = cache_in(proj.path());
  std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
  std::fs::write(&cache, "{not json").unwrap();
  let index = Counting::new(&["7.2.0"]);
  assert!(check(proj.path(), &index, &cache, 1_000, "7.1.0", &tool_exe()).is_some());
  assert_eq!(index.calls.get(), 1);
}
