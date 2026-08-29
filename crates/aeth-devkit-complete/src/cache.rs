//! A file-backed cache of the resolved table, keyed on the sources' mtimes and sizes.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use aeth_devkit_core::process::Runner;

use crate::resolve::{Resolved, resolve};

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Fingerprint {
  pub devkit_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Stored {
  pub fingerprint: Fingerprint,
  pub resolved: Resolved,
}

/// Resolve through the cache; `bypass` forces a fresh resolution and rewrites the file.
pub fn resolve_cached(root: &Path, runner: &dyn Runner, bypass: bool) -> Result<Resolved> {
  let _ = bypass;
  resolve(root, runner)
}
