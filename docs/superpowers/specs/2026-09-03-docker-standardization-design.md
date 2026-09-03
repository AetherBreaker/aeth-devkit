# Docker standardization — design

Status: draft for review. Depends on
[the CI release workflow design](2026-09-03-ci-release-workflow-design.md): the container
binary is a release asset that workflow builds. The consent prompts here are
terminal-only and complete on their own;
[the VS Code extension design](2026-09-03-vscode-extension-design.md) layers an in-editor
flow on top of them later.

## Goal

`devkit setup-project` owns each project's Docker configuration the way it owns the rest
of the project config: a standard `Dockerfile`, a compose file that satisfies a shared
shape while keeping per-project values, and a single static entrypoint binary in place of
the shell script plus five Python helpers every sister project currently carries. Drift is
shown and replaced only with the user's explicit, per-file consent.

Surveyed 2026-09-03: aeth_ext, IMAPReportCollector, ScheduledInvoiceProcessor,
ScheduledReportAggregator. Dockerfiles are identical modulo CRLF and one blank line.
Entrypoints differ (aeth_ext runs a separate mkdirs pass; the others fold mkdir into the
chown loop). Helper scripts differ only by docstrings. Compose files share a common
service shape with per-project values.

## `[tool.docker]` schema

```toml
# setup-project: if-docker
[tool.docker]
  # Compose services setup-project validates against the standard. Empty = no Docker
  # handling at all. Side services (wireguard, ...) are simply not listed.
  services = []
  # Paths relative to /app the entrypoint guarantees exist, are backed by a bind mount
  # (the path itself or an ancestor), and are owned by nonroot. Sub-paths of a mount are
  # created on the mounted filesystem.
  required_persisted_dirs = ["persisted_data"]
```

- `services` is the **only** Docker switch. `ProjectContext.has_docker` becomes "the list
  is non-empty"; the Dockerfile/compose file-probe is deleted. It still gates the
  `.dockerignore` line merge and the `[tool.docker]` template merge.
- `required_persisted_dirs` replaces `chown_paths` and `mkdirs`. The template adds it when
  missing (merge semantics as today). **No migration code**: the old keys are left in
  place and an advisory names them (see Advisories). IMAPReportCollector's `mkdirs = [""]`
  is a data bug to fix by hand, listed in TODO.md.

## Container binary: crate `aeth-devkit-container`

A new workspace crate producing the binary `devkit-container`. No Python at build or run
time inside the image. Dependencies: `toml`, `anyhow`, `clap`, `nix` (already in the
lock) for the privilege drop. Subcommands, all reading `/app/pyproject.toml`
(overridable with `--pyproject` for tests):

- `app-extra` — prints `--extra app` when `[project.optional-dependencies].app` exists,
  else nothing. Replaces `detect_app_extra.py`.
- `readme` — prints `project.readme` or nothing. Replaces `get_readme.py`.
- `run` — the entrypoint (Unix only; on Windows it exits 1 with "unsupported platform").
  1. Must run as uid 0, else exit 1 (same rule as today).
  2. Launch command: the single `[project.scripts]` key with the `run-app-` prefix; zero
     or many is an error naming what was found (`get_launch_script.py`'s rule).
  3. Mount check, before touching the filesystem: for each entry, walk from
     `/app/<entry>` up to but excluding `/app`; some path on that walk must be a mount
     point per `/proc/self/mountinfo`. Otherwise print every unbacked entry and exit 1.
     This catches "container started with no volume" regardless of whether the directory
     already exists.
  4. For each entry: `mkdir -p` (a no-op on paths the mount provided), then recursive
     chown to 999:999 (files and subdirectories).
  5. `setgroups([])`, `setgid(999)`, `setuid(999)`, then `exec /app/.venv/bin/<script>`.
     gosu is no longer installed.

`get_mkdirs.py` has no successor: its distinction is gone with the merged key.

## Templates

`python/aeth_devkit/templates/docker/`:

- `Dockerfile.template` — the common content, LF, stray blank line removed. Changes from
  the current file: the builder stage `ADD`s
  `https://github.com/AetherBreaker/aeth-devkit/releases/download/v{devkit_version}/devkit-container-x86_64-unknown-linux-musl`
  to `/app/devkit-container` and `chmod +x`; the two `uv run --no-project python
  /app/scripts/...` invocations become `/app/devkit-container app-extra` / `readme`; the
  final stage copies the binary instead of `scripts/` and `entrypoint.sh`, drops the gosu
  layer, and sets `ENTRYPOINT ["/app/devkit-container", "run"]`. `{devkit_version}` is the
  version of the devkit that ran setup-project (`CARGO_PKG_VERSION`), so a devkit release
  drifts every project's Dockerfile by one line, surfaced through the normal prompt.
- `compose.template.yaml` — the fresh-file scaffold, one service block per listed
  service (see Compose scaffold).

`.dockerignore` handling is unchanged. `docker/entrypoint.sh` and `docker/scripts/` are
not templated; they are reported as **stray** (see Static files).

New placeholders: `{devkit_version}`; `{git_repo}` (origin URL normalised to
`https://github.com/<owner>/<repo>.git`, reusing docker-pin's normaliser);
`{git_tag}` (latest remote tag via `gh` as docker-pin resolves it, falling back to `v` +
the pyproject version when there are no tags or `gh` is unavailable, with a `note:`);
`{service}` (per block in the compose scaffold).

## setup-project flow

Runs inside the normal flow whenever `services` is non-empty. No `--docker` flag. New
flag `--replace-docker`: answers `replace all` up front.

### Static files: whole-file replace with consent

Applies to `docker/Dockerfile` and every other templated file under `docker/` except the
compose file.

- Missing: created from the template, reported as a normal change, no prompt.
- Present and equal after CRLF/LF normalisation: nothing (CRLF-only drift is not
  reported).
- Present and different: print a unified diff (project file vs rendered template; use the
  `similar` crate), then prompt `Replace docker/Dockerfile? [replace / replace all /
  anything else keeps it]:`. `replace` replaces this file; `replace all` replaces this and
  every remaining Docker file and compose edit without further prompts; anything else
  keeps the file and continues.
- Stray files the template no longer ships (`docker/entrypoint.sh`, `docker/scripts/`):
  never deleted; a single `note:` lists them as safe to delete.

The `Prompt` trait and `ScriptedPrompt` move from the release crate to core so setup can
use them.

### Compose: edit in place with consent

The compose file is found by docker-pin's discovery; created as `docker/compose.yaml`
when absent (no prompt, from the scaffold). For an existing file, every listed service
is checked against the matrix below. All intended edits to the file are shown as one
unified diff and confirmed with one prompt (same answers as above). Edits are
format-preserving line edits on top of `aeth_devkit_core::compose` (extended with nested
key lookup and insertion at the right indentation), never a YAML reserialise, so
comments and ordering survive.

A listed service absent from the file: prompt `Service "x" is not in docker/compose.yaml
(found: a, b). Add it? [add / anything else skips]:`; on `add`, a scaffold block is
appended under `services:` and included in the edit diff. Anything else skips that
service (a typo in pyproject should not grow the file).

### Compose rules

Rule kinds: **exact** (rewritten when different), **pattern** (checked; mismatch
rewritten), **presence** (inserted with the default when missing, never changed),
**at-least** (missing entries appended, existing entries untouched).

Per listed service:

| Key | Kind | Standard |
| --- | --- | --- |
| `container_name` | pattern | equals the service key |
| `build.context` | exact | `.` |
| `build.dockerfile` | exact | `docker/Dockerfile` |
| `build.args.GIT_REPO` | pattern | origin remote, normalised as docker-pin does |
| `build.args.GIT_TAG` | presence | `{git_tag}`; docker-pin owns the value afterwards |
| `restart` | presence | `no` |
| `volumes` | at-least | a bind entry with `target: /app/persisted_data`; default `source: /data/{package}_files` |
| `environment` | at-least, only when aeth_ext is a declared dependency | `ALERTS_EMAIL=info@sweetfiretobacco.com`, `ALERTS_EMAIL_PWD=${ALERTS_EMAIL_PWD:?}`, `ALERTS_RECIPIENTS=["jacob.ogden@sweetfiretobacco.com"]` |
| `networks` | presence | list form `- coolify`; an existing key of any shape is left alone |
| `healthcheck.test` | exact | the shared heartbeat `CMD-SHELL` command |
| `healthcheck.interval` / `timeout` / `retries` / `start_period` | exact | `30s` / `5s` / `3` / `15s` |

Top level: `networks.coolify.external` exact `true` (inserted with the `networks:` map
when missing). Keys the standard does not name (`expose`, `labels`, `stop_grace_period`,
extra environment entries, network aliases, other services) are never touched.

### Compose scaffold

For each listed service, the block above with every presence/at-least default filled in,
`container_name: {service}`, `GIT_REPO: {git_repo}`, `GIT_TAG: {git_tag}`, and the
ALERTS entries only when aeth_ext is a dependency. Followed by the top-level `networks`
map.

### Modes

- `--dry-run`: every diff and intended edit is printed, no prompts, nothing written.
- `--check`: same; Docker drift counts for exit 1.
- Non-interactive stdin (`std::io::IsTerminal`): diffs printed, every answer treated as
  "keep", one `note:` saying the Docker files were left alone because no terminal was
  available. `--replace-docker` overrides this.
- Docker files join the normal auto-commit like any other managed file; declined files
  are simply not in the change set.

### Advisories (`note:` lines)

- `services` empty but a Dockerfile or compose file exists: "Docker files found but
  `[tool.docker].services` is empty; list the app service(s) to manage them."
- `[tool.docker]` present with `chown_paths` or `mkdirs`: "fold these into
  `required_persisted_dirs` and delete them; the entrypoint no longer reads them." (This
  replaces today's "table but no Docker setup" advisory, which keys off the new switch.)
- Stray `docker/entrypoint.sh` / `docker/scripts/` as above.
- `{git_tag}` fallback used.

## Release workflow extension

The Rust release workflow template's build matrix gains, per platform, a
`cargo build --release -p aeth-devkit-container --target <target>` step and an artefact
upload. Linux uses `x86_64-unknown-linux-musl` (static; installed via
`rustup target add` in the job) for the container binary while the wheel keeps the gnu
target; Windows uses `x86_64-pc-windows-msvc`. Asset names:
`devkit-container-x86_64-unknown-linux-musl` and
`devkit-container-x86_64-pc-windows-msvc.exe`. The publish job uploads them alongside the
wheels. The Windows build exists for parity of the build pattern; nothing consumes it
today.

Because this template is devkit-managed and only devkit has Rust, the matrix step is
unconditional in the Rust variant; a future Rust sister project without a container crate
would need a `{container_crate}`-style gate, noted in TODO.md.

`docker-pin` is unchanged: a consumer's Dockerfile pin on a devkit asset is independent
of the consumer's own release completeness, and devkit's own release now waits for CI, so
the asset exists before `Released` is reported.

## Testing

- Container crate: unit tests for the launch-script rule, `app-extra`, `readme`, the
  mount-check walk against a synthetic mountinfo, and the chown/mkdir loop in a temp
  dir (privilege drop and exec are exercised only by an ignored integration test that
  needs root).
- Setup crate: template rendering with the new placeholders; static-file states
  (missing / equal / CRLF-only / different) with `ScriptedPrompt` covering `replace`,
  `replace all`, decline, `--replace-docker`, and non-tty; compose rule evaluation for
  each kind against fixtures derived from the four sister files, including the
  aeth_ext-gated environment entries, the missing-service prompt, and multi-service
  files with an unlisted sidecar; `--check` exit code with Docker drift; idempotency of a
  second run.
- Release crate: the matrix template renders the container step.

## Documentation

README `setup-project` feature reference: replace the "Not yet implemented `--docker`"
bullet with the Docker bullets (switch, static replace, compose rules, prompts,
`--replace-docker`), and document the container binary under a new `devkit-container`
heading. TODO.md: delete the Docker scaffolding item; add the IMAPReportCollector
`mkdirs` fix, the sister-project migration checklist (add `services`, run setup-project,
fold the old keys, delete stray scripts), and the `{container_crate}` gate note.

## Out of scope

Deleting stray files automatically, validating unlisted services, YAML reserialisation,
`--python-dir`, the vendored-gitignore refresh.
