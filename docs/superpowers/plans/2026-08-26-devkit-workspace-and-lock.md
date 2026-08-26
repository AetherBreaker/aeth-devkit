# aeth-devkit Workspace, Rename, and `devkit lock` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the repo into a Cargo workspace, rename everything from `poe-tasks`/`sft-setup` to `aeth-devkit` (command `devkit`), and port `lock.sh` to Rust as `devkit lock`.

**Architecture:** A `crates/aeth-devkit-core` library holds shared git/process/pyproject/index/version helpers. Each command is its own crate (`aeth-devkit-setup`, `aeth-devkit-lock`) exposing `pub struct Args` + `pub fn run(...)` plus a dev-only bin. A thin `crates/aeth-devkit` dispatcher bin (`devkit`) wraps the commands as clap subcommands and is the only binary shipped in the wheel. The Python package `python/aeth_devkit/` carries poe task definitions, remaining shell scripts, and templates.

**Tech Stack:** Rust 2024 (cargo 1.98), clap 4 (derive), toml_edit 0.25, anyhow, regex, serde/serde_json, ureq 3.4 (HTTP), pep440_rs 0.7 (versions), tempfile (tests); maturin 1.15 with `bindings = "bin"`; uv; poethepoet.

**Spec:** `docs/specs/2026-08-26-devkit-workspace-and-lock-design.md`

## Global Constraints

- Names (verbatim from spec): dist `aeth-devkit`; Python package `aeth_devkit`; executable `devkit`; crates `aeth-devkit`, `aeth-devkit-core`, `aeth-devkit-setup`, `aeth-devkit-lock`; libs `aeth_devkit_core`, `aeth_devkit_setup`, `aeth_devkit_lock`; dev bins `devkit-setup`, `devkit-lock`.
- Env var `DEVKIT_TEMPLATES` replaces `SFT_SETUP_TEMPLATES`; template layers `devkit.gitignore` / `devkit.dockerignore` replace `sft.*`.
- Setup commit subject: `Standardize project configuration with devkit`. Lock commit subject: `Update uv.lock`.
- Version: `7.0.0` in both `pyproject.toml` and every crate `Cargo.toml`.
- Exit codes: 0 ok; 1 `--check` drift; 2 usage/IO/parse/network error printed as `error: …` on stderr; 3 applied-but-commit-failed; `uv sync` failures propagate its code.
- Only the `devkit` binary ships in the wheel. Keep this repo's SFTPyPI index/publish URLs in `pyproject.toml`; keep fixture content unchanged.
- No network access in tests. No `sft`/`SFT` wording in help text, docs, or identifiers (fixtures and the index URL excepted).
- Code style: `rustfmt.toml` (2-space indent, width 135). Run `cargo fmt --all` before each commit.
- Cargo is at `C:\Users\User\.cargo\bin` and is **not** on PATH in the tool shells. In Bash prefix commands with `export PATH="/c/Users/User/.cargo/bin:$PATH";`. Run all cargo commands from the repo root `d:/SFT Software Projects/SFT Workspace/poe_tasks`.
- Commit after each task with the trailer `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

## File Structure

```
Cargo.toml                                   workspace root (members, workspace.dependencies, workspace.package, release profile)
.cargo/config.toml                           rust-lld linker for MSVC
crates/aeth-devkit-core/Cargo.toml
crates/aeth-devkit-core/src/lib.rs           pub mod git, process, pyproject, index, version
crates/aeth-devkit-core/src/git.rs           is_git_tracked, is_dirty, commit_paths, short_head
crates/aeth-devkit-core/src/process.rs       Runner trait, SystemRunner, RecordingRunner
crates/aeth-devkit-core/src/pyproject.rs     find_requirement, set_requirement_version, index_url_for
crates/aeth-devkit-core/src/index.rs         IndexClient trait, HttpIndexClient, versions_from_filenames, parse_simple_json, parse_simple_html
crates/aeth-devkit-core/src/version.rs       latest_stable
crates/aeth-devkit-setup/Cargo.toml
crates/aeth-devkit-setup/src/{lib,changes,context,json_merge,lines,templates,toml_merge,git}.rs   moved from src/ (git.rs shrinks to commit_changes on core)
crates/aeth-devkit-setup/src/cli.rs          pub struct Args + pub fn run(&Args) -> Result<ExitCode>
crates/aeth-devkit-setup/src/main.rs         dev bin devkit-setup
crates/aeth-devkit-setup/tests/apply.rs + tests/fixtures/   moved from tests/
crates/aeth-devkit-lock/Cargo.toml
crates/aeth-devkit-lock/src/lib.rs           pub struct Args, pub fn run(&Args, &dyn IndexClient, &dyn Runner) -> Result<ExitCode>, pub fn run_real(&Args)
crates/aeth-devkit-lock/src/main.rs          dev bin devkit-lock
crates/aeth-devkit-lock/tests/lock.rs        integration tests with stub client + recording runner
crates/aeth-devkit/Cargo.toml
crates/aeth-devkit/src/main.rs               bin devkit: clap subcommands SetupProject | Lock
python/aeth_devkit/__init__.py               poe tasks (renamed from python/poe_tasks)
python/aeth_devkit/scripts/*.sh              moved; lock.sh deleted
python/aeth_devkit/templates/                moved; sft.template.gitignore → devkit.template.gitignore; include_script → aeth_devkit:tasks
pyproject.toml                               name aeth-devkit, version 7.0.0, maturin manifest-path, scripts
README.md                                    usage + downstream migration
TODO.md                                      tick lock item, update wording
```

---

### Task 1: Workspace scaffold — move the setup crate into `crates/aeth-devkit-setup`

**Files:**
- Modify: `Cargo.toml` (becomes workspace root)
- Create: `.cargo/config.toml`
- Create: `crates/aeth-devkit-setup/Cargo.toml`
- Move: `src/*.rs` → `crates/aeth-devkit-setup/src/`, `tests/` → `crates/aeth-devkit-setup/tests/`
- Modify: `crates/aeth-devkit-setup/src/lib.rs`, `src/main.rs`, `src/templates.rs`, `tests/apply.rs` (crate name + paths)

**Interfaces:**
- Produces: crate `aeth-devkit-setup`, lib `aeth_devkit_setup` with today's public API (`run`, `templates::locate`, `git::*`, `context::strip_verbatim`); dev bin `devkit-setup`.

- [ ] **Step 1: Move files with git so history follows**

```bash
cd "d:/SFT Software Projects/SFT Workspace/poe_tasks"
mkdir -p crates/aeth-devkit-setup
git mv src crates/aeth-devkit-setup/src
git mv tests crates/aeth-devkit-setup/tests
```

- [ ] **Step 2: Write the workspace root `Cargo.toml`** (replace the whole file)

```toml
[workspace]
  resolver = "3"
  members  = ["crates/*"]

[workspace.package]
  version = "7.0.0"
  edition = "2024"
  publish = false

[workspace.dependencies]
  anyhow     = "1.0.104"
  clap       = { version = "4", features = ["derive"] }
  pep440_rs  = "0.7.3"
  regex      = "1.13.1"
  serde      = "1.0.229"
  serde_json = { version = "1.0.151", features = ["preserve_order"] }
  tempfile   = "3.27.0"
  toml_edit  = "0.25.13"
  ureq       = { version = "3.4.0", features = ["json"] }

  aeth-devkit-core  = { path = "crates/aeth-devkit-core" }
  aeth-devkit-setup = { path = "crates/aeth-devkit-setup" }
  aeth-devkit-lock  = { path = "crates/aeth-devkit-lock" }

[profile.release]
  strip       = true
  incremental = true
```

- [ ] **Step 3: Write `crates/aeth-devkit-setup/Cargo.toml`**

```toml
[package]
  name    = "aeth-devkit-setup"
  version.workspace = true
  edition.workspace = true
  publish.workspace = true

[lib]
  name = "aeth_devkit_setup"

[[bin]]
  name = "devkit-setup"
  path = "src/main.rs"

[dependencies]
  anyhow     = { workspace = true }
  clap       = { workspace = true }
  regex      = { workspace = true }
  serde      = { workspace = true }
  serde_json = { workspace = true }
  toml_edit  = { workspace = true }

[dev-dependencies]
  tempfile = { workspace = true }
```

- [ ] **Step 4: Write `.cargo/config.toml`**

```toml
[target.x86_64-pc-windows-msvc]
  linker = "rust-lld"
```

- [ ] **Step 5: Rename the crate references**

In `crates/aeth-devkit-setup/src/main.rs` replace every `sft_setup::` with `aeth_devkit_setup::` (4 occurrences) and change the clap `#[command(name = "sft-setup", …)]` to `name = "devkit-setup"`. Also change the doc comment on line 6 to `/// Standardize a project's configuration from the templates shipped with aeth-devkit.` and the `--no-commit` doc to `/// Do not commit the changes. By default, when the project is git-tracked, the changed files (never env files) are committed with a standard message.`

In `crates/aeth-devkit-setup/tests/apply.rs` replace every `sft_setup::` with `aeth_devkit_setup::` (about 13 occurrences) and change `templates()` to point two directories up:

```rust
fn templates() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("..")
    .join("python")
    .join("poe_tasks")
    .join("templates")
}
```

In `crates/aeth-devkit-setup/src/templates.rs` `locate()` change the dev fallback to:

```rust
  let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("..")
    .join("python")
    .join("poe_tasks")
    .join("templates");
```

(Package/env names change in Task 9; this task only fixes paths and crate names.)

In `crates/aeth-devkit-setup/src/lib.rs` change the first two doc lines to:

```rust
//! `devkit setup-project` — standardize a project's configuration from the templates shipped
//! with `aeth-devkit`. See docs/specs/2026-08-26-setup-project-design.md.
```

- [ ] **Step 6: Build and test**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test --workspace 2>&1 | tail -20`
Expected: all tests pass (the same set that passed before the move). If the link step fails with a `rust-lld` error, first try `linker = "rust-lld.exe"`; if it still fails, delete `.cargo/config.toml`, note it in the commit message, and continue — the linker is an optimization, not a requirement.

- [ ] **Step 7: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "Move sft-setup into a Cargo workspace as aeth-devkit-setup

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: Core crate with shared git helpers

**Files:**
- Create: `crates/aeth-devkit-core/Cargo.toml`, `crates/aeth-devkit-core/src/lib.rs`, `crates/aeth-devkit-core/src/git.rs`
- Modify: `crates/aeth-devkit-setup/Cargo.toml` (add core dep), `crates/aeth-devkit-setup/src/git.rs` (build on core)
- Test: `crates/aeth-devkit-core/src/git.rs` (unit tests using a temp repo)

**Interfaces:**
- Produces (`aeth_devkit_core::git`):
  - `pub fn is_git_tracked(root: &Path) -> bool`
  - `pub fn is_dirty(root: &Path, paths: &[&str]) -> Result<bool>` — true when `git diff --quiet -- <paths>` reports changes (tracked, unstaged or staged) **or** any listed path is untracked-but-present.
  - `pub fn commit_paths(root: &Path, paths: &[String], message: &str) -> Result<String>` — `git add -- paths`, `git commit --quiet -m message -- paths`, returns short hash.
  - `pub fn short_head(root: &Path) -> Result<String>`
  - `pub fn init_test_repo(root: &Path)` behind `#[cfg(any(test, feature = "test-util"))]` — `git init -q`, sets `user.name`/`user.email`, `commit.gpgsign=false`.

- [ ] **Step 1: Create the crate**

`crates/aeth-devkit-core/Cargo.toml`:

```toml
[package]
  name    = "aeth-devkit-core"
  version.workspace = true
  edition.workspace = true
  publish.workspace = true

[lib]
  name = "aeth_devkit_core"

[features]
  test-util = []

[dependencies]
  anyhow     = { workspace = true }
  pep440_rs  = { workspace = true }
  regex      = { workspace = true }
  serde_json = { workspace = true }
  toml_edit  = { workspace = true }
  ureq       = { workspace = true }

[dev-dependencies]
  tempfile = { workspace = true }
```

`crates/aeth-devkit-core/src/lib.rs`:

```rust
//! Shared building blocks for the `devkit` commands.

pub mod git;
```

- [ ] **Step 2: Write the failing tests** in `crates/aeth-devkit-core/src/git.rs`

```rust
//! Thin wrappers over the `git` CLI used by every command.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result, bail};

fn git(root: &Path) -> Command {
  let mut c = Command::new("git");
  c.current_dir(root);
  c
}

#[cfg(test)]
mod tests {
  use super::*;

  fn write(root: &Path, rel: &str, s: &str) {
    std::fs::write(root.join(rel), s).unwrap();
  }

  #[test]
  fn tracked_and_dirty_and_commit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    assert!(!is_git_tracked(root));
    init_test_repo(root);
    assert!(is_git_tracked(root));

    write(root, "a.txt", "1\n");
    assert!(is_dirty(root, &["a.txt"]).unwrap(), "untracked file counts as dirty");
    let hash = commit_paths(root, &["a.txt".into()], "first").unwrap();
    assert_eq!(hash, short_head(root).unwrap());
    assert!(!is_dirty(root, &["a.txt"]).unwrap());

    write(root, "a.txt", "2\n");
    assert!(is_dirty(root, &["a.txt"]).unwrap());
    assert!(!is_dirty(root, &["missing.txt"]).unwrap());
  }

  #[test]
  fn commit_only_listed_paths() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    init_test_repo(root);
    write(root, "a.txt", "a\n");
    write(root, "b.txt", "b\n");
    let _ = git(root).args(["add", "b.txt"]).status().unwrap();
    commit_paths(root, &["a.txt".into()], "only a").unwrap();
    let out = git(root).args(["show", "--name-only", "--format=", "HEAD"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "a.txt");
  }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core 2>&1 | tail -5`
Expected: compile error — `is_git_tracked`, `is_dirty`, `commit_paths`, `short_head`, `init_test_repo` not found.

- [ ] **Step 4: Implement** (insert between `fn git` and the tests module)

```rust
/// True when `root` is inside a git checkout.
pub fn is_git_tracked(root: &Path) -> bool {
  git(root)
    .args(["rev-parse", "--is-inside-work-tree"])
    .output()
    .is_ok_and(|o| o.status.success() && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

/// True when any of `paths` has uncommitted changes (tracked and modified/staged, or
/// untracked but present on disk).
pub fn is_dirty(root: &Path, paths: &[&str]) -> Result<bool> {
  let status = git(root)
    .args(["diff", "--quiet", "HEAD", "--"])
    .args(paths)
    .status()
    .context("running git diff")?;
  // `git diff --quiet HEAD` exits 1 on differences; on a repo without commits it errors
  // (128), which we treat as "dirty" if any path exists.
  if status.code() == Some(1) {
    return Ok(true);
  }
  let out = git(root)
    .args(["ls-files", "--others", "--exclude-standard", "--"])
    .args(paths)
    .output()
    .context("running git ls-files")?;
  if !String::from_utf8_lossy(&out.stdout).trim().is_empty() {
    return Ok(true);
  }
  if status.code() == Some(128) {
    return Ok(paths.iter().any(|p| root.join(p).exists()));
  }
  Ok(false)
}

/// Stage exactly `paths` and commit only those paths (anything else the user staged is
/// left alone). Returns the short hash of the new commit.
pub fn commit_paths(root: &Path, paths: &[String], message: &str) -> Result<String> {
  let status = git(root).arg("add").arg("--").args(paths).status().context("running git add")?;
  if !status.success() {
    bail!("git add failed");
  }
  let out = git(root)
    .args(["commit", "--quiet", "-m", message, "--"])
    .args(paths)
    .output()
    .context("running git commit")?;
  if !out.status.success() {
    bail!("git commit failed: {}", String::from_utf8_lossy(&out.stderr).trim());
  }
  short_head(root)
}

pub fn short_head(root: &Path) -> Result<String> {
  let out = git(root).args(["rev-parse", "--short", "HEAD"]).output().context("reading HEAD")?;
  Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git init` plus the identity config needed to commit, for tests.
#[cfg(any(test, feature = "test-util"))]
pub fn init_test_repo(root: &Path) {
  for args in [
    &["init", "-q", "-b", "main"][..],
    &["config", "user.email", "test@example.com"],
    &["config", "user.name", "Test"],
    &["config", "commit.gpgsign", "false"],
  ] {
    assert!(git(root).args(args).status().unwrap().success(), "git {args:?}");
  }
}
```

- [ ] **Step 5: Run tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core 2>&1 | tail -5`
Expected: `test result: ok. 2 passed`.

- [ ] **Step 6: Make setup use core**

Add to `crates/aeth-devkit-setup/Cargo.toml` `[dependencies]`: `aeth-devkit-core = { workspace = true }` and to `[dev-dependencies]`: `aeth-devkit-core = { workspace = true, features = ["test-util"] }`.

Replace `crates/aeth-devkit-setup/src/git.rs` entirely with:

```rust
//! Committing the changes `devkit setup-project` made, when the project is git-tracked.

use std::path::Path;
use std::process::Command;

use anyhow::{Context as _, Result};

pub use aeth_devkit_core::git::is_git_tracked;

use crate::changes::Changes;

pub const COMMIT_SUBJECT: &str = "Standardize project configuration with devkit";

/// Env files carry secrets: never auto-commit them, even if the repo happens to track one.
fn is_env_file(rel: &str) -> bool {
  let name = rel.rsplit('/').next().unwrap_or(rel);
  name == ".env" || name.ends_with(".env")
}

/// Files from `changes` that should be committed: never env files, and not gitignored.
fn trackable(root: &Path, changes: &Changes) -> Result<Vec<String>> {
  let mut out = Vec::new();
  for f in &changes.files {
    let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
    if is_env_file(&rel) {
      continue;
    }
    let ignored = Command::new("git")
      .current_dir(root)
      .args(["check-ignore", "-q", "--", &rel])
      .status()
      .context("running git check-ignore")?
      .success();
    if !ignored {
      out.push(rel);
    }
  }
  Ok(out)
}

/// Stage exactly the changed, non-ignored files and commit them. Returns the short hash,
/// or `None` when nothing trackable changed.
pub fn commit_changes(root: &Path, changes: &Changes) -> Result<Option<String>> {
  let files = trackable(root, changes)?;
  if files.is_empty() {
    return Ok(None);
  }
  let mut body = String::new();
  for f in &changes.files {
    let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
    if files.contains(&rel) {
      body.push_str(&format!("- {rel}: {}\n", if f.created { "created" } else { "updated" }));
    }
  }
  let message = format!("{COMMIT_SUBJECT}\n\n{body}");
  aeth_devkit_core::git::commit_paths(root, &files, &message).map(Some)
}
```

In `crates/aeth-devkit-setup/tests/apply.rs`, the commit test currently expects the old subject via `aeth_devkit_setup::git::COMMIT_SUBJECT`, so it keeps passing. If that test initializes its own git repo with inline `Command::new("git")` calls, leave them as-is.

- [ ] **Step 7: Run the workspace tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test --workspace 2>&1 | tail -8`
Expected: all pass.

- [ ] **Step 8: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "Add aeth-devkit-core with shared git helpers; setup commits through it

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: Core `process::Runner`

**Files:**
- Create: `crates/aeth-devkit-core/src/process.rs`
- Modify: `crates/aeth-devkit-core/src/lib.rs` (add `pub mod process;`)

**Interfaces:**
- Produces:
  ```rust
  pub struct Invocation { pub program: String, pub args: Vec<String>, pub cwd: PathBuf }
  pub trait Runner {
    /// Run with inherited stdio; returns the exit status code (None if killed by signal).
    fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>>;
  }
  pub struct SystemRunner;
  pub struct RecordingRunner { pub calls: RefCell<Vec<Invocation>>, pub exit_code: i32 }
  impl RecordingRunner { pub fn new(exit_code: i32) -> Self }
  ```

- [ ] **Step 1: Write the failing test** — create `crates/aeth-devkit-core/src/process.rs` with only the test module:

```rust
//! Running external commands, with a recording implementation for tests.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn recording_runner_records_and_returns_code() {
    let r = RecordingRunner::new(3);
    let code = r.run_inherit("uv", &["sync".into(), "--upgrade".into()], Path::new("/proj")).unwrap();
    assert_eq!(code, Some(3));
    let calls = r.calls.borrow();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, "uv");
    assert_eq!(calls[0].args, vec!["sync", "--upgrade"]);
    assert_eq!(calls[0].cwd, PathBuf::from("/proj"));
  }

  #[test]
  fn system_runner_runs_git_version() {
    let code = SystemRunner.run_inherit("git", &["--version".into()], Path::new(".")).unwrap();
    assert_eq!(code, Some(0));
  }
}
```

- [ ] **Step 2: Run to verify failure**

Add `pub mod process;` to `lib.rs`. Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core process 2>&1 | tail -5`
Expected: compile error, `RecordingRunner`/`SystemRunner` not found.

- [ ] **Step 3: Implement** (above the tests module)

```rust
/// One recorded external command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
  pub program: String,
  pub args: Vec<String>,
  pub cwd: PathBuf,
}

pub trait Runner {
  /// Run `program args` in `cwd` with inherited stdio. Returns the exit code, or `None`
  /// when the process was terminated by a signal.
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>>;
}

/// Executes commands for real.
pub struct SystemRunner;

impl Runner for SystemRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    let status = Command::new(program)
      .args(args)
      .current_dir(cwd)
      .status()
      .with_context(|| format!("running {program}"))?;
    Ok(status.code())
  }
}

/// Records every call and returns a fixed exit code; for tests.
pub struct RecordingRunner {
  pub calls: RefCell<Vec<Invocation>>,
  pub exit_code: i32,
}

impl RecordingRunner {
  pub fn new(exit_code: i32) -> Self {
    Self { calls: RefCell::new(Vec::new()), exit_code }
  }
}

impl Runner for RecordingRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    self.calls.borrow_mut().push(Invocation {
      program: program.to_string(),
      args: args.to_vec(),
      cwd: cwd.to_path_buf(),
    });
    Ok(Some(self.exit_code))
  }
}
```

- [ ] **Step 4: Run tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core process 2>&1 | tail -5`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "core: add process::Runner with system and recording implementations

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: Core `pyproject` helpers

**Files:**
- Create: `crates/aeth-devkit-core/src/pyproject.rs`
- Modify: `crates/aeth-devkit-core/src/lib.rs` (add `pub mod pyproject;`)

**Interfaces:**
- Produces:
  ```rust
  pub fn normalize_dist_name(name: &str) -> String            // PEP 503
  pub struct Requirement { pub table: String, pub index: usize, pub spec: String }
  pub fn find_requirement(doc: &DocumentMut, name: &str) -> Option<Requirement>
  pub fn set_requirement_version(spec: &str, version: &str) -> Option<String>  // None if no version token
  pub fn replace_requirement(doc: &mut DocumentMut, req: &Requirement, new_spec: &str)
  pub fn index_url_for(doc: &DocumentMut, name: &str) -> Option<String>
  ```
  `Requirement.table` is a dotted path like `project.dependencies`, `project.optional-dependencies.async`, `dependency-groups.dev`.

- [ ] **Step 1: Write the failing tests** — create `crates/aeth-devkit-core/src/pyproject.rs`:

```rust
//! Reading and editing dependency pins and index configuration in `pyproject.toml`.

use regex::Regex;
use std::sync::LazyLock;
use toml_edit::{Array, DocumentMut, Item, Value};

#[cfg(test)]
mod tests {
  use super::*;

  const DOC: &str = r#"
[project]
  name = "demo"
  dependencies = ["requests>=2", "aeth-ext[sftp, async]>=8.0.2 ; sys_platform == 'win32'"]

  [project.optional-dependencies]
    extra = ["numpy"]

[dependency-groups]
  dev = [
    "pyright>=1.1",
    { include-group = "extra" },
    "aeth_devkit >= 6.0.2",
  ]
  extra = ["pytest"]

[tool.uv]
  [tool.uv.sources]
    aeth-devkit = { index = "Private" }
    aeth-ext = [{ index = "Private", marker = "sys_platform == 'linux'" }]

[[tool.uv.index]]
  name     = "Private"
  url      = "https://pypi.example.com/user/internal/+simple"
  explicit = true
"#;

  fn doc() -> DocumentMut {
    DOC.parse().unwrap()
  }

  #[test]
  fn finds_requirements_in_every_table() {
    let d = doc();
    let r = find_requirement(&d, "aeth-devkit").unwrap();
    assert_eq!((r.table.as_str(), r.index, r.spec.as_str()), ("dependency-groups.dev", 2, "aeth_devkit >= 6.0.2"));
    let r = find_requirement(&d, "AETH_EXT").unwrap();
    assert_eq!(r.table, "project.dependencies");
    assert_eq!(r.index, 1);
    let r = find_requirement(&d, "numpy").unwrap();
    assert_eq!(r.table, "project.optional-dependencies.extra");
    assert!(find_requirement(&d, "nope").is_none());
  }

  #[test]
  fn rewrites_only_the_version_token() {
    assert_eq!(set_requirement_version("aeth-devkit>=6.0.2", "7.0.0").unwrap(), "aeth-devkit>=7.0.0");
    assert_eq!(set_requirement_version("aeth_devkit >= 6.0.2", "7.0.0").unwrap(), "aeth_devkit >= 7.0.0");
    assert_eq!(set_requirement_version("x[a, b]==1.0a1", "1.0").unwrap(), "x[a, b]==1.0");
    assert_eq!(
      set_requirement_version("aeth-ext[sftp]>=8.0.2 ; sys_platform == 'win32'", "9.0.0").unwrap(),
      "aeth-ext[sftp]>=9.0.0 ; sys_platform == 'win32'"
    );
    assert_eq!(set_requirement_version("x~=1.2", "1.3").unwrap(), "x~=1.3");
    assert!(set_requirement_version("numpy", "2.0").is_none());
  }

  #[test]
  fn replaces_in_document_preserving_formatting() {
    let mut d = doc();
    let r = find_requirement(&d, "aeth-devkit").unwrap();
    replace_requirement(&mut d, &r, "aeth_devkit >= 7.0.0");
    let out = d.to_string();
    assert!(out.contains("    \"aeth_devkit >= 7.0.0\",\n"), "{out}");
    assert!(out.contains("{ include-group = \"extra\" }"));
  }

  #[test]
  fn resolves_index_url_from_sources() {
    let d = doc();
    assert_eq!(index_url_for(&d, "aeth-devkit").as_deref(), Some("https://pypi.example.com/user/internal/+simple"));
    assert_eq!(index_url_for(&d, "aeth-ext").as_deref(), Some("https://pypi.example.com/user/internal/+simple"));
    assert!(index_url_for(&d, "requests").is_none());
  }
}
```

- [ ] **Step 2: Run to verify failure**

Add `pub mod pyproject;` to `lib.rs`. Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core pyproject 2>&1 | tail -5`
Expected: compile errors for the missing functions.

- [ ] **Step 3: Implement** (above the tests module)

```rust
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

/// Distribution name at the start of a PEP 508 requirement string.
pub fn requirement_name(spec: &str) -> String {
  let end = spec
    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
    .unwrap_or(spec.len());
  normalize_dist_name(&spec[..end])
}

/// Where a dependency pin lives: the dotted table path, the array index, and the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
  pub table: String,
  pub index: usize,
  pub spec: String,
}

fn array_at<'a>(doc: &'a DocumentMut, path: &str) -> Option<&'a Array> {
  let mut item: &Item = doc.as_item();
  for key in path.split('.') {
    item = item.get(key)?;
  }
  item.as_array()
}

fn array_at_mut<'a>(doc: &'a mut DocumentMut, path: &str) -> Option<&'a mut Array> {
  let mut item: &mut Item = doc.as_item_mut();
  for key in path.split('.') {
    item = item.get_mut(key)?;
  }
  item.as_array_mut()
}

/// Every table path that can hold requirement strings, in document order.
fn requirement_tables(doc: &DocumentMut) -> Vec<String> {
  let mut out = vec!["project.dependencies".to_string()];
  if let Some(t) = doc.get("project").and_then(|p| p.get("optional-dependencies")).and_then(Item::as_table_like) {
    out.extend(t.iter().map(|(k, _)| format!("project.optional-dependencies.{k}")));
  }
  if let Some(t) = doc.get("dependency-groups").and_then(Item::as_table_like) {
    out.extend(t.iter().map(|(k, _)| format!("dependency-groups.{k}")));
  }
  out
}

/// Find the first requirement string for `name` (normalized comparison) across
/// `project.dependencies`, `project.optional-dependencies.*`, and `dependency-groups.*`.
/// Non-string array entries (e.g. `{ include-group = … }`) are skipped.
pub fn find_requirement(doc: &DocumentMut, name: &str) -> Option<Requirement> {
  let want = normalize_dist_name(name);
  for table in requirement_tables(doc) {
    let Some(arr) = array_at(doc, &table) else { continue };
    for (index, v) in arr.iter().enumerate() {
      if let Some(spec) = v.as_str()
        && requirement_name(spec) == want
      {
        return Some(Requirement { table, index, spec: spec.to_string() });
      }
    }
  }
  None
}

static VERSION_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
  // name, optional extras, optional whitespace, operator, optional whitespace, then the version.
  Regex::new(r"^(?P<head>[A-Za-z0-9][A-Za-z0-9._-]*(?:\s*\[[^\]]*\])?\s*(?:===|==|~=|!=|<=|>=|<|>)\s*)(?P<ver>[0-9][0-9A-Za-z.+!*-]*)").unwrap()
});

/// Replace the version token that directly follows the first comparison operator, keeping
/// the name, extras, operator, whitespace, and anything after (markers, further clauses).
/// Returns `None` when the spec has no version to replace.
pub fn set_requirement_version(spec: &str, version: &str) -> Option<String> {
  let caps = VERSION_TOKEN.captures(spec)?;
  let whole = caps.get(0)?;
  Some(format!("{}{}{}", &caps["head"], version, &spec[whole.end()..]))
}

/// Overwrite the spec at `req` with `new_spec`, keeping the element's surrounding whitespace.
pub fn replace_requirement(doc: &mut DocumentMut, req: &Requirement, new_spec: &str) {
  if let Some(arr) = array_at_mut(doc, &req.table)
    && let Some(cur) = arr.get(req.index)
  {
    let mut v = Value::from(new_spec);
    *v.decor_mut() = cur.decor().clone();
    arr.replace(req.index, v);
  }
}

/// The simple-index URL uv is told to use for `name`: `tool.uv.sources.<name>` (a table or
/// an array of tables) names an index, and `[[tool.uv.index]]` maps that name to a URL.
pub fn index_url_for(doc: &DocumentMut, name: &str) -> Option<String> {
  let want = normalize_dist_name(name);
  let sources = doc.get("tool")?.get("uv")?.get("sources")?.as_table_like()?;
  let (_, source) = sources.iter().find(|(k, _)| normalize_dist_name(k) == want)?;
  let index_name = match source {
    Item::Value(Value::InlineTable(t)) => t.get("index")?.as_str()?.to_string(),
    Item::Value(Value::Array(a)) => a.iter().find_map(|v| v.as_inline_table()?.get("index")?.as_str().map(str::to_string))?,
    Item::Table(t) => t.get("index")?.as_str()?.to_string(),
    Item::ArrayOfTables(a) => a.iter().find_map(|t| t.get("index")?.as_str().map(str::to_string))?,
    _ => return None,
  };
  let indexes = doc.get("tool")?.get("uv")?.get("index")?;
  let entries: Vec<&dyn toml_edit::TableLike> = match indexes {
    Item::ArrayOfTables(a) => a.iter().map(|t| t as &dyn toml_edit::TableLike).collect(),
    Item::Value(Value::Array(a)) => a.iter().filter_map(|v| v.as_inline_table().map(|t| t as &dyn toml_edit::TableLike)).collect(),
    _ => return None,
  };
  entries
    .iter()
    .find(|t| t.get("name").and_then(Item::as_str) == Some(index_name.as_str()))
    .and_then(|t| t.get("url").and_then(Item::as_str).map(str::to_string))
}
```

- [ ] **Step 4: Run tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core pyproject 2>&1 | tail -8`
Expected: 4 passed. If `TableLike` casts fail to compile in `index_url_for`, replace the `entries` construction with two explicit loops over `ArrayOfTables` and inline-table arrays that each check `name`/`url` and return the url string.

- [ ] **Step 5: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "core: pyproject helpers for finding/rewriting pins and resolving uv index URLs

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: Core `version::latest_stable`

**Files:**
- Create: `crates/aeth-devkit-core/src/version.rs`
- Modify: `crates/aeth-devkit-core/src/lib.rs` (add `pub mod version;`)

**Interfaces:**
- Produces: `pub fn latest_stable<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<String>` — highest PEP 440 version that is not pre/dev/post/local; returned in its normalized string form.

- [ ] **Step 1: Write the failing test** — `crates/aeth-devkit-core/src/version.rs`:

```rust
//! Choosing a release version from an index listing.

use pep440_rs::Version;

#[cfg(test)]
mod tests {
  use super::latest_stable;

  #[test]
  fn picks_highest_final_release() {
    let v = ["6.0.2", "7.0.0a1", "6.10.0", "6.9.9.post1", "6.10.0.dev3", "6.3.0+local", "junk"];
    assert_eq!(latest_stable(v).as_deref(), Some("6.10.0"));
  }

  #[test]
  fn none_when_only_prereleases() {
    assert_eq!(latest_stable(["1.0a1", "1.0rc1"]), None);
    assert_eq!(latest_stable([]), None);
  }
}
```

- [ ] **Step 2: Run to verify failure**

Add `pub mod version;` to `lib.rs`. Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core version 2>&1 | tail -5`
Expected: compile error, `latest_stable` not found.

- [ ] **Step 3: Implement**

```rust
/// The highest final release among `versions` (no pre/dev/post/local segments), rendered
/// in normalized PEP 440 form. Unparseable strings are ignored.
pub fn latest_stable<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<String> {
  versions
    .into_iter()
    .filter_map(|s| s.parse::<Version>().ok())
    .filter(|v| !v.any_prerelease() && !v.is_post() && !v.is_local())
    .max()
    .map(|v| v.to_string())
}
```

- [ ] **Step 4: Run tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core version 2>&1 | tail -5`
Expected: 2 passed. (If `is_post`/`is_local` don't exist in pep440_rs 0.7.3, use `v.post().is_none()` and `v.local().is_empty()` — check with `cargo doc -p pep440_rs --open` or the crate's `src/version/mod.rs` in `~/.cargo/registry`.)

- [ ] **Step 5: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "core: latest_stable version selection via pep440_rs

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: Core `index` — simple-index client

**Files:**
- Create: `crates/aeth-devkit-core/src/index.rs`
- Modify: `crates/aeth-devkit-core/src/lib.rs` (add `pub mod index;`)

**Interfaces:**
- Produces:
  ```rust
  pub trait IndexClient { fn versions(&self, simple_url: &str, package: &str) -> Result<Vec<String>>; }
  pub struct HttpIndexClient;
  pub struct StubIndexClient { pub versions: Vec<String> }   // returns a clone; for tests
  pub fn project_url(simple_url: &str, package: &str) -> String   // "<simple>/<normalized pkg>/"
  pub fn versions_from_filenames<'a>(package: &str, filenames: impl IntoIterator<Item = &'a str>) -> Vec<String>
  pub fn parse_simple_json(body: &str, package: &str) -> Result<Vec<String>>
  pub fn parse_simple_html(body: &str, package: &str) -> Vec<String>
  ```

- [ ] **Step 1: Write the failing tests** — `crates/aeth-devkit-core/src/index.rs`:

```rust
//! Looking up published versions on a PEP 503 / PEP 691 simple index.

use anyhow::{Context as _, Result};
use regex::Regex;
use std::sync::LazyLock;

use crate::pyproject::normalize_dist_name;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn builds_project_url() {
    assert_eq!(project_url("https://x/+simple", "Aeth_DevKit"), "https://x/+simple/aeth-devkit/");
    assert_eq!(project_url("https://x/simple/", "a"), "https://x/simple/a/");
  }

  #[test]
  fn extracts_versions_from_wheel_and_sdist_names() {
    let files = [
      "aeth_devkit-6.0.2-py3-none-any.whl",
      "aeth_devkit-6.0.2.tar.gz",
      "aeth_devkit-7.0.0a1-cp314-cp314-win_amd64.whl",
      "aeth-devkit-5.1.0.zip",
      "other_pkg-1.0.0.tar.gz",
      "README.txt",
    ];
    let mut v = versions_from_filenames("aeth-devkit", files);
    v.sort();
    v.dedup();
    assert_eq!(v, vec!["5.1.0", "6.0.2", "7.0.0a1"]);
  }

  #[test]
  fn parses_pep691_json() {
    let body = r#"{"name":"aeth-devkit","versions":["6.0.2","6.1.0"],"files":[{"filename":"aeth_devkit-6.0.2-py3-none-any.whl","url":"x"}]}"#;
    let mut v = parse_simple_json(body, "aeth-devkit").unwrap();
    v.sort();
    v.dedup();
    assert_eq!(v, vec!["6.0.2", "6.1.0"]);
    let body = r#"{"name":"aeth-devkit","files":[{"filename":"aeth_devkit-6.0.2.tar.gz"},{"filename":"aeth_devkit-6.0.3-py3-none-any.whl"}]}"#;
    assert_eq!(parse_simple_json(body, "aeth-devkit").unwrap(), vec!["6.0.2", "6.0.3"]);
  }

  #[test]
  fn parses_simple_html() {
    let body = r#"<html><body><h1>Links for aeth-devkit</h1>
<a href="../../packages/aeth_devkit-6.0.2-py3-none-any.whl#sha256=abc">aeth_devkit-6.0.2-py3-none-any.whl</a><br/>
<a href="../../packages/aeth_devkit-6.1.0.tar.gz">aeth_devkit-6.1.0.tar.gz</a>
</body></html>"#;
    assert_eq!(parse_simple_html(body, "aeth-devkit"), vec!["6.0.2", "6.1.0"]);
  }

  #[test]
  fn stub_client_returns_versions() {
    let c = StubIndexClient { versions: vec!["1.0".into()] };
    assert_eq!(c.versions("u", "p").unwrap(), vec!["1.0"]);
  }
}
```

- [ ] **Step 2: Run to verify failure**

Add `pub mod index;` to `lib.rs`. Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core index 2>&1 | tail -5`
Expected: compile errors.

- [ ] **Step 3: Implement** (above the tests module)

```rust
pub const PEP691_JSON: &str = "application/vnd.pypi.simple.v1+json";

/// Something that can list the versions published for a package.
pub trait IndexClient {
  fn versions(&self, simple_url: &str, package: &str) -> Result<Vec<String>>;
}

/// `<simple_url>/<normalized package>/`.
pub fn project_url(simple_url: &str, package: &str) -> String {
  format!("{}/{}/", simple_url.trim_end_matches('/'), normalize_dist_name(package))
}

/// Versions found in wheel (`name-ver-…​.whl`) and sdist (`name-ver.tar.gz` / `.zip`) file
/// names belonging to `package`. Order follows the input; duplicates are kept.
pub fn versions_from_filenames<'a>(package: &str, filenames: impl IntoIterator<Item = &'a str>) -> Vec<String> {
  let want = normalize_dist_name(package);
  let mut out = Vec::new();
  for f in filenames {
    let stem = if let Some(s) = f.strip_suffix(".whl") {
      // name-version(-build)?-python-abi-platform
      let mut parts = s.splitn(3, '-');
      let (Some(name), Some(ver)) = (parts.next(), parts.next()) else { continue };
      if normalize_dist_name(name) != want {
        continue;
      }
      Some(ver.to_string())
    } else if let Some(s) = f.strip_suffix(".tar.gz").or_else(|| f.strip_suffix(".zip")) {
      // sdist names may use '-' inside the project name; the version is after the last '-'
      // whose prefix normalizes to the package name.
      s.rmatch_indices('-')
        .find(|(i, _)| normalize_dist_name(&s[..*i]) == want)
        .map(|(i, _)| s[i + 1..].to_string())
    } else {
      None
    };
    if let Some(v) = stem {
      out.push(v);
    }
  }
  out
}

/// PEP 691 JSON project page: use `versions` when present, else derive from `files`.
pub fn parse_simple_json(body: &str, package: &str) -> Result<Vec<String>> {
  let v: serde_json::Value = serde_json::from_str(body).context("parsing simple-index JSON")?;
  if let Some(arr) = v.get("versions").and_then(|x| x.as_array()) {
    return Ok(arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
  }
  let files = v
    .get("files")
    .and_then(|x| x.as_array())
    .map(|a| a.iter().filter_map(|f| f.get("filename").and_then(|n| n.as_str())).collect::<Vec<_>>())
    .unwrap_or_default();
  Ok(versions_from_filenames(package, files))
}

static ANCHOR_TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<a\b[^>]*>(.*?)</a>").unwrap());

/// PEP 503 HTML project page: the text of each `<a>` is a file name.
pub fn parse_simple_html(body: &str, package: &str) -> Vec<String> {
  let names: Vec<&str> = ANCHOR_TEXT.captures_iter(body).map(|c| c.get(1).unwrap().as_str().trim()).collect();
  versions_from_filenames(package, names)
}

/// Fetches the project page over HTTP, preferring PEP 691 JSON and falling back to HTML.
pub struct HttpIndexClient;

impl IndexClient for HttpIndexClient {
  fn versions(&self, simple_url: &str, package: &str) -> Result<Vec<String>> {
    let url = project_url(simple_url, package);
    let mut resp = ureq::get(&url)
      .header("Accept", &format!("{PEP691_JSON}, text/html;q=0.1"))
      .call()
      .with_context(|| format!("fetching {url}"))?;
    let content_type = resp
      .headers()
      .get("content-type")
      .and_then(|v| v.to_str().ok())
      .unwrap_or("")
      .to_ascii_lowercase();
    let body = resp.body_mut().read_to_string().with_context(|| format!("reading {url}"))?;
    if content_type.contains("json") {
      parse_simple_json(&body, package)
    } else {
      Ok(parse_simple_html(&body, package))
    }
  }
}

/// Returns a fixed list; for tests.
pub struct StubIndexClient {
  pub versions: Vec<String>,
}

impl IndexClient for StubIndexClient {
  fn versions(&self, _simple_url: &str, _package: &str) -> Result<Vec<String>> {
    Ok(self.versions.clone())
  }
}
```

- [ ] **Step 4: Run tests and clippy**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-core 2>&1 | tail -5 && cargo clippy -p aeth-devkit-core 2>&1 | tail -5`
Expected: all core tests pass (13 total), no clippy errors. If ureq 3's API differs (`headers()` / `body_mut().read_to_string()`), check `~/.cargo/registry/src/*/ureq-3.4.0/README.md` and adapt — the behaviour (Accept header, content-type branch) must stay.

- [ ] **Step 5: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "core: simple-index client (PEP 691 JSON with HTML fallback)

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: `aeth-devkit-lock` crate

**Files:**
- Create: `crates/aeth-devkit-lock/Cargo.toml`, `src/lib.rs`, `src/main.rs`, `tests/lock.rs`

**Interfaces:**
- Consumes: `aeth_devkit_core::{git, process::{Runner, SystemRunner, RecordingRunner}, pyproject, index::{IndexClient, HttpIndexClient, StubIndexClient}, version}`.
- Produces:
  ```rust
  #[derive(clap::Parser, Debug, Clone)] pub struct Args { root: PathBuf, package: Vec<String>, no_commit: bool, dry_run: bool, uv_args: Vec<String> }
  pub const COMMIT_SUBJECT: &str = "Update uv.lock";
  pub const DEFAULT_PACKAGE: &str = "aeth-devkit";
  pub fn run(args: &Args, index: &dyn IndexClient, runner: &dyn Runner) -> Result<ExitCode>
  pub fn run_real(args: &Args) -> Result<ExitCode>   // wires HttpIndexClient + SystemRunner
  ```

- [ ] **Step 1: Create the crate**

`crates/aeth-devkit-lock/Cargo.toml`:

```toml
[package]
  name    = "aeth-devkit-lock"
  version.workspace = true
  edition.workspace = true
  publish.workspace = true

[lib]
  name = "aeth_devkit_lock"

[[bin]]
  name = "devkit-lock"
  path = "src/main.rs"

[dependencies]
  aeth-devkit-core = { workspace = true }
  anyhow           = { workspace = true }
  clap             = { workspace = true }
  toml_edit        = { workspace = true }

[dev-dependencies]
  aeth-devkit-core = { workspace = true, features = ["test-util"] }
  tempfile         = { workspace = true }
```

`crates/aeth-devkit-lock/src/main.rs`:

```rust
use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  let args = aeth_devkit_lock::Args::parse();
  match aeth_devkit_lock::run_real(&args) {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
```

- [ ] **Step 2: Write the failing integration tests** — `crates/aeth-devkit-lock/tests/lock.rs`:

```rust
//! `devkit lock` end to end against a temp git repo, with the index and uv stubbed.

use std::path::Path;
use std::process::ExitCode;

use aeth_devkit_core::git;
use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_lock::{Args, COMMIT_SUBJECT, run};

const PYPROJECT: &str = r#"[project]
  name = "demo"
  dependencies = ["requests>=2"]

[dependency-groups]
  dev = [
    "aeth-devkit>=6.0.2",
  ]

[tool.uv.sources]
  aeth-devkit = { index = "Private" }

[[tool.uv.index]]
  name = "Private"
  url  = "https://pypi.example.com/+simple"
"#;

fn project(with_git: bool) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(dir.path().join("pyproject.toml"), PYPROJECT).unwrap();
  std::fs::write(dir.path().join("uv.lock"), "version = 1\n").unwrap();
  if with_git {
    git::init_test_repo(dir.path());
    git::commit_paths(dir.path(), &["pyproject.toml".into(), "uv.lock".into()], "init").unwrap();
  }
  dir
}

fn args(root: &Path) -> Args {
  Args { root: root.to_path_buf(), package: vec![], no_commit: false, dry_run: false, uv_args: vec![] }
}

fn read(root: &Path, rel: &str) -> String {
  std::fs::read_to_string(root.join(rel)).unwrap()
}

fn last_subject(root: &Path) -> String {
  let out = std::process::Command::new("git")
    .current_dir(root)
    .args(["log", "-1", "--format=%s"])
    .output()
    .unwrap();
  String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn bumps_pin_syncs_and_commits() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["6.0.2".into(), "7.1.0".into(), "8.0.0a1".into()] };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.uv_args = vec!["--upgrade".into()];

  let code = run(&a, &index, &runner).unwrap();
  assert_eq!(code, ExitCode::SUCCESS);
  assert!(read(root, "pyproject.toml").contains("\"aeth-devkit>=7.1.0\","));

  let calls = runner.calls.borrow();
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0].program, "uv");
  assert_eq!(calls[0].args, vec!["sync", "--upgrade"]);
  assert_eq!(calls[0].cwd, root);

  assert_eq!(last_subject(root), COMMIT_SUBJECT);
  assert!(!git::is_dirty(root, &["pyproject.toml", "uv.lock"]).unwrap());
}

#[test]
fn already_current_pin_and_clean_lock_commits_nothing() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["6.0.2".into()] };
  let runner = RecordingRunner::new(0);
  run(&args(root), &index, &runner).unwrap();
  assert_eq!(last_subject(root), "init");
  assert_eq!(runner.calls.borrow().len(), 1, "uv sync still runs");
}

#[test]
fn missing_pin_is_skipped_but_sync_runs() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["9.9.9".into()] };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.package = vec!["not-there".into()];
  run(&a, &index, &runner).unwrap();
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=6.0.2"));
  assert_eq!(runner.calls.borrow().len(), 1);
}

#[test]
fn uv_failure_propagates_exit_code_and_skips_commit() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["7.0.0".into()] };
  let runner = RecordingRunner::new(7);
  let code = run(&args(root), &index, &runner).unwrap();
  assert_eq!(code, ExitCode::from(7));
  assert_eq!(last_subject(root), "init");
}

#[test]
fn dry_run_changes_nothing() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["7.0.0".into()] };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.dry_run = true;
  run(&a, &index, &runner).unwrap();
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=6.0.2"));
  assert!(runner.calls.borrow().is_empty());
  assert_eq!(last_subject(root), "init");
}

#[test]
fn no_commit_leaves_changes_in_tree() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["7.0.0".into()] };
  let runner = RecordingRunner::new(0);
  let mut a = args(root);
  a.no_commit = true;
  run(&a, &index, &runner).unwrap();
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=7.0.0"));
  assert_eq!(last_subject(root), "init");
  assert!(git::is_dirty(root, &["pyproject.toml"]).unwrap());
}

#[test]
fn not_a_git_repo_skips_commit() {
  let dir = project(false);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["7.0.0".into()] };
  let runner = RecordingRunner::new(0);
  let code = run(&args(root), &index, &runner).unwrap();
  assert_eq!(code, ExitCode::SUCCESS);
  assert!(read(root, "pyproject.toml").contains("aeth-devkit>=7.0.0"));
}

#[test]
fn no_stable_version_is_an_error() {
  let dir = project(true);
  let root = dir.path();
  let index = StubIndexClient { versions: vec!["7.0.0a1".into()] };
  let runner = RecordingRunner::new(0);
  let err = run(&args(root), &index, &runner).unwrap_err().to_string();
  assert!(err.contains("No stable release versions found for aeth-devkit"), "{err}");
  assert!(runner.calls.borrow().is_empty());
}
```

- [ ] **Step 3: Write a minimal `src/lib.rs` so the tests compile, then run to see them fail**

```rust
//! `devkit lock` — bump a dependency pin to the latest stable release on its index, run
//! `uv sync`, and commit `uv.lock` (and the pin change).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::process::Runner;

pub const COMMIT_SUBJECT: &str = "Update uv.lock";
pub const DEFAULT_PACKAGE: &str = "aeth-devkit";
const PUBLIC_INDEX: &str = "https://pypi.org/simple/";

/// Bump a dependency pin to the latest stable release, `uv sync`, and commit uv.lock.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-lock", version, about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Dependency pin(s) to bump; repeatable. Defaults to aeth-devkit.
  #[arg(long = "package", short = 'p')]
  pub package: Vec<String>,

  /// Do not commit uv.lock / pyproject.toml after syncing.
  #[arg(long)]
  pub no_commit: bool,

  /// Report what would change without writing, syncing, or committing.
  #[arg(long)]
  pub dry_run: bool,

  /// Extra arguments forwarded verbatim to `uv sync` (after `--`).
  #[arg(last = true)]
  pub uv_args: Vec<String>,
}

pub fn run(_args: &Args, _index: &dyn IndexClient, _runner: &dyn Runner) -> Result<ExitCode> {
  todo!()
}

pub fn run_real(args: &Args) -> Result<ExitCode> {
  run(args, &aeth_devkit_core::index::HttpIndexClient, &aeth_devkit_core::process::SystemRunner)
}
```

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-lock 2>&1 | tail -12`
Expected: 8 tests, all FAIL with `not yet implemented`.

- [ ] **Step 4: Implement `run`** (replace the `todo!()` body and add the helpers)

```rust
pub fn run(args: &Args, index: &dyn IndexClient, runner: &dyn Runner) -> Result<ExitCode> {
  let root = args.root.canonicalize().with_context(|| format!("resolving {}", args.root.display()))?;
  let root = strip_verbatim(root);
  let pyproject_path = root.join("pyproject.toml");
  let original = std::fs::read_to_string(&pyproject_path)
    .with_context(|| format!("{} not found", pyproject_path.display()))?;
  let mut doc: DocumentMut = original.parse().context("parsing pyproject.toml")?;

  let targets: Vec<&str> = if args.package.is_empty() {
    vec![DEFAULT_PACKAGE]
  } else {
    args.package.iter().map(String::as_str).collect()
  };
  for pkg in targets {
    bump_pin(&mut doc, pkg, index)?;
  }

  let updated = doc.to_string();
  if updated != original {
    if args.dry_run {
      println!("Would write pyproject.toml");
    } else {
      std::fs::write(&pyproject_path, &updated).context("writing pyproject.toml")?;
    }
  }

  let mut uv_args = vec!["sync".to_string()];
  uv_args.extend(args.uv_args.iter().cloned());
  if args.dry_run {
    println!("Would run: uv {}", uv_args.join(" "));
    return Ok(ExitCode::SUCCESS);
  }
  match runner.run_inherit("uv", &uv_args, &root)? {
    Some(0) => {}
    Some(code) => return Ok(ExitCode::from(code.clamp(1, 255) as u8)),
    None => bail!("uv sync was terminated by a signal"),
  }

  if args.no_commit {
    return Ok(ExitCode::SUCCESS);
  }
  if !git::is_git_tracked(&root) {
    println!("Not a git repository; skipping commit");
    return Ok(ExitCode::SUCCESS);
  }
  let paths = ["uv.lock", "pyproject.toml"];
  if !git::is_dirty(&root, &paths)? {
    println!("uv.lock is up to date; nothing to commit");
    return Ok(ExitCode::SUCCESS);
  }
  let owned: Vec<String> = paths.iter().map(|s| s.to_string()).collect();
  match git::commit_paths(&root, &owned, COMMIT_SUBJECT) {
    Ok(hash) => {
      println!("Committed as {hash}.");
      Ok(ExitCode::SUCCESS)
    }
    Err(e) => {
      eprintln!("warning: synced but not committed: {e:#}");
      Ok(ExitCode::from(3))
    }
  }
}

/// Rewrite `pkg`'s requirement in `doc` to the latest stable version on its index.
fn bump_pin(doc: &mut DocumentMut, pkg: &str, index: &dyn IndexClient) -> Result<()> {
  let Some(req) = pyproject::find_requirement(doc, pkg) else {
    println!("No {pkg} pin found in pyproject.toml; skipping pin update");
    return Ok(());
  };
  let simple_url = pyproject::index_url_for(doc, pkg).unwrap_or_else(|| PUBLIC_INDEX.to_string());
  println!("Querying {simple_url} for latest stable {pkg} version...");
  let versions = index.versions(&simple_url, pkg)?;
  let latest = version::latest_stable(versions.iter().map(String::as_str))
    .with_context(|| format!("No stable release versions found for {pkg} on {simple_url}"))?;
  let Some(new_spec) = pyproject::set_requirement_version(&req.spec, &latest) else {
    println!("{pkg} requirement \"{}\" has no version to update; skipping", req.spec);
    return Ok(());
  };
  if new_spec == req.spec {
    println!("{pkg} pin already at {latest}");
  } else {
    pyproject::replace_requirement(doc, &req, &new_spec);
    println!("Updated {pkg} pin to {latest}");
  }
  Ok(())
}

/// `\\?\D:\foo` → `D:\foo` (Windows canonicalize adds the verbatim prefix).
fn strip_verbatim(p: PathBuf) -> PathBuf {
  let s = p.to_string_lossy();
  match s.strip_prefix(r"\\?\") {
    Some(rest) => PathBuf::from(rest),
    None => p,
  }
}
```

Update the `use` block at the top of `lib.rs` to:

```rust
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::{git, pyproject, version};
```

Note: `anyhow::Context::with_context` on an `Option` produces the error message used by the `no_stable_version_is_an_error` test.

- [ ] **Step 5: Run tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-lock 2>&1 | tail -12`
Expected: 8 passed. The `cwd` assertion compares against the canonicalized root; if it fails only on `\\?\` prefixing, compare `calls[0].cwd` against `root.canonicalize()` after stripping the prefix the same way `strip_verbatim` does (copy that helper into the test).

- [ ] **Step 6: Try the dev bin against this repo in dry-run**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo run -q -p aeth-devkit-lock -- --dry-run --package poe-tasks 2>&1 | tail -5`
Expected: `No poe-tasks pin found in pyproject.toml; skipping pin update` then `Would run: uv sync` (this repo has no pin on itself). Exit 0.

- [ ] **Step 7: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "Add aeth-devkit-lock: Rust port of lock.sh with index lookup from pyproject

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Setup command `Args`/`run` and the `devkit` dispatcher

**Files:**
- Create: `crates/aeth-devkit-setup/src/cli.rs`
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (add `pub mod cli;`), `crates/aeth-devkit-setup/src/main.rs` (delegate)
- Create: `crates/aeth-devkit/Cargo.toml`, `crates/aeth-devkit/src/main.rs`

**Interfaces:**
- Produces: `aeth_devkit_setup::cli::{Args, run(&Args) -> Result<ExitCode>}`; binary `devkit` with subcommands `setup-project` and `lock`.

- [ ] **Step 1: Extract setup's CLI into `cli.rs`**

`crates/aeth-devkit-setup/src/cli.rs`:

```rust
//! Command-line surface of `devkit setup-project`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::Parser;

/// Standardize a project's configuration from the templates shipped with aeth-devkit.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-setup", version, about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Directory containing the templates (defaults to the installed aeth_devkit package).
  #[arg(long)]
  pub templates_dir: Option<PathBuf>,

  /// Print the changes that would be made without writing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// Like --dry-run, but exit non-zero if anything would change.
  #[arg(long)]
  pub check: bool,

  /// Do not commit the changes. By default, when the project is git-tracked, the changed
  /// files (never env files) are committed with a standard message.
  #[arg(long)]
  pub no_commit: bool,
}

/// Exit codes: 0 ok, 1 `--check` found drift, 3 applied but commit failed. Errors bubble up
/// for the caller to print (exit 2).
pub fn run(args: &Args) -> Result<ExitCode> {
  let dry_run = args.dry_run || args.check;
  let templates = crate::templates::locate(args.templates_dir.as_deref())?;
  let changes = crate::run(&args.root, &templates, dry_run)?;
  let root = crate::context::strip_verbatim(args.root.canonicalize().unwrap_or(args.root.clone()));
  if changes.is_empty() {
    println!("Nothing to do — project already matches the templates.");
    return Ok(ExitCode::SUCCESS);
  }
  let header = if dry_run { "Would change:" } else { "Changed:" };
  println!("{header}\n{}", changes.report(&root));
  if args.check {
    return Ok(ExitCode::from(1));
  }
  if !dry_run && !args.no_commit && crate::git::is_git_tracked(&root) {
    match crate::git::commit_changes(&root, &changes) {
      Ok(Some(hash)) => println!("Committed as {hash}."),
      Ok(None) => println!("Nothing to commit (only gitignored or env files changed)."),
      Err(e) => {
        eprintln!("warning: changes applied but not committed: {e:#}");
        return Ok(ExitCode::from(3));
      }
    }
  }
  Ok(ExitCode::SUCCESS)
}
```

Add `pub mod cli;` to `crates/aeth-devkit-setup/src/lib.rs`. Replace `crates/aeth-devkit-setup/src/main.rs` with:

```rust
use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  let args = aeth_devkit_setup::cli::Args::parse();
  match aeth_devkit_setup::cli::run(&args) {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
```

- [ ] **Step 2: Verify setup still builds and passes**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-setup 2>&1 | tail -5 && cargo run -q -p aeth-devkit-setup -- --check; echo "exit=$?"`
Expected: tests pass; `--check` on this repo prints `Nothing to do…` with exit 0 (or lists drift with exit 1 — either proves the CLI works).

- [ ] **Step 3: Create the dispatcher crate**

`crates/aeth-devkit/Cargo.toml`:

```toml
[package]
  name    = "aeth-devkit"
  version.workspace = true
  edition.workspace = true
  publish.workspace = true

[[bin]]
  name = "devkit"
  path = "src/main.rs"

[dependencies]
  aeth-devkit-lock  = { workspace = true }
  aeth-devkit-setup = { workspace = true }
  anyhow            = { workspace = true }
  clap              = { workspace = true }
```

`crates/aeth-devkit/src/main.rs`:

```rust
//! `devkit` — project maintenance commands. Each subcommand lives in its own crate; this
//! binary only parses and dispatches.

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "devkit", version, about = "Project maintenance commands")]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
  /// Standardize the project's configuration from the shipped templates.
  SetupProject(aeth_devkit_setup::cli::Args),
  /// Bump the aeth-devkit pin, run `uv sync`, and commit uv.lock.
  Lock(aeth_devkit_lock::Args),
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let result = match &cli.command {
    Command::SetupProject(args) => aeth_devkit_setup::cli::run(args),
    Command::Lock(args) => aeth_devkit_lock::run_real(args),
  };
  match result {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
```

Clap needs `Args` structs used as subcommands to derive `clap::Args`. `Parser` already implies `Args`, so the two command structs work as-is.

- [ ] **Step 4: Build and exercise the dispatcher**

Run:
```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"
cargo build -p aeth-devkit 2>&1 | tail -3
./target/debug/devkit --help
./target/debug/devkit lock --help | head -5
./target/debug/devkit setup-project --check; echo "exit=$?"
```
Expected: help lists `setup-project` and `lock`; `lock --help` shows `--package`, `--no-commit`, `--dry-run`, and `[UV_ARGS]...`; `setup-project --check` behaves as in Step 2.

- [ ] **Step 5: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "Add devkit dispatcher binary with setup-project and lock subcommands

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: Rename the Python package, templates, wheel, and poe tasks

**Files:**
- Move: `python/poe_tasks/` → `python/aeth_devkit/`
- Delete: `python/aeth_devkit/scripts/lock.sh`
- Rename: `python/aeth_devkit/templates/sft.template.gitignore` → `devkit.template.gitignore`
- Modify: `python/aeth_devkit/templates/pyproject.template.toml`, `python/aeth_devkit/__init__.py`, `pyproject.toml`, `.gitignore`, `crates/aeth-devkit-setup/src/templates.rs`, `crates/aeth-devkit-setup/src/lib.rs`, `crates/aeth-devkit-setup/src/toml_merge.rs`, `crates/aeth-devkit-setup/tests/apply.rs`
- Test: `crates/aeth-devkit-setup/tests/apply.rs` (new case for `include_script` replacement)

**Interfaces:**
- Consumes: everything from Tasks 1–8.
- Produces: wheel `aeth-devkit` 7.0.0 shipping `devkit`; poe tasks `setup-project` and `lock` calling it.

- [ ] **Step 1: Move and rename files**

```bash
cd "d:/SFT Software Projects/SFT Workspace/poe_tasks"
git mv python/poe_tasks python/aeth_devkit
git rm -q python/aeth_devkit/scripts/lock.sh
git mv python/aeth_devkit/templates/sft.template.gitignore python/aeth_devkit/templates/devkit.template.gitignore
```

- [ ] **Step 2: Write the failing setup test for `include_script` replacement** — append to `crates/aeth-devkit-setup/tests/apply.rs`:

```rust
#[test]
fn replaces_legacy_poe_tasks_include_script() {
  let dir = make_project();
  let root = dir.path();
  let py = read(root, "pyproject.toml").replace("aeth_devkit:tasks", "poe_tasks:tasks");
  assert!(py.contains("poe_tasks:tasks"), "fixture should start with the legacy include");
  write(root, "pyproject.toml", &py);

  aeth_devkit_setup::run(root, &templates(), false).unwrap();
  let out = read(root, "pyproject.toml");
  assert!(!out.contains("poe_tasks:tasks"), "{out}");
  assert_eq!(out.matches("aeth_devkit:tasks").count(), 1, "{out}");
}
```

Also in `tests/apply.rs` change `templates()` to join `"aeth_devkit"` instead of `"poe_tasks"`. In `tests/fixtures/pyproject.fixture.toml` change the `include_script` line's `poe_tasks:tasks` to `aeth_devkit:tasks` (the test above rewrites it back to the legacy form for its scenario).

- [ ] **Step 3: Run to verify failure**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-setup replaces_legacy 2>&1 | tail -8`
Expected: FAIL — `poe_tasks:tasks` still present (the union keeps both entries). If it fails earlier because the templates dir isn't found, complete Step 4 first and re-run.

- [ ] **Step 4: Update the templates and the setup crate's names**

`python/aeth_devkit/templates/pyproject.template.toml` line 2 → `# setup-project template — merged into each project's pyproject.toml by \`devkit setup-project\`.` and the `[tool.poe]` block →

```toml
[tool.poe]
  include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]
```

`crates/aeth-devkit-setup/src/templates.rs`: in `locate`, replace `SFT_SETUP_TEMPLATES` with `DEVKIT_TEMPLATES` (both occurrences), the dev path segment `"poe_tasks"` with `"aeth_devkit"`, the bail message with `could not locate aeth_devkit templates; pass --templates-dir or set DEVKIT_TEMPLATES`, and in `from_python` the Python snippet with `import aeth_devkit, os; print(os.path.join(os.path.dirname(aeth_devkit.__file__), 'templates'))`. Update the doc comment above `from_python` to say `aeth_devkit`.

`crates/aeth-devkit-setup/src/lib.rs`: in `load_with_rust_overlay` change `layers.push(format!("sft.{name}"));` to `layers.push(format!("devkit.{name}"));` and its doc comment `then the \`sft.<name>\` additions` → `then the \`devkit.<name>\` additions`.

- [ ] **Step 5: Implement the `include_script` replacement in `toml_merge.rs`**

In `merge_value`, before the array union branch, add a dedicated branch. Replace:

```rust
      Some(Item::Value(Value::Array(existing))) if tval.is_array() => {
        let added = if path.starts_with("dependency-groups.") || path == "project.dependencies" {
```

with:

```rust
      Some(Item::Value(Value::Array(existing))) if tval.is_array() => {
        if path == "tool.poe.include_script" {
          let removed = remove_legacy_include_scripts(existing);
          if removed > 0 {
            self.log.push(format!("{path}: removed {removed} legacy poe_tasks include"));
          }
        }
        let added = if path.starts_with("dependency-groups.") || path == "project.dependencies" {
```

and add this free function near `union_array`:

```rust
/// Drop `include_script` entries that point at the pre-rename `poe_tasks:tasks` module so
/// the template's `aeth_devkit:tasks` entry replaces rather than joins them.
fn remove_legacy_include_scripts(existing: &mut Array) -> usize {
  let legacy = |v: &Value| -> bool {
    let script = match v {
      Value::InlineTable(t) => t.get("script").and_then(Value::as_str),
      Value::String(s) => Some(s.value().as_str()),
      _ => None,
    };
    script.is_some_and(|s| s == "poe_tasks:tasks")
  };
  let before = existing.len();
  existing.retain(|v| !legacy(v));
  before - existing.len()
}
```

- [ ] **Step 6: Run the setup tests**

Run: `export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo test -p aeth-devkit-setup 2>&1 | tail -8`
Expected: all pass including `replaces_legacy_poe_tasks_include_script`. If `Array::retain` is unavailable in toml_edit 0.25, rebuild the array: collect the kept `Value`s (cloned) into a `Vec`, `existing.clear()`, then `push_formatted` each.

- [ ] **Step 7: Rewrite `python/aeth_devkit/__init__.py`**

Keep the file as-is except these edits:
- Module docstring: none exists; leave it.
- `release` help: replace `"Bump version, commit, tag, build, and publish to GitHub and SFTPyPI. "` with `"Bump version, commit, tag, build, and publish to GitHub and the package index. "` (same in `release-and-pin`).
- `docker-pin-latest` help: `"(--version or latest stable from SFTPyPI)"` → `"(--version or latest stable release)"`, and `"the latest stable release is fetched from SFTPyPI."` → `"the latest stable release is fetched from the package index."`.
- `rescind-release` help: `"removes the package from SFTPyPI"` → `"removes the package from the package index"`.
- Replace the whole `lock` task with:

```python
tasks.add(
  task_name="lock",
  task_config={
    "help": (
      "Update the aeth-devkit pin in pyproject.toml to the latest stable release on its index, "
      "run uv sync (updating uv.lock), and commit the lockfile and pin change with a "
      "standardized message. Skips the commit if nothing changed. "
      "Pass --upgrade / --all-extras to forward the same flags to uv sync; "
      "other extra args go to `devkit lock` (e.g. --package NAME, --no-commit, --dry-run)."
    ),
    "shell": f'devkit lock $POE_EXTRA_ARGS -- ${{upgrade:+--upgrade}} ${{all_extras:+--all-extras}}',
    "interpreter": "bash",
    "args": [
      {
        "name": "upgrade",
        "options": ["--upgrade", "-U"],
        "type": "boolean",
        "help": "Allow package upgrades, ignoring pinned versions in uv.lock (uv sync --upgrade)",
      },
      {
        "name": "all_extras",
        "options": ["--all-extras"],
        "type": "boolean",
        "help": "Include all optional dependencies (uv sync --all-extras)",
      },
    ],
  },
)
```

- Replace the `setup-project` task with:

```python
tasks.add(
  task_name="setup-project",
  task_config={
    "help": (
      "Standardize this project's configuration from the templates shipped with aeth-devkit "
      "(cache dirs under .cache/, PYTHONPYCACHEPREFIX in .env and VS Code, inlined ruff/pyright "
      "config, .gitignore/.gitattributes/.dockerignore). Idempotent. "
      "Extra args are passed to devkit setup-project: --dry-run, --check, --no-commit, --templates-dir PATH."
    ),
    "cmd": "devkit setup-project",
  },
)
```

- [ ] **Step 8: Update `pyproject.toml`**

Change `[project]`: `name = "aeth-devkit"`, `version = "7.0.0"`, `description = "Personal project-maintenance toolkit: poe tasks plus the devkit CLI"`. Change `[tool.maturin]` to:

```toml
[tool.maturin]
  bindings      = "bin"
  manifest-path = "crates/aeth-devkit/Cargo.toml"
  module-name   = "aeth_devkit"
  python-source = "python"
```

Change `[tool.poe] include_script` script to `"aeth_devkit:tasks"`, `known-first-party = ["aeth_devkit"]`, `source_pkgs = ["aeth_devkit"]`. Leave the `[[tool.uv.index]]` block unchanged.

In `.gitignore` replace the `!python/poe_tasks/templates/**` negation line (search for `poe_tasks`) with `!python/aeth_devkit/templates/**`.

- [ ] **Step 9: Update `docs/specs/2026-08-26-setup-project-design.md`**

Search-and-replace within that file: `sft-setup` → `devkit setup-project`, `SFT_SETUP_TEMPLATES` → `DEVKIT_TEMPLATES`, `poe_tasks` → `aeth_devkit`, `poe-tasks` → `aeth-devkit`, `sft.gitignore` → `devkit.gitignore`, `sft.dockerignore` → `devkit.dockerignore`, and `an SFT project's` → `a project's`. Leave the SFTPyPI index URL text alone.

- [ ] **Step 10: Search for leftovers**

Run: `cd "d:/SFT Software Projects/SFT Workspace/poe_tasks"; grep -rn -i "sft\|poe_tasks\|poe-tasks" --include=*.rs --include=*.py --include=*.toml --include=*.md --include=*.jsonc --include=*.sh . | grep -v "\.venv\|target/\|uv.lock\|Cargo.lock\|sweetfiretobacco\|SFTPyPI\|fixtures/\|remove_legacy\|poe_tasks:tasks\|replaces_legacy"`
Expected: no output except the `.sh` scripts' internal SFTPyPI comments (those scripts are out of scope) and `TODO.md`, which Task 10 updates. Fix anything else that appears.

- [ ] **Step 11: Full workspace test + wheel build**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"
cargo test --workspace 2>&1 | tail -6
cargo clippy --workspace 2>&1 | tail -3
uv sync 2>&1 | tail -3
uv run maturin develop 2>&1 | tail -3
uv run devkit --version
uv run poe setup-project --check; echo "exit=$?"
uv run poe lock --dry-run
```
Expected: tests and clippy clean; `maturin develop` installs `aeth-devkit`; `devkit --version` prints `devkit 7.0.0`; `poe setup-project --check` runs the Rust binary; `poe lock --dry-run` prints `No aeth-devkit pin found in pyproject.toml; skipping pin update` and `Would run: uv sync`.

- [ ] **Step 12: Commit**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"; cargo fmt --all
git add -A
git commit -m "Rename to aeth-devkit: Python package, templates, wheel, and poe tasks now use the devkit CLI

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 10: README, TODO, and final verification

**Files:**
- Modify: `README.md` (currently empty), `TODO.md`

- [ ] **Step 1: Write `README.md`**

```markdown
# aeth-devkit

Personal project-maintenance toolkit: a set of [poe](https://poethepoet.natn.io/) tasks
plus the `devkit` CLI (Rust) they call.

## Commands

| poe task | Backing | What it does |
| --- | --- | --- |
| `poe setup-project` | `devkit setup-project` | Standardize a project's config from the shipped templates (idempotent). |
| `poe lock [-U] [--all-extras]` | `devkit lock` | Bump the `aeth-devkit` pin to the latest stable release on its index, `uv sync`, commit `uv.lock`. |
| `poe release …` | `scripts/release.sh` | Bump version, tag, build, publish. |
| `poe docker-pin-latest` | `scripts/docker-pin-latest.sh` | Pin the compose file's package version. |
| `poe rescind-release` | `scripts/rescind-release.sh` | Undo a release. |

`devkit --help` lists the Rust subcommands. Each lives in its own crate under `crates/`;
`cargo run -p aeth-devkit-lock -- --help` runs one command's dev binary without linking the
others.

## Using it in a project

In `pyproject.toml`:

```toml
[dependency-groups]
  dev = ["aeth-devkit>=7.0.0"]

[tool.uv.sources]
  aeth-devkit = { index = "<your index name>" }

[tool.poe]
  include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]
```

Then `uv sync` and `poe setup-project`.

## Migrating from `poe-tasks`

1. Replace the `poe-tasks` dev dependency with `aeth-devkit>=7.0.0` and rename the
   `tool.uv.sources` key from `poe-tasks` to `aeth-devkit`.
2. `uv sync --upgrade`.
3. `poe setup-project` — it rewrites `include_script` from `poe_tasks:tasks` to
   `aeth_devkit:tasks`.

`poe lock` keeps the pin current from then on. It reads the index URL from
`tool.uv.sources` / `[[tool.uv.index]]`; with no source declared it queries PyPI.

## Development

```
cargo test --workspace
uv run maturin develop     # installs the devkit binary into .venv
```
```

- [ ] **Step 2: Update `TODO.md`**

- Under "Script migration to Rust": change the intro to `Planned order (each command is its own crate under \`crates/\`; the \`devkit\` binary dispatches):` and replace the first item with `- [x] \`lock.sh\` → \`devkit lock\` (7.0.0)`.
- Under "Release / packaging": replace the 6.0.1 item with `- [ ] Release 7.0.0 (\`aeth-devkit\`), then migrate downstream projects per README.` and change `poe-tasks` in the Linux-wheel item to `aeth-devkit`.
- Change the header line `Item tracking for \`poe_tasks\` / \`sft-setup\`.` to `Item tracking for \`aeth-devkit\`.` and replace `sft-setup` with `devkit setup-project` in the Docker item.

- [ ] **Step 3: Final verification**

```bash
export PATH="/c/Users/User/.cargo/bin:$PATH"
cargo test --workspace 2>&1 | tail -4
cargo clippy --workspace -- -D warnings 2>&1 | tail -3
uv run maturin develop 2>&1 | tail -1
uv run poe lock --dry-run
git status --short
```
Expected: tests pass, clippy clean, dry-run output as in Task 9 Step 11, only README/TODO modified.

- [ ] **Step 4: Commit**

```bash
git add README.md TODO.md
git commit -m "Document aeth-devkit usage and downstream migration; update TODO

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## Self-review notes

- Spec coverage: workspace layout (T1, T8), core modules git/process/pyproject/index/version (T2–T6), lock behaviour and exit codes (T7), dispatcher (T8), rename incl. env var, template layers, wheel, poe tasks, include_script replacement (T9), README migration and TODO (T10), testing strategy (each task). `docker-pin-latest.sh` and friends intentionally untouched apart from the move.
- Interfaces: `git::{is_git_tracked, is_dirty, commit_paths, short_head, init_test_repo}`, `process::{Runner, SystemRunner, RecordingRunner, Invocation}`, `pyproject::{find_requirement, set_requirement_version, replace_requirement, index_url_for, normalize_dist_name}`, `index::{IndexClient, HttpIndexClient, StubIndexClient, project_url, versions_from_filenames, parse_simple_json, parse_simple_html}`, `version::latest_stable`, `aeth_devkit_lock::{Args, run, run_real, COMMIT_SUBJECT, DEFAULT_PACKAGE}`, `aeth_devkit_setup::cli::{Args, run}` — names match across tasks.
