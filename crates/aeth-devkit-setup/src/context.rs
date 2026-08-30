//! Facts about the target project that templates and merges need.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

pub use aeth_devkit_core::paths::strip_verbatim;

#[derive(Debug, Clone)]
pub struct ProjectContext {
  pub root: PathBuf,
  /// Import name of the project's package (e.g. `imap_report_collector`).
  pub package: String,
  /// Normalized names of every declared dependency (runtime, optional, and groups).
  pub dependencies: HashSet<String>,
  /// Whether the project has a Docker setup (`docker/` dir or `Dockerfile*`).
  pub has_docker: bool,
  /// Directory holding the Python package: `python` for mixed Rust/Python projects
  /// (where `src/` is Rust), otherwise `src`.
  pub python_dir: String,
  /// Whether the project also contains a Rust crate (`Cargo.toml` at the root).
  pub has_rust: bool,
}

impl ProjectContext {
  pub fn discover(root: &Path) -> Result<Self> {
    let root = root.canonicalize().with_context(|| format!("resolving {}", root.display()))?;
    let root = strip_verbatim(root);
    let pyproject_path = root.join("pyproject.toml");
    if !pyproject_path.is_file() {
      bail!("{} not found — run from a project root", pyproject_path.display());
    }
    let text = std::fs::read_to_string(&pyproject_path)?;
    let doc: toml_edit::DocumentMut = text.parse().context("parsing pyproject.toml")?;

    let project_name = doc
      .get("project")
      .and_then(|p| p.get("name"))
      .and_then(|n| n.as_str())
      .map(str::to_string)
      .unwrap_or_else(|| root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());

    let has_rust = root.join("Cargo.toml").is_file();
    let python_dir = if find_package_in(&root.join("python")).is_some() {
      "python"
    } else {
      "src"
    }
    .to_string();
    let package = find_package_in(&root.join(&python_dir)).unwrap_or_else(|| normalize_import_name(&project_name));

    let mut dependencies = HashSet::new();
    let mut collect = |item: Option<&toml_edit::Item>| {
      if let Some(arr) = item.and_then(|i| i.as_array()) {
        for v in arr.iter() {
          if let Some(s) = v.as_str() {
            dependencies.insert(dependency_name(s));
          }
        }
      }
    };
    collect(doc.get("project").and_then(|p| p.get("dependencies")));
    if let Some(t) = doc
      .get("project")
      .and_then(|p| p.get("optional-dependencies"))
      .and_then(|o| o.as_table_like())
    {
      for (_, v) in t.iter() {
        collect(Some(v));
      }
    }
    if let Some(t) = doc.get("dependency-groups").and_then(|g| g.as_table_like()) {
      for (_, v) in t.iter() {
        collect(Some(v));
      }
    }

    // A bare `docker/` directory (empty, or a stray leftover) is not a Docker setup; it
    // has to hold a Dockerfile or a compose file, or the root has to have a Dockerfile.
    let has_docker = dir_has_docker_content(&root) || dir_has_docker_content(&root.join("docker"));

    Ok(Self {
      root,
      package,
      dependencies,
      has_docker,
      python_dir,
      has_rust,
    })
  }

  pub fn has_dependency(&self, name: &str) -> bool {
    self.dependencies.contains(&normalize_dist_name(name))
  }

  /// Replace `${workspaceFolder}` with the project root and normalize separators.
  pub fn resolve_workspace_var(&self, value: &str) -> PathBuf {
    let replaced = value.replace("${workspaceFolder}", &self.root.to_string_lossy());
    PathBuf::from(replaced.replace('/', std::path::MAIN_SEPARATOR_STR))
  }
}

/// Whether `dir` directly contains a `Dockerfile*` or a compose file. Both the modern
/// (`compose.yml`) and the legacy (`docker-compose.yml`) spellings count: the legacy one is
/// still the more common in the wild, and the previous `docker/` -is-a-directory probe
/// accepted it, so leaving it out would silently demote real Docker projects.
fn dir_has_docker_content(dir: &Path) -> bool {
  let Ok(entries) = std::fs::read_dir(dir) else { return false };
  entries.flatten().any(|e| {
    let name = e.file_name().to_string_lossy().to_string();
    e.path().is_file() && (name.starts_with("Dockerfile") || is_compose_file(&name))
  })
}

/// The four compose filenames Docker itself looks for, modern spelling first.
fn is_compose_file(name: &str) -> bool {
  matches!(name, "compose.yml" | "compose.yaml" | "docker-compose.yml" | "docker-compose.yaml")
}

/// The sole package directory under `dir` (has `__init__.py`), if unambiguous.
fn find_package_in(dir: &Path) -> Option<String> {
  let entries = std::fs::read_dir(dir).ok()?;
  let mut pkgs: Vec<String> = entries
    .flatten()
    .filter(|e| e.path().is_dir() && e.path().join("__init__.py").is_file())
    .map(|e| e.file_name().to_string_lossy().to_string())
    .filter(|n| !n.starts_with('_') && !n.starts_with('.') && !n.ends_with(".egg-info"))
    .collect();
  if pkgs.len() == 1 { pkgs.pop() } else { None }
}

pub fn normalize_import_name(dist_name: &str) -> String {
  dist_name.replace(['-', '.'], "_").to_lowercase()
}

/// PEP 503 normalization: lowercase, runs of `-_.` → `-`.
pub fn normalize_dist_name(name: &str) -> String {
  let mut out = String::with_capacity(name.len());
  let mut last_sep = false;
  for c in name.chars() {
    if c == '-' || c == '_' || c == '.' {
      if !last_sep {
        out.push('-');
      }
      last_sep = true;
    } else {
      out.push(c.to_ascii_lowercase());
      last_sep = false;
    }
  }
  out
}

/// Package name from a PEP 508 requirement string (`poe-tasks[extra]>=4.0; marker` → `poe-tasks`).
pub fn dependency_name(requirement: &str) -> String {
  let end = requirement
    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
    .unwrap_or(requirement.len());
  normalize_dist_name(requirement[..end].trim())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn dependency_names() {
    assert_eq!(dependency_name("poe-tasks>=4.0.0"), "poe-tasks");
    assert_eq!(dependency_name("aeth-ext[sftp, async]>=8.0.2"), "aeth-ext");
    assert_eq!(dependency_name("Mypy"), "mypy");
    assert_eq!(dependency_name("pandas_stubs >= 3"), "pandas-stubs");
  }
}

#[cfg(test)]
mod docker_detection {
  use super::*;

  fn project(files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"p\"\n").unwrap();
    for f in files {
      let p = dir.path().join(f);
      std::fs::create_dir_all(p.parent().unwrap()).unwrap();
      std::fs::write(p, "").unwrap();
    }
    dir
  }

  #[test]
  fn a_bare_docker_directory_is_not_a_docker_setup() {
    let dir = project(&["docker/.keep"]);
    assert!(!ProjectContext::discover(dir.path()).unwrap().has_docker);
  }

  #[test]
  fn real_docker_content_is() {
    for files in [
      &["Dockerfile"][..],
      &["Dockerfile.dev"],
      &["docker/Dockerfile"],
      &["docker/compose.yaml"],
      &["docker/compose.yml"],
      // The legacy spelling is still the common one, and the old `docker/`-is-a-directory
      // probe accepted it; dropping it would demote real Docker projects.
      &["docker-compose.yml"],
      &["docker-compose.yaml"],
      &["docker/docker-compose.yml"],
    ] {
      let dir = project(files);
      assert!(ProjectContext::discover(dir.path()).unwrap().has_docker, "{files:?}");
    }
  }

  #[test]
  fn unrelated_yaml_is_not_a_docker_setup() {
    for files in [&["docker/notes.yml"][..], &["compose-overrides.yml"], &["docker/README.md"]] {
      let dir = project(files);
      assert!(!ProjectContext::discover(dir.path()).unwrap().has_docker, "{files:?}");
    }
  }
}
