//! Change collection and (optional) writing.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

#[derive(Debug)]
pub struct FileChange {
  pub path: PathBuf,
  pub created: bool,
  pub details: Vec<String>,
}

#[derive(Debug)]
pub struct Changes {
  dry_run: bool,
  pub files: Vec<FileChange>,
}

impl Changes {
  pub fn new(dry_run: bool) -> Self {
    Self {
      dry_run,
      files: Vec::new(),
    }
  }

  pub fn is_empty(&self) -> bool {
    self.files.is_empty()
  }

  /// Record a change to an existing file. Writes unless dry-run; no-op if unchanged.
  pub fn record(&mut self, path: &Path, original: &str, merged: &str, details: Vec<String>) -> Result<()> {
    self.record_optional(path, Some(original), merged, details)
  }

  /// Record a change to a possibly-absent file.
  pub fn record_optional(&mut self, path: &Path, original: Option<&str>, merged: &str, details: Vec<String>) -> Result<()> {
    if original == Some(merged) {
      return Ok(());
    }
    let created = original.is_none();
    let details = if created && details.is_empty() {
      vec!["created".to_string()]
    } else {
      details
    };
    if !self.dry_run {
      if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
      }
      std::fs::write(path, merged).with_context(|| format!("writing {}", path.display()))?;
    }
    self.files.push(FileChange {
      path: path.to_path_buf(),
      created,
      details,
    });
    Ok(())
  }

  /// Human-readable report, one file per block.
  pub fn report(&self, root: &Path) -> String {
    let mut out = String::new();
    for f in &self.files {
      let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
      let verb = if f.created { "created" } else { "updated" };
      out.push_str(&format!("{rel}: {verb}\n"));
      for d in &f.details {
        if d != "created" {
          out.push_str(&format!("  - {d}\n"));
        }
      }
    }
    out
  }
}
