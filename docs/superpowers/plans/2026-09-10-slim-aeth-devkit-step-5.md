# Slim aeth-devkit (split step 5) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the devkit split: rule on and remove the code the split left behind in `aeth-devkit` and `devkit-poe-complete`, retire `REMOVAL-CANDIDATES.md`, prune `TODO.md` and `README.md` to what `aeth-devkit` still contains, and release `aeth-devkit` 14.1.0 and `devkit-poe-complete` 1.1.0.

**Architecture:** No new code. Three removals in the `setup` crate (an unused package constant, the pre-Rust `.claude/hooks/*.py` hook migration) plus one behaviour change (`problem:` findings become `error:` on stderr and the run exits 1 after writing and committing what it could); two removals in `devkit-poe-complete` (the `tasks`/`args` subcommands with their formatter, and the devkit 7 to 12 profile-line migration); then docs and releases. Every task is its own commit directly on `main` of its repository (spec 7.1: "each its own commit on `main`"; no branch, no PR).

**Tech Stack:** Rust 2024 (clap 4, serde_json, toml_edit), uv, `poe release` (`devkit release`), git.

**Spec:** `docs/superpowers/specs/2026-09-08-devkit-split-design.md`, section 7.1 ("Step 5 as it stands after step 4") and 4.5. The rulings below were made by the owner on 2026-09-10 in the planning session and override the "rule on each" wording in 7.1; do not re-open them.

## Rulings (final, 2026-09-10)

| Candidate | Ruling |
|---|---|
| `packages::DEVKIT` constant (aeth-devkit) | Remove. Its two tests use `TEMPLATES` instead. |
| `Changes::problems` apart from `warnings` (aeth-devkit) | Rename the category to `error:`. Nothing in it is advisory, so nothing moves to `warning:`. `error:` lines go to stderr. A run that ends with any error still writes and commits what it can, then exits 1; a dry run with errors exits 1 too. |
| `legacy_hook_key` / `matches_key` (aeth-devkit) | Remove. `aeth_ext-2`, the one project with the old wiring, is a dead copy. `hook_key` keeps recognising both `devkit-hook <name>` and the pre-split `devkit hook <name>`. |
| `if-docker` and `if-docker-services` gates | Keep both. Not touched. |
| `devkit-complete tasks` / `args` (devkit-poe-complete) | Remove, with `format.rs` and their tests. |
| `OLD_DEVKIT_POWERSHELL_LINE` migration (devkit-poe-complete) | Remove, with its three tests. |
| `REMOVAL-CANDIDATES.md` | Delete once the above are done. |
| TODO.md consumer-project entries | Remove from this repo (the owner tracks downstream work elsewhere). |
| README "Migrating from `poe-tasks`" | Drop, with the `aeth-devkit>=7.0.0` example. |
| Release sizes | `aeth-devkit` minor, 14.1.0. `devkit-poe-complete` minor, 1.1.0 (same reasoning: nothing calls the removed subcommands). |
| Consumer migration | Not this plan. Struck out of the spec; the owner does it by hand. |

## Global Constraints

- Work on `main` in both repositories, one commit per task, pushed by the release at the end (Task 6 and Task 10) or by hand after the last docs commit. No branch, no PR.
- Repositories: `D:\SFT Software Projects\SFT Workspace\aeth_devkit` (this one) and `D:\SFT Software Projects\SFT Workspace\devkit-poe-complete`.
- Conventional Commits (`AGENTS.md`), trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`. A `fix` body states bug, cause, fix; none of these commits is a `fix`.
- The project's Bash hook refuses `uv add`, `uv remove`, `uv lock`. Use `uv sync`, `poe lock`, `setup-project` only. Nothing in this plan needs any of them.
- `poe release` notes: a multi-word final argument; no word may start with a dash; no apostrophes.
- Edit scripts go to the scratchpad and are run from there; never inline multi-line Python in a heredoc.
- On `main` the full suite runs normally: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check`, and in `aeth_devkit` also `uv run pytest`. Run the targeted tests named in each task while working; run the full set once before each release task.
- `README.md`'s feature reference is updated in the same commit as the behaviour it describes (Tasks 2 and 3 carry their README lines).
- Comments carry reasoning densely; no single-use helpers of four lines or fewer (Task 4 inlines one).
- `.env` holds live credentials in both repositories: never print or commit it.

---

## Part A: aeth-devkit code

### Task 1: Remove the devkit self-package constant

**Files:**
- Modify: `crates/aeth-devkit-setup/src/packages.rs` (constant at lines 63-67; tests `the_stub_venv_answers_from_the_map` ~line 588 and `the_probe_reads_the_workspace_venv` ~line 602)

**Interfaces:**
- Consumes: `TEMPLATES: DevkitPackage` (same file, line 58), `probe(&Path, &DevkitPackage)`, `StubVenv`.
- Produces: nothing; `DEVKIT` no longer exists.

- [ ] **Step 1: Delete the constant**

Remove these lines from `packages.rs`:

```rust
/// devkit itself; named by the `probe` unit test.
pub const DEVKIT: DevkitPackage = DevkitPackage {
  name: "aeth-devkit",
  import_name: "aeth_devkit",
};
```

- [ ] **Step 2: Point the two tests at `TEMPLATES`**

In `the_stub_venv_answers_from_the_map`, replace

```rust
    assert_eq!(venv.installed(root, &DEVKIT), None);
```

with

```rust
    assert_eq!(venv.installed(root, &TEMPLATES), None);
```

In `the_probe_reads_the_workspace_venv`, replace the leading comment and the probe line:

```rust
    // The workspace venv has aeth-devkit installed editable, which is the layout a
    // dist-info scan beside the package would miss; skipped where there is no venv (CI's
    // Rust job builds without one).
```

becomes

```rust
    // devkit-templates is in the workspace venv (the dev group carries it), so the probe
    // is exercised against a real site-packages; skipped where there is no venv (CI's
    // Rust job builds without one).
```

and

```rust
    let found = probe(&python, &DEVKIT).expect("aeth-devkit is installed in the workspace venv");
```

becomes

```rust
    let found = probe(&python, &TEMPLATES).expect("devkit-templates is installed in the workspace venv");
```

The following assertions (`parse_lenient(&found.version).is_some()`, `found.dir.join("__init__.py").is_file()`, the `devkit-nonexistent` probe) stay as they are: `devkit_templates` has an `__init__.py` and a version.

- [ ] **Step 3: Run the two tests**

Run: `cargo test -p aeth-devkit-setup --lib packages::tests`
Expected: PASS, and `cargo build -p aeth-devkit-setup` has no unused-constant warning.

- [ ] **Step 4: Confirm nothing else named it**

Run: `grep -rn '\bDEVKIT\b' crates/ --include='*.rs' | grep -v 'RUNNING_DEVKIT\|DEVKIT_\|CARGO_BIN_EXE_devkit\|Command::new(DEVKIT)'`
Expected: no lines (the `update_nag.rs` hits are a different, local constant).

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-setup/src/packages.rs
git commit -m "refactor(setup): drop the unused devkit self-package constant

Named devkit's own package for the beside-the-binary templates lookup, which left
with the templates in 14.0.0. Only two unit tests still used it as a package known
to be installed; they use devkit-templates now.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 2: `problem:` becomes `error:`, on stderr, with exit 1

**Files:**
- Modify: `crates/aeth-devkit-setup/src/changes.rs` (field and doc, lines 44-49, 65)
- Modify: `crates/aeth-devkit-setup/src/cli.rs` (exit-code doc lines 79-81; printing and exit at lines 193-210 and 235)
- Modify: `crates/aeth-devkit-setup/src/packages.rs` (`refuse_stale_lock`, lines 202-224)
- Modify: `crates/aeth-devkit-setup/src/docker/mod.rs` (lines 215, 272, 278, 313)
- Modify: `crates/aeth-devkit-setup/src/docker/compose_rules.rs` (`Outcome.problems` lines 45-50; pushes at 103, 162, 203; 299, 315, 393; tests 580-662)
- Modify: `crates/aeth-devkit-setup/tests/apply.rs` (`an_unsupported_compose_shape_is_a_problem_reported_on_every_dry_run`, lines 587-617)
- Modify: `crates/aeth-devkit-setup/tests/docker.rs` (lines 413-441)
- Modify: `crates/aeth-devkit-setup/tests/packages.rs` (lines 245-256, 529-537)
- Modify: `README.md` (lines 44-46, 122-129)

**Interfaces:**
- Produces: `Changes.errors: Vec<String>` (was `problems`); `compose_rules::Outcome.errors` (was `problems`); `cli::run` returns `ExitCode::from(1)` when `changes.errors` is non-empty and the commit did not fail.
- Exit codes after this task: 0 ok; 1 finished with `error:` findings (everything writable was written and, when committing, committed); 2 an error bubbled (unchanged); 3 commit failed (unchanged, takes precedence over 1).

- [ ] **Step 1: Rewrite the two existing tests to the new contract first**

In `tests/apply.rs`, replace the whole test `an_unsupported_compose_shape_is_a_problem_reported_on_every_dry_run` with:

```rust
#[test]
fn an_unsupported_compose_shape_is_an_error_on_every_run_and_exits_1() {
  // A listed service is a declared intent to have the compose file managed, so a shape
  // the engine cannot edit is an `error:`: the rest of the run still writes, and the exit
  // code says the project is not clean.
  let dir = make_project();
  let root = dir.path();
  let args = aeth_devkit_setup::cli::Args {
    root: root.to_path_buf(),
    templates_dir: Some(templates()),
    dry_run: true,
    no_commit: true,
    yes: false,
    vscode: false,
    no_vscode: true,
  };
  run(root, false).unwrap();
  assert_eq!(aeth_devkit_setup::cli::run(&args).unwrap(), std::process::ExitCode::SUCCESS);
  // An include-only aggregator is a supported layout: a warning, no error.
  write(root, "docker/compose.yaml", "include:\n  - path: other.yaml\n");
  let changes = run(root, true).unwrap();
  assert!(changes.errors.is_empty(), "{:?}", changes.errors);
  assert_eq!(changes.warnings.len(), 1, "{:?}", changes.warnings);
  // A shape the user could reformat is the error, on this run and the next.
  write(root, "docker/compose.yaml", "services: {imap-report-collector: {image: x}}\n");
  for _ in 0..2 {
    let changes = run(root, true).unwrap();
    assert!(changes.is_empty(), "no drift, only an error: {changes:?}");
    assert_eq!(changes.errors.len(), 1, "{:?}", changes.errors);
    assert_eq!(aeth_devkit_setup::cli::run(&args).unwrap(), std::process::ExitCode::from(1));
  }
}
```

`write` is the helper already used in this file. Only the dry-run path goes through `cli::run` here, as before: a plain `cli::run` uses the real `uv` and index client for the package step. The plain-run exit code is the same `exit` value returned at the end of `cli::run` (Step 5), so the dry-run assertion covers the computation.

In `tests/docker.rs`, lines 413-420 and 441: replace `problem:`s with `error:`s in the comment, and `changes.problems` with `changes.errors` (three occurrences).

In `tests/packages.rs`: rename `a_dry_run_reports_a_lock_on_another_devkit_as_a_problem` to `a_dry_run_reports_a_lock_on_another_devkit_as_an_error` and `a_stale_lock_is_one_problem_on_a_dry_run_and_stops_a_plain_run_before_any_write` to `a_stale_lock_is_one_error_on_a_dry_run_and_stops_a_plain_run_before_any_write`; replace every `changes.problems` in both with `changes.errors`.

- [ ] **Step 2: Run them to see the field does not exist**

Run: `cargo test -p aeth-devkit-setup --test apply an_unsupported_compose_shape`
Expected: compile error, `no field 'errors' on type Changes`.

- [ ] **Step 3: Rename the field and its doc in `changes.rs`**

Replace lines 44-49:

```rust
  /// `problem:` lines: drift the run saw in a file it manages but could not edit (a compose
  /// shape the engine does not model). Never written, never committed; reported on every
  /// run until it is fixed by hand, since a listed service is a declared intent to have the
  /// file managed.
  pub problems: Vec<String>,
```

with

```rust
  /// `error:` lines (stderr): drift the run saw in a file it manages but could not edit (a
  /// compose shape the engine does not model), or a dry run's stale lock. Never written,
  /// never committed; reported on every run until fixed by hand, since a listed service is
  /// a declared intent to have the file managed. The run still writes and commits the rest,
  /// then exits 1 (`cli::run`): a `warning:` would read as clean, and these are not.
  pub errors: Vec<String>,
```

and in `Changes::new`, `problems: Vec::new(),` becomes `errors: Vec::new(),`.

- [ ] **Step 4: Rename in `compose_rules.rs`, `docker/mod.rs`, `packages.rs`**

`compose_rules.rs`: the `Outcome` field

```rust
  /// Drift the engine saw but would not edit: a YAML shape it does not model (flow style,
  /// a list where the standard has a mapping). Splicing block lines into those is not
  /// YAML, so the user is told instead.
  pub problems: Vec<String>,
```

becomes `pub errors: Vec<String>,` with the same doc. Then every `out.problems`, `.problems`, `o.problems`, `t.problems` in that file (production at 103, 162, 203, 299, 315, 393 and the tests at 580-662) becomes `errors`. The comment at line 100 ("do not repeat the problem") becomes "do not repeat the error".

`docker/mod.rs`: `changes.problems.push(` at 215 and 272, `changes.problems.extend(o.problems)` at 278 and 313 become `changes.errors.push(` / `changes.errors.extend(o.errors)`.

`packages.rs`, `refuse_stale_lock`: the doc sentence "A plain run stops; a dry run records a problem (it must not read as clean), once, though the bootstrap and each `advance` all ask." becomes "A plain run stops; a dry run records an error (it must not read as clean), once, though the bootstrap and each `advance` all ask." and the body's `changes.problems` (two occurrences) becomes `changes.errors`.

Run: `grep -rn 'problem' crates/aeth-devkit-setup/src crates/aeth-devkit-setup/tests`
Expected: only `json_merge.rs:358` ("the user's problem to notice"), which is ordinary English and stays.

- [ ] **Step 5: Print and exit in `cli.rs`**

Replace the exit-code doc (lines 79-81):

```rust
/// Exit codes: 0 ok (a `problem:` line is a finding for a hand edit, not a failure), 3
/// commit failed (the template changes were rolled back). Errors bubble up for the caller
/// to print (exit 2).
```

with

```rust
/// Exit codes: 0 ok; 1 finished with `error:` findings (drift in a managed file the run
/// could not edit; everything else was written and committed); 3 commit failed (the
/// template changes were rolled back). Errors bubble up for the caller to print (exit 2).
```

Replace lines 193-210:

```rust
  for problem in &changes.problems {
    println!("problem: {problem}");
  }
  if changes.is_empty() {
    // No file differs from its merge base; undo the staging so the user's uncommitted
    // edits to managed files are back in place.
    if let Some(bases) = &bases {
      let _w = crate::interrupt::Writing::begin();
      aeth_devkit_core::commit::unstage_clean_base(&root, bases)?;
      crate::packages::resync_after_replay(&root, &runner, bases, &changes);
    }
    if changes.problems.is_empty() {
      println!("Nothing to do — project already matches the templates.");
      return Ok(ExitCode::SUCCESS);
    }
    // A problem is a finding on the repo, not something to write.
    println!("Nothing to write; the problem(s) above need a hand edit.");
    return Ok(ExitCode::SUCCESS);
  }
```

with

```rust
  for error in &changes.errors {
    eprintln!("error: {error}");
  }
  // An error is a finding on the repo, not something to write: the run finishes, and the
  // exit code carries the finding (a commit failure below still wins with 3).
  let exit = if changes.errors.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) };
  if changes.is_empty() {
    // No file differs from its merge base; undo the staging so the user's uncommitted
    // edits to managed files are back in place.
    if let Some(bases) = &bases {
      let _w = crate::interrupt::Writing::begin();
      aeth_devkit_core::commit::unstage_clean_base(&root, bases)?;
      crate::packages::resync_after_replay(&root, &runner, bases, &changes);
    }
    if changes.errors.is_empty() {
      println!("Nothing to do — project already matches the templates.");
    } else {
      println!("Nothing to write; the error(s) above need a hand edit.");
    }
    return Ok(exit);
  }
```

and the function's last line `Ok(ExitCode::SUCCESS)` (line 235) becomes `Ok(exit)`.

- [ ] **Step 6: Run the touched tests**

Run: `cargo test -p aeth-devkit-setup --test apply an_unsupported_compose_shape` then `cargo test -p aeth-devkit-setup --test docker` then `cargo test -p aeth-devkit-setup --test packages stale_lock` and `cargo test -p aeth-devkit-setup --lib compose_rules`
Expected: all PASS.

- [ ] **Step 7: README**

Lines 44-46 currently read:

```
answered the run is cancelled (exit 2, the changes rolled back when committing), never
finished on defaults. A run with no stdin at all (a closed handle or the null device) is
refused up front unless nothing will be asked: `-y` or `--dry-run`. A dry run exits 0; a
`problem:` line is a finding for a hand edit, not an exit code. Idempotent — a second run
is a byte-for-byte no-op.
```

Change the last two sentences to:

```
refused up front unless nothing will be asked: `-y` or `--dry-run`. An `error:` line
(stderr) is drift in a managed file the run could not edit; the run still writes and
commits everything else, then exits 1, dry or not. Otherwise exit 0. Idempotent — a second
run is a byte-for-byte no-op.
```

In the **Docker** bullet (lines 122-129), replace `problem:` with `error:` in the three places (`reported as a \`problem:\` rather than edited`, `A \`problem:\` is reported on every run`, `is a \`warning:\` on stderr instead, not a problem:`), the last becoming `is a \`warning:\` on stderr instead, not an error:`.

- [ ] **Step 8: Commit**

```bash
git add crates/aeth-devkit-setup README.md
git commit -m "feat(setup): report unfixable drift as error: on stderr and exit 1

problem: existed for --check's exit code, which went in 14.0.0, and what it named was
never advisory: a compose file the engine cannot edit, or a dry run's stale lock. As a
warning it would read as clean. It is an error: now, on stderr like warnings, and a run
that ends with one exits 1 after writing and committing everything it could (a commit
failure still exits 3).

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 3: Remove the pre-Rust hook-script migration

**Files:**
- Modify: `crates/aeth-devkit-setup/src/json_merge.rs` (`reconcile_hook` comment lines 395-396 and call at 403; `matches_key` lines 464-470; `legacy_hook_key` lines 489-500; tests at 617-627, 629-642, 655-677, 691-701, 703-720)
- Modify: `README.md` (**Migrations** bullet, lines 69-71)

**Interfaces:**
- Consumes: `hook_key(entry: &Value) -> Option<String>` (stays; recognises `devkit-hook <name>` and `… hook <name>`).
- Produces: nothing new; `matches_key` and `legacy_hook_key` no longer exist.

- [ ] **Step 1: Rewrite the tests that encoded the migration**

Delete these three tests outright: `legacy_python_hook_entries_are_replaced_not_duplicated`, `a_users_own_script_that_collides_with_a_template_name_is_still_theirs`, `legacy_entries_migrate_in_every_spelling`.

Rename `a_users_own_hook_script_is_not_claimed_by_the_fallback` to `a_users_own_hook_script_is_left_alone`; body unchanged (a `.claude/hooks/my_custom_check.py` command survives and the template entries are added beside it, which is still the wanted behaviour).

Replace `a_legacy_entry_in_a_later_group_is_migrated_not_left_to_run_twice` with the same property stated on the pre-split spelling, which `hook_key` still recognises:

```rust
  #[test]
  fn a_pre_split_entry_in_a_later_group_is_migrated_not_left_to_run_twice() {
    // One hook per group object is an ordinary hand-written shape. Reconciling only the
    // first matcher-matching group left the second one holding the old spelling, so the
    // hook ran twice on every Stop — and re-running never healed it, because the state was
    // stable.
    let mut target = json!({"Stop": [
      {"hooks": [{"type": "command", "command": "uv run devkit hook stop-ruff"}]},
      {"hooks": [{"type": "command", "command": "uv run devkit hook stop-pyright"}]}
    ]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    let commands: Vec<String> = target["Stop"]
      .as_array()
      .unwrap()
      .iter()
      .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
      .map(|e| e["command"].as_str().unwrap_or("").to_string())
      .collect();
    assert_eq!(commands.iter().filter(|c| c.contains("stop-pyright")).count(), 1, "{commands:?}");
    assert!(!commands.iter().any(|c| c.contains(" hook ")), "old spelling left behind: {commands:?}");
  }
```

- [ ] **Step 2: Run the module's tests to see the current state**

Run: `cargo test -p aeth-devkit-setup --lib json_merge`
Expected: PASS (the migration code is still there; the rewritten test passes on the `devkit hook` path). This confirms the new test is valid before the removal.

- [ ] **Step 3: Remove the code**

Delete `matches_key` (with its doc comment, lines 464-470) and `legacy_hook_key` (with its doc comment, lines 489-500). In `reconcile_hook`, the comment

```rust
  // Every (group index, entry index) whose entry means this same hook — including the legacy
  // `.claude/hooks/<snake>.py` spelling, which is the same hook by another name.
```

becomes

```rust
  // Every (group index, entry index) whose entry means this same hook, in either spelling
  // `hook_key` accepts.
```

and `if matches_key(e, key) {` becomes `if hook_key(e).as_deref() == Some(key) {`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p aeth-devkit-setup --lib json_merge` and `cargo test -p aeth-devkit-setup --test apply`
Expected: PASS; `cargo clippy -p aeth-devkit-setup --all-targets -- -D warnings` clean (no dead code).

- [ ] **Step 5: README**

The **Migrations** bullet (line 69-71):

```
- **Migrations** - `poe_tasks:tasks` include_script → `aeth_devkit:tasks`; drops
  `tool.ruff.extend` / `tool.pyright.extends` pointing at a parent pyproject; rewrites
  legacy `.claude/hooks/*.py` and `devkit hook` hook commands to `devkit-hook` in place.
```

becomes

```
- **Migrations** - `poe_tasks:tasks` include_script → `aeth_devkit:tasks`; drops
  `tool.ruff.extend` / `tool.pyright.extends` pointing at a parent pyproject; rewrites
  pre-split `devkit hook` hook commands to `devkit-hook` in place.
```

- [ ] **Step 6: Commit**

```bash
git add crates/aeth-devkit-setup/src/json_merge.rs README.md
git commit -m "refactor(setup): drop the pre-Rust hook-script migration

The hook merge recognised python .claude/hooks/<name>.py command lines so the Rust
hooks replaced them in place. Every live project has been through setup-project since
the Rust hooks arrived; the one repository still wired that way is a dead copy.
hook_key alone covers what exists: devkit-hook and the pre-split devkit hook spelling.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

## Part B: devkit-poe-complete

All paths in this part are relative to `D:\SFT Software Projects\SFT Workspace\devkit-poe-complete`. Check `git status` is clean and the branch is `main` before starting.

### Task 4: Remove the `tasks` and `args` subcommands

**Files:**
- Modify: `src/lib.rs` (`pub mod format;` line 20; `Command::Tasks`/`Command::Args` lines 52-62; `output` doc and arms lines 113-130; `project_dir` lines 147-156 and its call at 186; `run_real` doc lines 245-246)
- Delete: `src/format.rs`
- Modify: `tests/cli.rs` (comment lines 6-7; tests at 53-118)
- Rename + modify: `tests/format_cache.rs` → `tests/cache.rs` (drop lines 4, 8-95: the `format` import, the `arg` helper, the section banner and five format tests; trim the `resolve` import)
- Modify: `README.md` (lines 9-11, 22-23)

**Interfaces:**
- Consumes: `cache::resolve_cached` (still used by `engine`), `scripts::{BASH, POWERSHELL}`, `engine::complete`, `wire::render`.
- Produces: `Command` has variants `Script`, `Install`, `Query` only; `output(&Command, bool)` handles those three; `format` module gone.

- [ ] **Step 1: Delete the tests that name the subcommands**

`tests/cli.rs`: delete `tasks_and_args_print_nothing_for_a_directory_without_a_pyproject`, `tasks_and_args_for_a_real_project_directory`, and `an_empty_dir_argument_parses` (with its doc comment). In `scripts_register_for_poe_and_call_devkit_for_data`, the comment

```rust
    // The shims ask one question now. `tasks` and `args` remain as subcommands for shims
    // installed by an older devkit, but the shipped shims no longer use them.
```

becomes

```rust
    // The shims ask one question, `query`; the old two-question protocol is gone.
```

`git mv tests/format_cache.rs tests/cache.rs`, then in `tests/cache.rs` delete the import `use devkit_poe_complete::format::{describe_task_args, list_tasks};`, change `use devkit_poe_complete::resolve::{Resolved, Task, TaskArg, resolve};` to `use devkit_poe_complete::resolve::{Resolved, resolve};`, delete the `arg(...)` helper (lines 8-16), the banner `// ---- format: byte-compatible with ...` and the five tests `list_tasks_is_one_space_separated_line`, `describe_task_args_uses_poes_tab_separated_format`, `help_is_first_line_only_truncated_and_escaped`, `choices_with_quotes_use_shell_quote_splicing`, `a_task_with_no_args_describes_as_nothing`. Everything from the `// ---- cache ----` banner down stays, including `resolved_tasks_match_poe_list_tasks_for_this_repo` (it compares the resolver with poe's own hidden `_list_tasks`, not with our subcommand).

- [ ] **Step 2: Run to see the tests still compile against the old code**

Run: `cargo test --test cli --test cache`
Expected: PASS (nothing removed yet on the production side).

- [ ] **Step 3: Remove the variants, the arms, the module and the helper**

`src/lib.rs`:

Delete `pub mod format;` (line 20).

Delete from `enum Command`:

```rust
  /// Print task names on one line (replaces `poe _list_tasks`).
  Tasks {
    /// Project directory (defaults to the current directory; an empty string means the same).
    dir: Option<String>,
  },
  /// Print a task's arguments, tab-separated (replaces `poe _describe_task_args`).
  Args {
    task: String,
    /// Project directory (defaults to the current directory; an empty string means the same).
    dir: Option<String>,
  },
```

Change the `output` doc

```rust
/// What to print for the data and script subcommands. Separated from the I/O so it can be
/// tested without a shell. (`install` has side effects and goes through [`run_install`].)
```

to

```rust
/// What to print for `script` and `query`. Separated from the I/O so it can be tested
/// without a shell. (`install` has side effects and goes through [`run_install`].)
```

and delete the two arms:

```rust
    Command::Tasks { dir } => {
      let root = project_dir(dir.as_deref());
      cache::resolve_cached(&root, &SystemRunner, no_cache)
        .map(|r| format::list_tasks(&r.tasks))
        .unwrap_or_default()
    }
    Command::Args { task, dir } => {
      let root = project_dir(dir.as_deref());
      cache::resolve_cached(&root, &SystemRunner, no_cache)
        .ok()
        .and_then(|r| r.tasks.into_iter().find(|t| &t.name == task))
        .map(|t| format::describe_task_args(&t))
        .unwrap_or_default()
    }
```

Delete `project_dir` with its doc comment (lines 147-156). In `build_request`, replace

```rust
  let cwd = project_dir(None);
```

with

```rust
  let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
```

(`PathBuf` is still imported for `home_dir`.)

Change the `run_real` doc

```rust
/// Production entry. The data/script subcommands never exit non-zero — a completer that
/// errors breaks the shell — but `install` is an ordinary command and may.
```

to

```rust
/// Production entry. `script` and `query` never exit non-zero — a completer that errors
/// breaks the shell — but `install` is an ordinary command and may.
```

Then `git rm src/format.rs`.

- [ ] **Step 4: Build, lint, test**

Run: `cargo clippy --all-targets -- -D warnings` then `cargo test`
Expected: clean and PASS. If `cache::resolve_cached`'s `Resolved.tasks[*].args` is now only read by `engine`, that is fine; if clippy reports a dead field on `TaskArg`, keep the field (the engine's arguments completion reads it) and check the warning's location before touching anything.

- [ ] **Step 5: README**

Lines 9-11:

```
Subcommands: `query` (the per-Tab request, called by the shims), `tasks [DIR]` and `args
<TASK> [DIR]` (retained for shims installed by an older devkit), `script
--powershell|--bash`, `install --powershell --bash [--dry-run]`; global `--no-cache`.
```

become

```
Subcommands: `query` (the per-Tab request, called by the shims), `script
--powershell|--bash`, `install --powershell --bash [--dry-run]`; global `--no-cache`.
```

Lines 22-23, in the **Caching** bullet, `and the data subcommands never exit non-zero — a failing completer would break the shell.` becomes `and \`query\` never exits non-zero — a failing completer would break the shell.`

- [ ] **Step 6: Commit**

```bash
git add -A src tests README.md
git commit -m "refactor: drop the tasks and args subcommands nothing can call

They answered the two questions the pre-13.0.0 shell scripts asked through devkit
complete, which no longer exists; the shipped shims ask query only. The poe-format
renderer behind them goes with them.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 5: Remove the devkit 7 to 12 profile-line migration

**Files:**
- Modify: `src/install.rs` (constant lines 23-26; branch lines 64-74 inside `patch_profile`)
- Modify: `tests/install.rs` (tests at 130-141, 190-204, 206-217)
- Modify: `README.md` (**Install** bullet, line 26-27)

**Interfaces:**
- Produces: `patch_profile(original: Option<&str>) -> (String, Vec<String>)` unchanged in signature; it now removes only poe's own line and adds devkit's.

- [ ] **Step 1: Delete the three tests**

In `tests/install.rs` delete `migrating_a_profile_replaces_the_previous_devkit_line`, `migration_leaves_a_comment_mentioning_the_old_command_alone`, and `migration_still_removes_the_real_command_with_a_call_operator`. `the_profile_line_no_longer_invokes_devkit` stays: it asserts the current line's shape, not the migration.

- [ ] **Step 2: Remove the code**

In `src/install.rs` delete

```rust
/// Fragment identifying the *previous* devkit registration, which this one replaces. That
/// line ran devkit at every shell start merely to fetch the script text, which is precisely
/// what made a global install mandatory.
const OLD_DEVKIT_POWERSHELL_LINE: &str = "devkit complete script --powershell";
```

and, inside `patch_profile`'s loop, the block

```rust
    // Migration: an earlier devkit put a line here that shelled out at every shell start.
    //
    // `starts_with` on the normalised form, not `contains` on the raw line: a `contains`
    // would also match a line that merely mentions the command -- a commented-out note or
    // an instruction in the user's own profile -- and silently delete it. `drop` has
    // already stripped a leading `&` and lowercased, so both call-operator and
    // differently-cased spellings still match.
    if drop.starts_with(OLD_DEVKIT_POWERSHELL_LINE) {
      log.push(format!("removed the previous devkit registration: {t}"));
      continue;
    }
```

The `drop` normalisation stays: the poe-line check above it uses it, and the `starts_with`-not-`contains` reasoning still applies to that check. Move the two sentences that carry it onto the poe check:

```rust
    // `starts_with` on the normalised form, not `contains` on the raw line: a `contains`
    // would also match a line that merely mentions the command (a commented-out note in
    // the user's own profile) and silently delete it. `drop` has stripped a leading `&`
    // and lowercased, so call-operator and differently-cased spellings still match.
    if drop.starts_with(POE_POWERSHELL_LINE) {
```

- [ ] **Step 3: Build, lint, test**

Run: `cargo clippy --all-targets -- -D warnings` then `cargo test --test install`
Expected: clean and PASS.

- [ ] **Step 4: README**

The **Install** bullet: `(also removing poe's own slow registration, and any previous devkit line)` becomes `(also removing poe's own slow registration)`.

- [ ] **Step 5: Commit**

```bash
git add src/install.rs tests/install.rs README.md
git commit -m "refactor(install): drop the devkit 7 to 12 profile-line migration

install removed the profile line devkit 7.x wrote (devkit complete script --powershell
piped to Invoke-Expression). Every profile on the fleet has been through install since;
a machine mirrored from WORKSPACE.md never had the line.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 6: Release devkit-poe-complete 1.1.0

- [ ] **Step 1: Full verification on `main`**

Run, in the devkit-poe-complete root: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `uv sync`, `uv run devkit-complete --version`.
Expected: all clean; the version prints 1.0.0 (pre-release).

- [ ] **Step 2: Release**

Run: `uv run poe release minor "drop the tasks and args subcommands and the pre 13 profile line migration; nothing calls either"`
Expected: bumps to 1.1.0, commits, tags `v1.1.0`, pushes, creates the GitHub release, waits for the workflow, and reports `Released`. The command pushes the Task 4 and 5 commits along with the bump.

- [ ] **Step 3: Confirm**

`devkit release` waits for the workflow and verifies the wheel on the index before it prints `Released`; that line is the confirmation. Run `git log --oneline -4` and `git status`: the bump commit is on top of the Task 4 and 5 commits, the tree is clean, and `main` is not ahead of `origin/main`.

## Part C: aeth-devkit docs and release

Back in `D:\SFT Software Projects\SFT Workspace\aeth_devkit`.

### Task 7: Delete `REMOVAL-CANDIDATES.md`

**Files:**
- Delete: `REMOVAL-CANDIDATES.md`

- [ ] **Step 1: Check every entry is ruled on**

Read the file once more. Its entries are exactly the seven in the Rulings table; the `devkit-claude-hooks` section is empty. Nothing else references the file except the step 4 plan and the spec, which are history.

- [ ] **Step 2: Remove and commit**

```bash
git rm REMOVAL-CANDIDATES.md
git commit -m "chore: delete REMOVAL-CANDIDATES.md, every entry ruled on

It collected what the split left behind. Rulings (2026-09-10): the self-package
constant, the pre-Rust hook migration, and devkit-poe-complete's tasks and args
subcommands and 7.x profile-line migration are removed; problem: is now error: with
exit 1; both Docker gates stay.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 8: Prune `TODO.md`

**Files:**
- Modify: `TODO.md`

- [ ] **Step 1: Delete these entries**

Under **setup-project**:
- `- [ ] Sister-project Docker migration (after the aeth-devkit major ...` (the whole entry through `... (on their own TODO lists, high priority).`)
- `- [ ] IMAPReportCollector: \`[tool.docker].mkdirs = [""]\` is a data bug ...`
- `- [x] \`if-docker\` conditional marker for template tables ... Done on \`feat/agent-config\`.` (released; the file's header says checked items go once released)

Under **Release / packaging**:
- `- [ ] Release 7.0.0 (\`aeth-devkit\`), then migrate downstream projects per README.`
- `- [ ] Now that \`vscode-extension-v1\` has shipped: delete \`.vscode/extension/\` and \`install.ps1\` from aeth_ext and aeth_ext-2, ...` (the whole entry)

Under **Script migration to Rust**:
- the three `[x]` entries (`lock.sh`, `docker-pin-latest.sh` with all its sub-bullets, `release.sh`); only `- [ ] \`rescind-release.sh\`` remains. Change the section's lead sentence to: `The one shell script left; it becomes its own crate under \`crates/\` and the \`devkit\` binary dispatches, like the others did.`

Under **Housekeeping**:
- `- [ ] \`uv run ruff format python\` — ...` (`ruff format --check python` reports both files formatted; the drift is gone)
- `- [ ] IMAPReportCollector: \`tool.coverage.run.source_pkgs\` ...`
- `- [ ] Rename remaining \`master\` default branches if desired: ...`

Keep everything else, in particular: the `ci.yml` templating entry, the managed-entry labelling entry, the untracked-committable-file entry, the shell-detection entry, the marker-validation entry, the Dockerfile customisation entry (spec 4.1's opt-out), the `--python-dir` entry, the complete-release rule, the release-watch TUI entry with its sub-bullets, the two VS Code consent entries, the `code-insiders`/`cursor` entry, the devkit-vscode release-workflow entry (spec 4.3), the `release-workflow = false` entry, the per-key opt-out note, and the system-level `init.defaultBranch` entry (a machine setting, not consumer work).

- [ ] **Step 2: Check the result**

Run: `grep -n 'aeth_ext\|IMAPReportCollector\|ScheduledInvoice\|ScheduledReport\|timeclock\|apscheduler\|poe-tasks\|7\.0\.0\|\[x\]' TODO.md`
Expected: no lines. The **Housekeeping** section keeps its heading if the `init.defaultBranch` entry is its only remaining item.

- [ ] **Step 3: Commit**

```bash
git add TODO.md
git commit -m "docs(todo): drop entries the split settled and the downstream project work

The released Rust migrations and the if-docker marker go per the file's own rule; the
7.0.0 migration entry and the ruff drift are stale; the sister-project Docker migration,
the aeth_ext extension-folder cleanup, and the IMAPReportCollector and default-branch
items are downstream work the owner does by hand and tracks outside this repo.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 9: README describes only what `aeth-devkit` still contains

**Files:**
- Modify: `README.md` (**Using it in a project** lines 382-397; **Migrating from `poe-tasks`** lines 399-408; **Development** lines 410-423)

- [ ] **Step 1: Using it in a project**

In the TOML block, `  dev = ["aeth-devkit>=7.0.0"]` becomes `  dev = ["aeth-devkit"]`. After the block, replace `Then \`uv sync\` and \`poe setup-project\`.` with:

```
Then `uv sync` and `poe setup-project`, which adds `devkit-templates`, `devkit-claude-hooks`
and `devkit-poe-complete` to the dev group and locks them (see **Devkit packages**), and
`poe lock` whenever the `aeth-devkit` pin should move.
```

- [ ] **Step 2: Delete "Migrating from `poe-tasks`"**

Remove the heading and its two paragraphs (the numbered list and the `poe lock keeps the pin current ...` paragraph). Nothing in them is lost: the **`devkit lock`** feature section's **Index resolution** bullet already says the index comes from `tool.uv.sources` + `[[tool.uv.index]]` with a PyPI fallback, and the `poe_tasks:tasks` rewrite is in the **Migrations** bullet.

- [ ] **Step 3: Development**

The layout paragraph:

```
Layout: `crates/aeth-devkit-core` (shared git/process/pyproject/index helpers),
`crates/aeth-devkit-setup` and `crates/aeth-devkit-lock` (one command each, library +
dev binary), `crates/aeth-devkit` (the shipped `devkit` dispatcher),
`python/aeth_devkit` (poe tasks, remaining shell scripts). The setup crate's tests render
the snapshot under `crates/aeth-devkit-setup/tests/fixtures/templates`; to render a
templates checkout instead, pass `--templates-dir` or set `DEVKIT_TEMPLATES`. CI's `render`
job dry-runs the newest released templates through the working-tree binary.
```

becomes

```
Layout: `crates/aeth-devkit-core` (shared git/process/pyproject/index helpers),
`crates/aeth-devkit-setup`, `-lock`, `-release` and `-pin` (one command each, library +
dev binary), `crates/aeth-devkit` (the shipped `devkit` dispatcher), `python/aeth_devkit`
(the poe task table and the one remaining shell script). The setup crate's tests render
the snapshot under `crates/aeth-devkit-setup/tests/fixtures/templates`; to render a
templates checkout instead, pass `--templates-dir` or set `DEVKIT_TEMPLATES`. CI's
`Templates:` job dry-runs the newest released templates through the working-tree binary.
```

- [ ] **Step 4: Read the four satellite sections once**

`### \`devkit-container\``, `### VS Code extension`, `### \`devkit-claude-hooks\` and \`devkit-poe-complete\``, `### \`devkit-templates\``: each already states what `setup-project` does with the package and points at the repository. Leave them. If any sentence describes the satellite binary's own subcommands or internals rather than what `setup-project` does with it, cut that sentence; the target is the repository's README.

- [ ] **Step 5: Commit**

```bash
git add README.md
git commit -m "docs(readme): describe only what aeth-devkit still contains

Drops the poe-tasks migration (no live project is on poe-tasks) and its 7.0.0 example,
lists every crate in the layout, and names CI's Templates job as it is named.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 10: Release aeth-devkit 14.1.0

- [ ] **Step 1: Full verification on `main`**

Run: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `uv run pytest`, `uv run maturin develop`, `uv run devkit --version`.
Expected: all clean; version 14.0.0 (pre-release). Also `uv run devkit setup-project --dry-run` in this repo exits 0 with no `error:` line; a proposed advance of `devkit-poe-complete` to 1.1.0 (Task 6 published it) is the one change it may list, and a plain `uv run poe setup-project` before the release takes it and commits (that commit rides along with the release push).

- [ ] **Step 2: Release**

Run: `uv run poe release minor "setup-project reports drift it cannot edit as error on stderr and exits 1 after writing what it can; drops the pre Rust hook script migration; docs describe only what this repository still contains"`
Expected: 14.1.0 tagged, pushed, released, workflow green, wheel on SFTPyPI. The release pushes the Task 1, 2, 3, 7, 8, 9 commits.

- [ ] **Step 3: Execution notes**

Append an `## Execution notes` section to this plan recording anything that deviated (a test that needed a different fixture file in Task 2 Step 1, a clippy finding in Task 4 Step 4, the actual release versions and times), and commit it:

```bash
git add docs/superpowers/plans/2026-09-10-slim-aeth-devkit-step-5.md
git commit -m "docs(plans): execution notes for split step 5

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git push
```

## Self-review against spec 7.1

- Item 1 (constants and leftover code): Tasks 1-5 and 7. Every `REMOVAL-CANDIDATES.md` entry has a ruling; the gates are kept, so no devkit-templates release.
- Item 2 (TODO.md): Task 8. The 4.1 opt-out entry ("Dockerfile customisation") and the 4.3 entry ("devkit-vscode: a stronger release workflow") are present and kept; the stale 7.0.0 entry goes; the `ci.yml` entry stays.
- Item 3 (README.md): Tasks 2, 3 and 9. Satellite sections already point at their repositories.
- Item 4 (release): Task 10, minor. Consumer migration is struck out of the spec and not here.
- "Decided: the bake stays": nothing here touches `build.rs`.
- Nothing in this plan runs `uv add`, `uv remove` or `uv lock`.
