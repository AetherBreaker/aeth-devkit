# `aeth-devkit` workspace, rename, and `devkit lock` — design spec

Status: READY FOR IMPLEMENTATION

## Purpose

`poe_tasks` is a personal toolkit of project-maintenance commands used across all of the
author's coding projects. It is being renamed to **`aeth-devkit`** (shipped command:
**`devkit`**), restructured into a Cargo workspace so each command is its own crate, and
its shell scripts are being migrated to Rust one at a time. This pass covers:

1. The workspace restructure (dispatcher crate + core crate + one crate per command).
2. The rename (crates, binary, wheel/dist, Python package, env vars, template layers).
3. Porting `lock.sh` to Rust as `devkit lock`.

Out of scope for this pass: porting `docker-pin-latest.sh`, `release.sh`,
`rescind-release.sh` (they move to the new Python package unchanged and stay wrapped by
poe tasks); renaming the GitHub repository (the author does that by hand).

## Naming

| Thing | Name |
| --- | --- |
| Wheel / dist / PyPI name | `aeth-devkit` |
| Python package (import name) | `aeth_devkit` → `python/aeth_devkit/` |
| Shipped executable | `devkit` (`[project.scripts] devkit`) |
| Dispatcher crate (bin `devkit`) | `aeth-devkit` |
| Shared library crate | `aeth-devkit-core` (lib `aeth_devkit_core`) |
| Setup command crate | `aeth-devkit-setup` (lib `aeth_devkit_setup`, dev bin `devkit-setup`) |
| Lock command crate | `aeth-devkit-lock` (lib `aeth_devkit_lock`, dev bin `devkit-lock`) |
| Templates env override | `DEVKIT_TEMPLATES` (was `SFT_SETUP_TEMPLATES`) |
| Project-specific template layers | `devkit.gitignore`, `devkit.dockerignore` (were `sft.*`) |
| Setup commit subject | `Standardize project configuration with devkit` |
| Lock commit subject | `Update uv.lock` (unchanged) |
| Version | `7.0.0` (continues the `poe-tasks` line so "latest" comparisons stay monotonic) |

All "SFT project" wording in docs/help text is dropped. The SFTPyPI index and publish
URLs in **this repo's own** `pyproject.toml` stay: that is where the package is published.
Test fixture content (`aeth-ext[sftp, …]`, an index named `SFTPyPI`) stays: it is test
input, not branding.

## Workspace layout

```
Cargo.toml                    [workspace] members = ["crates/*"]; [workspace.dependencies];
                              [profile.release] strip = true, incremental = true
.cargo/config.toml            rust-lld as linker for x86_64-pc-windows-msvc
crates/aeth-devkit-core/      git, process, pyproject, index, version
crates/aeth-devkit-setup/     today's src/*.rs + src/main.rs (dev bin); tests/ + fixtures move here
crates/aeth-devkit-lock/      lib + src/main.rs (dev bin); tests/
crates/aeth-devkit/           bin `devkit`: clap subcommand enum → each crate's run()
python/aeth_devkit/           __init__.py (poe tasks), scripts/*.sh, templates/
```

### Command crate contract

Every command crate exposes:

```rust
#[derive(clap::Parser, clap::Args, Debug)]
pub struct Args { /* all flags for the command */ }
pub fn run(args: &Args) -> anyhow::Result<std::process::ExitCode>;
```

- `src/main.rs` (dev bin) is `Args::parse()` → `run()` → exit code. It exists so
  `cargo run -p aeth-devkit-lock -- …` builds and links only that command while iterating.
- The dispatcher's subcommand enum wraps the same `Args` struct, so the shipped and dev
  interfaces cannot drift.
- Only the `devkit` binary ships in the wheel (`[tool.maturin] bindings = "bin"` with
  `manifest-path = "crates/aeth-devkit/Cargo.toml"`).

### Dispatcher (`crates/aeth-devkit`)

```
devkit <COMMAND>
  setup-project   Standardize the project's configuration from the shipped templates
  lock            Bump the aeth-devkit pin, uv sync, commit uv.lock
```

`devkit setup-project` is exactly today's `sft-setup` CLI (`--root`, `--templates-dir`,
`--dry-run`, `--check`, `--no-commit`). The dispatcher's `main` calls the chosen crate's
`run()` and maps `Err` to `error: …` on stderr with exit 2.

### Core crate (`crates/aeth-devkit-core`)

- `git` — moved from setup. `is_git_tracked`, `commit_paths(root, paths, message)`,
  `is_dirty(root, paths)`. Setup's `commit_changes` stays in setup, built on
  `commit_paths`.
- `process` — `Runner` trait with one impl that executes real commands (`Command`) and
  a recording impl for tests. Methods: `run_inherit(program, args, cwd) -> ExitStatus` and
  `output(program, args, cwd) -> Output`.
- `pyproject` — `find_requirement(doc, name) -> Option<Requirement>` scanning
  `project.dependencies`, `project.optional-dependencies.*`, `dependency-groups.*`
  (skipping `{include-group = …}` tables); `set_requirement_version(req_str, version)`
  which rewrites only the version token, preserving name, extras, operator, and markers;
  `index_url_for(doc, name) -> Option<String>` resolving `tool.uv.sources.<name>`
  (`index = NAME`, table or first element of an array) to `[[tool.uv.index]].url`.
- `index` — `IndexClient` trait: `fn versions(&self, simple_url: &str, package: &str) ->
  Result<Vec<String>>`. `HttpIndexClient` (ureq, rustls) GETs `<simple_url>/<pkg>/` with
  `Accept: application/vnd.pypi.simple.v1+json`; on a JSON response reads `files[].filename`
  (and `versions` when present); otherwise parses `<a …>filename</a>` from the HTML simple
  page. A pure function `versions_from_filenames(package, filenames)` extracts versions
  from wheel and sdist names.
- `version` — `latest_stable(versions) -> Option<pep440_rs::Version>`: parse with
  `pep440_rs`, drop pre/dev/post-release and local versions, return max.

## `devkit lock`

```
devkit lock [--root PATH] [--package NAME]... [--no-commit] [--dry-run] [-- <uv sync args>...]
```

poe task `lock` keeps `--upgrade/-U` and `--all-extras` and forwards them after `--`.

Steps, in order:

1. Read `<root>/pyproject.toml` with `toml_edit` (error if missing). Targets are the
   `--package` values, default `["aeth-devkit"]`.
2. For each target: locate its requirement (see `pyproject::find_requirement`). Not found →
   print `No <pkg> pin found in pyproject.toml; skipping pin update` and continue.
3. Resolve the index: `pyproject::index_url_for`, else `https://pypi.org/simple/`. Strip a
   trailing `/` and ensure the URL ends in the simple root (uv index URLs already point at
   `…/+simple` or `…/simple`).
4. `IndexClient::versions` → `version::latest_stable`. Empty → error
   `No stable release versions found for <pkg> on <index>`.
5. Rewrite the requirement's version. If unchanged: `<pkg> pin already at <v>`; else
   `Updated <pkg> pin to <v>` and, unless `--dry-run`, write `pyproject.toml` back.
6. Run `uv sync <forwarded args>` in `root` with inherited stdio (skipped under
   `--dry-run`, printed instead). Non-zero → exit with that status code.
7. Unless `--no-commit` or `--dry-run`: if the project is git-tracked and
   `git diff --quiet -- uv.lock pyproject.toml` reports changes, `git add` those two paths
   and commit with subject `Update uv.lock`; print the short hash. Clean → `uv.lock is up
   to date; nothing to commit`. Not git-tracked → `Not a git repository; skipping commit`.

Behavioural differences from `lock.sh`, all intentional: index derived from
`pyproject.toml` instead of hardcoded; multiple/alternate pin targets via `--package`;
`--dry-run`/`--no-commit`; requirement search covers optional-deps and dependency-groups,
not just a regex over the file.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | `setup-project --check` found drift |
| 2 | usage, IO, parse, or network error (`error: …` on stderr) |
| 3 | changes applied but commit failed |
| n | `uv sync` exited with `n` |

## poe tasks (`python/aeth_devkit/__init__.py`)

- `setup-project` → `cmd: devkit setup-project` (extra args pass through).
- `lock` → `cmd: devkit lock -- ${upgrade:+--upgrade} ${all_extras:+--all-extras}` via
  the existing `shell`/bash form so empty flags vanish.
- `release`, `docker-pin-latest`, `release-and-pin`, `rescind-release`, `fix-bash`,
  `clean` unchanged except for script paths.

`setup-project` also gets one rename-aware rule: when merging `tool.poe.include_script`,
an existing entry whose `script` is `poe_tasks:tasks` is **replaced** by the template's
`aeth_devkit:tasks` entry rather than unioned alongside it.

## Downstream migration (README)

In each project: replace the `poe-tasks` dev dependency with `aeth-devkit`, rename the
`tool.uv.sources` key, then `uv sync --upgrade` and `poe setup-project` (which fixes
`include_script`). `poe lock` then keeps the `aeth-devkit` pin current.

## Testing

- **core**: unit tests for `set_requirement_version` (operators, extras, markers,
  whitespace), `find_requirement` across all three tables, `index_url_for` (table, array,
  missing), `versions_from_filenames` (wheels, sdists, unrelated files), `latest_stable`
  (pre/dev/post/local filtering), and PEP 691 JSON + HTML parsing from fixture strings.
  No network in tests.
- **lock**: integration tests in a temp git repo with a fixture `pyproject.toml`, a stub
  `IndexClient`, and a recording `Runner`; assert the rewritten requirement, the exact
  `uv sync` argv, the commit subject and committed paths, and the skip/`--dry-run`/
  `--no-commit` branches. `run()` takes `&dyn IndexClient` and `&dyn Runner`; the CLI wires
  the real ones.
- **setup**: existing `tests/apply.rs` moves with path/name updates, plus a case for the
  `poe_tasks:tasks` → `aeth_devkit:tasks` replacement.
- `cargo test --workspace`, `cargo clippy --workspace`, `maturin develop`, then `poe lock`
  in this repo as the smoke test.
