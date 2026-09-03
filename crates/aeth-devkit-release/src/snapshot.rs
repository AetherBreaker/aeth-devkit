//! Byte-exact copies of the files a release rewrites, so rollback can put them back.
//!
//! `uv version`, `uv lock`, and the Cargo.toml edit all change files on disk before any
//! commit exists to reset to. Rather than reversing each edit, the affected files are copied
//! into a temporary directory up front and copied back on failure. Files that did *not*
//! exist before the run are deleted on restore, so the tree ends up exactly as it started.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
// `TempDir` deletes its directory when dropped, so a `Snapshot` cleans up after itself
// automatically — on success, on failure, and on panic.
use tempfile::TempDir;

/// The versioned files a release may rewrite.
pub const TRACKED: [&str; 4] = ["pyproject.toml", "uv.lock", "Cargo.toml", "Cargo.lock"];

/// A saved copy of the tracked files.
///
/// Fields are private: the only things a caller can do are `take` one and `restore` it,
/// which keeps the invariant "what is restored is exactly what was taken".
pub struct Snapshot {
  dir: TempDir,
  // Which of `TRACKED` existed at snapshot time. `&'static str` because the names come
  // from the `TRACKED` constant and live for the whole program — no allocation needed.
  present: Vec<&'static str>,
}

/// Copy the tracked files into a fresh temp directory.
pub fn take(root: &Path) -> Result<Snapshot> {
  let dir = tempfile::tempdir().context("creating snapshot dir")?;
  let mut present = Vec::new();
  for rel in TRACKED {
    let src = root.join(rel);
    if src.is_file() {
      std::fs::copy(&src, dir.path().join(rel)).with_context(|| format!("snapshotting {rel}"))?;
      present.push(rel);
    }
  }
  Ok(Snapshot { dir, present })
}

impl Snapshot {
  /// Where the copies live on disk, for the manual-recovery message.
  pub fn path(&self) -> &Path {
    self.dir.path()
  }

  /// Give up automatic cleanup and return the directory's path. Called when `restore`
  /// failed: the copies are now the only pre-run state left, so they must outlive us for
  /// the user to recover from by hand. `self` by value — the `Snapshot` is consumed, and
  /// `TempDir::keep` disarms the delete-on-drop.
  pub fn keep(self) -> PathBuf {
    self.dir.keep()
  }

  /// Did `rel` exist when the snapshot was taken?
  pub fn present(&self, rel: &str) -> bool {
    self.present.contains(&rel)
  }

  /// A paste-able equivalent of [`restore`](Self::restore) for when it failed: delete the
  /// managed files that did not exist before, then copy the saved originals back.
  pub fn manual_restore_command(&self) -> String {
    let rm: Vec<&str> = TRACKED.iter().copied().filter(|r| !self.present(r)).collect();
    let mut s = String::new();
    if !rm.is_empty() {
      s += &format!("rm -f {} && ", rm.join(" "));
    }
    s + &format!("cp -r \"{}\"/. .", self.dir.path().display())
  }

  /// Put everything back: tracked files copied over, or deleted if they were absent.
  pub fn restore(&self, root: &Path) -> Result<()> {
    for rel in TRACKED {
      let dst = root.join(rel);
      if self.present(rel) {
        std::fs::copy(self.dir.path().join(rel), &dst).with_context(|| format!("restoring {rel}"))?;
      } else if dst.exists() {
        std::fs::remove_file(&dst).with_context(|| format!("removing {rel}"))?;
      }
    }
    Ok(())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(root: &Path, rel: &str, s: &str) {
    std::fs::write(root.join(rel), s).unwrap();
  }

  #[test]
  fn restores_tracked_files_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "v=1\n");
    write(root, "Cargo.toml", "c=1\n");
    let snap = take(root).unwrap();
    assert!(snap.present("pyproject.toml") && !snap.present("uv.lock"));

    // Simulate everything a failed release might have done to the tree.
    write(root, "pyproject.toml", "v=2\n");
    write(root, "uv.lock", "new\n");
    std::fs::remove_file(root.join("Cargo.toml")).unwrap();

    snap.restore(root).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("pyproject.toml")).unwrap(), "v=1\n");
    assert_eq!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap(), "c=1\n");
    assert!(!root.join("uv.lock").exists());
  }

  #[test]
  fn manual_command_removes_only_files_that_were_absent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "x");
    let snap = take(root).unwrap();
    let manual = snap.manual_restore_command();
    assert!(manual.starts_with("rm -f uv.lock Cargo.toml Cargo.lock && cp -r "), "{manual}");
    for rel in TRACKED {
      write(root, rel, "x");
    }
    let all = take(root).unwrap().manual_restore_command();
    assert!(all.starts_with("cp -r "), "{all}");
  }
}
