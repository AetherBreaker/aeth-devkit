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

## Feature reference (Rust commands)

<!-- Feature-tracking section. Update a command's list in the same commit that changes its
behavior. Shell-script commands (docker-pin-latest, rescind-release) are documented here
only once they migrate to Rust. -->

### `devkit setup-project`

Flags: `--root`, `--templates-dir` (or `DEVKIT_TEMPLATES`), `--dry-run`, `--check`
(dry-run that exits 1 on drift), `--no-commit`. No prompts; idempotent — a second run is a
byte-for-byte no-op.

- Detects the package name and layout (`src/` vs `python/`), Rust (`Cargo.toml` enables the
  Rust overlays), Docker (a real Dockerfile/compose file enables `.dockerignore` and
  `[tool.docker]`), and the declared dependencies (drives `if-dep` gating).
- `pyproject.toml`: comment-preserving deep merge of the template — scalars replace, arrays
  union, dependency arrays match by normalized package name so pins upgrade in place,
  `[tool.setup-project].keep` opts paths out, `if-dep` / `if-docker` markers gate
  conditional tables. Managed keys: the dev dependency group, `tool.coverage`,
  `tool.docker`, `tool.mypy.cache_dir`, `tool.poe.include_script`, `tool.pyright`
  (incl. `executionEnvironments`), `tool.pytest`, `tool.ruff` — incl.
  `lint.isort.known-first-party = ["{package}"]` and the import headings — and `tool.tombi`.
- Migrations: `poe_tasks:tasks` include_script → `aeth_devkit:tasks`; drops
  `tool.ruff.extend` / `tool.pyright.extends` pointing at a parent pyproject; rewrites
  legacy `.claude/hooks/*.py` hook commands to `devkit hook` in place.
- `.vscode/`: `settings.json` + `extensions.json` deep JSON merge (plus Rust overlay);
  `launch.json` created from template or patched (`envFile` + `PYTHON*` env vars on Python
  launch configs only); `tasks.json` patched only (`PYTHONPYCACHEPREFIX`).
- `.env` and every `envFile` referenced by `launch.json`: key-wise upsert; other lines and
  secrets preserved.
- Line-merged files: `.gitignore` (template prepended or replaces, project-specific rules
  kept under a header; base + Rust + devkit layers), `.gitattributes`, `.dockerignore`
  (Docker projects only).
- `AGENTS.md`: devkit-managed `<!-- devkit:begin/end -->` block; text outside is never
  touched. Create-if-missing only: `.claude/CLAUDE.md`, `.github/workflows/claude.yml`.
- Claude config: `.claude/settings.json` (shared, no machine-specific paths) vs
  `settings.local.json` (absolute env paths + hook commands). Hook merge keeps exactly one
  entry per devkit hook, updates it in place, and leaves user hooks alone. `.mcp.json`:
  adds missing servers, never edits ones the project already defines.
- Placeholders `{project_root}`, `{package}`, `{python_dir}`, `{devkit_bin}` with
  per-format escaping; `{devkit_bin}` prefers the venv binary over `uv run devkit`.
- After apply: `tombi format` on pyproject (non-fatal), then an auto-commit of exactly the
  changed files (`Standardize project configuration with devkit`, per-file body; never env
  files or `settings.local.json`; unrelated staged work is left alone), then `note:`
  advisories (git-ignored managed files, stale `[tool.docker]`, `copilot-instructions.md`).
- Not yet implemented (see TODO.md): `--docker` scaffolding flags, `--python-dir`
  override, vendored-gitignore refresh task.

### `devkit lock`

Flags: `--root`, `-p/--package` (repeatable; default `aeth-devkit`), `--dry-run`,
`--no-commit`, and a trailing `-- <uv args>` forwarded verbatim to `uv sync` (`poe lock`'s
`-U` / `--all-extras` arrive this way).

- Finds each pin across `project.dependencies`, `optional-dependencies` and
  `dependency-groups` (PEP 503 name normalization).
- Resolves the package's index from `tool.uv.sources` + `[[tool.uv.index]]` (PyPI
  fallback) and queries it for the latest stable release (PEP 691 JSON or PEP 503 HTML;
  pre/dev/post/local versions excluded).
- Rewrites `>=` / `==` / `===` / `~=` pins and one-major `>=A,<B` ranges in place,
  preserving extras, markers, whitespace and comments; anything odder is skipped with a
  message naming the latest version.
- Always runs `uv sync` (plus forwarded args); a sync failure becomes the exit code and
  the pin edit is left on disk (no rollback).
- Commits exactly `uv.lock` + `pyproject.toml` (`Update uv.lock`), pathspec-limited so
  other staged work stays out; skips cleanly outside git or with nothing to commit.
  Synced-but-commit-failed is exit 3.

### `devkit release`

Args: `[bump …] ["notes"]` (bump kinds are uv's `major minor patch stable alpha beta rc
post dev`, chainable; notes must be multi-word; no bump = re-release the current version,
pushing only the tag), `-f/--force`, `--dry-run` (prints the numbered plan, changes
nothing), `--index` (defaults to the sole index with a `publish-url`), `--root`. Flags
parse anywhere on the line.

- Pre-flight (read-only): git/uv/gh present; on `main` with upstream, fetched, not behind;
  release config committed and matching HEAD; `Cargo.toml` version in sync; no merge
  conflicts in managed files; target version computed via `uv version --dry-run`.
- Detects existing artefacts of the target tag (local/remote tag, GitHub release, devpi
  version), shows a table, and removes them after confirmation (commits are never rewound
  here — that's `rescind-release`).
- Two prompts, both requiring the literal word `force` (dirty tree; remove existing
  artefacts); `--force` skips both.
- Steps: snapshot managed files + `dist/` → bump (pyproject, `Cargo.toml`,
  `cargo update`) → `uv lock` → `uv build` into a fresh `dist/` → commit built through a
  scratch index (uncommitted edits to managed files are replayed back afterwards; the
  user's staging is untouched) → annotated tag → `uv publish` (credentials from
  `UV_INDEX_<NAME>_USERNAME/_PASSWORD`, passed by env, never argv) → one atomic
  `git push` of branch + tag → `gh release create` with the built artifacts and the notes
  (or `--generate-notes`).
- Rollback on any failure or Ctrl-C: the journal is walked backwards (restore files,
  soft-reset the commit, delete tag / remote tag / devpi version / GitHub release), with
  force-with-lease guards and byte-comparison checks so a concurrent release is never
  clobbered; anything that can't be undone prints an exact manual cleanup command.
- Exit codes: 0 released, 1 aborted or rolled back, 2 pre-flight/config error.

### `devkit complete`

Subcommands: `tasks [DIR]` and `args <TASK> [DIR]` (the per-Tab data), `script
--powershell|--bash`, `install --powershell --bash [--dry-run]`; global `--no-cache`.

- Serves poe's task/argument completion data from Rust (~13 ms warm vs poe's ~200 ms);
  the shell scripts are poe's own generated ones with only the two data calls swapped.
- Mirrors poe's task resolution: `[tool.poe.tasks]`, recursive `include` files (env-var
  expansion, cycle guard), hidden `_` tasks skipped, first definition wins;
  `include_script` is executed against the venv python directly, skipping poe's startup.
- Fingerprint cache at `.cache/devkit-completions.json` (devkit version + each source's
  mtime/size); a corrupt cache is a miss, and the data subcommands never exit non-zero —
  a failing completer would break the shell.
- `install`: requires a global `devkit` on PATH, patches `$PROFILE` (also removing poe's
  own slow registration) and writes the bash completion files for Git Bash and Linux;
  refuses to overwrite files it didn't generate; idempotent.
- PowerShell and bash only.

### `devkit hook`

Five Claude Code hooks, registered by `setup-project` in `.claude/settings.local.json`.
Payload on stdin, at most one JSON line on stdout, always exits 0 — every failure path
degrades to silence, and the update check never runs on this path.

- `pre-edit-protect`: denies Edit/Write to `.env` and `uv.lock`, matched on the basename
  with Windows name normalization.
- `pre-bash-protect-deps`: denies `uv add|remove|lock` via a quote-aware command tokenizer
  (handles wrappers, env-var prefixes, `bash -c` recursion, and uv's value-taking global
  flags — not a regex).
- Stop hooks re-report tool failures as `additionalContext`: `stop-ruff` (`--fix
  --unfixable F401`) scoped to the branch diff, `stop-pyright` project-wide on purpose,
  `stop-clean` (`poe clean`); venv binaries preferred over `uv run`; output capped at
  4000 chars; `stop_hook_active` loop guard.

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
