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
  - `docker-pin-latest.sh` already knows how to set `GIT_TAG`; the scaffold should leave
    `GIT_TAG` unset/placeholder and let `poe docker-pin-latest` fill it.
  - Tests: e2e case for a fresh project with `--docker`; idempotency on re-run.
- [ ] `if-docker` conditional marker for template tables (mirrors `if-dep`; drives the
      `[tool.docker]` item above).
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
- [ ] `docker-pin-latest.sh`
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
