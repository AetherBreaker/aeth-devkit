//! Finding and editing docker compose files.
//!
//! Everything here is deliberately line-based and format-preserving: a YAML round-trip
//! would normalize quoting, ordering, and comments, and the compose file belongs to the
//! user, not to us.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::github::normalize_repo;
use crate::pyproject::normalize_dist_name;

pub mod tree;

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

/// One `KEY: value` line: where it is (0-based) and the value with quotes stripped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyLine {
  pub line: usize,
  pub value: String,
}

/// The pin-relevant keys found inside one service under `services:`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceBlock {
  pub name: String,
  pub git_repo: Option<KeyLine>,
  pub package_name: Option<KeyLine>,
  pub git_tag: Option<KeyLine>,
  pub package_version: Option<KeyLine>,
}

pub(crate) fn indent_of(line: &str) -> usize {
  line.len() - line.trim_start().len()
}

/// A scalar as written, with surrounding matching quotes removed and a trailing
/// ` # comment` dropped. The one definition of "what a value is" for mapping lines and
/// list items alike.
pub(crate) fn unquote(s: &str) -> String {
  let v = s.split(" #").next().unwrap_or(s).trim();
  let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
  let v = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(v);
  v.to_string()
}

/// `("KEY", "value")` for a plain mapping line; `None` for blanks, comments, and list items.
fn key_value(line: &str) -> Option<(&str, String)> {
  let t = line.trim_start();
  if t.is_empty() || t.starts_with('#') || t.starts_with('-') {
    return None;
  }
  let (k, v) = t.split_once(':')?;
  Some((k.trim(), unquote(v)))
}

/// The service blocks under the top-level `services:` key. Only the four pin-relevant keys
/// are collected (first occurrence per block wins); everything else is ignored. Commented
/// lines never count.
pub fn parse_services(text: &str) -> Vec<ServiceBlock> {
  let lines: Vec<&str> = text.lines().collect();
  let Some(svc) = lines
    .iter()
    .position(|l| indent_of(l) == 0 && key_value(l).is_some_and(|(k, v)| k == "services" && v.is_empty()))
  else {
    return Vec::new();
  };
  let mut blocks: Vec<ServiceBlock> = Vec::new();
  let mut service_indent: Option<usize> = None;
  for (i, line) in lines.iter().enumerate().skip(svc + 1) {
    let t = line.trim();
    if t.is_empty() || t.starts_with('#') {
      continue;
    }
    let ind = indent_of(line);
    if ind == 0 {
      break; // next top-level key ends the services section
    }
    let si = *service_indent.get_or_insert(ind);
    if ind == si {
      if let Some((k, _)) = key_value(line) {
        blocks.push(ServiceBlock {
          name: k.to_string(),
          ..Default::default()
        });
      }
    } else if ind > si
      && let Some(b) = blocks.last_mut()
      && let Some((k, v)) = key_value(line)
    {
      let entry = KeyLine { line: i, value: v };
      match k {
        "GIT_REPO" if b.git_repo.is_none() => b.git_repo = Some(entry),
        "PACKAGE_NAME" if b.package_name.is_none() => b.package_name = Some(entry),
        "GIT_TAG" if b.git_tag.is_none() => b.git_tag = Some(entry),
        "PACKAGE_VERSION" if b.package_version.is_none() => b.package_version = Some(entry),
        _ => {}
      }
    }
  }
  blocks
}

/// Which key a matched service gets pinned through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinKind {
  GitTag,
  PackageVersion,
}

/// One line to rewrite: the service it belongs to, which key, where, and the value it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinTarget {
  pub service: String,
  pub kind: PinKind,
  pub line: usize,
  pub current: String,
}

/// The services that build *this* project, and the line each one is pinned on.
///
/// A block matches in git mode when its `GIT_REPO` names the same repository as the
/// project's `origin` remote (normalized comparison), and in pypi mode when its
/// `PACKAGE_NAME` normalizes to the project's package name. Git wins inside one block
/// (a `GIT_REPO` build arg means the image builds from source). A matched block missing
/// its pin key, or no match at all, is an error that lists what was found.
pub fn match_services(blocks: &[ServiceBlock], project_name: &str, origin_normalized: Option<&str>) -> Result<Vec<PinTarget>> {
  let want_pkg = normalize_dist_name(project_name);
  let mut out = Vec::new();
  for b in blocks {
    let git_match = match (&b.git_repo, origin_normalized) {
      (Some(r), Some(origin)) => normalize_repo(&r.value).as_deref() == Some(origin),
      _ => false,
    };
    let pypi_match = b.package_name.as_ref().is_some_and(|p| normalize_dist_name(&p.value) == want_pkg);
    if git_match {
      let Some(tag) = &b.git_tag else {
        bail!(
          "service {} builds this repo (GIT_REPO matches origin) but has no GIT_TAG to pin",
          b.name
        );
      };
      out.push(PinTarget {
        service: b.name.clone(),
        kind: PinKind::GitTag,
        line: tag.line,
        current: tag.value.clone(),
      });
    } else if pypi_match {
      let Some(ver) = &b.package_version else {
        bail!(
          "service {} builds this package (PACKAGE_NAME matches) but has no PACKAGE_VERSION to pin",
          b.name
        );
      };
      out.push(PinTarget {
        service: b.name.clone(),
        kind: PinKind::PackageVersion,
        line: ver.line,
        current: ver.value.clone(),
      });
    }
  }
  if out.is_empty() {
    let found: Vec<String> = blocks
      .iter()
      .map(|b| {
        format!(
          "{}: GIT_REPO={} PACKAGE_NAME={}",
          b.name,
          b.git_repo.as_ref().map_or("<none>", |k| &k.value),
          b.package_name.as_ref().map_or("<none>", |k| &k.value),
        )
      })
      .collect();
    bail!(
      "no service in the compose file builds this project ({project_name}); services found:\n  {}",
      if found.is_empty() {
        "<no services>".to_string()
      } else {
        found.join("\n  ")
      }
    );
  }
  Ok(out)
}

/// Rewrite the value of a `KEY: value` line, preserving indentation, the key, spacing,
/// the quoting style of the old value, and any trailing comment.
pub fn replace_value(line: &str, new_value: &str) -> String {
  let Some(colon) = line.find(':') else { return line.to_string() };
  let (head, rest) = line.split_at(colon + 1);
  let lead = rest.len() - rest.trim_start().len();
  let (space, after) = rest.split_at(lead);
  // Split off a trailing ` # comment`, then peel the whitespace gap between it and the
  // value so both survive the rewrite byte-for-byte.
  let cut = after.find(" #").map(|i| i + 1).unwrap_or(after.len());
  let (val_part, comment) = after.split_at(cut);
  let value = val_part.trim_end();
  let gap = &val_part[value.len()..];
  let rendered = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
    format!("\"{new_value}\"")
  } else if value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2 {
    format!("'{new_value}'")
  } else {
    new_value.to_string()
  };
  format!("{head}{space}{rendered}{gap}{comment}")
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

  const COMPOSE: &str = "\
services:
  app:
    build:
      args:
        GIT_REPO: https://github.com/Owner/Repo.git
        GIT_TAG: v1.0.0  # pinned
  worker:
    build:
      args:
        PACKAGE_NAME: My_Package
        PACKAGE_VERSION: \"1.0.0\"
  other:
    build:
      args:
        PACKAGE_NAME: unrelated
        PACKAGE_VERSION: 9.9.9
        # GIT_TAG: commented-out-never-counts
volumes: {}
";

  #[test]
  fn parses_service_blocks() {
    let blocks = parse_services(COMPOSE);
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].name, "app");
    assert_eq!(blocks[0].git_repo.as_ref().unwrap().value, "https://github.com/Owner/Repo.git");
    assert_eq!(blocks[0].git_tag.as_ref().unwrap().value, "v1.0.0");
    assert_eq!(blocks[1].package_version.as_ref().unwrap().value, "1.0.0"); // quotes stripped
    assert!(blocks[2].git_tag.is_none(), "commented lines never count");
  }

  #[test]
  fn matches_by_repo_and_package_only() {
    let blocks = parse_services(COMPOSE);
    let t = match_services(&blocks, "my-package", Some("github.com/owner/repo")).unwrap();
    assert_eq!(t.len(), 2);
    assert_eq!((t[0].service.as_str(), t[0].kind), ("app", PinKind::GitTag));
    assert_eq!((t[1].service.as_str(), t[1].kind), ("worker", PinKind::PackageVersion));
    // No origin and a foreign package name → error listing what was found.
    let err = match_services(&blocks, "nope", None).unwrap_err().to_string();
    assert!(err.contains("app") && err.contains("unrelated"), "{err}");
  }

  #[test]
  fn replace_value_preserves_shape() {
    assert_eq!(
      replace_value("        GIT_TAG: v1.0.0  # pinned", "v2.0.0"),
      "        GIT_TAG: v2.0.0  # pinned"
    );
    assert_eq!(
      replace_value("  PACKAGE_VERSION: \"1.0.0\"", "2.0.0"),
      "  PACKAGE_VERSION: \"2.0.0\""
    );
    assert_eq!(replace_value("  X: '1.0'", "2.0"), "  X: '2.0'");
  }
}
