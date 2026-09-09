//! The devkit packages `setup-project` keeps current in a project (spec section 4.0): which
//! they are, where the venv keeps them, what the lock says about them, and [`advance`],
//! which locks them under the running devkit, writes the `{latest}` floors and syncs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use toml_edit::{DocumentMut, Item};

use aeth_devkit_core::pyproject::{
  find_requirement, index_url_for, normalize_dist_name, replace_requirement, set_requirement_version,
};
use aeth_devkit_core::version::{latest_stable, parse_lenient};

use crate::changes::Changes;
use crate::context::ProjectContext;

/// The version of the devkit running this code; every resolution is constrained to it so
/// devkit is never upgraded as a side effect (spec 4.0). All workspace crates share one
/// version, so the setup crate's is the binary's.
pub const RUNNING_DEVKIT: &str = env!("CARGO_PKG_VERSION");

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
/// hooks and completion packages join in later split steps. The container's condition is the
/// template's `if-docker` gate (services listed, or Docker files present), so a dependency
/// the merge adds is always one this step locks and installs. A project never carries
/// itself, so each satellite repo can be devkit-managed without depending on its own name.
pub fn active(ctx: &ProjectContext) -> Vec<&'static DevkitPackage> {
  let own = normalize_dist_name(&ctx.name);
  let mut out = Vec::new();
  if ctx.has_docker || ctx.docker_files {
    out.push(&CONTAINER);
  }
  out.retain(|p| normalize_dist_name(p.name) != own);
  out
}

/// Where the project at `root` keeps an installed package, by import name. A trait so tests
/// can point at a fixture instead of a real site-packages.
pub trait PackageDirs {
  fn dir(&self, root: &Path, import_name: &str) -> Option<PathBuf>;
}

/// Asks the project's own venv (`<root>/.venv`), which is where the locked package lives
/// whichever devkit binary is running: the venv's, `target/debug`'s or a tool install's. No
/// fallback to the interpreter beside the binary or on PATH, because those can answer from
/// another environment with a version the project's lock does not name.
pub struct SystemPackageDirs;

impl PackageDirs for SystemPackageDirs {
  fn dir(&self, root: &Path, import_name: &str) -> Option<PathBuf> {
    [".venv/Scripts/python.exe", ".venv/bin/python"]
      .iter()
      .find_map(|rel| crate::templates::package_dir_via(&root.join(rel), import_name))
  }
}

/// Canned answers by import name, whatever the root; for tests.
#[derive(Default)]
pub struct StubPackageDirs(pub HashMap<String, PathBuf>);

impl PackageDirs for StubPackageDirs {
  fn dir(&self, _root: &Path, import_name: &str) -> Option<PathBuf> {
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

/// The requirements a rendered pyproject template marks `{latest}`, by normalised name, so
/// `advance` knows whose floor to write after locking.
pub fn latest_requested(template: &str) -> Vec<String> {
  let Ok(doc) = template.parse::<DocumentMut>() else {
    return Vec::new();
  };
  let mut arrays: Vec<&toml_edit::Array> = Vec::new();
  if let Some(a) = doc.get("project").and_then(|p| p.get("dependencies")).and_then(Item::as_array) {
    arrays.push(a);
  }
  if let Some(groups) = doc.get("dependency-groups").and_then(Item::as_table_like) {
    arrays.extend(groups.iter().filter_map(|(_, v)| v.as_array()));
  }
  arrays
    .iter()
    .flat_map(|a| a.iter())
    .filter_map(|v| v.as_str())
    .filter(|s| s.contains(crate::toml_merge::LATEST))
    .map(crate::context::dependency_name)
    .collect()
}

/// Bring the active devkit packages to the newest release the running devkit accepts, write
/// the floors the template marked `{latest}`, and sync the venv. Runs after the pyproject
/// merge has listed the packages and before anything reads them from the venv.
pub fn advance(ctx: &ProjectContext, deps: &crate::Deps, dry_run: bool, latest: &[String], changes: &mut Changes) -> Result<()> {
  let packages = active(ctx);
  if packages.is_empty() {
    return Ok(());
  }
  if dry_run {
    for p in packages.iter().filter(|p| deps.packages.dir(&ctx.root, p.import_name).is_none()) {
      changes.notes.push(format!(
        "{} is not installed in this venv; a plain run adds it to pyproject.toml, locks it and syncs.",
        p.name
      ));
    }
    return Ok(());
  }
  let root = &ctx.root;
  let lock_path = root.join("uv.lock");
  let lock_before = crate::read_optional(&lock_path)?;
  // The constraint is the running binary's version, so a lock that already names another
  // devkit means the venv is out of step with the lock: the lock step must not paper over
  // that by moving devkit's entry to match the binary. On a committing run this is HEAD's
  // lock (the run merges against HEAD), so a lock moved but not committed reads as stale.
  if let Some(v) = lock_before.as_deref().and_then(|l| locked_registry_version(l, "aeth-devkit"))
    && v != RUNNING_DEVKIT
  {
    bail!(
      "uv.lock pins aeth-devkit {v} but this devkit is {RUNNING_DEVKIT}; run `uv sync` so the venv matches the lock (or commit a uv.lock you already moved), then rerun setup-project"
    );
  }
  // `uv lock` has no constraints flag; a version specifier on `--upgrade-package` is a hard
  // constraint for this resolution (verified: an unmeetable one is "No solution found"), so
  // `aeth-devkit==<running>` rides beside the packages allowed to move. Every other locked
  // package stays a preference, as spec 4.0 wants.
  let mut args: Vec<String> = vec!["lock".into()];
  for p in &packages {
    args.push("--upgrade-package".into());
    args.push(p.name.into());
  }
  args.push("--upgrade-package".into());
  args.push(format!("aeth-devkit=={RUNNING_DEVKIT}"));
  let out = deps.docker.runner.run_capture("uv", &args, root)?;
  if !out.success() {
    let stderr = out.stderr.trim();
    // uv's report starts at the `×` headline; what precedes it (interpreter discovery) is
    // noise, and the `╰─▶` lines after it are the reason the user needs.
    if let Some(at) = stderr.find("No solution found") {
      let reason = stderr[at..].lines().map(str::trim).collect::<Vec<_>>().join("\n  ");
      bail!(
        "a devkit package's floor cannot be met by the running devkit {RUNNING_DEVKIT}:\n  {reason}\nrun `devkit lock`, then rerun setup-project"
      );
    }
    bail!("uv lock failed: {stderr}");
  }
  let lock_after = std::fs::read_to_string(&lock_path).context("reading uv.lock after locking")?;
  let pyproject_path = root.join("pyproject.toml");
  let text = std::fs::read_to_string(&pyproject_path).context("reading pyproject.toml")?;
  let mut doc: DocumentMut = text.parse().context("parsing pyproject.toml")?;
  let mut pinned: Vec<String> = Vec::new();
  let mut locked_all: Vec<String> = Vec::new();
  // Whether the venv must be synced: a package absent or installed at another version
  // than the lock names would hand the Docker step the wrong template.
  let mut stale = false;
  for p in &packages {
    let locked = locked_version(&lock_after, p.name).with_context(|| format!("{} is not in uv.lock after locking", p.name))?;
    locked_all.push(format!("{} {locked}", p.name));
    let installed = deps.packages.dir(root, p.import_name).and_then(|d| installed_version(&d));
    stale |= installed.as_deref() != Some(locked.as_str());
    if latest.iter().any(|n| n == p.name)
      && let Some(req) = find_requirement(&doc, p.name)
    {
      let bare = req.spec.trim() == p.name;
      let new_spec = if bare {
        Some(format!("{}>={locked}", p.name))
      } else {
        set_requirement_version(&req.spec, &locked)
      };
      if let Some(new_spec) = new_spec
        && new_spec != req.spec
      {
        pinned.push(if bare {
          format!("pinned {new_spec}")
        } else {
          format!("pinned {new_spec} (was {})", req.spec)
        });
        replace_requirement(&mut doc, &req, &new_spec);
      }
    }
    // The nudge: newer on the index than uv could choose under the constraint.
    if let Some(url) = index_url_for(&doc, p.name) {
      match deps.index.versions(&url, p.name) {
        Ok(versions) => {
          let newest = latest_stable(versions.iter().map(String::as_str));
          if let (Some(newest), Some(have)) = (newest.as_deref().and_then(parse_lenient), parse_lenient(&locked))
            && newest > have
          {
            changes.warnings.push(format!(
              "{} {newest} is available but {locked} is the newest the running aeth-devkit {RUNNING_DEVKIT} accepts; run `devkit lock`, then rerun setup-project",
              p.name
            ));
          }
        }
        Err(e) => changes
          .warnings
          .push(format!("could not check {url} for a newer {}: {e:#}", p.name)),
      }
    }
  }
  if !pinned.is_empty() {
    // A managed-file write like every other: a Ctrl-C waits for it (see `interrupt`).
    let _w = crate::interrupt::Writing::begin();
    std::fs::write(&pyproject_path, doc.to_string()).context("writing pyproject.toml")?;
    for line in &pinned {
      changes.note(&pyproject_path, line);
    }
  }
  let lock_changed = lock_before.as_deref() != Some(lock_after.as_str());
  if lock_changed || stale {
    match deps.docker.runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root)? {
      Some(0) => {}
      Some(code) => bail!("uv sync --frozen exited with {code}"),
      None => bail!("uv sync --frozen was terminated by a signal"),
    }
  }
  if lock_changed {
    changes.note(&lock_path, &format!("locked {}", locked_all.join(", ")));
  }
  Ok(())
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
  fn latest_requested_lists_the_placeholder_requirements() {
    let tpl = "[project]\n  dependencies = [\"devkit-container>={latest}\", \"requests>=2\"]\n[dependency-groups]\n  dev = [\"Devkit_Templates>={latest}\"]\n";
    assert_eq!(latest_requested(tpl), vec!["devkit-container", "devkit-templates"]);
    assert!(latest_requested("[tool.x]\n").is_empty());
  }

  #[test]
  fn stub_dirs_answer_from_the_map() {
    let mut map = std::collections::HashMap::new();
    map.insert("devkit_container".to_string(), PathBuf::from("/site/devkit_container"));
    let dirs = StubPackageDirs(map);
    let root = Path::new("/p");
    assert_eq!(dirs.dir(root, "devkit_container"), Some(PathBuf::from("/site/devkit_container")));
    assert_eq!(dirs.dir(root, "other"), None);
  }
}
