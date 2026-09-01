# aeth-devkit

Personal project-maintenance toolkit: a set of [poe](https://poethepoet.natn.io/) tasks
plus the `devkit` CLI (Rust) they call.

## Commands

| poe task                                                          | Backing                        | What it does                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `poe setup-project`                                               | `devkit setup-project`         | Standardize a project's config from the shipped templates (idempotent).                                                                                                                                                                                       |
| `poe lock [-U] [--all-extras] [-p PKG] [--dry-run] [--no-commit]` | `devkit lock`                  | Bump the `aeth-devkit` pin to the latest stable release on its index, `uv sync`, commit `uv.lock`.                                                                                                                                                            |
| `poe release [-f] [--dry-run] [bump …] ["notes"]`                 | `devkit release`               | Bump version, build, tag, publish to the index and GitHub; rolls back on failure.                                                                                                                                                                             |
| `poe docker-pin [-V VER] [--dry-run] [--no-commit] [--no-push]`   | `devkit docker-pin`            | Pin the compose file's `GIT_TAG` / `PACKAGE_VERSION` to a released version of the project, commit, and push.                                                                                                                                                  |
| `poe release-and-pin [-f] [--dry-run] [bump …] ["notes"]`         | `devkit release-and-pin`       | `devkit release` then `devkit docker-pin` with the freshly released version, in-process.                                                                                                                                                                      |
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
behavior. Shell-script commands (rescind-release) are documented here
only once they migrate to Rust. -->

### `devkit setup-project`

Flags: `--root`, `--templates-dir` (or `DEVKIT_TEMPLATES`), `--dry-run`, `--check`
(dry-run that exits 1 on drift), `--no-commit`. No prompts; idempotent — a second run is a
byte-for-byte no-op.

- **Project discovery** - Detects the package name and layout (`src/` vs `python/`), Rust
  (`Cargo.toml` enables the Rust overlays), Docker (a real Dockerfile/compose file enables
  `.dockerignore` and `[tool.docker]`), and the declared dependencies (drives `if-dep` gating).
- **pyproject merge** - Comment-preserving deep merge of the template into
  `pyproject.toml` — scalars replace, arrays union, dependency arrays match by normalized
  package name so pins upgrade in place, `[tool.setup-project].keep` opts paths out,
  `if-dep` / `if-docker` markers gate conditional tables. Managed keys: the dev dependency
  group, `tool.coverage`, `tool.docker`, `tool.mypy.cache_dir`, `tool.poe.include_script`,
  `tool.pyright` (incl. `executionEnvironments`), `tool.pytest`, `tool.ruff` — incl.
  `lint.isort.known-first-party = ["{package}"]` and the import headings — and `tool.tombi`.
- **Migrations** - `poe_tasks:tasks` include_script → `aeth_devkit:tasks`; drops
  `tool.ruff.extend` / `tool.pyright.extends` pointing at a parent pyproject; rewrites
  legacy `.claude/hooks/*.py` hook commands to `devkit hook` in place.
- **VS Code config** - `settings.json` + `extensions.json` deep JSON merge (plus Rust
  overlay); `launch.json` created from template or patched (`envFile` + `PYTHON*` env vars
  on Python launch configs only); `tasks.json` patched only (`PYTHONPYCACHEPREFIX`).
- **Env files** - `.env` and every `envFile` referenced by `launch.json`: key-wise upsert;
  other lines and secrets preserved.
- **Line-merged files** - `.gitignore` (template prepended or replaces, project-specific
  rules kept under a header; base + Rust + devkit layers), `.gitattributes`,
  `.dockerignore` (Docker projects only).
- **Agent docs** - `AGENTS.md` devkit-managed `<!-- devkit:begin/end -->` block; text
  outside is never touched. Create-if-missing only: `.claude/CLAUDE.md`,
  `.github/workflows/claude.yml`.
- **Claude config** - `.claude/settings.json` (shared, no machine-specific paths) vs
  `settings.local.json` (absolute env paths + hook commands). Hook merge keeps exactly one
  entry per devkit hook, updates it in place, and leaves user hooks alone. `.mcp.json`:
  adds missing servers, never edits ones the project already defines.
- **Placeholders** - `{project_root}`, `{package}`, `{python_dir}`, `{devkit_bin}` with
  per-format escaping; `{devkit_bin}` prefers the venv binary over `uv run devkit`.
- **Post-apply** - `tombi format` on pyproject (non-fatal), then a quiet auto-commit of
  exactly the changed files (`Standardize project configuration with devkit`, per-file
  body; never env files or `settings.local.json`) via the machinery shared with `lock` and
  `release`: committable managed files are merged against their HEAD content, the commit
  carries only this run's changes, and uncommitted edits are replayed back on top —
  overlapping edits reject the run and roll it back (exit 3); unrelated staged work is
  left alone. Then `note:` advisories (git-ignored managed files, stale `[tool.docker]`,
  `copilot-instructions.md`).
- **Not yet implemented** - (see TODO.md) `--docker` scaffolding flags, `--python-dir`
  override, vendored-gitignore refresh task.

### `devkit lock`

Flags: `--root`, `-p/--package` (repeatable; default `aeth-devkit`), `--dry-run`,
`--no-commit`, and a trailing `-- <uv args>` forwarded to `uv sync`, appended to the
default `--upgrade --all-extras` (a forwarded copy of a default is dropped, not doubled).

- **Pin discovery** - Finds each pin across `project.dependencies`,
  `optional-dependencies` and `dependency-groups` (PEP 503 name normalization).
- **Index resolution** - Resolves the package's index from `tool.uv.sources` +
  `[[tool.uv.index]]` (PyPI fallback) and queries it for the latest stable release
  (PEP 691 JSON or PEP 503 HTML; pre/dev/post/local versions excluded).
- **Pin rewrite** - Rewrites `>=` / `==` / `===` / `~=` pins and one-major `>=A,<B` ranges
  in place, preserving extras, markers, whitespace and comments; anything odder is skipped
  with a message naming the latest version.
- **Sync** - Always runs `uv sync --upgrade --all-extras` (plus forwarded args); a sync
  failure becomes the exit code (in commit mode the pin edit is rolled back first; with
  `--no-commit` or outside git it is left on disk).
- **Commit** - Quiet commit of exactly `uv.lock` + `pyproject.toml` (`Update uv.lock`),
  via the machinery shared with `release` and `setup-project`: the pin update and sync run
  against the files as committed in HEAD, the commit is built through a scratch index
  (staged work untouched) and carries only this command's changes, and uncommitted edits
  are replayed back on top afterwards — edits overlapping the pin update reject the run
  and roll everything back (exit 3). Skips cleanly outside git or with nothing to commit;
  safe on any branch (no `main` check).

### `devkit release`

Args: `[bump …] ["notes"]` (bump kinds are uv's `major minor patch stable alpha beta rc
post dev`, chainable; notes must be multi-word; no bump = re-release the current version,
pushing only the tag), `-f/--force`, `--dry-run` (prints the numbered plan, changes
nothing), `--index` (defaults to the sole index with a `publish-url`), `--root`. Flags
parse anywhere on the line.

- **Pre-flight** - Read-only checks: git/uv/gh present; on `main` with upstream, fetched,
  not behind; release config committed and matching HEAD; `Cargo.toml` version in sync; no
  merge conflicts in managed files; target version computed via `uv version --dry-run`.
- **Artefact detection** - Detects existing artefacts of the target tag (local/remote tag,
  GitHub release, devpi version), shows a table, and removes them after confirmation
  (commits are never rewound here — that's `rescind-release`).
- **Prompts** - Two, both requiring the literal word `force` (dirty tree; remove existing
  artefacts); `--force` skips both.
- **Release steps** - Snapshot managed files + `dist/` → bump (pyproject, `Cargo.toml`,
  `cargo update`) → `uv lock` → `uv build` into a fresh `dist/` → quiet commit built
  through a scratch index (the machinery shared with `lock` and `setup-project`:
  uncommitted edits to managed files are replayed back afterwards; the
  user's staging is untouched; comparisons and the merge-back go through git's
  clean/smudge filters, so a `core.autocrlf` CRLF checkout is neither mistaken for an
  edit nor rewritten to LF) → annotated tag → `uv publish` (credentials from
  `UV_INDEX_<NAME>_USERNAME/_PASSWORD`, passed by env, never argv) → one atomic
  `git push` of branch + tag → `gh release create` with the built artifacts and the notes
  (or `--generate-notes`).
- **Rollback** - On any failure or Ctrl-C the journal is walked backwards (restore files,
  soft-reset the commit, delete tag / remote tag / devpi version / GitHub release), with
  force-with-lease guards and byte-comparison checks so a concurrent release is never
  clobbered; anything that can't be undone prints an exact manual cleanup command.
- **Exit codes** - 0 released, 1 aborted or rolled back, 2 pre-flight/config error.

### `devkit docker-pin`

Flags: `-V/--version` (exact version, `v` prefix optional, pre-releases allowed; default =
latest stable release present everywhere), `--dry-run`, `--no-commit` (edit only, implies
no push), `--no-push`, `-c/--compose-file`, `--root`.

- **Compose discovery** - Breadth-first from the git repo root, shallowest directory
  first, Docker's own name precedence (`compose.yaml` > `compose.yml` >
  `docker-compose.yaml` > `docker-compose.yml`) within a directory, first hit wins;
  hidden and environment/build directories are skipped; the chosen file is printed.
- **Service matching** - Line-based, format-preserving parse of `services:`; a service is
  pinned only when it builds *this* project — `GIT_REPO` naming the same repository as
  `origin` (https/ssh/`.git`/case-insensitive comparison) pins `GIT_TAG`, or
  `PACKAGE_NAME` normalizing to `[project].name` pins `PACKAGE_VERSION`. All matching
  services move together; commented lines never count; no match is an error listing what
  was found.
- **Completeness preflight** - The target version must exist on every source before
  anything is edited: GitHub tags *and* a GitHub release (via `gh`, so auth and pagination
  come free) when `origin` is on GitHub, plus every `[[tool.uv.index]]` with a
  `publish-url` (queried through its simple `url`). "Latest" is the highest stable version
  common to all sources — a half-published release can never be pinned.
- **Version handling** - PEP 440 end to end: parsed-equality membership checks
  (`1.2.0-alpha1` == `1.2.0a1`), `GIT_TAG` written with the tag's exact remote spelling,
  `PACKAGE_VERSION` written normalized. Already-pinned everywhere is a clean no-op.
- **Behind-origin preflight** - When pushing: fetch, require an upstream, refuse to edit
  while behind origin.
- **Commit & push** - Commits exactly the compose file (`chore: pin <package> to <ver>`),
  pathspec-limited so other staged work stays out; pushes the current branch. A dirty
  compose file gets the pin committed against HEAD's copy through a scratch index and the
  user's uncommitted edits merged back on top of the working tree (3-way, through git's
  clean/smudge filters so a CRLF checkout merges cleanly and stays CRLF); overlapping
  edits abort before anything is committed.

### `devkit release-and-pin`

Args: identical to `devkit release` (all of them forward verbatim).

- **Composition** - Runs the release and the pin in one process — no subprocess, no shell
  glue; the pin step receives the released version explicitly and runs its full preflights
  as free post-release verification.
- **Dry run stays dry** - `--dry-run` prints the release plan and skips the pin step (an
  unpublished version cannot pass the completeness preflight).
- **Abort safety** - A declined prompt or rolled-back release never reaches the pin step.

### `devkit complete`

Subcommands: `tasks [DIR]` and `args <TASK> [DIR]` (the per-Tab data), `script
--powershell|--bash`, `install --powershell --bash [--dry-run]`; global `--no-cache`.

- **Fast data path** - Serves poe's task/argument completion data from Rust (~13 ms warm
  vs poe's ~200 ms); the shell scripts are poe's own generated ones with only the two data
  calls swapped.
- **Task resolution** - Mirrors poe's: `[tool.poe.tasks]`, recursive `include` files
  (env-var expansion, cycle guard), hidden `_` tasks skipped, first definition wins;
  `include_script` is executed against the venv python directly, skipping poe's startup.
- **Caching** - Fingerprint cache at `.cache/devkit-completions.json` (devkit version +
  each source's mtime/size); a corrupt cache is a miss, and the data subcommands never
  exit non-zero — a failing completer would break the shell.
- **Install** - Requires a global `devkit` on PATH, patches `$PROFILE` (also removing
  poe's own slow registration) and writes the bash completion files for Git Bash and
  Linux; refuses to overwrite files it didn't generate; idempotent.
- **Shells** - PowerShell and bash only.

### `devkit hook`

Five Claude Code hooks, registered by `setup-project` in `.claude/settings.local.json`.
Payload on stdin, at most one JSON line on stdout, always exits 0 — every failure path
degrades to silence, and the update check never runs on this path.

- **`pre-edit-protect`** - Denies Edit/Write to `.env` and `uv.lock`, matched on the
  basename with Windows name normalization.
- **`pre-bash-protect-deps`** - Denies `uv add|remove|lock` via a quote-aware command
  tokenizer (handles wrappers, env-var prefixes, `bash -c` recursion, and uv's
  value-taking global flags — not a regex).
- **Stop hooks** - Re-report tool failures as `additionalContext`: `stop-ruff` (`--fix
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
