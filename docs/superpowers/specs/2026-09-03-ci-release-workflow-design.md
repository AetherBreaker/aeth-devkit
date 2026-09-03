# CI release workflow — design

Status: draft for review. Prerequisite for
[the Docker standardization design](2026-09-03-docker-standardization-design.md).

## Goal

Move artefact building and publishing out of the machine that runs `devkit release` and
into a GitHub Actions workflow, for every project devkit manages. The local command keeps
the human-driven half (bump, commit, tag, push, create the release); CI does the
reproducible half (build, attach assets, publish to the index). This is the convention
both PyPA and maturin document, and it is what makes multi-platform artefacts (devkit's
own wheels and the container binary from the Docker design) possible without a
cross-compilation toolchain on any developer machine.

## Current state (verified 2026-09-03)

`devkit release` runs nine steps locally: snapshot, bump, `uv lock`, `uv build` into a
cleared `dist/`, commit, tag, `uv publish`, push, `gh release create` **with `dist/*`
attached**. Release v8.2.1 carries `aeth_devkit-8.2.1-py3-none-win_amd64.whl` and the
sdist. There is no "missing assets" bug; the limitation is that everything is built on one
platform.

## Division of labour

| Concern | Where | Why |
| --- | --- | --- |
| Version bump, `uv lock`, commit, annotated tag, push | local | needs the developer's judgement and git identity |
| `gh release create` (notes, title) | local | the release object is the CI trigger |
| `uv build` (wheel + sdist), platform matrix | CI | clean checkout of the tagged commit, every platform, no laptop toolchains |
| `gh release upload` of every artefact | CI | CI holds the only complete set |
| `uv publish` to the project's index | CI | credentials live in repository secrets, not on laptops |
| Waiting for CI and verifying completeness | local | so "release finished" still means "installable everywhere" |

The SFTPyPI publish URL (`pypi.sweetfiretobacco.com`) answers over HTTPS through
Cloudflare, so GitHub-hosted runners can reach it.

## `devkit release` changes

### Steps

The plan becomes:

1. snapshot `pyproject.toml`, `uv.lock`, `Cargo.toml`, `Cargo.lock` (no `dist/`)
2. `uv version --bump ...` ; `Cargo.toml` version ; `cargo update --workspace` (bump mode)
3. `uv lock` (bump mode)
4. quiet commit `Bump version to X` (bump mode)
5. `git tag -a vX`
6. one atomic `git push origin <branch> vX` (tag only in no-bump mode)
7. `gh release create vX --title vX` with notes or `--generate-notes`, **no files**
8. wait for the release workflow run and verify completeness (see below)

Removed: `snapshot::clear_dist`, the `dist/` half of the snapshot, step 4 `uv build`,
step 7 `uv publish` with its devpi-ownership probe, and the `Undo::DeleteDevpi` journal
entry pushed by that step. `DevpiClient` stays: pre-flight still probes and removes an
existing devpi version of the target, and the post-CI verification reads the index.

### Waiting for CI (step 8)

After the release exists, poll `gh run list --workflow release.yml --event release
--json databaseId,status,conclusion,headBranch` until a run for tag `vX` appears
(bounded by a start-up timeout, default 120 s), then `gh run watch <id> --exit-status`.
On success, run the same complete-release check `docker-pin` uses (the
`resolve_version` rule in `aeth-devkit-pin`, factored into core if needed): the version
must be present on every publish index and the release must exist. A missing index entry
after a green run is an error.

`--no-wait` skips step 8 and prints the run URL; the outcome is still `Released`.

Workflow failure or timeout is a release failure: the journal is unwound exactly as a
step-9 failure is today (delete the GitHub release, delete the remote tag under its lease,
force-push the branch back under its lease, soft-reset, restore files). Transient CI
outages are handled by re-running `devkit release` afterwards; leaving a tag without
artefacts is the state the complete-release check exists to reject.

### Pre-flight additions

- The release workflow file (`.github/workflows/release.yml`) must exist at `HEAD`. Its
  absence is a config error (exit 2) whose message says to run `devkit setup-project` and
  commit.
- `gh` must be able to read workflow runs; the existing `check_tools` probe is extended
  with `gh run list --limit 1`.
- **No publish index is no longer an error.** `config::resolve` returns a `PublishTarget`
  enum: `Index { name, publish_url, username, password }` or `Pypi`. In `Pypi` mode the
  existing-artefact probe checks `https://pypi.org/simple/<package>/` (the existing
  `IndexClient`, PEP 691) instead of devpi, offers no removal (PyPI files are immutable;
  an existing version is reported and the release aborts), and the post-CI completeness
  check reads the same URL. Credentials and the `--index` flag are meaningless in `Pypi`
  mode; `--index` naming an index without a `publish-url` stays an error.

### Unchanged

Args and positional parsing, dirty-tree and existing-artefact prompts, exit codes
(0 released / 1 aborted or rolled back / 2 config), `--dry-run` (prints the eight-step
plan), `release-and-pin` (it composes on `Outcome::Released`, which now implies CI has
finished, so the pin's preflights pass immediately), `rescind-release.sh`.

## Workflow templates

Two templates under `python/aeth_devkit/templates/github/workflows/`, chosen by
`ctx.has_rust`, both installed as `.github/workflows/release.yml`:

- `release.template.yml` — pure-Python projects. One job on `ubuntu-latest`.
- `release.rust.template.yml` — maturin projects. A build matrix plus a publish job.

Both share the same skeleton:

```yaml
name: Release
on:
  release:
    types: [published]
concurrency:
  group: release-${{ github.event.release.tag_name }}
  cancel-in-progress: false
permissions:
  contents: write
```

Every job checks out `github.event.release.tag_name`, installs uv (`astral-sh/setup-uv`),
and, before anything is uploaded, asserts that `uv version --short` equals the tag without
its `v` prefix — a tag pointing at the wrong commit fails before it can publish.

### Publish target

Where the wheels go is decided from `pyproject.toml` at template-render time:

- **A private index** — the sole `[[tool.uv.index]]` with a `publish-url`:
  `uv publish --index {publish_index} dist/*` with `UV_PUBLISH_USERNAME` /
  `UV_PUBLISH_PASSWORD` taken from repository secrets
  `UV_INDEX_{publish_index_key}_USERNAME` / `_PASSWORD`. The key is the index name
  upper-cased with `-` mapped to `_`, the same mapping `config::env_var_names` uses, so
  one naming rule covers laptops and CI.
- **No publish index** — PyPI via trusted publishing: `uv publish --trusted-publishing
  always dist/*`, with `id-token: write` added to the job's permissions. No secrets. The
  project must be registered as a trusted publisher on PyPI (repository, workflow file
  name `release.yml`); setup-project prints a `note:` with those values the first time it
  installs the file, in place of the secret-names note.

Two or more publish indexes remain a config error, as today.

### Pure-Python job

`uv build`, then `gh release upload "$TAG" dist/* --clobber`, then the publish step
above.

### Rust (maturin) jobs

- `build` matrix: `{ os: windows-latest, target: x86_64-pc-windows-msvc }` and
  `{ os: ubuntu-latest, target: x86_64-unknown-linux-gnu, manylinux: "2_17" }`.
  Steps: checkout, `dtolnay/rust-toolchain@stable` with the target,
  `PyO3/maturin-action` (`--release --strip --out dist`, `manylinux` on Linux only),
  upload `dist/*` as artifact `wheel-<target>`. The Docker design adds a second binary to
  this matrix; the job is written so an extra `cargo build` step and artefact slot in.
- `sdist` on `ubuntu-latest`: `uv build --sdist`, upload as artifact `sdist`.
- `publish` needs `[build, sdist]`: download all artifacts into `dist/`, the version
  assertion, `gh release upload` with `--clobber`, then the publish step from
  "Publish target".

Wheels are uploaded and published from one job so a partial publish can only happen after
every build succeeded.

### Placeholders

New setup-project placeholders: `{publish_index}`, the name of the sole
`[[tool.uv.index]]` with a `publish-url`, resolved the way the release crate's
`config::resolve` does; and `{publish_index_key}`, its secret-name form. The publish
step is selected at render time, so the two templates each carry both variants behind a
`# setup-project: if-publish-index` / `if-no-publish-index` block marker (the YAML
analogue of the TOML `if-dep` markers), and the rendered file contains only one.

### setup-project merge rule

The release workflow is devkit-owned: it is replaced whenever it differs from the
rendered template, reported as a normal change, and counts as drift for `--check`. This
differs from `claude.yml` (create-if-missing) because nothing in the release workflow is
project-specific beyond the placeholders. The one manual step is the credential setup:
the two repository secrets for a private index, or the PyPI trusted-publisher
registration; setup-project prints the matching `note:` the first time it installs the
file.

## Devkit-specific consequences

- Windows wheel, Linux wheel and sdist appear on every devkit release, built by CI.
  Consumers keep installing from SFTPyPI; the Linux wheel means a Linux checkout of any
  sister project can `uv sync` devkit as well.
- `[tool.maturin]` is unchanged; maturin-action reads it.
- TODO.md's "Linux wheel" item is closed by this design.

## Testing

Rust unit tests with `RecordingRunner` for: the eight-step plan text, the `gh run`
polling state machine (no run yet, run appears, success, failure, timeout), the
`--no-wait` path, the workflow-missing pre-flight, and the journal on CI failure.
Template rendering tests in the setup crate for both workflow variants, both publish
targets, and the `{publish_index}` placeholders; release-crate tests for `PublishTarget`
resolution and the PyPI-mode probe. No live GitHub or PyPI calls in tests.

## Documentation

README feature reference: rewrite the `devkit release` bullets (steps, waiting,
`--no-wait`, rollback on CI failure) and add the workflow to the `setup-project` list.
TODO.md: close the Linux-wheel item; note the Docker design as the next consumer.

## Out of scope

Trusted publishing against devpi (it has none), signing or attestations,
`rescind-release.sh` migration, changing the sister projects' release cadence.
