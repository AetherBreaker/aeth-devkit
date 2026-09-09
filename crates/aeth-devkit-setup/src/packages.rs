//! The devkit packages `setup-project` keeps current in a project (spec section 4.0): which
//! they are, where the venv keeps them, and what the lock says about them. The advancing
//! itself lives in `advance`, added in a later task.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use aeth_devkit_core::pyproject::normalize_dist_name;

use crate::context::ProjectContext;

/// A package devkit owns and `setup-project` installs and advances. `name` is the
/// distribution name (index, pyproject); `import_name` is the directory in site-packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevkitPackage {
  pub name: &'static str,
  pub import_name: &'static str,
}

/// The image-side helper: a runtime dependency of every project with Docker services, whose
/// wheel also carries the Dockerfile template `setup-project` renders.
pub const CONTAINER: DevkitPackage = DevkitPackage {
  name: "devkit-container",
  import_name: "devkit_container",
};

/// The devkit packages this project should carry. Only the container so far; the templates,
/// hooks and completion packages join in later split steps. A project never carries itself,
/// so each satellite repo can be devkit-managed without depending on its own name.
pub fn active(ctx: &ProjectContext) -> Vec<&'static DevkitPackage> {
  let own = normalize_dist_name(&ctx.name);
  let mut out = Vec::new();
  if ctx.has_docker {
    out.push(&CONTAINER);
  }
  out.retain(|p| normalize_dist_name(p.name) != own);
  out
}

/// Where the venv keeps an installed package, by import name. A trait so tests can point at
/// a fixture instead of a real site-packages.
pub trait PackageDirs {
  fn dir(&self, import_name: &str) -> Option<PathBuf>;
}

/// Asks the project's own venv (`<root>/.venv`), which is where the locked package lives
/// whichever devkit binary is running: the venv's, `target/debug`'s or a tool install's. No
/// fallback to the interpreter beside the binary or on PATH, because those can answer from
/// another environment with a version the project's lock does not name.
pub struct SystemPackageDirs {
  pub root: PathBuf,
}

impl PackageDirs for SystemPackageDirs {
  fn dir(&self, import_name: &str) -> Option<PathBuf> {
    [".venv/Scripts/python.exe", ".venv/bin/python"]
      .iter()
      .find_map(|rel| crate::templates::package_dir_via(&self.root.join(rel), import_name))
  }
}

/// Canned answers by import name; for tests.
#[derive(Default)]
pub struct StubPackageDirs(pub HashMap<String, PathBuf>);

impl PackageDirs for StubPackageDirs {
  fn dir(&self, import_name: &str) -> Option<PathBuf> {
    self.0.get(import_name).cloned()
  }
}

fn locked_entry<'a>(doc: &'a DocumentMut, name: &str) -> Option<&'a toml_edit::Table> {
  let want = normalize_dist_name(name);
  doc
    .get("package")?
    .as_array_of_tables()?
    .iter()
    .find(|t| t.get("name").and_then(Item::as_str).is_some_and(|n| normalize_dist_name(n) == want))
}

/// The version `uv.lock` holds for `name`, whatever its source. `None` for an unparsable
/// lock, so a missing or half-written file reads as "not locked" rather than an error.
pub fn locked_version(lock: &str, name: &str) -> Option<String> {
  let doc: DocumentMut = lock.parse().ok()?;
  locked_entry(&doc, name)?.get("version")?.as_str().map(str::to_string)
}

/// Like [`locked_version`], but only for a package that comes from an index: the project
/// itself appears in its own lock as an editable entry and is not a pin.
pub fn locked_registry_version(lock: &str, name: &str) -> Option<String> {
  let doc: DocumentMut = lock.parse().ok()?;
  let entry = locked_entry(&doc, name)?;
  entry.get("source")?.as_table_like()?.get("registry")?;
  entry.get("version")?.as_str().map(str::to_string)
}

/// The installed version of the package at `package_dir`, read from the
/// `<import_name>-<version>.dist-info` directory beside it. No interpreter call: the
/// directory name is the metadata.
pub fn installed_version(package_dir: &Path) -> Option<String> {
  let import_name = package_dir.file_name()?.to_string_lossy().into_owned();
  let prefix = format!("{import_name}-");
  std::fs::read_dir(package_dir.parent()?).ok()?.flatten().find_map(|e| {
    let file = e.file_name().to_string_lossy().into_owned();
    let stem = file.strip_suffix(".dist-info")?;
    stem.strip_prefix(&prefix).map(str::to_string)
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  const LOCK: &str = r#"version = 1
requires-python = ">=3.14"

[[package]]
name = "aeth-devkit"
version = "11.0.1"
source = { registry = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple" }

[[package]]
name = "demo-app"
version = "1.2.3"
source = { editable = "." }

[[package]]
name = "devkit-container"
version = "1.4.0"
source = { registry = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple" }
"#;

  #[test]
  fn locked_versions_are_read_by_normalised_name() {
    assert_eq!(locked_version(LOCK, "devkit_container").as_deref(), Some("1.4.0"));
    assert_eq!(locked_version(LOCK, "Demo-App").as_deref(), Some("1.2.3"));
    assert_eq!(locked_version(LOCK, "missing"), None);
    assert_eq!(locked_version("not toml [", "x"), None);
  }

  #[test]
  fn registry_versions_skip_the_editable_root() {
    assert_eq!(locked_registry_version(LOCK, "aeth-devkit").as_deref(), Some("11.0.1"));
    assert_eq!(locked_registry_version(LOCK, "demo-app"), None, "the project itself is not a pin");
  }

  #[test]
  fn installed_version_comes_from_the_dist_info_beside_the_package() {
    let site = tempfile::tempdir().unwrap();
    let pkg = site.path().join("devkit_container");
    std::fs::create_dir_all(&pkg).unwrap();
    assert_eq!(installed_version(&pkg), None);
    std::fs::create_dir(site.path().join("devkit_container-1.4.0.dist-info")).unwrap();
    std::fs::create_dir(site.path().join("devkit_other-9.9.9.dist-info")).unwrap();
    assert_eq!(installed_version(&pkg).as_deref(), Some("1.4.0"));
  }

  #[test]
  fn the_container_is_active_only_for_docker_projects() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
      dir.path().join("pyproject.toml"),
      "[project]\nname = \"p\"\n[tool.docker]\nservices = [\"p\"]\n",
    )
    .unwrap();
    let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
    assert_eq!(active(&ctx).iter().map(|p| p.name).collect::<Vec<_>>(), vec!["devkit-container"]);
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"p\"\n").unwrap();
    let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
    assert!(active(&ctx).is_empty());
    // The container repo itself has no reason to depend on its own wheel.
    std::fs::write(
      dir.path().join("pyproject.toml"),
      "[project]\nname = \"devkit_container\"\n[tool.docker]\nservices = [\"x\"]\n",
    )
    .unwrap();
    let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
    assert!(active(&ctx).is_empty(), "a project never carries itself");
  }

  #[test]
  fn stub_dirs_answer_from_the_map() {
    let mut map = std::collections::HashMap::new();
    map.insert("devkit_container".to_string(), PathBuf::from("/site/devkit_container"));
    let dirs = StubPackageDirs(map);
    assert_eq!(dirs.dir("devkit_container"), Some(PathBuf::from("/site/devkit_container")));
    assert_eq!(dirs.dir("other"), None);
  }
}
