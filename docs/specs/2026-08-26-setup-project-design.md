# `poe setup-project` — design spec

Status: DRAFT v2 (template-driven). Open questions at the bottom.

## Purpose

A one-and-done, idempotent script that standardizes a project's configuration from a set
of **templates shipped inside `poe_tasks`**. Two goals:

1. All generated clutter lands in `<project_root>/.cache/`.
2. Tool/editor/repo configuration travels *with* each repository (no more `extends`
   chains to parent directories), while still being maintained in exactly one place:
   the templates. To change a convention, edit the template and re-run
   `poe setup-project` in each project.

Run once per project; re-run only when something drifts or a template changes.

Explicitly **not** responsible for: cleaning existing stragglers (`poe clean`), Docker or
deployment files, moving `dist/`, or deleting `.cache/`.

## Invocation

```
poe setup-project [--check] [--dry-run]
```

- Implemented as `src/poe_tasks/scripts/setup_project.py` (Python, stdlib + `tomlkit`).
- Runs against cwd (must contain `pyproject.toml`).
- `--dry-run`: print changes, write nothing. `--check`: dry-run + exit 1 if anything would change.
- Prints one line per changed file; silent for unchanged files. Second run = no output.

## Templates

Live in `src/poe_tasks/templates/` and ship with the wheel:

| Template                   | Target                       | Merge mode |
| -------------------------- | ---------------------------- | ---------- |
| `pyproject.toml`           | `pyproject.toml`             | TOML deep-merge |
| `vscode/settings.json`     | `.vscode/settings.json`      | JSON deep-merge |
| `vscode/launch.json`       | `.vscode/launch.json`        | create-if-missing + per-config env patch |
| `vscode/extensions.json`   | `.vscode/extensions.json`    | JSON deep-merge (`recommendations` list-union) |
| `gitignore`                | `.gitignore`                 | line-union |
| `dockerignore`             | `.dockerignore`              | line-union, only if `docker/` or `Dockerfile*` exists |
| `gitattributes`            | `.gitattributes`             | line-union |
| `env`                      | `.env` + other referenced env files | key upsert |

Templates may use `{placeholders}` resolved at runtime:

| Placeholder          | Value |
| -------------------- | ----- |
| `{project_root}`     | absolute path, native separators |
| `{package}`          | import name: sole directory under `src/` if unambiguous, else `project.name` with `-`→`_` |
| `{latest_poe_tasks}` | latest stable `poe-tasks` on SFTPyPI (same lookup as `lock.sh`) |

### Merge modes

**TOML deep-merge** (`pyproject.toml`): walk the template; tables merge recursively,
every leaf (scalar *or* array) present in the template **overwrites** the project value;
keys absent from the template are left untouched (project name, version, deps,
`tool.docker`, `tool.uv.sources` for other packages, …). Comments/formatting preserved via
`tomlkit`. Special rules:
- Arrays flagged *union* (see open Q1) are unioned, not replaced.
- `tool.ruff.extend` and `tool.pyright.extends` are **removed** from the project once the
  template has inlined the full config (that is the whole point).
- Array-of-tables `[[tool.uv.index]]`: ensure an entry with `name = "SFTPyPI"` exists and
  matches the template; other index entries untouched.
- `dependency-groups.dev`: ensure an entry for each template dep by package name; a
  matching existing entry (any specifier) is replaced with the template's specifier.

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

**line-union** (`.gitignore`, `.dockerignore`, `.gitattributes`): create from template if
absent; otherwise append each template line not already present (comparison ignores
surrounding whitespace and treats `.cache` ≡ `.cache/`). Never removes or reorders.

**env upsert**: for each `KEY=value` in the `env` template, replace the existing `KEY=`
line in place or append; all other lines preserved byte-for-byte; line endings preserved;
file created if missing. Applied to `.env` and to every distinct `envFile` referenced in
`launch.json` (e.g. `testing.env`).

## Template contents (v1)

### `pyproject.toml`
- `[dependency-groups].dev`: `poe-tasks>={latest_poe_tasks}`, `poethepoet>=0.46.0`, `pyright>=1.1.411`
- `[tool.poe].include_script = [{ script = "poe_tasks:tasks", executor = { type = "uv", frozen = true } }]`
- `[[tool.uv.index]]` SFTPyPI block; `[tool.uv.sources].poe-tasks = [{ index = "SFTPyPI" }]`
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
- gitignore: standard GitHub Python template + `.cache/`, `.env`, `*.env`, `persisted_data/`.
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
git-tracked-file reports; no `.python-version` or README scaffolding; no
`requires-python`/build-system enforcement.

## Open questions
1. Array merge policy for `pytest.addopts`, `ruff.lint.extend-select`, `ruff.lint.ignore`,
   `ruff.exclude`: replace from template, or union with project values?
2. Per-project opt-out: honour a `[tool.setup-project].keep = ["tool.pyright.reportMissingTypeStubs", ...]`
   list of dotted keys the merge must not overwrite (aeth_ext currently flips two pyright
   flags), or is "template always wins, edit the project after" acceptable?
