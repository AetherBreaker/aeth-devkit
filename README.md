# aeth-devkit

Personal project-maintenance toolkit: a set of [poe](https://poethepoet.natn.io/) tasks
plus the `devkit` CLI (Rust) they call.

## Commands

| poe task                                                          | Backing                        | What it does                                                                                                                                                                                                                                                  |
| ----------------------------------------------------------------- | ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `poe setup-project`                                               | `devkit setup-project`         | Standardize a project's config from the `devkit-templates` package in its environment (idempotent).                                                                                                                                                           |
| `poe lock [-U] [--all-extras] [-p PKG] [--dry-run] [--no-commit]` | `devkit lock`                  | Bump the `aeth-devkit` pin to the latest stable release on its index, `uv sync`, commit `uv.lock`.                                                                                                                                                            |
| `poe release [-f] [--dry-run] [bump …] ["notes"]`                 | `devkit release`               | Bump version, commit, tag, push, create the GitHub release, then wait for the release workflow to build and publish; rolls back on failure.                                                                                                                                                                             |
| `poe docker-pin [-V VER] [--dry-run] [--no-commit] [--no-push]`   | `devkit docker-pin`            | Pin the compose file's `GIT_TAG` / `PACKAGE_VERSION` to a released version of the project, commit, and push.                                                                                                                                                  |
| `poe release-and-pin [-f] [--dry-run] [bump …] ["notes"]`         | `devkit release-and-pin`       | `devkit release` then `devkit docker-pin` with the freshly released version, in-process.                                                                                                                                                                      |
| `poe rescind-release`                                             | `scripts/rescind-release.sh`   | Undo a release.                                                                                                                                                                                                                                               |

`devkit --help` lists the Rust subcommands. Each lives in its own crate under `crates/`;
`cargo run -p aeth-devkit-lock -- --help` runs one command's dev binary without linking the
others.

**Update check.** Every command ends with a `note:` on stderr when the running `devkit` is
older than the latest stable release on the project's `[[tool.uv.index]]` entry for
`aeth-devkit`, naming the fix (`uv tool upgrade aeth-devkit`, or `devkit lock` when running
from a project `.venv`). The index is queried at most once a day and the answer cached at
`%LOCALAPPDATA%\aeth-devkit\update-check.json` (`~/.cache/aeth-devkit/` elsewhere;
`DEVKIT_UPDATE_CACHE=<file>` relocates it). Failures are silent. `DEVKIT_NO_UPDATE_CHECK=1`
disables the check.

## Feature reference (Rust commands)

<!-- Feature-tracking section. Update a command's list in the same commit that changes its
behavior. Shell-script commands (rescind-release) are documented here
only once they migrate to Rust. -->

### `devkit setup-project`

Flags: `--root`, `--templates-dir` (or `DEVKIT_TEMPLATES`, or `[tool.devkit].templates-dir`;
a working tree rendered instead of the environment's `devkit-templates` package),
`--dry-run`, `--no-commit`, `-y`/`--yes`. Prompts only before replacing a Docker file (see
**Docker** below); otherwise no prompts. `-y` accepts every proposal without asking. The
prompts read stdin, a terminal or a pipe alike; if the input ends before a question is
answered the run is cancelled (exit 2, the changes rolled back when committing), never
finished on defaults. A run with no stdin at all (a closed handle or the null device) is
refused up front unless nothing will be asked: `-y` or `--dry-run`. An `error:` line
(stderr) is drift in a managed file the run could not edit; the run still writes and
commits everything else, then exits 1, dry or not. Otherwise exit 0. Idempotent — a second
run is a byte-for-byte no-op.

- **Project discovery** - Detects the package name and layout (`src/` vs `python/`), Rust
  (`Cargo.toml` enables the Rust overlays), Docker (`[tool.docker].services` non-empty
  enables `.dockerignore` and the Docker step; Docker files on disk seed the `[tool.docker]`
  table with `services = []` and a warning to list the service, silenced by
  `[tool.docker].silence_unlisted_services_warning = true`, which nothing else reads), and the declared
  dependencies (drives `if-dep` gating). A committing run refuses a `services` value that
  differs from HEAD's: commit it, then rerun.
- **Templates** - Read from the project's environment: the `devkit_templates` package
  (`AetherBreaker/devkit-templates`). A project that lacks it gets `devkit-templates` added
  to its dev group with its index source, locked under the running devkit and synced before
  anything renders; `--dry-run` on such a project is an error saying so. The package
  depends on `aeth-devkit`, and a uv source covers a direct dependency only, so the project
  must list `aeth-devkit` itself with the same index source (a project that runs devkit
  from its own environment already does). Templates are versioned by `uv.lock` like the
  other devkit packages. `--templates-dir`,
  `DEVKIT_TEMPLATES` and `[tool.devkit].templates-dir` (in that order of precedence, each
  an existing directory) render a working tree instead, with no bootstrap; the templates
  repository renders its own tree that way.
- **pyproject merge** - Comment-preserving deep merge of the template into
  `pyproject.toml` — scalars replace, arrays union, dependency arrays match by normalized
  package name so pins upgrade in place, `if-dep` / `if-docker` / `if-docker-services`
  markers above a table header or a key-value line gate that table or key; a project never
  gets its own package. Managed keys: the dev dependency group, `tool.coverage`,
  `tool.docker`, `tool.mypy.cache_dir`, `tool.poe.include_script`,
  `tool.pyright` (incl. `executionEnvironments`), `tool.pytest`, `tool.ruff` — incl.
  `lint.isort.known-first-party = ["{package}"]` and the import headings — and `tool.tombi`.
- **Migrations** - `poe_tasks:tasks` include_script → `aeth_devkit:tasks`; drops
  `tool.ruff.extend` / `tool.pyright.extends` pointing at a parent pyproject; rewrites
  legacy `.claude/hooks/*.py` and `devkit hook` hook commands to `devkit-hook` in place.
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
  any drift is replaced and reported. The publish step targets the
  sole `[[tool.uv.index]]` with a `publish-url` through repository secrets
  `UV_INDEX_<KEY>_USERNAME` / `_PASSWORD`, or PyPI via trusted publishing when no index
  publishes; several publish indexes are a config error. Before attaching or publishing,
  the workflow checks that the release owning the tag is still the one that triggered
  it, so a release deleted and recreated mid-build gets nothing from the old run. The
  first install prints a `note:` with the secret names or the trusted-publisher
  registration values. The `devkit-container` package is not part of this workflow: it has
  its own repository and releases (see **`devkit-container`** below).
  A repository whose artefact is not a wheel sets `[tool.devkit] release-workflow = false`
  and keeps a `release.yml` of its own (the VS Code extension); setup-project then writes
  nothing there.
- **Claude config** - `.claude/settings.json` (shared, no machine-specific paths) vs
  `settings.local.json` (absolute env paths + hook commands). Hook merge keeps exactly one
  entry per devkit hook, updates it in place, and leaves user hooks alone. `.mcp.json`:
  adds missing servers, never edits ones the project already defines.
- **Hooks and completion** - Every project gets `devkit-claude-hooks` and
  `devkit-poe-complete` in its dev group at the newest release the running devkit accepts
  (the same floor-and-lock step as `devkit-container`); the hook lines in
  `.claude/settings.local.json` call the venv's `devkit-hook`, and the run ends with
  `devkit-complete install` for the shells on `PATH`, reporting what changed.
- **Docker** - Runs whenever `[tool.docker].services` lists at least one compose service.
  `docker/Dockerfile` is created when missing; when present and different — ignoring CRLF/LF, and
  written back in the file's own line endings — a unified diff is printed and
  the file is replaced only on `replace` (`replace all` answers every remaining Docker
  question; anything else keeps it). The compose file (docker-pin's discovery; created as
  `docker/compose.yaml` when absent) is edited in place, format-preserving, one diff per
  listed service: exact keys (`build.context`, `build.dockerfile`, `container_name`,
  `healthcheck.*`), pattern (`GIT_REPO` vs origin), presence (`GIT_TAG`, `restart`,
  `networks`) and at-least (`volumes` mounting `/app/persisted_data`; the ALERTS_*
  environment when the project uses aeth_ext), plus one diff for the top-level
  `networks.coolify.external`. A listed service missing from the file is its own one-hunk
  diff; accepting it adds the scaffold block. Keys the standard does not name are never
  touched, and a shape the engine does not model (a flow-style `volumes: [...]` /
  `environment: {...}`, a list-form `build.args`) is judged on its text and reported as a
  `error:` rather than edited, so the file is never left unparseable; so is an inline
  `services:` or an inline service block, which the step leaves whole. An `error:` is
  reported on every run until the file is fixed by hand, since a listed service declares
  the file managed. A compose file with no top-level `services:` key is a `warning:` on
  stderr instead, not an error:
  an `include:`-only aggregator is a supported Compose layout whose services live in the
  included files, and writing one here would conflict with them rather than override. The
  Dockerfile is rendered from the installed `devkit-container` package; the package step
  (below) installs and advances it first, and a dry run on a project that has not adopted it
  yet notes that instead of rendering.
  `-y` accepts everything up front, an add included; a typed `replace all` covers the
  shown diffs that follow, and adding a listed-but-absent service is still asked.
  `--dry-run` prints everything, Docker drift included. Inside a VS Code
  terminal the diff opens in the editor instead (see **VS Code extension**).
  `docker/entrypoint.sh` and `docker/scripts/` are reported as safe to delete, never
  removed.
- **Placeholders** - `{project_root}`, `{package}`, `{python_dir}`, `{hook_bin}`,
  `{publish_index}`, `{publish_index_key}`, `{devkit_index}` (the index the project's
  `aeth-devkit` source names, `SFTPyPI` when there is none), `{git_repo}` with per-format
  escaping; `{hook_bin}` prefers the project environment's `devkit-hook` over
  `uv run devkit-hook`; `{git_tag}` (latest stable
  remote tag, resolved lazily, falling back to `v<pyproject version>` with a note) and
  `{service}` are filled per compose scaffold block; `{latest}` in a pyproject template
  requirement means the newest release the running devkit accepts (see **Devkit
  packages**). YAML templates gate blocks with `# setup-project: if-<name>` / `if-no-<name>` …
  `end` markers (`publish-index`, `aeth-ext`).
- **Devkit packages** - `devkit-claude-hooks` and `devkit-poe-complete` (every project, dev
  group) and `devkit-container` (Docker projects, `[project].dependencies`) are added when
  missing, locked with `uv lock --upgrade-package` under the constraint
  `aeth-devkit==<the running version>`, and installed with `uv sync --frozen`; the floor
  written to `pyproject.toml` is the version
  uv chose (for a plain name or a `>=` requirement locked from an index; anything else is
  left as written with a note), followed by a plain `uv lock` so the lock's metadata records
  it. A floor no release can meet with this devkit stops the run with "run `devkit lock`"; a
  newer release on the index that needs a newer devkit is a warning, and so is a refresh that
  fails once the package is locked (offline, say). The project's environment is
  `UV_PROJECT_ENVIRONMENT` or `.venv`, and a sync that does not land the locked version there
  stops the run. `uv.lock` is committed with the run; when the user's own lock had uncommitted
  changes the venv is synced again once it is back. Devkit itself is never upgraded here;
  that is `devkit lock`'s job.
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
  `optional-dependencies` and `dependency-groups` (PEP 503 name normalization); a
  `dependency-groups` pin is preferred, and a `[project].dependencies` requirement moves
  only when no group names the package (the templates package keeps its `aeth-devkit`
  floor that way).
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
- **Local venv** - For a project with a `Cargo.toml`, `uv sync --inexact
  --reinstall-package <name>` runs alongside the workflow wait (in the foreground with
  `--no-wait`) so the venv's binary is the released version by the time the command
  returns; a pure-Python editable install needs no rebuild. Its failure is a warning,
  never a rollback.
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
- **Dockerfile refresh** - Before pinning, the committed `docker/Dockerfile` is compared with
  the template of the locked `devkit-container` (the venv is synced first if it lags
  `uv.lock`) and replaced, without a prompt, in its own commit when it differs
  (`chore(docker): refresh Dockerfile from devkit-container <ver>`), in the file's own line
  endings. Uncommitted edits ride on top through the same 3-way merge as the compose file; a
  Dockerfile that was never committed is refused rather than replaced. Only
  `[tool.docker].services` decides whether the step runs, so a project without services is
  pinned as before.
- **Commit & push** - Commits exactly the compose file (`chore: pin <package> to <ver>`),
  pathspec-limited so other staged work stays out; pushes the current branch. A dirty
  compose file gets the pin committed against HEAD's copy through a scratch index and the
  user's uncommitted edits merged back on top of the working tree (3-way, through git's
  clean/smudge filters so a CRLF checkout merges cleanly and stays CRLF); overlapping
  edits abort before anything is committed.

### `devkit-container`

Lives in its own repository, `AetherBreaker/devkit-container`, and is distributed as a wheel on
SFTPyPI. `setup-project` adds it to `[project].dependencies` of every project with
`[tool.docker].services`, locks it to the newest release this devkit accepts, and renders
`docker/Dockerfile` from the `devkit_container/template.Dockerfile` in the project's venv, so
the Dockerfile and the entrypoint the image installs are always the same version.
`docker-pin` refreshes a Dockerfile that drifted from the locked version before it pins. See
that repository's README for the binary's subcommands and the `[tool.docker]` schema.

### VS Code extension

`aeth.aeth-devkit`, in its own repository, `AetherBreaker/devkit-vscode`. Never published
to the marketplace: each build is a GitHub release there (`vN`, asset
`aeth-devkit-vscode-N.vsix`), cut by that repository's workflow from a `vN` tag push. Build 1
is `vscode-extension-v1` on this repository and stays published.

When `devkit setup-project` runs in a VS Code terminal (`TERM_PROGRAM=vscode`; force with
`--vscode`, disable with `--no-vscode`) with stdin a terminal and not `-y`, it
installs the newest compatible extension if none is present (a one-off
`code --install-extension`; an upgrade over a running one exits 2 asking you to reload the
window and run again), adds itself to `enable-proposed-api` in `~/.vscode/argv.json`
(restart VS Code once; this enables the floating Replace/Keep button), and then opens
each Docker change as a native diff instead of the typed prompt. Per hunk: `Accept` and
`Reject` CodeLens (the extension defaults `diffEditor.codeLens` to on; an explicit
`false` in your settings still wins); a decided hunk shows the same lines in both panels,
so its diff collapses, and the status bar counts `n of m hunks accepted`. Whole file, as the floating
editor buttons or the tab-bar icons until the proposal is live: `Apply accepted hunks`,
`Accept all hunks`, `Replace file`, `Replace all` (rest of the run), `Keep file`.
Closing the diff without deciding falls back to the terminal prompt for that file; Ctrl-C
in the terminal does the same, and a second Ctrl-C aborts (after any file write of its
own in progress completes, so nothing is left half-written; uncommitted edits to managed files
that a committing run was holding are lost, and a rerun re-standardises). Partial answers are
reassembled by the CLI from the accepted hunk indices; the extension never writes project
files. `--dry-run` opens every proposed change in one multi-diff review instead.

The extension also carries `Add to runtime-evaluated-base-classes` (Python editor context
menu), ported from the Drekker extension; setup-project reports the old junction and
`.vscode/extension/` folder when it finds them.

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

### `devkit-claude-hooks` and `devkit-poe-complete`

The Claude Code hooks (`devkit-hook <name>`) and the poe shell completion
(`devkit-complete`) live in their own repositories, `AetherBreaker/devkit-claude-hooks` and
`AetherBreaker/devkit-poe-complete`, released as wheels on SFTPyPI. `setup-project` installs
both into every project and wires them in (see **Hooks and completion** above); their READMEs
describe the hooks and the completion engine. `devkit hook` and `devkit complete` were removed
in 13.0.0: a project whose venv takes that devkit before `setup-project` has rewritten its
hook lines gets a usage error from every hook until `poe setup-project` runs.

### `devkit-templates`

The templates `setup-project` renders (`pyproject.toml`, the VS Code files, the ignore
files, `.env`, the compose scaffold, the workflows, `AGENTS.md`, the Claude settings,
`.mcp.json`) live in `AetherBreaker/devkit-templates`, a pure-Python `devkit_templates`
wheel on SFTPyPI whose only content is `templates/`. `setup-project` installs it into every
project (its first run on a project adds it to the dev group, locks it under the running
devkit and syncs) and reads it from the environment (see **Templates** above), so a
project's templates version is its `uv.lock`. aeth-devkit ships none since 14.0.0. The
package's `[project].dependencies` floor on `aeth-devkit` is its compatibility contract,
raised by hand; `poe lock` there moves only the dev-group pin.

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
`python/aeth_devkit` (poe tasks, remaining shell scripts). The setup crate's tests render
the snapshot under `crates/aeth-devkit-setup/tests/fixtures/templates`; to render a
templates checkout instead, pass `--templates-dir` or set `DEVKIT_TEMPLATES`. CI's `render`
job dry-runs the newest released templates through the working-tree binary.
