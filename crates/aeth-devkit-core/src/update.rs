//! Nagging when a newer devkit is published.
//!
//! Every user-facing command ends by asking "is the running devkit the latest stable on the
//! project's index?" and, if not, printing a one-line `note:` on stderr with the right fix.
//! The index is consulted at most once a day — a fetch per Tab press or per `lock` would be
//! wasteful — and the answer is remembered in a small JSON file, so the nag itself is free
//! and repeats on every run until the upgrade happens. Anything that goes wrong (no
//! pyproject, no index entry, network down, garbage cache) silently produces no nag; an
//! update reminder must never break the command it decorates.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pep440_rs::Version;
use serde::{Deserialize, Serialize};

use crate::index::IndexClient;
use crate::pyproject;
use crate::version::latest_stable;

/// The package whose releases we look for.
pub const PACKAGE: &str = "aeth-devkit";
/// How long a cached answer stays fresh.
pub const DAY_SECS: u64 = 24 * 60 * 60;
/// Set (to anything) to skip the check entirely.
pub const DISABLE_ENV: &str = "DEVKIT_NO_UPDATE_CHECK";
/// Network budget for the daily fetch; past this the user gets no nag, not a slow command.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(3);

/// What the cache file holds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stored {
  /// Unix seconds of the last successful fetch.
  pub checked_at: u64,
  /// Highest stable version seen then; `None` when the index listed no stable release.
  pub latest: Option<String>,
}

/// The nag text, or `None` when `latest` is not strictly newer than `current` (or either
/// is not a PEP 440 version). The fix depends on how this devkit was installed: a binary
/// inside a `.venv` came from the project's pin, so `devkit lock` (which bumps that pin) is
/// the upgrade; anything else is a `uv tool install`.
pub fn message(current: &str, latest: &str, exe: &Path) -> Option<String> {
  let (cur, new) = (current.parse::<Version>().ok()?, latest.parse::<Version>().ok()?);
  if new <= cur {
    return None;
  }
  let fix = if exe.components().any(|c| c.as_os_str() == ".venv") {
    "Run `devkit lock` to bump the project's pin."
  } else {
    "Run `uv tool upgrade aeth-devkit`."
  };
  Some(format!("note: devkit {current} is outdated ({latest} available). {fix}"))
}

/// Read the cache file; a missing or corrupt file is simply "no cache".
fn read_cache(path: &Path) -> Option<Stored> {
  let text = std::fs::read_to_string(path).ok()?;
  serde_json::from_str(&text).ok()
}

/// Write the cache file, creating its directory. Failure is ignored: the nag still works
/// this run, and next run will just fetch again.
fn write_cache(path: &Path, stored: &Stored) {
  if let Some(dir) = path.parent() {
    let _ = std::fs::create_dir_all(dir);
  }
  if let Ok(text) = serde_json::to_string(stored) {
    let _ = std::fs::write(path, text);
  }
}

/// The index URL the project pins `aeth-devkit` from, if it has one. Only projects that
/// pin devkit from a named index get checked: there is no other way to know where its
/// releases live.
fn project_index_url(root: &Path) -> Option<String> {
  let text = std::fs::read_to_string(root.join("pyproject.toml")).ok()?;
  let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
  pyproject::index_url_for(&doc, PACKAGE)
}

/// Decide whether to nag, given an injected index client and clock (`now` in Unix seconds).
/// `cache` is the JSON file path; `current` is the running version; `exe` its binary.
pub fn check(root: &Path, index: &dyn IndexClient, cache: &Path, now: u64, current: &str, exe: &Path) -> Option<String> {
  let simple_url = project_index_url(root)?;
  let cached = read_cache(cache);
  let fresh = cached.as_ref().is_some_and(|s| now.saturating_sub(s.checked_at) < DAY_SECS);
  let stored = if fresh {
    cached?
  } else {
    match index.versions(&simple_url, PACKAGE) {
      Ok(versions) => {
        let stored = Stored {
          checked_at: now,
          latest: latest_stable(versions.iter().map(String::as_str)),
        };
        write_cache(cache, &stored);
        stored
      }
      // Offline: the last answer is better than none, and the cache keeps its old stamp so
      // the next run tries again.
      Err(_) => cached?,
    }
  };
  message(current, stored.latest.as_deref()?, exe)
}

/// Set to a file path to relocate the cache (tests, or a user who wants it elsewhere).
pub const CACHE_ENV: &str = "DEVKIT_UPDATE_CACHE";

/// This user's devkit cache directory: `%LOCALAPPDATA%\aeth-devkit` on Windows, else
/// `$XDG_CACHE_HOME/aeth-devkit` (default `~/.cache/aeth-devkit`). The VS Code extension
/// computes the same path, so the two find each other's files without configuration.
pub fn cache_dir() -> Option<PathBuf> {
  let base = if cfg!(windows) {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?
  } else {
    match std::env::var_os("XDG_CACHE_HOME") {
      Some(x) => PathBuf::from(x),
      None => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    }
  };
  Some(base.join("aeth-devkit"))
}

/// Where the update-check cache lives: [`CACHE_ENV`] if set, else
/// `update-check.json` under [`cache_dir`].
pub fn cache_path() -> Option<PathBuf> {
  if let Some(p) = std::env::var_os(CACHE_ENV) {
    return Some(PathBuf::from(p));
  }
  Some(cache_dir()?.join("update-check.json"))
}

/// Production entry: run [`check`] against the current directory with the real index client
/// and print the nag to stderr. `current` is the caller's `CARGO_PKG_VERSION`. Honours [`DISABLE_ENV`]. Never fails.
pub fn nag(current: &str) {
  if std::env::var_os(DISABLE_ENV).is_some() {
    return;
  }
  let (Ok(root), Some(cache), Ok(exe)) = (std::env::current_dir(), cache_path(), std::env::current_exe()) else {
    return;
  };
  let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
  let index = crate::index::HttpIndexClient::with_timeout(FETCH_TIMEOUT);
  if let Some(msg) = check(&root, &index, &cache, now, current, &exe) {
    eprintln!("{msg}");
  }
}
