# aeth-devkit

Personal project-maintenance toolkit: a set of [poe](https://poethepoet.natn.io/) tasks
plus the `devkit` CLI (Rust) they call.

## Commands

| poe task                                                          | Backing                        | What it does                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `poe setup-project`                                               | `devkit setup-project`         | Standardize a project's config from the shipped templates (idempotent).                                                                                                                                                                                       |
| `poe lock [-U] [--all-extras] [-p PKG] [--dry-run] [--no-commit]` | `devkit lock`                  | Bump the `aeth-devkit` pin to the latest stable release on its index, `uv sync`, commit `uv.lock`.                                                                                                                                                            |
| `poe release [-f] [--dry-run] [bump …] ["notes"]`                 | `devkit release`               | Bump version, commit, tag, push, create the GitHub release, then wait for the release workflow to build and publish; rolls back on failure.                                                                                                                                                                             |
| `poe docker-pin [-V VER] [--dry-run] [--no-commit] [--no-push]`   | `devkit docker-pin`            | Pin the compose file's `GIT_TAG` / `PACKAGE_VERSION` to a released version of the project, commit, and push.                                                                                                                                                  |
| `poe release-and-pin [-f] [--dry-run] [bump …] ["notes"]`         | `devkit release-and-pin`       | `devkit release` then `devkit docker-pin` with the freshly released version, in-process.                                                                                                                                                                      |
| —                                                                 | `devkit complete`              | Shell completion for `poe` served from Rust (~13 ms per Tab instead of ~200 ms). `devkit complete install --powershell --bash` wires it into `$PROFILE` and the bash completion files. Uses whichever `devkit` is on PATH at Tab time, which an activated venv provides; no global install required. |
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
(dry-run that exits 1 on drift), `--no-commit`, `--replace-docker`. Prompts only before
replacing a Docker file (see **Docker** below); otherwise no prompts. Without a terminal on
stdin only `--dry-run`/`--check` are accepted; anything else is refused up front, since a
headless run can neither answer a prompt nor commit on someone's behalf. Idempotent — a
second run is a byte-for-byte no-op.

- **Project discovery** - Detects the package name and layout (`src/` vs `python/`), Rust
  (`Cargo.toml` enables the Rust overlays), Docker (`[tool.docker].services` non-empty
  enables `.dockerignore` and the Docker step; Docker files on disk seed the `[tool.docker]`
  table with `services = []` and a reminder to list the service), and the declared
  dependencies (drives `if-dep` gating). A committing run refuses a `services` value that
  differs from HEAD's: commit it, then rerun.
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
- **Release workflow** - `.github/workflows/release.yml` is rendered from the pure-Python
  or the maturin-matrix template (`Cargo.toml` selects the latter) and is devkit-owned:
  any drift is replaced, reported, and counts for `--check`. The publish step targets the
  sole `[[tool.uv.index]]` with a `publish-url` through repository secrets
  `UV_INDEX_<KEY>_USERNAME` / `_PASSWORD`, or PyPI via trusted publishing when no index
  publishes; several publish indexes are a config error. Before attaching or publishing,
  the workflow checks that the release owning the tag is still the one that triggered
  it, so a release deleted and recreated mid-build gets nothing from the old run. The
  first install prints a `note:` with the secret names or the trusted-publisher
  registration values. In devkit itself (the `aeth-devkit-container` crate is present) the
  maturin-matrix variant also builds the container binary per platform (static musl on
  Linux) and attaches it to the release; a Rust sister project gets the wheels alone.
- **Claude config** - `.claude/settings.json` (shared, no machine-specific paths) vs
  `settings.local.json` (absolute env paths + hook commands). Hook merge keeps exactly one
  entry per devkit hook, updates it in place, and leaves user hooks alone. `.mcp.json`:
  adds missing servers, never edits ones the project already defines.
- **Docker** - Runs whenever `[tool.docker].services` lists at least one compose service.
  `docker/Dockerfile` is created when missing; when present and different — ignoring CRLF/LF — a unified diff is printed and
  the file is replaced only on `replace` (`replace all` answers every remaining Docker
  question; anything else keeps it). The compose file (docker-pin's discovery; created as
  `docker/compose.yaml` when absent) is edited in place, format-preserving, per listed
  service: exact keys (`build.context`, `build.dockerfile`, `container_name`,
  `healthcheck.*`), pattern (`GIT_REPO` vs origin), presence (`GIT_TAG`, `restart`,
  `networks`) and at-least (`volumes` mounting `/app/persisted_data`; the ALERTS_*
  environment when the project uses aeth_ext), plus top-level `networks.coolify.external`.
  All compose edits are one diff and one prompt; a listed service missing from the file is
  offered as a scaffold block (`add`). Keys the standard does not name are never touched,
  and a shape the engine does not model (a flow-style `volumes: [...]` / `environment: {...}`,
  a list-form `build.args`) is judged on its text and reported as a `note:` rather than
  edited, so the file is never left unparseable.
  `--replace-docker` answers `replace all` up front; adding a listed-but-absent service is
  always asked, never pre-answered. `--dry-run`/`--check` print everything and count Docker
  drift. `docker/entrypoint.sh` and `docker/scripts/` are reported as safe to delete, never
  removed.
- **Placeholders** - `{project_root}`, `{package}`, `{python_dir}`, `{devkit_bin}`,
  `{publish_index}`, `{publish_index_key}`, `{devkit_version}`, `{git_repo}` with
  per-format escaping; `{devkit_bin}` prefers the venv binary over `uv run devkit`;
  `{git_tag}` (latest stable remote tag, resolved lazily, falling back to `v<pyproject
  version>` with a note) and `{service}` are filled per compose scaffold block. YAML
  templates gate blocks with `# setup-project: if-<name>` / `if-no-<name>` … `end` markers
  (`publish-index`, `container-crate`, `aeth-ext`).
- **Post-apply** - `tombi format` on pyproject (non-fatal), then a quiet auto-commit of
  exactly the changed files (`Standardize project configuration with devkit`, per-file
  body; never env files or `settings.local.json`) via the machinery shared with `lock` and
  `release`: committable managed files are merged against their HEAD content, the commit
  carries only this run's changes, and uncommitted edits are replayed back on top —
  overlapping edits reject the run and roll it back (exit 3); unrelated staged work is
  left alone. Then `note:` advisories (git-ignored managed files, stale `[tool.docker]`,
  `copilot-instructions.md`).
- **Not yet implemented** - (see TODO.md) `--python-dir` override, vendored-gitignore
  refresh task.

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
nothing), `--index` (defaults to the sole index with a `publish-url`, or PyPI when there is
none), `--no-wait` (return once the GitHub release exists), `--root`. Flags parse anywhere
on the line.

- **Division of labour** - The command does the human half (bump, commit, tag, push,
  create the release) and waits for the devkit-installed release workflow to do the
  reproducible half (build every artefact on CI, attach it to the release, publish to the
  index). Nothing is built or published on the developer's machine.
- **Pre-flight** - Read-only checks: git/uv/gh present and `gh` able to list workflow
  runs; `.github/workflows/release.yml` committed at `HEAD`, publishing to the configured
  target (a workflow rendered for another index, or for PyPI, is refused) and, for a
  tag-only release, already identical on `origin/main`, where GitHub reads release
  workflows from; on `main` with upstream,
  fetched, not behind; release config committed and matching HEAD; `Cargo.toml` version in
  sync; no merge conflicts in managed files; target version computed via `uv version
  --dry-run`; no run of an earlier release of that tag still queued or in progress (it
  would attach to and publish against the new release).
- **Publish target** - The sole `[[tool.uv.index]]` with a `publish-url` (credentials
  `UV_INDEX_<KEY>_USERNAME/_PASSWORD` must be set locally for the pre-flight probe and the
  post-CI check; CI reads the same names from repository secrets), or PyPI when no index
  publishes (no credentials; trusted publishing in CI).
- **Artefact detection** - Detects existing artefacts of the target tag (local/remote tag,
  GitHub release, index version — devpi's REST endpoint for a private index, the simple
  index for PyPI), shows a table, and removes them after confirmation (commits are never
  rewound here — that's `rescind-release`). An existing PyPI version aborts: PyPI files
  cannot be removed.
- **Prompts** - Two, both requiring the literal word `force` (dirty tree; remove existing
  artefacts); `--force` skips both.
- **Release steps** - Snapshot managed files → bump (pyproject, `Cargo.toml`, `cargo
  update`) → `uv lock` → quiet commit built through a scratch index (the machinery shared
  with `lock` and `setup-project`: uncommitted edits to managed files are replayed back
  afterwards; the user's staging is untouched; comparisons and the merge-back go through
  git's clean/smudge filters, so a `core.autocrlf` CRLF checkout is neither mistaken for
  an edit nor rewritten to LF) → annotated tag → one atomic `git push` of branch + tag →
  `gh release create` with the notes (or `--generate-notes`) and no files → wait for the
  release workflow run (`gh run list` until a run that did not exist before the release
  appears, up to 120 s, then `gh run watch --exit-status`; a watcher that dies — Ctrl-C, API blip — while the
  run is still going cancels the run and waits for it to stop, so nothing is published
  after the rollback, and a Ctrl-C that lands once the release exists does the same; a
  run that will not stop, or runs that cannot be listed at all, leave the release in
  place with the manual undo commands printed, never rolled back under) and verify the version is on
  the publish target (polling up to 120 s for index propagation) and the release still
  exists. `--no-wait` skips the last step and prints the workflow's Actions URL.
- **Rollback** - On any failure or Ctrl-C — a failed or missing workflow run included —
  the journal is walked backwards (restore files, soft-reset the commit, delete tag /
  remote tag / the GitHub release, by the id it was created with), with force-with-lease
  guards so a concurrent release is never clobbered; anything that can't be undone prints an exact manual cleanup command.
  Artefacts the workflow already published are not removed: on a private index the next
  `devkit release` of the same version detects and offers to remove them; on PyPI, where
  files are immutable, it aborts and the version must be bumped past.
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

### `devkit-container`

A separate static binary (crate `aeth-devkit-container`, release assets
`devkit-container-x86_64-unknown-linux-musl` and `devkit-container-x86_64-pc-windows-msvc.exe`)
that the templated Dockerfile downloads at build time, pinned to the devkit version that
rendered it. No Python runs in the image outside the app itself.

- `app-extra` - prints `--extra app` when `[project.optional-dependencies].app` exists.
- `readme` - prints `project.readme` (string or `{ file = … }` form).
- `run` - the entrypoint (Linux only). Must be root. Resolves the single `run-app-*`
  script in `[project.scripts]`; checks every `[tool.docker].required_persisted_dirs`
  entry is backed by a bind mount (the path or an ancestor below `/app`, per
  `/proc/self/mountinfo`) and refuses to start otherwise; `mkdir -p` + recursive chown to
  `999:999`; `setgroups([])`, `setgid`, `setuid`; `exec /app/.venv/bin/<script>`. `/app`
  itself stays root-owned: the app writes only to its mounted dirs or temp dirs. Entries
  that are empty, `.`, `..`, absolute or escape `/app` are errors; a table still carrying
  `chown_paths`/`mkdirs` (with or without `required_persisted_dirs`) is refused with the
  migration hint. Flags `--pyproject`, `--app-root`, `--mountinfo` exist for tests.

### `devkit release-and-pin`

Args: identical to `devkit release` (all of them forward verbatim).

- **Composition** - Runs the release and the pin in one process — no subprocess, no shell
  glue; the pin step receives the released version explicitly and runs its full preflights
  as free post-release verification.
- **Dry run stays dry** - `--dry-run` prints the release plan and skips the pin step (an
  unpublished version cannot pass the completeness preflight).
- **Abort safety** - A declined prompt or rolled-back release never reaches the pin step.
- **Waits for CI** - `--no-wait` is refused: the pin's completeness preflight needs the
  artefacts the workflow publishes, and `Released` already means the workflow finished.

### `devkit complete`

Subcommands: `query` (the per-Tab request, called by the shims), `tasks [DIR]` and `args
<TASK> [DIR]` (retained for shims installed by an older devkit), `script
--powershell|--bash`, `install --powershell --bash [--dry-run]`; global `--no-cache`.

- **Fast data path** - Serves poe's completion from Rust (~13 ms warm vs poe's ~200 ms).
- **Thin shims** - Each shell installs a ~50-line shim that forwards the command line to
  `devkit complete query` and acts on a directory/file sentinel; all the logic (task
  location, global options, choices, positional indexing) lives in one Rust engine rather
  than in two near-duplicate shell scripts. The shells still do their own path completion,
  keeping their own quoting rules.
- **Task resolution** - Mirrors poe's: `[tool.poe.tasks]`, recursive `include` files
  (env-var expansion, cycle guard), hidden `_` tasks skipped, first definition wins;
  `include_script` is executed against the venv python directly, skipping poe's startup.
- **Caching** - Fingerprint cache at `.cache/devkit-completions.json` (devkit version +
  each source's mtime/size); a corrupt cache is a miss, and the data subcommands never
  exit non-zero — a failing completer would break the shell.
- **No global install needed** - The shims call `devkit` only at Tab time, so an activated
  venv's copy is used. A global install is only wanted if you want completion in shells
  where no venv is activated.
- **Install** - Writes the PowerShell shim to `~/.local/share/devkit/poe-completion.ps1`
  and puts one permanent, content-free line in `$PROFILE` that dot-sources it (also
  removing poe's own slow registration, and any previous devkit line); writes the bash
  completion files for Git Bash and Linux; refuses to overwrite files it didn't generate;
  idempotent.
- **Self-repair** - Each request carries a shim version. A shim older than the binary is
  rewritten in place (atomically) for the next shell, while the current request is still
  answered.
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
