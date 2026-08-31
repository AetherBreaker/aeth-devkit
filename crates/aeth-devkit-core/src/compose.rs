//! Finding and editing docker compose files.
//!
//! Everything here is deliberately line-based and format-preserving: a YAML round-trip
//! would normalize quoting, ordering, and comments, and the compose file belongs to the
//! user, not to us.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

/// Docker Compose's own file-name precedence.
pub const COMPOSE_NAMES: [&str; 4] = ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];

/// Directories the walk never descends into: hidden dot-directories (`.git`, `.venv`,
/// `.cache`, …) and recognized environment/build trees. No compose file we would ever want
/// to pin lives inside one of these.
fn skip_dir(name: &str) -> bool {
  name.starts_with('.') || matches!(name, "__pycache__" | "node_modules" | "target")
}

/// The compose file to edit: breadth-first from `root` so a shallower file always beats a
/// deeper one; within one directory the [`COMPOSE_NAMES`] precedence decides; the first hit
/// wins (single compose file assumed — extend if that ever changes). Same-depth directories
/// are visited in name order, so the choice is deterministic.
pub fn find_compose_file(root: &Path) -> Result<Option<PathBuf>> {
  let mut queue = VecDeque::from([root.to_path_buf()]);
  while let Some(dir) = queue.pop_front() {
    for name in COMPOSE_NAMES {
      let candidate = dir.join(name);
      if candidate.is_file() {
        return Ok(Some(candidate));
      }
    }
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(&dir)
      .with_context(|| format!("reading {}", dir.display()))?
      .filter_map(|e| e.ok())
      .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
      .filter(|e| !skip_dir(&e.file_name().to_string_lossy()))
      .map(|e| e.path())
      .collect();
    subdirs.sort();
    queue.extend(subdirs);
  }
  Ok(None)
}

#[cfg(test)]
mod tests {
  use super::*;

  fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, "services:\n").unwrap();
  }

  #[test]
  fn shallower_beats_deeper_and_name_precedence_within_a_dir() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    touch(root, "deep/nested/compose.yaml");
    touch(root, "docker/docker-compose.yml");
    touch(root, "docker/compose.yml");
    // docker/ is depth 1, deep/nested is depth 2 → docker wins; compose.yml beats docker-compose.yml.
    assert_eq!(find_compose_file(root).unwrap().unwrap(), root.join("docker/compose.yml"));
    touch(root, "compose.yaml");
    assert_eq!(find_compose_file(root).unwrap().unwrap(), root.join("compose.yaml"));
  }

  #[test]
  fn skips_hidden_and_environment_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    touch(root, ".cache/compose.yaml");
    touch(root, "node_modules/compose.yaml");
    touch(root, "__pycache__/compose.yaml");
    assert_eq!(find_compose_file(root).unwrap(), None);
    touch(root, "ok/compose.yaml");
    assert_eq!(find_compose_file(root).unwrap().unwrap(), root.join("ok/compose.yaml"));
  }
}
