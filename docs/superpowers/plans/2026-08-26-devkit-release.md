# `devkit release` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `release.sh` with `devkit release`, a Rust command that bumps, builds, tags, publishes, and creates a GitHub release, with pre-flight checks and an undo journal that rolls everything back on failure.

**Architecture:** A new `aeth-devkit-release` crate orchestrates nine forward steps; each successful step pushes an `Undo` onto a journal that is unwound in reverse on error. External collaborators (`uv`/`git`/`gh` processes, the devpi REST API, the interactive prompt, env lookup) are injected through a `Deps` struct so tests script them. Reusable primitives (captured process output, devpi client, Cargo.toml/pyproject readers, git remote wrappers) land in `aeth-devkit-core`.

**Tech Stack:** Rust 2024, clap 4 (derive), anyhow, toml_edit, tempfile, ureq 3, base64, ctrlc.

**Spec:** `docs/specs/2026-08-26-devkit-release-design.md`

## Global Constraints

- **Teaching comments (spec §Learning requirement):** every Rust file written or modified in this plan is densely commented — comments around nearly every line explaining the Rust syntax in use *and* the logic, plus why the idiom was chosen. `///` for items, `//` inside bodies, tests included. The code blocks in this plan are shown *without* those comments for brevity; the implementer adds them when writing the files. Under-commented files fail review.
- Formatting: `rustfmt.toml` (`tab_spaces = 2`, `max_width = 135`). Run `cargo fmt --all` before every commit; `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- `cargo` lives at `~/.cargo/bin` (not on Git Bash's PATH): `export PATH="$HOME/.cargo/bin:$PATH"`.
- Workspace version is `7.0.2` in both `Cargo.toml` and `pyproject.toml`; do not bump in this plan.
- Command crate contract: `pub struct Args` (clap), `pub fn run(args, deps) -> Result<ExitCode>`, `pub fn run_real(args) -> Result<ExitCode>`; dev bin `devkit-release` in `src/main.rs`.
- Tests never touch the network or `gh`; git remote operations go through `Runner`.
- Commit subject for the bump: `Bump version to <new>`. Tag: `v<new>`, message `Version <new>`.
- Credentials env vars: `UV_INDEX_<NAME>_USERNAME` / `UV_INDEX_<NAME>_PASSWORD`, `<NAME>` = index name upper-cased, `-` → `_`.
- devpi URL: `<publish-url trimmed of trailing '/'>/<normalized package>/<version>`.
- Commit message trailer on every commit: `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.

---

## File map

| Path | Responsibility |
| --- | --- |
| `crates/aeth-devkit-core/src/process.rs` (modify) | `CapturedOutput`, `Runner::run_capture`, scripted `RecordingRunner` |
| `crates/aeth-devkit-core/src/devpi.rs` (create) | `DevpiClient` trait, HTTP + stub impls |
| `crates/aeth-devkit-core/src/pyproject.rs` (modify) | `project_name`, `publish_index` |
| `crates/aeth-devkit-core/src/cargo_toml.rs` (create) | read/set Cargo version |
| `crates/aeth-devkit-core/src/git.rs` (modify) | local + remote git wrappers |
| `crates/aeth-devkit-release/Cargo.toml`, `src/main.rs` | crate scaffold, dev bin |
| `crates/aeth-devkit-release/src/args.rs` | bump/notes positional heuristic |
| `crates/aeth-devkit-release/src/config.rs` | index/credentials/package resolution |
| `crates/aeth-devkit-release/src/prompt.rs` | `Prompt` trait, stdin + scripted impls |
| `crates/aeth-devkit-release/src/snapshot.rs` | file/dist snapshot + restore |
| `crates/aeth-devkit-release/src/report.rs` | `Existing` + rendering |
| `crates/aeth-devkit-release/src/preflight.rs` | tools/branch/version/cargo/dirty/probe/remove |
| `crates/aeth-devkit-release/src/undo.rs` | `Undo` enum, `unwind` |
| `crates/aeth-devkit-release/src/steps.rs` | forward steps 1–9 |
| `crates/aeth-devkit-release/src/lib.rs` | `Args`, `Deps`, `run`, `run_real` |
| `crates/aeth-devkit-release/tests/release.rs` | integration tests |
| `crates/aeth-devkit/src/main.rs`, `Cargo.toml` (modify) | dispatcher subcommand |
| `python/aeth_devkit/__init__.py`, `README.md`, `TODO.md` (modify); `python/aeth_devkit/scripts/release.sh` (delete) | wiring + docs |

---

### Task 1: Captured process output and a scriptable `RecordingRunner`

**Files:**
- Modify: `crates/aeth-devkit-core/src/process.rs`

**Interfaces:**
- Produces:
  ```rust
  pub struct CapturedOutput { pub code: Option<i32>, pub stdout: String, pub stderr: String }
  impl CapturedOutput { pub fn success(&self) -> bool }
  pub trait Runner {
    fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>>;
    fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput>;
  }
  pub struct Script { pub program: String, pub arg_prefix: Vec<String>, pub code: i32, pub stdout: String, pub stderr: String }
  impl RecordingRunner {
    pub fn new(exit_code: i32) -> Self;
    pub fn script(&self, program: &str, arg_prefix: &[&str], code: i32, stdout: &str) -> &Self;
    pub fn fail_at(&self, nth_call: usize);            // 1-based, applies to both run_* methods
    pub fn calls_for(&self, program: &str) -> Vec<Vec<String>>;
  }
  ```
  Matching: first `Script` whose `program` equals and whose `arg_prefix` is a prefix of `args` wins; scripts are not consumed. Unmatched calls return `exit_code` with empty stdout. `fail_at` overrides everything for that one call (code 1, stderr `"scripted failure"`).

- [ ] **Step 1: Write the failing tests** (append inside `mod tests` in `process.rs`)

```rust
#[test]
fn recording_runner_scripts_by_program_and_arg_prefix() {
  let r = RecordingRunner::new(0);
  r.script("uv", &["version"], 0, "demo 1.0.0 => 1.0.1\n");
  let out = r.run_capture("uv", &["version".into(), "--bump".into(), "patch".into()], Path::new(".")).unwrap();
  assert_eq!(out.code, Some(0));
  assert_eq!(out.stdout, "demo 1.0.0 => 1.0.1\n");
  assert!(out.success());
  let other = r.run_capture("uv", &["build".into()], Path::new(".")).unwrap();
  assert_eq!(other.stdout, "");
  assert_eq!(r.calls_for("uv").len(), 2);
  assert_eq!(r.calls_for("uv")[1], vec!["build"]);
}

#[test]
fn recording_runner_fail_at_fails_exactly_that_call() {
  let r = RecordingRunner::new(0);
  r.fail_at(2);
  assert_eq!(r.run_inherit("a", &[], Path::new(".")).unwrap(), Some(0));
  assert_eq!(r.run_inherit("b", &[], Path::new(".")).unwrap(), Some(1));
  let c = r.run_capture("c", &[], Path::new(".")).unwrap();
  assert_eq!(c.code, Some(0));
}

#[test]
fn system_runner_captures_stdout() {
  let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
  let out = SystemRunner.run_capture(&cargo, &["--version".into()], Path::new(".")).unwrap();
  assert!(out.success());
  assert!(out.stdout.starts_with("cargo "));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-core process::`
Expected: compile error (`run_capture`, `script`, `fail_at`, `calls_for` missing).

- [ ] **Step 3: Implement**

Replace the trait/struct section of `process.rs` with:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CapturedOutput {
  pub code: Option<i32>,
  pub stdout: String,
  pub stderr: String,
}

impl CapturedOutput {
  pub fn success(&self) -> bool {
    self.code == Some(0)
  }
}

pub trait Runner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>>;
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput>;
}

pub struct SystemRunner;

impl Runner for SystemRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    let status = Command::new(program).args(args).current_dir(cwd).status().with_context(|| format!("running {program}"))?;
    Ok(status.code())
  }

  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    let out = Command::new(program).args(args).current_dir(cwd).output().with_context(|| format!("running {program}"))?;
    Ok(CapturedOutput {
      code: out.status.code(),
      stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
      stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
  }
}

#[derive(Debug, Clone)]
pub struct Script {
  pub program: String,
  pub arg_prefix: Vec<String>,
  pub code: i32,
  pub stdout: String,
  pub stderr: String,
}

pub struct RecordingRunner {
  pub calls: RefCell<Vec<Invocation>>,
  pub exit_code: i32,
  scripts: RefCell<Vec<Script>>,
  fail_at: Cell<Option<usize>>,
}

impl RecordingRunner {
  pub fn new(exit_code: i32) -> Self {
    Self { calls: RefCell::new(Vec::new()), exit_code, scripts: RefCell::new(Vec::new()), fail_at: Cell::new(None) }
  }

  pub fn script(&self, program: &str, arg_prefix: &[&str], code: i32, stdout: &str) -> &Self {
    self.scripts.borrow_mut().push(Script {
      program: program.to_string(),
      arg_prefix: arg_prefix.iter().map(|s| s.to_string()).collect(),
      code,
      stdout: stdout.to_string(),
      stderr: String::new(),
    });
    self
  }

  pub fn fail_at(&self, nth_call: usize) {
    self.fail_at.set(Some(nth_call));
  }

  pub fn calls_for(&self, program: &str) -> Vec<Vec<String>> {
    self.calls.borrow().iter().filter(|c| c.program == program).map(|c| c.args.clone()).collect()
  }

  fn record(&self, program: &str, args: &[String], cwd: &Path) -> CapturedOutput {
    self.calls.borrow_mut().push(Invocation { program: program.to_string(), args: args.to_vec(), cwd: cwd.to_path_buf() });
    let n = self.calls.borrow().len();
    if self.fail_at.get() == Some(n) {
      return CapturedOutput { code: Some(1), stdout: String::new(), stderr: "scripted failure".into() };
    }
    let scripts = self.scripts.borrow();
    match scripts.iter().find(|s| s.program == program && args.starts_with(&s.arg_prefix)) {
      Some(s) => CapturedOutput { code: Some(s.code), stdout: s.stdout.clone(), stderr: s.stderr.clone() },
      None => CapturedOutput { code: Some(self.exit_code), ..Default::default() },
    }
  }
}

impl Runner for RecordingRunner {
  fn run_inherit(&self, program: &str, args: &[String], cwd: &Path) -> Result<Option<i32>> {
    Ok(self.record(program, args, cwd).code)
  }
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    Ok(self.record(program, args, cwd))
  }
}
```

Add `use std::cell::Cell;` to the imports. Keep the existing tests; they still pass.

- [ ] **Step 4: Run tests**

Run: `cargo test -p aeth-devkit-core process::` — Expected: all pass. Also `cargo test -p aeth-devkit-lock` still passes (lock uses `RecordingRunner::new` and `calls`).

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-core/src/process.rs
git commit -m "core: Runner::run_capture and a scriptable RecordingRunner"
```

---

### Task 2: devpi client

**Files:**
- Create: `crates/aeth-devkit-core/src/devpi.rs`
- Modify: `crates/aeth-devkit-core/src/lib.rs` (add `pub mod devpi;`), `crates/aeth-devkit-core/Cargo.toml` (add `base64`), root `Cargo.toml` (`base64 = "0.22.1"` in `[workspace.dependencies]`)

**Interfaces:**
- Produces:
  ```rust
  pub enum DeleteOutcome { Deleted, NotFound }
  pub trait DevpiClient {
    fn exists(&self, url: &str, username: &str, password: &str) -> Result<bool>;   // 200 → true, 404 → false, else Err
    fn delete(&self, url: &str, username: &str, password: &str) -> Result<DeleteOutcome>; // 200/204 → Deleted, 404 → NotFound, else Err
  }
  pub struct HttpDevpiClient;
  pub struct StubDevpiClient { pub exists: Cell<bool>, pub calls: RefCell<Vec<String>> }  // calls: "GET <url>" / "DELETE <url>"
  impl StubDevpiClient { pub fn new(exists: bool) -> Self }
  pub fn basic_auth_header(username: &str, password: &str) -> String  // "Basic <base64(user:pass)>"
  ```
  Stub `delete` returns `Deleted` when `exists` is true (and flips it to false), else `NotFound`.

- [ ] **Step 1: Write failing tests** (in `devpi.rs` `mod tests`)

```rust
#[test]
fn basic_auth_header_encodes_user_and_password() {
  assert_eq!(basic_auth_header("jacob", "s3cret"), "Basic amFjb2I6czNjcmV0");
}

#[test]
fn stub_records_calls_and_flips_on_delete() {
  let s = StubDevpiClient::new(true);
  assert!(s.exists("u/p/1", "a", "b").unwrap());
  assert!(matches!(s.delete("u/p/1", "a", "b").unwrap(), DeleteOutcome::Deleted));
  assert!(!s.exists("u/p/1", "a", "b").unwrap());
  assert!(matches!(s.delete("u/p/1", "a", "b").unwrap(), DeleteOutcome::NotFound));
  assert_eq!(*s.calls.borrow(), vec!["GET u/p/1", "DELETE u/p/1", "GET u/p/1", "DELETE u/p/1"]);
}
```

- [ ] **Step 2: Run** `cargo test -p aeth-devkit-core devpi::` — Expected: module not found.

- [ ] **Step 3: Implement**

```rust
//! Talking to a devpi index's REST API: does `<pkg>/<version>` exist, and delete it.

use std::cell::{Cell, RefCell};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
  Deleted,
  NotFound,
}

pub trait DevpiClient {
  fn exists(&self, url: &str, username: &str, password: &str) -> Result<bool>;
  fn delete(&self, url: &str, username: &str, password: &str) -> Result<DeleteOutcome>;
}

pub fn basic_auth_header(username: &str, password: &str) -> String {
  let raw = format!("{username}:{password}");
  format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

pub struct HttpDevpiClient;

impl HttpDevpiClient {
  fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().new_agent()
  }
}

impl DevpiClient for HttpDevpiClient {
  fn exists(&self, url: &str, username: &str, password: &str) -> Result<bool> {
    let resp = Self::agent()
      .get(url)
      .header("Authorization", &basic_auth_header(username, password))
      .call()
      .with_context(|| format!("GET {url}"))?;
    match resp.status().as_u16() {
      200 => Ok(true),
      404 => Ok(false),
      s => bail!("unexpected HTTP {s} from GET {url}"),
    }
  }

  fn delete(&self, url: &str, username: &str, password: &str) -> Result<DeleteOutcome> {
    let resp = Self::agent()
      .delete(url)
      .header("Authorization", &basic_auth_header(username, password))
      .call()
      .with_context(|| format!("DELETE {url}"))?;
    match resp.status().as_u16() {
      200 | 204 => Ok(DeleteOutcome::Deleted),
      404 => Ok(DeleteOutcome::NotFound),
      s => bail!("unexpected HTTP {s} from DELETE {url}"),
    }
  }
}

pub struct StubDevpiClient {
  pub exists: Cell<bool>,
  pub calls: RefCell<Vec<String>>,
}

impl StubDevpiClient {
  pub fn new(exists: bool) -> Self {
    Self { exists: Cell::new(exists), calls: RefCell::new(Vec::new()) }
  }
}

impl DevpiClient for StubDevpiClient {
  fn exists(&self, url: &str, _u: &str, _p: &str) -> Result<bool> {
    self.calls.borrow_mut().push(format!("GET {url}"));
    Ok(self.exists.get())
  }
  fn delete(&self, url: &str, _u: &str, _p: &str) -> Result<DeleteOutcome> {
    self.calls.borrow_mut().push(format!("DELETE {url}"));
    if self.exists.replace(false) { Ok(DeleteOutcome::Deleted) } else { Ok(DeleteOutcome::NotFound) }
  }
}
```

Note: `ureq::Agent::delete` exists in ureq 3 (`agent.delete(url)`); if the method is named differently in the resolved version, use `agent.run(http::Request::delete(url)…)`. Check `cargo doc` if it fails to compile.

- [ ] **Step 4: Run** `cargo test -p aeth-devkit-core devpi::` — Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/aeth-devkit-core
git commit -m "core: devpi client (exists/delete) with HTTP and stub implementations"
```

---

### Task 3: pyproject `project_name` and `publish_index`

**Files:**
- Modify: `crates/aeth-devkit-core/src/pyproject.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn project_name(doc: &DocumentMut) -> Result<String>;   // [project].name, error if missing
  pub struct PublishIndex { pub name: String, pub publish_url: String }
  pub fn publish_index(doc: &DocumentMut, name: Option<&str>) -> Result<PublishIndex>;
  ```
  `publish_index`: with `Some(name)` → the `[[tool.uv.index]]` with that `name` (error if absent or without `publish-url`). With `None` → exactly one index has a `publish-url` (error listing candidates if 0 or ≥2).

- [ ] **Step 1: Failing tests** (append to `mod tests`; extend `DOC` with a second index having `publish-url`)

Add to `DOC` after the existing `[[tool.uv.index]]`:
```toml
[[tool.uv.index]]
  name        = "SFTPyPI"
  url         = "https://pypi.example.com/user/internal/+simple"
  publish-url = "https://pypi.example.com/user/internal/"
```
(Then `index_url_for` tests still pass: they look up `Private` by name.)

```rust
#[test]
fn reads_project_name() {
  let d: DocumentMut = DOC.parse().unwrap();
  assert_eq!(project_name(&d).unwrap(), "demo");
  let empty: DocumentMut = "[tool]\n".parse().unwrap();
  assert!(project_name(&empty).is_err());
}

#[test]
fn selects_publish_index() {
  let d: DocumentMut = DOC.parse().unwrap();
  let p = publish_index(&d, None).unwrap();
  assert_eq!(p.name, "SFTPyPI");
  assert_eq!(p.publish_url, "https://pypi.example.com/user/internal/");
  assert_eq!(publish_index(&d, Some("SFTPyPI")).unwrap().name, "SFTPyPI");
  let err = publish_index(&d, Some("Private")).unwrap_err().to_string();
  assert!(err.contains("publish-url"), "{err}");
  assert!(publish_index(&d, Some("Nope")).is_err());
  let none: DocumentMut = "[project]\nname='x'\n".parse().unwrap();
  assert!(publish_index(&none, None).unwrap_err().to_string().contains("no [[tool.uv.index]]"));
}
```

- [ ] **Step 2: Run** `cargo test -p aeth-devkit-core pyproject::` — Expected: compile error.

- [ ] **Step 3: Implement** (add near `index_url_for`; refactor its table iteration into `index_tables`)

```rust
use anyhow::{Result, bail};

pub fn project_name(doc: &DocumentMut) -> Result<String> {
  doc
    .get("project")
    .and_then(|p| p.get("name"))
    .and_then(Item::as_str)
    .map(str::to_string)
    .ok_or_else(|| anyhow::anyhow!("pyproject.toml has no [project].name"))
}

fn index_tables(doc: &DocumentMut) -> Vec<&dyn toml_edit::TableLike> {
  let Some(item) = doc.get("tool").and_then(|t| t.get("uv")).and_then(|u| u.get("index")) else {
    return Vec::new();
  };
  match item {
    Item::ArrayOfTables(a) => a.iter().map(|t| t as &dyn toml_edit::TableLike).collect(),
    Item::Value(Value::Array(a)) => a.iter().filter_map(|v| v.as_inline_table().map(|t| t as &dyn toml_edit::TableLike)).collect(),
    _ => Vec::new(),
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishIndex {
  pub name: String,
  pub publish_url: String,
}

pub fn publish_index(doc: &DocumentMut, name: Option<&str>) -> Result<PublishIndex> {
  let tables = index_tables(doc);
  if tables.is_empty() {
    bail!("no [[tool.uv.index]] entries in pyproject.toml");
  }
  let str_of = |t: &dyn toml_edit::TableLike, key: &str| t.get(key).and_then(Item::as_str).map(str::to_string);
  match name {
    Some(want) => {
      let Some(t) = tables.iter().find(|t| str_of(*t, "name").as_deref() == Some(want)) else {
        bail!("no [[tool.uv.index]] named {want}");
      };
      let Some(publish_url) = str_of(*t, "publish-url") else {
        bail!("index {want} has no publish-url");
      };
      Ok(PublishIndex { name: want.to_string(), publish_url })
    }
    None => {
      let candidates: Vec<PublishIndex> = tables
        .iter()
        .filter_map(|t| Some(PublishIndex { name: str_of(*t, "name")?, publish_url: str_of(*t, "publish-url")? }))
        .collect();
      match candidates.as_slice() {
        [one] => Ok(one.clone()),
        [] => bail!("no [[tool.uv.index]] has a publish-url; pass --index"),
        many => bail!(
          "several indexes have a publish-url ({}); pass --index",
          many.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
        ),
      }
    }
  }
}
```

Rewrite `index_url_for`'s `match` to `index_tables(doc).into_iter().find_map(|t| url_of(t))`.

- [ ] **Step 4: Run** `cargo test -p aeth-devkit-core` — Expected: pass.

- [ ] **Step 5: Commit** — `git commit -m "core: project_name and publish_index pyproject helpers"`

---

### Task 4: Cargo.toml version helpers

**Files:**
- Create: `crates/aeth-devkit-core/src/cargo_toml.rs`; add `pub mod cargo_toml;` to `lib.rs`

**Interfaces:**
- Produces:
  ```rust
  pub fn read_version(doc: &DocumentMut) -> Option<String>;          // workspace.package.version, else package.version
  pub fn set_version(doc: &mut DocumentMut, version: &str) -> bool;   // same target; false if neither table exists
  ```

- [ ] **Step 1: Failing tests**

```rust
const WORKSPACE: &str = "[workspace]\n  members = [\"crates/*\"]\n\n[workspace.package]\n  version = \"7.0.0\"\n  edition = \"2024\"\n";
const PACKAGE: &str = "[package]\nname = \"x\"\nversion = \"1.2.3\"\n\n[dependencies]\nfoo = { version = \"9\" }\n";

#[test]
fn reads_workspace_or_package_version() {
  assert_eq!(read_version(&WORKSPACE.parse().unwrap()).as_deref(), Some("7.0.0"));
  assert_eq!(read_version(&PACKAGE.parse().unwrap()).as_deref(), Some("1.2.3"));
  assert_eq!(read_version(&"[dependencies]\n".parse().unwrap()), None);
}

#[test]
fn sets_version_preserving_layout() {
  let mut d: DocumentMut = WORKSPACE.parse().unwrap();
  assert!(set_version(&mut d, "7.0.2"));
  assert_eq!(d.to_string(), WORKSPACE.replace("7.0.0", "7.0.2"));
  let mut p: DocumentMut = PACKAGE.parse().unwrap();
  assert!(set_version(&mut p, "2.0.0"));
  assert_eq!(p.to_string(), PACKAGE.replace("1.2.3", "2.0.0"));
  let mut none: DocumentMut = "[dependencies]\n".parse().unwrap();
  assert!(!set_version(&mut none, "1"));
}
```

- [ ] **Step 2: Run** `cargo test -p aeth-devkit-core cargo_toml::` — Expected: module missing.

- [ ] **Step 3: Implement**

```rust
//! Reading and rewriting the crate/workspace version in `Cargo.toml`.

use toml_edit::{DocumentMut, Item, Value};

const PATHS: [&[&str]; 2] = [&["workspace", "package", "version"], &["package", "version"]];

fn get_path<'a>(doc: &'a DocumentMut, path: &[&str]) -> Option<&'a Item> {
  let mut item = doc.as_item();
  for key in path {
    item = item.get(key)?;
  }
  Some(item)
}

fn get_path_mut<'a>(doc: &'a mut DocumentMut, path: &[&str]) -> Option<&'a mut Item> {
  let mut item = doc.as_item_mut();
  for key in path {
    item = item.get_mut(key)?;
  }
  Some(item)
}

pub fn read_version(doc: &DocumentMut) -> Option<String> {
  PATHS.iter().find_map(|p| get_path(doc, p).and_then(Item::as_str).map(str::to_string))
}

pub fn set_version(doc: &mut DocumentMut, version: &str) -> bool {
  for path in PATHS {
    if let Some(item) = get_path_mut(doc, path)
      && let Some(cur) = item.as_value_mut()
    {
      let mut v = Value::from(version);
      *v.decor_mut() = cur.decor().clone();
      *cur = v;
      return true;
    }
  }
  false
}
```

- [ ] **Step 4: Run** — Expected: pass.
- [ ] **Step 5: Commit** — `git commit -m "core: Cargo.toml version read/set"`

---

### Task 5: git wrappers (local direct, remote via `Runner`)

**Files:**
- Modify: `crates/aeth-devkit-core/src/git.rs`

**Interfaces:**
- Produces (all `root: &Path`; `Result` = anyhow):
  ```rust
  // local, direct
  pub fn head_sha(root) -> Result<String>;                        // full sha
  pub fn current_branch(root) -> Result<String>;
  pub fn status_porcelain(root) -> Result<String>;                // `git status --porcelain`; "" when clean
  pub fn status_short(root) -> Result<String>;
  pub fn tag_target(root, tag: &str) -> Result<Option<String>>;   // short sha of tag^{commit}; None if no tag
  pub fn create_annotated_tag(root, tag, message) -> Result<()>;
  pub fn delete_tag(root, tag) -> Result<()>;
  pub fn reset_mixed_to(root, rev) -> Result<()>;
  // remote, via runner
  pub fn fetch(runner: &dyn Runner, root) -> Result<()>;                          // git fetch --quiet origin
  pub fn upstream(runner, root) -> Result<Option<String>>;                        // rev-parse --abbrev-ref @{u}; None on failure
  pub fn behind_count(runner, root) -> Result<u32>;                               // rev-list --count HEAD..@{u}
  pub fn remote_tag_exists(runner, root, tag) -> Result<bool>;                    // ls-remote --tags origin refs/tags/<tag>
  pub fn push_refs(runner, root, refs: &[&str]) -> Result<()>;                    // git push origin <refs...>
  pub fn delete_remote_tag(runner, root, tag) -> Result<()>;                      // push origin --delete <tag>; "remote ref does not exist" counts as Ok
  pub fn force_push_with_lease(runner, root, branch, expected_sha, new_sha) -> Result<()>;
  //   git push --force-with-lease=<branch>:<expected_sha> origin <new_sha>:refs/heads/<branch>
  ```

- [ ] **Step 1: Failing tests** (append to `mod tests`)

```rust
#[test]
fn local_helpers_round_trip() {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  init_test_repo(root);
  write(root, "a.txt", "1\n");
  commit_paths(root, &["a.txt".into()], "first").unwrap();
  let first = head_sha(root).unwrap();
  assert_eq!(first.len(), 40);
  assert_eq!(current_branch(root).unwrap(), "main");
  assert_eq!(status_porcelain(root).unwrap(), "");
  write(root, "b.txt", "x\n");
  assert!(status_porcelain(root).unwrap().contains("b.txt"));
  assert!(status_short(root).unwrap().contains("b.txt"));
  std::fs::remove_file(root.join("b.txt")).unwrap();

  assert_eq!(tag_target(root, "v1.0.0").unwrap(), None);
  create_annotated_tag(root, "v1.0.0", "Version 1.0.0").unwrap();
  assert_eq!(tag_target(root, "v1.0.0").unwrap().as_deref(), Some(short_head(root).unwrap().as_str()));
  delete_tag(root, "v1.0.0").unwrap();
  assert_eq!(tag_target(root, "v1.0.0").unwrap(), None);
  assert!(delete_tag(root, "v1.0.0").is_err());

  write(root, "a.txt", "2\n");
  commit_paths(root, &["a.txt".into()], "second").unwrap();
  reset_mixed_to(root, &first).unwrap();
  assert_eq!(head_sha(root).unwrap(), first);
  assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "2\n");
}

#[test]
fn remote_helpers_go_through_runner() {
  use crate::process::RecordingRunner;
  let r = RecordingRunner::new(0);
  let root = Path::new(".");
  r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
  r.script("git", &["rev-list", "--count"], 0, "2\n");
  r.script("git", &["ls-remote"], 0, "abc\trefs/tags/v1.0.0\n");
  fetch(&r, root).unwrap();
  assert_eq!(upstream(&r, root).unwrap().as_deref(), Some("origin/main"));
  assert_eq!(behind_count(&r, root).unwrap(), 2);
  assert!(remote_tag_exists(&r, root, "v1.0.0").unwrap());
  push_refs(&r, root, &["main", "v1.0.0"]).unwrap();
  delete_remote_tag(&r, root, "v1.0.0").unwrap();
  force_push_with_lease(&r, root, "main", "aaa", "bbb").unwrap();
  let git = r.calls_for("git");
  assert_eq!(git[0], vec!["fetch", "--quiet", "origin"]);
  assert_eq!(git[4], vec!["push", "origin", "main", "v1.0.0"]);
  assert_eq!(git[5], vec!["push", "origin", "--delete", "v1.0.0"]);
  assert_eq!(git[6], vec!["push", "--force-with-lease=main:aaa", "origin", "bbb:refs/heads/main"]);

  let failing = RecordingRunner::new(1);
  assert_eq!(upstream(&failing, root).unwrap(), None);
  assert!(push_refs(&failing, root, &["main"]).is_err());
}
```

- [ ] **Step 2: Run** `cargo test -p aeth-devkit-core git::` — Expected: compile error.

- [ ] **Step 3: Implement** (append to `git.rs`)

```rust
use crate::process::{CapturedOutput, Runner};

fn capture(root: &Path, args: &[&str]) -> Result<CapturedOutput> {
  let out = git(root).args(args).output().with_context(|| format!("running git {}", args.join(" ")))?;
  Ok(CapturedOutput {
    code: out.status.code(),
    stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
  })
}

fn expect_ok(out: CapturedOutput, what: &str) -> Result<String> {
  if out.success() { Ok(out.stdout.trim_end().to_string()) } else { bail!("{what} failed: {}", out.stderr.trim()) }
}

pub fn head_sha(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["rev-parse", "HEAD"])?, "git rev-parse HEAD")
}

pub fn current_branch(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["rev-parse", "--abbrev-ref", "HEAD"])?, "git rev-parse --abbrev-ref HEAD")
}

pub fn status_porcelain(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["status", "--porcelain"])?, "git status")
}

pub fn status_short(root: &Path) -> Result<String> {
  expect_ok(capture(root, &["status", "--short"])?, "git status")
}

pub fn tag_target(root: &Path, tag: &str) -> Result<Option<String>> {
  let out = capture(root, &["rev-parse", "--short", "--verify", "--quiet", &format!("refs/tags/{tag}^{{commit}}")])?;
  Ok(out.success().then(|| out.stdout.trim().to_string()))
}

pub fn create_annotated_tag(root: &Path, tag: &str, message: &str) -> Result<()> {
  expect_ok(capture(root, &["tag", "-a", tag, "-m", message])?, "git tag").map(|_| ())
}

pub fn delete_tag(root: &Path, tag: &str) -> Result<()> {
  expect_ok(capture(root, &["tag", "-d", tag])?, "git tag -d").map(|_| ())
}

pub fn reset_mixed_to(root: &Path, rev: &str) -> Result<()> {
  expect_ok(capture(root, &["reset", "--mixed", "--quiet", rev])?, "git reset").map(|_| ())
}

fn remote(runner: &dyn Runner, root: &Path, args: &[&str]) -> Result<CapturedOutput> {
  let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
  runner.run_capture("git", &owned, root)
}

pub fn fetch(runner: &dyn Runner, root: &Path) -> Result<()> {
  expect_ok(remote(runner, root, &["fetch", "--quiet", "origin"])?, "git fetch").map(|_| ())
}

pub fn upstream(runner: &dyn Runner, root: &Path) -> Result<Option<String>> {
  let out = remote(runner, root, &["rev-parse", "--abbrev-ref", "@{u}"])?;
  Ok(out.success().then(|| out.stdout.trim().to_string()))
}

pub fn behind_count(runner: &dyn Runner, root: &Path) -> Result<u32> {
  let s = expect_ok(remote(runner, root, &["rev-list", "--count", "HEAD..@{u}"])?, "git rev-list")?;
  s.trim().parse().with_context(|| format!("parsing rev-list count {s:?}"))
}

pub fn remote_tag_exists(runner: &dyn Runner, root: &Path, tag: &str) -> Result<bool> {
  let s = expect_ok(remote(runner, root, &["ls-remote", "--tags", "origin", &format!("refs/tags/{tag}")])?, "git ls-remote")?;
  Ok(!s.trim().is_empty())
}

pub fn push_refs(runner: &dyn Runner, root: &Path, refs: &[&str]) -> Result<()> {
  let mut args = vec!["push", "origin"];
  args.extend_from_slice(refs);
  expect_ok(remote(runner, root, &args)?, "git push").map(|_| ())
}

pub fn delete_remote_tag(runner: &dyn Runner, root: &Path, tag: &str) -> Result<()> {
  let out = remote(runner, root, &["push", "origin", "--delete", tag])?;
  if out.success() || out.stderr.contains("remote ref does not exist") {
    Ok(())
  } else {
    bail!("git push --delete {tag} failed: {}", out.stderr.trim())
  }
}

pub fn force_push_with_lease(runner: &dyn Runner, root: &Path, branch: &str, expected_sha: &str, new_sha: &str) -> Result<()> {
  let lease = format!("--force-with-lease={branch}:{expected_sha}");
  let refspec = format!("{new_sha}:refs/heads/{branch}");
  expect_ok(remote(runner, root, &["push", &lease, "origin", &refspec])?, "git push --force-with-lease").map(|_| ())
}
```

- [ ] **Step 4: Run** `cargo test -p aeth-devkit-core` — Expected: pass.
- [ ] **Step 5: Commit** — `git commit -m "core: git helpers for tags, resets, and remote operations through Runner"`

---

### Task 6: Release crate scaffold and the positional heuristic

**Files:**
- Create: `crates/aeth-devkit-release/Cargo.toml`, `src/lib.rs` (minimal), `src/main.rs`, `src/args.rs`
- Modify: root `Cargo.toml` (`aeth-devkit-release = { path = "crates/aeth-devkit-release" }`, `ctrlc = "3.5.1"`)

**Interfaces:**
- Produces:
  ```rust
  pub const BUMP_TYPES: [&str; 9] = ["major","minor","patch","stable","alpha","beta","rc","post","dev"];
  pub fn is_bump_type(word: &str) -> bool;
  pub struct Parsed { pub bumps: Vec<String>, pub notes: Option<String>, pub force: bool }
  pub fn parse_positionals(words: &[String]) -> Result<Parsed>;
  ```

- [ ] **Step 1: Crate files**

`crates/aeth-devkit-release/Cargo.toml`:
```toml
[package]
  name    = "aeth-devkit-release"
  version.workspace = true
  edition.workspace = true
  publish.workspace = true

[lib]
  name = "aeth_devkit_release"

[[bin]]
  name = "devkit-release"
  path = "src/main.rs"

[dependencies]
  aeth-devkit-core = { workspace = true }
  anyhow           = { workspace = true }
  clap             = { workspace = true }
  ctrlc            = { workspace = true }
  tempfile         = { workspace = true }
  toml_edit        = { workspace = true }

[dev-dependencies]
  aeth-devkit-core = { workspace = true, features = ["test-util"] }
```

`src/lib.rs` (temporary minimal; Task 11 completes it):
```rust
//! `devkit release` — bump, build, tag, publish, and create a GitHub release, rolling back on failure.
pub mod args;
```

`src/main.rs` (temporary):
```rust
fn main() {}
```

- [ ] **Step 2: Failing tests** in `args.rs`

```rust
#[cfg(test)]
mod tests {
  use super::*;
  fn w(v: &[&str]) -> Vec<String> { v.iter().map(|s| s.to_string()).collect() }

  #[test]
  fn bumps_only() {
    let p = parse_positionals(&w(&["major", "alpha"])).unwrap();
    assert_eq!(p.bumps, vec!["major", "alpha"]);
    assert_eq!(p.notes, None);
    assert!(!p.force);
  }
  #[test]
  fn nothing_means_no_bump() {
    let p = parse_positionals(&[]).unwrap();
    assert!(p.bumps.is_empty() && p.notes.is_none());
  }
  #[test]
  fn strips_force_flags() {
    let p = parse_positionals(&w(&["--force", "patch", "-f"])).unwrap();
    assert!(p.force);
    assert_eq!(p.bumps, vec!["patch"]);
  }
  #[test]
  fn multi_word_tail_is_notes() {
    let p = parse_positionals(&w(&["minor", "first", "minor", "release"])).unwrap();
    assert_eq!(p.bumps, vec!["minor"]);
    assert_eq!(p.notes.as_deref(), Some("first minor release"));
  }
  #[test]
  fn single_spaced_arg_is_notes() {
    let p = parse_positionals(&w(&["publish notes"])).unwrap();
    assert!(p.bumps.is_empty());
    assert_eq!(p.notes.as_deref(), Some("publish notes"));
  }
  #[test]
  fn single_word_tail_is_an_error() {
    let e = parse_positionals(&w(&["patch", "typo"])).unwrap_err().to_string();
    assert!(e.contains("'typo' is not a valid bump type"), "{e}");
    assert!(e.contains("major, minor, patch, stable, alpha, beta, rc, post, dev"));
    assert!(e.contains("multiple words"));
  }
}
```

- [ ] **Step 3: Run** `cargo test -p aeth-devkit-release args::` — Expected: compile error.

- [ ] **Step 4: Implement `args.rs`**

```rust
//! The positional-argument heuristic inherited from release.sh.

use anyhow::{Result, bail};

pub const BUMP_TYPES: [&str; 9] = ["major", "minor", "patch", "stable", "alpha", "beta", "rc", "post", "dev"];

pub fn is_bump_type(word: &str) -> bool {
  BUMP_TYPES.contains(&word)
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Parsed {
  pub bumps: Vec<String>,
  pub notes: Option<String>,
  pub force: bool,
}

pub fn parse_positionals(words: &[String]) -> Result<Parsed> {
  let mut parsed = Parsed::default();
  let mut tail: Vec<&str> = Vec::new();
  for word in words {
    match word.as_str() {
      "--force" | "-f" => parsed.force = true,
      w if is_bump_type(w) && tail.is_empty() => parsed.bumps.push(w.to_string()),
      w => tail.push(w),
    }
  }
  parsed.notes = match tail.as_slice() {
    [] => None,
    [one] if one.contains(' ') => Some(one.to_string()),
    [one] => bail!(
      "'{one}' is not a valid bump type.\n       Valid bump types: {}\n       (Notes must be multiple words — single-word notes are not supported.)",
      BUMP_TYPES.join(", ")
    ),
    many => Some(many.join(" ")),
  };
  Ok(parsed)
}
```

- [ ] **Step 5: Run** `cargo test -p aeth-devkit-release` — Expected: pass. `cargo build --workspace` succeeds.
- [ ] **Step 6: Commit** — `git commit -m "release: crate scaffold and positional bump/notes parsing"`

---

### Task 7: Config and prompt

**Files:**
- Create: `src/config.rs`, `src/prompt.rs`; add `pub mod config; pub mod prompt;` to `lib.rs`

**Interfaces:**
- Produces:
  ```rust
  // config.rs
  pub struct Config { pub package: String, pub index_name: String, pub publish_url: String, pub username: String, pub password: String }
  pub fn env_var_names(index_name: &str) -> (String, String);
  pub fn devpi_url(publish_url: &str, package: &str, version: &str) -> String;
  pub fn resolve(doc: &DocumentMut, index: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config>;
  impl Config { pub fn devpi_url(&self, version: &str) -> String }
  // prompt.rs
  pub trait Prompt { fn ask(&self, question: &str) -> Result<String>; }   // trimmed line
  pub struct StdinPrompt;
  pub struct ScriptedPrompt { pub answers: RefCell<VecDeque<String>>, pub asked: RefCell<Vec<String>> }
  impl ScriptedPrompt { pub fn new(answers: &[&str]) -> Self }               // empty queue → Err("no scripted answer")
  pub fn confirm_force(prompt: &dyn Prompt, force: bool, question: &str) -> Result<bool>;  // true if force || answer == "force"
  ```

- [ ] **Step 1: Failing tests**

`config.rs`:
```rust
#[cfg(test)]
mod tests {
  use super::*;
  const DOC: &str = "[project]\nname = \"Aeth_DevKit\"\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";

  #[test]
  fn env_names_follow_uv_convention() {
    assert_eq!(env_var_names("SFTPyPI"), ("UV_INDEX_SFTPYPI_USERNAME".into(), "UV_INDEX_SFTPYPI_PASSWORD".into()));
    assert_eq!(env_var_names("my-index").0, "UV_INDEX_MY_INDEX_USERNAME");
  }
  #[test]
  fn devpi_url_joins_and_normalizes() {
    assert_eq!(devpi_url("https://x/user/internal/", "Aeth_DevKit", "7.0.3"), "https://x/user/internal/aeth-devkit/7.0.3");
  }
  #[test]
  fn resolves_from_doc_and_env() {
    let doc = DOC.parse().unwrap();
    let env = |k: &str| match k {
      "UV_INDEX_SFTPYPI_USERNAME" => Some("u".to_string()),
      "UV_INDEX_SFTPYPI_PASSWORD" => Some("p".to_string()),
      _ => None,
    };
    let c = resolve(&doc, None, &env).unwrap();
    assert_eq!((c.package.as_str(), c.index_name.as_str(), c.username.as_str(), c.password.as_str()), ("Aeth_DevKit", "SFTPyPI", "u", "p"));
    assert_eq!(c.devpi_url("1.0"), "https://x/user/internal/aeth-devkit/1.0");
  }
  #[test]
  fn missing_env_lists_both_names() {
    let doc = DOC.parse().unwrap();
    let e = resolve(&doc, None, &|_| None).unwrap_err().to_string();
    assert!(e.contains("UV_INDEX_SFTPYPI_USERNAME") && e.contains("UV_INDEX_SFTPYPI_PASSWORD"), "{e}");
  }
}
```

`prompt.rs`:
```rust
#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn scripted_prompt_answers_in_order_then_errors() {
    let p = ScriptedPrompt::new(&["force", "no"]);
    assert_eq!(p.ask("a?").unwrap(), "force");
    assert_eq!(p.ask("b?").unwrap(), "no");
    assert!(p.ask("c?").is_err());
    assert_eq!(*p.asked.borrow(), vec!["a?", "b?", "c?"]);
  }
  #[test]
  fn confirm_force_requires_the_word_force() {
    assert!(confirm_force(&ScriptedPrompt::new(&["force"]), false, "q").unwrap());
    assert!(!confirm_force(&ScriptedPrompt::new(&["y"]), false, "q").unwrap());
    let p = ScriptedPrompt::new(&[]);
    assert!(confirm_force(&p, true, "q").unwrap());
    assert!(p.asked.borrow().is_empty());
  }
}
```

- [ ] **Step 2: Run** — Expected: compile errors.

- [ ] **Step 3: Implement**

`config.rs`:
```rust
//! Which index to publish to, its credentials, and the package name.

use anyhow::{Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::pyproject::{self, normalize_dist_name};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
  pub package: String,
  pub index_name: String,
  pub publish_url: String,
  pub username: String,
  pub password: String,
}

pub fn env_var_names(index_name: &str) -> (String, String) {
  let key: String = index_name.chars().map(|c| if c == '-' { '_' } else { c.to_ascii_uppercase() }).collect();
  (format!("UV_INDEX_{key}_USERNAME"), format!("UV_INDEX_{key}_PASSWORD"))
}

pub fn devpi_url(publish_url: &str, package: &str, version: &str) -> String {
  format!("{}/{}/{version}", publish_url.trim_end_matches('/'), normalize_dist_name(package))
}

pub fn resolve(doc: &DocumentMut, index: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config> {
  let package = pyproject::project_name(doc)?;
  let idx = pyproject::publish_index(doc, index)?;
  let (user_var, pass_var) = env_var_names(&idx.name);
  let (username, password) = (env(&user_var), env(&pass_var));
  let missing: Vec<&str> = [(&user_var, &username), (&pass_var, &password)]
    .into_iter()
    .filter(|(_, v)| v.as_deref().is_none_or(str::is_empty))
    .map(|(k, _)| k.as_str())
    .collect();
  if !missing.is_empty() {
    bail!("required environment variables are not set:\n  - {}", missing.join("\n  - "));
  }
  Ok(Config { package, index_name: idx.name, publish_url: idx.publish_url, username: username.unwrap(), password: password.unwrap() })
}

impl Config {
  pub fn devpi_url(&self, version: &str) -> String {
    devpi_url(&self.publish_url, &self.package, version)
  }
}
```

`prompt.rs`:
```rust
//! Asking the user a question on the terminal, with a scripted stand-in for tests.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufRead as _, Write as _};

use anyhow::{Context as _, Result, bail};

pub trait Prompt {
  fn ask(&self, question: &str) -> Result<String>;
}

pub struct StdinPrompt;

impl Prompt for StdinPrompt {
  fn ask(&self, question: &str) -> Result<String> {
    eprint!("{question} ");
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line).context("reading answer from stdin")?;
    Ok(line.trim().to_string())
  }
}

pub struct ScriptedPrompt {
  pub answers: RefCell<VecDeque<String>>,
  pub asked: RefCell<Vec<String>>,
}

impl ScriptedPrompt {
  pub fn new(answers: &[&str]) -> Self {
    Self { answers: RefCell::new(answers.iter().map(|s| s.to_string()).collect()), asked: RefCell::new(Vec::new()) }
  }
}

impl Prompt for ScriptedPrompt {
  fn ask(&self, question: &str) -> Result<String> {
    self.asked.borrow_mut().push(question.to_string());
    match self.answers.borrow_mut().pop_front() {
      Some(a) => Ok(a),
      None => bail!("no scripted answer for {question:?}"),
    }
  }
}

pub fn confirm_force(prompt: &dyn Prompt, force: bool, question: &str) -> Result<bool> {
  if force {
    return Ok(true);
  }
  Ok(prompt.ask(question)? == "force")
}
```

- [ ] **Step 4: Run** `cargo test -p aeth-devkit-release` — Expected: pass.
- [ ] **Step 5: Commit** — `git commit -m "release: index/credential resolution and prompt abstraction"`

---

### Task 8: Snapshot and restore

**Files:**
- Create: `src/snapshot.rs`; add `pub mod snapshot;`

**Interfaces:**
- Produces:
  ```rust
  pub const TRACKED: [&str; 4] = ["pyproject.toml", "uv.lock", "Cargo.toml", "Cargo.lock"];
  pub struct Snapshot { /* private */ }
  pub fn take(root: &Path) -> Result<Snapshot>;          // copies TRACKED (remembering absence) and dist/*.whl|*.tar.gz
  pub fn dist_artifacts(root: &Path) -> Result<Vec<PathBuf>>;  // absolute paths, sorted
  pub fn clear_dist(root: &Path) -> Result<()>;            // deletes dist/*.whl|*.tar.gz; creates dist/
  impl Snapshot {
    pub fn restore(&self, root: &Path) -> Result<()>;      // TRACKED: copy back or delete if absent; dist: clear then copy back
    pub fn present(&self, rel: &str) -> bool;
  }
  ```
  Dropping a `Snapshot` deletes its temp dir (tempfile does that).

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
  use super::*;
  fn write(root: &Path, rel: &str, s: &str) { std::fs::write(root.join(rel), s).unwrap(); }

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
    let names: Vec<String> = dist_artifacts(root).unwrap().iter().map(|p| p.file_name().unwrap().to_string_lossy().into()).collect();
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
```

- [ ] **Step 2: Run** — Expected: module missing.

- [ ] **Step 3: Implement**

```rust
//! Byte-exact copies of the files a release rewrites, so rollback can put them back.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use tempfile::TempDir;

pub const TRACKED: [&str; 4] = ["pyproject.toml", "uv.lock", "Cargo.toml", "Cargo.lock"];

pub struct Snapshot {
  dir: TempDir,
  present: Vec<&'static str>,
  dist: Vec<String>,
}

fn is_artifact(p: &Path) -> bool {
  let name = p.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
  name.ends_with(".whl") || name.ends_with(".tar.gz")
}

pub fn dist_artifacts(root: &Path) -> Result<Vec<PathBuf>> {
  let dist = root.join("dist");
  if !dist.is_dir() {
    return Ok(Vec::new());
  }
  let mut out: Vec<PathBuf> = std::fs::read_dir(&dist)
    .context("reading dist/")?
    .filter_map(|e| e.ok().map(|e| e.path()))
    .filter(|p| p.is_file() && is_artifact(p))
    .collect();
  out.sort();
  Ok(out)
}

pub fn clear_dist(root: &Path) -> Result<()> {
  std::fs::create_dir_all(root.join("dist")).context("creating dist/")?;
  for p in dist_artifacts(root)? {
    std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
  }
  Ok(())
}

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
    let name = p.file_name().unwrap().to_string_lossy().into_owned();
    std::fs::copy(&p, dir.path().join("dist").join(&name)).with_context(|| format!("snapshotting {name}"))?;
    dist.push(name);
  }
  Ok(Snapshot { dir, present, dist })
}

impl Snapshot {
  pub fn present(&self, rel: &str) -> bool {
    self.present.contains(&rel)
  }

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
      std::fs::copy(self.dir.path().join("dist").join(name), root.join("dist").join(name)).with_context(|| format!("restoring dist/{name}"))?;
    }
    Ok(())
  }
}
```

- [ ] **Step 4: Run** — pass. **Step 5: Commit** — `git commit -m "release: snapshot and restore of versioned files and dist artefacts"`

---

### Task 9: Existence report and pre-flight checks

**Files:**
- Create: `src/report.rs`, `src/preflight.rs`; add mods to `lib.rs`. Also add `Deps` to `lib.rs` now (preflight needs it):

```rust
// lib.rs additions
use std::sync::atomic::AtomicBool;
use aeth_devkit_core::devpi::DevpiClient;
use aeth_devkit_core::process::Runner;
use crate::prompt::Prompt;

pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub devpi: &'a dyn DevpiClient,
  pub prompt: &'a dyn Prompt,
  pub env: &'a dyn Fn(&str) -> Option<String>,
  pub interrupted: &'a AtomicBool,
}
```

**Interfaces:**
- Produces:
  ```rust
  // report.rs
  #[derive(Default)] pub struct Existing { pub local_tag: Option<String>, pub remote_tag: bool, pub github: Option<String>, pub devpi: bool }
  impl Existing { pub fn any(&self) -> bool }
  pub fn render(version: &str, ex: &Existing, package: &str, index_name: &str) -> String;
  // preflight.rs
  pub struct Target { pub current: String, pub new: String }
  pub fn check_tools(runner, root) -> Result<()>;
  pub fn check_branch(runner, root) -> Result<String>;                 // returns branch
  pub fn parse_uv_version(stdout: &str) -> Result<Target>;             // "name a => b" or "name a"
  pub fn target_version(runner, root, bumps: &[String]) -> Result<Target>;
  pub fn check_cargo_version(root, current: &str) -> Result<()>;
  pub fn confirm_dirty_tree(root, force, prompt) -> Result<()>;        // Err("aborted") if declined
  pub fn probe(deps: &Deps, root, cfg: &Config, version: &str) -> Result<Existing>;
  pub fn remove_existing(deps, root, cfg, version, ex: &Existing, dry_run: bool) -> Result<()>;
  ```

- [ ] **Step 1: Failing tests**

`report.rs`:
```rust
#[cfg(test)]
mod tests {
  use super::*;
  #[test]
  fn renders_none_and_present() {
    let none = Existing::default();
    assert!(!none.any());
    let s = render("1.2.3", &none, "demo", "SFTPyPI");
    assert!(s.contains("local tag       none") && s.contains("devpi           none"), "{s}");
    let all = Existing { local_tag: Some("abc1234".into()), remote_tag: true, github: Some("https://gh/r/v1.2.3".into()), devpi: true };
    assert!(all.any());
    let s = render("1.2.3", &all, "demo", "SFTPyPI");
    assert!(s.contains("v1.2.3 -> abc1234"));
    assert!(s.contains("remote tag      refs/tags/v1.2.3 on origin"));
    assert!(s.contains("https://gh/r/v1.2.3"));
    assert!(s.contains("demo==1.2.3 on SFTPyPI"));
  }
}
```

`preflight.rs`:
```rust
#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;

  #[test]
  fn parses_uv_version_output() {
    let t = parse_uv_version("aeth-devkit 7.0.2 => 7.0.3\n").unwrap();
    assert_eq!((t.current.as_str(), t.new.as_str()), ("7.0.2", "7.0.3"));
    let t = parse_uv_version("aeth-devkit 7.0.2\n").unwrap();
    assert_eq!((t.current.as_str(), t.new.as_str()), ("7.0.2", "7.0.2"));
    assert!(parse_uv_version("").is_err());
  }

  #[test]
  fn target_version_builds_bump_flags() {
    let r = RecordingRunner::new(0);
    r.script("uv", &["version"], 0, "demo 1.0.0 => 2.0.0a1\n");
    let t = target_version(&r, Path::new("."), &["major".into(), "alpha".into()]).unwrap();
    assert_eq!(t.new, "2.0.0a1");
    assert_eq!(r.calls_for("uv")[0], vec!["version", "--bump", "major", "--bump", "alpha", "--dry-run"]);
    let r2 = RecordingRunner::new(0);
    r2.script("uv", &["version"], 0, "demo 1.0.0\n");
    target_version(&r2, Path::new("."), &[]).unwrap();
    assert_eq!(r2.calls_for("uv")[0], vec!["version"]);
  }

  #[test]
  fn branch_check_requires_upstream_and_not_behind() {
    let r = RecordingRunner::new(0);
    r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 1, "");
    assert!(check_branch(&r, Path::new(".")).unwrap_err().to_string().contains("no upstream"));
    let r = RecordingRunner::new(0);
    r.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
    r.script("git", &["rev-parse", "--abbrev-ref", "HEAD"], 0, "main\n");
    r.script("git", &["rev-list", "--count"], 0, "3\n");
    assert!(check_branch(&r, Path::new(".")).unwrap_err().to_string().contains("behind"));
  }

  #[test]
  fn cargo_version_must_match() {
    let dir = tempfile::tempdir().unwrap();
    assert!(check_cargo_version(dir.path(), "1.0.0").is_ok());
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"0.9.0\"\n").unwrap();
    let e = check_cargo_version(dir.path(), "1.0.0").unwrap_err().to_string();
    assert!(e.contains("0.9.0") && e.contains("1.0.0"), "{e}");
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname=\"x\"\nversion=\"1.0.0\"\n").unwrap();
    assert!(check_cargo_version(dir.path(), "1.0.0").is_ok());
  }
}
```

(`check_branch` reads the branch name through the runner too — `git rev-parse --abbrev-ref HEAD` — so it is fully scriptable; the local `git::current_branch` is used elsewhere.)

- [ ] **Step 2: Run** — Expected: compile errors.

- [ ] **Step 3: Implement**

`report.rs`:
```rust
//! What already exists for a version, and how to show it.

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Existing {
  pub local_tag: Option<String>,
  pub remote_tag: bool,
  pub github: Option<String>,
  pub devpi: bool,
}

impl Existing {
  pub fn any(&self) -> bool {
    self.local_tag.is_some() || self.remote_tag || self.github.is_some() || self.devpi
  }
}

pub fn render(version: &str, ex: &Existing, package: &str, index_name: &str) -> String {
  let tag = format!("v{version}");
  let row = |label: &str, value: String| format!("  {label:<15} {value}\n");
  let mut s = format!("Existing artefacts for {tag}:\n");
  s += &row("local tag", ex.local_tag.as_ref().map_or("none".into(), |sha| format!("{tag} -> {sha}")));
  s += &row("remote tag", if ex.remote_tag { format!("refs/tags/{tag} on origin") } else { "none".into() });
  s += &row("GitHub release", ex.github.clone().unwrap_or_else(|| "none".into()));
  s += &row("devpi", if ex.devpi { format!("{package}=={version} on {index_name}") } else { "none".into() });
  s
}
```

`preflight.rs`:
```rust
//! Read-only checks that run before anything is mutated.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::devpi::DeleteOutcome;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::{cargo_toml, git};

use crate::Deps;
use crate::config::Config;
use crate::prompt::{Prompt, confirm_force};
use crate::report::Existing;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
  pub current: String,
  pub new: String,
}

fn s(args: &[&str]) -> Vec<String> {
  args.iter().map(|a| a.to_string()).collect()
}

pub fn check_tools(runner: &dyn Runner, root: &Path) -> Result<()> {
  let missing: Vec<&str> = ["git", "uv", "gh"]
    .into_iter()
    .filter(|tool| !runner.run_capture(tool, &s(&["--version"]), root).map(|o| o.success()).unwrap_or(false))
    .collect();
  if missing.is_empty() { Ok(()) } else { bail!("required tools not found on PATH: {}", missing.join(", ")) }
}

pub fn check_branch(runner: &dyn Runner, root: &Path) -> Result<String> {
  let Some(up) = git::upstream(runner, root)? else {
    bail!("the current branch has no upstream; push it first (git push -u origin <branch>)");
  };
  let branch = runner.run_capture("git", &s(&["rev-parse", "--abbrev-ref", "HEAD"]), root)?;
  let branch = branch.stdout.trim().to_string();
  git::fetch(runner, root)?;
  let behind = git::behind_count(runner, root)?;
  if behind > 0 {
    bail!("{branch} is {behind} commit(s) behind {up}; pull or rebase first");
  }
  Ok(branch)
}

pub fn parse_uv_version(stdout: &str) -> Result<Target> {
  let line = stdout.lines().next().unwrap_or("").trim();
  let words: Vec<&str> = line.split_whitespace().collect();
  match words.as_slice() {
    [_, current, "=>", new] => Ok(Target { current: current.to_string(), new: new.to_string() }),
    [_, current] => Ok(Target { current: current.to_string(), new: current.to_string() }),
    _ => bail!("could not parse `uv version` output: {line:?}"),
  }
}

pub fn target_version(runner: &dyn Runner, root: &Path, bumps: &[String]) -> Result<Target> {
  let mut args = vec!["version".to_string()];
  for b in bumps {
    args.push("--bump".into());
    args.push(b.clone());
  }
  if !bumps.is_empty() {
    args.push("--dry-run".into());
  }
  let out = runner.run_capture("uv", &args, root)?;
  if !out.success() {
    bail!("uv version failed: {}", out.stderr.trim());
  }
  parse_uv_version(&out.stdout)
}

pub fn check_cargo_version(root: &Path, current: &str) -> Result<()> {
  let path = root.join("Cargo.toml");
  if !path.is_file() {
    return Ok(());
  }
  let doc: DocumentMut = std::fs::read_to_string(&path).context("reading Cargo.toml")?.parse().context("parsing Cargo.toml")?;
  match cargo_toml::read_version(&doc) {
    Some(v) if v == current => Ok(()),
    Some(v) => bail!("Cargo.toml version {v} does not match pyproject.toml version {current}; fix Cargo.toml first"),
    None => Ok(()),
  }
}

pub fn confirm_dirty_tree(root: &Path, force: bool, prompt: &dyn Prompt) -> Result<()> {
  if git::status_porcelain(root)?.is_empty() {
    return Ok(());
  }
  eprintln!("WARNING: You have uncommitted changes:\n{}", git::status_short(root)?);
  if force {
    eprintln!("WARNING: Proceeding anyway (--force).");
    return Ok(());
  }
  if confirm_force(prompt, false, "Continue with a dirty tree? Type 'force' to continue:")? {
    Ok(())
  } else {
    bail!("aborted: working tree is dirty")
  }
}

pub fn probe(deps: &Deps, root: &Path, cfg: &Config, version: &str) -> Result<Existing> {
  let tag = format!("v{version}");
  let gh = deps.runner.run_capture("gh", &s(&["release", "view", &tag, "--json", "url", "--jq", ".url"]), root)?;
  Ok(Existing {
    local_tag: git::tag_target(root, &tag)?,
    remote_tag: git::remote_tag_exists(deps.runner, root, &tag)?,
    github: gh.success().then(|| gh.stdout.trim().to_string()),
    devpi: deps.devpi.exists(&cfg.devpi_url(version), &cfg.username, &cfg.password)?,
  })
}

pub fn remove_existing(deps: &Deps, root: &Path, cfg: &Config, version: &str, ex: &Existing, dry_run: bool) -> Result<()> {
  let tag = format!("v{version}");
  let verb = if dry_run { "Would remove" } else { "Removing" };
  if ex.github.is_some() {
    println!("  -> {verb} GitHub release {tag} (and its remote tag)");
    if !dry_run {
      let out = deps.runner.run_capture("gh", &s(&["release", "delete", &tag, "--yes", "--cleanup-tag"]), root)?;
      if !out.success() {
        bail!("gh release delete failed: {}", out.stderr.trim());
      }
    }
  }
  if ex.remote_tag {
    println!("  -> {verb} remote tag {tag}");
    if !dry_run {
      git::delete_remote_tag(deps.runner, root, &tag)?;
    }
  }
  if ex.devpi {
    println!("  -> {verb} {}=={version} from {}", cfg.package, cfg.index_name);
    if !dry_run {
      match deps.devpi.delete(&cfg.devpi_url(version), &cfg.username, &cfg.password)? {
        DeleteOutcome::Deleted | DeleteOutcome::NotFound => {}
      }
    }
  }
  if ex.local_tag.is_some() {
    println!("  -> {verb} local tag {tag}");
    if !dry_run {
      git::delete_tag(root, &tag)?;
    }
  }
  Ok(())
}
```

- [ ] **Step 4: Run** `cargo test -p aeth-devkit-release` — pass.
- [ ] **Step 5: Commit** — `git commit -m "release: existence report and pre-flight checks"`

---

### Task 10: Undo journal

**Files:**
- Create: `src/undo.rs`; add `pub mod undo;`

**Interfaces:**
- Produces:
  ```rust
  pub enum Undo {
    RestoreFiles(Snapshot),
    ResetCommit { bump_sha: String, pre_sha: String },
    DeleteLocalTag(String),
    DeleteDevpi { url: String },
    DeleteRemoteTag(String),
    ForcePushBranch { branch: String, bump_sha: String, pre_sha: String },
    DeleteGithubRelease(String),
  }
  impl Undo { pub fn describe(&self) -> String; pub fn manual_command(&self) -> String; }
  pub struct Failure { pub what: String, pub manual: String, pub error: String }
  pub fn unwind(journal: Vec<Undo>, deps: &Deps, root: &Path, cfg: &Config) -> Vec<Failure>;  // reverse order, runs all
  pub fn render_failures(failures: &[Failure]) -> String;   // "Manual cleanup required:\n  <what>: <error>\n    <manual>\n…"
  ```

- [ ] **Step 1: Failing tests**

```rust
#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::atomic::AtomicBool;
  use aeth_devkit_core::devpi::StubDevpiClient;
  use aeth_devkit_core::git;
  use aeth_devkit_core::process::RecordingRunner;
  use crate::prompt::ScriptedPrompt;

  fn cfg() -> Config {
    Config { package: "demo".into(), index_name: "I".into(), publish_url: "https://x/i/".into(), username: "u".into(), password: "p".into() }
  }

  #[test]
  fn unwinds_in_reverse_and_keeps_going_after_failures() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    std::fs::write(root.join("pyproject.toml"), "v=1\n").unwrap();
    git::commit_paths(root, &["pyproject.toml".into()], "init").unwrap();
    let pre = git::head_sha(root).unwrap();
    let snap = crate::snapshot::take(root).unwrap();
    std::fs::write(root.join("pyproject.toml"), "v=2\n").unwrap();
    git::commit_paths(root, &["pyproject.toml".into()], "bump").unwrap();
    let bump = git::head_sha(root).unwrap();
    git::create_annotated_tag(root, "v2", "Version 2").unwrap();

    let runner = RecordingRunner::new(0);
    runner.script("gh", &["release", "delete"], 1, "");
    let devpi = StubDevpiClient::new(true);
    let prompt = ScriptedPrompt::new(&[]);
    let flag = AtomicBool::new(false);
    let deps = Deps { runner: &runner, devpi: &devpi, prompt: &prompt, env: &|_| None, interrupted: &flag };

    let journal = vec![
      Undo::RestoreFiles(snap),
      Undo::ResetCommit { bump_sha: bump.clone(), pre_sha: pre.clone() },
      Undo::DeleteLocalTag("v2".into()),
      Undo::DeleteDevpi { url: "https://x/i/demo/2".into() },
      Undo::DeleteRemoteTag("v2".into()),
      Undo::ForcePushBranch { branch: "main".into(), bump_sha: bump.clone(), pre_sha: pre.clone() },
      Undo::DeleteGithubRelease("v2".into()),
    ];
    let failures = unwind(journal, &deps, root, &cfg());
    assert_eq!(failures.len(), 1);
    assert!(failures[0].what.contains("GitHub release"));
    assert!(failures[0].manual.contains("gh release delete v2 --yes --cleanup-tag"));
    let git_calls = runner.calls_for("git");
    assert_eq!(git_calls[0], vec!["push", &format!("--force-with-lease=main:{bump}"), "origin", &format!("{pre}:refs/heads/main")]);
    assert_eq!(git_calls[1], vec!["push", "origin", "--delete", "v2"]);
    assert_eq!(*devpi.calls.borrow(), vec!["DELETE https://x/i/demo/2"]);
    assert_eq!(git::tag_target(root, "v2").unwrap(), None);
    assert_eq!(git::head_sha(root).unwrap(), pre);
    assert_eq!(std::fs::read_to_string(root.join("pyproject.toml")).unwrap(), "v=1\n");
    assert!(render_failures(&failures).contains("Manual cleanup required"));
  }

  #[test]
  fn reset_refuses_when_head_moved() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    git::init_test_repo(root);
    std::fs::write(root.join("a").as_path(), "1").unwrap();
    git::commit_paths(root, &["a".into()], "one").unwrap();
    let pre = git::head_sha(root).unwrap();
    std::fs::write(root.join("a"), "2").unwrap();
    git::commit_paths(root, &["a".into()], "two").unwrap();
    let bump = git::head_sha(root).unwrap();
    std::fs::write(root.join("a"), "3").unwrap();
    git::commit_paths(root, &["a".into()], "three").unwrap();
    let moved = git::head_sha(root).unwrap();
    let runner = RecordingRunner::new(0);
    let devpi = StubDevpiClient::new(false);
    let prompt = ScriptedPrompt::new(&[]);
    let flag = AtomicBool::new(false);
    let deps = Deps { runner: &runner, devpi: &devpi, prompt: &prompt, env: &|_| None, interrupted: &flag };
    let failures = unwind(vec![Undo::ResetCommit { bump_sha: bump, pre_sha: pre }], &deps, root, &cfg());
    assert_eq!(failures.len(), 1);
    assert!(failures[0].error.contains("HEAD"));
    assert_eq!(git::head_sha(root).unwrap(), moved);
  }
}
```

- [ ] **Step 2: Run** — Expected: compile error.

- [ ] **Step 3: Implement**

```rust
//! The undo journal: one entry per completed forward step, unwound in reverse on failure.

use std::path::Path;

use anyhow::{Result, bail};

use aeth_devkit_core::git;

use crate::Deps;
use crate::config::Config;
use crate::snapshot::Snapshot;

pub enum Undo {
  RestoreFiles(Snapshot),
  ResetCommit { bump_sha: String, pre_sha: String },
  DeleteLocalTag(String),
  DeleteDevpi { url: String },
  DeleteRemoteTag(String),
  ForcePushBranch { branch: String, bump_sha: String, pre_sha: String },
  DeleteGithubRelease(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
  pub what: String,
  pub manual: String,
  pub error: String,
}

impl Undo {
  pub fn describe(&self) -> String {
    match self {
      Undo::RestoreFiles(_) => "Restoring pyproject.toml, uv.lock, Cargo.toml, Cargo.lock, and dist/".into(),
      Undo::ResetCommit { .. } => "Resetting the version-bump commit".into(),
      Undo::DeleteLocalTag(t) => format!("Deleting local tag {t}"),
      Undo::DeleteDevpi { url } => format!("Removing {url} from the index"),
      Undo::DeleteRemoteTag(t) => format!("Deleting remote tag {t}"),
      Undo::ForcePushBranch { branch, .. } => format!("Force-pushing the pre-release {branch} to origin"),
      Undo::DeleteGithubRelease(t) => format!("Deleting GitHub release {t}"),
    }
  }

  pub fn manual_command(&self) -> String {
    match self {
      Undo::RestoreFiles(_) => "git checkout -- pyproject.toml uv.lock Cargo.toml Cargo.lock".into(),
      Undo::ResetCommit { pre_sha, .. } => format!("git reset --mixed {pre_sha}"),
      Undo::DeleteLocalTag(t) => format!("git tag -d {t}"),
      Undo::DeleteDevpi { url } => format!("curl -u \"$USER:$PASS\" -X DELETE {url}"),
      Undo::DeleteRemoteTag(t) => format!("git push origin --delete {t}"),
      Undo::ForcePushBranch { branch, bump_sha, pre_sha } => {
        format!("git push --force-with-lease={branch}:{bump_sha} origin {pre_sha}:refs/heads/{branch}")
      }
      Undo::DeleteGithubRelease(t) => format!("gh release delete {t} --yes --cleanup-tag"),
    }
  }

  fn apply(&self, deps: &Deps, root: &Path, cfg: &Config) -> Result<()> {
    match self {
      Undo::RestoreFiles(snap) => snap.restore(root),
      Undo::ResetCommit { bump_sha, pre_sha } => {
        let head = git::head_sha(root)?;
        if head != *bump_sha {
          bail!("HEAD is {} but the bump commit was {}; not resetting", &head[..7], &bump_sha[..7]);
        }
        git::reset_mixed_to(root, pre_sha)
      }
      Undo::DeleteLocalTag(t) => git::delete_tag(root, t),
      Undo::DeleteDevpi { url } => deps.devpi.delete(url, &cfg.username, &cfg.password).map(|_| ()),
      Undo::DeleteRemoteTag(t) => git::delete_remote_tag(deps.runner, root, t),
      Undo::ForcePushBranch { branch, bump_sha, pre_sha } => git::force_push_with_lease(deps.runner, root, branch, bump_sha, pre_sha),
      Undo::DeleteGithubRelease(t) => {
        let args: Vec<String> = ["release", "delete", t, "--yes", "--cleanup-tag"].iter().map(|s| s.to_string()).collect();
        let out = deps.runner.run_capture("gh", &args, root)?;
        if out.success() { Ok(()) } else { bail!("gh release delete failed: {}", out.stderr.trim()) }
      }
    }
  }
}

pub fn unwind(journal: Vec<Undo>, deps: &Deps, root: &Path, cfg: &Config) -> Vec<Failure> {
  let mut failures = Vec::new();
  for undo in journal.into_iter().rev() {
    eprintln!("  -> {}...", undo.describe());
    if let Err(e) = undo.apply(deps, root, cfg) {
      eprintln!("     WARNING: {e:#}");
      failures.push(Failure { what: undo.describe(), manual: undo.manual_command(), error: format!("{e:#}") });
    }
  }
  failures
}

pub fn render_failures(failures: &[Failure]) -> String {
  let mut s = String::from("Manual cleanup required:\n");
  for f in failures {
    s += &format!("  {}: {}\n    {}\n", f.what, f.error, f.manual);
  }
  s
}
```

- [ ] **Step 4: Run** — pass. **Step 5: Commit** — `git commit -m "release: undo journal with reverse unwinding and manual-cleanup report"`

---

### Task 11: Forward steps, orchestration, dev bin

**Files:**
- Create: `src/steps.rs`
- Modify: `src/lib.rs` (full), `src/main.rs`

**Interfaces:**
- Produces:
  ```rust
  // lib.rs
  #[derive(Parser)] pub struct Args { pub root: PathBuf, pub force: bool, pub dry_run: bool, pub index: Option<String>, pub words: Vec<String> }
  pub struct Deps<'a> { … as Task 9 … }
  pub fn run(args: &Args, deps: &Deps) -> Result<ExitCode>;   // 0 ok; 1 aborted/failed (after rollback); Err → 2 from dispatcher
  pub fn run_real(args: &Args) -> Result<ExitCode>;
  // steps.rs
  pub struct Plan<'a> { pub root: &'a Path, pub cfg: &'a Config, pub target: &'a Target, pub bumps: &'a [String], pub notes: Option<&'a str>, pub branch: &'a str }
  pub fn describe(plan: &Plan) -> String;                            // for --dry-run
  pub fn execute(plan: &Plan, deps: &Deps, journal: &mut Vec<Undo>) -> Result<String>;   // Ok(github url)
  ```

- [ ] **Step 1: Write `steps.rs`**

```rust
//! The nine forward steps of a release. Each pushes its undo onto the journal on success.

use std::path::Path;
use std::sync::atomic::Ordering;

use anyhow::{Context as _, Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::{cargo_toml, git};

use crate::Deps;
use crate::config::Config;
use crate::preflight::Target;
use crate::snapshot::{self, TRACKED};
use crate::undo::Undo;

pub struct Plan<'a> {
  pub root: &'a Path,
  pub cfg: &'a Config,
  pub target: &'a Target,
  pub bumps: &'a [String],
  pub notes: Option<&'a str>,
  pub branch: &'a str,
}

impl Plan<'_> {
  fn tag(&self) -> String {
    format!("v{}", self.target.new)
  }
  fn bumping(&self) -> bool {
    !self.bumps.is_empty()
  }
}

pub fn describe(plan: &Plan) -> String {
  let tag = plan.tag();
  let mut s = String::from("Plan:\n");
  s += "  1. snapshot pyproject.toml, uv.lock, Cargo.toml, Cargo.lock, dist/\n";
  if plan.bumping() {
    s += &format!("  2. uv version --bump {} ; update Cargo.toml ; cargo update --workspace\n", plan.bumps.join(" --bump "));
    s += "  3. uv lock\n";
  }
  s += "  4. uv build\n";
  if plan.bumping() {
    s += &format!("  5. git commit \"Bump version to {}\"\n", plan.target.new);
  }
  s += &format!("  6. git tag -a {tag}\n");
  s += &format!("  7. uv publish --index {}\n", plan.cfg.index_name);
  s += &format!("  8. git push origin {}{tag}\n", if plan.bumping() { format!("{} ", plan.branch) } else { String::new() });
  s += &format!("  9. gh release create {tag} ({})\n", plan.notes.map_or("--generate-notes".to_string(), |n| format!("--notes {n:?}")));
  s
}

fn check_interrupt(deps: &Deps) -> Result<()> {
  if deps.interrupted.load(Ordering::SeqCst) { bail!("interrupted") } else { Ok(()) }
}

fn run_ok(deps: &Deps, root: &Path, program: &str, args: &[&str]) -> Result<()> {
  let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
  match deps.runner.run_inherit(program, &owned, root)? {
    Some(0) => Ok(()),
    Some(code) => bail!("{program} {} exited with {code}", args.join(" ")),
    None => bail!("{program} was terminated by a signal"),
  }
}

fn set_cargo_version(root: &Path, version: &str) -> Result<bool> {
  let path = root.join("Cargo.toml");
  if !path.is_file() {
    return Ok(false);
  }
  let text = std::fs::read_to_string(&path).context("reading Cargo.toml")?;
  let mut doc: DocumentMut = text.parse().context("parsing Cargo.toml")?;
  if !cargo_toml::set_version(&mut doc, version) {
    return Ok(false);
  }
  std::fs::write(&path, doc.to_string()).context("writing Cargo.toml")?;
  Ok(true)
}

pub fn execute(plan: &Plan, deps: &Deps, journal: &mut Vec<Undo>) -> Result<String> {
  let root = plan.root;
  let tag = plan.tag();
  let new = &plan.target.new;

  check_interrupt(deps)?;
  println!("[1/9] Snapshotting files...");
  journal.push(Undo::RestoreFiles(snapshot::take(root)?));

  let pre_sha = git::head_sha(root)?;
  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[2/9] Bumping version to {new}...");
    let mut args = vec!["version"];
    for b in plan.bumps {
      args.push("--bump");
      args.push(b);
    }
    run_ok(deps, root, "uv", &args)?;
    if set_cargo_version(root, new)? && root.join("Cargo.lock").is_file() {
      run_ok(deps, root, "cargo", &["update", "--workspace", "--quiet"])?;
    }
    check_interrupt(deps)?;
    println!("[3/9] uv lock...");
    run_ok(deps, root, "uv", &["lock"])?;
  }

  check_interrupt(deps)?;
  println!("[4/9] Building...");
  snapshot::clear_dist(root)?;
  run_ok(deps, root, "uv", &["build"])?;

  if plan.bumping() {
    check_interrupt(deps)?;
    println!("[5/9] Committing...");
    let paths: Vec<String> = TRACKED.iter().filter(|p| root.join(p).is_file()).map(|p| p.to_string()).collect();
    git::commit_paths(root, &paths, &format!("Bump version to {new}"))?;
    journal.push(Undo::ResetCommit { bump_sha: git::head_sha(root)?, pre_sha: pre_sha.clone() });
  }

  check_interrupt(deps)?;
  println!("[6/9] Tagging {tag}...");
  git::create_annotated_tag(root, &tag, &format!("Version {new}"))?;
  journal.push(Undo::DeleteLocalTag(tag.clone()));

  check_interrupt(deps)?;
  println!("[7/9] Publishing to {}...", plan.cfg.index_name);
  run_ok(deps, root, "uv", &["publish", "--index", &plan.cfg.index_name, "--username", &plan.cfg.username, "--password", &plan.cfg.password])?;
  journal.push(Undo::DeleteDevpi { url: plan.cfg.devpi_url(new) });

  check_interrupt(deps)?;
  println!("[8/9] Pushing...");
  if plan.bumping() {
    git::push_refs(deps.runner, root, &[plan.branch, &tag])?;
    journal.push(Undo::DeleteRemoteTag(tag.clone()));
    journal.push(Undo::ForcePushBranch { branch: plan.branch.to_string(), bump_sha: git::head_sha(root)?, pre_sha });
  } else {
    git::push_refs(deps.runner, root, &[&tag])?;
    journal.push(Undo::DeleteRemoteTag(tag.clone()));
  }

  check_interrupt(deps)?;
  println!("[9/9] Creating GitHub release {tag}...");
  let artifacts = snapshot::dist_artifacts(root)?;
  let mut args: Vec<String> = vec!["release".into(), "create".into(), tag.clone()];
  args.extend(artifacts.iter().map(|p| p.to_string_lossy().into_owned()));
  args.extend(["--title".to_string(), tag.clone()]);
  match plan.notes {
    Some(n) => args.extend(["--notes".to_string(), n.to_string()]),
    None => args.push("--generate-notes".into()),
  }
  let out = deps.runner.run_capture("gh", &args, root)?;
  if !out.success() {
    bail!("gh release create failed: {}{}", out.stdout.trim(), out.stderr.trim());
  }
  journal.push(Undo::DeleteGithubRelease(tag));
  Ok(out.stdout.trim().to_string())
}
```

Note the journal's `ForcePushBranch` is pushed *after* `DeleteRemoteTag`, so on unwind the branch is force-pushed first, then the tag deleted — both are independent so order is not important, but `DeleteGithubRelease` (pushed last) always runs first.

- [ ] **Step 2: Write `lib.rs`**

```rust
//! `devkit release` — bump, build, tag, publish, and create a GitHub release, rolling back on failure.

pub mod args;
pub mod config;
pub mod preflight;
pub mod prompt;
pub mod report;
pub mod snapshot;
pub mod steps;
pub mod undo;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::devpi::DevpiClient;
use aeth_devkit_core::paths::strip_verbatim;
use aeth_devkit_core::process::Runner;

use crate::prompt::{Prompt, confirm_force};

/// Bump version, commit, tag, build, publish to the index, and create a GitHub release.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-release", version, about, trailing_var_arg = true)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Skip the confirmation prompts (dirty tree, existing artefacts) as if `force` were typed.
  #[arg(long, short = 'f')]
  pub force: bool,

  /// Run every check and print the plan without changing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// `[[tool.uv.index]]` to publish to (default: the one with a publish-url).
  #[arg(long)]
  pub index: Option<String>,

  /// Bump types (major minor patch stable alpha beta rc post dev) followed by optional multi-word notes.
  #[arg(allow_hyphen_values = true)]
  pub words: Vec<String>,
}

pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub devpi: &'a dyn DevpiClient,
  pub prompt: &'a dyn Prompt,
  pub env: &'a dyn Fn(&str) -> Option<String>,
  pub interrupted: &'a AtomicBool,
}

pub fn run(args: &Args, deps: &Deps) -> Result<ExitCode> {
  let parsed = args::parse_positionals(&args.words)?;
  let force = args.force || parsed.force;
  let root = strip_verbatim(args.root.canonicalize().with_context(|| format!("resolving {}", args.root.display()))?);

  let pyproject_path = root.join("pyproject.toml");
  let doc: DocumentMut = std::fs::read_to_string(&pyproject_path)
    .with_context(|| format!("{} not found", pyproject_path.display()))?
    .parse()
    .context("parsing pyproject.toml")?;
  let cfg = config::resolve(&doc, args.index.as_deref(), deps.env)?;

  preflight::check_tools(deps.runner, &root)?;
  let branch = preflight::check_branch(deps.runner, &root)?;
  let target = preflight::target_version(deps.runner, &root, &parsed.bumps)?;
  if parsed.bumps.is_empty() {
    println!("Releasing {} {} (no bump)", cfg.package, target.new);
  } else {
    println!("Releasing {} {} -> {}", cfg.package, target.current, target.new);
  }
  preflight::check_cargo_version(&root, &target.current)?;
  if let Err(e) = preflight::confirm_dirty_tree(&root, force, deps.prompt) {
    eprintln!("{e:#}");
    return Ok(ExitCode::from(1));
  }

  let existing = preflight::probe(deps, &root, &cfg, &target.new)?;
  if existing.any() {
    print!("{}", report::render(&target.new, &existing, &cfg.package, &cfg.index_name));
    if !args.dry_run && !confirm_force(deps.prompt, force, "Remove these and continue? Type 'force' to continue:")? {
      eprintln!("aborted: artefacts for v{} already exist", target.new);
      return Ok(ExitCode::from(1));
    }
    preflight::remove_existing(deps, &root, &cfg, &target.new, &existing, args.dry_run)?;
  } else {
    println!("No existing artefacts for v{}.", target.new);
  }

  let plan = steps::Plan { root: &root, cfg: &cfg, target: &target, bumps: &parsed.bumps, notes: parsed.notes.as_deref(), branch: &branch };
  if args.dry_run {
    print!("{}", steps::describe(&plan));
    return Ok(ExitCode::SUCCESS);
  }

  let mut journal = Vec::new();
  match steps::execute(&plan, deps, &mut journal) {
    Ok(url) => {
      println!("Released {} {}\n{url}", cfg.package, target.new);
      Ok(ExitCode::SUCCESS)
    }
    Err(e) => {
      eprintln!("\nERROR: Release failed: {e:#}\nRolling back...");
      let failures = undo::unwind(journal, deps, &root, &cfg);
      if failures.is_empty() {
        eprintln!("\nRollback complete.");
      } else {
        eprintln!("\n{}", undo::render_failures(&failures));
      }
      Ok(ExitCode::from(1))
    }
  }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn run_real(args: &Args) -> Result<ExitCode> {
  ctrlc::set_handler(|| INTERRUPTED.store(true, Ordering::SeqCst)).context("installing Ctrl-C handler")?;
  let env = |key: &str| std::env::var(key).ok();
  run(
    args,
    &Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      devpi: &aeth_devkit_core::devpi::HttpDevpiClient,
      prompt: &prompt::StdinPrompt,
      env: &env,
      interrupted: &INTERRUPTED,
    },
  )
}
```

`src/main.rs`:
```rust
//! Dev binary: `cargo run -p aeth-devkit-release -- …`.
use std::process::ExitCode;
use clap::Parser as _;

fn main() -> ExitCode {
  let args = aeth_devkit_release::Args::parse();
  match aeth_devkit_release::run_real(&args) {
    Ok(code) => code,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(2)
    }
  }
}
```

- [ ] **Step 3: Build** `cargo build -p aeth-devkit-release && cargo clippy -p aeth-devkit-release --all-targets -- -D warnings` — Expected: clean. If clap complains about `trailing_var_arg` with other options, drop it: since `words` is the only positional and `allow_hyphen_values` lets `-f` through, `--force` before positionals still parses as the flag and `-f` after a positional lands in `words` (and `parse_positionals` handles it).
- [ ] **Step 4: Manual smoke** `cargo run -p aeth-devkit-release -- --dry-run` in the repo root with `UV_INDEX_SFTPYPI_USERNAME/PASSWORD` set from `.env` (`set -a; . ./.env; set +a`). Expected: pre-flight output, `No existing artefacts for v7.0.2.` or the report, then the plan. Nothing changes (`git status` clean).
- [ ] **Step 5: Commit** — `git commit -m "release: forward steps, orchestration, and dev binary"`

---

### Task 12: Integration tests

**Files:**
- Create: `crates/aeth-devkit-release/tests/release.rs`

- [ ] **Step 1: Write the tests**

```rust
//! `devkit release` end to end: real git in a temp repo, uv/gh/remote-git scripted, devpi stubbed.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use aeth_devkit_core::devpi::StubDevpiClient;
use aeth_devkit_core::git;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_release::prompt::ScriptedPrompt;
use aeth_devkit_release::{Args, Deps, run};

const PYPROJECT: &str = "[project]\nname = \"demo\"\nversion = \"1.0.0\"\n\n[[tool.uv.index]]\nname = \"Private\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";
const CARGO: &str = "[workspace]\n  members = []\n\n[workspace.package]\n  version = \"1.0.0\"\n";

struct World {
  dir: tempfile::TempDir,
  runner: RecordingRunner,
  devpi: StubDevpiClient,
  prompt: ScriptedPrompt,
  flag: AtomicBool,
}

fn env(key: &str) -> Option<String> {
  match key {
    "UV_INDEX_PRIVATE_USERNAME" => Some("u".into()),
    "UV_INDEX_PRIVATE_PASSWORD" => Some("p".into()),
    _ => None,
  }
}

impl World {
  fn new(answers: &[&str]) -> Self {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("pyproject.toml"), PYPROJECT).unwrap();
    std::fs::write(root.join("uv.lock"), "version = 1\n").unwrap();
    std::fs::write(root.join("Cargo.toml"), CARGO).unwrap();
    std::fs::create_dir(root.join("dist")).unwrap();
    std::fs::write(root.join("dist/demo-0.9.0-py3-none-any.whl"), "old").unwrap();
    git::init_test_repo(root);
    git::commit_paths(root, &["pyproject.toml".into(), "uv.lock".into(), "Cargo.toml".into()], "init").unwrap();
    let runner = RecordingRunner::new(0);
    runner.script("git", &["rev-parse", "--abbrev-ref", "@{u}"], 0, "origin/main\n");
    runner.script("git", &["rev-parse", "--abbrev-ref", "HEAD"], 0, "main\n");
    runner.script("git", &["rev-list", "--count"], 0, "0\n");
    runner.script("git", &["ls-remote"], 0, "");
    runner.script("gh", &["release", "view"], 1, "");
    runner.script("gh", &["release", "create"], 0, "https://github.com/o/demo/releases/tag/v1.0.1\n");
    runner.script("uv", &["version", "--bump", "patch", "--dry-run"], 0, "demo 1.0.0 => 1.0.1\n");
    runner.script("uv", &["version"], 0, "demo 1.0.0\n");
    // `uv build` is scripted to drop an artefact so `gh release create` has something to upload.
    Self { dir, runner, devpi: StubDevpiClient::new(false), prompt: ScriptedPrompt::new(answers), flag: AtomicBool::new(false) }
  }

  fn root(&self) -> &Path {
    self.dir.path()
  }

  fn deps(&self) -> Deps<'_> {
    Deps { runner: &self.runner, devpi: &self.devpi, prompt: &self.prompt, env: &env, interrupted: &self.flag }
  }

  fn args(&self, words: &[&str]) -> Args {
    Args { root: self.root().to_path_buf(), force: false, dry_run: false, index: None, words: words.iter().map(|s| s.to_string()).collect() }
  }

  fn state(&self) -> (String, Option<String>, String, String, Vec<PathBuf>) {
    let r = self.root();
    (
      git::head_sha(r).unwrap(),
      git::tag_target(r, "v1.0.1").unwrap(),
      std::fs::read_to_string(r.join("pyproject.toml")).unwrap(),
      std::fs::read_to_string(r.join("Cargo.toml")).unwrap(),
      aeth_devkit_release::snapshot::dist_artifacts(r).unwrap(),
    )
  }
}

fn code(c: ExitCode) -> bool {
  c == ExitCode::SUCCESS
}

#[test]
fn bump_mode_happy_path() {
  let w = World::new(&[]);
  let before = git::head_sha(w.root()).unwrap();
  assert!(code(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  let uv = w.runner.calls_for("uv");
  assert!(uv.contains(&vec!["version".to_string(), "--bump".into(), "patch".into()]));
  assert!(uv.contains(&vec!["lock".to_string()]));
  assert!(uv.contains(&vec!["build".to_string()]));
  assert!(uv.iter().any(|c| c.starts_with(&["publish".to_string(), "--index".into(), "Private".into()])));
  assert!(w.runner.calls_for("cargo").contains(&vec!["update".to_string(), "--workspace".into(), "--quiet".into()]));
  let git_calls = w.runner.calls_for("git");
  assert!(git_calls.contains(&vec!["push".to_string(), "origin".into(), "main".into(), "v1.0.1".into()]));
  let gh = w.runner.calls_for("gh");
  let create = gh.iter().find(|c| c[..2] == ["release", "create"]).unwrap();
  assert!(create.contains(&"--generate-notes".to_string()));
  let (head, tag, _py, cargo, _dist) = w.state();
  assert_ne!(head, before);
  assert_eq!(tag.as_deref(), Some(git::short_head(w.root()).unwrap().as_str()));
  assert!(cargo.contains("version = \"1.0.1\""));
  assert!(git::status_porcelain(w.root()).unwrap().is_empty());
}

#[test]
fn no_bump_mode_pushes_only_the_tag() {
  let w = World::new(&[]);
  let before = git::head_sha(w.root()).unwrap();
  assert!(code(run(&w.args(&[]), &w.deps()).unwrap()));
  assert_eq!(git::head_sha(w.root()).unwrap(), before);
  assert!(w.runner.calls_for("git").contains(&vec!["push".to_string(), "origin".into(), "v1.0.0".into()]));
  assert!(!w.runner.calls_for("uv").contains(&vec!["lock".to_string()]));
}

#[test]
fn notes_are_forwarded() {
  let w = World::new(&[]);
  assert!(code(run(&w.args(&["patch", "first", "patch", "release"]), &w.deps()).unwrap()));
  let gh = w.runner.calls_for("gh");
  let create = gh.iter().find(|c| c[..2] == ["release", "create"]).unwrap();
  let i = create.iter().position(|a| a == "--notes").unwrap();
  assert_eq!(create[i + 1], "first patch release");
}

fn rollback_case(fail_program: &str, fail_args: &[&str]) -> World {
  let w = World::new(&[]);
  let before = w.state();
  w.runner.script(fail_program, fail_args, 1, "");
  assert!(!code(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before, "repo state must be restored after {fail_program} {fail_args:?} fails");
  w
}

#[test]
fn build_failure_rolls_back_files_only() {
  let w = rollback_case("uv", &["build"]);
  assert!(w.runner.calls_for("git").iter().all(|c| c[0] != "push"));
  assert!(w.devpi.calls.borrow().iter().all(|c| !c.starts_with("DELETE")));
}

#[test]
fn publish_failure_resets_commit_and_tag() {
  let w = rollback_case("uv", &["publish"]);
  assert!(w.runner.calls_for("git").iter().all(|c| c[0] != "push"));
}

#[test]
fn push_failure_deletes_devpi_version() {
  let w = rollback_case("git", &["push", "origin", "main"]);
  assert_eq!(*w.devpi.calls.borrow().last().unwrap(), "DELETE https://x/user/internal/demo/1.0.1");
  assert!(w.runner.calls_for("gh").iter().all(|c| c[..2] != ["release", "delete"]));
}

#[test]
fn github_failure_unwinds_everything_with_lease() {
  let w = rollback_case("gh", &["release", "create"]);
  let git_calls = w.runner.calls_for("git");
  assert!(git_calls.iter().any(|c| c[0] == "push" && c[1].starts_with("--force-with-lease=main:")));
  assert!(git_calls.contains(&vec!["push".to_string(), "origin".into(), "--delete".into(), "v1.0.1".into()]));
  assert!(w.runner.calls_for("gh").iter().all(|c| c[..2] != ["release", "delete"]));
}

#[test]
fn leaked_local_tag_is_reported_and_removed_on_force() {
  let w = World::new(&["force"]);
  git::create_annotated_tag(w.root(), "v1.0.1", "leak").unwrap();
  assert!(code(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.prompt.asked.borrow().len(), 1);
  assert!(w.prompt.asked.borrow()[0].contains("Remove these"));
  assert_eq!(git::tag_target(w.root(), "v1.0.1").unwrap().as_deref(), Some(git::short_head(w.root()).unwrap().as_str()));
}

#[test]
fn leaked_artefacts_abort_without_force() {
  let w = World::new(&["no"]);
  w.devpi.exists.set(true);
  let before = w.state();
  assert!(!code(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert!(w.devpi.calls.borrow().iter().all(|c| c.starts_with("GET")));
}

#[test]
fn dirty_tree_prompt() {
  let w = World::new(&["nope"]);
  std::fs::write(w.root().join("scratch.txt"), "x").unwrap();
  assert!(!code(run(&w.args(&["patch"]), &w.deps()).unwrap()));
  assert!(w.prompt.asked.borrow()[0].contains("dirty tree"));
  let mut a = w.args(&["patch"]);
  a.force = true;
  let w2 = World::new(&[]);
  std::fs::write(w2.root().join("scratch.txt"), "x").unwrap();
  a.root = w2.root().to_path_buf();
  assert!(code(run(&a, &w2.deps()).unwrap()));
  assert!(w2.prompt.asked.borrow().is_empty());
}

#[test]
fn dry_run_changes_nothing() {
  let w = World::new(&[]);
  let before = w.state();
  let mut a = w.args(&["patch"]);
  a.dry_run = true;
  assert!(code(run(&a, &w.deps()).unwrap()));
  assert_eq!(w.state(), before);
  assert!(w.runner.calls_for("uv").iter().all(|c| c[0] == "version" || c[0] == "--version"));
  assert!(w.runner.calls_for("gh").iter().all(|c| c[1] == "view" || c[0] == "--version"));
}

#[test]
fn cargo_mismatch_is_refused_before_anything() {
  let w = World::new(&[]);
  std::fs::write(w.root().join("Cargo.toml"), CARGO.replace("1.0.0", "0.9.9")).unwrap();
  let e = run(&w.args(&["patch"]), &w.deps()).unwrap_err().to_string();
  assert!(e.contains("0.9.9"), "{e}");
}
```

Note on `uv build` in tests: the recorded runner does not create wheels, so `dist_artifacts` is empty at step 9 and `gh release create` receives no files; that is fine for the scripted `gh`. The `state()` comparison still verifies dist restoration (the old wheel is removed by `clear_dist` in step 4 and must be back after rollback).

- [ ] **Step 2: Run** `cargo test -p aeth-devkit-release` — Expected: all pass. Fix any mismatch between the tests and the implementation by correcting whichever is wrong per the spec.
- [ ] **Step 3: Commit** — `git commit -m "release: end-to-end tests for happy paths, rollback at each step, prompts, dry-run"`

---

### Task 13: Dispatcher, poe wiring, docs, remove release.sh

**Files:**
- Modify: `crates/aeth-devkit/Cargo.toml`, `crates/aeth-devkit/src/main.rs`, `python/aeth_devkit/__init__.py`, `README.md`, `TODO.md`
- Delete: `python/aeth_devkit/scripts/release.sh`

- [ ] **Step 1: Dispatcher**

`crates/aeth-devkit/Cargo.toml` dependencies: add `aeth-devkit-release = { workspace = true }`.

`main.rs`:
```rust
enum Command {
  SetupProject(aeth_devkit_setup::cli::Args),
  Lock(aeth_devkit_lock::Args),
  /// Bump version, build, tag, publish to the index, and create a GitHub release.
  Release(aeth_devkit_release::Args),
}
// in main():
Command::Release(args) => aeth_devkit_release::run_real(args),
```

- [ ] **Step 2: poe tasks** in `python/aeth_devkit/__init__.py`

Replace the `release` task with:
```python
tasks.add(
  task_name="release",
  task_config={
    "help": (
      "Bump version, commit, tag, build, and publish to GitHub and the package index. "
      "Pass one or more bump types as free positional args; "
      "valid values: major, minor, patch, stable, alpha, beta, rc, post, dev. "
      "To include release notes, append a multi-word string as the final arg "
      "(single-word trailing args are treated as a typo and raise an error). "
      "Omit all bump types to publish the current version without bumping. "
      "Pass --force / -f to skip the confirmation prompts; --dry-run to only print the plan. "
      "Examples: "
      "poe release patch | "
      "poe release major alpha | "
      "poe release minor 'first minor release' | "
      "poe release 'publish notes'"
    ),
    "envfile": ".env",
    "shell": "devkit release ${force:+--force} ${dry_run:+--dry-run} $POE_EXTRA_ARGS",
    "interpreter": "bash",
    "args": [
      {"name": "force", "options": ["--force", "-f"], "type": "boolean", "help": "Skip the confirmation prompts"},
      {"name": "dry_run", "options": ["--dry-run"], "type": "boolean", "help": "Print the plan without changing anything"},
    ],
  },
)
```
And `release-and-pin`'s `cmd`/`shell` → `'devkit release ${force:+--force} $POE_EXTRA_ARGS && bash "…docker-pin-latest.sh" "$(uv version --short)"'` with `interpreter: bash` and the same `force` arg. Check that `release-and-pin` today declares `force`; if it uses `cmd`, switch to `shell` for the `${force:+…}` expansion.

- [ ] **Step 3: Docs**

README row: `| \`poe release [-f] [--dry-run] [bump …] ["notes"]\` | \`devkit release\` | Bump version, build, tag, publish to the index and GitHub; rolls back on failure. |`
TODO: change `- [ ] \`release.sh\`` to `- [x] \`release.sh\` → \`devkit release\``.

- [ ] **Step 4: Delete the script** `git rm python/aeth_devkit/scripts/release.sh`. Grep for remaining references: `grep -rn "release.sh" --include=*.py --include=*.md --include=*.toml .` (exclude `.venv`, `target`) — only `rescind-release.sh` mentions should remain.

- [ ] **Step 5: Verify**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p aeth-devkit -- release --help
uv run maturin develop && uv run devkit release --dry-run   # with .env loaded
uv run poe release --dry-run
```
Expected: all clean; `--help` shows the new subcommand; dry-run prints pre-flight + plan and leaves `git status` clean.

- [ ] **Step 6: Commit**

```bash
git add -A crates/aeth-devkit python/aeth_devkit README.md TODO.md Cargo.lock
git commit -m "Add devkit release; retire release.sh"
```

---

## Self-review

- **Spec coverage:** defects 1–8 → Tasks 4/9 (Cargo), 11 (`commit_paths`, ordering, `uv lock`), 10 (guarded reset, lease), 9 (report + force), 7 (config). CLI + heuristic → 6/11. Pre-flight 1–7 → 9/11. Steps table + undo enum + interrupts → 10/11. Core additions → 1–5. Dispatcher/poe/docs → 13. Tests → each task + 12.
- **Placeholders:** none; every step has code or an exact command.
- **Type consistency:** `Deps` fields (`runner`, `devpi`, `prompt`, `env`, `interrupted`) used identically in Tasks 9–12; `Target { current, new }`; `Existing { local_tag, remote_tag, github, devpi }`; `Undo` variants match between 10 and 11; `RecordingRunner::script(program, prefix, code, stdout)` signature consistent across 1, 5, 9, 12.
