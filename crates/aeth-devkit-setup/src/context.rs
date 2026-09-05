//! Facts about the target project that templates and merges need.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};

pub use aeth_devkit_core::paths::strip_verbatim;

#[derive(Debug, Clone)]
pub struct ProjectContext {
  pub root: PathBuf,
  /// Import name of the project's package (e.g. `imap_report_collector`).
  pub package: String,
  /// Normalized names of every declared dependency (runtime, optional, and groups).
  pub dependencies: HashSet<String>,
  /// Whether `[tool.docker].services` lists at least one service — the only Docker
  /// switch. Docker files alone do not count (see `docker_files`).
  pub has_docker: bool,
  /// `[project].name` as written (dist name; `package` is the import name).
  pub name: String,
  /// `[project].version`, when present.
  pub version: Option<String>,
  /// The `origin` remote URL when the project is git-tracked and has one.
  pub origin: Option<String>,
  /// `[tool.docker].services`: the compose services setup-project manages.
  pub docker_services: Vec<String>,
  /// Legacy `[tool.docker]` keys still present (`chown_paths`, `mkdirs`); only reported.
  pub docker_legacy_keys: Vec<String>,
  /// A Dockerfile at the root or under `docker/`, or a compose file anywhere docker-pin
  /// would find one. Seeds the `[tool.docker]` table and drives the unlisted-services
  /// warning; never a switch on its own.
  pub docker_files: bool,
  /// `[tool.docker].silence_unlisted_services_warning`: quiets the warning that Docker
  /// files exist while `services` lists nothing, for projects that keep their own Docker
  /// setup. Read nowhere else; `services` stays the only functional switch.
  pub silence_unlisted_services_warning: bool,
  /// Directory holding the Python package: `python` for mixed Rust/Python projects
  /// (where `src/` is Rust), otherwise `src`.
  pub python_dir: String,
  /// Whether the project also contains a Rust crate (`Cargo.toml` at the root).
  pub has_rust: bool,
  /// Whether the workspace holds `crates/aeth-devkit-container`: only devkit itself does,
  /// and only its release workflow may build and attach the container binary.
  pub has_container_crate: bool,
  /// Name of the sole `[[tool.uv.index]]` with a `publish-url`, which the release workflow
  /// publishes to; `None` means PyPI via trusted publishing.
  pub publish_index: Option<String>,
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
    let has_container_crate = root.join("crates").join("aeth-devkit-container").join("Cargo.toml").is_file();
    // The workflow publishes to exactly one place; with several candidates any choice is
    // wrong half the time, so it is a configuration error rather than a guess.
    let publish = aeth_devkit_core::pyproject::publish_indexes(&doc)?;
    let publish_index = match publish.as_slice() {
      [] => None,
      [one] => Some(one.name.clone()),
      many => bail!(
        "several [[tool.uv.index]] entries have a publish-url ({}); the release workflow can publish to only one",
        many.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ")
      ),
    };
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

    let docker = doc.get("tool").and_then(|t| t.get("docker"));
    let docker_services = services_key(&doc)?.unwrap_or_default();
    let docker_legacy_keys: Vec<String> = ["chown_paths", "mkdirs"]
      .into_iter()
      .filter(|k| docker.is_some_and(|d| d.get(k).is_some()))
      .map(str::to_string)
      .collect();
    let has_docker = !docker_services.is_empty();
    let docker_files = docker_files_in(&root);
    let silence_unlisted_services_warning = match docker.and_then(|d| d.get("silence_unlisted_services_warning")) {
      None => false,
      Some(item) => item.as_bool().ok_or_else(|| {
        anyhow!(
          "[tool.docker].silence_unlisted_services_warning must be a boolean, got {}",
          item.type_name()
        )
      })?,
    };
    let version = doc
      .get("project")
      .and_then(|p| p.get("version"))
      .and_then(|v| v.as_str())
      .map(str::to_string);
    // `git remote get-url` outside a repository fails; `ok().flatten()` folds both "not a
    // repo" and "no origin" into `None`.
    let origin = if aeth_devkit_core::git::is_git_tracked(&root) {
      aeth_devkit_core::git::origin_url(&root).ok().flatten()
    } else {
      None
    };

    Ok(Self {
      root,
      package,
      dependencies,
      has_docker,
      name: project_name,
      version,
      origin,
      docker_services,
      docker_legacy_keys,
      docker_files,
      silence_unlisted_services_warning,
      python_dir,
      has_rust,
      has_container_crate,
      publish_index,
    })
  }

  pub fn has_dependency(&self, name: &str) -> bool {
    self.dependencies.contains(&normalize_dist_name(name))
  }

  /// The project depends on aeth_ext, or *is* aeth_ext: both get its compose conventions
  /// (the ALERTS_* environment).
  pub fn uses_aeth_ext(&self) -> bool {
    self.has_dependency("aeth-ext") || normalize_dist_name(&self.name) == "aeth-ext"
  }

  /// Replace `${workspaceFolder}` with the project root and normalize separators.
  pub fn resolve_workspace_var(&self, value: &str) -> PathBuf {
    let replaced = value.replace("${workspaceFolder}", &self.root.to_string_lossy());
    PathBuf::from(replaced.replace('/', std::path::MAIN_SEPARATOR_STR))
  }
}

/// `[tool.docker].services` as written: `None` when the key is absent. The only Docker
/// switch, so a value that cannot mean anything is an error: treated as empty it would
/// silently turn Docker off with no other signal. `cli` compares HEAD's against the
/// working copy's with this same reading.
pub fn services_key(doc: &toml_edit::DocumentMut) -> Result<Option<Vec<String>>> {
  let Some(item) = doc.get("tool").and_then(|t| t.get("docker")).and_then(|d| d.get("services")) else {
    return Ok(None);
  };
  // `and_then` flattens "not an array" and "an array with a non-string" into one `None`.
  item
    .as_array()
    .and_then(|a| a.iter().map(|v| v.as_str().map(str::to_string)).collect())
    .map(Some)
    .with_context(|| {
      format!(
        "[tool.docker].services must be an array of service names, got {}",
        item.to_string().trim()
      )
    })
}

/// See `ProjectContext::docker_files`.
fn docker_files_in(root: &Path) -> bool {
  let dockerfile_in = |dir: &Path| {
    std::fs::read_dir(dir)
      .map(|rd| {
        rd.flatten()
          .any(|e| e.path().is_file() && e.file_name().to_string_lossy().starts_with("Dockerfile"))
      })
      .unwrap_or(false)
  };
  dockerfile_in(root)
    || dockerfile_in(&root.join("docker"))
    || aeth_devkit_core::compose::find_compose_file(root).ok().flatten().is_some()
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

  fn project(pyproject: &str, files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
    for f in files {
      let p = dir.path().join(f);
      std::fs::create_dir_all(p.parent().unwrap()).unwrap();
      std::fs::write(p, "").unwrap();
    }
    dir
  }

  #[test]
  fn services_is_the_only_switch() {
    let on = project("[project]\nname = \"p\"\n[tool.docker]\nservices = [\"p\"]\n", &[]);
    let ctx = ProjectContext::discover(on.path()).unwrap();
    assert!(ctx.has_docker);
    assert_eq!(ctx.docker_services, vec!["p"]);
    // Real Docker files without a services list are not a Docker setup any more…
    let files = project("[project]\nname = \"p\"\n", &["docker/Dockerfile", "docker/compose.yaml"]);
    let ctx = ProjectContext::discover(files.path()).unwrap();
    assert!(!ctx.has_docker);
    // …but the seed and the advisory need to know they exist.
    assert!(ctx.docker_files);
    let empty = project("[project]\nname = \"p\"\n[tool.docker]\nservices = []\n", &[]);
    let ctx = ProjectContext::discover(empty.path()).unwrap();
    assert!(!ctx.has_docker && !ctx.docker_files);
    // A value that cannot be a service list is an error, not "no Docker".
    for bad in ["services = \"p\"", "services = [{ name = \"p\" }]", "services = [\"p\", 1]"] {
      let dir = project(&format!("[project]\nname = \"p\"\n[tool.docker]\n{bad}\n"), &[]);
      let err = ProjectContext::discover(dir.path()).unwrap_err().to_string();
      assert!(err.contains("services must be an array"), "{bad}: {err}");
    }
  }

  #[test]
  fn legacy_keys_name_and_version_are_read() {
    let dir = project(
      "[project]\nname = \"Aeth-Ext\"\nversion = \"8.1.0\"\n[tool.docker]\nchown_paths = [\"x\"]\nmkdirs = [\"\"]\n",
      &[],
    );
    let ctx = ProjectContext::discover(dir.path()).unwrap();
    assert_eq!(ctx.docker_legacy_keys, vec!["chown_paths", "mkdirs"]);
    assert_eq!(ctx.name, "Aeth-Ext");
    assert_eq!(ctx.version.as_deref(), Some("8.1.0"));
    assert!(ctx.uses_aeth_ext(), "aeth_ext itself counts");
    assert_eq!(ctx.origin, None, "not a git repo");
  }

  #[test]
  fn a_dependency_on_aeth_ext_counts() {
    let dir = project("[project]\nname = \"p\"\ndependencies = [\"aeth-ext[sftp]>=8\"]\n", &[]);
    assert!(ProjectContext::discover(dir.path()).unwrap().uses_aeth_ext());
    let dir = project("[project]\nname = \"p\"\ndependencies = [\"requests\"]\n", &[]);
    assert!(!ProjectContext::discover(dir.path()).unwrap().uses_aeth_ext());
  }
}

#[cfg(test)]
mod publish_index_detection {
  use super::*;

  fn project(pyproject: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
    dir
  }

  #[test]
  fn the_sole_publish_index_is_named() {
    let dir = project(
      "[project]\nname = \"p\"\n\n[[tool.uv.index]]\nname = \"Ro\"\nurl = \"https://x/+simple\"\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://y/+simple\"\npublish-url = \"https://y/\"\n",
    );
    assert_eq!(
      ProjectContext::discover(dir.path()).unwrap().publish_index.as_deref(),
      Some("SFTPyPI")
    );
  }

  #[test]
  fn no_publish_index_is_none() {
    let dir = project("[project]\nname = \"p\"\n\n[[tool.uv.index]]\nname = \"Ro\"\nurl = \"https://x/+simple\"\n");
    assert_eq!(ProjectContext::discover(dir.path()).unwrap().publish_index, None);
  }

  #[test]
  fn several_publish_indexes_is_an_error() {
    let dir = project(
      "[project]\nname = \"p\"\n\n[[tool.uv.index]]\nname = \"A\"\nurl = \"https://a/+simple\"\npublish-url = \"https://a/\"\n\n[[tool.uv.index]]\nname = \"B\"\nurl = \"https://b/+simple\"\npublish-url = \"https://b/\"\n",
    );
    let err = ProjectContext::discover(dir.path()).unwrap_err().to_string();
    assert!(err.contains("A, B"), "{err}");
  }
}
