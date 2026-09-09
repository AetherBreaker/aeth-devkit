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

- (entries added as Part B proceeds)
