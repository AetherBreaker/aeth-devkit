# aeth-devkit

Personal project-maintenance toolkit: a set of [poe](https://poethepoet.natn.io/) tasks
plus the `devkit` CLI (Rust) they call.

## Commands

| poe task                                                          | Backing                        | What it does                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `poe setup-project`                                               | `devkit setup-project`         | Standardize a project's config from the shipped templates (idempotent).                                                                                                                                                                                       |
| `poe lock [-U] [--all-extras] [-p PKG] [--dry-run] [--no-commit]` | `devkit lock`                  | Bump the `aeth-devkit` pin to the latest stable release on its index, `uv sync`, commit `uv.lock`.                                                                                                                                                            |
| `poe release [-f] [--dry-run] [bump …] ["notes"]`                 | `devkit release`               | Bump version, build, tag, publish to the index and GitHub; rolls back on failure.                                                                                                                                                                             |
| `poe docker-pin-latest`                                           | `scripts/docker-pin-latest.sh` | Pin the compose file's package version.                                                                                                                                                                                                                       |
| —                                                                 | `devkit complete`              | Shell completion for `poe` served from Rust (~13 ms per Tab instead of ~200 ms). `devkit complete install --powershell --bash` wires it into `$PROFILE` and the bash completion files (needs a global `devkit`: `uv tool install aeth-devkit --index <url>`). |
| `poe rescind-release`                                             | `scripts/rescind-release.sh`   | Undo a release.                                                                                                                                                                                                                                               |

`devkit --help` lists the Rust subcommands. Each lives in its own crate under `crates/`;
`cargo run -p aeth-devkit-lock -- --help` runs one command's dev binary without linking the
others.

**Update check.** `setup-project`, `lock`, `release` and `complete install` end with a
`note:` on stderr when the running `devkit` is older than the latest stable release on the
project's `[[tool.uv.index]]` entry for `aeth-devkit`, naming the fix (`uv tool upgrade
aeth-devkit`, or `devkit lock` when running from a project `.venv`). The index is queried at
most once a day and the answer cached at `%LOCALAPPDATA%\aeth-devkit\update-check.json`
(`~/.cache/aeth-devkit/` elsewhere; `DEVKIT_UPDATE_CACHE=<file>` relocates it). Failures are
silent. `DEVKIT_NO_UPDATE_CHECK=1` disables the check; the Tab-completion data path never runs it.

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

```sh
cargo test --workspace
uv run maturin develop     # installs the devkit binary into .venv
```

Layout: `crates/aeth-devkit-core` (shared git/process/pyproject/index helpers),
`crates/aeth-devkit-setup` and `crates/aeth-devkit-lock` (one command each, library +
dev binary), `crates/aeth-devkit` (the shipped `devkit` dispatcher),
`python/aeth_devkit` (poe tasks, remaining shell scripts, templates).
