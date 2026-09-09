# devkit-container extraction: autopsy

Step 1 of the devkit split (`docs/superpowers/specs/2026-09-08-devkit-split-design.md`,
plan `docs/superpowers/plans/2026-09-08-extract-devkit-container.md`). This report compares
`aeth-devkit` at `e5dc3f0` (the last commit before the branch) with `537f9ec` (PR #16
rebase-merged, 21 commits) and records what shipped as `aeth-devkit 12.0.0` and
`devkit-container 1.0.0`. Part 1 is the architecture: what lives where, what depends on what,
what flows between repositories. Part 2 is the behaviour: what each command and artefact did
before and does now. Parts 3 to 5 are what stayed, what departed from the spec, and what is
open.

Sizes, for scale: 43 files, +4538/−1762 lines including the spec and plan; code, templates,
workflows and manifests alone are 36 files, +1847/−1441. The container crate that left was
931 lines in 8 files. `#[test]` functions went from 537 to 555 despite the crate's tests
leaving with it.

---

## Part 1: Architecture

### 1.1 Repository topology

**Before:** one repository. `aeth-devkit` held nine workspace crates (`aeth-devkit`,
`core`, `setup`, `release`, `pin`, `lock`, `complete`, `hooks`, `container`), the Python
package with every template, the VS Code extension, and five workflows. One release event
(`v*` tag) fanned out into three artefact streams: the devkit wheel, `container-vN` (a static
musl binary attached to a GitHub release, cut by `devkit-container.yml` whenever the crate or
`Cargo.lock` had changed since the previous container tag), and `vscode-extension-vN`.

**After:** two repositories.

| | `aeth-devkit` (12.0.0) | `devkit-container` (1.0.0, `AetherBreaker/devkit-container`) |
|---|---|---|
| Contents | eight crates; Python package and templates; VS Code extension | the crate, verbatim (`main.rs`, `mounts.rs`, `prepare.rs`, `pyproject.rs`, `run.rs`, `tests/entrypoint.rs`, `tests/docker_smoke.rs`) plus `python/devkit_container/{__init__.py, template.Dockerfile}` |
| Artefacts | `v*` wheel on SFTPyPI; `vscode-extension-vN` | `v*` wheel on SFTPyPI (manylinux x86_64, win_amd64, sdist) |
| Workflows | `ci`, `release`, `claude`, `vscode-extension` (`devkit-container.yml` deleted) | `ci` (rust matrix, wheel job, container smoke), `release`, `claude` |
| History | unchanged | `git filter-repo` kept the 13 commits that touched the crate; the six aeth-devkit tags it re-pointed were deleted before the first push |

The new repository is itself devkit-managed: `setup-project` ran on it, it releases with
`devkit release`, and its `.env` carries the SFTPyPI publish credentials. It is the first
satellite of the split and the pattern the templates, hooks and completion repos will follow.

### 1.2 Distribution model: from a downloaded binary to a locked dependency

This is the structural change everything else follows from.

```
BEFORE                                            AFTER
aeth-devkit release (v11.x)                       devkit-container release (v1.x)
  └─ devkit-container.yml                           └─ standard release workflow
       └─ GitHub release container-vN                    └─ wheel on SFTPyPI
            └─ asset: devkit-container-…-musl
                                                  consumer pyproject.toml
consumer docker/Dockerfile                          [project].dependencies += "devkit-container>=1.x"
  ADD …/container-v{N}/devkit-container /app/…      [tool.uv.sources] devkit-container = [{index=…}]
  RUN chmod +x …                                  consumer uv.lock  ← the pin
  ENTRYPOINT ["/app/devkit-container", "run"]
                                                  consumer docker/Dockerfile
the pin: a number inside a URL in the Dockerfile    RUN uv sync --frozen --no-dev --no-install-project
                                                    ENTRYPOINT ["/app/.venv/bin/devkit-container", "run"]
```

- The container binary is now a **runtime dependency of the consuming project**, installed
  by `uv sync` into the image's venv (and into developers' venvs on Windows, where only the
  query subcommands run). It was an out-of-band asset fetched at image build.
- The **pin moved from the Dockerfile to `uv.lock`**, which is where every other dependency
  of the project is pinned. Nothing in the Dockerfile names a version any more.
- The **Dockerfile template moved into the wheel** as package data. Template and binary are
  one artefact and version together by construction; the template a project renders is the
  one that matches the binary its image installs, because both come from the same locked
  package in the same venv.
- The base image is glibc, so the static musl build is gone; the manylinux wheel is what the
  image installs. The smoke test builds a musl wheel locally under a generic `linux` tag only
  so it can run on a Windows host; CI's wheel job covers the manylinux path.

### 1.3 The `aeth-devkit` workspace

Removed: `crates/aeth-devkit-container/`, its `nix` workspace dependency,
`python/aeth_devkit/templates/docker/template.Dockerfile`, `.github/workflows/devkit-container.yml`,
and the `container-smoke` CI job. `setup` lost its container-pin machinery
(`pinned_container_version`, `newest_container_version`, `DEVKIT_REPO`, the `gh` tag
lookup): `docker/static_files.rs` went from 232 lines to 175.

Added, all in `crates/aeth-devkit-setup`:

- **`packages.rs` (497 lines), the spec 4.0 machinery.** `DevkitPackage { name, import_name }`
  with the constants `CONTAINER` and `DEVKIT`; `active(ctx)` (which packages a project should
  carry; the container when `[tool.docker].services` is non-empty, never the project's own
  name); the `Venv` seam (below); lock readers (`locked_version`,
  `locked_registry_version`); `latest_requested(template)`; `advance(...)`, the package step
  itself; and `resync_after_replay(...)`.
- **The `Venv` seam.** `trait Venv { fn installed(&self, root, &DevkitPackage) -> Option<Installed { dir, version }> }`.
  `SystemVenv` asks the project's own interpreter (`UV_PROJECT_ENVIRONMENT` or `<root>/.venv`)
  in one spawn: the package directory from `__file__` and the version from
  `importlib.metadata`. `StubVenv(HashMap<import_name, Installed>)` for tests. The interpreter
  beside the devkit binary or on PATH is deliberately never consulted for a project's
  packages; it can answer from another environment. The same `probe` now serves
  `templates::locate` for devkit's own package data.
- **`crate::Deps { docker: docker::Deps, index: &dyn IndexClient, venv: &dyn Venv }`.** Before,
  `run_with` took only `docker::Deps { runner, prompt, reviewer, mode }`. The package step
  needs an index client (the "newer release exists" nudge) and the venv; both were hoisted to
  a setup-level struct. `docker::apply` now receives the setup-level struct.
- **`Changes.venv_synced`** and `uv.lock` in `git::committable`.

`crates/aeth-devkit-pin` gained a workspace dependency on `aeth-devkit-setup`, the first
command-to-command edge in the workspace (every other command depended only on `core`). It
imports `ProjectContext`, `docker::static_files::{render, normalize_newlines}`,
`context::services_key` and `packages`. `pin::Deps { runner, index }` became
`{ runner, index, venv }`. The spec accepted this edge on the grounds that inside one workspace
it costs nothing; both reviews noted it is what let setup-project's configuration rules leak
into the pin path, which 2.3 below describes.

```
BEFORE                                AFTER
devkit ─┬─ setup ── core              devkit ─┬─ setup ── core
        ├─ pin ──── core                      ├─ pin ──┬─ core
        ├─ release ─ core                     │        └─ setup
        ├─ lock ──── core                     ├─ release ─ core
        ├─ complete ─ core                    ├─ lock ──── core
        ├─ hooks ─── core                     ├─ complete ─ core
        └─ container (no workspace deps)      └─ hooks ─── core
```

`core` gained two things: `pyproject::source_index_name` became public (the context reads
which index the project's `aeth-devkit` comes from), and `RecordingRunner` gained
`script_with_effect`, a scripted answer that also runs a closure in the call's working
directory, so a test can stand in for a tool that writes files (uv rewriting `uv.lock`).

### 1.4 The template system

The pyproject template touches runtime dependencies and sources for the first time, and the
merger grew the vocabulary for it:

- **`{latest}`** in a template requirement (`devkit-container>={latest}`): "the newest release
  the running devkit accepts". The merger writes only the bare name; `advance` writes the real
  floor once uv has chosen. The placeholder is scrubbed on every copy path (whole-table clone,
  fresh array, union) so it cannot reach a project file.
- **`# setup-project: if-docker-services`**, narrower than `if-docker`. `if-docker` fires on a
  listed service *or* Docker files on disk (it seeds the `[tool.docker]` switch);
  `if-docker-services` fires only when `[tool.docker].services` names something. A runtime
  dependency waits for the switch to be set.
- **`{devkit_index}`**: the index the project's own `aeth-devkit` source names (`SFTPyPI` when
  there is none). The `[tool.uv.sources]` entry for the container is written against it, so
  a project whose index is called something else still locks.
- **Marker validation.** An unknown `setup-project:` marker is an error at merge time rather
  than a gate that silently never fires.
- **Inline tables are promoted.** A project that wrote `[tool.uv] sources = { … }` inline
  had that value replaced by any template table of the same name; it is now converted to a
  header table and merged into.

`ProjectContext` gained `devkit_index`. `templates::substitute` gained `{devkit_index}`.

### 1.5 The quiet-commit machinery

`uv.lock` joined the committable set, so on a committing run it is staged to HEAD's copy
like `pyproject.toml`, the tools run on clean input, and the user's uncommitted edits are
replayed afterwards (or the run is rejected as overlapping). Two consequences were designed
in during review: the package step records that it synced (`Changes.venv_synced`), and after
the replay or the rollback the venv is synced again when the user's lock had been staged, so
it follows the lock that is actually on disk.

### 1.6 Tests and fixtures

- `crates/aeth-devkit-setup/tests/packages.rs` (new, 13 tests): the package step against
  scratch projects with the `uv` calls recorded.
- `tests/fixtures/docker/template.Dockerfile`: a fixture copy of the package's template,
  standing in for the venv through `StubVenv`.
- The pin tests gained a Docker fixture set (a committed Dockerfile, a lock naming the
  container, a stub venv) and nine refresh tests.
- The smoke test moved with the crate. It no longer swaps the Dockerfile's `ADD` for a
  `COPY`; it builds this checkout's wheel, gives the scratch app the wheel as a path-sourced
  dependency (because `uv sync --frozen` removes anything the lock does not name, a
  `pip install` between syncs did not survive), and builds the image from the template as
  shipped.

---

## Part 2: Functional changes

Each entry: what did it, what does it now, and where the moved piece went.

### 2.1 `setup-project`

**The Dockerfile is rendered from the project's venv, not from devkit's templates.**
Before, the Docker step loaded `docker/template.Dockerfile` from the installed
`aeth_devkit` package, filled `{container_version}` from the file's existing
`container-v<N>` URL or, failing that, from the newest `container-v*` tag on devkit's
repository via `gh api` (provisionally `1`, with a note, before any existed; a failed `gh`
call left the file alone as a `problem`), and offered the result through the consent flow.
Now it asks the venv where `devkit_container` is installed, reads
`devkit_container/template.Dockerfile` from there, substitutes `{python_dir}` (the only
placeholder left), and offers it through the same consent flow. `setup-project` no longer
calls `gh` for the Dockerfile and never writes a version into it. When the package is not in
the venv (a dry run before adoption) the file is skipped with a note.

**`setup-project` has a package step.** It did not touch runtime dependencies, run `uv`, or
touch the venv before. Now, for a project whose `[tool.docker].services` is non-empty, after
the pyproject merge and before the Docker step, it:

1. refuses when `uv.lock` pins an `aeth-devkit` other than the running one (the venv is out of
   step with the lock; the remedy is `uv sync`), and reports the same as a `problem` on a dry
   run so `--check` fails where a plain run would;
2. runs `uv lock --upgrade-package devkit-container --upgrade-package aeth-devkit==<running>`,
   so the container moves to the newest release the running devkit accepts while devkit
   itself cannot move; an unmeetable floor stops the run with "run `devkit lock`"; once the
   package is already locked, a failed refresh (offline) is a warning and the run continues
   on the locked version;
3. writes the floor `devkit-container>=<locked>` into `pyproject.toml` for a bare or `>=`
   requirement locked from an index (a `~=` clause, extras, markers or a path source are left
   as written, with a note), then runs a plain `uv lock` so the lock's `requires-dist`
   metadata records the new specifier;
4. warns when the index holds a newer release than the one uv could choose, naming both;
5. runs `uv sync --frozen` when the lock changed or the venv's installed version differs from
   the locked one, and stops if the locked version is still not in the project's environment
   afterwards (naming `UV_PROJECT_ENVIRONMENT` as the thing to check);
6. records `uv.lock` as a managed file, so it is committed with the run and a gitignored lock
   is warned about.

**Dependency and source are merged from the template.** `[project].dependencies` gains
`devkit-container` and `[tool.uv.sources]` gains `devkit-container = [{ index = "<devkit_index>" }]`,
both under `if-docker-services`. A project with Docker files but no listed service gets the
`[tool.docker]` switch seeded (as before) and nothing else.

**Committing runs stage `uv.lock`.** The run's commit now carries `uv.lock` alongside
`pyproject.toml`; uncommitted edits to the lock are replayed on top, or the run is rejected
as overlapping, exactly as for every other managed file; and the venv is resynced to the
lock the user gets back.

Everything else the command does (VS Code review, gitignore, launch/tasks, AGENTS.md,
workflows) is unchanged.

### 2.2 `docker-pin`

**Before:** matched services, resolved the version, edited and committed the compose file,
pushed. It never looked at the Dockerfile or the venv.

**Now**, additionally, before anything is written: when `[tool.docker].services` names
something and `uv.lock` names `devkit-container`, it syncs the venv if its installed
version lags the lock (re-checking afterwards), renders the template from the venv, and
compares it with the committed `docker/Dockerfile` (HEAD's copy when the working copy has
edits of its own). A difference is replaced without a prompt in its own commit,
`chore(docker): refresh Dockerfile from devkit-container <ver>`, ahead of the pin commit in
the same push; uncommitted edits ride on top through the same 3-way merge the compose file
gets, and overlapping edits abort before anything is written. The refresh is decided before
the already-pinned short-circuit (so an already pinned project still gets the refresh
committed and pushed), written after the behind-origin preflight, and the compose merge is
computed before the refresh commits (so a compose overlap strands nothing). A Dockerfile that
was never committed is refused rather than replaced. The file keeps its own line endings.
`release-and-pin` inherits all of it.

A project without Docker services is pinned exactly as before; the setup context, with its
single-publish-index rule, is only discovered when the refresh applies.

### 2.3 The Dockerfile a project builds

| | Before | Now |
|---|---|---|
| How the binary arrives | `ADD https://github.com/AetherBreaker/aeth-devkit/releases/download/container-v{N}/devkit-container-x86_64-unknown-linux-musl /app/devkit-container` + `chmod +x` | `uv sync --frozen --no-dev --no-install-project` installs it into `/app/.venv` from the lock |
| Build-time queries | `/app/devkit-container app-extra`, `/app/devkit-container readme` | `/app/.venv/bin/devkit-container app-extra`, `… readme`, after a first sync without extras |
| Final stage | copies the venv, `pyproject.toml` and the binary | copies the venv and `pyproject.toml`; the binary is inside the venv |
| Entrypoint | `["/app/devkit-container", "run"]` | `["/app/.venv/bin/devkit-container", "run"]` |
| Placeholders | `{python_dir}`, `{container_version}` | `{python_dir}` |
| Who owns the file | devkit's template, with the pin kept from the project's copy | devkit-owned outright: rendered from the locked package, refreshed by `docker-pin` |

The entrypoint's behaviour (`run`: check mounts, prepare persisted dirs, drop to nonroot,
exec the app; `app-extra`; `readme`) is unchanged: the crate moved verbatim.

### 2.4 Releasing

**Before:** `devkit release` on `aeth-devkit` published the wheel, and the same
`release: published` event ran `devkit-container.yml`, which computed the next `container-vN`
from the previous tag and released a new binary when the crate or `Cargo.lock` had changed.
**Now:** `devkit release` on `aeth-devkit` publishes only the wheel (and the extension
workflow runs as before). `devkit release` on `devkit-container` publishes that wheel through
the standard Rust release workflow; `1.0.0` is on SFTPyPI. The two release cadences are
independent; the coupling is the consumer's `uv.lock`.

### 2.5 What a consumer sees

- A Docker project's `pyproject.toml` carries `devkit-container>=<version>` in
  `[project].dependencies` and a `[tool.uv.sources]` entry for it; its `uv.lock` pins it.
- `poe setup-project` is what advances that pin (spec 4.0: the devkit packages are
  setup-project's to move; everything else, `aeth-devkit` included, is `poe lock`'s).
- `poe docker-pin` may produce two commits instead of one.
- The image installs the container from SFTPyPI at build time, so the build needs the index
  reachable; before, it needed GitHub reachable.
- Migration is a whole-file Dockerfile diff offered by `setup-project` (answer `replace`);
  the old `container-v1`/`container-v2` releases stay published, so unmigrated Dockerfiles
  keep building. No consumer has been migrated yet (TODO.md holds the per-project steps).

### 2.6 Smaller behavioural changes

- `templates::locate` finds devkit's own templates through the same `importlib.metadata`
  probe; behaviour is equivalent.
- `setup-project --check` on a Docker project whose lock pins another devkit now exits 1.
- `--dry-run` still runs no `uv` command; it notes a package that is not installed.

---

## Part 3: What did not change

- The container binary's subcommands and the `[tool.docker]` schema
  (`services`, `required_persisted_dirs`, and the legacy keys' warnings).
- The compose pin: resolution across publish indexes and tags, the behind-origin preflight,
  the dirty-file handling, the commit and push.
- The consent flow: `setup-project` still shows the Dockerfile diff and asks (terminal or VS
  Code, per hunk). Keeping a hunk there is still possible; see 5.1 for why it does not last.
- `devkit lock`, `devkit release`, the hooks, completion, the VS Code extension.
- `--check` exists as before; the spec removes it in step 4.

---

## Part 4: Where the result departs from the spec and the plan

- **No constraints file.** The spec and plan had `uv lock` run with `aeth-devkit==<running>`
  in a uv constraints file. No uv release has a `--constraints` flag on `uv lock` (0.11.1 and
  0.12.11 both reject it, and `uv lock` ignores `UV_CONSTRAINT`); the idea was an
  extrapolation from `uv pip compile -c` and `uv add -c` that was never checked against
  `uv lock --help`. The constraint rides on `--upgrade-package aeth-devkit==<running>`, which
  is the form uv documents for pinning a package during a lock and which the resolver treats
  as a hard constraint (an unmeetable one is "No solution found"). Verified against SFTPyPI.
- **A second gate.** The spec put the dependency under `if-docker`; that fires on Docker
  files alone (the `[tool.docker]` seed case) and would have added a runtime dependency to a
  project that had not set the switch. `if-docker-services` was added; `active()` and the
  template use the same condition.
- **A re-lock after the floor.** Not in the plan; found in review. Without it the lock uv had
  just written was out of date the moment the floor landed.
- **The venv lookup.** The plan's `PackageDirs::dir(root, import_name)` read the version from a
  `dist-info` directory beside the package; that layout does not hold for editable installs
  and would not hold for a distribution whose name differs from its import name. It became
  `Venv::installed`, answered by the interpreter.
- **The smoke test's scratch app** takes the wheel as a path-sourced dependency rather than a
  `pip install` (1.6).
- **Spec 4.0's "templates package first"** does not apply yet: that package arrives in step 4.
  The package step runs after the pyproject merge (which lists the dependency) and before
  the Docker step (which reads the venv).

---

## Part 5: Known consequences and open items

### 5.1 Dockerfile ownership

`docker/Dockerfile` is devkit-owned. `setup-project` still offers Keep and per-hunk decisions
on it because the reviewer is generic, and a hunk kept there is silently replaced by the next
`docker-pin`. Both pre-merge reviews named this the strongest remaining design objection;
the spec states it as a consequence and TODO.md carries the entry (a mechanism for
project-specific edits, or a `[tool.devkit]` opt-out). Unchanged by choice.

### 5.2 Left as designed, on record

- `[tool.setup-project].keep` on `project.dependencies` makes a Docker project's run fail: the
  merge cannot add the container and the package step requires it. The error names the cause.
- On a committing run the "lock pins another devkit" check reads HEAD's lock, so a lock the
  user moved but has not committed is refused, with the remedy in the message.
- A dry run cannot preview the floor or lock change; that needs `uv lock`.
- `advance` reaches uv through the Docker collaborators' runner; `pin` depends on `setup` for
  `render` and the context.
- Each run spawns the project's interpreter two or three times (about 45 ms each) to ask what
  is installed.

### 5.3 Still to do

- Consumer migration (`aeth_ext`, `IMAPReportCollector`, `ScheduledInvoiceProcessor`,
  `ScheduledReportAggregator`): per project, `poe lock`, `poe setup-project` (answer `replace`
  for the Dockerfile), `poe docker-pin`, push. Each needs a real terminal for the consent
  prompts. TODO.md has the full entry, including the `chown_paths`/`mkdirs` clean-up.
- Split steps 2 to 5 (templates, extension, hooks, completion) and step 4's removal of
  `--check`.
- A stash `setup-project output, first run` remains in the `devkit-container` checkout,
  superseded by the commit.
- Dependabot has a grouped npm PR open against the VS Code extension's lockfile (vite,
  vitest, esbuild), unrelated to this work.

---

## Appendix: the record

- PR #16, rebase-merged; `main` moved from `e5dc3f0` to `537f9ec`; `d69616d` is the version
  bump. `aeth-devkit 12.0.0`: tag `v12.0.0`, GitHub release with the manylinux and Windows
  wheels and the sdist, all three on SFTPyPI; the release workflow, CI on `main` and the
  extension workflow green; the local venv rebuilt at 12.0.0.
- `devkit-container 1.0.0`: `AetherBreaker/devkit-container`, tag `v1.0.0`, wheel on
  SFTPyPI, CI green.
- Review before merge: two independent extra-high-effort passes (in-session and on Opus),
  findings verified by reproduction against uv 0.12.11 where they concerned uv, and fixed in
  `1512139`; the plan's execution notes list every change.
