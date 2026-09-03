# CI Release Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `devkit release` stops building and publishing locally; it creates the GitHub release and waits for a devkit-installed GitHub Actions workflow that builds every artefact, attaches it to the release, and publishes to the project's index (or PyPI when there is none).

**Architecture:** The release crate loses its `uv build` / `uv publish` steps and their rollback entries, gains a `PublishTarget` (private index or PyPI) and a `ci` module that polls `gh run list`, watches the run, and verifies the version landed on the index. The setup crate gains two devkit-owned workflow templates (pure Python, and a maturin matrix for Rust projects), a `{publish_index}` / `{publish_index_key}` placeholder pair, and YAML block markers that render exactly one publish step.

**Tech Stack:** Rust 2024 (clap, anyhow, toml_edit), `aeth_devkit_core::process::Runner` / `RecordingRunner`, `gh` CLI, GitHub Actions (`astral-sh/setup-uv@v5`, `dtolnay/rust-toolchain@stable`, `PyO3/maturin-action@v1`, `actions/upload-artifact@v4`, `actions/download-artifact@v4`), uv.

**Spec:** `docs/superpowers/specs/2026-09-03-ci-release-workflow-design.md`

## Global Constraints

- Every Python-dependent command runs under `uv run`; Rust via `cargo` (see AGENTS.md).
- Tests: on `main` run the full suite normally; on a feature branch run only targeted tests while iterating and the whole suite once at the end.
- `devkit release` must stay project-agnostic: nothing in the release crate may be specific to devkit's own build (no crate names, no asset names, no extension knowledge).
- The workflow file is always installed as `.github/workflows/release.yml` and is devkit-owned: replaced silently on drift, never create-if-missing.
- No publish index → PyPI via `uv publish --trusted-publishing always` with `id-token: write`. Two or more publish indexes → config error.
- Secret names in CI: `UV_INDEX_<KEY>_USERNAME` / `UV_INDEX_<KEY>_PASSWORD` where `<KEY>` is the index name upper-cased with `-` → `_` (the same mapping `config::env_var_names` uses). CI maps them onto `UV_PUBLISH_USERNAME` / `UV_PUBLISH_PASSWORD`.
- Exit codes are unchanged: 0 released / dry run, 1 aborted or rolled back, 2 pre-flight or config error.
- rustfmt: `tab_spaces = 2`, `max_width = 135`. `cargo clippy --workspace --all-targets -- -D warnings` must stay clean.
- Comments carry reasoning, densely (AGENTS.md "Comment Density"). Don't extract helpers of ≤4 lines unless a lint forces it.
- Conventional Commits with the crate as scope; a `fix` body says what the bug was, what caused it, how it is fixed. End every commit message with `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`.
- `.env` holds live credentials: never print it, commit it, or paste its values into a command shown to the user.
- README's per-command feature reference must be updated in the same commit as the behaviour change it documents.

---

## File structure

| File | Responsibility after this plan |
| --- | --- |
| `crates/aeth-devkit-core/src/pyproject.rs` | + `index_env_key(name)` — the `<KEY>` mapping shared by the release crate and setup templates |
| `crates/aeth-devkit-release/src/config.rs` | `Config { package, target: PublishTarget }`; `PublishTarget::{Index, Pypi}`; resolution from pyproject + env |
| `crates/aeth-devkit-release/src/snapshot.rs` | Tracked-file snapshot only (no `dist/`) |
| `crates/aeth-devkit-release/src/undo.rs` | Journal without `DeleteDevpi`; `unwind` no longer needs a `Config` |
| `crates/aeth-devkit-release/src/steps.rs` | Eight steps; step 8 delegates to `ci` |
| `crates/aeth-devkit-release/src/ci.rs` | **New.** `find_run`, `wait_for_run`, `verify_published` |
| `crates/aeth-devkit-release/src/preflight.rs` | + workflow-committed check, `gh run list` probe; probe/removal aware of `PublishTarget` |
| `crates/aeth-devkit-release/src/report.rs` | `Existing.index` (was `devpi`), label passed in |
| `crates/aeth-devkit-release/src/lib.rs` | `Args.no_wait`, `Deps.index`, PyPI abort path |
| `crates/aeth-devkit/src/main.rs` | `release-and-pin` refuses `--no-wait` |
| `crates/aeth-devkit-setup/src/context.rs` | + `ProjectContext.publish_index: Option<String>` |
| `crates/aeth-devkit-setup/src/templates.rs` | + `{publish_index}`, `{publish_index_key}` placeholders; `gate_publish_index` block markers |
| `crates/aeth-devkit-setup/src/lib.rs` | + step 10b: render and install the release workflow; first-install `note:` |
| `python/aeth_devkit/templates/github/workflows/release.template.yml` | **New.** Pure-Python workflow |
| `python/aeth_devkit/templates/github/workflows/release.rust.template.yml` | **New.** maturin matrix workflow |
| `python/aeth_devkit/_tasks_source.py` | `poe release` help text |
| `README.md`, `TODO.md` | Feature reference, closed items |
| `.github/workflows/release.yml` | devkit's own copy, produced by running setup-project on this repo |

Task order is chosen so the workspace compiles after every task.

---

### Task 1: Remove the local build and publish steps from the release crate

The release still snapshots, bumps, locks, commits, tags, pushes and creates the release; it no longer builds, publishes, or journals a devpi deletion. `Config` keeps its current shape for this task (Task 2 changes it).

**Files:**

- Modify: `crates/aeth-devkit-release/src/snapshot.rs` (full rewrite below)
- Modify: `crates/aeth-devkit-release/src/undo.rs`
- Modify: `crates/aeth-devkit-release/src/steps.rs`
- Modify: `crates/aeth-devkit-release/src/lib.rs:242` (the `undo::unwind` call)
- Modify: `crates/aeth-devkit-release/src/preflight.rs` (docstring only; `remove_existing` still deletes devpi during pre-flight — that is deliberate and unchanged)

**Interfaces:**

- Produces: `snapshot::take(root) -> Result<Snapshot>`, `Snapshot::restore(&self, root)`, `Snapshot::manual_restore_command(&self) -> String`, `Snapshot::keep(self) -> PathBuf`, `Snapshot::present(&self, rel) -> bool`. `undo::unwind(journal, deps, root) -> Vec<Failure>` (no `cfg`). `Undo` has no `DeleteDevpi` variant. `steps::describe` prints seven numbered steps (step 8 is added in Task 4).

- [ ] **Step 1: Rewrite `snapshot.rs` without the `dist/` half**

Replace the whole file with:

```rust
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
```

- [ ] **Step 2: Drop `Undo::DeleteDevpi` and the `cfg` parameter from `undo.rs`**

In `crates/aeth-devkit-release/src/undo.rs`:

- Delete the `DeleteDevpi { url, index_name }` variant and its doc comment.
- `describe`: delete the `Undo::DeleteDevpi` arm; change the `RestoreFiles` text to `"Restoring pyproject.toml, uv.lock, Cargo.toml, Cargo.lock".into()`.
- `manual_command`: delete the `Undo::DeleteDevpi` arm.
- `apply`: change the signature to `fn apply(&self, deps: &Deps, root: &Path) -> Result<()>` and delete the `Undo::DeleteDevpi` arm.
- `unwind`: signature becomes `pub fn unwind(journal: Vec<Undo>, deps: &Deps, root: &Path) -> Vec<Failure>`; the call inside becomes `undo.apply(deps, root)`.
- Remove the now-unused `use crate::config::Config;` import. Update the module doc's first paragraph so it no longer says "one entry per completed forward step" with the old count — the wording "one entry per completed forward step" is still true; leave it.
- Tests: delete the `fn cfg()` helper; in `unwinds_in_reverse_and_keeps_going_after_failures` remove the `Undo::DeleteDevpi { .. }` journal entry, the `devpi_manual` assertions, and the `assert_eq!(*devpi.calls.borrow(), vec!["DELETE …"])` line; call `unwind(journal, &deps, root)`. In `reset_refuses_when_head_moved` call `unwind(vec![…], &deps, root)`.

- [ ] **Step 3: Cut steps 4 and 7 out of `steps.rs`**

In `crates/aeth-devkit-release/src/steps.rs`:

- Replace the module doc with:

```rust
//! The forward steps of a release. Each pushes its undo onto the journal on success.
//!
//! Ordering is the point of this module. Every purely local step (snapshot, bump, lock,
//! commit, tag) happens first and the remote-git/GitHub steps last, so the further a run
//! gets, the more it has already proven, and the expensive compensations are reserved for
//! the rare late failure. Building and publishing are not steps at all any more: the
//! GitHub release created in step 7 triggers the release workflow, which owns them.
```

- Replace `describe` with (step 8 is appended in Task 4):

```rust
/// The numbered plan printed by `--dry-run`.
pub fn describe(plan: &Plan) -> String {
  let tag = plan.tag();
  let mut s = String::from("Plan:\n");
  s += "  1. snapshot pyproject.toml, uv.lock, Cargo.toml, Cargo.lock\n";
  if plan.bumping() {
    s += &format!(
      "  2. uv version --bump {} ; update Cargo.toml ; cargo update --workspace\n",
      plan.bumps.join(" --bump ")
    );
    s += "  3. uv lock\n";
    s += &format!("  4. git commit \"Bump version to {}\"\n", plan.target.new);
  }
  s += &format!("  5. git tag -a {tag}\n");
  s += &format!(
    "  6. git push origin {}{tag}\n",
    if plan.bumping() {
      format!("{} ", plan.branch)
    } else {
      String::new()
    }
  );
  s += &format!(
    "  7. gh release create {tag} ({})\n",
    plan.notes.map_or("--generate-notes".to_string(), |n| format!("--notes {n:?}"))
  );
  s
}
```

- Delete `run_ok_env` and fold its body into `run_ok` (the env variant existed only for the publish credentials):

```rust
/// Run a tool with inherited stdio (the user sees its output) and require exit code 0.
fn run_ok(deps: &Deps, root: &Path, program: &str, args: &[&str]) -> Result<()> {
  let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
  match deps.runner.run_inherit(program, &owned, root)? {
    Some(0) => Ok(()),
    Some(code) => bail!("{program} {} exited with {code}", args.join(" ")),
    None => bail!("{program} was terminated by a signal"),
  }
}
```

- Delete `devpi_version_is_ours` and the whole `#[cfg(test)] mod tests` block (its only test was the ownership check).

- In `execute`: renumber the progress prefixes to `[1/7]` … `[7/7]` (Task 4 changes them to `/8`); delete the `[4/9] Building...` block (`snapshot::clear_dist` + `uv build`); delete the entire `[7/9] Publishing…` block from `println!("[7/9] …")` through `journal.push(Undo::DeleteDevpi { … });`; in the `[9/9]` block delete `let artifacts = snapshot::dist_artifacts(root)?;` and the `args.extend(artifacts…)` line so the `gh release create` args are `["release", "create", tag, "--title", tag, --notes… | --generate-notes]`; update its comment to `// No files: the release workflow attaches the artefacts it builds.` Replace `use crate::snapshot::{self, TRACKED};` with `use crate::snapshot::{self, TRACKED};` unchanged (still used by `snapshot::take` and `TRACKED`). The `use crate::config::Config;` import stays (`Plan.cfg`).

- In `crates/aeth-devkit-release/src/lib.rs`, change `let failures = undo::unwind(journal, deps, &root, &cfg);` to `let failures = undo::unwind(journal, deps, &root);`.

- [ ] **Step 4: Build and run the release crate tests**

Run: `cargo test -p aeth-devkit-release`
Expected: all pass (the removed test is gone; `snapshot` and `undo` tests updated).

Run: `cargo clippy -p aeth-devkit-release --all-targets -- -D warnings`
Expected: clean. If `snapshot::path` is reported unused, keep it (it is `pub`; no warning) — only delete things clippy actually names.

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-release
git commit -m "refactor(release): drop the local build and publish steps

The release workflow (installed by setup-project) now builds and publishes; the
command keeps snapshot, bump, lock, commit, tag, push and release creation. The
dist/ half of the snapshot, uv build, uv publish, the devpi ownership probe and
the DeleteDevpi undo entry go with them.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: `PublishTarget` — private index or PyPI

**Files:**

- Modify: `crates/aeth-devkit-core/src/pyproject.rs` (add `index_env_key`)
- Modify: `crates/aeth-devkit-release/src/config.rs` (full rewrite below)
- Modify: `crates/aeth-devkit-release/src/report.rs`
- Modify: `crates/aeth-devkit-release/src/preflight.rs` (`probe`, `remove_existing`, `check_config_committed`)
- Modify: `crates/aeth-devkit-release/src/lib.rs` (`Deps.index`, PyPI abort, labels)
- Modify: `crates/aeth-devkit-release/src/undo.rs` tests (`Deps` literal)

**Interfaces:**

- Produces:
  - `aeth_devkit_core::pyproject::index_env_key(name: &str) -> String` (`"SFTPyPI"` → `"SFTPYPI"`, `"my-index"` → `"MY_INDEX"`).
  - `config::PublishTarget::{ Index { name, url, publish_url, username, password }, Pypi }` with `fn label(&self) -> &str` (index name or `"PyPI"`) and `fn simple_url(&self) -> &str` (index `url` or `config::PYPI_SIMPLE`).
  - `config::Config { package: String, target: PublishTarget }` with `fn devpi_url(&self, version) -> Option<String>` (`None` for PyPI).
  - `config::PYPI_SIMPLE: &str = "https://pypi.org/simple"`.
  - `report::Existing { local_tag, remote_tag, github, index: bool }`; `report::render(version, ex, package, label)`.
  - `Deps` gains `pub index: &'a dyn aeth_devkit_core::index::IndexClient`.

- [ ] **Step 1: Add `index_env_key` to core and use it from `env_var_names`**

In `crates/aeth-devkit-core/src/pyproject.rs`, after `normalize_dist_name`:

```rust
/// The `<KEY>` in uv's `UV_INDEX_<KEY>_USERNAME` / `_PASSWORD`: the index name upper-cased
/// with `-` mapped to `_`. Shared by the release command (which reads those variables) and
/// the release workflow template (whose repository secrets are named the same way).
pub fn index_env_key(index_name: &str) -> String {
  index_name
    .chars()
    .map(|c| if c == '-' { '_' } else { c.to_ascii_uppercase() })
    .collect()
}
```

Add to that file's tests:

```rust
  #[test]
  fn index_env_key_follows_uv_convention() {
    assert_eq!(index_env_key("SFTPyPI"), "SFTPYPI");
    assert_eq!(index_env_key("my-index"), "MY_INDEX");
  }
```

- [ ] **Step 2: Rewrite `config.rs`**

Replace the whole file with:

```rust
//! Where a release publishes, its credentials, and the package name.
//!
//! Everything is derived from `pyproject.toml`: the `[[tool.uv.index]]` entry that has a
//! `publish-url` is the publish target, and its `name` decides which credential variables
//! to read (`UV_INDEX_<NAME>_USERNAME` / `_PASSWORD`). No publish index means PyPI, which
//! the workflow reaches through trusted publishing — no credentials at all. Nothing
//! project-specific remains in code.
//!
//! The credential variables are uv's for *reading* from an index, which a developer already
//! has set in order to install from it. The workflow reads the same names from repository
//! secrets and hands them to `uv publish` as `UV_PUBLISH_USERNAME` / `_PASSWORD`.

use anyhow::{Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::pyproject::{self, index_env_key, normalize_dist_name};

/// PyPI's simple index, used for the existing-version probe and the post-CI check when no
/// private index is configured.
pub const PYPI_SIMPLE: &str = "https://pypi.org/simple";

/// Where the workflow publishes, as resolved from `pyproject.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishTarget {
  /// The sole `[[tool.uv.index]]` with a `publish-url`. `url` is its simple index (what
  /// readers query), `publish_url` its devpi root (what pre-flight removal talks to).
  Index {
    name: String,
    url: String,
    publish_url: String,
    username: String,
    password: String,
  },
  /// No publish index: PyPI, via trusted publishing in CI.
  Pypi,
}

impl PublishTarget {
  /// Name for messages and the artefact table.
  pub fn label(&self) -> &str {
    match self {
      PublishTarget::Index { name, .. } => name,
      PublishTarget::Pypi => "PyPI",
    }
  }

  /// The simple index to read versions from.
  pub fn simple_url(&self) -> &str {
    match self {
      PublishTarget::Index { url, .. } => url,
      PublishTarget::Pypi => PYPI_SIMPLE,
    }
  }
}

/// Everything the release needs to know about the package and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
  pub package: String,
  pub target: PublishTarget,
}

/// `SFTPyPI` → (`UV_INDEX_SFTPYPI_USERNAME`, `UV_INDEX_SFTPYPI_PASSWORD`).
pub fn env_var_names(index_name: &str) -> (String, String) {
  let key = index_env_key(index_name);
  (format!("UV_INDEX_{key}_USERNAME"), format!("UV_INDEX_{key}_PASSWORD"))
}

/// The devpi REST URL for one release: `<publish-url>/<normalized package>/<version>`.
pub fn devpi_url(publish_url: &str, package: &str, version: &str) -> String {
  // `trim_end_matches('/')` avoids a double slash whether or not the configured URL ends
  // with one; the package name is PEP 503-normalized because that is how devpi keys it.
  format!("{}/{}/{version}", publish_url.trim_end_matches('/'), normalize_dist_name(package))
}

/// Build the [`Config`] from the parsed `pyproject.toml`, an optional `--index` override,
/// and an environment lookup.
///
/// `env` is a parameter (`&dyn Fn(&str) -> Option<String>`) rather than a direct
/// `std::env::var` call so tests can supply variables without touching the real process
/// environment, which is shared between test threads and therefore racy to mutate.
///
/// `--index` must name an index with a `publish-url`; without the flag, one publish index
/// selects it, none selects PyPI, and several is an error (the workflow publishes to one
/// place, and guessing which would be wrong half the time).
pub fn resolve(doc: &DocumentMut, index: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config> {
  let package = pyproject::project_name(doc)?;
  let all = pyproject::publish_indexes(doc)?;
  let chosen = match index {
    Some(want) => {
      // `publish_index` produces the precise "no such index" / "no publish-url" errors.
      let named = pyproject::publish_index(doc, Some(want))?;
      all.into_iter().find(|i| i.name == named.name)
    }
    None => match all.len() {
      0 => None,
      1 => all.into_iter().next(),
      _ => bail!(
        "several indexes have a publish-url ({}); pass --index",
        all.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ")
      ),
    },
  };
  let Some(idx) = chosen else {
    return Ok(Config {
      package,
      target: PublishTarget::Pypi,
    });
  };
  let (user_var, pass_var) = env_var_names(&idx.name);
  let (username, password) = (env(&user_var), env(&pass_var));
  // Names of variables that are unset *or empty*, so the error lists all of them at once.
  let missing: Vec<&str> = [(&user_var, &username), (&pass_var, &password)]
    .into_iter()
    .filter(|(_, v)| v.as_deref().is_none_or(str::is_empty))
    .map(|(k, _)| k.as_str())
    .collect();
  if !missing.is_empty() {
    bail!("required environment variables are not set:\n  - {}", missing.join("\n  - "));
  }
  Ok(Config {
    package,
    target: PublishTarget::Index {
      name: idx.name,
      url: idx.url,
      publish_url: idx.publish_url,
      // Safe: the `missing` check above proved both are `Some` and non-empty.
      username: username.unwrap(),
      password: password.unwrap(),
    },
  })
}

impl Config {
  /// The devpi URL of `version` on the private index; `None` for PyPI, which has no such
  /// endpoint (and whose releases cannot be deleted anyway).
  pub fn devpi_url(&self, version: &str) -> Option<String> {
    match &self.target {
      PublishTarget::Index { publish_url, .. } => Some(devpi_url(publish_url, &self.package, version)),
      PublishTarget::Pypi => None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const DOC: &str = "[project]\nname = \"Aeth_DevKit\"\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";

  fn env(k: &str) -> Option<String> {
    match k {
      "UV_INDEX_SFTPYPI_USERNAME" => Some("u".to_string()),
      "UV_INDEX_SFTPYPI_PASSWORD" => Some("p".to_string()),
      _ => None,
    }
  }

  #[test]
  fn env_names_follow_uv_convention() {
    assert_eq!(
      env_var_names("SFTPyPI"),
      ("UV_INDEX_SFTPYPI_USERNAME".into(), "UV_INDEX_SFTPYPI_PASSWORD".into())
    );
    assert_eq!(env_var_names("my-index").0, "UV_INDEX_MY_INDEX_USERNAME");
  }

  #[test]
  fn devpi_url_joins_and_normalizes() {
    assert_eq!(
      devpi_url("https://x/user/internal/", "Aeth_DevKit", "7.0.3"),
      "https://x/user/internal/aeth-devkit/7.0.3"
    );
  }

  #[test]
  fn resolves_an_index_target_from_doc_and_env() {
    let doc = DOC.parse().unwrap();
    let c = resolve(&doc, None, &env).unwrap();
    assert_eq!(c.package, "Aeth_DevKit");
    assert_eq!(
      c.target,
      PublishTarget::Index {
        name: "SFTPyPI".into(),
        url: "https://x/+simple".into(),
        publish_url: "https://x/user/internal/".into(),
        username: "u".into(),
        password: "p".into(),
      }
    );
    assert_eq!(c.target.label(), "SFTPyPI");
    assert_eq!(c.target.simple_url(), "https://x/+simple");
    assert_eq!(c.devpi_url("1.0").as_deref(), Some("https://x/user/internal/aeth-devkit/1.0"));
  }

  #[test]
  fn no_publish_index_means_pypi() {
    for doc in [
      "[project]\nname = \"demo\"\n",
      "[project]\nname = \"demo\"\n\n[[tool.uv.index]]\nname = \"Ro\"\nurl = \"https://x/+simple\"\n",
    ] {
      let c = resolve(&doc.parse().unwrap(), None, &|_| None).unwrap();
      assert_eq!(c.target, PublishTarget::Pypi, "{doc}");
      assert_eq!(c.target.label(), "PyPI");
      assert_eq!(c.target.simple_url(), PYPI_SIMPLE);
      assert_eq!(c.devpi_url("1.0"), None);
    }
  }

  #[test]
  fn explicit_index_must_have_a_publish_url() {
    let doc: DocumentMut = "[project]\nname = \"demo\"\n\n[[tool.uv.index]]\nname = \"Ro\"\nurl = \"https://x/+simple\"\n"
      .parse()
      .unwrap();
    let e = resolve(&doc, Some("Ro"), &|_| None).unwrap_err().to_string();
    assert!(e.contains("no publish-url"), "{e}");
    assert!(resolve(&doc, Some("Nope"), &|_| None).is_err());
  }

  #[test]
  fn several_publish_indexes_is_an_error() {
    let doc: DocumentMut = format!("{DOC}\n[[tool.uv.index]]\nname = \"B\"\nurl = \"https://b/+simple\"\npublish-url = \"https://b/\"\n")
      .parse()
      .unwrap();
    let e = resolve(&doc, None, &env).unwrap_err().to_string();
    assert!(e.contains("SFTPyPI, B") && e.contains("--index"), "{e}");
    assert_eq!(resolve(&doc, Some("B"), &|_| Some("x".into())).unwrap().target.label(), "B");
  }

  #[test]
  fn missing_env_lists_both_names() {
    let doc = DOC.parse().unwrap();
    let e = resolve(&doc, None, &|_| None).unwrap_err().to_string();
    assert!(
      e.contains("UV_INDEX_SFTPYPI_USERNAME") && e.contains("UV_INDEX_SFTPYPI_PASSWORD"),
      "{e}"
    );
  }
}
```

- [ ] **Step 3: Rename `Existing.devpi` to `index` and label the row in `report.rs`**

In `crates/aeth-devkit-release/src/report.rs`:

- Field: `pub index: bool,` with the doc line "while remote tag and index presence are simple yes/no (`bool`)".
- `any()`: `|| self.index`.
- `render(version: &str, ex: &Existing, package: &str, label: &str)`: the last row becomes

```rust
  s += &row(
    "index",
    if ex.index {
      format!("{package}=={version} on {label}")
    } else {
      "none".into()
    },
  );
```

- Test: rename `devpi: true` → `index: true`; assertions `s.contains("index           none")` and `s.contains("demo==1.2.3 on SFTPyPI")` (call `render("1.2.3", &all, "demo", "SFTPyPI")`).

- [ ] **Step 4: Make pre-flight target-aware**

In `crates/aeth-devkit-release/src/preflight.rs`:

Imports: add `use aeth_devkit_core::version::{contains, parse_lenient};` and `use crate::config::{Config, PublishTarget};` (replacing `use crate::config::Config;`).

Replace `check_config_committed`'s comparison block (from `let mut differ = Vec::new();` to the end of the function) with:

```rust
  // Credentials come from the environment, not the file, so only the file-borne values
  // can disagree between the two copies.
  let mut differ = Vec::new();
  if committed.package != cfg.package {
    differ.push("the project name");
  }
  match (&committed.target, &cfg.target) {
    (PublishTarget::Pypi, PublishTarget::Pypi) => {}
    (
      PublishTarget::Index {
        name: a,
        url: ua,
        publish_url: pa,
        ..
      },
      PublishTarget::Index {
        name: b,
        url: ub,
        publish_url: pb,
        ..
      },
    ) => {
      if a != b {
        differ.push("the publish index");
      }
      if ua != ub {
        differ.push("the index url");
      }
      if pa != pb {
        differ.push("the index publish-url");
      }
    }
    _ => differ.push("the publish target (private index vs PyPI)"),
  }
  if differ.is_empty() {
    Ok(())
  } else {
    bail!(
      "uncommitted pyproject.toml edits change {}; the release would build from the committed values but publish and roll back with the edited ones — commit or revert those edits first",
      differ.join(", ")
    )
  }
```

Replace the `devpi:` line in `probe` so the whole `Ok(Existing { … })` reads:

```rust
  // A private index answers its devpi REST endpoint (authenticated, exact). PyPI has no
  // such endpoint, so its simple index is read instead — the same page the post-CI check
  // and `docker-pin` read.
  let index = match &cfg.target {
    PublishTarget::Index { username, password, .. } => {
      let url = cfg.devpi_url(version).expect("an index target has a devpi url");
      deps.devpi.exists(&url, username, password)?
    }
    PublishTarget::Pypi => {
      let want = parse_lenient(version).with_context(|| format!("{version} is not a PEP 440 version"))?;
      let versions = deps.index.versions(cfg.target.simple_url(), &cfg.package)?;
      contains(versions.iter().map(String::as_str), &want)
    }
  };
  Ok(Existing {
    local_tag: git::tag_target(root, &tag)?,
    remote_tag: git::remote_tag_exists(deps.runner, root, &tag)?,
    github,
    index,
  })
```

In `remove_existing`, replace the `if ex.devpi { … }` block with:

```rust
  if ex.index {
    // Only a private index can drop a version; `run` aborts before reaching here for
    // PyPI, so a PyPI target with `index: true` is a caller bug, not a user error.
    let PublishTarget::Index { username, password, .. } = &cfg.target else {
      bail!("{}=={version} exists on PyPI and cannot be removed", cfg.package);
    };
    println!("  -> {verb} {}=={version} from {}", cfg.package, cfg.target.label());
    if !dry_run {
      let url = cfg.devpi_url(version).expect("an index target has a devpi url");
      // Both outcomes are fine: the goal is "not there afterwards". Matching exhaustively
      // (rather than `let _ =`) means a future third variant would be a compile error here.
      match deps.devpi.delete(&url, username, password)? {
        DeleteOutcome::Deleted | DeleteOutcome::NotFound => {}
      }
    }
  }
```

- [ ] **Step 5: Wire `Deps.index` and the PyPI abort into `lib.rs`**

In `crates/aeth-devkit-release/src/lib.rs`:

- Imports: add `use aeth_devkit_core::index::IndexClient;` and `use crate::config::PublishTarget;`.
- `Deps`: add after `devpi`:

```rust
  /// Reads simple-index pages: the PyPI existence probe, and the post-CI completeness check
  /// on whichever target the workflow published to.
  pub index: &'a dyn IndexClient,
```

- In `run_outcome`, replace the `if existing.any() { … }` block with:

```rust
  if existing.any() {
    print!("{}", report::render(&target.new, &existing, &cfg.package, cfg.target.label()));
    // PyPI files are immutable: nothing here can make room for the version, so the only
    // way forward is a different version number.
    if existing.index && cfg.target == PublishTarget::Pypi {
      eprintln!(
        "aborted: {}=={} is already on PyPI and PyPI releases cannot be removed; bump to a new version",
        cfg.package, target.new
      );
      return Ok(Outcome::Aborted);
    }
    if !args.dry_run && !confirm_force(deps.prompt, force, "Remove these and continue? Type 'force' to continue:")? {
      eprintln!("aborted: artefacts for v{} already exist", target.new);
      return Ok(Outcome::Aborted);
    }
    // The probe above may have taken a while; do not start deleting after a Ctrl-C.
    deps.check_interrupt()?;
    preflight::remove_existing(deps, &root, &cfg, &target.new, &existing, args.dry_run)?;
  } else {
    println!("No existing artefacts for v{}.", target.new);
  }
```

- In `run_outcome_real`, build the index client and pass it:

```rust
  let index = aeth_devkit_core::index::HttpIndexClient::with_timeout(std::time::Duration::from_secs(30));
  run_outcome(
    args,
    &Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      devpi: &aeth_devkit_core::devpi::HttpDevpiClient,
      index: &index,
      prompt: &prompt::StdinPrompt,
      env: &env,
      interrupted: &INTERRUPTED,
    },
  )
```

- Update the `Args` doc comments:

```rust
/// Bump version, commit, tag, push, create the GitHub release, and wait for the release
/// workflow to build and publish.
// … on `index`:
  /// `[[tool.uv.index]]` to publish to (default: the one with a publish-url, or PyPI when there is none).
```

- `undo.rs` tests: every `Deps { … }` literal gains `index: &aeth_devkit_core::index::StubIndexClient { versions: vec![] },` — bind it first (`let index = StubIndexClient { versions: vec![] };` then `index: &index,`) since the struct holds a borrow.

- [ ] **Step 6: Add a PyPI-mode probe test in `preflight.rs`**

Append to `preflight.rs`'s `mod tests`:

```rust
  #[test]
  fn probe_reads_pypi_when_there_is_no_publish_index() {
    use std::sync::atomic::AtomicBool;

    use aeth_devkit_core::devpi::StubDevpiClient;
    use aeth_devkit_core::index::StubIndexClient;

    use crate::prompt::ScriptedPrompt;

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    std::fs::write(root.join("a"), "1").unwrap();
    git::commit_paths(root, &["a".into()], "init").unwrap();
    let runner = RecordingRunner::new(0);
    runner.script_err("gh", &["release", "view"], 1, "release not found");
    runner.script("git", &["ls-remote"], 0, "");
    let devpi = StubDevpiClient::new(true); // must not be consulted
    let index = StubIndexClient {
      versions: vec!["1.0.0".into(), "2.0.0".into()],
    };
    let prompt = ScriptedPrompt::new(&[]);
    let flag = AtomicBool::new(false);
    let deps = Deps {
      runner: &runner,
      devpi: &devpi,
      index: &index,
      prompt: &prompt,
      env: &|_| None,
      interrupted: &flag,
    };
    let cfg = Config {
      package: "demo".into(),
      target: PublishTarget::Pypi,
    };
    assert!(probe(&deps, root, &cfg, "2.0.0").unwrap().index);
    assert!(!probe(&deps, root, &cfg, "3.0.0").unwrap().index);
    assert!(devpi.calls.borrow().is_empty(), "PyPI mode must not touch devpi");
  }
```

If `git::remote_tag_exists` needs a different scripted `git` call than `ls-remote`, read `crates/aeth-devkit-core/src/git.rs` for its exact argv and script that prefix instead.

- [ ] **Step 7: Run the tests**

Run: `cargo test -p aeth-devkit-core pyproject && cargo test -p aeth-devkit-release`
Expected: PASS.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean. (`aeth-devkit-pin` and `aeth-devkit-setup` do not touch `Config`; the binary crate compiles unchanged.)

- [ ] **Step 8: Commit**

```bash
git add crates/aeth-devkit-core/src/pyproject.rs crates/aeth-devkit-release
git commit -m "feat(release): publish to PyPI when no index has a publish-url

config::resolve now returns a PublishTarget: the sole publish index (with its
credentials) or Pypi. In PyPI mode the existing-version probe reads
https://pypi.org/simple and an existing version aborts the release, since PyPI
files cannot be removed.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Wait for the release workflow (step 8) and the new pre-flight checks

**Files:**

- Create: `crates/aeth-devkit-release/src/ci.rs`
- Modify: `crates/aeth-devkit-release/src/steps.rs` (`Plan.no_wait`, step 8, `describe`)
- Modify: `crates/aeth-devkit-release/src/preflight.rs` (`check_tools`, `check_workflow_committed`)
- Modify: `crates/aeth-devkit-release/src/lib.rs` (`Args.no_wait`, plan wiring, pre-flight call)
- Modify: `crates/aeth-devkit/src/main.rs` (`release-and-pin` refuses `--no-wait`)

**Interfaces:**

- Consumes: `Deps` (Task 2, with `index`), `Config`/`PublishTarget` (Task 2), `preflight::GH_NOT_FOUND`.
- Produces:
  - `ci::WORKFLOW_FILE: &str = ".github/workflows/release.yml"`
  - `ci::RUN_START_TIMEOUT: Duration` (120 s), `ci::POLL_INTERVAL: Duration` (5 s)
  - `ci::find_run(runner, root, tag) -> Result<Option<String>>`
  - `ci::wait_for_run(deps, root, tag, sleep: &mut dyn FnMut(Duration)) -> Result<String>` (returns the run URL)
  - `ci::verify_published(deps, root, cfg, version) -> Result<()>`
  - `ci::actions_url(root) -> Option<String>`
  - `preflight::check_workflow_committed(root) -> Result<()>`
  - `steps::Plan` gains `pub no_wait: bool`; `Args` gains `pub no_wait: bool`.

- [ ] **Step 1: Write the failing tests for `ci.rs`**

Create `crates/aeth-devkit-release/src/ci.rs` with only the test module for now (the implementation follows in Step 3):

```rust
//! Step 8: waiting for the release workflow the GitHub release triggered, and proving that
//! what it published is installable.

#[cfg(test)]
mod tests {
  use super::*;
  use std::cell::Cell;
  use std::sync::atomic::AtomicBool;

  use aeth_devkit_core::devpi::StubDevpiClient;
  use aeth_devkit_core::index::StubIndexClient;
  use aeth_devkit_core::process::{CapturedOutput, RecordingRunner, Runner};

  use crate::config::{Config, PublishTarget};
  use crate::prompt::ScriptedPrompt;

  /// Answers `gh run list` empty for the first `empty_polls` calls, then like `inner`.
  struct LateRun {
    inner: RecordingRunner,
    empty_polls: usize,
    polls: Cell<usize>,
  }

  impl Runner for LateRun {
    fn run_inherit_env(&self, program: &str, args: &[String], cwd: &Path, env: &[(&str, &str)]) -> Result<Option<i32>> {
      self.inner.run_inherit_env(program, args, cwd, env)
    }
    fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
      if program == "gh" && args.starts_with(&["run".to_string(), "list".to_string()]) {
        let n = self.polls.get();
        self.polls.set(n + 1);
        if n < self.empty_polls {
          self.inner.run_capture(program, args, cwd)?; // recorded, but answered empty
          return Ok(CapturedOutput {
            code: Some(0),
            ..Default::default()
          });
        }
      }
      self.inner.run_capture(program, args, cwd)
    }
  }

  fn scripted() -> RecordingRunner {
    let r = RecordingRunner::new(0);
    r.script("gh", &["run", "list"], 0, "123456\n");
    r.script("gh", &["run", "view"], 0, "https://github.com/o/r/actions/runs/123456\n");
    r
  }

  fn deps<'a>(runner: &'a dyn Runner, index: &'a StubIndexClient, flag: &'a AtomicBool) -> Deps<'a> {
    // Leaked stubs keep the test short; these tests own tiny amounts of memory.
    let devpi: &'static StubDevpiClient = Box::leak(Box::new(StubDevpiClient::new(false)));
    let prompt: &'static ScriptedPrompt = Box::leak(Box::new(ScriptedPrompt::new(&[])));
    Deps {
      runner,
      devpi,
      index,
      prompt,
      env: &|_| None,
      interrupted: flag,
    }
  }

  #[test]
  fn find_run_selects_the_run_for_the_tag() {
    let r = scripted();
    assert_eq!(find_run(&r, Path::new("."), "v1.2.3").unwrap().as_deref(), Some("123456"));
    let call = &r.calls_for("gh")[0];
    assert_eq!(call[..6].to_vec(), vec!["run", "list", "--workflow", "release.yml", "--event", "release"]);
    assert!(call.iter().any(|a| a.contains(r#"select(.headBranch == "v1.2.3")"#)), "{call:?}");
    let empty = RecordingRunner::new(0);
    assert_eq!(find_run(&empty, Path::new("."), "v1.2.3").unwrap(), None);
    let broken = RecordingRunner::new(1);
    assert!(find_run(&broken, Path::new("."), "v1.2.3").is_err());
  }

  #[test]
  fn waits_for_the_run_to_appear_then_watches_it() {
    let late = LateRun {
      inner: scripted(),
      empty_polls: 2,
      polls: Cell::new(0),
    };
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&late, &index, &flag);
    let mut slept = Vec::new();
    let url = wait_for_run(&d, Path::new("."), "v1.2.3", &mut |dur| slept.push(dur)).unwrap();
    assert_eq!(url, "https://github.com/o/r/actions/runs/123456");
    assert_eq!(slept, vec![POLL_INTERVAL, POLL_INTERVAL]);
    let gh = late.inner.calls_for("gh");
    assert!(gh.iter().any(|c| c == &["run", "watch", "123456", "--exit-status"]), "{gh:?}");
  }

  #[test]
  fn gives_up_when_no_run_starts_in_time() {
    let r = RecordingRunner::new(0); // `gh run list` always empty
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag);
    let mut total = Duration::ZERO;
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &mut |dur| total += dur).unwrap_err();
    assert!(err.to_string().contains("no release workflow run for v1.2.3"), "{err}");
    assert!(total >= RUN_START_TIMEOUT, "{total:?}");
    assert!(r.calls_for("gh").iter().all(|c| c[1] != "watch"));
  }

  #[test]
  fn a_failed_run_is_an_error() {
    let r = scripted();
    r.script("gh", &["run", "watch"], 1, "");
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("run 123456 failed"), "{err}");
  }

  #[test]
  fn an_interrupt_between_polls_stops_waiting() {
    let r = RecordingRunner::new(0);
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(true);
    let d = deps(&r, &index, &flag);
    assert!(wait_for_run(&d, Path::new("."), "v1", &mut |_| {}).is_err());
    assert!(r.calls_for("gh").is_empty());
  }

  #[test]
  fn verify_requires_the_version_on_the_target_index_and_the_release() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["release", "view"], 0, "url\n");
    let flag = AtomicBool::new(false);
    let cfg = Config {
      package: "demo".into(),
      target: PublishTarget::Pypi,
    };
    let present = StubIndexClient {
      versions: vec!["1.0.0".into()],
    };
    assert!(verify_published(&deps(&r, &present, &flag), Path::new("."), &cfg, "1.0.0").is_ok());
    let absent = StubIndexClient { versions: vec![] };
    let err = verify_published(&deps(&r, &absent, &flag), Path::new("."), &cfg, "1.0.0").unwrap_err();
    assert!(err.to_string().contains("demo==1.0.0 is not on PyPI"), "{err}");
    let no_release = RecordingRunner::new(0);
    no_release.script_err("gh", &["release", "view"], 1, "release not found");
    let err = verify_published(&deps(&no_release, &present, &flag), Path::new("."), &cfg, "1.0.0").unwrap_err();
    assert!(err.to_string().contains("no GitHub release"), "{err}");
  }
}
```

Register the module in `lib.rs`: add `pub mod ci;` (alphabetically, after `pub mod args;`) and add `- [`ci`]        — waiting for the release workflow and verifying its output;` to the module list in the crate doc.

- [ ] **Step 2: Run the tests to see them fail to compile**

Run: `cargo test -p aeth-devkit-release ci::`
Expected: compile errors — `find_run`, `wait_for_run`, `verify_published`, `POLL_INTERVAL`, `RUN_START_TIMEOUT` not found.

- [ ] **Step 3: Implement `ci.rs`**

Insert above the test module:

```rust
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::version::{contains, parse_lenient};
use aeth_devkit_core::{git, github};

use crate::Deps;
use crate::config::Config;

/// The workflow setup-project installs; the release requires it at `HEAD`.
pub const WORKFLOW_FILE: &str = ".github/workflows/release.yml";
/// How long the release event may take to produce a run before we give up. GitHub usually
/// starts one within seconds; two minutes covers a queue backlog without hiding a workflow
/// that is never going to start (file missing on the default branch, Actions disabled).
pub const RUN_START_TIMEOUT: Duration = Duration::from_secs(120);
/// Spacing between `gh run list` polls.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// `&["a", "b"]` → `vec!["a".to_string(), "b".to_string()]`, for the `Runner` API.
fn s(args: &[&str]) -> Vec<String> {
  args.iter().map(|a| a.to_string()).collect()
}

/// The id of the release workflow run triggered by `tag`, once it exists. For a `release`
/// event `gh` reports the tag as the run's head branch, which is what selects it.
pub fn find_run(runner: &dyn Runner, root: &Path, tag: &str) -> Result<Option<String>> {
  let jq = format!(".[] | select(.headBranch == \"{tag}\") | .databaseId");
  let args = s(&[
    "run",
    "list",
    "--workflow",
    "release.yml",
    "--event",
    "release",
    "--json",
    "databaseId,headBranch",
    "--jq",
    &jq,
  ]);
  let out = runner.run_capture("gh", &args, root)?;
  if !out.success() {
    bail!("gh run list failed: {}", out.stderr.trim());
  }
  Ok(out.stdout.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string))
}

/// Poll until the run exists (bounded by [`RUN_START_TIMEOUT`]), then `gh run watch` it to
/// completion with inherited stdio so the user sees the job progress. Returns the run URL.
///
/// `sleep` is injected so tests can count the waits instead of serving them.
pub fn wait_for_run(deps: &Deps, root: &Path, tag: &str, sleep: &mut dyn FnMut(Duration)) -> Result<String> {
  let mut waited = Duration::ZERO;
  let id = loop {
    deps.check_interrupt()?;
    if let Some(id) = find_run(deps.runner, root, tag)? {
      break id;
    }
    if waited >= RUN_START_TIMEOUT {
      bail!(
        "no release workflow run for {tag} started within {}s; is {WORKFLOW_FILE} on the default branch and Actions enabled?",
        RUN_START_TIMEOUT.as_secs()
      );
    }
    sleep(POLL_INTERVAL);
    waited += POLL_INTERVAL;
  };
  println!("  run {id} started; watching...");
  match deps.runner.run_inherit("gh", &s(&["run", "watch", &id, "--exit-status"]), root)? {
    Some(0) => {}
    Some(code) => bail!("release workflow run {id} failed (gh run watch exited with {code})"),
    None => bail!("gh run watch was terminated by a signal"),
  }
  let out = deps.runner.run_capture("gh", &s(&["run", "view", &id, "--json", "url", "--jq", ".url"]), root)?;
  Ok(out.stdout.trim().to_string())
}

/// A green run is not the same as an installable release: the version must be listed on
/// the publish target and the GitHub release must still exist. This is the rule
/// `docker-pin` applies before pinning, checked here so `Released` means what it says.
pub fn verify_published(deps: &Deps, root: &Path, cfg: &Config, version: &str) -> Result<()> {
  let want = parse_lenient(version).with_context(|| format!("{version} is not a PEP 440 version"))?;
  let versions = deps
    .index
    .versions(cfg.target.simple_url(), &cfg.package)
    .with_context(|| format!("querying {}", cfg.target.label()))?;
  if !contains(versions.iter().map(String::as_str), &want) {
    bail!(
      "the release workflow succeeded but {}=={version} is not on {}; inspect the publish job's log",
      cfg.package,
      cfg.target.label()
    );
  }
  if !github::release_exists(deps.runner, root, &format!("v{version}"))? {
    bail!("v{version} has no GitHub release any more; it was deleted while the workflow ran");
  }
  Ok(())
}

/// The Actions page for the release workflow, when `origin` is on GitHub. Printed by
/// `--no-wait` in place of the run URL, which may not exist yet.
pub fn actions_url(root: &Path) -> Option<String> {
  let origin = git::origin_url(root).ok().flatten()?;
  let repo = github::github_repo_path(&origin)?;
  Some(format!("https://github.com/{repo}/actions/workflows/release.yml"))
}
```

Note the `use std::path::Path;` and `use anyhow::Result;` are also what the test module's `LateRun` needs through `use super::*;`.

- [ ] **Step 4: Run the `ci` tests**

Run: `cargo test -p aeth-devkit-release ci::`
Expected: 6 tests pass. If `github::release_exists` is scripted with a different prefix than `["release", "view"]`, keep the test as written — that is the argv it uses.

- [ ] **Step 5: Add the two pre-flight checks**

In `crates/aeth-devkit-release/src/preflight.rs`:

Replace `check_tools` with:

```rust
/// `git`, `uv`, and `gh` must all answer `--version`, and `gh` must be able to list
/// workflow runs — the token needs the `workflow` scope for step 8, and finding that out
/// after the release exists would mean a rollback for a login problem.
pub fn check_tools(runner: &dyn Runner, root: &Path) -> Result<()> {
  // `filter` keeps the tools that *fail*; the closure maps a run error or a non-zero exit
  // to `false` via `map(…).unwrap_or(false)`, so any kind of trouble counts as missing.
  let missing: Vec<&str> = ["git", "uv", "gh"]
    .into_iter()
    .filter(|tool| {
      !runner
        .run_capture(tool, &s(&["--version"]), root)
        .map(|o| o.success())
        .unwrap_or(false)
    })
    .collect();
  if !missing.is_empty() {
    bail!("required tools not found on PATH: {}", missing.join(", "))
  }
  let runs = runner.run_capture("gh", &s(&["run", "list", "--limit", "1"]), root)?;
  if !runs.success() {
    bail!(
      "gh cannot read workflow runs ({}); run `gh auth login` (or `gh auth refresh -s workflow`) first",
      runs.stderr.trim()
    );
  }
  Ok(())
}
```

Add after `check_tools`:

```rust
/// The release workflow must be committed: the GitHub release created in step 7 is only a
/// trigger, and without the workflow nothing would build or publish. Checked at `HEAD`
/// rather than on disk because the tag points at `HEAD`'s tree.
pub fn check_workflow_committed(root: &Path) -> Result<()> {
  if git::head_blob(root, crate::ci::WORKFLOW_FILE)?.is_some() {
    Ok(())
  } else {
    bail!("{} is not committed; run `devkit setup-project` and commit the workflow first", crate::ci::WORKFLOW_FILE)
  }
}
```

Add tests to `preflight.rs`'s `mod tests`:

```rust
  #[test]
  fn tools_check_requires_gh_to_list_runs() {
    let r = RecordingRunner::new(0);
    r.script_err("gh", &["run", "list"], 1, "HTTP 403: Resource not accessible");
    let err = check_tools(&r, Path::new(".")).unwrap_err().to_string();
    assert!(err.contains("cannot read workflow runs") && err.contains("403"), "{err}");
    let ok = RecordingRunner::new(0);
    assert!(check_tools(&ok, Path::new(".")).is_ok());
    assert_eq!(ok.calls_for("gh").last().unwrap(), &["run", "list", "--limit", "1"]);
  }

  #[test]
  fn workflow_must_be_committed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    std::fs::write(root.join("a"), "1").unwrap();
    git::commit_paths(root, &["a".into()], "init").unwrap();
    let err = check_workflow_committed(root).unwrap_err().to_string();
    assert!(err.contains("release.yml is not committed"), "{err}");
    std::fs::create_dir_all(root.join(".github/workflows")).unwrap();
    std::fs::write(root.join(".github/workflows/release.yml"), "name: Release\n").unwrap();
    // On disk only: still refused.
    assert!(check_workflow_committed(root).is_err());
    git::commit_paths(root, &[".github/workflows/release.yml".into()], "wf").unwrap();
    assert!(check_workflow_committed(root).is_ok());
  }
```

- [ ] **Step 6: Wire step 8, `--no-wait`, and the pre-flight into `steps.rs` / `lib.rs` / `main.rs`**

`steps.rs`:

- `Plan` gains `pub no_wait: bool,` after `branch`.
- `describe`: append before `s` is returned:

```rust
  if plan.no_wait {
    s += "  8. (skipped: --no-wait) wait for the release workflow\n";
  } else {
    s += &format!(
      "  8. wait for the release workflow, then verify {}=={} on {}\n",
      plan.cfg.package, plan.target.new, plan.cfg.target.label()
    );
  }
```

- In `execute`: change every `[n/7]` prefix to `[n/8]`, and append after `journal.push(Undo::DeleteGithubRelease(tag.clone()));` (change the earlier `Undo::DeleteGithubRelease(tag)` to clone since `tag` is still needed) and before `Ok(out.stdout…)`:

```rust
  let release_url = out.stdout.trim().to_string();

  check_interrupt(deps)?;
  if plan.no_wait {
    println!(
      "[8/8] Not waiting for the release workflow (--no-wait): {}",
      crate::ci::actions_url(root).unwrap_or_else(|| "see the repository's Actions tab".into())
    );
    return Ok(release_url);
  }
  // A failed or missing run is a failed release: the journal is unwound like any other
  // late failure (release deleted, tag and branch rewound under their leases). A tag with
  // no artefacts is exactly the state the completeness check exists to reject, so it is
  // better rolled back than left for a later `devkit release` to trip over.
  println!("[8/8] Waiting for the release workflow...");
  let run_url = crate::ci::wait_for_run(deps, root, &tag, &mut std::thread::sleep)?;
  crate::ci::verify_published(deps, root, plan.cfg, new)?;
  println!("  workflow succeeded: {run_url}");
  Ok(release_url)
```

`lib.rs`:

- `Args` gains, after `index`:

```rust
  /// Create the GitHub release and return without waiting for the release workflow.
  #[arg(long)]
  pub no_wait: bool,
```

- In `run_outcome`, right after `preflight::check_tools(deps.runner, &root)?;` add `preflight::check_workflow_committed(&root)?;`.
- The `steps::Plan { … }` literal gains `no_wait: args.no_wait,`.
- The `Released` print stays as it is; the doc on `Outcome::Released` becomes:

```rust
  /// The release completed and (unless `--no-wait`) the workflow published it; `version` is
  /// the released version (PEP 440, no `v`).
```

- `cli_tests::flags_parse_anywhere_on_the_line`: add `let a = Args::try_parse_from(["devkit-release", "patch", "--no-wait"]).unwrap(); assert!(a.no_wait);`.

`crates/aeth-devkit/src/main.rs`, in `release_and_pin` before `match`:

```rust
  // The pin's completeness preflight needs the artefacts the workflow publishes.
  if args.no_wait {
    anyhow::bail!("release-and-pin waits for the release workflow so it can pin the result; drop --no-wait");
  }
```

- [ ] **Step 7: Run the release crate tests and clippy**

Run: `cargo test -p aeth-devkit-release && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, clean.

- [ ] **Step 8: Dry-run the real command against this repo to eyeball the plan text**

Run: `cargo run -p aeth-devkit-release -- --dry-run`
Expected: exit 2 with `.github/workflows/release.yml is not committed; run devkit setup-project …` (the workflow lands in Task 6). That confirms the new pre-flight is first in line; the plan text itself is covered by reading `describe`.

- [ ] **Step 9: Commit**

```bash
git add crates/aeth-devkit-release crates/aeth-devkit/src/main.rs
git commit -m "feat(release): wait for the release workflow and verify what it published

Step 8 polls gh run list for the run the new GitHub release triggered, watches
it with gh run watch --exit-status, then requires the version on the publish
target and the release to still exist. A failed or missing run unwinds the
journal. --no-wait skips the step; release-and-pin refuses it. Pre-flight now
requires the workflow file at HEAD and gh with access to workflow runs.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: setup-project context and placeholders for the publish index

**Files:**

- Modify: `crates/aeth-devkit-setup/src/context.rs`
- Modify: `crates/aeth-devkit-setup/src/templates.rs`
- Modify: test `ProjectContext { … }` literals in `crates/aeth-devkit-setup/src/{md_block.rs,toml_merge.rs,templates.rs}`

**Interfaces:**

- Produces: `ProjectContext.publish_index: Option<String>` (name of the sole `[[tool.uv.index]]` with a `publish-url`; `None` when there is none; `discover` errors when there are several). `templates::substitute` replaces `{publish_index}` (the name, or `""`) and `{publish_index_key}` (`pyproject::index_env_key(name)`, or `""`). `templates::gate_publish_index(text: &str, has_publish_index: bool) -> String` keeps the lines inside `# setup-project: if-publish-index` … `# setup-project: end` blocks only when `has_publish_index`, the lines inside `# setup-project: if-no-publish-index` … `# setup-project: end` only when not, and never emits a marker line.

- [ ] **Step 1: Write the failing tests**

In `crates/aeth-devkit-setup/src/context.rs`, add a test module:

```rust
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
    assert_eq!(ProjectContext::discover(dir.path()).unwrap().publish_index.as_deref(), Some("SFTPyPI"));
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
```

In `crates/aeth-devkit-setup/src/templates.rs`, add a test module:

```rust
#[cfg(test)]
mod publish_index_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(publish_index: Option<&str>) -> ProjectContext {
    ProjectContext {
      root: std::path::PathBuf::from("/p"),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: false,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: publish_index.map(str::to_string),
    }
  }

  #[test]
  fn publish_index_placeholders() {
    let out = substitute("{publish_index} {publish_index_key}", &ctx(Some("my-index")), Escape::None);
    assert_eq!(out, "my-index MY_INDEX");
    assert_eq!(substitute("[{publish_index}]", &ctx(None), Escape::None), "[]");
  }

  const GATED: &str = "a\n# setup-project: if-publish-index\nidx1\n  # setup-project: end\nb\n  # setup-project: if-no-publish-index\n  pypi\n  # setup-project: end\nc\n";

  #[test]
  fn gate_keeps_exactly_one_variant_and_no_markers() {
    assert_eq!(gate_publish_index(GATED, true), "a\nidx1\nb\nc\n");
    assert_eq!(gate_publish_index(GATED, false), "a\nb\n  pypi\nc\n");
  }

  #[test]
  fn gate_leaves_unmarked_text_alone() {
    assert_eq!(gate_publish_index("x\n  y\n", true), "x\n  y\n");
  }
}
```

- [ ] **Step 2: Run them to confirm they fail**

Run: `cargo test -p aeth-devkit-setup publish_index`
Expected: compile errors (`publish_index` field, `gate_publish_index`).

- [ ] **Step 3: Implement**

`context.rs`:

- Field, after `has_rust`:

```rust
  /// Name of the sole `[[tool.uv.index]]` with a `publish-url`, which the release workflow
  /// publishes to; `None` means PyPI via trusted publishing.
  pub publish_index: Option<String>,
```

- In `discover`, after `let has_rust = …;`:

```rust
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
```

and add `publish_index,` to the `Ok(Self { … })` literal.

`templates.rs`:

- `substitute`: add after the `{devkit_bin}` replacement:

```rust
    .replace("{publish_index}", &esc(ctx.publish_index.as_deref().unwrap_or("")))
    .replace(
      "{publish_index_key}",
      &esc(&ctx.publish_index.as_deref().map(aeth_devkit_core::pyproject::index_env_key).unwrap_or_default()),
    )
```

and extend the `load` doc comment to list the placeholders: `` `{project_root}` / `{package}` / `{python_dir}` / `{devkit_bin}` / `{publish_index}` / `{publish_index_key}` ``.

- Add:

```rust
/// Block markers for line-based templates (YAML): `# setup-project: if-publish-index` …
/// `# setup-project: end` survives only when the project has a publish index,
/// `# setup-project: if-no-publish-index` … `# setup-project: end` only when it has none.
/// Marker lines are dropped either way. Markers may be indented; the lines inside keep
/// their own indentation, so a block can sit anywhere in the document.
pub fn gate_publish_index(text: &str, has_publish_index: bool) -> String {
  let mut out = String::with_capacity(text.len());
  // `Some(keep)` while inside a block, saying whether its lines are emitted.
  let mut block: Option<bool> = None;
  for line in text.lines() {
    match line.trim().strip_prefix("# setup-project: ") {
      Some("if-publish-index") => block = Some(has_publish_index),
      Some("if-no-publish-index") => block = Some(!has_publish_index),
      Some("end") => block = None,
      _ => {
        if block.unwrap_or(true) {
          out.push_str(line);
          out.push('\n');
        }
      }
    }
  }
  out
}
```

- Every existing test `ProjectContext { … }` literal in `templates.rs` (`devkit_bin_tests::ctx`), `md_block.rs` (`ctx`), and `toml_merge.rs` (both `ctx` helpers) gains `publish_index: None,`. Find them with `grep -n "has_rust:" crates/aeth-devkit-setup/src/*.rs`.

- [ ] **Step 4: Run the setup crate tests**

Run: `cargo test -p aeth-devkit-setup`
Expected: PASS (including the existing e2e tests — the fixture pyproject has one publish index, so `discover` succeeds).

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-setup
git commit -m "feat(setup): publish-index context, placeholders and YAML block markers

ProjectContext names the sole publish index (several is an error), templates
gain {publish_index} / {publish_index_key}, and gate_publish_index renders one
of two marked blocks so a workflow template carries both publish variants.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: The two workflow templates and their installation

**Files:**

- Create: `python/aeth_devkit/templates/github/workflows/release.template.yml`
- Create: `python/aeth_devkit/templates/github/workflows/release.rust.template.yml`
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (new step after step 10)
- Modify: `crates/aeth-devkit-setup/tests/apply.rs`

**Interfaces:**

- Consumes: `templates::load`, `templates::gate_publish_index`, `ctx.publish_index`, `ctx.has_rust`, `changes.record_optional`, `changes.notes`, `crate::git::is_git_tracked`, `aeth_devkit_core::git::origin_url`, `aeth_devkit_core::github::github_repo_path`, `aeth_devkit_core::pyproject::index_env_key`.
- Produces: `.github/workflows/release.yml` in every project setup-project touches, rendered from `github/workflows/release.rust.yml` (template name) when `ctx.has_rust`, else `github/workflows/release.yml`.

- [ ] **Step 1: Write the pure-Python template**

Create `python/aeth_devkit/templates/github/workflows/release.template.yml`:

```yaml
# Installed and kept current by `devkit setup-project`; edits are replaced on the next run.
# Triggered by `devkit release`, which creates the GitHub release with no assets and then
# waits for this workflow to build, attach and publish them.
name: Release

on:
  release:
    types: [published]

concurrency:
  group: release-${{ github.event.release.tag_name }}
  cancel-in-progress: false

permissions:
  contents: write

env:
  TAG: ${{ github.event.release.tag_name }}

jobs:
  publish:
    runs-on: ubuntu-latest
    # setup-project: if-no-publish-index
    permissions:
      contents: write
      id-token: write
    # setup-project: end
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.event.release.tag_name }}

      - uses: astral-sh/setup-uv@v5

      - name: The tag must name the committed version
        shell: bash
        run: |
          v="$(uv version --short)"
          [ "v$v" = "$TAG" ] || { echo "tag $TAG does not match pyproject.toml version $v" >&2; exit 1; }

      - name: Build
        run: uv build --out-dir dist

      - name: Attach artefacts to the release
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release upload "$TAG" dist/* --clobber

      # setup-project: if-publish-index
      - name: Publish to {publish_index}
        env:
          UV_PUBLISH_USERNAME: ${{ secrets.UV_INDEX_{publish_index_key}_USERNAME }}
          UV_PUBLISH_PASSWORD: ${{ secrets.UV_INDEX_{publish_index_key}_PASSWORD }}
        run: uv publish --index {publish_index} dist/*
      # setup-project: end
      # setup-project: if-no-publish-index
      - name: Publish to PyPI
        run: uv publish --trusted-publishing always dist/*
      # setup-project: end
```

- [ ] **Step 2: Write the Rust (maturin) template**

Create `python/aeth_devkit/templates/github/workflows/release.rust.template.yml`:

```yaml
# Installed and kept current by `devkit setup-project`; edits are replaced on the next run.
# Triggered by `devkit release`, which creates the GitHub release with no assets and then
# waits for this workflow to build, attach and publish them. Wheels are built per platform
# with maturin; every artefact is attached and published from one job, so a partial
# publish can only follow a complete build.
name: Release

on:
  release:
    types: [published]

concurrency:
  group: release-${{ github.event.release.tag_name }}
  cancel-in-progress: false

permissions:
  contents: write

env:
  TAG: ${{ github.event.release.tag_name }}

jobs:
  build:
    name: Wheel (${{ matrix.target }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: true
      matrix:
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            manylinux: "2_17"
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.event.release.tag_name }}

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}

      - uses: astral-sh/setup-uv@v5

      - name: The tag must name the committed version
        shell: bash
        run: |
          v="$(uv version --short)"
          [ "v$v" = "$TAG" ] || { echo "tag $TAG does not match pyproject.toml version $v" >&2; exit 1; }

      - uses: PyO3/maturin-action@v1
        with:
          target: ${{ matrix.target }}
          # Empty on Windows, where the input is ignored.
          manylinux: ${{ matrix.manylinux }}
          args: --release --strip --out dist

      - uses: actions/upload-artifact@v4
        with:
          name: wheel-${{ matrix.target }}
          path: dist/*.whl
          if-no-files-found: error

  sdist:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.event.release.tag_name }}

      - uses: dtolnay/rust-toolchain@stable

      - uses: astral-sh/setup-uv@v5

      - name: Build the sdist
        run: uv build --sdist --out-dir dist

      - uses: actions/upload-artifact@v4
        with:
          name: sdist
          path: dist/*.tar.gz
          if-no-files-found: error

  publish:
    needs: [build, sdist]
    runs-on: ubuntu-latest
    # setup-project: if-no-publish-index
    permissions:
      contents: write
      id-token: write
    # setup-project: end
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.event.release.tag_name }}

      - uses: astral-sh/setup-uv@v5

      - uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true

      - name: The tag must name the committed version
        shell: bash
        run: |
          v="$(uv version --short)"
          [ "v$v" = "$TAG" ] || { echo "tag $TAG does not match pyproject.toml version $v" >&2; exit 1; }
          ls -l dist

      - name: Attach artefacts to the release
        env:
          GH_TOKEN: ${{ github.token }}
        run: gh release upload "$TAG" dist/* --clobber

      # setup-project: if-publish-index
      - name: Publish to {publish_index}
        env:
          UV_PUBLISH_USERNAME: ${{ secrets.UV_INDEX_{publish_index_key}_USERNAME }}
          UV_PUBLISH_PASSWORD: ${{ secrets.UV_INDEX_{publish_index_key}_PASSWORD }}
        run: uv publish --index {publish_index} dist/*
      # setup-project: end
      # setup-project: if-no-publish-index
      - name: Publish to PyPI
        run: uv publish --trusted-publishing always dist/*
      # setup-project: end
```

- [ ] **Step 3: Write the failing e2e tests**

Append to `crates/aeth-devkit-setup/tests/apply.rs`:

```rust
#[test]
fn release_workflow_is_installed_and_replaced_on_drift() {
  let dir = make_project();
  let root = dir.path();
  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let wf = read(root, ".github/workflows/release.yml");
  // The fixture is a pure-Python project with one publish index (SFTPyPI).
  assert!(wf.contains("uv publish --index SFTPyPI dist/*"), "{wf}");
  assert!(wf.contains("secrets.UV_INDEX_SFTPYPI_USERNAME"), "{wf}");
  assert!(!wf.contains("trusted-publishing") && !wf.contains("id-token"), "{wf}");
  assert!(!wf.contains("setup-project:"), "markers leaked:\n{wf}");
  assert!(!wf.contains("maturin"), "pure-Python project got the Rust variant:\n{wf}");
  assert!(
    changes.notes.iter().any(|n| n.contains("UV_INDEX_SFTPYPI_USERNAME") && n.contains("UV_INDEX_SFTPYPI_PASSWORD")),
    "{:?}",
    changes.notes
  );

  // Devkit-owned: a hand edit is put back, and it counts as a change (so `--check` fails).
  write(root, ".github/workflows/release.yml", "name: mine\n");
  let changes = aeth_devkit_setup::run(root, &templates(), true).unwrap();
  assert!(changes.files.iter().any(|f| f.path.ends_with("release.yml")), "{:?}", changes.files);
  assert_eq!(read(root, ".github/workflows/release.yml"), "name: mine\n", "dry run must not write");
  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert_eq!(read(root, ".github/workflows/release.yml"), wf);
  // The secrets note is for the first install only.
  let again = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  assert!(again.notes.iter().all(|n| !n.contains("UV_INDEX_")), "{:?}", again.notes);
}

#[test]
fn release_workflow_uses_pypi_when_no_index_publishes() {
  let dir = make_project();
  let root = dir.path();
  let py = read(root, "pyproject.toml").replace("publish-url", "x-publish-url");
  write(root, "pyproject.toml", &py);
  let changes = aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let wf = read(root, ".github/workflows/release.yml");
  assert!(wf.contains("uv publish --trusted-publishing always dist/*"), "{wf}");
  assert!(wf.contains("id-token: write"), "{wf}");
  assert!(!wf.contains("uv publish --index") && !wf.contains("setup-project:"), "{wf}");
  assert!(
    changes.notes.iter().any(|n| n.contains("trusted publisher") && n.contains("release.yml")),
    "{:?}",
    changes.notes
  );
}

#[test]
fn rust_projects_get_the_maturin_matrix_workflow() {
  let dir = make_project();
  let root = dir.path();
  write(root, "Cargo.toml", "[package]\nname = \"x\"\nversion = \"0.1.0\"\n");
  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let wf = read(root, ".github/workflows/release.yml");
  assert!(wf.contains("PyO3/maturin-action@v1"), "{wf}");
  assert!(wf.contains("x86_64-pc-windows-msvc") && wf.contains("x86_64-unknown-linux-gnu"), "{wf}");
  assert!(wf.contains("needs: [build, sdist]"), "{wf}");
  assert!(wf.contains("uv publish --index SFTPyPI dist/*") && !wf.contains("setup-project:"), "{wf}");
}
```

If `make_project()` writes a `docker/Dockerfile` and the fixture toggles `has_docker`, that is irrelevant here. Check `pyproject.fixture.toml` line 86–89 to confirm the index is named `SFTPyPI`; if it has another name, use that name in the assertions.

- [ ] **Step 4: Run the new tests to see them fail**

Run: `cargo test -p aeth-devkit-setup --test apply release_workflow`
Expected: FAIL — `.github/workflows/release.yml` not found.

- [ ] **Step 5: Install the workflow from `lib.rs`**

In `crates/aeth-devkit-setup/src/lib.rs`, after the step-10 `for` loop and before the step-11 comment, insert:

```rust
  // 10b. The release workflow is devkit-owned, unlike `claude.yml`: nothing in it is
  //      project-specific beyond the placeholders, so drift is replaced and reported. The
  //      one manual step — credentials — is announced the first time the file is installed.
  {
    let path = ctx.root.join(".github").join("workflows").join("release.yml");
    let template_name = if ctx.has_rust {
      "github/workflows/release.rust.yml"
    } else {
      "github/workflows/release.yml"
    };
    let raw = templates::load(templates_dir, template_name, &ctx, templates::Escape::None)?;
    let rendered = templates::gate_publish_index(&raw, ctx.publish_index.is_some());
    let original = read_optional(&path)?;
    let first_install = original.is_none();
    changes.record_optional(&path, original.as_deref(), &rendered, vec!["replaced with the devkit release workflow".into()])?;
    if first_install {
      changes.notes.push(match &ctx.publish_index {
        Some(name) => {
          let key = aeth_devkit_core::pyproject::index_env_key(name);
          format!(
            "the release workflow publishes to {name}; add the repository secrets UV_INDEX_{key}_USERNAME and \
             UV_INDEX_{key}_PASSWORD (gh secret set <NAME>) before the first `devkit release`."
          )
        }
        None => {
          let repo = git::is_git_tracked(&ctx.root)
            .then(|| aeth_devkit_core::git::origin_url(&ctx.root).ok().flatten())
            .flatten()
            .and_then(|u| aeth_devkit_core::github::github_repo_path(&u))
            .unwrap_or_else(|| "<owner>/<repo>".into());
          format!(
            "the release workflow publishes to PyPI with trusted publishing; register {repo} with workflow file name \
             release.yml as a trusted publisher at https://pypi.org/manage/account/publishing/ before the first `devkit release`."
          )
        }
      });
    }
  }
```

`git::is_git_tracked` is the setup crate's own `crate::git` (already imported as `git` in this file via `use` at the top? — it is referenced as `git::is_git_tracked(&ctx.root)` in step 13, so the module path `git::` resolves to `crate::git`; keep that spelling).

Update the step-10 comment so it no longer implies all workflows are create-if-missing: change `the Claude GitHub workflow (a project may have customized it; a routine run must not revert that)` to `the Claude GitHub workflow (a project may have customized it; a routine run must not revert that — the release workflow below is the opposite case)`.

- [ ] **Step 6: Run the setup crate tests**

Run: `cargo test -p aeth-devkit-setup`
Expected: PASS. If `applies_and_is_idempotent` asserts an exact list of changed files, add `"release.yml"` to it; if `claude_config_files_are_created_and_create_if_missing_ones_are_never_rewritten` asserts on the count of created files, adjust for the extra one. Read the failing assertion before editing — do not blanket-loosen tests.

- [ ] **Step 7: Commit**

```bash
git add python/aeth_devkit/templates/github/workflows crates/aeth-devkit-setup
git commit -m "feat(setup): install the devkit release workflow in every project

Two templates rendered as .github/workflows/release.yml: a single-job build,
attach and publish for pure-Python projects, and a maturin matrix (windows-msvc,
linux-gnu manylinux_2_17) plus sdist and publish jobs for Rust projects. The
publish step targets the sole publish index through UV_INDEX_<KEY>_* secrets,
or PyPI via trusted publishing when no index publishes. The file is
devkit-owned: drift is replaced and reported; the first install prints the
credential setup note.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Documentation, poe help text, and devkit's own workflow

**Files:**

- Modify: `README.md` (Commands table row; `devkit setup-project` and `devkit release` feature bullets; `release-and-pin`)
- Modify: `TODO.md`
- Modify: `python/aeth_devkit/_tasks_source.py` (`release` help) and regenerate `python/aeth_devkit/_tasks_generated.py`
- Create: `.github/workflows/release.yml` (by running setup-project on this repo)

- [ ] **Step 1: README — Commands table**

Replace the `poe release` row's description with:

`Bump version, commit, tag, push, create the GitHub release, then wait for the release workflow to build and publish; rolls back on failure.`

- [ ] **Step 2: README — `devkit setup-project` bullets**

In the `Agent docs` bullet, keep `.github/workflows/claude.yml` under create-if-missing and add a new bullet directly after it:

```markdown
- **Release workflow** - `.github/workflows/release.yml` is rendered from the pure-Python
  or the maturin-matrix template (`Cargo.toml` selects the latter) and is devkit-owned:
  any drift is replaced, reported, and counts for `--check`. The publish step targets the
  sole `[[tool.uv.index]]` with a `publish-url` through repository secrets
  `UV_INDEX_<KEY>_USERNAME` / `_PASSWORD`, or PyPI via trusted publishing when no index
  publishes; several publish indexes are a config error. The first install prints a
  `note:` with the secret names or the trusted-publisher registration values.
```

In the `Placeholders` bullet, extend the list to `{project_root}`, `{package}`, `{python_dir}`, `{devkit_bin}`, `{publish_index}`, `{publish_index_key}` (each in its own code span) and append this sentence:

```markdown
  YAML templates gate blocks with `# setup-project: if-publish-index` / `if-no-publish-index` …
  `end` markers.
```

- [ ] **Step 3: README — `devkit release` section**

Replace the whole `### devkit release` section with:

```markdown
### `devkit release`

Args: `[bump …] ["notes"]` (bump kinds are uv's `major minor patch stable alpha beta rc
post dev`, chainable; notes must be multi-word; no bump = re-release the current version,
pushing only the tag), `-f/--force`, `--dry-run` (prints the numbered plan, changes
nothing), `--index` (defaults to the sole index with a `publish-url`, or PyPI when there is
none), `--no-wait` (return once the GitHub release exists), `--root`. Flags parse anywhere
on the line.

- **Division of labour** - The command does the human half (bump, commit, tag, push,
  create the release) and waits for the devkit-installed release workflow to do the
  reproducible half (build every artefact on CI, attach it to the release, publish to the
  index). Nothing is built or published on the developer's machine.
- **Pre-flight** - Read-only checks: git/uv/gh present and `gh` able to list workflow
  runs; `.github/workflows/release.yml` committed at `HEAD`; on `main` with upstream,
  fetched, not behind; release config committed and matching HEAD; `Cargo.toml` version in
  sync; no merge conflicts in managed files; target version computed via `uv version
  --dry-run`.
- **Publish target** - The sole `[[tool.uv.index]]` with a `publish-url` (credentials
  `UV_INDEX_<KEY>_USERNAME/_PASSWORD` must be set locally for the pre-flight probe; CI
  reads the same names from repository secrets), or PyPI when no index publishes (no
  credentials; trusted publishing in CI).
- **Artefact detection** - Detects existing artefacts of the target tag (local/remote tag,
  GitHub release, index version — devpi's REST endpoint for a private index, the simple
  index for PyPI), shows a table, and removes them after confirmation (commits are never
  rewound here — that's `rescind-release`). An existing PyPI version aborts: PyPI files
  cannot be removed.
- **Prompts** - Two, both requiring the literal word `force` (dirty tree; remove existing
  artefacts); `--force` skips both.
- **Release steps** - Snapshot managed files → bump (pyproject, `Cargo.toml`, `cargo
  update`) → `uv lock` → quiet commit built through a scratch index (the machinery shared
  with `lock` and `setup-project`: uncommitted edits to managed files are replayed back
  afterwards; the user's staging is untouched; comparisons and the merge-back go through
  git's clean/smudge filters, so a `core.autocrlf` CRLF checkout is neither mistaken for
  an edit nor rewritten to LF) → annotated tag → one atomic `git push` of branch + tag →
  `gh release create` with the notes (or `--generate-notes`) and no files → wait for the
  release workflow run (`gh run list` until it appears, up to 120 s, then `gh run watch
  --exit-status`) and verify the version is on the publish target and the release still
  exists. `--no-wait` skips the last step and prints the workflow's Actions URL.
- **Rollback** - On any failure or Ctrl-C — a failed or missing workflow run included —
  the journal is walked backwards (restore files, soft-reset the commit, delete tag /
  remote tag / GitHub release), with force-with-lease guards so a concurrent release is
  never clobbered; anything that can't be undone prints an exact manual cleanup command.
  Artefacts the workflow already published are not removed; the next `devkit release` of
  the same version detects and offers to remove them.
- **Exit codes** - 0 released, 1 aborted or rolled back, 2 pre-flight/config error.
```

In `### devkit release-and-pin`, add a bullet:

```markdown
- **Waits for CI** - `--no-wait` is refused: the pin's completeness preflight needs the
  artefacts the workflow publishes, and `Released` already means the workflow finished.
```

- [ ] **Step 4: TODO.md**

- Under `## Release / packaging`, delete the `Linux wheel (…)` item and add:

```markdown
- [ ] Docker standardization (`docs/superpowers/specs/2026-09-03-docker-standardization-design.md`)
      extends the Rust release workflow's matrix with the container binary; the VS Code
      extension design adds a `vsix` job.
```

- Under `## setup-project`, delete the `--docker` scaffolding item's sub-bullet `Tests: e2e case for a fresh project with --docker` only if the Docker spec supersedes the item — it does not yet; leave the item alone.

- [ ] **Step 5: poe help text and the baked task table**

In `python/aeth_devkit/_tasks_source.py`, change the `release` task help's first sentence to:

`"Bump version, commit, tag, push, create the GitHub release, then wait for the release workflow to build and publish. "`

and add after the `--force` sentence: `"--no-wait returns as soon as the GitHub release exists. "`.

Regenerate the baked table and check it is what the build produces:

Run: `uv sync && uv run maturin develop && git diff --stat -- python/aeth_devkit/_tasks_generated.py`
Expected: `_tasks_generated.py` shows a change limited to the release help text.

Run: `uv run pytest tests/test_generated_tasks.py tests/test_build_regenerates.py`
Expected: PASS.

- [ ] **Step 6: Install devkit's own release workflow**

Run: `uv run devkit setup-project --no-commit`
Expected: `Changed:` lists `.github/workflows/release.yml: created` with the `note:` naming `UV_INDEX_SFTPYPI_USERNAME` / `UV_INDEX_SFTPYPI_PASSWORD`; the file uses the maturin matrix (this repo has `Cargo.toml`). Inspect it: `cat .github/workflows/release.yml`. If setup-project also touched other files, review them — they are expected to be no-ops; if not, stop and report what changed before committing.

Run: `uv run devkit setup-project --check; echo exit=$?`
Expected: `exit=0`.

- [ ] **Step 7: Full verification on main**

Run: `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && uv run pytest`
Expected: all clean and passing.

- [ ] **Step 8: Commit**

```bash
git add README.md TODO.md python/aeth_devkit/_tasks_source.py python/aeth_devkit/_tasks_generated.py .github/workflows/release.yml
git commit -m "docs(release): document the CI release workflow and install devkit's own

README feature reference for release and setup-project, TODO items closed and
handed to the Docker design, poe release help text, and this repository's
.github/workflows/release.yml rendered by setup-project.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

- [ ] **Step 9: Hand the manual steps to the user (do not perform them)**

Report to the user, verbatim in the final message:

- Add the two repository secrets before the next release (values are the ones in the local `.env`; never paste them into chat):

```bash
gh secret set UV_INDEX_SFTPYPI_USERNAME
gh secret set UV_INDEX_SFTPYPI_PASSWORD
```

(`gh secret set NAME` reads the value from stdin.)

- The first release after this lands should be `devkit release patch --dry-run` to see the eight-step plan, then a real `devkit release patch`; the workflow will build the Windows and Linux wheels plus the sdist, attach them, and publish to SFTPyPI. Any failure rolls back automatically.

---

## Self-review

### Spec coverage

| Spec section | Task |
| --- | --- |
| Steps 1–8, removal of build/publish/`DeleteDevpi`, `DevpiClient` stays for pre-flight | 1, 3 |
| Waiting for CI: `gh run list` poll with 120 s start-up timeout, `gh run watch --exit-status`, completeness check, `--no-wait`, CI failure unwinds | 3 |
| Pre-flight: workflow at `HEAD`, `gh run list --limit 1`, `PublishTarget` with PyPI probe and no removal, `--index` rules | 2, 3 |
| Unchanged: args, prompts, exit codes, dry-run plan, `release-and-pin` on `Released` | 3 (plan text), 3 (`--no-wait` guard) |
| Two templates, shared skeleton, version assertion, publish target variants, matrix/sdist/publish jobs, upload-and-publish from one job | 5 |
| Placeholders `{publish_index}` / `{publish_index_key}`, `if-publish-index` block markers | 4 |
| Devkit-owned merge rule, first-install `note:` | 5 |
| Devkit-specific consequences (Linux wheel, TODO item) | 6 |
| Testing list | 1–5 (unit + e2e) |
| Documentation | 6 |

Gap check: the spec's "Every job checks out the tag … asserts `uv version --short` equals the tag" — the sdist job in the Rust template does not assert (it builds nothing version-dependent that the publish job does not re-check before uploading; the publish job asserts before any upload). Accepted deviation, noted here.

### Placeholder scan

No TBD/TODO; every code step carries its code.

### Type consistency

`Deps.index: &dyn IndexClient` (Task 2) used by `ci::verify_published` and `preflight::probe` (Tasks 2–3); `PublishTarget::{label, simple_url}` used in `report`, `steps::describe`, `ci`; `Existing.index` used in `lib.rs` and `preflight`; `undo::unwind(journal, deps, root)` used in `lib.rs`; `ctx.publish_index: Option<String>` used by `templates::substitute` and `lib.rs` step 10b; `gate_publish_index(&str, bool)` used in step 10b; `index_env_key` used by `config::env_var_names`, `templates::substitute`, and the note in step 10b.
