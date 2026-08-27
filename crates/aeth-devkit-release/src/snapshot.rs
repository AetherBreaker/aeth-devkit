//! Byte-exact copies of the files a release rewrites, so rollback can put them back.
//!
//! `uv version`, `uv lock`, the Cargo.toml edit, and `uv build` all change files on disk
//! before any commit exists to reset to. Rather than trying to reverse each edit, we copy
//! the affected files into a temporary directory up front and copy them back on failure.
//! Files that did *not* exist before the run are deleted on restore, so the tree ends up
//! exactly as it started.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
// `TempDir` deletes its directory when dropped, so a `Snapshot` cleans up after itself
// automatically — on success, on failure, and on panic.
use tempfile::TempDir;

/// The versioned files a release may rewrite.
pub const TRACKED: [&str; 4] = ["pyproject.toml", "uv.lock", "Cargo.toml", "Cargo.lock"];

/// A saved copy of the tracked files and the `dist/` artefacts.
///
/// Fields are private: the only things a caller can do are `take` one and `restore` it,
/// which keeps the invariant "what is restored is exactly what was taken".
pub struct Snapshot {
  dir: TempDir,
  // Which of `TRACKED` existed at snapshot time. `&'static str` because the names come
  // from the `TRACKED` constant and live for the whole program — no allocation needed.
  present: Vec<&'static str>,
  // File names (not paths) of the dist artefacts we copied.
  dist: Vec<String>,
}

/// Wheels and sdists are the only things `uv build` writes to `dist/`; anything else in
/// there (a README, say) is left alone.
fn is_artifact(p: &Path) -> bool {
  // `file_name()` is `None` for paths ending in `..`; `unwrap_or_default` gives "" then.
  let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
  name.ends_with(".whl") || name.ends_with(".tar.gz")
}

/// Absolute paths of the artefacts in `root/dist`, sorted for deterministic output.
pub fn dist_artifacts(root: &Path) -> Result<Vec<PathBuf>> {
  let dist = root.join("dist");
  if !dist.is_dir() {
    return Ok(Vec::new());
  }
  // `read_dir` yields `Result<DirEntry>` per entry; `filter_map(|e| e.ok()…)` drops the
  // unreadable ones rather than failing the whole listing on a single bad entry.
  let mut out: Vec<PathBuf> = std::fs::read_dir(&dist)
    .context("reading dist/")?
    .filter_map(|e| e.ok().map(|e| e.path()))
    .filter(|p| p.is_file() && is_artifact(p))
    .collect();
  out.sort();
  Ok(out)
}

/// Delete every artefact in `dist/` (creating the directory if needed) — what the old
/// script did with `rm -f dist/*.whl dist/*.tar.gz` before `uv build`.
pub fn clear_dist(root: &Path) -> Result<()> {
  std::fs::create_dir_all(root.join("dist")).context("creating dist/")?;
  for p in dist_artifacts(root)? {
    std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
  }
  Ok(())
}

/// Copy the tracked files and dist artefacts into a fresh temp directory.
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
  std::fs::create_dir(dir.path().join("dist")).context("creating dist snapshot dir")?;
  let mut dist = Vec::new();
  for p in dist_artifacts(root)? {
    // Safe unwrap: `dist_artifacts` only returns real files, which always have a name.
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::copy(&p, dir.path().join("dist").join(&name)).with_context(|| format!("snapshotting {name}"))?;
    dist.push(name);
  }
  Ok(Snapshot { dir, present, dist })
}

impl Snapshot {
  /// Did `rel` exist when the snapshot was taken?
  pub fn present(&self, rel: &str) -> bool {
    self.present.contains(&rel)
  }

  /// Put everything back: tracked files copied over (or deleted if they were absent), and
  /// `dist/` cleared then refilled with the original artefacts.
  pub fn restore(&self, root: &Path) -> Result<()> {
    for rel in TRACKED {
      let dst = root.join(rel);
      if self.present(rel) {
        std::fs::copy(self.dir.path().join(rel), &dst).with_context(|| format!("restoring {rel}"))?;
      } else if dst.exists() {
        std::fs::remove_file(&dst).with_context(|| format!("removing {rel}"))?;
      }
    }
    clear_dist(root)?;
    for name in &self.dist {
      std::fs::copy(self.dir.path().join("dist").join(name), root.join("dist").join(name))
        .with_context(|| format!("restoring dist/{name}"))?;
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
  fn restores_files_and_dist_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(root, "pyproject.toml", "v=1\n");
    write(root, "Cargo.toml", "c=1\n");
    std::fs::create_dir(root.join("dist")).unwrap();
    write(root, "dist/old-1.0.whl", "w");
    write(root, "dist/README", "keep");
    let snap = take(root).unwrap();
    assert!(snap.present("pyproject.toml") && !snap.present("uv.lock"));

    // Simulate everything a failed release might have done to the tree.
    write(root, "pyproject.toml", "v=2\n");
    write(root, "uv.lock", "new\n");
    std::fs::remove_file(root.join("Cargo.toml")).unwrap();
    clear_dist(root).unwrap();
    write(root, "dist/new-2.0.whl", "w2");
    write(root, "dist/new-2.0.tar.gz", "t2");
    assert_eq!(dist_artifacts(root).unwrap().len(), 2);

    snap.restore(root).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("pyproject.toml")).unwrap(), "v=1\n");
    assert_eq!(std::fs::read_to_string(root.join("Cargo.toml")).unwrap(), "c=1\n");
    assert!(!root.join("uv.lock").exists());
    let names: Vec<String> = dist_artifacts(root)
      .unwrap()
      .iter()
      .map(|p| p.file_name().unwrap().to_string_lossy().into())
      .collect();
    assert_eq!(names, vec!["old-1.0.whl"]);
    assert!(root.join("dist/README").exists());
  }

  #[test]
  fn take_works_without_dist_dir() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "pyproject.toml", "x");
    let snap = take(dir.path()).unwrap();
    snap.restore(dir.path()).unwrap();
    assert!(dir.path().join("dist").is_dir());
  }
}
