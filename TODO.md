# TODO

Item tracking for `aeth-devkit`. Keep entries short; link the spec when a
design exists. Check items off in place; delete them once released.

## setup-project

- [ ] **Docker scaffolding flags** — add opt-in flags to `devkit setup-project` (exposed through
      `poe setup-project`) that copy the now-generic Docker files from `aeth_ext` into a
      project as templates:
  - `--docker` → `docker/Dockerfile`, `docker/compose.yaml`, `docker/entrypoint.sh`,
    `docker/scripts/*`, `.dockerignore` (line-union like today).
  - Placeholders to introduce: `{git_repo}` (from `git remote get-url origin`), `{package}`,
    `{project_name}` (compose service/container name), `{git_tag}` default.
  - `[tool.docker]` in `pyproject.template.toml` (`chown_paths`, `mkdirs`) should only be
    merged when `--docker` is given or a Docker setup already exists (`ctx.has_docker`) —
    today it is unconditional.
  - Merge semantics: create-if-missing only; never overwrite an existing Dockerfile /
    compose file (report "exists, skipped"). A `--docker-force` flag can overwrite.
  - Still aeth_ext-specific as of 2026-08-26 (must become placeholders or be dropped):
    `compose.yaml` service/`container_name` = `central-log-server`, `GIT_REPO` URL,
    `GIT_TAG: v8.0.4` pin, the heartbeat health-check command, the `coolify` service.
    `Dockerfile`, `entrypoint.sh` and `docker/scripts/get_{chown_paths,mkdirs,launch_script,readme}.py`
    / `detect_app_extra.py` already read everything from `pyproject.toml` (`[tool.docker]`,
    `[project.scripts]`) and look fully generic.
  - `devkit docker-pin` already knows how to set `GIT_TAG`; the scaffold should leave
    `GIT_TAG` unset/placeholder and let `poe docker-pin` fill it.
  - Tests: e2e case for a fresh project with `--docker`; idempotency on re-run.
- [x] `if-docker` conditional marker for template tables (mirrors `if-dep`; drives the
      `[tool.docker]` item above). Done on `feat/agent-config`.
- [ ] Vendored gitignore refresh: a `poe` task or script that re-fetches
      `Python.gitignore` / `Rust.gitignore` from GitHub into the templates.
- [ ] Consider a `--python-dir` override for projects whose Python package is neither in
      `src/` nor `python/`.

## Release / packaging

- [ ] Release 7.0.0 (`aeth-devkit`), then migrate downstream projects per README.
- [ ] Linux wheel (`maturin build --target x86_64-unknown-linux-musl --zig`) if `aeth-devkit`
      is ever installed outside Windows dev machines.
- [ ] Fix system-level `init.defaultBranch = master` in
      `C:\Program Files\Git\etc\gitconfig` (needs an elevated shell; user config already
      overrides it to `main`).

## Script migration to Rust

Planned order (each command is its own crate under `crates/`; the `devkit` binary
dispatches):

- [x] `lock.sh` → `devkit lock` (7.0.0)
- [x] `docker-pin-latest.sh` → `devkit docker-pin`. Agreed requirements:
  - [x] **Rename** — command and poe task become `docker-pin` (it pins any version, not just
        latest); `release-and-pin` keeps its name.
  - [x] **Crate layout** — thin `crates/aeth-devkit-pin` (clap `Args` + orchestration, `run_real`
        dispatched from `devkit`); reusable pieces (compose discovery, version resolution,
        GitHub tags client, pin edit) live in `aeth-devkit-core` so a future in-process
        `release-and-pin` can call the raw functions directly (no subprocess).
  - [x] **Index config from pyproject** — kill the hardcoded SFTPyPI URL. Query *every*
        `[[tool.uv.index]]` with a `publish-url` via its simple `url` (existing `IndexClient`).
        Explicit version must exist on ALL of them (missing from any = failed release, name
        the index); latest = `latest_stable` over the *intersection* of version sets.
  - [x] **GitHub via `gh` CLI** through the `Runner` trait (`gh api ... --paginate`): picks up
        auth, no rate-limit issues, no 100-tag cap, testable with `RecordingRunner`.
  - [x] **Block-aware compose edit** — format-preserving line edit scoped to service blocks
        that build *this* project: `PACKAGE_NAME` (normalized) == `project.name`, or
        `GIT_REPO` == origin remote (normalized https/ssh/.git/case). All matching blocks
        move together, each change reported; no match = error listing what was found.
        Mode (git/pypi) follows from which match kind hits, not key order in the file.
  - [x] **Preflights before any edit** — resolve → validate → behind-check → edit → commit → push:
    - Complete-release check (mode-independent): remote tag `v<ver>` AND GitHub release AND
      present on every publish index. Index check skipped only when no publish index is
      configured; GitHub checks skipped only when origin is not a GitHub remote. Applies to
      resolved-latest as well as explicit versions; failure lists exactly what is missing.
    - Behind-origin check (`fetch` + `behind_count`) when pushing; fail before touching files.
  - [x] **Commit only the compose path** (`commit_paths`, not bare `git commit` which sweeps
        the user's staged files). Message: `chore: pin <name> to <version>`.
  - [x] **Dirty compose file** — apply the pin to the HEAD blob and commit via
        `commit_files_on_head` (user's index/worktree untouched), then write the 3-way merge
        (worktree over base + pin) back to the worktree so the user's uncommitted changes ride
        on top. Merge conflict = abort before committing anything.
  - [x] **Flags** — `--version/-V`, `--dry-run`, `--no-commit` (edit only, implies no push),
        `--no-push`, `--compose-file <path>`.
  - [x] **Compose discovery** — anchored at the git repo root; walk shallowest-first with
        Docker name precedence (`compose.yaml` > `compose.yml` > `docker-compose.yaml` >
        `docker-compose.yml`) within each directory; first hit wins (single compose file
        assumed; extend later if ever needed). Skip known-irrelevant dirs (`.git`, `.venv`,
        `.cache`, `__pycache__`, `.mypy_cache`, `.pytest_cache`, `.ruff_cache`,
        `node_modules`, root Cargo `target/`). Always print the chosen file.
  - [x] **Version handling via `pep440_rs` end to end** — explicit input accepted with or
        without `v` prefix, parsed on entry (error if unparseable); all membership checks
        (indexes, git tags) use parsed equality, not string equality; latest via
        `latest_stable` (no hand-rolled regex filters). Written form: `GIT_TAG` = `v` + the
        actual tag spelling found on the remote; `PACKAGE_VERSION` = PEP 440 normalized.
  - [x] **Poe wiring + script removal** — the poe task becomes `docker-pin` running
        `devkit docker-pin` with declared poe args mirroring the flags (lock-task
        style); delete `docker-pin-latest.sh`. README: move the command out of the
        shell-script table and add its per-command Rust feature-reference bullets.
  - [x] **Migrate `release-and-pin` in the same pass** (both constituents are then Rust):
        a thin `ReleaseAndPin` subcommand in the `devkit` binary crate composing
        `aeth_devkit_release` + pin lib in-process (no subprocesses). Release lib entry
        point grows a structured outcome (released version + released/aborted) so the
        composition knows what to pin; `devkit release` behavior unchanged. `--dry-run`
        runs release's dry-run then *skips* the pin step ("dry run: skipping docker pin" —
        an unpublished version cannot pass pin's preflights). All other args forward to
        release verbatim; the pin step runs with the released version and full preflights
        (free post-release verification). Poe task: `devkit release-and-pin $POE_EXTRA_ARGS`.
- [x] `release.sh` → `devkit release` (spec: `docs/specs/2026-08-26-devkit-release-design.md`)
- [ ] `rescind-release.sh`

## Housekeeping

- [ ] `uv run ruff format python` — `python/aeth_devkit/__init__.py` has pre-existing
      formatting drift now visible with the inlined ruff config.
- [ ] IMAPReportCollector: `tool.coverage.run.source_pkgs` still lists
      `scheduled_invoice_processor` (copy-paste leftover); remove after `setup-project`
      unions in the correct name.
- [ ] Rename remaining `master` default branches if desired: `ScheduledReportAggregator`,
      `apscheduler-stubs`.
