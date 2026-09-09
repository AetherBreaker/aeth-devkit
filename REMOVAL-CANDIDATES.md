# Removal candidates

Things noticed while executing the split (step 3 onward) that look unused or not worth
their complexity. Nothing here has been removed; each entry is for review. Entries name
the repository they live in, the evidence, and what removal would cost.

## devkit-poe-complete

- **`devkit-complete tasks [DIR]` and `args <TASK> [DIR]`** (`src/lib.rs`, `Command::Tasks`,
  `Command::Args`, `format::list_tasks`, `format::describe_task_args`, the `dir` handling in
  `project_dir`, their tests in `tests/cli.rs` and `tests/format_cache.rs`). Kept "for shims
  installed by an older devkit": those shims called `devkit complete tasks`, and `devkit
  complete` stops existing with 13.0.0, so no installed shim can reach these subcommands any
  more. The v3 shims use `query` only. Removal cost: none once 13.0.0 is out and
  `devkit-complete install` has replaced the shims (`setup-project` runs it).
- **`OLD_DEVKIT_POWERSHELL_LINE` migration** (`src/install.rs`): removes the legacy
  `devkit complete script --powershell | Invoke-Expression` line from `$PROFILE`, a design
  retired in devkit 7.x. Worth keeping only until every machine's profile has been through
  `devkit-complete install` once; after that it is a dead branch with three tests.

## devkit-claude-hooks

- (none yet)

## aeth-devkit

- **`legacy_hook_key` and `matches_key`** (`crates/aeth-devkit-setup/src/json_merge.rs`):
  recognise the pre-Rust `.claude/hooks/<name>.py` script lines (several path spellings) so
  the hook merge updates them in place. Every devkit-managed project has been through
  `setup-project` since the Rust hooks arrived (August 2026), so the branch has nothing left to
  migrate; `hook_key` alone (both `devkit-hook` and the old `devkit hook` spelling) covers
  what exists. Removal cost: a project that skipped every run since then would get a second
  hook entry beside its Python one, visible in the diff review.
- **Two Docker gates, `if-docker` and `if-docker-services`** (`toml_merge.rs`,
  `pyproject.template.toml`): `if-docker` merges `[tool.docker]` when the project has
  services *or* Docker files; `if-docker-services` gates the container dependency and its
  source on services alone. One template table uses the first, two entries the second. If
  "Docker files without services" stops being a state worth supporting (it already produces a
  warning every run), one gate would do.
