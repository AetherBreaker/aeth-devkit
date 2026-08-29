//! A file-backed cache of the resolved table, keyed on the sources' mtimes and sizes.
//!
//! Resolution costs ~16 ms when everything is TOML and ~70 ms when an `include_script` has
//! to run Python. Completion fires on every Tab press, so the warm path — a stat of a few
//! files plus one small JSON read — is what the user feels.
//!
//! Stated limitation: an `include_script` that returns *dynamic* tasks (reading env vars, a
//! database, the clock) is invisible to a file-based fingerprint. Nothing in the fleet does
//! this; `--no-cache` forces a fresh resolution when it matters.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use aeth_devkit_core::process::Runner;

use crate::resolve::{Resolved, resolve};

pub const CACHE_REL: &str = ".cache/devkit-completions.json";

/// One consulted file, as observed when the cache was written. `mtime_ns` is nanoseconds
/// since the Unix epoch; `size` guards against editors that preserve mtime.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourceStamp {
  pub path: PathBuf,
  pub mtime_ns: u128,
  pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Fingerprint {
  pub devkit_version: String,
  pub sources: Vec<SourceStamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Stored {
  pub fingerprint: Fingerprint,
  pub resolved: Resolved,
}

/// Stamp every source file. A source that cannot be stat'ed (deleted since) gets a zero
/// stamp, which never matches a real one, so its disappearance invalidates too.
fn stamp(sources: &[PathBuf]) -> Vec<SourceStamp> {
  sources
    .iter()
    .map(|p| {
      let meta = std::fs::metadata(p).ok();
      let mtime_ns = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
      SourceStamp {
        path: p.clone(),
        mtime_ns,
        size: meta.map(|m| m.len()).unwrap_or(0),
      }
    })
    .collect()
}

/// Resolve through the cache; `bypass` forces a fresh resolution and rewrites the file.
pub fn resolve_cached(root: &Path, runner: &dyn Runner, bypass: bool) -> Result<Resolved> {
  let cache_file = root.join(CACHE_REL);
  let version = env!("CARGO_PKG_VERSION");

  if !bypass
    && let Ok(text) = std::fs::read_to_string(&cache_file)
    // A corrupt or old-format file is simply a miss; never an error a shell would see.
    && let Ok(stored) = serde_json::from_str::<Stored>(&text)
    && stored.fingerprint.devkit_version == version
    // Re-stamp exactly the files the cached resolution consulted and compare.
    && stored.fingerprint.sources == stamp(&stored.resolved.sources)
  {
    return Ok(stored.resolved);
  }

  let resolved = resolve(root, runner)?;
  let stored = Stored {
    fingerprint: Fingerprint {
      devkit_version: version.to_string(),
      sources: stamp(&resolved.sources),
    },
    resolved,
  };
  // Writing the cache is best-effort: a read-only checkout still gets correct completions,
  // just without the speedup.
  if let Some(dir) = cache_file.parent() {
    let _ = std::fs::create_dir_all(dir);
  }
  if let Ok(json) = serde_json::to_string(&stored) {
    let _ = std::fs::write(&cache_file, json);
  }
  Ok(stored.resolved)
}
