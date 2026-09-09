//! The devkit packages `setup-project` keeps current in a project (spec section 4.0): which
//! they are, how the venv holds them, what the lock says about them, and [`advance`],
//! which locks them under the running devkit, writes the `{latest}` floors and syncs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};
use toml_edit::{DocumentMut, Item};

use aeth_devkit_core::commit::TrackedBase;
use aeth_devkit_core::process::Runner;
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

/// The Claude Code hooks: `devkit-hook <name>`, wired into `.claude/settings.local.json`
/// by the settings template.
pub const HOOKS: DevkitPackage = DevkitPackage {
  name: "devkit-claude-hooks",
  import_name: "devkit_claude_hooks",
};

/// poe shell completion: `devkit-complete`, whose shims the run installs (step 15).
pub const COMPLETE: DevkitPackage = DevkitPackage {
  name: "devkit-poe-complete",
  import_name: "devkit_poe_complete",
};

/// devkit itself, for the lookups that read its own package data (the templates).
pub const DEVKIT: DevkitPackage = DevkitPackage {
  name: "aeth-devkit",
  import_name: "aeth_devkit",
};

/// The devkit packages this project should carry: the hooks and the completion for every
/// project, the container for Docker projects. The container's condition is `[tool.docker].services`, the same `if-docker-services` gate
/// the template adds the dependency under, so a dependency the merge adds is always one this
/// step locks and installs. A project never carries itself, so each satellite repo can be
/// devkit-managed without depending on its own name.
pub fn active(ctx: &ProjectContext) -> Vec<&'static DevkitPackage> {
  let own = normalize_dist_name(&ctx.name);
  let mut out = vec![&HOOKS, &COMPLETE];
  if ctx.has_docker {
    out.push(&CONTAINER);
  }
  out.retain(|p| normalize_dist_name(p.name) != own);
  out
}

/// A devkit package as an environment holds it: where its files are (the Dockerfile
/// template is read from `dir`) and the distribution version installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
  pub dir: PathBuf,
  pub version: String,
}

/// What the project at `root` has installed of a devkit package. A trait so tests can
/// answer from a fixture instead of a real environment.
pub trait Venv {
  fn installed(&self, root: &Path, package: &DevkitPackage) -> Option<Installed>;
}

/// The project's own environment: `UV_PROJECT_ENVIRONMENT` when set (relative to the
/// project root, as uv reads it), else `<root>/.venv`. That is where the locked package
/// lives whichever devkit binary is running: the venv's, `target/debug`'s or a tool
/// install's. No fallback to the interpreter beside the binary or on PATH, because those
/// can answer from another environment with a version the project's lock does not name.
pub struct SystemVenv;

impl Venv for SystemVenv {
  fn installed(&self, root: &Path, package: &DevkitPackage) -> Option<Installed> {
    let env = std::env::var_os("UV_PROJECT_ENVIRONMENT").map_or_else(|| PathBuf::from(".venv"), PathBuf::from);
    let env = if env.is_absolute() { env } else { root.join(env) };
    ["Scripts/python.exe", "bin/python"]
      .iter()
      .find_map(|rel| probe(&env.join(rel), package))
  }
}

/// `package` as the interpreter at `python` has it, or `None` when that interpreter cannot
/// be spawned (`python.exe` on Unix) or lacks the package. The version comes from
/// `importlib.metadata`, not from a `dist-info` directory beside the package: an editable
/// install keeps the two in different places, and a distribution's name need not match its
/// import name.
pub fn probe(python: &Path, package: &DevkitPackage) -> Option<Installed> {
  let (name, import_name) = (package.name, package.import_name);
  let code = format!(
    "import importlib.metadata as m, os, {import_name}; print(os.path.dirname({import_name}.__file__)); print(m.version('{name}'))"
  );
  // `-X utf8`: a piped stdout is otherwise the ANSI code page, which mangles a non-ASCII path.
  let out = Command::new(python).args(["-X", "utf8", "-c", &code]).output().ok()?;
  if !out.status.success() {
    return None;
  }
  let stdout = String::from_utf8_lossy(&out.stdout);
  let mut lines = stdout.lines().map(str::trim);
  let dir = PathBuf::from(lines.next()?);
  let version = lines.next()?.to_string();
  (dir.is_dir() && !version.is_empty()).then_some(Installed { dir, version })
}

/// Canned answers by import name, whatever the root; for tests.
#[derive(Default)]
pub struct StubVenv(pub HashMap<String, Installed>);

impl Venv for StubVenv {
  fn installed(&self, _root: &Path, package: &DevkitPackage) -> Option<Installed> {
    self.0.get(package.import_name).cloned()
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
  let root = &ctx.root;
  let lock_path = root.join("uv.lock");
  let lock_before = crate::read_optional(&lock_path)?;
  // The constraint is the running binary's version, so a lock that already names another
  // devkit means the venv is out of step with the lock: the lock step must not paper over
  // that by moving devkit's entry to match the binary. On a committing run this is HEAD's
  // lock (the run merges against HEAD), so a lock moved but not committed reads as stale.
  // A dry run reports it as a problem: `--check` must not pass a project a plain run refuses.
  if let Some(v) = lock_before.as_deref().and_then(|l| locked_registry_version(l, "aeth-devkit"))
    && v != RUNNING_DEVKIT
  {
    let message = format!(
      "uv.lock pins aeth-devkit {v} but this devkit is {RUNNING_DEVKIT}; run `uv sync --frozen` so the venv matches the lock (or commit a uv.lock you already moved), then rerun setup-project"
    );
    if !dry_run {
      bail!(message);
    }
    changes.problems.push(message);
  }
  if dry_run {
    for p in packages.iter().filter(|p| deps.venv.installed(&ctx.root, p).is_none()) {
      changes.notes.push(format!(
        "{} is not installed in this venv; a plain run adds it to pyproject.toml, locks it and syncs.",
        p.name
      ));
    }
    return Ok(());
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
    // Once every package is locked the lock stays usable, so a refresh that fails (no
    // network, most often: `--upgrade-package` re-fetches the index page) is a warning and
    // the run goes on with the versions it names. Before adoption there is nothing to fall
    // back on.
    let adopted = lock_before
      .as_deref()
      .is_some_and(|l| packages.iter().all(|p| locked_version(l, p.name).is_some()));
    if !adopted {
      bail!("uv lock failed: {stderr}");
    }
    changes
      .warnings
      .push(format!("uv lock failed; the locked devkit packages stay as they are: {stderr}"));
  }
  let mut lock_after = std::fs::read_to_string(&lock_path).context("reading uv.lock after locking")?;
  let pyproject_path = root.join("pyproject.toml");
  let text = std::fs::read_to_string(&pyproject_path).context("reading pyproject.toml")?;
  let mut doc: DocumentMut = text.parse().context("parsing pyproject.toml")?;
  let mut pinned: Vec<String> = Vec::new();
  let mut locked_all: Vec<(&DevkitPackage, String)> = Vec::new();
  for p in &packages {
    let locked = locked_version(&lock_after, p.name).with_context(|| {
      format!(
        "{} is not in uv.lock after locking; the merge lists every active devkit package in pyproject.toml, so the lock should carry it",
        p.name
      )
    })?;
    if latest.iter().any(|n| n == p.name)
      && let Some(req) = find_requirement(&doc, p.name)
    {
      let spec = req.spec.trim();
      let bare = normalize_dist_name(spec) == normalize_dist_name(p.name);
      // Only an index release can be a published floor: a path or editable source (the
      // package's own checkout beside the project) locks a version no index serves. And a
      // compatible-release clause (`~=`) is the user's ceiling as much as a floor; moving
      // its version would narrow it.
      let new_spec = match locked_registry_version(&lock_after, p.name) {
        None => None,
        Some(_) if bare => Some(format!("{}>={locked}", p.name)),
        Some(_) if spec.contains("~=") => None,
        Some(_) => set_requirement_version(&req.spec, &locked),
      };
      match new_spec {
        Some(new_spec) if new_spec != req.spec => {
          pinned.push(if bare {
            format!("pinned {new_spec}")
          } else {
            format!("pinned {new_spec} (was {})", req.spec)
          });
          replace_requirement(&mut doc, &req, &new_spec);
        }
        Some(_) => {}
        // Extras, markers, `~=`, a non-index source: the requirement is left as written,
        // and the user hears why the floor did not move.
        None => changes.notes.push(format!(
          "{}: `{}` was left as written (locked {locked}); a floor is only written for a plain name or a `>=` requirement locked from an index.",
          p.name, req.spec
        )),
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
    locked_all.push((p, locked));
  }
  if !pinned.is_empty() {
    changes.record(&pyproject_path, &text, &doc.to_string(), pinned)?;
    // The floor is a requirement uv has already copied into the lock's `requires-dist`, so
    // the file uv just wrote is out of date the moment it lands, and the next `uv run` would
    // rewrite it behind the commit. Preferences keep every version; this pass only refreshes
    // that metadata.
    let out = deps.docker.runner.run_capture("uv", &["lock".into()], root)?;
    if !out.success() {
      bail!("uv lock failed after writing the floor: {}", out.stderr.trim());
    }
    lock_after = std::fs::read_to_string(&lock_path).context("reading uv.lock after locking")?;
  }
  // The venv must hold what the lock names before the Docker step reads the template from
  // it. Asked again after the sync: one that lands elsewhere (another
  // `UV_PROJECT_ENVIRONMENT`, say) would otherwise hand that step nothing, and every later
  // run would sync again to no effect.
  let lagging = || -> Vec<String> {
    locked_all
      .iter()
      .filter(|(p, locked)| deps.venv.installed(root, p).is_none_or(|i| i.version != *locked))
      .map(|(p, locked)| format!("{} {locked}", p.name))
      .collect()
  };
  let lock_changed = lock_before.as_deref() != Some(lock_after.as_str());
  if lock_changed || !lagging().is_empty() {
    match deps.docker.runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root)? {
      Some(0) => {}
      Some(code) => bail!("uv sync --frozen exited with {code}"),
      None => bail!("uv sync --frozen was terminated by a signal"),
    }
    changes.venv_synced = true;
    let still = lagging();
    if !still.is_empty() {
      bail!(
        "{} locked but not in the project's environment after `uv sync --frozen`; is it elsewhere (UV_PROJECT_ENVIRONMENT)?",
        still.join(", ")
      );
    }
  }
  // uv wrote the lock; recording it like every managed file registers it as devkit's (a
  // gitignored lock is then warned about) and marks a first lock as created. The rewrite
  // is byte-identical.
  let detail = format!(
    "locked {}",
    locked_all
      .iter()
      .map(|(p, v)| format!("{} {v}", p.name))
      .collect::<Vec<_>>()
      .join(", ")
  );
  changes.record_optional(&lock_path, lock_before.as_deref(), &lock_after, vec![detail])?;
  Ok(())
}

/// After a committing run has put the user's `uv.lock` back — merged with this run's
/// changes, or restored by a rollback — bring the venv in step with it. [`advance`] synced
/// against the lock as staged, which is HEAD's copy whenever the user's differed, so the
/// venv would otherwise match neither the file on disk nor their uncommitted work. A
/// failure is a warning: the files are already right, and the next `uv run` syncs again.
pub fn resync_after_replay(root: &Path, runner: &dyn Runner, bases: &[TrackedBase], changes: &Changes) {
  let lock_was_staged = bases
    .iter()
    .any(|b| b.path == "uv.lock" && b.head.is_some() && b.worktree.as_deref() != b.head.as_deref());
  if !(changes.venv_synced && lock_was_staged) {
    return;
  }
  println!("Syncing the venv to the uv.lock now in the working tree");
  let outcome = match runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root) {
    Ok(Some(0)) => return,
    Ok(Some(code)) => format!("uv sync --frozen exited with {code}"),
    Ok(None) => "uv sync --frozen was terminated by a signal".to_string(),
    Err(e) => format!("could not run uv sync --frozen: {e:#}"),
  };
  eprintln!("warning: {outcome}; run `uv sync` to bring the venv in step with uv.lock");
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
  fn hooks_and_completion_are_always_active_the_container_only_with_docker() {
    let dir = tempfile::tempdir().unwrap();
    let names = |pyproject: &str| {
      std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
      let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
      active(&ctx).iter().map(|p| p.name).collect::<Vec<_>>()
    };
    assert_eq!(
      names("[project]\nname = \"p\"\n"),
      vec!["devkit-claude-hooks", "devkit-poe-complete"]
    );
    assert_eq!(
      names("[project]\nname = \"p\"\n[tool.docker]\nservices = [\"p\"]\n"),
      vec!["devkit-claude-hooks", "devkit-poe-complete", "devkit-container"]
    );
    // A satellite never carries itself, under any spelling of its name.
    assert_eq!(names("[project]\nname = \"devkit-claude-hooks\"\n"), vec!["devkit-poe-complete"]);
    assert_eq!(names("[project]\nname = \"Devkit_Poe_Complete\"\n"), vec!["devkit-claude-hooks"]);
    assert_eq!(
      names("[project]\nname = \"devkit_container\"\n[tool.docker]\nservices = [\"x\"]\n"),
      vec!["devkit-claude-hooks", "devkit-poe-complete"]
    );
  }

  #[test]
  fn latest_requested_lists_the_placeholder_requirements() {
    let tpl = "[project]\n  dependencies = [\"devkit-container>={latest}\", \"requests>=2\"]\n[dependency-groups]\n  dev = [\"Devkit_Templates>={latest}\"]\n";
    assert_eq!(latest_requested(tpl), vec!["devkit-container", "devkit-templates"]);
    assert!(latest_requested("[tool.x]\n").is_empty());
  }

  #[test]
  fn the_stub_venv_answers_from_the_map() {
    let installed = Installed {
      dir: PathBuf::from("/site/devkit_container"),
      version: "1.4.0".into(),
    };
    let mut map = HashMap::new();
    map.insert("devkit_container".to_string(), installed.clone());
    let venv = StubVenv(map);
    let root = Path::new("/p");
    assert_eq!(venv.installed(root, &CONTAINER), Some(installed));
    assert_eq!(venv.installed(root, &DEVKIT), None);
  }

  #[test]
  fn the_probe_reads_the_workspace_venv() {
    // The workspace venv has aeth-devkit installed editable, which is the layout a
    // dist-info scan beside the package would miss; skipped where there is no venv (CI's
    // Rust job builds without one).
    let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".venv");
    let Some(python) = ["Scripts/python.exe", "bin/python"]
      .iter()
      .map(|rel| venv.join(rel))
      .find(|p| p.is_file())
    else {
      return;
    };
    let found = probe(&python, &DEVKIT).expect("aeth-devkit is installed in the workspace venv");
    assert!(parse_lenient(&found.version).is_some(), "{:?}", found.version);
    assert!(found.dir.join("templates").is_dir(), "{}", found.dir.display());
    assert_eq!(
      probe(
        &python,
        &DevkitPackage {
          name: "devkit-nonexistent",
          import_name: "devkit_nonexistent",
        }
      ),
      None
    );
  }
}
