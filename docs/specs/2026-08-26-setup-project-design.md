# `poe setup-project` — design spec

Status: READY FOR IMPLEMENTATION (v2, template-driven)

## Purpose

A one-and-done, idempotent script that standardizes a project's configuration from a set
of **templates shipped inside `aeth_devkit`**. Two goals:

1. All generated clutter lands in `<project_root>/.cache/`.
2. Tool/editor/repo configuration travels *with* each repository (no more `extends`
   chains to parent directories), while still being maintained in exactly one place:
   the templates. To change a convention, edit the template and re-run
   `poe setup-project` in each project.

Run once per project; re-run only when something drifts or a template changes.

## Intended new-project workflow

1. `uv init --lib`
2. Manually add the SFTPyPI index + `tool.uv.sources.aeth-devkit` and add `aeth-devkit` to the
   `dev` dependency group.
3. `uv sync --upgrade --all-extras`
4. `poe setup-project`

Anything steps 1–3 already produce is **out of scope** for the script: `[project]`
metadata, `requires-python`, `[build-system]`, `src/<pkg>/` layout, `py.typed`,
`README.md`, `.python-version`, the SFTPyPI index/source blocks, and the `aeth-devkit` dev
dependency (its pin is maintained by `poe lock`).

Explicitly **not** responsible for: cleaning existing stragglers (`poe clean`), Docker or
deployment files, moving `dist/`, or deleting `.cache/`.

## Invocation

```
poe setup-project [--check] [--dry-run] [--no-commit] [--root PATH] [--templates-dir PATH]
```

- Implemented in **Rust** as the `setup-project` subcommand of the `devkit` binary
  (crate `aeth-devkit-setup`, dispatched by crate `aeth-devkit`) and packaged into the
  `aeth_devkit` wheel by **maturin (`bindings = "bin"`)** — no pyo3, no Python ABI coupling.
  The poe task is simply `cmd = "devkit setup-project"`, the same pattern as the existing
  shell scripts. See "Rust / packaging" below.
- Runs against cwd (must contain `pyproject.toml`).
- `--dry-run`: print changes, write nothing. `--check`: dry-run + exit 1 if anything would change.
- Prints one line per changed file; silent for unchanged files. Second run = no output.
- After the merges are written, `tombi format pyproject.toml` runs so the result is sorted
  and cleanly formatted. The step is best-effort: if `tombi` is not on `PATH` it is skipped
  with a note; if it exits non-zero the merged file is kept as-is with a warning. Any
  reformatting counts as a change (reported and committed with the rest). Not run under
  `--dry-run` / `--check`.
- When the project is git-tracked, the changed files are committed automatically
  (`git commit -- <files>` so pre-staged user changes are untouched; env files and
  gitignored files are never committed). `--no-commit` opts out.

## Templates

Live in `python/aeth_devkit/templates/` and ship with the wheel. Template files keep a real
extension (for editor support) but never the exact filename a tool keys on:
`pyproject.template.toml`, `vscode/settings.template.jsonc` (jsonc so comments are valid),
`template.gitignore`, `template.env`, …. The table below lists them by target name:

| Template                   | Target                       | Merge mode |
| -------------------------- | ---------------------------- | ---------- |
| `pyproject.toml`           | `pyproject.toml`             | TOML deep-merge |
| `vscode/settings.json`     | `.vscode/settings.json`      | JSON deep-merge |
| `vscode/launch.json`       | `.vscode/launch.json`        | create-if-missing + per-config env patch |
| `vscode/extensions.json`   | `.vscode/extensions.json`    | JSON deep-merge (`recommendations` list-union) |
| `gitignore`                | `.gitignore`                 | replace-or-prepend (see below) |
| `dockerignore`             | `.dockerignore`              | line-union, only if `docker/` or `Dockerfile*` exists |
| `gitattributes`            | `.gitattributes`             | line-union |
| `env`                      | `.env` + other referenced env files | key upsert |

Templates may use `{placeholders}` resolved at runtime:

| Placeholder          | Value |
| -------------------- | ----- |
| `{project_root}`     | absolute path, native separators |
| `{package}`          | import name: sole directory under `src/` if unambiguous, else `project.name` with `-`→`_` |

### Merge modes

**TOML deep-merge** (`pyproject.toml`): walk the template; tables merge recursively,
every leaf (scalar *or* array) present in the template **overwrites** the project value;
keys absent from the template are left untouched (project name, version, deps,
`tool.docker`, `tool.uv.sources` for other packages, …). Comments/formatting preserved via
`tomlkit`. Special rules:
- **Arrays are unioned**, never replaced: template entries missing from the project array
  are appended (order preserved, existing entries untouched). Whenever entries are added
  to an existing array, a trailing line comment is attached listing exactly what was
  added, e.g. `# setup-project added: "PERF", "RUF"`, so local tweaking is easy. A
  re-run that adds nothing leaves the comment as-is; a re-run that adds more replaces
  the comment with the new additions only.
- **Scalars are replaced** when present; added when absent.
- **Opt-out**: `[tool.setup-project].keep = ["tool.pyright.reportMissingTypeStubs", ...]`
  lists dotted keys the merge must never touch (scalar or array). The table itself is
  never written by the script.
- `tool.ruff.extend` and `tool.pyright.extends` are **removed** from the project once the
  template has inlined the full config (that is the whole point).
- `dependency-groups.dev`: ensure an entry for each template dep by package name; a
  matching existing entry (any specifier) is replaced with the template's specifier.
  (`aeth-devkit` itself is not in the template — see workflow above.)

**JSON deep-merge** (`settings.json`, `extensions.json`): same as TOML; objects merge,
leaves overwrite, lists replace unless flagged union. JSONC input tolerated (comments and
trailing commas stripped; the leading `//` header block is preserved verbatim).

**launch.json**: if absent, write the template. If present, do **not** add/remove
configurations; for every configuration with `type` in `{debugpy, python}` and
`request == "launch"`: ensure `envFile` (default `${workspaceFolder}/.env`, existing value
kept) and set `env.PYTHONPYCACHEPREFIX`, `env.PYTHONUNBUFFERED`, `env.PYTHONSAFEPATH` from
the template's `env` block (overwrite; other `env` keys kept). Non-Python configs, attach
configs and compounds are untouched. `tasks.json` is likewise patched
(`options.env.PYTHONPYCACHEPREFIX` on every task) but never created — tasks are
project-specific.

**gitignore replace-or-prepend**: the template is a vendored copy of
<https://github.com/github/gitignore/blob/main/Python.gitignore> (plus a short SFT block:
`.cache/`, `*.env`, `persisted_data/`). If the existing `.gitignore` contains no lines
outside the template (ignoring blanks/whitespace; `.cache` ≡ `.cache/`) — which is what
`uv init` leaves behind — it is **replaced** by the template. Otherwise the template is
**prepended** and every line of the original that already appears in the template is
dropped from the remainder, so the project-specific tail survives once, without
duplicates. Vendored copy is refreshed by hand when GitHub updates it.

**line-union** (`.dockerignore`, `.gitattributes`): create from template if
absent; otherwise append each template line not already present (comparison ignores
surrounding whitespace and treats `.cache` ≡ `.cache/`). Never removes or reorders.

**env upsert**: for each `KEY=value` in the `env` template, replace the existing `KEY=`
line in place or append; all other lines preserved byte-for-byte; line endings preserved;
file created if missing. Applied to `.env` and to every distinct `envFile` referenced in
`launch.json` (e.g. `testing.env`).

## Template contents (v1)

### `pyproject.toml`
- `[dependency-groups].dev`: `poethepoet>=0.46.0`, `pyright>=1.1.411`
- `[tool.poe].include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]`
- `[tool.pyright]`: full block currently in the grandparent `pyproject.toml` (no `extends`);
  `executionEnvironments = [{ root = "src", extraPaths = ["src"] }]`
- `[tool.ruff]`: full block from grandparent (`exclude`, `fix`, `indent-width`, `line-length`,
  `format`, `lint.extend-select`, `lint.ignore`, `lint.isort.*`) + `cache-dir = ".cache/ruff"`,
  `src = ["./src"]`, `lint.isort.known-first-party = ["{package}"]`
- `[tool.tombi]`: full block from grandparent
- `[tool.pytest.ini_options]`: `addopts`, `cache_dir = ".cache/pytest"`, `testpaths`, `xfail_strict`, `asyncio_mode`
- `[tool.coverage.run]`: `data_file = ".cache/.coverage"`, `source_pkgs = ["{package}"]`;
  `[tool.coverage.report].show_missing = true`; `[tool.coverage.html].directory = ".cache/htmlcov"`;
  `[tool.coverage.xml].output = ".cache/coverage.xml"`; `[tool.coverage.lcov].output = ".cache/coverage.lcov"`
- `[tool.mypy].cache_dir = ".cache/mypy"` — applied only if `mypy` is in any dependency list
  (the only conditional section; marked in the template with a `# setup-project: if-dep mypy` comment)

### `vscode/settings.json`
```jsonc
"python.envFile": "${workspaceFolder}/.env",
"python.testing.pytestEnabled": true, "python.testing.unittestEnabled": false,
"python.testing.pytestArgs": ["tests"],
"[python]": { "editor.defaultFormatter": "charliermarsh.ruff", "editor.formatOnSave": true },
"terminal.integrated.env.windows": { "PYTHONPYCACHEPREFIX": "${workspaceFolder}\\.cache\\pycache" },
"terminal.integrated.env.linux":   { "PYTHONPYCACHEPREFIX": "${workspaceFolder}/.cache/pycache" },
"terminal.integrated.env.osx":     { "PYTHONPYCACHEPREFIX": "${workspaceFolder}/.cache/pycache" },
"files.exclude":        { ".cache": true, ".venv": true, "**/__pycache__": true },
"search.exclude":       { ".cache": true, ".venv": true, "**/__pycache__": true },
"files.watcherExclude": { "**/.cache/**": true, "**/.venv/**": true, "**/__pycache__/**": true }
```
The terminal env is a straggler-catcher *in addition to* `.env`; `python.envFile` only
feeds extension-spawned processes, and debugpy only reads `.env` when `envFile` is set.

### `vscode/launch.json`
One `Current File` debugpy config: `program=${file}`, `console=integratedTerminal`,
`justMyCode=false`, `cwd=${workspaceFolder}`, `envFile=${workspaceFolder}/.env`,
`env = { PYTHONPATH: ${workspaceFolder}/src, PYTHONPYCACHEPREFIX: ${workspaceFolder}/.cache/pycache, PYTHONUNBUFFERED: "1", PYTHONSAFEPATH: "1" }`.

### `vscode/extensions.json`
`recommendations`: `ms-python.python`, `ms-python.debugpy`, `charliermarsh.ruff`,
`ms-python.vscode-pylance`, `tamasfe.even-better-toml` (list-union; project extras kept).

### `env`
`PYTHONPYCACHEPREFIX="{project_root}\.cache\pycache"` (backslashes are kept literal by
both poethepoet's and VS Code's env parsers — verified).

### `gitignore` / `dockerignore` / `gitattributes`
- gitignore: vendored GitHub `Python.gitignore` + SFT block (`.cache/`, `*.env`, `persisted_data/`).
- dockerignore: `.cache/`, `.venv/`, `**/__pycache__`, `.git`, `.vscode`, `*.env`, `dist/`.
- gitattributes: `* text=auto eol=lf`, `*.sh text eol=lf`.

## Naming
Bytecode cache dir is `.cache/pycache` everywhere (existing `.cache/__pycache__` values in
`.env`/launch.json are overwritten to match).

## Idempotency / safety
- Second run makes no changes. Never deletes files/dirs. Never rewrites secrets.
- `.env` handled line-wise only; `pyproject.toml` via tomlkit round-trip.

## Decided / out of scope (2026-08-26)
No `poe clean` changes; no `dist/` move; no Docker/compose edits; no straggler or
git-tracked-file reports; nothing `uv init --lib` or the manual index step already
produces (`.python-version`, README, `requires-python`, build-system, SFTPyPI index/source,
`aeth-devkit` dev dep).

## Resolved (2026-08-26)

1. Arrays union with an `# setup-project added: …` comment; scalars replace.
2. `[tool.setup-project].keep` opt-out list is honoured.
3. `extends` / `extend` lines pointing at `../pyproject.toml` are removed.

## Rust / packaging

Why Rust rather than Python: the merge logic wants a first-class comment-preserving TOML
editor (`toml_edit`, the one `cargo` itself uses), the binary is usable outside poe and
outside Python projects, a broken build fails at `uv build` rather than at `poe` import
time, and the maintainer wants to learn Rust. The higher-frequency shell scripts
(`release.sh`, `lock.sh`, `docker-pin-latest.sh`, `rescind-release.sh`) are expected to
migrate to the same binary later once this path is proven.

Layout:

```text
aeth-devkit/
├── pyproject.toml                 # build-backend = "maturin", [tool.maturin] bindings = "bin"
├── Cargo.toml                     # [workspace] members = crates/*
├── crates/aeth-devkit/            # [[bin]] name = "devkit" — the shipped dispatcher
├── crates/aeth-devkit-setup/      # this command: lib + merge modules, tests/ + fixtures
├── crates/aeth-devkit-core/       # shared git/process/pyproject helpers
├── python/aeth_devkit/            # Python: __init__.py, scripts/, templates/
└── uv.lock
```

Build / release contract:

- `uv build` runs `cargo build --release` via maturin and produces a
  `py3-none-win_amd64` wheel containing `devkit.exe` as a console script plus the
  Python package and templates. Downstream `uv sync` installs the wheel — no Rust
  toolchain needed downstream.
- `[project].version` in `pyproject.toml` stays the single version; `release.sh` is
  unchanged apart from keeping `Cargo.toml`'s version in step.
- Wheels are platform-specific. Windows-only is sufficient today because `aeth-devkit` is a
  `dev`-group dependency and never installed in Docker images. Linux wheels, if ever
  needed, come from `maturin build --target x86_64-unknown-linux-musl --zig`.
- An editable `../aeth_devkit` source in a downstream project triggers `maturin develop`
  (a compile) on `uv sync`; only machines doing that need the toolchain.
- Templates are read at runtime from the installed Python package
  (`devkit setup-project` locates them via `python -c "import aeth_devkit.templates"` or an explicit
  `--templates-dir`), so editing a template never requires a rebuild.

Toolchain (dev machines): MSVC Build Tools (C++ workload), `rustup` stable, `maturin`
via `uv tool`, VS Code `rust-analyzer` + `vadimcn.vscode-lldb`. Shared
`CARGO_TARGET_DIR` on D: to keep `target/` out of every repo.

## Implementation notes

- Crates: `clap` (CLI), `toml_edit` (pyproject), `serde_json` + a small JSONC
  comment/trailing-comma stripper (VS Code files), `anyhow` (errors). No `tomlkit`.
- Templates are package data under `python/aeth_devkit/templates/`.
- Tests: Rust unit/integration tests (`cargo test`) cover each merge mode against fixture
  files copied from the current projects, plus an idempotency test (apply twice → second
  diff empty). CI's wheel job confirms the installed wheel exposes a working
  `devkit --version` and `devkit setup-project --help`.
- Sequence: (1) toolchain + hello-world build, (2) maturin skeleton in `aeth_devkit` with a
  stub binary, verify `uv build` → install → `devkit --version` works end-to-end,
  (3) merge logic per the spec.
