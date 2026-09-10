# devkit split

Date: 2026-09-08, revised the same day after review against the code. Status: design approved in
discussion; each numbered step in section 7 gets its own implementation plan, written in a separate
session. Nothing in it is provisional.

The wireguard mode for `devkit-container` has its own spec, which lands after step 1 of this
one and lives with the crate:
`devkit-container/docs/superpowers/specs/2026-09-08-container-wireguard-mode-design.md`. The
consuming project's view is `ScheduledReportAggregator/docs/superpowers/specs/2026-09-08-wireguard-db-access-design.md`.

## 1. Why

- **The repo is a kitchen sink.** Nine crates, a Python package, a VS Code extension and five
  workflows, releasing three artefact streams (`v*` wheel, `container-vN`, `vscode-extension-vN`)
  off one release event. Every session that edits any part loads all of it. The code is entirely
  LLM-written and the owner does not read Rust, so review happens through behaviour specs and
  tests, which are buried in the volume. Docs already lag code: `TODO.md` lists the sister-project
  Docker migration as pending, and it is done.
- **`devkit-container` is about to become security-sensitive.** Its wireguard mode adds
  root-phase private-key handling, interface bring-up, and PID 1 supervision of the app. Code
  with that job must be small enough to read top to bottom. The extraction happens before that
  code is written, not after.
- **Template changes should not require a devkit release.** Most template edits are content
  (standards prose, pins, lint rules), not code.

## 2. End state: six repositories

Names are working names; the extraction plans may rename. Every repository is devkit-managed: it
has a `pyproject.toml`, runs `setup-project` on itself, and releases with `devkit release`. Four
of the six publish a wheel to SFTPyPI, which is the one distribution mechanism for everything a
project consumes; the pin for each is the consuming project's `uv.lock`.

| Repo | Contents | Artefact and release | How consumers get it |
|---|---|---|---|
| `aeth-devkit` (slimmed) | `devkit` binary; crates `core`, `setup`, `release`, `pin`, `lock`; Python package (poe task table, two scripts); no templates | maturin `bindings = "bin"` wheel on SFTPyPI via `devkit release`; workflows `ci`, `release`, `claude` | dev dependency, `uv sync` |
| `devkit-container` | the crate, plus a `devkit_container` package holding the Dockerfile template as package data; `[tool.docker]` schema doc; smoke test | maturin bin wheel on SFTPyPI via `devkit release`, standard Rust release workflow | runtime dependency of Docker projects, added by `setup-project`; installed into the image by `uv sync` |
| `devkit-templates` | every template except the Dockerfile, as package data of a `devkit_templates` package; no code | pure-Python wheel on SFTPyPI via `devkit release`; CI renders through the latest released devkit | dev dependency, added by `setup-project` |
| `devkit-vscode` | the extension; a stub `pyproject.toml` | `vN` tags cut by a tag push; hand-written workflow moved from `vscode-extension.yml` | installed by `setup-project` from releases, as today |
| `devkit-claude-hooks` | Rust crate; binary `devkit-hook` | maturin bin wheel on SFTPyPI via `devkit release` | dev dependency; `.claude/settings.local.json` command lines |
| `devkit-poe-complete` | Rust crate; binary `devkit-complete` | same shape as hooks | dev dependency; the shell shims it installs |

## 3. What stays together, and why

**`setup`, `release`, `pin`, `lock` and `core` remain workspace crates in `aeth-devkit`.**

- *Diamond dependency.* They pass `&dyn Runner` and `&dyn Prompt` from `core` across crate
  boundaries. As separate repos each would pin `core` by git tag, and Cargo treats two tags of a
  git dependency as two unrelated crates: the moment `setup` and `release` pin different tags the
  trait objects are incompatible types and `devkit` does not build. Every `core` change would
  force a lockstep tag across four repos.
- *Co-evolution.* A typical change touches a template, `setup`, and `core` together. One
  workspace makes that one commit.
- *One wheel.* Consumers get one `devkit` on PATH. One workspace is what makes that one build.
- `pin` gains a workspace dependency on `setup` for the Dockerfile refresh (4.1); inside one
  workspace that edge costs nothing.

**No shared library repo.** The only code the departing crates share with `core` is the
`process::Runner` seam (`complete` and `hooks` use nothing else). A library repo whose export is a
test seam adds a dependency edge to every satellite to save roughly a hundred lines. Each
satellite carries its own copy. Revisit only if `setup` and `release` ever split, which would make
`core` a genuinely shared, evolving model.

**Hooks and completion are standalone binaries, not libraries pulled into `devkit`.** Nobody types
`devkit hook` or `devkit complete`: Claude Code runs hooks, the shell runs completion. The
principle for every satellite: functionality belongs to its own repo, installation belongs to
devkit. The library model would also cost a two-step release (tag the library, bump the pin,
release devkit) for every change.

## 4. Per-component specifics

### 4.0 The devkit packages, and what `setup-project` does for them

`devkit-templates`, `devkit-claude-hooks`, `devkit-poe-complete` and, for Docker projects,
`devkit-container` are features of devkit and part of its standardisation promise. `setup-project`
is the idempotent checkpoint that moves a stale project up to the standard, so for these four
packages, and only these, it adds the dependency when missing, advances the locked version, and
resyncs the venv. Every other dependency, `aeth-devkit` itself included, is `devkit lock`'s to
move.

**The constraint.** Every dependency resolution `setup-project` performs runs with
`aeth-devkit==<the running devkit's version>` as an additional constraint (a uv constraints file),
so devkit can never be upgraded as a side effect. Two outcomes follow:

- *An explicit floor cannot be met.* The project's `pyproject.toml` or the pyproject template names
  a version (`devkit-claude-hooks>=2.1`) every release of which requires a newer devkit than the
  one running. uv reports that no resolution exists; `setup-project` stops, cancels the run
  (nothing has been rendered yet), and says: run `devkit lock`, then rerun `setup-project`. The
  requirement cannot be satisfied any other way without devkit updating itself.
- *A `{latest}` specifier is throttled.* The pyproject template may write `{latest}` in place of a
  version, meaning "the newest this devkit can use". uv under the constraint chooses the newest
  release that still works with the running devkit, and that version is what renders into the
  project's floor. When the index holds a release newer than the one chosen, `setup-project`
  warns, naming both versions and suggesting `devkit lock` followed by a rerun; the run continues.

devkit parses no package metadata in either branch: the two cases are told apart by what uv
returns, a resolution failure versus a chosen version below the index's newest (`IndexClient`
already reports the newest). How the floor is written and how the lock is invoked is the plan's
call; the constraint and the two outcomes are not.

**Lock and venv.** `uv sync --frozen` installs the result. A `uv.lock` whose `aeth-devkit` entry is
not the running version means the venv is out of step with the lock; that is the same stop, with
`uv sync` as the remedy, rather than letting the lock step move devkit's entry to match the
binary.

**Order within a run.** The templates package first, because nothing renders without it: on a
project that has never had it, `setup-project` adds it by its constant name (no template
involved), locks under the constraint, syncs, and reads the templates from the venv. Then the
pyproject merge, which adds the other floors where missing; then lock and sync for those; then
every other template, since the hook and completion command lines look for the installed
binaries. Lock and pyproject changes are committed quietly against `HEAD` the way `devkit lock`
commits (`aeth_devkit_core::commit`), `uv.lock` joins the committable set, and the run's own
commit follows.

**`--check` is removed.** Nothing invoked it: no template workflow, no hook, no poe task. It was
`--dry-run` plus an exit code and a suppressed VS Code review. `--dry-run` stays (testing and
development of devkit) and exits 0. Idempotence is the acceptance test: a second plain run
immediately after the first reports nothing to do.

**`[tool.devkit]`** is the table for devkit-level project settings, should any arrive; the first
candidate is the Dockerfile opt-out in 4.1's TODO entry. Nothing is written there yet. In
particular no templates version is recorded anywhere but `uv.lock`.

**Self-dependency.** `setup-project` never adds a package whose name is the project's own, so the
templates, hooks and completion repos are devkit-managed without depending on themselves.

**Sources.** Every project's index is `explicit = true`, so each devkit package also gets a
`[tool.uv.sources]` entry, naming the `SFTPyPI` index the way every project's existing
`aeth-devkit` entry does. Standardising index names is out of scope; the new repos copy
`aeth-devkit`'s `[[tool.uv.index]]` block as it is.

### 4.1 `devkit-container`

- The crate moves verbatim: `app-extra`, `readme`, `run` all stay. It already depends on no
  workspace crate (its own small pyproject parser is deliberate). The wireguard mode is a later
  change to this repo, specified separately.
- **Packaging.** maturin `bindings = "bin"` with `python-source`, the shape `aeth-devkit` has: a
  `devkit_container` package whose only content is an empty `__init__.py` and
  `template.Dockerfile` as package data. The template is a real file so it can be inspected and
  rendered by hand from the venv. The Dockerfile is the binary's usage contract, and they version
  together by being one wheel.
- **Distribution.** Wheel on SFTPyPI, released by `devkit release` through the standard Rust
  release workflow template. The manylinux x86_64 wheel is what the image installs; the base image
  is glibc bookworm-slim, so the static musl build goes. The Windows wheel is required, not
  optional: the package is a runtime dependency and `uv sync` installs it on Windows dev machines,
  where only the query subcommands run (`run` refuses off Linux, as today).
- **In consuming projects** it is a runtime dependency. The pyproject template unions
  `devkit-container>={latest}` into `[project].dependencies` under an `if-docker` gate, with the
  `[tool.uv.sources]` entry from 4.0. The pin is `uv.lock`; 4.0 governs how it advances.
- **The Dockerfile template** has no `ADD`, no `chmod`, no `COPY` of a binary. The builder stage
  runs `uv sync --frozen --no-dev --no-install-project` once without extras, which installs the
  package; asks `/app/.venv/bin/devkit-container app-extra`; then syncs again with the extras and
  installs the project. The final stage copies the venv as today and its entrypoint is
  `/app/.venv/bin/devkit-container run`. `{container_version}`, `pinned_container_version`,
  `newest_container_version` and `DEVKIT_REPO` are deleted from `setup`; `setup-project` no longer
  calls `gh` for the Dockerfile.
- **`setup-project`** reads the template from the installed package (it asks the venv's Python
  where `devkit_container` lives, the pattern `templates::from_python()` uses today), substitutes
  `{python_dir}`, applies the gates, and hands the result to the existing whole-file consent flow:
  a unified diff in the terminal, or the VS Code reviewer with per-hunk accept and reject,
  replace, keep, and replace-all for the rest of the run. The version rendered is the version the
  image installs, by construction.
- **`docker-pin`**, before it pins, renders the same template from the venv and compares it with
  the committed `docker/Dockerfile`. On any difference it replaces the file, with no prompt, in
  its own commit ahead of the pin commit in the same push, with the same dirty-file handling as
  the compose file (edit `HEAD`'s blob, commit, merge back to the worktree). A venv whose
  installed package version differs from the locked one is synced first (`uv sync --frozen`), so
  a stale venv cannot refresh the Dockerfile to the wrong version. The rendering is a public
  function in `setup`; `release-and-pin` inherits all of this.
- **Consequence, stated so nobody discovers it at deploy time:** `docker/Dockerfile` is
  devkit-owned. `setup-project` still shows its diff and asks, because the reviewer is generic and
  the diff shows what is about to change, but a hunk kept there does not survive the next
  `docker-pin`. `TODO.md` gains an entry: consider a mechanism for project-specific Dockerfile
  edits, or for opting out of Dockerfile management (a `[tool.devkit]` setting when it comes).
- **Smoke test.** Moves with the crate. It builds the wheel locally and points the scratch app's
  `[tool.uv.sources]` at that file, so the image exercises the exact artefact a release publishes,
  instead of swapping the `ADD` line for a `COPY`.
- The `container-v1` and `container-v2` releases on `aeth-devkit` stay published. Dockerfiles that
  still pin them keep building until their project's next `setup-project` run, which offers the
  new shape as an ordinary whole-file diff.

### 4.2 `devkit-templates`

- Contents: everything under `python/aeth_devkit/templates` except `template.Dockerfile`, as
  package data of a `devkit_templates` package (an empty `__init__.py`, no code), built by a
  pure-Python backend (`uv_build` or hatchling; the plan's call). The two shell scripts under
  `scripts/` stay in devkit: they are code the poe tasks call, not standards.
- Distribution: wheel on SFTPyPI via `devkit release`, through the non-Rust release workflow
  template. A dev dependency of every project; the pin is `uv.lock`, advanced per 4.0. Releasing
  is the ceremony that makes a change flow: push freely, release when it is meant to reach
  projects.
- **Language compatibility.** Templates are not passive text: the placeholders, the `if-*` line
  gates, the compose `service-block` markers, the `if-dep` and `if-docker` table markers, the
  AGENTS.md block markers, and the fixed inventory of template files and their merge strategies in
  `setup`'s `run_with` are a language that `setup` implements. The package declares
  `aeth-devkit>=X` as an ordinary dependency and raises it in the same commit that first uses a
  new placeholder, gate, marker, file or merge shape. A content change needs nothing; a shape
  change is a devkit change first (add support, release), then a templates release that raises
  its floor and uses the feature. The 4.0 constraint turns an incompatible pair into a stop or a
  throttled choice, so a new placeholder never renders as literal braces into a project file.
- `setup-project` reads the templates from the venv. `--templates-dir` and `DEVKIT_TEMPLATES`
  remain as the override that renders a directory instead: a sibling checkout for editing
  templates and `setup` together, and the templates repo's own CI.
- **Templates CI.** On every push, render the working tree through the latest released devkit
  (`--templates-dir .` with `--dry-run`) against scratch projects (pure Python, Rust, Docker) and
  fail on a render error or an unresolved placeholder. The compatibility guard lives here, before a
  release exists.
- No git fetch, no tag as a distribution unit, no per-user cache, no snapshot in the devkit wheel,
  no recorded tag in the project: `uv.lock` is the record.

### 4.3 `devkit-vscode`

- `vscode-extension/` and its workflow move. A stub `pyproject.toml` (name, version, dev group
  with `aeth-devkit`, the poe include) makes the repo devkit-managed; the Python tooling tables the
  pyproject template merges in are inert there.
- The consent protocol already carries a version: every request has `protocol: 1`
  (`setup/src/vscode/protocol.rs`), the extension checks it (`consent.ts`, `protocolMismatch`),
  and its error response retires the reviewer for the run. Nothing new is needed there.
- **Versioning and release.** Integer build numbers continue: the new repo's tags are `vN`,
  starting at `v2`, and the vsix is stamped `N.0.0` and named `aeth-devkit-vscode-N.vsix` as
  today, so `install.rs` changes only `REPO` and `TAG_PREFIX`, and the `MIN_EXTENSION_VERSION`
  comparison is untouched. `devkit release` does not fit a vsix (it bumps a Python version and
  verifies the result on a package index), so the hand-written workflow stays and fires on a tag
  push: build, typecheck, test, package, create the GitHub release with the asset. The repo is
  public, as `aeth-devkit` is, which is what keeps the anonymous vsix download working. A stronger
  release workflow for it is deferred; `TODO.md` records that.
- `package.json`'s `repository` URL changes with the move.

### 4.4 `devkit-claude-hooks` and `devkit-poe-complete`

- Each repo contains everything its binary needs to function, including the shell shim text
  `devkit-complete install` writes; `setup-project`'s job is to install them.
- Each crate becomes its own repo with a private process seam: the trait, the system runner, and
  as much of the recording runner as its tests use. No dependency on `core`.
- Each gets a minimal `pyproject.toml` with maturin `bindings = "bin"`, publishing binaries
  `devkit-hook` and `devkit-complete`. That is the shape `aeth-devkit` itself has, so both are
  devkit-managed and released with `devkit release` through the standard Rust release workflow.
- **`setup-project` installs them.** The dev group gains both at `>={latest}` and 4.0 advances and
  syncs them. The `.claude/settings.local.json` template commands become `devkit-hook <name>`,
  through a placeholder that resolves to the venv's `devkit-hook` the way `{devkit_bin}` resolves
  `devkit` today. `setup-project` runs `devkit-complete install` for the shells detected on the
  machine instead of telling the user to. `hook_key` in `json_merge` recognises the new command
  form as well as the old `devkit hook <name>` form, so an existing entry is updated in place
  rather than duplicated beside a broken one.
- **Dispatcher.** Remove the `Complete` and `Hook` variants and the `wants_update_check` special
  cases. This is a breaking change and a major version bump. The window is accepted: a project
  whose venv picks up the new devkit before `setup-project` has rewritten its hook lines gets a
  clap usage error, exit 2, from `devkit hook`, which Claude Code treats as a blocking hook error
  on every Edit, Write and Bash. The release note says so: run `poe setup-project` from a plain
  terminal before the next Claude Code session. No forwarding shim, no exit-0 stub.
- The shims and profile lines `devkit-complete install` writes call `devkit-complete query`; the
  "no global install" property holds: the venv on PATH supplies the right version at Tab time and
  at hook time.

### 4.5 `aeth-devkit` after the split

- Crates: `aeth-devkit`, `core`, `setup`, `release`, `pin`, `lock`. Python package: the poe task
  table and the two scripts; no templates directory. Workflows: `ci` (drop the container-smoke and
  extension jobs), `release`, `claude`; `devkit-container.yml` and `vscode-extension.yml` are
  deleted.
- `templates::locate` becomes: the override, else the venv's `devkit_templates` package; the
  source-tree fallback goes with the directory. One "where does the venv keep this package"
  helper serves `devkit_templates` and `devkit_container`.
- Its own `pyproject.toml` gains the three dev-group packages through its own `setup-project`
  run, like every project.
- `TODO.md`: drop the entries the split makes moot, including the stale migration entry; add the
  Dockerfile opt-out entry from 4.1 and the extension release-workflow entry from 4.3.
- **`WORKSPACE.md`** at the repo root: how to mirror the whole set on another machine. It lists
  every repo with its clone command into `D:\SFT Software Projects\<name>`, says which repos need
  the publishing `.env` copied in (the four wheel repos and `aeth-devkit`), and ends with
  `uv sync` and `poe setup-project` in each. Created in step 1 with the first new repo; each later
  step adds its own.
- Optional, decided separately: stop baking the poe task table in `build.rs`. The generated file
  is a plain dict; authoring it directly removes a build-time Python dependency, the
  `poethepoet-tasks` build requirement, and the regeneration test.

## 5. Pin policy

One mechanism (a floor in `pyproject.toml`, the exact version in `uv.lock`) with a stated policy
per pin:

| Pin | Written where | Advances |
|---|---|---|
| `devkit-container` | `[project].dependencies` floor under `if-docker`; `uv.lock` | by `setup-project`, to the newest the running devkit accepts (4.0) |
| `devkit-templates` | dev group floor; `uv.lock` | same |
| `devkit-claude-hooks`, `devkit-poe-complete` | dev group floors; `uv.lock` | same |
| `aeth-devkit` | dev group floor; `uv.lock` | `devkit lock` only |
| extension release | at install time | newest compatible at install (existing) |

## 6. Cross-repo contracts

Each has one owner and a version, and the consumer checks the version.

| Contract | Owner | Consumer |
|---|---|---|
| Dockerfile template and `[tool.docker]` schema | `devkit-container` | `setup-project` and `docker-pin`, from the installed package; version-matched by `uv.lock` |
| consent protocol (the existing `protocol` field) | `devkit-vscode` | `setup` |
| template language (placeholders, gates, markers, file inventory, merge shapes) | `setup` | `devkit-templates`, via its `aeth-devkit>=` dependency |
| `devkit-hook <name>` command line and payload | `devkit-claude-hooks` | the settings template; a floor in the pyproject template when a template needs a newer hook |
| shim to `devkit-complete query` wire format | `devkit-poe-complete` | internal; it installs its own shims |

## 7. Sequence

Each step is its own plan, branch and PR.

1. Extract `devkit-container` as a wheel, and build 4.0 in `setup` in its general form with the
   container as its first package: add when missing, the constraint, `{latest}`, sync, quiet
   commit, `uv.lock` committable. The Dockerfile from the venv; the `docker-pin` refresh. Unblocks
   the wireguard spec.
2. Extract `devkit-vscode`.
3. Extract hooks and completion: the wheels published first, then the templates that reference
   them, then the devkit major that removes the subcommands.
4. `devkit-templates` as a package; `setup` reads from the venv; `--check` removal; templates CI.
5. Slim `aeth-devkit`: constants, CI, TODO, README; optionally the bake removal. Scoped in 7.1
   as it stands after step 4.

Steps 2, 3 and 4 are independent of each other; 3 and 4 use the 4.0 machinery from step 1. Step 5
is last. The wireguard spec follows step 1 and is independent of 2 to 5.

Publication order inside any step that introduces a package: the wheel exists on SFTPyPI before
any template or devkit release references it, or `uv sync` fails in every project that picks the
reference up.

**Repository creation** is done by the plan, not by hand. Each new repo is created with
`gh repo create AetherBreaker/<name> --public` (default branch `main`) and cloned to
`D:\SFT Software Projects\<name>`. History comes along: `git filter-repo` on a throwaway clone
keeps the commits that touched the moved paths, renamed into their new places; if that does not
come out clean, the repo starts from one initial commit instead. Each `pyproject.toml` copies
`aeth-devkit`'s `[[tool.uv.index]]` block and starts at version 1.0.0. The SFTPyPI publish
secrets (`UV_INDEX_SFTPYPI_USERNAME`, `UV_INDEX_SFTPYPI_PASSWORD`) are piped from `aeth-devkit`'s
`.env` into each wheel repo with `gh secret set`, never printed. The Claude Code OAuth token is
set by hand later; the Claude workflow is installed regardless. Then `setup-project`, and one
release.

### 7.1 Step 5 as it stands after step 4 (written 2026-09-10)

Steps 1 to 4 are done and released (aeth-devkit 14.0.0, devkit-templates 1.1.0); the six repos
are on 14.0.0. Much of 4.5 happened inside those steps, so step 5 is smaller than its line
suggests. ~~Consumers are still not migrated: they move after step 5, not during it.~~ Downstream
consumer migration (not the devkit satellite repos) is done by the owner by hand, outside any
plan; no step of this spec covers it.

**Already done, do not redo**: the CI jobs 4.5 names are gone and `devkit-container.yml` /
`vscode-extension.yml` are deleted; the templates directory is gone and `templates::locate`
reads the venv package (an override, else `devkit_templates`); `aeth-devkit`'s own
`pyproject.toml` carries the dev-group packages through its own `setup-project` run;
`WORKSPACE.md` exists and lists all six repos; every workflow, job and step across the six repos
is named by what it runs and the convention is in the AGENTS.md template (GitHub Workflow
Naming). CI has no structural work left; the `Templates:` job in `aeth-devkit`'s `ci.yml` is
the cross-repo check and stays.

**Decided**: the bake stays. `build.rs` keeps generating `_tasks_generated.py` from
`_tasks_source.py`, the regeneration check in CI stays. Step 5 does not touch it.

**What remains**, each its own commit on `main` (small fixes go there, no branch):

1. *Constants and leftover code*: `REMOVAL-CANDIDATES.md` lists what the split left behind,
   with the evidence and the removal cost. Rule on each `aeth-devkit` entry (remove, or keep
   with the reason written where the code lives): `packages::DEVKIT` (only tests name it),
   `Changes::problems` as a list apart from `warnings` (`--check` went in 14.0.0), the
   `legacy_hook_key` / `matches_key` pre-Rust hook migration, and the two Docker gates
   `if-docker` / `if-docker-services` (a template change too, so a devkit-templates release
   if it goes). The `devkit-poe-complete` entries (`tasks` / `args` subcommands, the
   `OLD_DEVKIT_POWERSHELL_LINE` migration) are the same review in that repo. When every entry
   is ruled on, delete the file: it existed to collect candidates during the split.
2. *TODO.md*: drop the stale "Release 7.0.0, then migrate downstream projects per README"
   entry and any other the split made moot; the 4.3 extension release-workflow entry is
   present ("devkit-vscode: a stronger release workflow"), the 4.1 Dockerfile opt-out entry
   must be checked for and added if missing. The 2026-09-10 entry to template `ci.yml` and
   have `setup-project` install it when the project runs tests stays.
3. *README.md*: the feature reference describes only what `aeth-devkit` still contains. The
   four satellite sections (`devkit-container`, VS Code extension, hooks and completion,
   `devkit-templates`) shrink to what `setup-project` does with each package plus a pointer
   to that repo's README. "Migrating from `poe-tasks`" is a 7.0.0-era section: keep it only
   if a `poe-tasks` project still exists, else drop it and the `aeth-devkit>=7.0.0` example
   with it.
4. *Release* `aeth-devkit` (patch or minor; no contract changes unless item 1 removes a gate).
   ~~Then the consumer migration per section 9: `aeth_ext`, `IMAPReportCollector`,
   `ScheduledInvoiceProcessor`, `ScheduledReportAggregator`, `timeclock_entry_processor`,
   each through `poe setup-project`, twice, the second run reporting nothing to do.~~ (Owner's
   job, by hand; see the note at the top of 7.1.)

Constraints that bind every item: the project's Bash hook refuses `uv add` / `uv remove` /
`uv lock` (use `uv sync`, `poe lock`, `setup-project`); `poe release` note words cannot start
with a dash and must avoid apostrophes; edit scripts go to the scratchpad, never inline
multi-line Python in a heredoc.

## 8. Rejected alternatives

- **Library repos for `setup`, `release`, `pin`, `lock`.** Diamond dependency through `core`;
  co-evolution; one wheel. See section 3.
- **A shared `core` library repo.** Only the `Runner` seam is shared with anything that leaves.
- **Hooks and completion as libraries pulled in at devkit release.** Two-step release for no
  consumer benefit, since nobody invokes them by the `devkit` name.
- **Forwarding shims or exit-0 stubs for `devkit hook` and `devkit complete`.** The window is
  accepted instead; the shims would be functionality living in the wrong repo.
- **A uv workspace monorepo.** uv workspaces share one lockfile across Python packages that
  co-develop; devkit has one Python package and the rest is Rust and TypeScript. The Cargo
  workspace is already the monorepo for the Rust parts. Neither pain (per-session context, release
  coupling) is addressed by a monorepo; the current repo already simulates per-component releases,
  and that simulation is the awkward part.
- **Templates fetched from git, at HEAD or at tags, with a per-user cache and a snapshot in the
  devkit wheel.** HEAD breaks idempotence and has no nameable unit. The snapshot had nowhere to be
  produced: `aeth-devkit`'s release workflow is itself template-owned and replaced on drift, and a
  build-time fetch is not reproducible; it helped only a cold offline cache. A package on the index
  gives the same flow with tooling that already exists.
- **A recorded templates tag in `pyproject.toml`, and `--check`.** Nothing consumed either;
  `uv.lock` is the record.
- **A template spec-version integer, or a min-devkit file.** The package's own `aeth-devkit>=`
  dependency says the same thing, to the resolver, with no second numbering scheme.
- **`aeth-devkit` depending on a templates version range instead.** The templates repo
  dev-depends on `aeth-devkit`, and a project cannot depend on its own name.
- **A static musl binary fetched by `ADD` from a pinned GitHub release.** The wheel on SFTPyPI and
  `uv.lock` replace the URL pin, the `container-vN` tag stream and the `gh` lookup, and the image
  is glibc anyway.
- **The Dockerfile template embedded in the binary.** Not inspectable from the venv.
- **A no-Python mode for `setup-project`** so repos without a Python package can be managed. A
  stub `pyproject.toml` is cheaper.

## 9. Done means

- ~~Every sister project, after migration: a second `poe setup-project` immediately after the first
  reports nothing to do, and Claude Code hooks and poe completion work from the new binaries.~~
  (Consumer migration is the owner's, by hand; not a done-criterion of any step.)
- `ScheduledReportAggregator` builds with `devkit-container` installed from SFTPyPI.
- The container smoke test is green in the container repo's CI.
- A templates release flows to a project through `setup-project` with no devkit release. A
  templates release that needs a newer devkit produces the warning (throttled `{latest}`) or the
  stop (explicit floor), never a rendered file with literal braces.
- `docker-pin` refreshes a drifted Dockerfile before pinning.
- `aeth-devkit` CI has two jobs fewer and its README describes only what it still contains.
