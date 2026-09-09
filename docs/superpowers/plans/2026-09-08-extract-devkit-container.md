# Extract devkit-container Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the container entrypoint into its own repository published as a wheel on SFTPyPI, and teach `setup-project` and `docker-pin` to manage it from the project's venv.

**Architecture:** The crate, its Dockerfile template and its smoke test leave `aeth-devkit` by `git filter-repo` into `devkit-container`, a maturin `bindings = "bin"` project whose wheel also carries `devkit_container/template.Dockerfile` as package data. In `aeth-devkit`, a new `packages` module in `setup` implements spec section 4.0 (add when missing, lock under `aeth-devkit==<running>`, `{latest}` floors, `uv sync --frozen`), `static_files` renders the Dockerfile from the installed package instead of the templates directory, and `pin` refreshes a drifted committed Dockerfile before pinning. The old crate, its `container-vN` workflow and the `ADD` pin machinery are deleted.

**Tech Stack:** Rust 2024 (clap, toml_edit, anyhow, nix), maturin, uv (`lock --upgrade-package --constraints`, `sync --frozen`), `gh`, `git filter-repo` via `uvx`, Docker for the smoke test.

**Spec:** `docs/superpowers/specs/2026-09-08-devkit-split-design.md`, sections 2, 3, 4.0, 4.1, 4.5, 5 and 7 (step 1). Read it first; every task below argues from it.

## Global Constraints

- New repo: `AetherBreaker/devkit-container`, public, default branch `main`, cloned to `D:\SFT Software Projects\devkit-container`. Created by this plan with `gh`, never by hand.
- Package name `devkit-container`, import name `devkit_container`, binary `devkit-container`, initial version `1.0.0`, index block copied verbatim from `aeth-devkit`'s `pyproject.toml` (`SFTPyPI`, `explicit = true`).
- The wheel's Dockerfile template is a real file at `python/devkit_container/template.Dockerfile`, never embedded in the binary.
- Every dependency resolution `setup-project` performs runs with the constraint `aeth-devkit==<the running devkit's version>` (`env!("CARGO_PKG_VERSION")` of the `setup` crate, which shares the workspace version). Two outcomes only: an unsatisfiable explicit floor stops the run with "run `devkit lock`, then rerun setup-project"; a `{latest}` specifier is throttled to the newest release uv can choose under the constraint, with a warning when the index holds a newer one.
- `uv sync --frozen` installs the result; `uv.lock` joins `setup-project`'s committable set; a `uv.lock` that pins a different `aeth-devkit` than the running one stops the run with "run `uv sync`".
- `docker/Dockerfile` is devkit-owned: `docker-pin` replaces drift with no prompt, in its own commit ahead of the pin commit. `setup-project` keeps showing the diff and asking.
- Publish secrets `UV_INDEX_SFTPYPI_USERNAME` and `UV_INDEX_SFTPYPI_PASSWORD` are piped from `aeth-devkit/.env` into the new repo with `gh secret set`; never print them. The Claude Code OAuth token is not set by this plan.
- `--dry-run` writes nothing and runs no `uv` command; `--check` is not touched here (step 4 removes it).
- Project rules (AGENTS.md): run Python tooling under `uv run`; tests carry no docstrings; comments carry reasoning, densely; do not extract helpers under five lines; Conventional Commits with the `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` trailer; on a feature branch run only targeted tests while iterating and the full suite once at the end.
- `aeth-devkit` work happens on branch `feat/extract-devkit-container` in the main checkout (worktrees are broken on this machine); the PR is opened at the end and merged by hand.
- Old `container-v1` and `container-v2` GitHub releases on `aeth-devkit` are left in place.

---

## Part A: the `devkit-container` repository

### Task 1: Extract the crate with its history into a standalone repository

**Files:**
- Create (new repo): `Cargo.toml`, `Cargo.lock`, `rustfmt.toml`, `.gitignore`
- Moved by filter-repo: `src/*.rs`, `tests/*.rs`, `python/devkit_container/template.Dockerfile`
- Delete (new repo): `.github/workflows/devkit-container.yml`

**Interfaces:**
- Produces: a git repository at `D:\SFT Software Projects\devkit-container` on branch `main` whose history is the crate's, building and passing `cargo test` on its own. No remote yet.

- [ ] **Step 1: Clone and filter**

Run, in Git Bash (paths with forward slashes):

```bash
cd "/d/SFT Software Projects"
git clone --no-local aeth-devkit devkit-container
cd devkit-container
uvx --from git-filter-repo git-filter-repo --force \
  --path crates/aeth-devkit-container \
  --path python/aeth_devkit/templates/docker/template.Dockerfile \
  --path .github/workflows/devkit-container.yml \
  --path-rename crates/aeth-devkit-container/: \
  --path-rename python/aeth_devkit/templates/docker/template.Dockerfile:python/devkit_container/template.Dockerfile
git log --oneline | wc -l
git log --oneline --follow -- src/run.rs | head -5
ls
```

Expected: several dozen commits; `src/run.rs` has history; the tree holds `Cargo.toml`, `src/`, `tests/`, `python/devkit_container/template.Dockerfile`, `.github/workflows/devkit-container.yml`; `git remote -v` prints nothing (filter-repo drops `origin`). If filter-repo fails in a way that cannot be fixed in a few minutes, fall back to `git init` in a fresh directory, copy the same three paths into the same places, and make one commit `chore: extract devkit-container from aeth-devkit`; note the fallback in the task's commit message.

- [ ] **Step 2: Make the crate standalone**

Replace `Cargo.toml` with:

```toml
[package]
  name    = "devkit-container"
  version = "1.0.0"
  edition = "2024"
  publish = false

[[bin]]
  name = "devkit-container"
  path = "src/main.rs"

[dependencies]
  anyhow    = "1.0.104"
  clap      = { version = "4", features = ["derive"] }
  toml_edit = "0.25.13"

# `nix` has no Windows build at all, so it must not even be resolved there.
[target.'cfg(unix)'.dependencies]
  nix = { version = "0.31.3", features = ["user"] }

[dev-dependencies]
  serde_json = { version = "1.0.151", features = ["preserve_order"] }
  tempfile   = "3.27.0"

[profile.release]
  strip       = true
  incremental = true
```

Copy `rustfmt.toml` from `aeth-devkit` unchanged. Write `.gitignore` with:

```
/target
.venv/
.cache/
dist/
```

Delete `.github/workflows/devkit-container.yml` (its tag-stream logic is superseded by the wheel release; the history stays).

- [ ] **Step 3: Build and run the non-Docker tests**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test
```

Expected: `entrypoint` and unit tests pass; `docker_smoke` is `#[ignore]` and skipped. `Cargo.lock` is generated. If `tests/entrypoint.rs` locates the binary through `env!("CARGO_BIN_EXE_devkit-container")`, nothing changes; if it used the old crate name anywhere, fix the name.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: make devkit-container a standalone crate

Extracted from aeth-devkit with git filter-repo: the crate, its Dockerfile
template and the container release workflow, with their history. The crate
is renamed devkit-container, pins its own dependency versions, and drops the
container-vN workflow, which the wheel release in the next commits replaces.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 2: Package it as a maturin wheel with the Dockerfile template as package data

**Files:**
- Create: `pyproject.toml`, `python/devkit_container/__init__.py`, `README.md`
- Modify: `python/devkit_container/template.Dockerfile` (whole file)

**Interfaces:**
- Produces: a wheel `devkit_container-1.0.0-py3-none-<platform>.whl` containing `devkit_container/__init__.py`, `devkit_container/template.Dockerfile` and the `devkit-container` binary as a script. The template's contract for `setup-project`: placeholders `{python_dir}` only; the entrypoint path `/app/.venv/bin/devkit-container`.

- [ ] **Step 1: Write `pyproject.toml`**

```toml
[project]
  name            = "devkit-container"
  version         = "1.0.0"
  description     = "Image-side helper for devkit-managed Docker projects: build-time pyproject queries and the container entrypoint"
  readme          = "README.md"
  requires-python = ">=3.14"
  dependencies    = []

[dependency-groups]
  dev = [
    "aeth-devkit>=11.0.1",
    "maturin>=1.7,<2",
  ]

[build-system]
  requires      = ["maturin>=1.7,<2"]
  build-backend = "maturin"

[tool.maturin]
  bindings      = "bin"
  python-source = "python"
  module-name   = "devkit_container"

[tool.uv.sources]
  aeth-devkit = [{ index = "SFTPyPI" }]

[[tool.uv.index]]
  name        = "SFTPyPI"
  url         = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"
  publish-url = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/"
  explicit    = true
```

Write `python/devkit_container/__init__.py`:

```python
"""Package data for devkit-container.

`template.Dockerfile` beside this file is the Dockerfile `devkit setup-project` renders for a
project; it ships in the same wheel as the entrypoint binary so the two never drift apart.
"""
```

- [ ] **Step 2: Rewrite the Dockerfile template**

Replace `python/devkit_container/template.Dockerfile` with:

```dockerfile
# syntax=docker/dockerfile:1

# ---- Builder stage ----
FROM ghcr.io/astral-sh/uv:python3.14-bookworm-slim AS builder

WORKDIR /app

ARG GIT_TAG
ARG GIT_REPO

# Enable bytecode compilation
ENV UV_COMPILE_BYTECODE=1

# Copy from the cache instead of linking since it's a mounted volume
ENV UV_LINK_MODE=copy

# Install git (required for uv to fetch git-based dependencies)
RUN apt-get update && apt-get install -y --no-install-recommends git \
  && rm -rf /var/lib/apt/lists/*

# Clone only the dependency manifest files first so the dep install layer
# can be cached independently of source code changes.
RUN git clone --depth 1 --branch "${GIT_TAG}" "${GIT_REPO}" /tmp/repo \
  && mv /tmp/repo/pyproject.toml /tmp/repo/uv.lock /app/

# Install the dependencies without the project itself, from the frozen lockfile. This
# brings in devkit-container (a runtime dependency of every devkit-managed Docker project),
# whose binary answers the build-time pyproject questions below and is the entrypoint in
# the final stage. The version is the one uv.lock names; setup-project rendered this file
# from that same version.
RUN --mount=type=cache,target=/root/.cache/uv \
  uv sync --frozen --no-dev --no-install-project

# A second sync adds the `app` extra when pyproject declares one; the layer is a no-op
# otherwise. Cached as long as pyproject.toml/uv.lock don't change.
RUN --mount=type=cache,target=/root/.cache/uv \
  extras=$(/app/.venv/bin/devkit-container app-extra) \
  && uv sync --frozen --no-dev --no-install-project $extras

# Now bring in the source tree, then the readme the wheel build reads, at the same
# relative path (`project.readme` may point into a subdirectory). The tree moves first so
# a readme inside it comes along and never pre-creates `/app/{python_dir}`, which would
# make `mv` nest the tree. Only a missing readme is tolerated; a failing helper is not.
RUN mv /tmp/repo/{python_dir} /app/{python_dir} \
  && readme_file=$(/app/.venv/bin/devkit-container readme) \
  && if [ -n "${readme_file}" ] && [ -f "/tmp/repo/${readme_file}" ]; then \
       mkdir -p "/app/$(dirname "${readme_file}")" \
       && mv "/tmp/repo/${readme_file}" "/app/${readme_file}"; \
     fi \
  && rm -rf /tmp/repo

# Install the project itself as a non-editable wheel so the source tree is not
# required at runtime.
RUN --mount=type=cache,target=/root/.cache/uv \
  extras=$(/app/.venv/bin/devkit-container app-extra) \
  && uv sync --frozen --no-dev --no-editable $extras

# ---- Final stage ----
FROM ghcr.io/astral-sh/uv:python3.14-bookworm-slim

# Setup a non-root user. /app stays root-owned: the code is read-only to the app, which
# writes only to its mounted persisted dirs (or temp dirs).
RUN groupadd --system --gid 999 nonroot \
  && useradd --system --gid 999 --uid 999 --create-home nonroot

WORKDIR /app

# Prevents Python from writing pyc files.
ENV PYTHONDONTWRITEBYTECODE=1
# Keeps Python from buffering stdout and stderr to avoid situations where
# the application crashes without emitting any logs due to buffering.
ENV PYTHONUNBUFFERED=1
# Enable Python optimizations (removes assert statements and sets __debug__ to False)
ENV PYTHONOPTIMIZE=1

# Copy the virtual environment from the builder stage; it carries the entrypoint binary.
COPY --from=builder /app/.venv /app/.venv

# The entrypoint reads the project's pyproject.toml.
COPY --from=builder /app/pyproject.toml /app/pyproject.toml

# Place executables in the environment at the front of the path
ENV PATH="/app/.venv/bin:$PATH"

# The entrypoint checks every required_persisted_dir is bind-mounted, chowns them to
# nonroot, drops privileges, and execs the project's run-app-* script.
ENTRYPOINT ["/app/.venv/bin/devkit-container", "run"]
```

- [ ] **Step 3: Write `README.md`**

```markdown
# devkit-container

The image-side helper for devkit-managed Docker projects. One small static-free binary,
`devkit-container`, installed into the project's venv like any dependency, plus the Dockerfile
template `devkit setup-project` renders for the project. Both ship in one wheel, so the
Dockerfile a project builds with always matches the binary its image installs.

## How a project uses it

`setup-project` adds `devkit-container` to `[project].dependencies` of every project with
`[tool.docker].services`, locks it to the newest release the installed devkit accepts, and
renders `docker/Dockerfile` from `devkit_container/template.Dockerfile` in the venv. The image
installs the package with `uv sync --frozen` and uses `/app/.venv/bin/devkit-container` both for
the build-time queries and as the entrypoint. `devkit docker-pin` refreshes a Dockerfile that
drifted from the locked version before it pins. No Python runs in the image outside the app
itself.

## Subcommands

- `app-extra` prints `--extra app` when `[project.optional-dependencies].app` exists.
- `readme` prints `project.readme` (string or `{ file = … }` form).
- `run` is the entrypoint (Linux only). Must be root. Resolves the single `run-app-*` script
  in `[project.scripts]`; checks every `[tool.docker].required_persisted_dirs` entry is backed
  by a bind mount (the path or an ancestor below `/app`, per `/proc/self/mountinfo`) and refuses
  to start otherwise; `mkdir -p` + recursive chown to `999:999`; `setgroups([])`, `setgid`,
  `setuid`; `exec /app/.venv/bin/<script>`. `/app` itself stays root-owned: the app writes only
  to its mounted dirs or temp dirs. Entries that are empty, `.`, `..`, absolute or escape `/app`
  are errors; a table still carrying `chown_paths`/`mkdirs` is refused with the migration hint.
  Flags `--pyproject`, `--app-root`, `--mountinfo` exist for tests.

## `[tool.docker]` schema

| Key | Meaning |
|---|---|
| `services` | compose services `setup-project` manages; the only Docker switch |
| `required_persisted_dirs` | paths relative to `/app` the entrypoint guarantees exist, are bind-mounted and are owned by nonroot |
| `silence_unlisted_services_warning` | quiets `setup-project`'s warning when Docker files exist but `services` is empty |

`chown_paths` and `mkdirs` are legacy keys the entrypoint refuses.

## Tests

`cargo test` covers the parsers and, on Linux as root, the entrypoint. The smoke test
(`cargo test --test docker_smoke -- --ignored --nocapture`; CI runs it) builds the wheel for the
image platform, builds the template Dockerfile around a scratch app with that wheel installed
into the venv, starts it on a named volume and checks the app's own report: PID 1, uid/gid 999,
`/app` read-only, the persisted dirs created, owned and writable, the venv, the `app` extra and
the wheel install; a run without the volume or as non-root is refused first.

## Releasing

`uv run devkit release <bump>`. The standard release workflow builds Windows and manylinux
wheels and publishes them to SFTPyPI. The Windows wheel is required, not optional: projects
install this package on Windows dev machines too, where only the query subcommands run.
```

- [ ] **Step 4: Sync, build the wheel, inspect it**

```bash
uv sync
uv run devkit-container --version
uv run maturin build --release --out dist
uv run python -c "import zipfile,glob; w=glob.glob('dist/*.whl')[0]; print(w); print('\n'.join(n for n in zipfile.ZipFile(w).namelist()))"
```

Expected: `devkit-container 1.0.0`; the listing contains `devkit_container/__init__.py`, `devkit_container/template.Dockerfile`, and `devkit_container-1.0.0.data/scripts/devkit-container.exe` (or without `.exe` on Linux). If the template is missing, maturin did not pick up the package data: confirm `python-source = "python"` and that `__init__.py` exists beside the template.

- [ ] **Step 5: Commit**

```bash
git add pyproject.toml uv.lock python/devkit_container/__init__.py python/devkit_container/template.Dockerfile README.md
git commit -m "feat: package devkit-container as a wheel carrying its Dockerfile template

maturin bindings=bin with python-source: the wheel installs the binary as a script and
ships devkit_container/template.Dockerfile as package data, so the Dockerfile a project
renders and the entrypoint its image installs are one artefact. The template installs the
package from uv.lock in the builder stage (a first sync without extras, then the app-extra
query, then the sync with extras) and uses the venv's binary as the entrypoint; the ADD of
a static musl build and its container-vN pin are gone.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 3: Adapt the smoke test to the wheel, and add CI

**Files:**
- Modify: `tests/docker_smoke.rs`
- Create: `.github/workflows/ci.yml`

**Interfaces:**
- Consumes: the template from Task 2 (its first `RUN --mount=type=cache` block is the dependency sync).
- Produces: a smoke test that builds a `linux_x86_64` wheel of this checkout, installs it into the image's venv right after the dependency sync, and asserts the same report as before.

- [ ] **Step 1: Replace `workspace()` and `build_entrypoint()`**

In `tests/docker_smoke.rs`, replace the `workspace` function with:

```rust
fn root() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).canonicalize().unwrap()
}
```

and replace `build_entrypoint` with:

```rust
/// The wheel the image installs, built from this checkout for the image's platform. The
/// binary is a static musl build so the same wheel serves a glibc image, and the platform
/// tag is the generic `linux` one so uv accepts it there; the release workflow builds the
/// manylinux wheel instead, which is the only difference between this image and a real one.
fn build_wheel(root: &Path, out: &Path) -> PathBuf {
  let mut cmd = Command::new("uv");
  cmd
    .args(["run", "maturin", "build", "--release", "--target", MUSL, "--compatibility", "linux", "--out"])
    .arg(out)
    .current_dir(root);
  // Windows has no `cc` for the musl target; rustc's bundled lld links it self-contained.
  if cfg!(windows) {
    cmd.env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", "rust-lld");
  }
  ok(&mut cmd);
  let wheel = std::fs::read_dir(out)
    .unwrap()
    .flatten()
    .map(|e| e.path())
    .find(|p| p.extension().is_some_and(|x| x == "whl"))
    .expect("maturin wrote a wheel");
  let name = wheel.file_name().unwrap().to_string_lossy().into_owned();
  assert!(name.ends_with("linux_x86_64.whl"), "the image needs the generic linux tag, got {name}");
  wheel
}
```

If maturin rejects `--compatibility linux` for the musl target, use `--compatibility off`; the assertion on the file name is what matters.

- [ ] **Step 2: Patch the Dockerfile for the local wheel**

Replace the `dockerfile` function with:

```rust
/// The shipped template with `{python_dir}` filled, the bare repo copied in for the clone to
/// read, and the local wheel installed into the venv right after the dependency sync, which
/// is where a real build gets the package from uv.lock. Nothing else changes.
fn dockerfile(root: &Path, wheel_name: &str) -> String {
  let template = std::fs::read_to_string(root.join("python/devkit_container/template.Dockerfile")).unwrap();
  let (mut copied, mut installed) = (0, 0);
  let mut out = String::new();
  // `RUN` blocks continue over `\`-terminated lines; the wheel goes in after the first one.
  let mut in_first_run = false;
  for line in template.lines() {
    if line.starts_with("RUN git clone ") {
      out.push_str("COPY scratch.git /tmp/scratch.git\n");
      copied += 1;
    }
    out.push_str(&line.replace("{python_dir}", "src"));
    out.push('\n');
    if installed == 0 && line.starts_with("RUN --mount=type=cache") {
      in_first_run = true;
    }
    if in_first_run && !line.trim_end().ends_with('\\') {
      out.push_str(&format!(
        "COPY {wheel_name} /tmp/wheels/{wheel_name}\nRUN uv pip install --python /app/.venv/bin/python /tmp/wheels/{wheel_name}\n"
      ));
      in_first_run = false;
      installed += 1;
    }
  }
  assert_eq!((copied, installed), (1, 1), "the template changed shape; update this test");
  out
}
```

- [ ] **Step 3: Rewire the test body**

In `the_image_starts_the_app_through_the_entrypoint_with_a_working_environment`, replace the lines from `let ws = workspace();` through `std::fs::write(context.join("Dockerfile"), dockerfile(&ws)).unwrap();` with:

```rust
  let root = root();
  let work = tempfile::tempdir().unwrap();
  let id = format!("{}-{}", std::process::id(), std::time::UNIX_EPOCH.elapsed().unwrap().as_secs());
  let guard = Cleanup {
    image: format!("{IMAGE_TAG_PREFIX}:{id}"),
    volume: format!("{IMAGE_TAG_PREFIX}-{id}"),
  };

  eprintln!("building the wheel for {MUSL}");
  let wheel = build_wheel(&root, &work.path().join("wheels"));
  let wheel_name = wheel.file_name().unwrap().to_string_lossy().into_owned();
  eprintln!("scratch project + bare repo");
  scratch_repo(work.path());
  let context = work.path().join("context");
  std::fs::copy(&wheel, context.join(&wheel_name)).unwrap();
  std::fs::write(context.join("Dockerfile"), dockerfile(&root, &wheel_name)).unwrap();
```

Then change the two build-time query assertions to use the venv path: `"--entrypoint", "/app/.venv/bin/devkit-container"` in both `docker run` calls. Everything else in the test stays.

- [ ] **Step 4: Run it if Docker is available; otherwise compile it**

```bash
docker version --format '{{.Server.Os}}' && cargo test --test docker_smoke -- --ignored --nocapture
```

Expected: the report's `failures` is `[]`, `pid_1` is 1, `installed_as_wheel` contains `/site-packages/`. Without Docker locally, run `cargo test --test docker_smoke --no-run` and rely on CI in Step 6.

- [ ] **Step 5: Write `.github/workflows/ci.yml`**

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

jobs:
  rust:
    name: Rust (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [windows-latest, ubuntu-latest]
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          components: rustfmt, clippy

      - uses: Swatinem/rust-cache@v2

      - name: Format
        run: cargo fmt --all --check

      - name: Clippy
        run: cargo clippy --all-targets -- -D warnings

      - name: Test
        run: cargo test

  wheel:
    name: Wheel build (${{ matrix.os }})
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [windows-latest, ubuntu-latest]
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"

      # The package data must ride in the wheel: a project renders its Dockerfile from it.
      - name: Build the wheel and check the template ships in it
        shell: bash
        run: |
          uv sync
          uv run maturin build --release --out dist
          uv run python -c "import zipfile,glob; names=zipfile.ZipFile(glob.glob('dist/*.whl')[0]).namelist(); assert 'devkit_container/template.Dockerfile' in names, names"
          uv run maturin develop
          uv run devkit-container --version

  # The only place the entrypoint runs for real: the shipped template built around a
  # scratch app with this checkout's wheel installed (tests/docker_smoke.rs).
  container-smoke:
    name: Container smoke (docker)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-unknown-linux-musl

      - uses: Swatinem/rust-cache@v2

      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"

      - name: Build the image from the template and start the app through the entrypoint
        run: |
          uv sync
          cargo test --test docker_smoke -- --ignored --nocapture
```

- [ ] **Step 6: Commit**

```bash
git add tests/docker_smoke.rs .github/workflows/ci.yml
git commit -m "test: build the smoke image from this checkout's wheel, and add CI

The smoke test builds a generic-linux wheel of the checkout (a musl binary, so it runs in
the glibc image) and installs it into the venv right after the dependency sync, the point
where a real build installs the package from uv.lock; the ADD-for-COPY swap is gone. CI
runs fmt, clippy and tests on both platforms, checks the template ships in the wheel, and
runs the smoke test on Linux.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 4: Create the GitHub repository, make it devkit-managed, and cut the first release

**Files:**
- Created by `setup-project` in the new repo: `AGENTS.md`, `.claude/*`, `.vscode/*`, `.gitignore`, `.gitattributes`, `.github/workflows/release.yml`, `.github/workflows/claude.yml`, `.mcp.json`, merged `pyproject.toml`

**Interfaces:**
- Produces: `https://github.com/AetherBreaker/devkit-container` with `main` pushed, secrets set, release `v1.0.0` published, and `devkit-container==1.0.0` on SFTPyPI. Part B depends on that package existing on the index.

- [ ] **Step 1: Create the repo and push**

```bash
cd "/d/SFT Software Projects/devkit-container"
gh repo create AetherBreaker/devkit-container --public --source=. --remote=origin --push \
  --description "Image-side helper for devkit-managed Docker projects: build-time pyproject queries, the container entrypoint, and the Dockerfile template"
git branch -u origin/main main
gh repo view AetherBreaker/devkit-container --json name,visibility,defaultBranchRef --jq '{name,visibility,default:.defaultBranchRef.name}'
```

Expected: `{"name":"devkit-container","visibility":"PUBLIC","default":"main"}`.

- [ ] **Step 2: Pipe the publish secrets from `aeth-devkit/.env`**

```bash
for k in UV_INDEX_SFTPYPI_USERNAME UV_INDEX_SFTPYPI_PASSWORD; do
  grep "^$k=" "/d/SFT Software Projects/aeth-devkit/.env" | cut -d= -f2- | tr -d '"' | tr -d '\r' | gh secret set "$k" --repo AetherBreaker/devkit-container
done
gh secret list --repo AetherBreaker/devkit-container
```

Expected: both names listed. Never echo the values; if a command fails, rerun it, do not debug by printing.

- [ ] **Step 3: Run setup-project on the repo**

The Bash tool's stdin is a terminal here (verified: `sys.stdin.isatty()` is `True`), so the headless refusal does not trigger; there is no Docker setup in this repo, so nothing prompts.

```bash
uv run devkit setup-project --no-vscode
git log --oneline -3
git status --short
```

Expected: a commit "Standardize project configuration with devkit" containing `AGENTS.md`, `.claude/settings.json`, `.vscode/*`, `.gitignore`, `.gitattributes`, `.github/workflows/release.yml`, `.github/workflows/claude.yml`, `.mcp.json`, `pyproject.toml`; a note about the SFTPyPI secrets (already set); `.claude/settings.local.json` present and ignored. If it refuses as headless, run the same command from a real terminal and continue.

- [ ] **Step 4: Verify the merged pyproject and the venv still build the wheel**

```bash
grep -n "source_pkgs\|python_dir\|src \+=" pyproject.toml | head
uv sync
uv run maturin develop
uv run devkit-container --version
git add uv.lock
git commit -m "chore: lock the dev group setup-project added

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git push
```

Expected: `source_pkgs = ["devkit_container"]`, `src = ["./python", ...]`; version prints; the tree is clean after the lock commit (`devkit release` refuses an uncommitted pyproject and would otherwise see a dirty lock); push succeeds.

- [ ] **Step 5: Release 1.0.0**

`devkit release` with no bump releases the committed version. `--force` skips the confirmation prompts, so it runs unattended. It waits for the workflow (several minutes).

```bash
uv run devkit release --force
```

Run with a 600000 ms timeout, or in the background and poll `gh run list --repo AetherBreaker/devkit-container --workflow release.yml`. Expected: "workflow succeeded" and a final line confirming `devkit-container==1.0.0` is on `SFTPyPI`. Then:

```bash
gh release view v1.0.0 --repo AetherBreaker/devkit-container --json assets --jq '.assets[].name'
uv pip install --dry-run --index https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple devkit-container==1.0.0
```

Expected: two wheels and an sdist attached; the dry-run install resolves 1.0.0.

- [ ] **Step 6: Record the workspace doc in aeth-devkit**

In `aeth-devkit`, on branch `feat/extract-devkit-container` (create it now: `git switch -c feat/extract-devkit-container`), write `WORKSPACE.md`:

```markdown
# Mirroring the devkit workspace on another machine

Every repository lives beside the others under one folder; the paths below assume
`D:\SFT Software Projects`. Each is devkit-managed: `uv sync` installs the tooling and
`poe setup-project` renders the standard configuration.

## Clone

```bash
cd "/d/SFT Software Projects"
gh repo clone AetherBreaker/aeth-devkit
gh repo clone AetherBreaker/devkit-container
```

## Publishing credentials

The release workflows read the SFTPyPI credentials from repository secrets, but `uv publish`
and the local index queries read them from the environment. Copy `aeth-devkit/.env` (never
committed) into every repository that publishes a wheel:

```bash
for r in devkit-container; do cp aeth-devkit/.env "$r/.env"; done
```

## Bring each repository up

```bash
for r in aeth-devkit devkit-container; do
  (cd "$r" && uv sync && uv run poe setup-project)
done
```

`setup-project` installs the VS Code extension, the Claude Code hook lines and the shell
completion for this machine as part of that run.
```

Commit it in `aeth-devkit`:

```bash
git add WORKSPACE.md
git commit -m "docs: add WORKSPACE.md for mirroring the repo set on another machine

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

## Part B: `aeth-devkit`

All Part B work is on branch `feat/extract-devkit-container` in `D:\SFT Software Projects\aeth-devkit`. Run only the named tests while iterating; the full suite runs once in Task 10.

### Task 5: Package locating and lock reading (`setup::packages`)

**Files:**
- Create: `crates/aeth-devkit-setup/src/packages.rs`
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (add `pub mod packages;`), `crates/aeth-devkit-setup/src/templates.rs:158-181` (generalise `from_python`)

**Interfaces:**
- Produces:
  - `pub struct DevkitPackage { pub name: &'static str, pub import_name: &'static str }` and `pub const CONTAINER: DevkitPackage`.
  - `pub fn active(ctx: &ProjectContext) -> Vec<&'static DevkitPackage>`: `[&CONTAINER]` when `ctx.has_docker`, else empty.
  - `pub trait PackageDirs { fn dir(&self, import_name: &str) -> Option<PathBuf>; }`, `pub struct SystemPackageDirs;`, `pub struct StubPackageDirs(pub HashMap<String, PathBuf>)`.
  - `pub fn locked_version(lock: &str, name: &str) -> Option<String>` (any `[[package]]`), `pub fn locked_registry_version(lock: &str, name: &str) -> Option<String>` (only entries whose `source` has `registry`).
  - `pub fn installed_version(package_dir: &Path) -> Option<String>` from the sibling `<import_name>-<version>.dist-info`.
  - `templates::installed_package_dir(import_name: &str) -> Option<PathBuf>`.

- [ ] **Step 1: Generalise `from_python`**

In `templates.rs`, replace `from_python` with a public function that takes the import name, and make `locate` call it:

```rust
/// Where the Python interpreter that lives alongside this binary (the venv's `Scripts/` or
/// `bin/`) has `import_name` installed: the package directory, or `None` when it is not
/// importable there.
pub fn installed_package_dir(import_name: &str) -> Option<PathBuf> {
  let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
  let candidates = [exe_dir.join("python.exe"), exe_dir.join("python"), PathBuf::from("python")];
  let code = format!("import {import_name}, os; print(os.path.dirname({import_name}.__file__))");
  for py in candidates {
    // A candidate that cannot be spawned (e.g. `python.exe` on Unix) must not end the search.
    let Ok(out) = Command::new(&py).args(["-c", &code]).output() else {
      continue;
    };
    if out.status.success() {
      let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
      if p.is_dir() {
        return Some(p);
      }
    }
  }
  None
}
```

In `locate`, replace `if let Some(p) = from_python() { return Ok(p); }` with:

```rust
  if let Some(p) = installed_package_dir("aeth_devkit") {
    let templates = p.join("templates");
    if templates.is_dir() {
      return Ok(templates);
    }
  }
```

- [ ] **Step 2: Write the failing tests for `packages`**

Create `crates/aeth-devkit-setup/src/packages.rs` with the tests first (the module body follows in Step 4):

```rust
#[cfg(test)]
mod tests {
  use super::*;

  const LOCK: &str = r#"version = 1
requires-python = ">=3.14"

[[package]]
name = "aeth-devkit"
version = "11.0.1"
source = { registry = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple" }

[[package]]
name = "demo-app"
version = "1.2.3"
source = { editable = "." }

[[package]]
name = "devkit-container"
version = "1.4.0"
source = { registry = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple" }
"#;

  #[test]
  fn locked_versions_are_read_by_normalised_name() {
    assert_eq!(locked_version(LOCK, "devkit_container").as_deref(), Some("1.4.0"));
    assert_eq!(locked_version(LOCK, "Demo-App").as_deref(), Some("1.2.3"));
    assert_eq!(locked_version(LOCK, "missing"), None);
    assert_eq!(locked_version("not toml [", "x"), None);
  }

  #[test]
  fn registry_versions_skip_the_editable_root() {
    assert_eq!(locked_registry_version(LOCK, "aeth-devkit").as_deref(), Some("11.0.1"));
    assert_eq!(locked_registry_version(LOCK, "demo-app"), None, "the project itself is not a pin");
  }

  #[test]
  fn installed_version_comes_from_the_dist_info_beside_the_package() {
    let site = tempfile::tempdir().unwrap();
    let pkg = site.path().join("devkit_container");
    std::fs::create_dir_all(&pkg).unwrap();
    assert_eq!(installed_version(&pkg), None);
    std::fs::create_dir(site.path().join("devkit_container-1.4.0.dist-info")).unwrap();
    std::fs::create_dir(site.path().join("devkit_other-9.9.9.dist-info")).unwrap();
    assert_eq!(installed_version(&pkg).as_deref(), Some("1.4.0"));
  }

  #[test]
  fn the_container_is_active_only_for_docker_projects() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"p\"\n[tool.docker]\nservices = [\"p\"]\n").unwrap();
    let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
    assert_eq!(active(&ctx).iter().map(|p| p.name).collect::<Vec<_>>(), vec!["devkit-container"]);
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"p\"\n").unwrap();
    let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
    assert!(active(&ctx).is_empty());
    // The container repo itself has no reason to depend on its own wheel.
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"devkit_container\"\n[tool.docker]\nservices = [\"x\"]\n").unwrap();
    let ctx = crate::context::ProjectContext::discover(dir.path()).unwrap();
    assert!(active(&ctx).is_empty(), "a project never carries itself");
  }

  #[test]
  fn stub_dirs_answer_from_the_map() {
    let mut map = std::collections::HashMap::new();
    map.insert("devkit_container".to_string(), PathBuf::from("/site/devkit_container"));
    let dirs = StubPackageDirs(map);
    assert_eq!(dirs.dir("devkit_container"), Some(PathBuf::from("/site/devkit_container")));
    assert_eq!(dirs.dir("other"), None);
  }
}
```

- [ ] **Step 3: Run the tests to see them fail**

Add `pub mod packages;` to `lib.rs` (alphabetically, after `pub mod md_block;`). Run: `cargo test -p aeth-devkit-setup packages::`
Expected: compile errors for the missing items.

- [ ] **Step 4: Write the module**

Above the tests in `packages.rs`:

```rust
//! The devkit packages `setup-project` keeps current in a project (spec section 4.0): which
//! they are, where the venv keeps them, and what the lock says about them. The advancing
//! itself lives in [`advance`], added in a later task.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

use aeth_devkit_core::pyproject::normalize_dist_name;

use crate::context::ProjectContext;

/// A package devkit owns and `setup-project` installs and advances. `name` is the
/// distribution name (index, pyproject); `import_name` is the directory in site-packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DevkitPackage {
  pub name: &'static str,
  pub import_name: &'static str,
}

/// The image-side helper: a runtime dependency of every project with Docker services, whose
/// wheel also carries the Dockerfile template `setup-project` renders.
pub const CONTAINER: DevkitPackage = DevkitPackage {
  name: "devkit-container",
  import_name: "devkit_container",
};

/// The devkit packages this project should carry. Only the container so far; the templates,
/// hooks and completion packages join in later split steps. A project never carries itself,
/// so each satellite repo can be devkit-managed without depending on its own name.
pub fn active(ctx: &ProjectContext) -> Vec<&'static DevkitPackage> {
  let own = normalize_dist_name(&ctx.name);
  let mut out = Vec::new();
  if ctx.has_docker {
    out.push(&CONTAINER);
  }
  out.retain(|p| p.name != own);
  out
}

/// Where the venv keeps an installed package, by import name. A trait so tests can point at
/// a fixture instead of a real site-packages.
pub trait PackageDirs {
  fn dir(&self, import_name: &str) -> Option<PathBuf>;
}

/// Asks the interpreter next to this binary (see `templates::installed_package_dir`).
pub struct SystemPackageDirs;

impl PackageDirs for SystemPackageDirs {
  fn dir(&self, import_name: &str) -> Option<PathBuf> {
    crate::templates::installed_package_dir(import_name)
  }
}

/// Canned answers by import name; for tests.
#[derive(Default)]
pub struct StubPackageDirs(pub HashMap<String, PathBuf>);

impl PackageDirs for StubPackageDirs {
  fn dir(&self, import_name: &str) -> Option<PathBuf> {
    self.0.get(import_name).cloned()
  }
}

fn locked_entry<'a>(doc: &'a DocumentMut, name: &str) -> Option<&'a toml_edit::Table> {
  let want = normalize_dist_name(name);
  doc
    .get("package")?
    .as_array_of_tables()?
    .iter()
    .find(|t| t.get("name").and_then(Item::as_str).is_some_and(|n| normalize_dist_name(n) == want))
}

/// The version `uv.lock` holds for `name`, whatever its source. `None` for an unparsable
/// lock, so a missing or half-written file reads as "not locked" rather than an error.
pub fn locked_version(lock: &str, name: &str) -> Option<String> {
  let doc: DocumentMut = lock.parse().ok()?;
  locked_entry(&doc, name)?.get("version")?.as_str().map(str::to_string)
}

/// Like [`locked_version`], but only for a package that comes from an index: the project
/// itself appears in its own lock as an editable entry and is not a pin.
pub fn locked_registry_version(lock: &str, name: &str) -> Option<String> {
  let doc: DocumentMut = lock.parse().ok()?;
  let entry = locked_entry(&doc, name)?;
  entry.get("source")?.as_table_like()?.get("registry")?;
  entry.get("version")?.as_str().map(str::to_string)
}

/// The installed version of the package at `package_dir`, read from the
/// `<import_name>-<version>.dist-info` directory beside it. No interpreter call: the
/// directory name is the metadata.
pub fn installed_version(package_dir: &Path) -> Option<String> {
  let import_name = package_dir.file_name()?.to_string_lossy().into_owned();
  let prefix = format!("{import_name}-");
  std::fs::read_dir(package_dir.parent()?).ok()?.flatten().find_map(|e| {
    let file = e.file_name().to_string_lossy().into_owned();
    let stem = file.strip_suffix(".dist-info")?;
    stem.strip_prefix(&prefix).map(str::to_string)
  })
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p aeth-devkit-setup packages:: templates::`
Expected: PASS (the existing `template_names` test still passes).

- [ ] **Step 6: Commit**

```bash
git add crates/aeth-devkit-setup/src/packages.rs crates/aeth-devkit-setup/src/lib.rs crates/aeth-devkit-setup/src/templates.rs
git commit -m "feat(setup): name the devkit packages and read their locked and installed versions

The first piece of spec 4.0: which packages setup-project owns in a project (the container,
for Docker projects), a PackageDirs seam over the venv's site-packages, and readers for
the uv.lock entry and the dist-info version. from_python() becomes
installed_package_dir(name) so the same lookup serves aeth_devkit and devkit_container.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 6: Render the Dockerfile from the installed package

**Files:**
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (new top-level `Deps`, `run`, `run_with`), `crates/aeth-devkit-setup/src/docker/mod.rs:33-40,178-188` (`apply` signature), `crates/aeth-devkit-setup/src/docker/static_files.rs` (whole rendering path), `crates/aeth-devkit-setup/src/cli.rs:120-134`, `crates/aeth-devkit-setup/tests/docker.rs`, `crates/aeth-devkit-setup/tests/apply.rs`
- Create: `crates/aeth-devkit-setup/tests/fixtures/docker/template.Dockerfile`

**Interfaces:**
- Produces:
  - `pub struct aeth_devkit_setup::Deps<'a> { pub docker: docker::Deps<'a>, pub index: &'a dyn IndexClient, pub packages: &'a dyn PackageDirs }`; `run_with(ctx, templates_dir, dry_run, deps: &Deps)`.
  - `pub fn static_files::render(ctx: &ProjectContext, packages: &dyn PackageDirs) -> Result<Option<String>>` (Task 9 reuses it).
  - `static_files::apply(ctx, packages, consent, changes)`; `docker::apply(ctx, templates_dir, deps: &crate::Deps, changes)`.
- Consumes: `packages::{PackageDirs, CONTAINER}` from Task 5.

- [ ] **Step 1: Add the fixture template**

Copy the new template from Task 2 (`devkit-container/python/devkit_container/template.Dockerfile`) to `crates/aeth-devkit-setup/tests/fixtures/docker/template.Dockerfile`. It is a stand-in for what the venv provides; setup's tests exercise substitution, diffing and consent, not the Dockerfile's content.

- [ ] **Step 2: Rewrite the test harness in `tests/docker.rs`**

Replace the `run` helper with:

```rust
fn package_dirs() -> aeth_devkit_setup::packages::StubPackageDirs {
  let mut map = std::collections::HashMap::new();
  map.insert("devkit_container".to_string(), fixtures());
  aeth_devkit_setup::packages::StubPackageDirs(map)
}

fn run(root: &Path, mode: Mode, answers: &[&str], dry_run: bool) -> (Changes, ScriptedPrompt, RecordingRunner) {
  let prompt = ScriptedPrompt::new(answers);
  let runner = RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\nv1.0.0\n");
  let index = aeth_devkit_core::index::StubIndexClient { versions: vec![] };
  let dirs = package_dirs();
  let changes = {
    let deps = aeth_devkit_setup::Deps {
      docker: Deps {
        runner: &runner,
        prompt: &prompt,
        reviewer: None,
        mode,
      },
      index: &index,
      packages: &dirs,
    };
    let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
    aeth_devkit_setup::run_with(&ctx, &templates(), dry_run, &deps).unwrap()
  };
  (changes, prompt, runner)
}
```

(`fixtures()` already points at `tests/fixtures/docker`, which is where the template now sits beside the other fixtures.) Then:

- In `fresh_project_gets_dockerfile_and_compose_then_is_idempotent`: replace the two `container-v3` assertions with `assert!(df.contains("ENTRYPOINT [\"/app/.venv/bin/devkit-container\", \"run\"]"), "{df}");` and `assert!(!df.contains("{python_dir}"), "{df}");`; change the `gh` call count assertion to `1` with the comment "one lookup for GIT_TAG".
- Delete `an_existing_container_pin_is_kept_and_a_missing_one_is_filled`, `without_devkit_tags_the_pin_is_provisional_and_noted_only_when_written` and `a_failed_tag_lookup_leaves_the_dockerfile_alone_as_a_problem` entirely; the behaviours they covered no longer exist.
- Add:

```rust
#[test]
fn without_the_container_package_the_dockerfile_is_skipped_with_a_note() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  let prompt = ScriptedPrompt::new(&[]);
  let runner = RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\n");
  let index = aeth_devkit_core::index::StubIndexClient { versions: vec![] };
  let dirs = aeth_devkit_setup::packages::StubPackageDirs::default();
  let deps = aeth_devkit_setup::Deps {
    docker: Deps { runner: &runner, prompt: &prompt, reviewer: None, mode: Mode::DryRun },
    index: &index,
    packages: &dirs,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
  let changes = aeth_devkit_setup::run_with(&ctx, &templates(), true, &deps).unwrap();
  assert!(!root.join("docker/Dockerfile").exists());
  assert!(
    changes.notes.iter().any(|n| n.contains("docker/Dockerfile") && n.contains("devkit-container")),
    "{:?}",
    changes.notes
  );
}
```

Apply the same `Deps` wrapping to the `run` helper in `tests/apply.rs` (a `StubIndexClient` with no versions and a `StubPackageDirs` pointing at the same fixture directory).

- [ ] **Step 3: Run to see the failures**

Run: `cargo test -p aeth-devkit-setup --test docker`
Expected: compile errors on `aeth_devkit_setup::Deps` and `StubPackageDirs`.

- [ ] **Step 4: Add the top-level `Deps` and thread it through**

In `lib.rs`, after the `use` lines:

```rust
/// Everything `run_with` needs from outside: the Docker collaborators, the index client the
/// package step asks for newer releases, and where the venv keeps installed packages.
pub struct Deps<'a> {
  pub docker: docker::Deps<'a>,
  pub index: &'a dyn aeth_devkit_core::index::IndexClient,
  pub packages: &'a dyn packages::PackageDirs,
}
```

Change `run` to build one with `HttpIndexClient::default()` and `SystemPackageDirs`, and `run_with` to take `deps: &Deps` (its Docker calls use `&deps.docker`; step 8b becomes `docker::apply(ctx, templates_dir, deps, &mut changes)?`). In `docker::apply`, take `deps: &crate::Deps`, build `Consent` from `deps.docker`, and call `static_files::apply(ctx, deps.packages, &consent, changes)?` and `compose(ctx, templates_dir, deps.docker.runner, &consent, changes)?`. In `cli.rs`, build:

```rust
    let index = aeth_devkit_core::index::HttpIndexClient::with_timeout(std::time::Duration::from_secs(30));
    let deps = crate::Deps {
      docker: crate::docker::Deps {
        runner: &runner,
        prompt: &aeth_devkit_core::prompt::StdinPrompt,
        reviewer: reviewer.as_ref().map(|r| r as &dyn crate::vscode::protocol::Reviewer),
        mode: match (dry_run, args.replace_docker, tty) {
          (true, _, _) => crate::docker::Mode::DryRun,
          (false, true, _) => crate::docker::Mode::ReplaceAll,
          (false, false, true) => crate::docker::Mode::Ask,
          (false, false, false) => crate::docker::Mode::KeepAll,
        },
      },
      index: &index,
      packages: &crate::packages::SystemPackageDirs,
    };
```

- [ ] **Step 5: Rewrite the rendering in `static_files.rs`**

Replace the module doc's second sentence and everything from `pub const DEVKIT_REPO` through `newest_container_version` with:

```rust
/// The template's file name inside the installed `devkit_container` package.
pub const TEMPLATE_FILE: &str = "template.Dockerfile";

/// The Dockerfile as the installed devkit-container renders it for this project, or `None`
/// when the package is not in the venv. The version rendered is the version the image will
/// install, because both come from the same locked package.
pub fn render(ctx: &ProjectContext, packages: &dyn PackageDirs) -> Result<Option<String>> {
  let Some(dir) = packages.dir(crate::packages::CONTAINER.import_name) else {
    return Ok(None);
  };
  let path = dir.join(TEMPLATE_FILE);
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
  Ok(Some(templates::substitute(&text, ctx, templates::Escape::None)))
}
```

(add `use anyhow::Context as _;` and `use crate::packages::PackageDirs;`; drop the `github` and `Runner` imports). Change `apply` to `pub fn apply(ctx: &ProjectContext, packages: &dyn PackageDirs, consent: &Consent, changes: &mut Changes) -> Result<()>` and replace its body up to the `let Some(original) = original else` line with:

```rust
  for target in TARGETS {
    let rel = format!("docker/{target}");
    let path = ctx.root.join("docker").join(target);
    let original = crate::read_optional(&path)?;
    // On a plain run the package step has already installed the package, so `None` here
    // means a dry run on a project that has not adopted it yet.
    let Some(rendered) = render(ctx, packages)? else {
      changes.notes.push(format!(
        "{rel} was not rendered: devkit-container is not installed in this venv yet; a plain run adds it and renders on the next run."
      ));
      if let Some(original) = &original {
        changes.record_optional(&path, Some(original), original, vec![])?;
      }
      continue;
    };
```

The rest of the loop (unchanged-check, CRLF handling, diff, consent, record) stays as it is. Delete the tests `the_pin_is_read_from_a_live_download_url_only`, `the_newest_container_tag_is_numeric_and_a_lookup_failure_is_an_error` and `an_unrecognised_pin_is_named_rather_than_swapped_silently`. Add:

```rust
  #[test]
  fn render_substitutes_python_dir_from_the_installed_package() {
    let site = tempfile::tempdir().unwrap();
    let pkg = site.path().join("devkit_container");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join(TEMPLATE_FILE), "RUN mv /tmp/repo/{python_dir} /app/{python_dir}\n").unwrap();
    let mut map = std::collections::HashMap::new();
    map.insert("devkit_container".to_string(), pkg);
    let dirs = crate::packages::StubPackageDirs(map);
    let ctx = ProjectContext {
      root: std::path::PathBuf::from("/p"),
      package: "proj".into(),
      dependencies: Default::default(),
      has_docker: true,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec!["proj".into()],
      docker_legacy_keys: vec![],
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "python".into(),
      has_rust: true,
      publish_index: None,
    };
    assert_eq!(render(&ctx, &dirs).unwrap().unwrap(), "RUN mv /tmp/repo/python /app/python\n");
    assert_eq!(render(&ctx, &crate::packages::StubPackageDirs::default()).unwrap(), None);
  }
```

- [ ] **Step 6: Run the setup tests**

Run: `cargo test -p aeth-devkit-setup`
Expected: PASS. Fix any remaining assertion in `tests/apply.rs` that mentions `container-v`.

- [ ] **Step 7: Commit**

```bash
git add crates/aeth-devkit-setup
git commit -m "feat(setup): render docker/Dockerfile from the installed devkit-container package

The template comes from devkit_container/template.Dockerfile in the project's venv, so
the Dockerfile a project renders is the one its locked entrypoint version ships; the
consent flow around it is unchanged. The container-vN pin machinery
({container_version}, pinned_container_version, newest_container_version, DEVKIT_REPO,
the gh tag lookup) is deleted. run_with now takes a setup-level Deps carrying the index
client and the PackageDirs seam beside the Docker collaborators.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 7: `{latest}` in the pyproject template and merge

**Files:**
- Modify: `crates/aeth-devkit-setup/src/toml_merge.rs:123-170,279-305`, `python/aeth_devkit/templates/pyproject.template.toml:42-51`
- Modify: `crates/aeth-devkit-setup/src/packages.rs` (add `latest_requested`)

**Interfaces:**
- Produces:
  - `pub const toml_merge::LATEST: &str = "{latest}"`; a template requirement `name>={latest}` is merged as: keep the project's own specifier when the package is listed, otherwise add the bare name.
  - `pub fn packages::latest_requested(template: &str) -> Vec<String>`: normalised names of every requirement in the rendered template that carries `{latest}`.
  - The template adds `[project].dependencies = ["devkit-container>={latest}"]` and `[tool.uv.sources].devkit-container = [{ index = "SFTPyPI" }]`, both under `if-docker`.

- [ ] **Step 1: Failing tests in `toml_merge.rs`**

Add to the existing `mod tests`:

```rust
  #[test]
  fn a_latest_specifier_keeps_the_project_floor_and_adds_a_bare_name() {
    let tpl = "[project]\n  dependencies = [\"devkit-container>={latest}\", \"requests>=2\"]\n";
    let mut log = vec![];
    let out = merge_pyproject("[project]\n  name = \"p\"\n  dependencies = [\"devkit-container>=1.2.0\"]\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("\"devkit-container>=1.2.0\""), "kept: {out}");
    assert!(!out.contains("{latest}"), "{out}");
    assert!(out.contains("\"requests>=2\""));
    let out = merge_pyproject("[project]\n  name = \"p\"\n  dependencies = []\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("\"devkit-container\""), "bare name: {out}");
    assert!(!out.contains("{latest}"), "{out}");
    let out = merge_pyproject("[project]\n  name = \"p\"\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("\"devkit-container\"") && !out.contains("{latest}"), "no array yet: {out}");
  }
```

And in `packages.rs` tests:

```rust
  #[test]
  fn latest_requested_lists_the_placeholder_requirements() {
    let tpl = "[project]\n  dependencies = [\"devkit-container>={latest}\", \"requests>=2\"]\n[dependency-groups]\n  dev = [\"Devkit_Templates>={latest}\"]\n";
    assert_eq!(latest_requested(tpl), vec!["devkit-container", "devkit-templates"]);
    assert!(latest_requested("[tool.x]\n").is_empty());
  }
```

Run: `cargo test -p aeth-devkit-setup toml_merge:: packages::` — expected: the new tests fail (`{latest}` leaks / missing function).

- [ ] **Step 2: Implement in `toml_merge.rs`**

Add near the top:

```rust
/// In a template specifier, "the newest release this devkit can use". The merge only makes
/// sure the package is listed; `packages::advance` writes the real floor once uv has chosen
/// the version under the running-devkit constraint (spec 4.0), so the placeholder never
/// reaches a project file.
pub const LATEST: &str = "{latest}";
```

In `union_dependencies`, right after `let name = dependency_name(spec);`:

```rust
    if spec.contains(LATEST) {
      if pos.is_none() {
        push_like_last(existing, Value::from(name.clone()));
        added.push(name);
      }
      continue;
    }
```

(move the `let pos = …` line above this block). In `merge_value`, replace the `None =>` arm with:

```rust
      None => {
        if tval.is_array() && (path.starts_with("dependency-groups.") || path == "project.dependencies") {
          // A fresh array built through the same union, so a `{latest}` entry lands as a
          // bare name here too instead of the literal placeholder.
          let mut arr = Array::new();
          let added = union_dependencies(&mut arr, tval.as_array().unwrap());
          target.insert_formatted(tkey, Item::Value(Value::Array(arr)));
          self.log.push(format!("added {path}: {}", added.join(", ")));
        } else {
          // Carry the template key's decor (indentation) so the new line matches its neighbours.
          target.insert_formatted(tkey, Item::Value(tval.clone()));
          self.log.push(format!("added {path}"));
        }
      }
```

In `packages.rs` add:

```rust
/// The requirements a rendered pyproject template marks `{latest}`, by normalised name, so
/// `advance` knows whose floor to write after locking.
pub fn latest_requested(template: &str) -> Vec<String> {
  let Ok(doc) = template.parse::<DocumentMut>() else {
    return Vec::new();
  };
  let mut arrays: Vec<&toml_edit::Array> = Vec::new();
  if let Some(a) = doc.get("project").and_then(|p| p.get("dependencies")).and_then(Item::as_array) {
    arrays.push(a);
  }
  if let Some(groups) = doc.get("dependency-groups").and_then(Item::as_table_like) {
    arrays.extend(groups.iter().filter_map(|(_, v)| v.as_array()));
  }
  arrays
    .iter()
    .flat_map(|a| a.iter())
    .filter_map(|v| v.as_str())
    .filter(|s| s.contains(crate::toml_merge::LATEST))
    .map(crate::context::dependency_name)
    .collect()
}
```

- [ ] **Step 3: Update the template**

In `python/aeth_devkit/templates/pyproject.template.toml`, directly above the existing `# setup-project: if-docker` / `[tool.docker]` block, insert:

```toml
# setup-project: if-docker
[project]
  # The image-side helper (build-time queries and the entrypoint). `{latest}` is the newest
  # release the installed devkit accepts; setup-project locks it and renders docker/Dockerfile
  # from the template in that same wheel.
  dependencies = ["devkit-container>={latest}"]

# setup-project: if-docker
[tool.uv.sources]
  devkit-container = [{ index = "SFTPyPI" }]

```

Also change the comment in `[tool.docker]` from `Side services (wireguard, ...) are simply not listed.` to `Side services are simply not listed.`

- [ ] **Step 4: Run the tests**

Run: `cargo test -p aeth-devkit-setup toml_merge:: packages:: --test apply --test docker`
Expected: PASS. Check one rendered fixture by hand: the Docker project in `tests/docker.rs` now has `"devkit-container"` in `[project].dependencies` and a `[tool.uv.sources]` table; the non-Docker project in `tests/apply.rs` has neither.

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-setup/src/toml_merge.rs crates/aeth-devkit-setup/src/packages.rs python/aeth_devkit/templates/pyproject.template.toml
git commit -m "feat(setup): {latest} specifiers, and devkit-container as a Docker project dependency

A template requirement written name>={latest} means the newest release the running
devkit can use. The merge keeps the project's own floor when the package is listed and
adds the bare name when it is not; the package step writes the real floor after uv has
resolved it under the constraint. The pyproject template adds devkit-container to
[project].dependencies with its SFTPyPI source, both under if-docker.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 8: The package step: lock under the constraint, write floors, sync, commit `uv.lock`

**Files:**
- Modify: `crates/aeth-devkit-setup/src/packages.rs` (add `advance`), `crates/aeth-devkit-setup/src/lib.rs` (call it after step 1), `crates/aeth-devkit-setup/src/git.rs:26-54` (`uv.lock` committable), `crates/aeth-devkit-setup/Cargo.toml` (`tempfile` becomes a dependency)
- Test: `crates/aeth-devkit-setup/tests/packages.rs` (new)

**Interfaces:**
- Consumes: `Deps` (Task 6), `latest_requested` (Task 7), `locked_version`, `locked_registry_version`, `PackageDirs` (Task 5), `aeth_devkit_core::pyproject::{find_requirement, set_requirement_version, replace_requirement, index_url_for}`, `aeth_devkit_core::version::latest_stable`.
- Produces: `pub fn advance(ctx: &ProjectContext, deps: &crate::Deps, dry_run: bool, latest: &[String], changes: &mut Changes) -> Result<()>` with the behaviour below. `pub const RUNNING_DEVKIT: &str = env!("CARGO_PKG_VERSION")`.

Behaviour, from spec 4.0:
1. No active packages: return.
2. `dry_run`: for each active package not in the venv, note "`<name>` is not installed in this venv; a plain run adds it, locks and syncs"; return. No `uv` call.
3. If `uv.lock` exists and `locked_registry_version(lock, "aeth-devkit")` is `Some(v)` with `v != RUNNING_DEVKIT`: bail `uv.lock pins aeth-devkit {v} but this devkit is {RUNNING_DEVKIT}; run \`uv sync\` so the venv matches the lock, then rerun setup-project`.
4. Write a constraints file containing `aeth-devkit=={RUNNING_DEVKIT}\n` to `<root>/.cache/devkit-constraints.txt` (`.cache/` is gitignored in every devkit-managed project and already holds the tool caches; a fixed path keeps the file inspectable and testable, and nothing needs `tempfile`).
5. `uv lock --upgrade-package <name>… --constraints <file>` through `runner.run_capture`. Non-zero exit: if stderr contains `No solution found`, bail `a devkit package's floor cannot be met by the running devkit {RUNNING_DEVKIT}: {first non-empty stderr line}; run \`devkit lock\`, then rerun setup-project`; otherwise bail with the stderr.
6. Read `uv.lock`; for each active package take `locked_version`; a missing entry is an error naming the package.
7. For each package in `latest`: `find_requirement` in `pyproject.toml`; if its spec is the bare name, replace with `{name}>={locked}`; else `set_requirement_version(spec, locked)` and replace when it differs; write the file and `changes.note(pyproject, "pinned {name}>={locked}")` (with " (was {old})" when there was a floor).
8. Warning: `index_url_for(doc, name)` and `deps.index.versions(url, name)` then `latest_stable`; if the parsed newest is greater than the parsed locked version, `changes.warnings.push("{name} {newest} is available but needs a newer aeth-devkit than the running {RUNNING_DEVKIT}; run `devkit lock`, then rerun setup-project")`. An index error is a warning naming the error, never a stop.
9. If the lock text changed or any active package has no `dir` in the venv: `uv sync --frozen` through `runner.run_inherit`; non-zero exit bails. Then `changes.note(uv_lock_path, "locked {name} {locked}, …")` when the lock changed.

- [ ] **Step 1: Failing integration test**

Create `crates/aeth-devkit-setup/tests/packages.rs`:

```rust
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use aeth_devkit_core::index::StubIndexClient;
use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_core::prompt::ScriptedPrompt;
use aeth_devkit_setup::docker::{Deps as DockerDeps, Mode};
use aeth_devkit_setup::packages::{self, StubPackageDirs, RUNNING_DEVKIT};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("docker")
}

fn lock_with(container: &str) -> String {
  format!(
    "version = 1\n\n[[package]]\nname = \"aeth-devkit\"\nversion = \"{RUNNING_DEVKIT}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n\n[[package]]\nname = \"devkit-container\"\nversion = \"{container}\"\nsource = {{ registry = \"https://idx/+simple\" }}\n"
  )
}

fn project(pyproject: &str, lock: Option<&str>) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
  if let Some(l) = lock {
    fs::write(dir.path().join("uv.lock"), l).unwrap();
  }
  dir
}

const DOCKER_PYPROJECT: &str = "[project]\n  name = \"p\"\n  dependencies = [\"devkit-container\"]\n\n[tool.docker]\n  services = [\"p\"]\n\n[tool.uv.sources]\n  devkit-container = [{ index = \"SFTPyPI\" }]\n\n[[tool.uv.index]]\n  name = \"SFTPyPI\"\n  url = \"https://idx/+simple\"\n  explicit = true\n";

fn advance(root: &Path, runner: &RecordingRunner, index: &StubIndexClient, installed: bool, dry_run: bool) -> aeth_devkit_setup::changes::Changes {
  let prompt = ScriptedPrompt::new(&[]);
  let mut map = HashMap::new();
  if installed {
    map.insert("devkit_container".to_string(), fixtures());
  }
  let dirs = StubPackageDirs(map);
  let deps = aeth_devkit_setup::Deps {
    docker: DockerDeps { runner, prompt: &prompt, reviewer: None, mode: Mode::Ask },
    index,
    packages: &dirs,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(root).unwrap();
  let mut changes = aeth_devkit_setup::changes::Changes::new(dry_run);
  packages::advance(&ctx, &deps, dry_run, &["devkit-container".to_string()], &mut changes).unwrap();
  changes
}

#[test]
fn a_latest_package_is_locked_under_the_devkit_constraint_and_its_floor_written() {
  // The recording runner does not rewrite files, so the lock is pre-written as uv would
  // have left it after upgrading to 1.4.0; the bare `devkit-container` requirement in the
  // pyproject is what the merge writes for a `{latest}` entry.
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let root = dir.path();
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec!["1.0.0".into(), "1.4.0".into()] };
  let changes = advance(root, &runner, &index, true, false);
  let lock_call = &runner.calls_for("uv")[0];
  assert_eq!(&lock_call[..3], &["lock", "--upgrade-package", "devkit-container"], "{lock_call:?}");
  let constraints = lock_call.iter().position(|a| a == "--constraints").map(|i| PathBuf::from(&lock_call[i + 1])).expect("a constraints file");
  assert_eq!(constraints, root.join(".cache").join("devkit-constraints.txt"));
  assert_eq!(fs::read_to_string(&constraints).unwrap().trim(), format!("aeth-devkit=={RUNNING_DEVKIT}"));
  let py = fs::read_to_string(root.join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-container>=1.4.0\""), "{py}");
  assert!(changes.warnings.is_empty(), "{:?}", changes.warnings);
  assert!(changes.files.iter().any(|f| f.path.ends_with("pyproject.toml")));
}

#[test]
fn a_throttled_latest_warns_when_the_index_is_ahead() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec!["1.4.0".into(), "2.0.0".into()] };
  let changes = advance(dir.path(), &runner, &index, true, false);
  assert!(
    changes.warnings.iter().any(|w| w.contains("2.0.0") && w.contains("devkit lock")),
    "{:?}",
    changes.warnings
  );
}

#[test]
fn an_unsatisfiable_floor_stops_with_the_remedy() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  runner.script_err("uv", &["lock"], 1, "  × No solution found when resolving dependencies:\n  ╰─▶ Because devkit-container>=2 depends on aeth-devkit>=12 ...");
  let prompt = ScriptedPrompt::new(&[]);
  let index = StubIndexClient { versions: vec![] };
  let dirs = StubPackageDirs::default();
  let deps = aeth_devkit_setup::Deps {
    docker: DockerDeps { runner: &runner, prompt: &prompt, reviewer: None, mode: Mode::Ask },
    index: &index,
    packages: &dirs,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(dir.path()).unwrap();
  let mut changes = aeth_devkit_setup::changes::Changes::new(false);
  let err = packages::advance(&ctx, &deps, false, &[], &mut changes).unwrap_err().to_string();
  assert!(err.contains("devkit lock") && err.contains(RUNNING_DEVKIT), "{err}");
  assert!(runner.calls_for("uv").iter().all(|c| c[0] != "sync"), "no sync after a failed lock");
}

#[test]
fn a_lock_on_another_devkit_stops_before_locking() {
  let lock = lock_with("1.4.0").replace(&format!("version = \"{RUNNING_DEVKIT}\""), "version = \"0.0.1\"");
  let dir = project(DOCKER_PYPROJECT, Some(&lock));
  let runner = RecordingRunner::new(0);
  let prompt = ScriptedPrompt::new(&[]);
  let index = StubIndexClient { versions: vec![] };
  let dirs = StubPackageDirs::default();
  let deps = aeth_devkit_setup::Deps {
    docker: DockerDeps { runner: &runner, prompt: &prompt, reviewer: None, mode: Mode::Ask },
    index: &index,
    packages: &dirs,
  };
  let ctx = aeth_devkit_setup::context::ProjectContext::discover(dir.path()).unwrap();
  let mut changes = aeth_devkit_setup::changes::Changes::new(false);
  let err = packages::advance(&ctx, &deps, false, &[], &mut changes).unwrap_err().to_string();
  assert!(err.contains("uv sync") && err.contains("0.0.1"), "{err}");
  assert!(runner.calls_for("uv").is_empty());
}

#[test]
fn a_missing_package_is_synced_and_a_dry_run_only_notes() {
  let dir = project(DOCKER_PYPROJECT, Some(&lock_with("1.4.0")));
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec!["1.4.0".into()] };
  let changes = advance(dir.path(), &runner, &index, false, true);
  assert!(runner.calls_for("uv").is_empty(), "dry run runs nothing");
  assert!(changes.notes.iter().any(|n| n.contains("devkit-container") && n.contains("plain run")), "{:?}", changes.notes);
  let runner = RecordingRunner::new(0);
  advance(dir.path(), &runner, &index, false, false);
  let calls = runner.calls_for("uv");
  assert_eq!(calls.last().map(|c| c.as_slice()), Some(&["sync".to_string(), "--frozen".to_string()][..]), "{calls:?}");
}

#[test]
fn nothing_happens_without_docker() {
  let dir = project("[project]\n  name = \"p\"\n", None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let changes = advance(dir.path(), &runner, &index, false, false);
  assert!(runner.calls_for("uv").is_empty() && changes.notes.is_empty());
}
```

Run: `cargo test -p aeth-devkit-setup --test packages` — expected: compile error, `advance` missing.

- [ ] **Step 2: Implement `advance`**

In `packages.rs`:

```rust
use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::pyproject::{find_requirement, index_url_for, replace_requirement, set_requirement_version};
use aeth_devkit_core::version::{latest_stable, parse_lenient};

use crate::changes::Changes;

/// The version of the devkit running this code; every resolution is constrained to it so
/// devkit is never upgraded as a side effect (spec 4.0). All workspace crates share one
/// version, so the setup crate's is the binary's.
pub const RUNNING_DEVKIT: &str = env!("CARGO_PKG_VERSION");

/// Bring the active devkit packages to the newest release the running devkit accepts, write
/// the floors the template marked `{latest}`, and sync the venv. Runs after the pyproject
/// merge has listed the packages and before anything reads them from the venv.
pub fn advance(ctx: &ProjectContext, deps: &crate::Deps, dry_run: bool, latest: &[String], changes: &mut Changes) -> Result<()> {
  let packages = active(ctx);
  if packages.is_empty() {
    return Ok(());
  }
  let missing: Vec<&DevkitPackage> = packages.iter().copied().filter(|p| deps.packages.dir(p.import_name).is_none()).collect();
  if dry_run {
    for p in &missing {
      changes.notes.push(format!(
        "{} is not installed in this venv; a plain run adds it to pyproject.toml, locks it and syncs.",
        p.name
      ));
    }
    return Ok(());
  }
  let root = &ctx.root;
  let lock_path = root.join("uv.lock");
  let lock_before = crate::read_optional(&lock_path)?;
  // The constraint is the running binary's version, so a lock that already names another
  // devkit means the venv is out of step with the lock: the lock step must not paper over
  // that by moving devkit's entry to match the binary.
  if let Some(v) = lock_before.as_deref().and_then(|l| locked_registry_version(l, "aeth-devkit"))
    && v != RUNNING_DEVKIT
  {
    bail!("uv.lock pins aeth-devkit {v} but this devkit is {RUNNING_DEVKIT}; run `uv sync` so the venv matches the lock, then rerun setup-project");
  }
  let constraints = root.join(".cache").join("devkit-constraints.txt");
  std::fs::create_dir_all(root.join(".cache")).context("creating .cache")?;
  std::fs::write(&constraints, format!("aeth-devkit=={RUNNING_DEVKIT}\n")).context("writing the constraints file")?;
  let mut args: Vec<String> = vec!["lock".into()];
  for p in &packages {
    args.push("--upgrade-package".into());
    args.push(p.name.into());
  }
  args.push("--constraints".into());
  args.push(constraints.to_string_lossy().into_owned());
  let out = deps.docker.runner.run_capture("uv", &args, root)?;
  if !out.success() {
    let first = out.stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
    if out.stderr.contains("No solution found") {
      bail!(
        "a devkit package's floor cannot be met by the running devkit {RUNNING_DEVKIT}: {first}; run `devkit lock`, then rerun setup-project"
      );
    }
    bail!("uv lock failed: {}", out.stderr.trim());
  }
  let lock_after = std::fs::read_to_string(&lock_path).context("reading uv.lock after locking")?;
  let pyproject_path = root.join("pyproject.toml");
  let text = std::fs::read_to_string(&pyproject_path).context("reading pyproject.toml")?;
  let mut doc: DocumentMut = text.parse().context("parsing pyproject.toml")?;
  let mut pinned: Vec<String> = Vec::new();
  let mut locked_all: Vec<String> = Vec::new();
  for p in &packages {
    let locked = locked_version(&lock_after, p.name).with_context(|| format!("{} is not in uv.lock after locking", p.name))?;
    locked_all.push(format!("{} {locked}", p.name));
    if latest.iter().any(|n| n == p.name)
      && let Some(req) = find_requirement(&doc, p.name)
    {
      let new_spec = if req.spec.trim() == p.name {
        Some(format!("{}>={locked}", p.name))
      } else {
        set_requirement_version(&req.spec, &locked)
      };
      if let Some(new_spec) = new_spec
        && new_spec != req.spec
      {
        pinned.push(if req.spec.trim() == p.name {
          format!("pinned {new_spec}")
        } else {
          format!("pinned {new_spec} (was {})", req.spec)
        });
        replace_requirement(&mut doc, &req, &new_spec);
      }
    }
    // The nudge: newer on the index than uv could choose under the constraint.
    if let Some(url) = index_url_for(&doc, p.name) {
      match deps.index.versions(&url, p.name) {
        Ok(versions) => {
          let newest = latest_stable(versions.iter().map(String::as_str));
          if let (Some(newest), Some(have)) = (newest.as_deref().and_then(parse_lenient), parse_lenient(&locked))
            && newest > have
          {
            changes.warnings.push(format!(
              "{} {newest} is available but needs a newer aeth-devkit than the running {RUNNING_DEVKIT}; run `devkit lock`, then rerun setup-project",
              p.name
            ));
          }
        }
        Err(e) => changes.warnings.push(format!("could not check {url} for a newer {}: {e:#}", p.name)),
      }
    }
  }
  if !pinned.is_empty() {
    std::fs::write(&pyproject_path, doc.to_string()).context("writing pyproject.toml")?;
    for line in &pinned {
      changes.note(&pyproject_path, line);
    }
  }
  let lock_changed = lock_before.as_deref() != Some(lock_after.as_str());
  if lock_changed || !missing.is_empty() {
    match deps.docker.runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root)? {
      Some(0) => {}
      Some(code) => bail!("uv sync --frozen exited with {code}"),
      None => bail!("uv sync --frozen was terminated by a signal"),
    }
  }
  if lock_changed {
    changes.note(&lock_path, &format!("locked {}", locked_all.join(", ")));
  }
  Ok(())
}
```

Also import `Changes` is unused if `changes` typed via path; keep imports tidy so clippy passes.

- [ ] **Step 3: Call it from `run_with` and make `uv.lock` committable**

In `lib.rs`, inside the step 1 block keep a copy of the loaded template text, then after the block:

```rust
  // 1b. The devkit packages (spec 4.0): list, lock under the running devkit, sync. Before
  //     the Docker step, which renders the Dockerfile from the installed container package.
  packages::advance(ctx, deps, dry_run, &packages::latest_requested(&pyproject_template), &mut changes)?;
```

where `pyproject_template` is the `template` string from step 1 (rename the local so it is visible after the block). In `git.rs::committable`, add `"uv.lock",` directly after `"pyproject.toml",` with the comment `// Written by the package step (uv lock); committed with the run like pyproject.toml.`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p aeth-devkit-setup --test packages --test docker --test apply`
Expected: PASS. The `docker.rs` tests script no `uv` calls, and the `RecordingRunner` returns exit 0 for unscripted programs with empty output; the fixture projects have no `uv.lock`, so `advance` reads none and fails at "not in uv.lock after locking". Give the `project()` helper in `tests/docker.rs` and `tests/apply.rs` a `uv.lock` in the shape of `lock_with("1.4.0")` above for Docker projects, then rerun.

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-setup
git commit -m "feat(setup): lock the devkit packages under the running devkit and sync the venv

The package step from spec 4.0: uv lock --upgrade-package for every active devkit
package with a constraints file pinning aeth-devkit to the running version, so devkit
is never upgraded as a side effect. An unsatisfiable explicit floor stops the run with
the devkit lock remedy; a {latest} package gets its floor written to the version uv
chose and a warning when the index is ahead; uv sync --frozen follows when the lock
moved or a package was missing. uv.lock joins the committable set so the run's commit
carries it.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 9: `docker-pin` refreshes a drifted Dockerfile

**Files:**
- Modify: `crates/aeth-devkit-pin/Cargo.toml`, `crates/aeth-devkit-pin/src/lib.rs:52-60,96-105,236-245`, `crates/aeth-devkit/src/main.rs:66-75` (unchanged args; `run_real` builds the new dep)
- Test: `crates/aeth-devkit-pin/tests/pin.rs`

**Interfaces:**
- Consumes: `aeth_devkit_setup::docker::static_files::{render, normalize_newlines}`, `aeth_devkit_setup::packages::{PackageDirs, SystemPackageDirs, StubPackageDirs, locked_version, installed_version, CONTAINER}`, `aeth_devkit_setup::context::ProjectContext`.
- Produces: `pin::Deps` gains `pub packages: &'a dyn PackageDirs`; `pub fn refresh_dockerfile(root: &Path, deps: &Deps, dry_run: bool, will_commit: bool) -> Result<()>` called from `run` after the compose path is printed and before the compose content is decided.

- [ ] **Step 1: Failing tests**

`tests/pin.rs` already has `fixture()` (a committed repo with `pyproject.toml` and `compose.yaml`), `git()`, `args()`, `happy_runner()` and `pushed()`. Every existing `&Deps { runner: &r, index: &idx }` in that file gains `packages: &StubPackageDirs::default()` (the temporary lives for the `run(...)` statement). The existing fixture has no `[tool.docker]`, so `has_docker` is false there and the refresh is skipped, which is what keeps those tests unchanged. Add:

```rust
use aeth_devkit_setup::packages::StubPackageDirs;

const DOCKER_PYPROJECT: &str = "[project]\nname = \"my-package\"\n\n[tool.docker]\nservices = [\"app\"]\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/internal/\"\n";

fn lock(container: &str) -> String {
  format!(
    "version = 1\n\n[[package]]\nname = \"devkit-container\"\nversion = \"{container}\"\nsource = {{ registry = \"https://x/+simple\" }}\n"
  )
}

/// `fixture()` made a Docker project: `services` set, a committed Dockerfile, and a lock
/// naming devkit-container when `locked` is given.
fn docker_fixture(dockerfile: &str, locked: Option<&str>) -> (tempfile::TempDir, PathBuf) {
  let (dir, root) = fixture();
  std::fs::write(root.join("pyproject.toml"), DOCKER_PYPROJECT).unwrap();
  std::fs::create_dir_all(root.join("docker")).unwrap();
  std::fs::write(root.join("docker/Dockerfile"), dockerfile).unwrap();
  if let Some(v) = locked {
    std::fs::write(root.join("uv.lock"), lock(v)).unwrap();
  }
  git(&root, &["add", "."]);
  git(&root, &["commit", "-q", "-m", "docker"]);
  (dir, root)
}

/// A fake site-packages holding devkit_container at `version` with `template` as its Dockerfile.
fn installed(version: &str, template: &str) -> (tempfile::TempDir, StubPackageDirs) {
  let site = tempfile::tempdir().unwrap();
  let pkg = site.path().join("devkit_container");
  std::fs::create_dir_all(&pkg).unwrap();
  std::fs::create_dir(site.path().join(format!("devkit_container-{version}.dist-info"))).unwrap();
  std::fs::write(pkg.join("template.Dockerfile"), template).unwrap();
  let mut map = std::collections::HashMap::new();
  map.insert("devkit_container".to_string(), pkg);
  (site, StubPackageDirs(map))
}

fn subjects(root: &Path) -> Vec<String> {
  let out = Command::new("git").current_dir(root).args(["log", "--format=%s"]).output().unwrap();
  String::from_utf8_lossy(&out.stdout).lines().map(str::to_string).collect()
}

#[test]
fn a_drifted_dockerfile_is_replaced_and_committed_before_the_pin() {
  let (_d, root) = docker_fixture("FROM old\n", Some("1.4.0"));
  let (_site, dirs) = installed("1.4.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient { versions: vec!["2.0.0".into()] };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &Deps { runner: &r, index: &idx, packages: &dirs }).unwrap();
  assert_eq!(std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(), "FROM new src\n");
  let log = subjects(&root);
  assert_eq!(log[0], "chore: pin my-package to 2.0.0", "{log:?}");
  assert_eq!(log[1], "chore(docker): refresh Dockerfile from devkit-container 1.4.0", "{log:?}");
  assert!(r.calls_for("uv").is_empty(), "installed 1.4.0 matches the lock: no sync");
}

#[test]
fn a_stale_venv_is_synced_before_the_dockerfile_is_compared() {
  let (_d, root) = docker_fixture("FROM new src\n", Some("1.4.0"));
  let (_site, dirs) = installed("1.3.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient { versions: vec!["2.0.0".into()] };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &Deps { runner: &r, index: &idx, packages: &dirs }).unwrap();
  assert_eq!(r.calls_for("uv")[0], vec!["sync", "--frozen"]);
  assert!(!subjects(&root).iter().any(|s| s.contains("refresh Dockerfile")), "the file already matched");
}

#[test]
fn without_the_package_in_the_lock_the_refresh_is_skipped() {
  let (_d, root) = docker_fixture("FROM old\n", None);
  let (_site, dirs) = installed("1.4.0", "FROM new {python_dir}\n");
  let r = happy_runner();
  let idx = StubIndexClient { versions: vec!["2.0.0".into()] };
  let mut a = args(&root);
  a.no_push = true;
  run(&a, &Deps { runner: &r, index: &idx, packages: &dirs }).unwrap();
  assert_eq!(std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap(), "FROM old\n");
  assert!(r.calls_for("uv").is_empty());
  assert_eq!(subjects(&root)[0], "chore: pin my-package to 2.0.0");
}
```

(`python_dir` resolves to `src` because the fixture has no package directory, which is why the rendered text is `FROM new src`.)

Run: `cargo test -p aeth-devkit-pin --test pin` — expected: compile errors (`packages` field, `refresh_dockerfile`).

- [ ] **Step 2: Implement**

Add `aeth-devkit-setup = { workspace = true }` to the pin crate's `[dependencies]`. In `lib.rs`:

```rust
use aeth_devkit_setup::context::ProjectContext;
use aeth_devkit_setup::docker::static_files::{normalize_newlines, render};
use aeth_devkit_setup::packages::{self, PackageDirs};

pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub index: &'a dyn IndexClient,
  pub packages: &'a dyn PackageDirs,
}

/// Before a pin: make the committed Dockerfile match the locked devkit-container's
/// template. The lock can advance the entrypoint (`poe lock`) while the Dockerfile stays on
/// the old shape, and a deploy would build the mismatch; this is the last moment before every
/// deploy. The file is devkit-owned, so drift is replaced without a prompt, in its own commit
/// ahead of the pin's.
pub fn refresh_dockerfile(root: &Path, deps: &Deps, dry_run: bool, will_commit: bool) -> Result<()> {
  let ctx = ProjectContext::discover(root)?;
  if !ctx.has_docker {
    return Ok(());
  }
  let lock = std::fs::read_to_string(root.join("uv.lock")).ok();
  let Some(locked) = lock.as_deref().and_then(|l| packages::locked_version(l, packages::CONTAINER.name)) else {
    println!("Dockerfile: devkit-container is not in uv.lock; run setup-project to adopt it. Skipping the refresh.");
    return Ok(());
  };
  // A venv behind the lock would render the wrong version's template.
  let installed = deps.packages.dir(packages::CONTAINER.import_name).and_then(|d| packages::installed_version(&d));
  if installed.as_deref() != Some(locked.as_str()) {
    println!("Syncing the venv (devkit-container {locked} is locked, {} installed)", installed.as_deref().unwrap_or("nothing"));
    match deps.runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root)? {
      Some(0) => {}
      Some(code) => bail!("uv sync --frozen exited with {code}"),
      None => bail!("uv sync --frozen was terminated by a signal"),
    }
  }
  let Some(rendered) = render(&ctx, deps.packages)? else {
    bail!("devkit-container {locked} is locked but not importable from this venv after uv sync");
  };
  let rel = "docker/Dockerfile".to_string();
  let path = root.join("docker").join("Dockerfile");
  let worktree = std::fs::read_to_string(&path).ok();
  let dirty = git::is_dirty(root, &[&rel])?;
  let head = git::head_blob(root, &rel)?;
  let base_text = match (&head, will_commit && dirty) {
    (Some(h), true) => String::from_utf8(h.clone()).context("docker/Dockerfile at HEAD is not UTF-8")?,
    _ => worktree.clone().unwrap_or_default(),
  };
  if normalize_newlines(&base_text) == normalize_newlines(&rendered) {
    println!("Dockerfile: matches devkit-container {locked}.");
    return Ok(());
  }
  println!("Dockerfile: drifted from devkit-container {locked}; refreshing.");
  if dry_run {
    return Ok(());
  }
  let message = format!("chore(docker): refresh Dockerfile from devkit-container {locked}");
  if will_commit && dirty && head.is_some() {
    let base = head.unwrap();
    let current = git::worktree_blob(root, &rel)?.with_context(|| format!("{rel} vanished during the run"))?;
    let merged = git::merge_file(root, &current.bytes, &base, rendered.as_bytes())?
      .context("your uncommitted Dockerfile changes overlap the refreshed lines; commit or revert them first")?;
    let mode = git::head_mode(root, &rel)?.unwrap_or_else(|| "100644".into());
    let sha = git::hash_object(root, rendered.as_bytes())?;
    git::commit_files_on_head(root, &[git::IndexEntry { path: rel.clone(), staged: Some((mode, sha)) }], &message)?;
    git::write_worktree(root, &rel, &merged, current.filtered).with_context(|| format!("writing {}", path.display()))?;
    println!("Committed the Dockerfile refresh on HEAD; your uncommitted changes to {rel} were kept in the working tree.");
  } else {
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, &rendered).with_context(|| format!("writing {}", path.display()))?;
    if will_commit {
      let hash = git::commit_paths(root, std::slice::from_ref(&rel), &message)?;
      println!("Committed {hash}: {message}");
    }
  }
  Ok(())
}
```

In `run`, right after `println!("Compose  : {rel}");` and the `will_commit` / `will_push` lines, insert `refresh_dockerfile(&root, deps, args.dry_run, will_commit)?;`. In `run_real`, pass `packages: &aeth_devkit_setup::packages::SystemPackageDirs`. The `git` functions named above all exist in `aeth_devkit_core::git` (they are what the compose pin path already calls).

- [ ] **Step 3: Run the tests**

Run: `cargo test -p aeth-devkit-pin && cargo build -p aeth-devkit`
Expected: PASS; the dispatcher builds unchanged because `release_and_pin` only constructs `Args`.

- [ ] **Step 4: Commit**

```bash
git add crates/aeth-devkit-pin crates/aeth-devkit/src/main.rs
git commit -m "feat(pin): refresh a drifted Dockerfile from the locked devkit-container before pinning

docker-pin is the last step before every deploy, so it renders the Dockerfile template
from the installed devkit-container (syncing the venv first when it lags uv.lock) and
replaces the committed docker/Dockerfile when it differs, in its own commit ahead of the
pin commit, with the same dirty-file handling the compose pin uses. The file is
devkit-owned; there is no prompt.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 10: Remove the crate and its release stream from `aeth-devkit`

**Files:**
- Delete: `crates/aeth-devkit-container/` (whole directory), `python/aeth_devkit/templates/docker/template.Dockerfile`, `.github/workflows/devkit-container.yml`
- Modify: `Cargo.toml:30-31` (workspace dependency line), `Cargo.lock` (regenerated), `.github/workflows/ci.yml:76-93` (drop `container-smoke`), `README.md:36-134,222-287`, `TODO.md`, `WORKSPACE.md`

**Interfaces:**
- Produces: a workspace of eight crates; CI without the smoke job; documentation that describes only what remains.

- [ ] **Step 1: Delete and rebuild**

```bash
git rm -r -q crates/aeth-devkit-container python/aeth_devkit/templates/docker/template.Dockerfile .github/workflows/devkit-container.yml
```

Remove the line `aeth-devkit-container = { path = "crates/aeth-devkit-container" }` from `Cargo.toml`. Remove the `container-smoke` job from `.github/workflows/ci.yml` (the block from its comment through its last `run:` line). Then:

```bash
cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
git status --short Cargo.lock
```

Expected: builds; `Cargo.lock` shows the crate removed.

- [ ] **Step 2: README**

Replace the `### \`devkit-container\`` section (from its heading to the line before `### VS Code extension`) with:

```markdown
### `devkit-container`

Lives in its own repository, `AetherBreaker/devkit-container`, and is distributed as a wheel on
SFTPyPI. `setup-project` adds it to `[project].dependencies` of every project with
`[tool.docker].services`, locks it to the newest release this devkit accepts, and renders
`docker/Dockerfile` from the `devkit_container/template.Dockerfile` in the project's venv, so
the Dockerfile and the entrypoint the image installs are always the same version.
`docker-pin` refreshes a Dockerfile that drifted from the locked version before it pins. See
that repository's README for the binary's subcommands and the `[tool.docker]` schema.
```

In the `devkit setup-project` section, find the Docker bullets that describe filling a `container-v<N>` pin and replace that sentence with: "The Dockerfile is rendered from the installed `devkit-container` package; the package step (below) installs and advances it first." Add one bullet to that section:

```markdown
- **Devkit packages.** For Docker projects, `devkit-container` is added when missing, locked
  with `uv lock --upgrade-package` under the constraint `aeth-devkit==<the running version>`,
  and installed with `uv sync --frozen`; the floor written to `pyproject.toml` is the version
  uv chose. A floor no release can meet with this devkit stops the run with "run `devkit
  lock`"; a newer release on the index that needs a newer devkit is a warning. `uv.lock` is
  committed with the run. Devkit itself is never upgraded here; that is `devkit lock`'s job.
```

In the `devkit docker-pin` section add: "Before pinning, the committed `docker/Dockerfile` is compared with the template of the locked `devkit-container` (the venv is synced first if it lags `uv.lock`) and replaced, without a prompt, in its own commit when it differs."

- [ ] **Step 3: TODO**

Delete the entry beginning `A command that advances a Dockerfile's \`container-v<N>\` pin`. Under `## setup-project` add:

```markdown
- [ ] Dockerfile customisation: `docker/Dockerfile` is devkit-owned and `docker-pin` replaces
      drift without asking, so a hunk kept in `setup-project` does not survive the next pin.
      Consider a mechanism for project-specific Dockerfile edits, or a `[tool.devkit]` setting
      that opts a project out of Dockerfile management.
```

- [ ] **Step 4: Full verification**

```bash
cargo test --workspace
uv sync && uv run pytest
git diff --exit-code -- python/aeth_devkit/_tasks_generated.py
```

Expected: everything green. This is the one full-suite run for the branch.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: remove the container crate, its template and its release stream

devkit-container now lives in its own repository and reaches projects as a wheel; the
crate, the Dockerfile template under the templates directory, the container-vN workflow
and the CI smoke job leave this one. README and TODO describe what remains, and TODO
gains the Dockerfile opt-out entry from the split spec.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

### Task 11: Pull request and rollout

**Files:** none new.

- [ ] **Step 1: Push and open the PR**

```bash
git push -u origin feat/extract-devkit-container
gh pr create --title "feat: extract devkit-container as a wheel and manage it from the venv" --body "$(cat <<'EOF'
Step 1 of docs/superpowers/specs/2026-09-08-devkit-split-design.md.

- devkit-container is its own repository (AetherBreaker/devkit-container, released as 1.0.0 on SFTPyPI) with its Dockerfile template as package data.
- setup-project adds it to Docker projects, locks it under `aeth-devkit==<running>`, writes `{latest}` floors, syncs, commits uv.lock, and renders docker/Dockerfile from the venv (spec 4.0, 4.1).
- docker-pin refreshes a drifted Dockerfile before pinning.
- The crate, its template and the container-vN workflow are removed here.

Breaking for consumers: the Dockerfile shape changes and setup-project now edits runtime dependencies and runs uv; release as a major.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

- [ ] **Step 2: After merge, the rollout (run by hand, each needs a terminal)**

1. In `aeth-devkit` on `main`: `poe release major`.
2. In each of `aeth_ext`, `IMAPReportCollector`, `ScheduledInvoiceProcessor`, `ScheduledReportAggregator`: `poe lock` (takes the new devkit), then `poe setup-project` (adds `devkit-container`, locks and syncs it, offers the new Dockerfile as a whole-file diff: answer `replace`), then `poe docker-pin` (verifies the Dockerfile matches and re-pins), then push. Coolify redeploys on the `docker/` change; watch the first build of each pull `devkit-container` from SFTPyPI.
3. Add `devkit-container` to the clone list and the `.env` copy loop in `WORKSPACE.md` if a later step forgot it (Task 4 already did).

---

## Self-review notes

- Spec 4.0 order says "the templates package first"; that package arrives in step 4, so this plan runs the package step after the pyproject merge and before the Docker step, which is the same order for the container. Step 4's plan moves the templates part ahead of the merge.
- Spec 4.0's "explicit floor cannot be met" and "`{latest}` throttled" are both decided by uv's output (Task 8 step 2); devkit parses no package metadata.
- `if-docker` on the new template tables also fires for a project that has Docker files but no `services` (the `[tool.docker]` seed case). Such a project then carries the dependency without setup-project managing its Dockerfile. No such project exists today; accepted.
- The smoke test's wheel carries a musl binary under a generic `linux` tag; the release wheel is manylinux. Both run the same code; the image test covers the packaging path, CI's wheel job covers the manylinux build.

## Execution notes (Part A done 2026-09-08)

- Tasks 1–4 are complete: `AetherBreaker/devkit-container` exists, CI is green, `v1.0.0` is released and `devkit-container==1.0.0` resolves from SFTPyPI. Part B starts at Task 5 on branch `feat/extract-devkit-container`.
- Task 1: filter-repo kept 13 commits and re-pointed six aeth-devkit tags at surviving commits; they were deleted before the push. A `.gitattributes` (`* text=auto eol=lf`) was added so a Windows checkout builds an LF template into the wheel.
- Task 3 changed design: `uv sync --frozen` removes any package the lock does not name, so a `uv pip install` of the wheel between the template's syncs was uninstalled by the next sync. The scratch app now depends on `devkit-container` through a `[tool.uv.sources]` path source to the local wheel (copied to `/app/wheels/` before the first sync), which is also the faithful shape: the image installs it from `uv.lock`.
- Task 4: `setup-project` needs a real terminal (`IsTerminal` on stdin); the tool shells have none, and opening console windows is not acceptable. It was run once successfully; future wet runs are the user's to run. `devkit release` reads the SFTPyPI credentials from the environment, so it is run as `poe release --force` after copying the two `UV_INDEX_SFTPYPI_*` lines from `aeth-devkit/.env` into the repo's `.env`; WORKSPACE.md says so.
- A stash "setup-project output, first run" remains in `devkit-container`; it is superseded by the commit and can be dropped.

## Execution notes (Part B done 2026-09-09)

- Tasks 5–10 are complete on `feat/extract-devkit-container`; Task 11 opens the PR. Each task had a fresh review; the findings that changed the design are below.
- Task 8: `uv lock` has no `--constraints` flag (uv 0.11 rejects it), so the plan's central command would have failed on every real run and the recording runner could not see it. A version specifier on `--upgrade-package` is a hard constraint for that resolution (verified against SFTPyPI: `aeth-devkit==<running>` holds devkit while `devkit-container` moves; an unmeetable pin is "No solution found"). The package step passes `--upgrade-package aeth-devkit==<running>` and writes no constraints file. It also syncs whenever an installed devkit package's version differs from the lock, not only when absent.
- Task 7: `{latest}` is scrubbed to a bare name in tables and arrays copied whole from the template too, so the placeholder cannot reach a project file and the template's multi-line layout survives. `active()` uses the same condition as the `if-docker` gate (`has_docker || docker_files`) so a dependency the merge adds is always one the step locks.
- Task 9: the refresh is decided before the pin's already-pinned short-circuit and written after the behind-origin preflight, ahead of the pin commit; the already-pinned path pushes a refresh commit. `--no-commit` merges uncommitted Dockerfile edits 3-way like commit mode. The installed version is checked again after `uv sync --frozen`.
- Tasks 5/6: `PackageDirs::dir(root, import_name)` takes the project root and `SystemPackageDirs` asks only `<root>/.venv`, never the interpreter beside the binary or PATH, which can answer from another environment; the interpreter query runs with `-X utf8`.
