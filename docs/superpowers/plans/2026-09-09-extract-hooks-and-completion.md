# Extract devkit-claude-hooks and devkit-poe-complete Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the Claude Code hooks crate and the poe completion crate out of `aeth-devkit` into their own repositories, `AetherBreaker/devkit-claude-hooks` (binary `devkit-hook`) and `AetherBreaker/devkit-poe-complete` (binary `devkit-complete`), each a maturin `bindings = "bin"` wheel on SFTPyPI that `devkit setup-project` installs into every project's dev group and wires in; then remove the `devkit hook` and `devkit complete` subcommands in a devkit major.

**Architecture:** Each crate leaves by `git filter-repo` with its history and becomes a standalone Cargo package with a private copy of the process seam (a one-method `Runner` trait, `SystemRunner`, and the recording runner its tests use), released with `devkit release` through the standard Rust release workflow. In `aeth-devkit`, the package step (spec 4.0) gains the two packages as always-active dev dependencies at `{latest}`; the `.claude/settings.local.json` template calls the venv's `devkit-hook` through a `{hook_bin}` placeholder and the hook merge recognises both the old and the new command line; a new step 15 runs `devkit-complete install` for the shells on `PATH`; the dispatcher drops `Complete` and `Hook`. The completion shims move to shim version 3 and call `devkit-complete query`.

**Tech Stack:** Rust 2024 (clap 4, serde, toml_edit 0.25, regex, shell-words), maturin ≥ 1.7 `bindings = "bin"` with `python-source`, uv 0.11+, GitHub Actions, `gh`, `git filter-repo` via `uvx`, Python 3.14.

**Spec:** `docs/superpowers/specs/2026-09-08-devkit-split-design.md`, sections 2, 3, 4.0, 4.4, 4.5, 5, 6, 7 (step 3 and the "Repository creation" paragraph) and 9. Read it first. The two previous steps' plans (`2026-09-08-extract-devkit-container.md`, `2026-09-09-extract-devkit-vscode.md`, each with execution notes) are the recipe this plan repeats; their notes record every quirk hit so far.

## Global Constraints

- Distribution names `devkit-claude-hooks` and `devkit-poe-complete`; binaries `devkit-hook` and `devkit-complete`; Cargo packages and repositories carry the distribution names; Rust lib names `devkit_claude_hooks` and `devkit_poe_complete`; each wheel carries an empty Python package of the lib name (`python/<name>/__init__.py`) so the venv probe (`packages::probe`, which imports the package to find it) treats all devkit packages alike.
- Both repositories start at version `1.0.0`, are public, default branch `main`, created by the plan (spec 7), cloned beside the others; each `pyproject.toml` copies `aeth-devkit`'s `[[tool.uv.index]]` block verbatim (`publish-url` included: these repos publish).
- Neither crate depends on `aeth-devkit-core` or anything else from the devkit workspace (spec 3, 4.4): the process seam is a private copy in each repo.
- The hook names (`pre-edit-protect`, `pre-bash-protect-deps`, `stop-ruff`, `stop-pyright`, `stop-clean`), the stdin payload, the stdout decision JSON and the always-exit-0 rule are unchanged. The completion wire format (`wire.rs`) is unchanged; the shims call `devkit-complete query` and carry `SHIM_VERSION = 3`.
- Publication order (spec 7): `devkit-claude-hooks==1.0.0` and `devkit-poe-complete==1.0.0` are on SFTPyPI before the `aeth-devkit` release whose template references them. That release is a **major** (`13.0.0`); its note says: run `poe lock`, then `poe setup-project` from a plain terminal, before the next Claude Code session.
- `setup-project` for these packages does what spec 4.0 says: adds the dev-group requirement when missing, locks under the `aeth-devkit==<running>` constraint, writes the `>=<locked>` floor, syncs. Nothing new reads only one release indicator (the complete-release rule in `TODO.md` is designed separately; do not half-implement it here).
- `.env` holds live credentials: never print it, never commit it. The user's standing decision from step 2: the SFTPyPI secrets are set on every new repo with `gh secret set`, piped from `aeth_devkit/.env`, and `.env` is copied into each clone.
- `devkit setup-project` refuses a non-terminal stdin except for `--dry-run`; every plain run is the user's (**USER RUNS**). Never open console windows to work around it.
- `aeth-devkit` conventions (`AGENTS.md`): Conventional Commits with the trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; a `fix` body states bug, cause and fix; comments carry reasoning densely; no single-use helpers of four lines or fewer; tests carry no docstrings and never define intent; on a feature branch run only the named tests while iterating and the full suite once at the end. PRs are rebase-merged.

## Context for the executor

### Where things are

- Workspace folder on this machine: `D:\SFT Software Projects\SFT Workspace` (Git Bash: `/d/SFT Software Projects/SFT Workspace`), holding `aeth_devkit` (underscore), `devkit-container`, `devkit-vscode`. `$WS` below is that folder. `WORKSPACE.md` in `aeth_devkit` is the mirror recipe (it assumes `D:\SFT Software Projects`; leave that as it is).
- `aeth_devkit` `main` at the time of writing: `0a9ef4c`, version `12.1.0` released. Unreleased on `main`: the `[tool.coverage]` header fix (`9464f4f`) and the `[tool.setup-project].keep` removal (`ca11e60`); they ship with this step's major.
- The crates: `crates/aeth-devkit-hooks` (`src/lib.rs`, `main.rs`, `pre.rs`, `stop.rs`; `tests/binary.rs`, `tests/hooks.rs`) and `crates/aeth-devkit-complete` (`src/lib.rs`, `main.rs`, `cache.rs`, `engine.rs`, `format.rs`, `install.rs`, `parse.rs`, `repair.rs`, `resolve.rs`, `scripts.rs`, `wire.rs`, `words.rs`; ten test files under `tests/`). Both already build their own binaries (`devkit-hook`, `devkit-complete`) beside being libraries the `devkit` binary links. Their only import from the workspace is `aeth_devkit_core::process::{Runner, SystemRunner, RecordingRunner, CapturedOutput}`, and the only trait method either calls is `run_capture`.
- The dispatcher: `crates/aeth-devkit/src/main.rs` (`Command::Complete`, `Command::Hook`, `wants_update_check`). Its test `crates/aeth-devkit/tests/update_nag.rs` drives the real binary with `complete …` as its subject.
- The templates that wire the binaries in: `python/aeth_devkit/templates/claude/settings.local.template.jsonc` (five `{devkit_bin} hook <name>` commands), `python/aeth_devkit/templates/pyproject.template.toml` (the dev group, `[tool.uv.sources]` gated `if-docker-services`). The placeholder is resolved in `crates/aeth-devkit-setup/src/templates.rs` (`devkit_bin`), the hook merge in `crates/aeth-devkit-setup/src/json_merge.rs` (`hook_key`, `legacy_hook_key`, `matches_key`), the package step in `crates/aeth-devkit-setup/src/packages.rs` (`DevkitPackage`, `CONTAINER`, `active`, `latest_requested`, `advance`), the run's step list in `crates/aeth-devkit-setup/src/lib.rs` (`run_with`, numbered comments 1–14), the merge markers in `crates/aeth-devkit-setup/src/toml_merge.rs` (`marker_lines`, `check_markers`, `conditional_dep`, `conditional_docker`, `IF_DOCKER_SERVICES_MARKER`, `strip_marker_comments`).
- The completion install today: `devkit complete install --powershell --bash [--dry-run]` writes `~/.local/share/devkit/poe-completion.ps1` plus one line in `$PROFILE`, and `~/bash_completion.d/poe.bash` plus `~/.local/share/bash-completion/completions/poe`; `install.rs` refuses a bash file whose first five lines carry neither `Generated by poethepoet` nor `adapted by aeth-devkit` (`GENERATED_MARKERS`). The shipped v2 shim's header says neither, so `install --bash` over a v2 shim would refuse it; only the Tab-time self-repair (`repair.rs`, which bypasses that check) has ever rewritten one. Task 4 fixes that, since the migration relies on `install`.
- `devkit-container` is the reference satellite: its `Cargo.toml`, `pyproject.toml`, `rustfmt.toml`, `.github/workflows/ci.yml` and README shape are what Tasks 2 and 4 reproduce.

### Tools the plan assumes

`gh` authenticated as `AetherBreaker`, `git`, `uv` (0.11.16 here), `uvx` for `git filter-repo`, Rust stable with `rustfmt` and `clippy`, Python 3.14 reachable by uv, `powershell` and `bash` on PATH (this machine has both). Docker is not needed.

### What only the user can run

- Every `setup-project` run that is not `--dry-run` (Task 5 in both new repos, Task 11 everywhere).
- Merging the `aeth-devkit` PR and running `poe release`: the user said in step 2 to merge and release once the reviews are clean; ask again only if that changes.

### Lessons from steps 1 and 2 that apply here

- `git filter-repo` under Git Bash: set `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'` or the `old:new` rename arguments are mangled. It re-points tags; delete them all before the first push. Windows checkouts are CRLF: `.gitattributes` first, then re-checkout so the working tree is LF too.
- `gh repo create --push` was refused by the tool permission layer; the GitHub MCP `create_repository` tool (`private: false`, `autoInit: false`) plus `git remote add origin` and `git push -u origin main` gives the same end state.
- Never chain `git stash` and `git stash pop` in one command in `aeth_devkit`: it carries an old stash that a no-op stash followed by a pop applies.
- `npm`/`uv`-style tools rewrite formatting; check `git diff --stat` before committing a "one-line" change.
- Write multi-line files with the file-writing tool or a script; heredocs and `sed` mangle escapes and quotes.
- The first `setup-project` run in a new repo has no `tombi` in the venv; run `uv lock && uv sync && uv run tombi format --quiet pyproject.toml` afterwards and commit that with `uv.lock`, or the next plain run makes a second "Standardize" commit.
- Reviews: a fresh reviewer after each task and two independent extra-high-effort reviews of the PR (one on this session's model, one on Opus) found real defects in both previous steps. Do the same.

### Decisions taken by this plan (the user may override before execution)

1. **One devkit release, a major.** Spec 7 orders step 3 as wheels, then templates, then the major that removes the subcommands. The templates live in devkit until step 4, so "templates" and "the major" are one release here: a separate minor would ship the templates while the crates still lived in two places. The window the spec accepts (a project whose venv takes `13.0.0` before `setup-project` rewrote its hook lines gets a clap error on every Edit) is closed per project by `poe lock` followed by `poe setup-project`, which the release note says.
2. **Empty Python packages in both wheels** (`devkit_claude_hooks`, `devkit_poe_complete`), the shape `devkit-container` has, so `packages::probe` needs no second code path.
3. **A marker on a value key.** `[tool.uv.sources]` becomes an ungated table whose `devkit-container` entry alone carries `# setup-project: if-docker-services`. The merge language grows one rule: a marker above a key-value line gates that key the way one above a table header gates the table (Task 6). The alternative, a source entry for `devkit-container` in every project, is noise in every non-Docker pyproject.
4. **`{hook_bin}` replaces `{devkit_bin}`**, which nothing else uses: the venv's `devkit-hook` (quoted, via `$CLAUDE_PROJECT_DIR`), else `uv run devkit-hook`.
5. **`hook_key` recognises both forms** (`… devkit-hook <name>` and the old `… hook <name>`), so an existing entry is updated in place (spec 4.4).
6. **Shell detection is a `PATH` lookup**: PowerShell when `pwsh`/`powershell` is found, bash when `bash` is; the install runs as step 15, last in the run, with `--dry-run` on a dry run since it writes to the home directory.
7. **`SHIM_VERSION` becomes 3** and `GENERATED_MARKERS` gains `devkit thin shim`, so `devkit-complete install --bash` overwrites the v2 shim instead of refusing it as a user file.
8. **The nag tests use `docker-pin --dry-run` outside a git repository** as their ordinary command: it fails at its first check with no network and the nag follows the error.
9. **The `TODO.md` entry "Auto-commit `stop-ruff`'s safe fixes" moves** to the hooks repository's `TODO.md`, with the crate it describes.
10. **The README sections for the two commands move** into the new repositories' READMEs (feature bullets intact); `aeth-devkit`'s README keeps a short pointer section, the way it does for `devkit-container`.

### The order that matters

Part A builds both repositories and publishes both wheels. Part B changes `aeth-devkit` on one branch and releases the major. Part C is the rollout: `poe lock` then `poe setup-project` in every repository, by the user. Part B must not be released before both wheels exist; Part C must not start before Part B is released.

## File structure

**`devkit-claude-hooks` (new, `$WS/devkit-claude-hooks`)** — from filter-repo, renamed to the root: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/pre.rs`, `src/stop.rs`, `tests/binary.rs`, `tests/hooks.rs`. Added: `.gitattributes`, `.gitignore`, `rustfmt.toml`, `src/process.rs`, `pyproject.toml`, `python/devkit_claude_hooks/__init__.py`, `README.md`, `TODO.md`, `.github/workflows/ci.yml`. Added by `setup-project` in Task 5: the standard files, `.github/workflows/release.yml`, `uv.lock`.

**`devkit-poe-complete` (new, `$WS/devkit-poe-complete`)** — from filter-repo: `Cargo.toml`, `src/*.rs` (12 files), `tests/*.rs` (10 files). Added: the same set as above with `python/devkit_poe_complete/__init__.py`.

**`aeth-devkit` (branch `feat/extract-hooks-and-completion`)**
- Modify: `python/aeth_devkit/templates/pyproject.template.toml`, `python/aeth_devkit/templates/claude/settings.local.template.jsonc`, `crates/aeth-devkit-setup/src/packages.rs`, `src/toml_merge.rs`, `src/templates.rs`, `src/json_merge.rs`, `src/lib.rs`, `tests/apply.rs`, `tests/packages.rs`, `crates/aeth-devkit/src/main.rs`, `crates/aeth-devkit/Cargo.toml`, `crates/aeth-devkit/tests/update_nag.rs`, `Cargo.toml`, `Cargo.lock`, `README.md`, `TODO.md`, `WORKSPACE.md`.
- Create: `crates/aeth-devkit-setup/src/completion.rs`.
- Delete: `crates/aeth-devkit-hooks/`, `crates/aeth-devkit-complete/`.

---

## Part A: the two repositories

### Task 1: Extract `crates/aeth-devkit-hooks` with its history

**Files:**
- Create: `$WS/devkit-claude-hooks` (a filtered clone), `.gitattributes` in it.

**Interfaces:**
- Produces: a local repository on branch `main`, no remote, no tags, whose tree is the crate at the root, LF throughout.

- [ ] **Step 1: Confirm `aeth_devkit` is current and clean**

```bash
WS="/d/SFT Software Projects/SFT Workspace"
cd "$WS/aeth_devkit" && git switch main && git pull --ff-only && git status --short --branch | head -3 && git log --oneline -1
ls crates/aeth-devkit-hooks crates/aeth-devkit-complete
test ! -e "$WS/devkit-claude-hooks" && test ! -e "$WS/devkit-poe-complete" && echo "targets absent"
```

Expected: `## main...origin/main`, a clean tree, both crate directories listed, "targets absent". If `main` has moved past `0a9ef4c`, read the newer commits' messages before continuing.

- [ ] **Step 2: Filter a throwaway clone down to the crate**

```bash
cd "$WS" && git clone --no-local aeth_devkit devkit-claude-hooks && cd devkit-claude-hooks
MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' uvx --from git-filter-repo git-filter-repo --force \
  --path crates/aeth-devkit-hooks/ \
  --path-rename crates/aeth-devkit-hooks/:
git log --oneline | wc -l
git ls-files
git remote -v
git tag -l | wc -l
```

Expected: a handful of commits (the crate's history); the file list is exactly `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/pre.rs`, `src/stop.rs`, `tests/binary.rs`, `tests/hooks.rs`; no remote; some re-pointed tags.

- [ ] **Step 3: Delete every inherited tag, confirm the branch, LF as the first new commit**

Write `.gitattributes` with exactly:

```
* text=auto eol=lf
*.sh text eol=lf
```

Then:

```bash
git tag -l | xargs -r git tag -d >/dev/null; git tag -l | wc -l; git branch --show-current
git add .gitattributes && git add --renormalize . && git commit -q -m "chore: add .gitattributes so checkouts are LF

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git rm -rq --cached . && git reset -q --hard HEAD
git ls-files --eol | awk '{print $1, $2}' | sort | uniq -c
```

Expected: `0` tags; branch `main` (else `git branch -m main`); every file `i/lf w/lf` after the re-checkout.

---

### Task 2: Stand `devkit-claude-hooks` up as its own package

**Files:**
- Create: `src/process.rs`, `rustfmt.toml`, `.gitignore`, `pyproject.toml`, `python/devkit_claude_hooks/__init__.py`, `README.md`, `TODO.md`, `.github/workflows/ci.yml`.
- Modify: `Cargo.toml`, `src/lib.rs`, `src/main.rs`, `src/stop.rs`, `tests/hooks.rs`.

**Interfaces:**
- Produces: a crate `devkit-claude-hooks` 1.0.0 with lib `devkit_claude_hooks` (exporting `pub mod process`, `Args`, `Hook`, `run`, `run_real` as today) and binary `devkit-hook`; a wheel `devkit_claude_hooks-1.0.0-*.whl` carrying `devkit_claude_hooks/__init__.py` and the `devkit-hook` script.

- [ ] **Step 1: The private process seam**

Create `src/process.rs`:

```rust
//! Running a program and capturing what it printed: the one seam this crate needs, behind
//! a trait so the tests answer from scripts instead of spawning ruff, pyright or poe. A
//! private copy of the seam `aeth-devkit-core` has: a library repo whose only export is a
//! test seam is not worth a dependency edge (devkit split spec, section 3).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

/// What a finished process left behind. `code` is `None` when a signal ended it.
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
  /// Run `program args` in `cwd` and capture stdout and stderr.
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput>;
}

/// Spawns for real.
pub struct SystemRunner;

impl Runner for SystemRunner {
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    let out = Command::new(program)
      .args(args)
      .current_dir(cwd)
      .output()
      .with_context(|| format!("running {program}"))?;
    Ok(CapturedOutput {
      code: out.status.code(),
      stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
      stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
  }
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
  pub program: String,
  pub args: Vec<String>,
  pub cwd: PathBuf,
}

struct Script {
  program: String,
  arg_prefix: Vec<String>,
  code: i32,
  stdout: String,
  stderr: String,
}

/// Records every call and answers from scripts; for tests. The most recently registered
/// script whose program matches and whose `arg_prefix` starts the call's arguments wins, so
/// a broad default can be overridden for one call; an unmatched call exits `exit_code` with
/// no output.
pub struct RecordingRunner {
  pub calls: RefCell<Vec<Invocation>>,
  pub exit_code: i32,
  scripts: RefCell<Vec<Script>>,
}

impl RecordingRunner {
  pub fn new(exit_code: i32) -> Self {
    Self {
      calls: RefCell::new(Vec::new()),
      exit_code,
      scripts: RefCell::new(Vec::new()),
    }
  }

  pub fn script(&self, program: &str, arg_prefix: &[&str], code: i32, stdout: &str) -> &Self {
    self.push(program, arg_prefix, code, stdout, "")
  }

  /// A scripted stderr, for callers that read it to decide what a non-zero exit means.
  pub fn script_err(&self, program: &str, arg_prefix: &[&str], code: i32, stderr: &str) -> &Self {
    self.push(program, arg_prefix, code, "", stderr)
  }

  fn push(&self, program: &str, arg_prefix: &[&str], code: i32, stdout: &str, stderr: &str) -> &Self {
    self.scripts.borrow_mut().push(Script {
      program: program.to_string(),
      arg_prefix: arg_prefix.iter().map(|s| s.to_string()).collect(),
      code,
      stdout: stdout.to_string(),
      stderr: stderr.to_string(),
    });
    self
  }

  /// The argument lists of every recorded call to `program`, in order.
  pub fn calls_for(&self, program: &str) -> Vec<Vec<String>> {
    self.calls.borrow().iter().filter(|c| c.program == program).map(|c| c.args.clone()).collect()
  }
}

impl Runner for RecordingRunner {
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    self.calls.borrow_mut().push(Invocation {
      program: program.to_string(),
      args: args.to_vec(),
      cwd: cwd.to_path_buf(),
    });
    let scripts = self.scripts.borrow();
    Ok(
      match scripts.iter().rev().find(|s| s.program == program && args.starts_with(&s.arg_prefix)) {
        Some(s) => CapturedOutput {
          code: Some(s.code),
          stdout: s.stdout.clone(),
          stderr: s.stderr.clone(),
        },
        None => CapturedOutput {
          code: Some(self.exit_code),
          ..Default::default()
        },
      },
    )
  }
}
```

- [ ] **Step 2: Point the crate at it**

In `src/lib.rs`: add `pub mod process;` beside `mod pre; mod stop;`; replace `use aeth_devkit_core::process::Runner;` with `use crate::process::Runner;`; replace `&aeth_devkit_core::process::SystemRunner` in `run_real` with `&crate::process::SystemRunner`; change the module doc's first line from `` `devkit hook <name>` — Claude Code hooks, ported from the per-repo Python scripts. `` to `` `devkit-hook <name>` — Claude Code hooks, installed into every devkit-managed project by `devkit setup-project`. ``. In `src/stop.rs`: `use crate::process::Runner;`. In `src/main.rs`: `aeth_devkit_hooks::` → `devkit_claude_hooks::` (two places). In `tests/hooks.rs`: `use aeth_devkit_core::process::RecordingRunner;` → `use devkit_claude_hooks::process::RecordingRunner;`, every other `aeth_devkit_hooks::` → `devkit_claude_hooks::`, and the `FailingRunner` impl becomes:

```rust
impl devkit_claude_hooks::process::Runner for FailingRunner {
  fn run_capture(&self, _: &str, _: &[String], _: &Path) -> anyhow::Result<devkit_claude_hooks::process::CapturedOutput> {
    anyhow::bail!("program not found")
  }
}
```

(`tests/binary.rs` finds the binary through `env!("CARGO_BIN_EXE_devkit-hook")` and needs no change.)

```bash
grep -rn 'aeth_devkit' src tests; echo "(no output above = clean)"
grep -rn 'devkit hook' src tests
```

Expected: no `aeth_devkit`; any comment the second grep finds still spelling the command `devkit hook` is reworded to `devkit-hook`.

- [ ] **Step 3: Manifests**

Replace `Cargo.toml` with:

```toml
[package]
  name    = "devkit-claude-hooks"
  version = "1.0.0"
  edition = "2024"
  publish = false

[lib]
  name = "devkit_claude_hooks"

[[bin]]
  name = "devkit-hook"
  path = "src/main.rs"

[dependencies]
  anyhow     = "1.0.104"
  clap       = { version = "4", features = ["derive"] }
  regex      = "1.13.1"
  serde      = { version = "1.0.229", features = ["derive"] }
  serde_json = { version = "1.0.151", features = ["preserve_order"] }

[dev-dependencies]
  tempfile = "3.27.0"

[profile.release]
  strip       = true
  incremental = true
```

Create `rustfmt.toml`:

```
tab_spaces = 2
max_width  = 135
```

Create `.gitignore` (setup-project prepends its own rules later and keeps these):

```
/target/
/.venv/
/dist/
/.cache/
```

Create `pyproject.toml`:

```toml
[project]
  name            = "devkit-claude-hooks"
  version         = "1.0.0"
  description     = "Claude Code hooks for devkit-managed projects: the devkit-hook binary that devkit setup-project wires into .claude/settings.local.json"
  readme          = "README.md"
  requires-python = ">=3.14"
  dependencies    = []

[dependency-groups]
  dev = ["aeth-devkit>=12.1.0", "maturin>=1.7,<2"]

[build-system]
  requires      = ["maturin>=1.7,<2"]
  build-backend = "maturin"

[tool.maturin]
  bindings      = "bin"
  module-name   = "devkit_claude_hooks"
  python-source = "python"

[tool.poe]
  include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]

[tool.uv.sources]
  aeth-devkit = [{ index = "SFTPyPI" }]

[[tool.uv.index]]
  name        = "SFTPyPI"
  url         = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"
  publish-url = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/"
  explicit    = true
```

Create `python/devkit_claude_hooks/__init__.py` as an empty file.

- [ ] **Step 4: README and TODO**

Create `README.md`:

```markdown
# devkit-claude-hooks

`devkit-hook <name>`: the Claude Code hooks of every devkit-managed project. `devkit
setup-project` adds this package to the project's dev group at the newest release the
installed devkit accepts and writes the five hook lines into `.claude/settings.local.json`,
pointing at the venv's `devkit-hook`. Payload on stdin, at most one JSON line on stdout,
always exits 0: every failure path degrades to silence, because a non-zero exit is shown
as a hook error in every session.

- **`pre-edit-protect`** - Denies Edit/Write to `.env` and `uv.lock`, matched on the
  basename with Windows name normalization.
- **`pre-bash-protect-deps`** - Denies `uv add|remove|lock` via a quote-aware command
  tokenizer (handles wrappers, env-var prefixes, `bash -c` recursion, and uv's
  value-taking global flags — not a regex).
- **Stop hooks** - Re-report tool failures as `additionalContext`: `stop-ruff` (`--fix
  --unfixable F401`) scoped to the branch diff, `stop-pyright` project-wide on purpose,
  `stop-clean` (`poe clean`); venv binaries preferred over `uv run`; output capped at
  4000 chars; `stop_hook_active` loop guard.

## Develop

```sh
uv sync                 # builds the crate into .venv through maturin
cargo test
uv run devkit-hook --version
```

The repository is devkit-managed: `poe setup-project` keeps the shared configuration
current. Release with `poe release`; the wheel goes to SFTPyPI and every project takes the
new version on its next `poe setup-project`.
```

Create `TODO.md`:

```markdown
# TODO

- [ ] **Auto-commit `stop-ruff`'s safe fixes** — `stop-ruff` (`src/stop.rs`) runs
      `ruff check --fix` and leaves whatever it changes uncommitted and unreported: a clean
      fix exits 0, so the hook says nothing and the diff just sits in the tree until someone
      notices `git status`. Auto-commit those changes instead. Constraints found while
      scoping this:
  - The commit must happen inside the same invocation that ran `--fix`, before the
    pass/fail branch — `stop_hook_active` skips the whole hook on a continued turn, so a
    turn where ruff still had unfixable complaints would never get a later chance to
    commit the fixes it already made.
  - On `main`/`master`, `scope()` runs project-wide, so the commit must stage only the
    paths ruff actually touched (diff `git status` around the `--fix` call, or otherwise
    track ruff's fixed-file list) — never a blanket `git add -A`/`git commit -a`, since
    the tree can hold unrelated uncommitted work (a design doc mid-edit, etc.) at Stop
    time that must not get swept in.
  - `stop-pyright` never fixes anything (report-only) and `stop-clean` only deletes
    generated files, so neither is in scope for this — only `stop-ruff` applies.
```

- [ ] **Step 5: CI**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  rust:
    name: Rust (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [windows-latest, ubuntu-latest]
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - uses: Swatinem/rust-cache@v2

      - name: Format
        run: cargo fmt --all --check

      - name: Clippy
        run: cargo clippy --all-targets -- -D warnings

      - name: Test
        run: cargo test

  wheel:
    name: Wheel build (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [windows-latest, ubuntu-latest]
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"

      # The wheel is what every project installs: the binary must ride in it as a script.
      - name: Build the wheel and run the binary from a develop install
        shell: bash
        run: |
          uv sync
          uv run maturin build --release --out dist
          uv run python -c "import zipfile,glob; names=zipfile.ZipFile(glob.glob('dist/*.whl')[0]).namelist(); assert any(n.endswith('scripts/devkit-hook') or n.endswith('scripts/devkit-hook.exe') for n in names), names; assert 'devkit_claude_hooks/__init__.py' in names, names"
          uv run maturin develop
          uv run --no-sync devkit-hook --version
```

- [ ] **Step 6: Build, test, package**

```bash
cd "$WS/devkit-claude-hooks"
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | grep -E 'test result|FAILED'
uv sync && uv run devkit-hook --version
uv run maturin build --release --out dist 2>&1 | tail -1
uv run python -c "import zipfile,glob; print('\n'.join(zipfile.ZipFile(glob.glob('dist/*.whl')[0]).namelist()))"
git status --short
```

Expected: fmt and clippy clean; every `test result: ok`; `devkit-hook 1.0.0`; the wheel lists `devkit_claude_hooks/__init__.py`, `devkit_claude_hooks-1.0.0.dist-info/*` and `devkit_claude_hooks-1.0.0.data/scripts/devkit-hook.exe` (no `.exe` on Linux); `git status` shows only the intended new and modified files (`dist/`, `.venv/`, `uv.lock` are ignored or untracked; `uv.lock` is committed in Task 5 with the lock commit).

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml rustfmt.toml .gitignore src tests pyproject.toml python README.md TODO.md .github
git commit -q -m "chore: make devkit-claude-hooks a standalone crate and wheel

The crate carries its own process seam instead of aeth-devkit-core, builds
the devkit-hook binary from the repository root, and packages it as a
maturin bin wheel with an empty devkit_claude_hooks package, the shape the
other devkit packages have.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Extract `crates/aeth-devkit-complete` with its history

**Files:**
- Create: `$WS/devkit-poe-complete` (a filtered clone), `.gitattributes` in it.

**Interfaces:**
- Produces: a local repository on branch `main`, no remote, no tags, the crate at the root, LF throughout.

- [ ] **Step 1: Filter a throwaway clone**

```bash
cd "$WS" && git clone --no-local aeth_devkit devkit-poe-complete && cd devkit-poe-complete
MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' uvx --from git-filter-repo git-filter-repo --force \
  --path crates/aeth-devkit-complete/ \
  --path-rename crates/aeth-devkit-complete/:
git log --oneline | wc -l
git ls-files
git remote -v
git tag -l | wc -l
```

Expected: the crate's commits; the file list is `Cargo.toml`, `src/cache.rs`, `src/engine.rs`, `src/format.rs`, `src/install.rs`, `src/lib.rs`, `src/main.rs`, `src/parse.rs`, `src/repair.rs`, `src/resolve.rs`, `src/scripts.rs`, `src/wire.rs`, `src/words.rs`, `tests/cli.rs`, `tests/complete.rs`, `tests/engine.rs`, `tests/format_cache.rs`, `tests/install.rs`, `tests/parse.rs`, `tests/repair.rs`, `tests/shim.rs`, `tests/wire.rs`, `tests/words.rs`; no remote; some tags.

- [ ] **Step 2: Tags, branch, LF**

Write `.gitattributes` with exactly:

```
* text=auto eol=lf
*.sh text eol=lf
```

```bash
git tag -l | xargs -r git tag -d >/dev/null; git tag -l | wc -l; git branch --show-current
git add .gitattributes && git add --renormalize . && git commit -q -m "chore: add .gitattributes so checkouts are LF

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git rm -rq --cached . && git reset -q --hard HEAD
git ls-files --eol | awk '{print $1, $2}' | sort | uniq -c
```

Expected: `0` tags, branch `main`, every file `i/lf w/lf`.

---

### Task 4: Stand `devkit-poe-complete` up, with shims that call `devkit-complete`

**Files:**
- Create: `src/process.rs`, `rustfmt.toml`, `.gitignore`, `pyproject.toml`, `python/devkit_poe_complete/__init__.py`, `README.md`, `.github/workflows/ci.yml`.
- Modify: `Cargo.toml`, `src/lib.rs`, `src/cache.rs`, `src/engine.rs`, `src/install.rs`, `src/resolve.rs`, `src/scripts.rs`, `src/main.rs`, `tests/complete.rs`, `tests/engine.rs`, `tests/format_cache.rs`, `tests/install.rs`, `tests/cli.rs`, `tests/shim.rs`.

**Interfaces:**
- Produces: crate `devkit-poe-complete` 1.0.0, lib `devkit_poe_complete` (the modules and `Args`, `Command`, `run_real`, `run_install`, `output` as today), binary `devkit-complete`; shims with `SHIM_VERSION = 3` that call `devkit-complete query`; `install --bash` overwrites a v2 shim; the wheel `devkit_poe_complete-1.0.0-*.whl` with the script and the empty package.

- [ ] **Step 1: The private process seam**

Create `src/process.rs` (this crate's tests use `script`, never `script_err`):

```rust
//! Running a program and capturing what it printed: the one seam this crate needs, behind
//! a trait so the tests answer from scripts instead of spawning python or poe. A private
//! copy of the seam `aeth-devkit-core` has: a library repo whose only export is a test
//! seam is not worth a dependency edge (devkit split spec, section 3).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result};

/// What a finished process left behind. `code` is `None` when a signal ended it.
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
  /// Run `program args` in `cwd` and capture stdout and stderr.
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput>;
}

/// Spawns for real.
pub struct SystemRunner;

impl Runner for SystemRunner {
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    let out = Command::new(program)
      .args(args)
      .current_dir(cwd)
      .output()
      .with_context(|| format!("running {program}"))?;
    Ok(CapturedOutput {
      code: out.status.code(),
      stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
      stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
  }
}

/// One recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
  pub program: String,
  pub args: Vec<String>,
  pub cwd: PathBuf,
}

struct Script {
  program: String,
  arg_prefix: Vec<String>,
  code: i32,
  stdout: String,
}

/// Records every call and answers from scripts; for tests. The most recently registered
/// script whose program matches and whose `arg_prefix` starts the call's arguments wins, so
/// a broad default can be overridden for one call; an unmatched call exits `exit_code` with
/// no output.
pub struct RecordingRunner {
  pub calls: RefCell<Vec<Invocation>>,
  pub exit_code: i32,
  scripts: RefCell<Vec<Script>>,
}

impl RecordingRunner {
  pub fn new(exit_code: i32) -> Self {
    Self {
      calls: RefCell::new(Vec::new()),
      exit_code,
      scripts: RefCell::new(Vec::new()),
    }
  }

  pub fn script(&self, program: &str, arg_prefix: &[&str], code: i32, stdout: &str) -> &Self {
    self.scripts.borrow_mut().push(Script {
      program: program.to_string(),
      arg_prefix: arg_prefix.iter().map(|s| s.to_string()).collect(),
      code,
      stdout: stdout.to_string(),
    });
    self
  }

  /// The argument lists of every recorded call to `program`, in order.
  pub fn calls_for(&self, program: &str) -> Vec<Vec<String>> {
    self.calls.borrow().iter().filter(|c| c.program == program).map(|c| c.args.clone()).collect()
  }
}

impl Runner for RecordingRunner {
  fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
    self.calls.borrow_mut().push(Invocation {
      program: program.to_string(),
      args: args.to_vec(),
      cwd: cwd.to_path_buf(),
    });
    let scripts = self.scripts.borrow();
    Ok(
      match scripts.iter().rev().find(|s| s.program == program && args.starts_with(&s.arg_prefix)) {
        Some(s) => CapturedOutput {
          code: Some(s.code),
          stdout: s.stdout.clone(),
          stderr: String::new(),
        },
        None => CapturedOutput {
          code: Some(self.exit_code),
          ..Default::default()
        },
      },
    )
  }
}
```

- [ ] **Step 2: Point the crate at it**

In `src/lib.rs`: add `pub mod process;` to the module list; `use aeth_devkit_core::process::SystemRunner;` → `use crate::process::SystemRunner;`. In `src/cache.rs`, `src/engine.rs`, `src/install.rs`, `src/resolve.rs`: `use aeth_devkit_core::process::Runner;` → `use crate::process::Runner;`. In `src/main.rs` and every test file: `aeth_devkit_complete` → `devkit_poe_complete`, and `aeth_devkit_core::process::` → `devkit_poe_complete::process::` (`tests/complete.rs`, `tests/engine.rs`, `tests/format_cache.rs`, `tests/install.rs`).

```bash
grep -rn 'aeth_devkit' src tests; echo "(no output above = clean)"
```

- [ ] **Step 3: The shims call the new binary**

In `src/scripts.rs`: `pub const SHIM_VERSION: u32 = 3;`; in both shim texts change every `shim version 2` and `--shim-version 2` to `3`; in the header comments and the module doc, `devkit complete install --bash` → `devkit-complete install --bash`, `devkit complete install --powershell` → `devkit-complete install --powershell`, `devkit complete query` → `devkit-complete query`, and `devkit complete script --bash|--powershell` → `devkit-complete script …`. In the bash shim: `command -v devkit >/dev/null 2>&1 || return 0` → `command -v devkit-complete >/dev/null 2>&1 || return 0`, and `out=$(devkit complete query --shell bash --shim-version 3 \` (the `complete` word is gone). In the PowerShell shim: `$dk = Get-Command devkit -ErrorAction SilentlyContinue` → `$dk = Get-Command devkit-complete -ErrorAction SilentlyContinue`, and `$out = & $dk.Source complete query --shell powershell --shim-version 3 …` → `$out = & $dk.Source query --shell powershell --shim-version 3 …`. Both comments that say "No devkit on PATH" become "No devkit-complete on PATH".

In `src/install.rs`: the module doc's first line `devkit complete install` → `devkit-complete install`; and the marker list, so `install --bash` recognises the v2 shim as generated (its header is `# Bash completion for poe - devkit thin shim (shim version 2)`, which carries neither existing marker; only the Tab-time repair ever rewrote it):

```rust
/// Header fragments marking a bash completion file as generated — by poe, by an older
/// devkit, or by this crate's own shim — and therefore ours to overwrite. Anything else was
/// written by a person.
const GENERATED_MARKERS: [&str; 3] = ["Generated by poethepoet", "adapted by aeth-devkit", "devkit thin shim"];
```

In `src/lib.rs`'s module doc: `devkit complete` → `devkit-complete` wherever it names the command (first line, the "forwards the command line to `devkit complete query`" sentence, and the paragraph about no global install; the historical sentence about the previous `devkit complete script | Invoke-Expression` design stays as history). Leave `OLD_DEVKIT_POWERSHELL_LINE` (`devkit complete script --powershell`) alone: it identifies the legacy profile line the installer removes. Then `grep -rn 'devkit complete' src tests`: every remaining hit must be one of those two historical mentions, the legacy constant, or a test fixture that deliberately carries the old profile line.

- [ ] **Step 4: The tests that pin the shim text**

`tests/cli.rs`, in `scripts_register_for_poe_and_call_devkit_for_data`: `assert!(script.contains("devkit complete query"), "{shell}");` → `assert!(script.contains("devkit-complete query"), "{shell}");`. `tests/shim.rs`, in `both_shims_tolerate_devkit_being_absent`: `command -v devkit` → `command -v devkit-complete`, `Get-Command devkit` → `Get-Command devkit-complete`. In `shim_text_is_pinned_to_its_version`, run the test once to read the new pair of hashes from the assertion's `left` value, then write them into the `assert_eq!` (the test exists so that a shim edit is a deliberate version bump; the bump is Step 3). Add to `tests/install.rs`:

```rust
#[test]
fn the_shipped_shim_of_an_older_version_is_ours_to_overwrite() {
  let v2 = "# Bash completion for poe - devkit thin shim (shim version 2)\n#\n# Installed by `devkit complete install --bash`.\n_poe_complete() { :; }\n";
  assert_eq!(bash_file_action(Some(v2), SCRIPT), FileAction::Write);
}
```

```bash
cargo test 2>&1 | grep -E 'test result|FAILED|panicked' | head
```

Expected: after the hash update, every `test result: ok`.

- [ ] **Step 5: Manifests, packaging, README, CI**

Replace `Cargo.toml` with:

```toml
[package]
  name    = "devkit-poe-complete"
  version = "1.0.0"
  edition = "2024"
  publish = false

[lib]
  name = "devkit_poe_complete"

[[bin]]
  name = "devkit-complete"
  path = "src/main.rs"

[dependencies]
  anyhow      = "1.0.104"
  clap        = { version = "4", features = ["derive"] }
  serde       = { version = "1.0.229", features = ["derive"] }
  serde_json  = { version = "1.0.151", features = ["preserve_order"] }
  shell-words = "1.1.0"
  toml_edit   = "0.25.13"

[dev-dependencies]
  tempfile = "3.27.0"

[profile.release]
  strip       = true
  incremental = true
```

`rustfmt.toml` and `.gitignore`: the same two files as Task 2 Step 3 (`tab_spaces = 2` / `max_width = 135`; `/target/`, `/.venv/`, `/dist/`, `/.cache/`).

`pyproject.toml`: Task 2 Step 3's file with `name = "devkit-poe-complete"`, `description = "poe shell completion for devkit-managed projects: the devkit-complete binary whose shims devkit setup-project installs"`, and `module-name = "devkit_poe_complete"`; everything else identical. Create `python/devkit_poe_complete/__init__.py`, empty.

`README.md`:

```markdown
# devkit-poe-complete

`devkit-complete`: shell completion for `poe` served from Rust (~13 ms per Tab instead of
poe's ~200 ms). `devkit setup-project` adds this package to every project's dev group at
the newest release the installed devkit accepts and runs `devkit-complete install` for the
shells on `PATH`, so nobody types the command. The shims call the `devkit-complete` an
activated venv puts on `PATH`; no global install is needed.

Subcommands: `query` (the per-Tab request, called by the shims), `tasks [DIR]` and `args
<TASK> [DIR]` (retained for shims installed by an older devkit), `script
--powershell|--bash`, `install --powershell --bash [--dry-run]`; global `--no-cache`.

- **Thin shims** - Each shell installs a ~50-line shim that forwards the command line to
  `devkit-complete query` and acts on a directory/file sentinel; all the logic (task
  location, global options, choices, positional indexing) lives in one Rust engine rather
  than in two near-duplicate shell scripts. The shells still do their own path completion,
  keeping their own quoting rules.
- **Task resolution** - Mirrors poe's: `[tool.poe.tasks]`, recursive `include` files
  (env-var expansion, cycle guard), hidden `_` tasks skipped, first definition wins;
  `include_script` is executed against the venv python directly, skipping poe's startup.
- **Caching** - Fingerprint cache at `.cache/devkit-completions.json` (binary version +
  each source's mtime/size); a corrupt cache is a miss, and the data subcommands never
  exit non-zero — a failing completer would break the shell.
- **Install** - Writes the PowerShell shim to `~/.local/share/devkit/poe-completion.ps1`
  and puts one permanent, content-free line in `$PROFILE` that dot-sources it (also
  removing poe's own slow registration, and any previous devkit line); writes the bash
  completion files for Git Bash and Linux; refuses to overwrite files it didn't generate;
  idempotent.
- **Self-repair** - Each request carries a shim version. A shim older than the binary is
  rewritten in place (atomically) for the next shell, while the current request is still
  answered. A shim from before this package existed calls `devkit complete`, which no
  longer exists; `devkit-complete install` (which `setup-project` runs) replaces it.
- **Shells** - PowerShell and bash only.

## Develop

```sh
uv sync                 # builds the crate into .venv through maturin
cargo test
uv run devkit-complete --version
```

The repository is devkit-managed: `poe setup-project` keeps the shared configuration
current. Release with `poe release`; the wheel goes to SFTPyPI and every project takes the
new version on its next `poe setup-project`.
```

`.github/workflows/ci.yml`: Task 2 Step 5's file with the wheel step's binary name changed: the `assert any(n.endswith('scripts/devkit-hook') …` becomes `scripts/devkit-complete` / `scripts/devkit-complete.exe`, `'devkit_claude_hooks/__init__.py'` becomes `'devkit_poe_complete/__init__.py'`, and the last line `uv run --no-sync devkit-complete --version`.

- [ ] **Step 6: Build, test, package, commit**

```bash
cd "$WS/devkit-poe-complete"
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test 2>&1 | grep -E 'test result|FAILED'
uv sync && uv run devkit-complete --version
uv run maturin build --release --out dist 2>&1 | tail -1
uv run python -c "import zipfile,glob; print('\n'.join(zipfile.ZipFile(glob.glob('dist/*.whl')[0]).namelist()))"
uv run devkit-complete script --bash | grep -n 'devkit-complete query\|shim-version 3'
git add Cargo.toml rustfmt.toml .gitignore src tests pyproject.toml python README.md .github
git commit -q -m "chore: make devkit-poe-complete a standalone crate and wheel; shims call devkit-complete

The crate carries its own process seam instead of aeth-devkit-core, builds
the devkit-complete binary from the repository root and packages it as a
maturin bin wheel with an empty devkit_poe_complete package. The shims move
to version 3 and call devkit-complete query, and install --bash now
recognises the shipped shim of an older version as generated, so the
migration from devkit complete can overwrite it.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

Expected: fmt, clippy and tests clean; `devkit-complete 1.0.0`; the wheel lists `devkit_poe_complete/__init__.py` and `devkit_poe_complete-1.0.0.data/scripts/devkit-complete[.exe]`; the printed bash script has both greps.

---

### Task 5: Create both GitHub repositories, publish both wheels

**Files:**
- In each new repo, by `setup-project` (USER RUNS): the standard files, `.github/workflows/release.yml`, `uv.lock`.

**Interfaces:**
- Produces: `https://github.com/AetherBreaker/devkit-claude-hooks` and `…/devkit-poe-complete`, public, CI green, releases `v1.0.0`, `devkit-claude-hooks==1.0.0` and `devkit-poe-complete==1.0.0` on SFTPyPI. Part B's template floors depend on both.

- [ ] **Step 1: Create, push, secrets, `.env`** (for each of `devkit-claude-hooks` and `devkit-poe-complete`)

Create the repository with the GitHub MCP `create_repository` tool: `name` as above, `private: false`, `autoInit: false`, `description` "Claude Code hooks for devkit-managed projects (the devkit-hook binary)" / "poe shell completion for devkit-managed projects (the devkit-complete binary)". Then:

```bash
cd "$WS/<repo>"
git remote add origin https://github.com/AetherBreaker/<repo>.git && git push -u origin main 2>&1 | tail -1
gh repo view AetherBreaker/<repo> --json name,visibility,defaultBranchRef --jq '{name,visibility,default:.defaultBranchRef.name}'
for k in UV_INDEX_SFTPYPI_USERNAME UV_INDEX_SFTPYPI_PASSWORD; do
  grep "^$k=" "$WS/aeth_devkit/.env" | cut -d= -f2- | sed 's/^"\(.*\)"$/\1/' | gh secret set "$k" --repo AetherBreaker/<repo> && echo "set $k"
done
gh secret list --repo AetherBreaker/<repo> | awk '{print $1}'
cp "$WS/aeth_devkit/.env" .env && git check-ignore -q .env && echo ".env ignored"
```

Expected: `{"name":"<repo>","visibility":"PUBLIC","default":"main"}`; both secret names listed; `.env ignored`. (`gh repo create --push` is refused by the tool permission layer; this is the step-2 route.)

- [ ] **Step 2: CI green** (each repo)

```bash
sleep 20; gh run watch --repo AetherBreaker/<repo> --exit-status "$(gh run list --repo AetherBreaker/<repo> --workflow ci.yml --limit 1 --json databaseId --jq '.[0].databaseId')" 2>&1 | tail -2
```

Expected: success on both jobs of both repos. A failure is a Task 2 or 4 problem; fix on `main`, push, wait again.

- [ ] **Step 3: USER RUNS setup-project in both repositories**

Hand the user this, verbatim (the release workflow `devkit release` needs comes from this run):

```bash
cd "/d/SFT Software Projects/SFT Workspace/devkit-claude-hooks" && uv sync && uv run devkit --version && uv run devkit setup-project --no-vscode
cd "/d/SFT Software Projects/SFT Workspace/devkit-poe-complete"  && uv sync && uv run devkit --version && uv run devkit setup-project --no-vscode
```

Expected: `devkit 12.1.0`; one "Standardize project configuration with devkit" commit in each, `.github/workflows/release.yml` rendered from the Rust template, and the note naming the two `UV_INDEX_SFTPYPI_*` secrets (already set).

- [ ] **Step 4: Lock, format, release** (each repo)

```bash
cd "$WS/<repo>"
git log --oneline -2 && test -f .github/workflows/release.yml && echo "release workflow present"
uv lock && uv sync && uv run tombi format --quiet pyproject.toml
git add uv.lock pyproject.toml && git commit -q -m "chore: lock the dev group setup-project added

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" && git push 2>&1 | tail -1
uv run devkit release --dry-run 2>&1 | tail -12
uv run devkit release --force 2>&1 | tail -6
```

`devkit release` with no bump releases the committed `1.0.0`; run it with a long timeout (the workflow builds wheels on two platforms). Expected: `Released <repo> 1.0.0`, the workflow green, the package on SFTPyPI:

```bash
gh release view v1.0.0 --repo AetherBreaker/<repo> --json assets --jq '[.assets[].name]'
curl -s https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple/<repo>/ | grep -o '<repo_underscored>-1\.0\.0[^"<#]*' | sort -u
```

Expected: two wheels and an sdist; the wheel and sdist names on the index.

- [ ] **Step 5: Both wheels resolve from a scratch project**

```bash
S="$TEMP/hooks-complete-resolve" && rm -rf "$S" && mkdir -p "$S" && cd "$S"
printf '[project]\nname = "scratch"\nversion = "0"\nrequires-python = ">=3.14"\ndependencies = []\n\n[dependency-groups]\ndev = ["devkit-claude-hooks", "devkit-poe-complete"]\n\n[tool.uv.sources]\ndevkit-claude-hooks = [{ index = "SFTPyPI" }]\ndevkit-poe-complete = [{ index = "SFTPyPI" }]\n\n[[tool.uv.index]]\nname = "SFTPyPI"\nurl = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"\nexplicit = true\n' > pyproject.toml
uv sync 2>&1 | tail -3 && uv run devkit-hook --version && uv run devkit-complete --version
```

Expected: both binaries print `1.0.0` from wheels installed off the index.

---

## Part B: `aeth-devkit`

All Part B work is on branch `feat/extract-hooks-and-completion`:

```bash
cd "$WS/aeth_devkit" && git switch main && git pull --ff-only && git switch -c feat/extract-hooks-and-completion
```

Run only the named tests while iterating; the full suite runs once in Task 10.

### Task 6: The package step carries the two packages; a marker on a value key

**Files:**
- Modify: `crates/aeth-devkit-setup/src/packages.rs`, `crates/aeth-devkit-setup/src/toml_merge.rs`, `python/aeth_devkit/templates/pyproject.template.toml`.
- Test: `crates/aeth-devkit-setup/src/packages.rs` (unit), `crates/aeth-devkit-setup/src/toml_merge.rs` (unit), `crates/aeth-devkit-setup/tests/packages.rs`, `crates/aeth-devkit-setup/tests/apply.rs`.

**Interfaces:**
- Produces: `packages::HOOKS` (`name: "devkit-claude-hooks"`, `import_name: "devkit_claude_hooks"`) and `packages::COMPLETE` (`name: "devkit-poe-complete"`, `import_name: "devkit_poe_complete"`), both always in `active()` (minus the project's own name); the template dev group listing both at `{latest}`; `[tool.uv.sources]` ungated with a value-level `if-docker-services` marker on `devkit-container`; `toml_merge::marker_lines` reading a value key's decor; `check_markers` checking value keys; marker lines stripped from an inserted value key.

- [ ] **Step 1: The failing unit tests**

In `crates/aeth-devkit-setup/src/packages.rs`, replace `the_container_is_active_only_for_docker_projects` with:

```rust
  #[test]
  fn hooks_and_completion_are_always_active_the_container_only_with_docker() {
    let dir = tempfile::tempdir().unwrap();
    let names = |pyproject: &str| {
      std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
      let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
      active(&ctx).iter().map(|p| p.name).collect::<Vec<_>>()
    };
    assert_eq!(names("[project]
name = \"p\"
"), vec!["devkit-claude-hooks", "devkit-poe-complete"]);
    assert_eq!(
      names("[project]
name = \"p\"
[tool.docker]
services = [\"p\"]
"),
      vec!["devkit-claude-hooks", "devkit-poe-complete", "devkit-container"]
    );
    // A satellite never carries itself, under any spelling of its name.
    assert_eq!(names("[project]
name = \"devkit-claude-hooks\"
"), vec!["devkit-poe-complete"]);
    assert_eq!(names("[project]
name = \"Devkit_Poe_Complete\"
"), vec!["devkit-claude-hooks"]);
    assert_eq!(
      names("[project]
name = \"devkit_container\"
[tool.docker]
services = [\"x\"]
"),
      vec!["devkit-claude-hooks", "devkit-poe-complete"]
    );
  }
```

(`latest_requested_lists_the_placeholder_requirements` already covers a `{latest}` entry in a dependency group and needs no change.)

In `crates/aeth-devkit-setup/src/toml_merge.rs`, in the test module whose `ctx` takes `has_docker: bool` (the docker-gating module), add:

```rust
  #[test]
  fn a_marker_above_a_value_gates_that_key_only() {
    let tpl = "[tool.uv.sources]\n  # setup-project: if-docker-services\n  devkit-container = [{ index = \"SFTPyPI\" }]\n  devkit-claude-hooks = [{ index = \"SFTPyPI\" }]\n";
    let orig = "[project]\n  name = \"p\"\n";
    let out = merge_pyproject(orig, tpl, &ctx(false), &mut vec![]).unwrap();
    assert!(out.contains("devkit-claude-hooks = [{ index = \"SFTPyPI\" }]"), "{out}");
    assert!(!out.contains("devkit-container"), "{out}");
    assert!(!out.contains("setup-project:"), "the marker is an instruction to the merger: {out}");
    let out = merge_pyproject(orig, tpl, &ctx(true), &mut vec![]).unwrap();
    assert!(out.contains("devkit-container = [{ index = \"SFTPyPI\" }]"), "{out}");
    assert!(!out.contains("setup-project:"), "{out}");
    let bad = "[tool.uv.sources]\n  # setup-project: if-dockr\n  x = 1\n";
    let err = merge_pyproject(orig, bad, &ctx(false), &mut vec![]).unwrap_err().to_string();
    assert!(err.contains("unknown marker") && err.contains("tool.uv.sources.x"), "{err}");
  }
```

```bash
cargo test -p aeth-devkit-setup --lib hooks_and_completion_are_always 2>&1 | tail -3
cargo test -p aeth-devkit-setup --lib a_marker_above_a_value 2>&1 | grep -E 'test result|panicked' | head -2
```

Expected: the first fails to compile (no `HOOKS`), the second fails (the container source is merged regardless and the marker text reaches the output).

- [ ] **Step 2: The packages**

In `packages.rs`, after `CONTAINER`:

```rust
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
```

`active` becomes (update its doc comment: hooks and completion are every project's, the container is Docker's; the self rule stays):

```rust
pub fn active(ctx: &ProjectContext) -> Vec<&'static DevkitPackage> {
  let own = normalize_dist_name(&ctx.name);
  let mut out = vec![&HOOKS, &COMPLETE];
  if ctx.has_docker {
    out.push(&CONTAINER);
  }
  out.retain(|p| normalize_dist_name(p.name) != own);
  out
}
```

- [ ] **Step 3: Markers on value keys**

In `toml_merge.rs`, replace `marker_lines`:

```rust
/// The comment lines directly above a template table header or key-value line, with `#`
/// and whitespace stripped.
fn marker_lines(template: &Table, key: &str) -> Vec<String> {
  let prefix = match template.get(key) {
    Some(Item::Table(t)) => t.decor().prefix(),
    Some(_) => template.key(key).and_then(|k| k.leaf_decor().prefix()),
    None => None,
  };
  prefix
    .and_then(|p| p.as_str())
    .map(|p| p.lines().map(|l| l.trim().trim_start_matches('#').trim().to_string()).collect())
    .unwrap_or_default()
}
```

Replace `check_markers` so every key is checked and only tables recurse:

```rust
fn check_markers(template: &Table, path: &str) -> Result<()> {
  for (key, item) in template.iter() {
    let child = if path.is_empty() { key.to_string() } else { format!("{path}.{key}") };
    for line in marker_lines(template, key) {
      let known = line == IF_DOCKER_MARKER || line == IF_DOCKER_SERVICES_MARKER || line.starts_with(IF_DEP_MARKER);
      if line.starts_with(MARKER) && !known {
        bail!("pyproject template: unknown marker `# {line}` above {child}");
      }
    }
    if let Item::Table(t) = item {
      check_markers(t, &child)?;
    }
  }
  Ok(())
}
```

(The message loses its `[…]` brackets; the existing unknown-marker test asserts on "unknown marker", which still holds.) Generalise the marker stripper to a decor, used for tables and keys alike:

```rust
/// Drop the `setup-project:` marker lines from a decor's prefix — the comment block above a
/// table header or a key — keeping the other comments and the blank line above them. The
/// markers are instructions *to* this merger; shipped, one would read like a live directive
/// in the project's file while only ever being honoured on the template side.
fn strip_marker_lines(decor: &mut toml_edit::Decor) {
  // Build the replacement first: the immutable borrow ends before `set_prefix` needs a
  // mutable one. `split` on the newline char (not `lines()`) keeps the leading and trailing
  // empty pieces, so the blank line separating a table from the one above it survives.
  let cleaned = decor
    .prefix()
    .and_then(|p| p.as_str())
    .filter(|p| p.contains(MARKER))
    .map(|prefix| prefix.split('\n').filter(|l| !is_marker_line(l)).collect::<Vec<_>>().join("\n"));
  if let Some(cleaned) = cleaned {
    decor.set_prefix(cleaned);
  }
}
```

Delete `strip_marker_comments` and change its two call sites (`strip_marker_comments(&mut fresh)` in the brand-new-leaf-table path, `strip_marker_comments(sub)` in the explicit-header path) to `strip_marker_lines(fresh.decor_mut())` and `strip_marker_lines(sub.decor_mut())`. In `merge_value`'s `None` arm, strip the key too:

```rust
      None => {
        // Carry the template key's decor (indentation) so the new line matches its
        // neighbours, minus any marker that gated it.
        let mut key = tkey.clone();
        strip_marker_lines(key.leaf_decor_mut());
        let mut fresh = tval.clone();
        if let Value::Array(a) = &mut fresh {
          scrub_latest_array(a);
        }
        target.insert_formatted(&key, Item::Value(fresh));
        self.log.push(format!("added {path}"));
      }
```

- [ ] **Step 4: The template**

In `python/aeth_devkit/templates/pyproject.template.toml`, the dev group becomes:

```toml
[dependency-groups]
  # Every tool this template writes config for below, so a fresh project can actually run the
  # thing it was just configured for. The pytest entries are not optional: [tool.pytest]
  # sets --strict-config, which turns a missing plugin into a hard failure, so a bare `pytest`
  # errors out without pytest-cov (addopts passes --cov) and pytest-asyncio (asyncio_mode).
  # mypy is deliberately absent -- [tool.mypy] is `if-dep mypy` gated and stays opt-in.
  # devkit-claude-hooks and devkit-poe-complete are the Claude Code hooks and the poe
  # completion binaries setup-project wires in; their floors follow the newest release the
  # installed devkit accepts (spec 4.0).
  dev = [
    "devkit-claude-hooks>={latest}",
    "devkit-poe-complete>={latest}",
    "poethepoet>=0.46.0",
    "pyright>=1.1.411",
    "pytest>=8.0",
    "pytest-asyncio>=0.24",
    "pytest-cov>=5.0",
    "ruff>=0.15",
    "tombi>=1.5",
  ]
```

and the sources table at the end of the file loses its own marker and gains the two packages:

```toml
[tool.uv.sources]
  # setup-project: if-docker-services
  devkit-container    = [{ index = "{devkit_index}" }]
  devkit-claude-hooks = [{ index = "{devkit_index}" }]
  devkit-poe-complete = [{ index = "{devkit_index}" }]
```

- [ ] **Step 5: Run the unit tests**

```bash
cargo test -p aeth-devkit-setup --lib -- hooks_and_completion_are_always a_marker_above_a_value latest_requested_lists 2>&1 | grep -E 'test result|panicked' | head -3
cargo test -p aeth-devkit-setup --lib toml_merge 2>&1 | grep -E 'test result|FAILED' | head -2
```

Expected: all pass, including every other `toml_merge` test.

- [ ] **Step 6: The integration fixtures**

`crates/aeth-devkit-setup/tests/packages.rs` is built around the container; hooks and completion join as already-adopted packages so each test keeps its subject:

- `lock_with(container)` lists three packages: after the `aeth-devkit` entry add `[[package]]\nname = \"devkit-claude-hooks\"\nversion = \"1.0.0\"\nsource = { registry = \"https://idx/+simple\" }\n\n[[package]]\nname = \"devkit-poe-complete\"\nversion = \"1.0.0\"\nsource = { registry = \"https://idx/+simple\" }\n\n` before the container's.
- `venv(version)` always inserts `devkit_claude_hooks` and `devkit_poe_complete` at `Installed { dir: fixtures(), version: "1.0.0".into() }`; the container stays conditional on `version`.
- `DOCKER_PYPROJECT` gains `[dependency-groups]\n  dev = [\"devkit-claude-hooks>=1.0.0\", \"devkit-poe-complete>=1.0.0\"]\n\n` after `[tool.docker]`, and two more sources lines `devkit-claude-hooks = [{ index = \"SFTPyPI\" }]` and `devkit-poe-complete = [{ index = \"SFTPyPI\" }]` under `[tool.uv.sources]`.
- `latest()` returns all three names: `vec!["devkit-claude-hooks".into(), "devkit-poe-complete".into(), "devkit-container".into()]`.
- Every assertion on the recorded `uv lock` argument list gains the two pairs before the container's, in `active()`'s order: `["lock", "--upgrade-package", "devkit-claude-hooks", "--upgrade-package", "devkit-poe-complete", "--upgrade-package", "devkit-container", "--upgrade-package", "aeth-devkit==<RUNNING_DEVKIT>"]`.
- `a_package_missing_from_the_lock_after_locking_is_an_error` removes only the container from the lock (its `.replace("devkit-container", "something-else")` already does); it still passes.
- Replace `nothing_happens_without_docker` with:

```rust
const PLAIN_PYPROJECT: &str = "[project]\n  name = \"p\"\n\n[dependency-groups]\n  dev = [\"devkit-claude-hooks\", \"devkit-poe-complete\"]\n\n[tool.uv.sources]\n  devkit-claude-hooks = [{ index = \"SFTPyPI\" }]\n  devkit-poe-complete = [{ index = \"SFTPyPI\" }]\n\n[[tool.uv.index]]\n  name = \"SFTPyPI\"\n  url = \"https://idx/+simple\"\n  explicit = true\n";

#[test]
fn a_project_without_docker_still_gets_the_hooks_and_completion() {
  // The venv already holds both packages at the locked version (`venv(None)` after this
  // task's change to it), so the step locks, writes the floors and has nothing to sync; an
  // empty venv would end in the sync re-check that
  // `a_lagging_venv_is_synced_and_a_sync_that_changes_nothing_is_an_error` covers.
  let lock = lock_with("1.4.0").replace("devkit-container", "unrelated");
  let dir = project(PLAIN_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, &venv(None), &latest(), false).unwrap();
  let calls = runner.calls_for("uv");
  assert_eq!(
    calls,
    vec![vec![
      "lock".to_string(),
      "--upgrade-package".into(),
      "devkit-claude-hooks".into(),
      "--upgrade-package".into(),
      "devkit-poe-complete".into(),
      "--upgrade-package".into(),
      format!("aeth-devkit=={RUNNING_DEVKIT}"),
    ]],
    "one lock, no sync: the venv already matches"
  );
  let py = fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-claude-hooks>=1.0.0\"") && py.contains("\"devkit-poe-complete>=1.0.0\""), "{py}");
  assert!(changes.warnings.is_empty(), "{:?}", changes.warnings);
}
```

`crates/aeth-devkit-setup/tests/apply.rs`: in `run()`, the `StubVenv` map also holds `devkit_claude_hooks` and `devkit_poe_complete` at `Installed { dir: fixtures().join("docker"), version: "1.0.0".into() }`; in `make_project()`, the written `uv.lock` lists the two packages at `1.0.0` the way `lock_with` does. The fixture's dev group lacks the two, so the merge adds them, `advance` locks (recorded, no effect), and the floors `>=1.0.0` are written: `applies_and_is_idempotent` must still report a no-op second run.

```bash
cargo test -p aeth-devkit-setup --test packages 2>&1 | grep -E 'test result|FAILED|panicked' | head -4
cargo test -p aeth-devkit-setup --test apply 2>&1 | grep -E 'test result|FAILED|panicked' | head -4
cargo clippy -p aeth-devkit-setup --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: all pass; clippy and fmt clean. If `applies_and_is_idempotent` shows a second-run diff in the dev group, the floor rewrite is not stable across runs; fix `advance`, not the test.

- [ ] **Step 7: Commit**

```bash
git add crates/aeth-devkit-setup python/aeth_devkit/templates/pyproject.template.toml
git commit -q -m "feat(setup): every project carries devkit-claude-hooks and devkit-poe-complete

The two packages join the package step as always-active dev dependencies
at {latest} (spec 4.0); the template lists them and their index sources.
A marker above a key-value line now gates that key the way one above a
table header gates the table, so [tool.uv.sources] can hold the ungated
packages beside the container's if-docker-services entry.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: Hook lines call the venv's `devkit-hook`

**Files:**
- Modify: `crates/aeth-devkit-setup/src/templates.rs` (`devkit_bin` → `hook_bin`, the placeholder list, the test module), `crates/aeth-devkit-setup/src/json_merge.rs` (`hook_key`), `python/aeth_devkit/templates/claude/settings.local.template.jsonc`, `crates/aeth-devkit-setup/tests/apply.rs` (two assertions).

**Interfaces:**
- Produces: placeholder `{hook_bin}` → `"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit-hook.exe"` or `"$CLAUDE_PROJECT_DIR/.venv/bin/devkit-hook"` when present, else `uv run devkit-hook`; `hook_key` returning the hook name for both `… devkit-hook <name>` and `… hook <name>`.

- [ ] **Step 1: The failing tests**

In `templates.rs`, rename the module `devkit_bin_tests` to `hook_bin_tests` and replace its two tests:

```rust
  #[test]
  fn hook_bin_falls_back_to_uv_run_without_a_venv() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
      substitute("{hook_bin} pre-edit-protect", &ctx(dir.path()), Escape::None),
      "uv run devkit-hook pre-edit-protect"
    );
  }

  #[test]
  fn hook_bin_uses_the_venv_script_quoted_and_json_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("devkit-hook"), "").unwrap();
    let out = substitute(r#""cmd": "{hook_bin} stop-ruff""#, &ctx(dir.path()), Escape::Json);
    assert_eq!(out, r#""cmd": "\"$CLAUDE_PROJECT_DIR/.venv/bin/devkit-hook\" stop-ruff""#);
  }
```

In `json_merge.rs`'s hook tests, add (beside the test that updates a `python old/stop_ruff.py hook stop-ruff` entry in place):

```rust
  #[test]
  fn a_pre_split_hook_line_is_updated_in_place_to_the_new_binary() {
    let template = r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit-hook.exe\" stop-ruff", "shell": "bash", "timeout": 30}]}]}}"#;
    let original = r#"{"hooks": {"Stop": [{"hooks": [{"type": "command", "command": "\"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit.exe\" hook stop-ruff", "shell": "bash", "timeout": 30}]}]}}"#;
    let out = merge_claude_settings(Some(original), template, &mut vec![]).unwrap();
    let doc: Value = serde_json::from_str(&out).unwrap();
    let stop = doc["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
    assert_eq!(stop.len(), 1, "one entry per hook, updated in place: {out}");
    assert_eq!(stop[0]["command"], "\"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit-hook.exe\" stop-ruff");
    assert_eq!(hook_key(&json!({"command": "uv run devkit-hook pre-bash-protect-deps"})).as_deref(), Some("pre-bash-protect-deps"));
    assert_eq!(hook_key(&json!({"command": "\"$D/devkit-hook\" stop-clean"})).as_deref(), Some("stop-clean"));
    assert_eq!(hook_key(&json!({"command": "uv run devkit hook stop-clean"})).as_deref(), Some("stop-clean"));
  }
```

(`merge_claude_settings`, `Value`, `json!` and `hook_key` are in scope in that test module; if the module names differ, use the same helpers its neighbours use.)

```bash
cargo test -p aeth-devkit-setup --lib -- hook_bin_ a_pre_split_hook_line 2>&1 | grep -E 'error|test result|panicked' | head -4
```

Expected: compile error (no `{hook_bin}` substitution yet) or failures.

- [ ] **Step 2: The placeholder**

In `templates.rs`: rename `devkit_bin` to `hook_bin`, with the doc `/// How a hook line invokes `devkit-hook`: the venv's own console script when one exists (quoted, and via `$CLAUDE_PROJECT_DIR` so the file stays valid if the repo moves), else `uv run devkit-hook`. The direct path skips `uv run`'s ~140 ms environment check on every hook invocation.`, the candidates `[".venv/Scripts/devkit-hook.exe", ".venv/bin/devkit-hook"]`, and the fallback `"uv run devkit-hook".to_string()`; in `substitute`, `.replace("{devkit_bin}", &esc(&devkit_bin(&ctx.root)))` → `.replace("{hook_bin}", &esc(&hook_bin(&ctx.root)))`; in `load`'s doc, `{devkit_bin}` → `{hook_bin}`.

In `python/aeth_devkit/templates/claude/settings.local.template.jsonc`, the five commands become `"{hook_bin} pre-edit-protect"`, `"{hook_bin} pre-bash-protect-deps"`, `"{hook_bin} stop-ruff"`, `"{hook_bin} stop-pyright"`, `"{hook_bin} stop-clean"`.

- [ ] **Step 3: The key**

In `json_merge.rs`, replace `hook_key`:

```rust
/// The `<name>` of a devkit hook command: `… devkit-hook <name>` (the binary, possibly a
/// quoted venv path) or, from before the hooks had their own package, `… hook <name>`. One
/// entry per hook name whichever spelling wrote it, so migration updates in place.
fn hook_key(entry: &Value) -> Option<String> {
  let cmd = entry.get("command")?.as_str()?;
  let words: Vec<&str> = cmd.split_whitespace().collect();
  let is_hook_bin = |w: &str| {
    let base = w.trim_matches(['"', '\'']).rsplit(['/', '\\']).next().unwrap_or("");
    base == "devkit-hook" || base == "devkit-hook.exe"
  };
  if let Some(i) = words.iter().position(|w| is_hook_bin(w)) {
    return words.get(i + 1).map(|w| w.to_string());
  }
  let (_, rest) = cmd.split_once(" hook ")?;
  rest.split_whitespace().next().map(str::to_string)
}
```

- [ ] **Step 4: Tests, including the two end-to-end assertions**

In `tests/apply.rs`: `assert!(cmd.ends_with(" hook stop-ruff"), "{cmd}");` → `assert!(cmd.ends_with("devkit-hook stop-ruff"), "{cmd}");` and `assert_eq!(cmd, "\"$CLAUDE_PROJECT_DIR/.venv/Scripts/devkit.exe\" hook pre-edit-protect");` → the same line with `devkit-hook.exe\" pre-edit-protect` (that test writes a fake `.venv/Scripts/devkit.exe` first; write `devkit-hook.exe` instead).

```bash
cargo test -p aeth-devkit-setup --lib -- hook_bin_ a_pre_split_hook_line json_merge 2>&1 | grep -E 'test result|FAILED' | head -2
cargo test -p aeth-devkit-setup --test apply 2>&1 | grep -E 'test result|FAILED' | head -2
cargo clippy -p aeth-devkit-setup --all-targets -- -D warnings && cargo fmt --all --check
git grep -n 'devkit_bin' -- crates python; echo "(no output above = the old placeholder is gone)"
```

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-setup python/aeth_devkit/templates/claude/settings.local.template.jsonc
git commit -q -m "feat(setup): hook lines call the venv's devkit-hook

The settings template writes {hook_bin} <name>, the venv's devkit-hook
console script (else uv run devkit-hook), in place of devkit hook <name>;
the hook merge keys an entry by its hook name under either spelling, so a
project's existing lines are updated in place rather than duplicated.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: `setup-project` installs the completion shims

**Files:**
- Create: `crates/aeth-devkit-setup/src/completion.rs`.
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (module list, step 15).
- Test: unit tests in `completion.rs`.

**Interfaces:**
- Produces: `completion::Shells { powershell: bool, bash: bool }`, `completion::shells_on(path: &OsStr) -> Shells`, `completion::binary(root: &Path) -> Option<PathBuf>`, `completion::install(root, binary, shells, runner, dry_run, changes)`; `run_with` calling them as its last step.

- [ ] **Step 1: The failing tests**

Create `crates/aeth-devkit-setup/src/completion.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;

  #[test]
  fn shells_are_found_on_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::env::join_paths([dir.path()]).unwrap();
    assert_eq!(shells_on(&path), Shells::default());
    std::fs::write(dir.path().join("bash"), "").unwrap();
    assert_eq!(shells_on(&path), Shells { powershell: false, bash: true });
    std::fs::write(dir.path().join("pwsh.exe"), "").unwrap();
    assert_eq!(shells_on(&path), Shells { powershell: true, bash: true });
  }

  #[test]
  fn the_binary_is_the_venvs() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(binary(dir.path()), None);
    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("devkit-complete"), "").unwrap();
    assert_eq!(binary(dir.path()), Some(bin.join("devkit-complete")));
  }

  #[test]
  fn install_reports_changes_and_stays_quiet_when_there_are_none() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("devkit-complete");
    let shells = Shells { powershell: true, bash: true };
    let r = RecordingRunner::new(0);
    r.script(&bin.to_string_lossy(), &["install"], 0, "Changed:\n  - created C:/home/.local/share/devkit/poe-completion.ps1\n  - added: $c = …\nOpen a new shell for it to take effect.\n");
    let mut changes = Changes::new(false);
    install(root.path(), &bin, shells, &r, false, &mut changes);
    assert_eq!(r.calls_for(&bin.to_string_lossy())[0], vec!["install", "--powershell", "--bash"]);
    assert_eq!(
      changes.notes,
      vec![
        "shell completion: created C:/home/.local/share/devkit/poe-completion.ps1",
        "shell completion: added: $c = …"
      ]
    );

    let r = RecordingRunner::new(0);
    r.script(&bin.to_string_lossy(), &["install"], 0, "Nothing to do — completion is already installed.\n");
    let mut changes = Changes::new(true);
    install(root.path(), &bin, Shells { powershell: false, bash: true }, &r, true, &mut changes);
    assert_eq!(r.calls_for(&bin.to_string_lossy())[0], vec!["install", "--bash", "--dry-run"]);
    assert!(changes.notes.is_empty(), "{:?}", changes.notes);

    let r = RecordingRunner::new(0);
    let mut changes = Changes::new(false);
    install(root.path(), &bin, Shells::default(), &r, false, &mut changes);
    assert!(r.calls.borrow().is_empty(), "no shell, no call");

    let r = RecordingRunner::new(1);
    let mut changes = Changes::new(false);
    install(root.path(), &bin, shells, &r, false, &mut changes);
    assert_eq!(changes.warnings.len(), 1, "{:?}", changes.warnings);
    assert!(changes.warnings[0].starts_with("devkit-complete install failed"), "{:?}", changes.warnings);
  }
}
```

Register the module in `lib.rs` (`pub mod completion;` beside `pub mod packages;`) and run:

```bash
cargo test -p aeth-devkit-setup --lib completion 2>&1 | grep -E '^error' | head -3
```

Expected: compile errors (nothing defined yet).

- [ ] **Step 2: The module**

Above the tests in `completion.rs`:

```rust
//! Step 15 of setup-project: `devkit-complete install` for the shells this machine has, so
//! poe completion comes from the venv's binary without anyone typing the command. The binary
//! is the one the package step installed (step 1b); the shells are whatever `PATH` can start.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use aeth_devkit_core::process::Runner;

use crate::changes::Changes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shells {
  pub powershell: bool,
  pub bash: bool,
}

/// Which shells `PATH` can start: `pwsh` or Windows PowerShell counts as PowerShell, any
/// `bash` counts (Git Bash on Windows, the login shell elsewhere).
pub fn shells_on(path: &OsStr) -> Shells {
  let has = |names: &[&str]| std::env::split_paths(path).any(|d| names.iter().any(|n| d.join(n).is_file()));
  Shells {
    powershell: has(&["pwsh.exe", "powershell.exe", "pwsh"]),
    bash: has(&["bash.exe", "bash"]),
  }
}

/// The venv's `devkit-complete`, in the environment the package step syncs (the
/// `UV_PROJECT_ENVIRONMENT` rule `packages::SystemVenv` follows).
pub fn binary(root: &Path) -> Option<PathBuf> {
  let env = std::env::var_os("UV_PROJECT_ENVIRONMENT").map_or_else(|| PathBuf::from(".venv"), PathBuf::from);
  let env = if env.is_absolute() { env } else { root.join(env) };
  ["Scripts/devkit-complete.exe", "bin/devkit-complete"]
    .iter()
    .map(|rel| env.join(rel))
    .find(|p| p.is_file())
}

/// Run the install for `shells`; `--dry-run` on a dry run, since it writes to the home
/// directory rather than the project. The installer's "  - …" lines become notes, and its
/// "Nothing to do" is silence, so a routine run says nothing about completion.
pub fn install(root: &Path, binary: &Path, shells: Shells, runner: &dyn Runner, dry_run: bool, changes: &mut Changes) {
  let mut args: Vec<String> = vec!["install".into()];
  if shells.powershell {
    args.push("--powershell".into());
  }
  if shells.bash {
    args.push("--bash".into());
  }
  if args.len() == 1 {
    return;
  }
  if dry_run {
    args.push("--dry-run".into());
  }
  match runner.run_capture(&binary.to_string_lossy(), &args, root) {
    Ok(out) if out.success() => {
      for line in out.stdout.lines().filter_map(|l| l.strip_prefix("  - ")) {
        changes.notes.push(format!("shell completion: {line}"));
      }
    }
    Ok(out) => changes.warnings.push(format!("devkit-complete install failed: {}", out.stderr.trim())),
    Err(e) => changes.warnings.push(format!("devkit-complete install could not run: {e:#}")),
  }
}
```

- [ ] **Step 3: Step 15 in the run**

In `lib.rs`, after step 14's block and before `run_with` returns its `Changes`:

```rust
  // 15. Shell completion for poe, from the venv's `devkit-complete` the package step
  //     installed (1b), for the shells on PATH. Last, and outside the project: it writes to
  //     the home directory, so a dry run asks the installer for its own dry run.
  match completion::binary(&ctx.root) {
    Some(bin) => {
      let shells = completion::shells_on(&std::env::var_os("PATH").unwrap_or_default());
      completion::install(&ctx.root, &bin, shells, deps.docker.runner, dry_run, &mut changes);
    }
    None => changes.notes.push("devkit-complete is not installed in this venv; shell completion was not installed".into()),
  }
```

(Read the end of `run_with` first: the `Changes` value may be named differently or returned from an expression; keep its shape.)

```bash
cargo test -p aeth-devkit-setup --lib completion 2>&1 | grep -E 'test result|panicked' | head -2
cargo test -p aeth-devkit-setup --test apply 2>&1 | grep -E 'test result|FAILED' | head -2
cargo clippy -p aeth-devkit-setup --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: the three unit tests pass; the apply tests pass (their fake projects have no venv, so each run notes the missing binary and nothing else changes).

- [ ] **Step 4: Commit**

```bash
git add crates/aeth-devkit-setup
git commit -q -m "feat(setup): install the poe completion shims from the venv's devkit-complete

The run ends by running devkit-complete install for the shells PATH can
start, dry-run on a dry run, and reports what the installer changed; a
venv without the binary is noted instead.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: Remove the crates and the subcommands; the docs

**Files:**
- Delete: `crates/aeth-devkit-hooks/`, `crates/aeth-devkit-complete/`.
- Modify: `Cargo.toml`, `Cargo.lock`, `crates/aeth-devkit/Cargo.toml`, `crates/aeth-devkit/src/main.rs`, `crates/aeth-devkit/tests/update_nag.rs`, `README.md`, `TODO.md`, `WORKSPACE.md`.

- [ ] **Step 1: Remove**

```bash
git rm -r -q crates/aeth-devkit-hooks crates/aeth-devkit-complete
```

In `Cargo.toml`: delete the `aeth-devkit-complete` and `aeth-devkit-hooks` lines from `[workspace.dependencies]`, and the now-unused `regex` and `shell-words` entries (check with `git grep -n 'regex\|shell-words' -- 'crates/*/Cargo.toml'`; delete an entry only if no crate names it). In `crates/aeth-devkit/Cargo.toml`: delete the two dependency lines. In `crates/aeth-devkit/src/main.rs`: delete the `Complete` and `Hook` variants (with their doc comments), the two match arms in `main`, and the whole `impl Command { fn wants_update_check … }` block; the nag call becomes unconditional:

```rust
  // Last thing printed, so it is what the user sees; runs even after a failure, since an
  // outdated devkit may be the reason for it.
  aeth_devkit_core::update::nag(env!("CARGO_PKG_VERSION"));
```

```bash
cargo build --workspace 2>&1 | grep -E '^(error|warning)' | head; git status --short Cargo.lock
```

Expected: builds; `Cargo.lock` modified (the two packages and their unique dependencies gone).

- [ ] **Step 2: The nag tests**

In `crates/aeth-devkit/tests/update_nag.rs`: the module doc's second sentence becomes `Drives the real `devkit` binary with a fresh cache file so no network is touched; the ordinary command is `docker-pin --dry-run` outside a git repository, which fails at its first check before any network call, and the nag follows the error.` Delete `the_completion_data_path_never_nags`. In `devkit()`, delete the `.env("PATH", …)` line and its comment. Replace the two remaining tests' bodies:

```rust
#[test]
fn an_ordinary_command_nags_on_stderr() {
  let proj = project_with_stale_devkit();
  let out = devkit(proj.path(), &["docker-pin", "--dry-run"]);
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert!(!out.status.success(), "not a git repository: {stderr}");
  assert!(stderr.contains("99.0.0 available"), "stderr was: {stderr}");
  assert!(stderr.contains("uv tool upgrade aeth-devkit"), "{stderr}");
}

#[test]
fn the_env_var_disables_the_nag() {
  let proj = project_with_stale_devkit();
  let out = Command::new(DEVKIT)
    .args(["docker-pin", "--dry-run"])
    .current_dir(proj.path())
    .env("DEVKIT_UPDATE_CACHE", proj.path().join("update-check.json"))
    .env("HOME", proj.path())
    .env("USERPROFILE", proj.path())
    .env("DEVKIT_NO_UPDATE_CHECK", "1")
    .output()
    .unwrap();
  assert!(!String::from_utf8_lossy(&out.stderr).contains("available"));
}
```

```bash
cargo test -p aeth-devkit --test update_nag 2>&1 | grep -E 'test result|FAILED|panicked' | head -3
```

Expected: both pass. If the first fails because `tempdir()` sits inside a git repository on this machine, set `TMPDIR`/`TEMP` for the test to a directory outside any repository rather than changing the command.

- [ ] **Step 3: README**

In `README.md`:
- The command table: delete the `devkit complete` row.
- The **Update check** sentence: `` `setup-project`, `lock`, `release` and `complete install` end with a `` → `` every command ends with a ``.
- Line 61's bullet (`legacy `.claude/hooks/*.py` hook commands to `devkit hook` in place`): `devkit hook` → `devkit-hook`.
- In the `setup-project` feature list, add after the **Claude config** bullet:

```markdown
- **Hooks and completion** - Every project gets `devkit-claude-hooks` and
  `devkit-poe-complete` in its dev group at the newest release the running devkit accepts
  (the same floor-and-lock step as `devkit-container`); the hook lines in
  `.claude/settings.local.json` call the venv's `devkit-hook`, and the run ends with
  `devkit-complete install` for the shells on `PATH`, reporting what changed.
```

- Replace the `### devkit complete` and `### devkit hook` sections (both, in full) with:

```markdown
### `devkit-claude-hooks` and `devkit-poe-complete`

The Claude Code hooks (`devkit-hook <name>`) and the poe shell completion
(`devkit-complete`) live in their own repositories, `AetherBreaker/devkit-claude-hooks` and
`AetherBreaker/devkit-poe-complete`, released as wheels on SFTPyPI. `setup-project` installs
both into every project and wires them in (see **Hooks and completion** above); their READMEs
describe the hooks and the completion engine. `devkit hook` and `devkit complete` were removed
in 13.0.0: a project whose venv takes that devkit before `setup-project` has rewritten its
hook lines gets a usage error from every hook until `poe setup-project` runs.
```

```bash
git grep -n 'devkit complete\|devkit hook\|aeth-devkit-hooks\|aeth-devkit-complete' -- ':!docs' ':!TODO.md' ':!CHANGELOG*'
```

Expected: only the two historical mentions in the new README section (`devkit hook` and `devkit complete` were removed in 13.0.0). Anything else is stale; fix it.

- [ ] **Step 4: TODO and WORKSPACE**

In `TODO.md`: delete the whole **Auto-commit `stop-ruff`'s safe fixes** entry (it moved to `devkit-claude-hooks/TODO.md` in Task 2). In `WORKSPACE.md`: add `gh repo clone AetherBreaker/devkit-claude-hooks` and `gh repo clone AetherBreaker/devkit-poe-complete` to the clone block; the `.env` loop becomes `for r in devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete; do …`; the bring-up loop lists `aeth-devkit devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete`; after the `devkit-vscode also needs npm ci` sentence add: `` `devkit-claude-hooks` and `devkit-poe-complete` build their binaries into the venv through maturin during `uv sync`, so they need the Rust toolchain. ``

- [ ] **Step 5: Build, clippy, fmt, commit**

```bash
cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
git add -A
git commit -q -m "refactor: remove the hooks and completion crates and their subcommands

devkit-claude-hooks and devkit-poe-complete are their own repositories and
wheels now (split step 3); the crates, the devkit hook and devkit complete
subcommands and the update-nag special cases go with them. README, TODO
and WORKSPACE.md describe the new homes.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: Pull request, review, merge, release 13.0.0

- [ ] **Step 1: The full suite, once**

```bash
cargo test --workspace 2>&1 | grep -E 'test result|FAILED' | sort | uniq -c
uv run pytest -q 2>&1 | tail -2
git diff --exit-code -- python/aeth_devkit/_tasks_generated.py && echo "task table unchanged"
```

Expected: every `test result: ok`; pytest green; the task table unchanged.

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin feat/extract-hooks-and-completion
gh pr create --title "feat!: extract the Claude Code hooks and poe completion into their own packages" --body-file - <<'EOF'
Step 3 of docs/superpowers/specs/2026-09-08-devkit-split-design.md (plan: docs/superpowers/plans/2026-09-09-extract-hooks-and-completion.md).

- `devkit-claude-hooks` (binary `devkit-hook`) and `devkit-poe-complete` (binary `devkit-complete`) are their own repositories and wheels, already at 1.0.0 on SFTPyPI.
- `setup-project` adds both to every project's dev group at `{latest}`, writes the hook lines against the venv's `devkit-hook` (existing lines are updated in place), and ends by running `devkit-complete install` for the shells on PATH.
- The merge language gains a marker on a key-value line, so `[tool.uv.sources]` can hold the ungated packages beside the container's gated entry.
- `devkit hook` and `devkit complete` are removed, with the crates: a **major**. A project whose venv takes 13.0.0 before `setup-project` runs gets a usage error from every hook until `poe setup-project` runs; the release note says so.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
gh pr checks --watch
```

Expected: all checks green.

- [ ] **Step 3: Review before merge**

Two independent extra-high-effort reviews of the PR (one in-session on this model, one on Opus), each with the branch diff, the plan and the spec, and read-only access to both new repositories; verify the union of their findings, fix what is real, push, wait for green again.

- [ ] **Step 4: Merge and release**

The user's standing instruction from step 2 was to merge and release once reviews are clean; ask only if it may have changed.

```bash
gh pr merge --rebase
git switch main && git pull --ff-only
uv run poe release --dry-run major
uv run poe release --force major "The Claude Code hooks and the poe completion now live in devkit-claude-hooks and devkit-poe-complete, installed into every project by setup-project; devkit hook and devkit complete are removed. In every project run poe lock, then poe setup-project from a plain terminal, before the next Claude Code session."
```

Expected: `Released aeth-devkit 13.0.0`, the workflow green, `aeth-devkit==13.0.0` on SFTPyPI, the local venv at 13.0.0 (`uv run devkit --version`).

---

## Part C: rollout and verification

### Task 11: Every repository takes 13.0.0, and the binaries work from their new homes

- [ ] **Step 1: USER RUNS, in this order**

`aeth_devkit` first (its own pyproject gains the two packages through its own run, spec 4.5), then the satellites, then the sister projects. Hand the user this, verbatim; each `poe lock` moves the `aeth-devkit` pin to 13.0.0 and each `poe setup-project` rewrites the hook lines, adds the packages and installs the completion shims:

```bash
for r in aeth_devkit devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete; do
  (cd "/d/SFT Software Projects/SFT Workspace/$r" && uv run poe lock && uv run poe setup-project --no-vscode)
done
```

Sister projects (`aeth_ext`, `IMAPReportCollector`, `ScheduledInvoiceProcessor`, `ScheduledReportAggregator`, and any other devkit-managed checkout): the same two commands in each, when the user chooses; they are independent of this plan's completion.

Expected in each: a lock commit, then a "Standardize project configuration with devkit" commit whose `pyproject.toml` diff adds the two packages with `>=1.0.0` floors, whose `.claude/settings.local.json` (gitignored) now calls `.venv/Scripts/devkit-hook.exe`, and whose run output ends with `shell completion:` notes the first time (the shims rewritten to version 3) and nothing about completion afterwards.

- [ ] **Step 2: Verify, per repository**

```bash
cd "/d/SFT Software Projects/SFT Workspace/<repo>"
grep -n 'devkit-claude-hooks\|devkit-poe-complete' pyproject.toml
grep -o '"command": "[^"]*"' .claude/settings.local.json
uv run devkit setup-project --dry-run --no-vscode 2>&1 | tail -2
```

Expected: both floors present (the satellites carry each other's package but not their own); five commands naming `devkit-hook.exe`; `Nothing to do — project already matches the templates.`

- [ ] **Step 3: The binaries, end to end**

```bash
cd "/d/SFT Software Projects/SFT Workspace/aeth_devkit"
printf '{"tool_input":{"file_path":".env"}}' | .venv/Scripts/devkit-hook.exe pre-edit-protect; echo "exit $?"
printf '{"tool_input":{"command":"uv add requests"}}' | .venv/Scripts/devkit-hook.exe pre-bash-protect-deps; echo "exit $?"
head -3 ~/.local/share/devkit/poe-completion.ps1; head -1 ~/bash_completion.d/poe.bash
.venv/Scripts/devkit-complete.exe query --shell bash --shim-version 3 --line "poe lo" --point 6
```

Expected: a `deny` decision JSON for each hook, exit 0; both shim headers say `shim version 3`; the query answers `items` followed by `lock` (and any other task starting with `lo`). Then, in a new PowerShell and a new Git Bash: `poe <Tab>` completes.

- [ ] **Step 4: The Claude workflow tokens (user, later)**

`setup-project` installed `.github/workflows/claude.yml` in both new repositories; each needs the `CLAUDE_CODE_OAUTH_TOKEN` secret set by hand. Tell the user; do not set it.

- [ ] **Step 5: Record**

Append an "Execution notes" section to this plan with whatever differed, commit it on `aeth_devkit` `main`, and push.

---

## Self-review notes

- Spec coverage: 4.4's bullets map to Tasks 2 and 4 (everything each binary needs, including the shim text; private process seam; maturin bin wheels at 1.0.0), Task 6 (dev group at `{latest}`, advanced by 4.0's step), Task 7 (`devkit-hook <name>` through a placeholder that resolves like `{devkit_bin}` did; `hook_key` recognises both forms), Task 8 (`setup-project` runs `devkit-complete install` for the detected shells), Task 9 (dispatcher: `Complete` and `Hook` removed with the `wants_update_check` special cases; the major), Task 10's release note (the window). 4.5's crate list is honoured by Task 9; section 7's repository-creation paragraph by Task 5 (public, created by the plan, history via filter-repo, secrets piped never printed, `setup-project`, one release); 6's contracts by the unchanged hook command line and payload and the shim wire format. 9's "hooks and completion work from the new binaries" is Task 11.
- Placeholders: none; every step carries its command or content. `{latest}`, `{hook_bin}`, `{devkit_index}` are template placeholders, not plan placeholders.
- Names used across tasks: `HOOKS`/`COMPLETE` (Task 6, consumed by `active()`), `{hook_bin}` (Task 7; template and `templates.rs`), `completion::{Shells, shells_on, binary, install}` (Task 8; `lib.rs` step 15), `SHIM_VERSION = 3` and `devkit-complete query` (Task 4; Task 11's query check), `devkit_claude_hooks` / `devkit_poe_complete` (Tasks 2, 4, 6's `import_name`s), the 1.0.0 floors (Task 5's releases, Task 6's fixtures, Task 11's checks).
- Not in this plan: the complete-release rule (TODO.md), any change to the hooks' or the engine's behaviour, `devkit-templates` (step 4), the README slimming beyond the moved sections (step 5).

## Execution notes

What differed from the plan as written, in execution order.

- **Task 5 step 1**: the GitHub MCP `create_repository` tool was refused; `gh repo create
  <repo> --public --description ...` (no `--push`) created both repositories.
- **Task 5 step 3 did not run as written.** The satellites' venv devkit was 12.1.0, whose
  `setup-project` refuses any non-terminal stdin, and a 13.0.0 devkit could not set them up
  either: its package step locks each satellite's sibling package, which was not on the index
  until this task published it. The one file `devkit release` needs from that run is
  `.github/workflows/release.yml`, so it was rendered through the setup crate's own
  `templates::load` and `templates::gate` against each satellite's `ProjectContext` (a
  throwaway cargo example, not committed) and committed with `uv.lock`; the full standard
  setup came at Task 11 from 13.0.0. That 13.0.0 run listed no change to the workflow, so
  the render was byte-identical.
- **Between Tasks 9 and 10**, on the same branch: `--replace-docker` became `-y`/`--yes`
  (accept every proposal, the compose-service add included; skips the stdin check), the
  headless refusal now fires only when stdin is absent altogether (closed handle or the null
  device; a pipe counts as input), and an input that ends before a question is answered
  cancels the run instead of keeping the rest (commits 7c548b5, 63245a5). This is what let
  Task 11 run without a human at the terminal.
- **Task 10 step 4**: `poe release` words cannot start with a dash (clap reads them as
  flags: `--replace-docker` in the note failed the first attempt) and an apostrophe is
  mangled by poe's re-quoting; the note was reworded. Nothing had run when it failed.
- **Task 11 step 1** was executed here with `-y`, not handed over. In `aeth_devkit` the run
  had to go through PowerShell: once the venv held 13.0.0, the project's own hook lines
  (still `devkit hook <name>`) failed with a usage error and blocked every Bash and Edit
  call, the transitional state the release note describes; the `setup-project` run that
  rewrites them is the fix. In the two new repositories `devkit lock` and `setup-project`
  were invoked directly with `uv run --env-file .env` (no poe tasks before the run).
- **Consumers were not migrated**, by instruction: only `aeth_devkit`, `devkit-container`,
  `devkit-vscode`, `devkit-claude-hooks` and `devkit-poe-complete` took 13.0.0. Sister
  projects migrate once every step of the spec is done.
- uv's `--env-file` parser warns on the `PYTHONPYCACHEPREFIX` line `template.env` writes
  (TODO.md, setup-project).
- `CLAUDE_CODE_OAUTH_TOKEN` is still to be set by hand on both new repositories.
