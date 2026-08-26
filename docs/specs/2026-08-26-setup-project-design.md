# `poe setup-project` — design spec

Status: DRAFT (open questions at the bottom)

## Purpose

A one-and-done, idempotent script that standardizes a project's configuration so all
generated clutter lands in `<project_root>/.cache/`, and (later) bootstraps the rest of
the SFT project conventions. Run once per project; re-run only if something drifts.

Explicitly **not** responsible for: cleaning up existing stragglers (that's `poe clean`),
touching Docker/deployment files, or moving `dist/`.

## Invocation

```
poe setup-project [--check] [--dry-run]
```

- Shipped in `poe_tasks` as `src/poe_tasks/scripts/setup_project.py`, exposed as a poe task.
- Runs against the cwd project root (where `pyproject.toml` lives).
- `--dry-run`: print the changes that would be made, write nothing.
- `--check`: like dry-run, but exit non-zero if anything would change (drift detection / CI).
- Every change is printed as `<file>: <what changed>`; unchanged files print nothing.

## Constants

| Name                | Value                                     |
| ------------------- | ----------------------------------------- |
| `CACHE_DIR`         | `.cache`                                  |
| `PYCACHE_DIR`       | `.cache/pycache`                          |
| `PYCACHE_ABS`       | `<project_root>\.cache\pycache` (native separators, resolved at runtime) |
| `PYCACHE_VSCODE`    | `${workspaceFolder}/.cache/pycache`       |

## Changes made

### 1. `pyproject.toml` (edited with `tomlkit`, preserving comments/formatting)

Set (create tables if missing, override if present — overriding the parent
`../pyproject.toml` that ruff/pyright `extend` is fine):

| Key                                   | Value                    | Condition |
| ------------------------------------- | ------------------------ | --------- |
| `tool.pytest.ini_options.cache_dir`   | `".cache/pytest"`        | always |
| `tool.ruff.cache-dir`                 | `".cache/ruff"`          | always |
| `tool.coverage.run.data_file`         | `".cache/.coverage"`     | always |
| `tool.coverage.html.directory`        | `".cache/htmlcov"`       | always |
| `tool.coverage.xml.output`            | `".cache/coverage.xml"`  | always |
| `tool.coverage.lcov.output`           | `".cache/coverage.lcov"` | always |
| `tool.mypy.cache_dir`                 | `".cache/mypy"`          | only if `mypy` appears in any dependency list |

`[tool.poe.tasks.clean]` / `poe clean` is **not** modified and must never delete `.cache/`.

### 2. `.env` (project root)

- Upsert `PYTHONPYCACHEPREFIX="<PYCACHE_ABS>"`, replacing an existing line in place
  (preserving position) or appending; create the file if missing.
- All other lines (secrets, comments, ordering) are preserved byte-for-byte.
- Backslashes are written literally; poethepoet's parser and VS Code's envFile reader
  both keep them as-is inside double quotes (verified).

### 3. Other env files referenced by VS Code

For every `envFile` value in `.vscode/launch.json` that is not `${workspaceFolder}/.env`
(e.g. `testing.env`): apply the same upsert as (2). If the referenced file does not
exist, create it with just the one variable.

### 4. `.vscode/launch.json`

Facts: debugpy launch configs do **not** load `.env` unless `envFile` is set, and `env`
entries override `envFile`.

For each configuration with `"type": "debugpy"` (or legacy `"python"`) and
`"request": "launch"`:
- Ensure `envFile` exists; if absent set `"${workspaceFolder}/.env"`. Existing values are kept.
- Ensure `env.PYTHONPYCACHEPREFIX = PYCACHE_VSCODE` (create `env` if missing; override
  any other value, e.g. the old `.cache/__pycache__` spelling).
- Other config types (`PowerShell`, `attach`, compounds) are untouched.
- If the file does not exist, do nothing (no invented launch configs — a later
  "project bootstrap" phase may add a default one; see open questions).

### 5. `.vscode/tasks.json`

For each task: ensure `options.env.PYTHONPYCACHEPREFIX = PYCACHE_VSCODE`. Skip if file absent.

### 6. `.vscode/settings.json` (create if missing)

```jsonc
"python.envFile": "${workspaceFolder}/.env",
"terminal.integrated.env.windows": { "PYTHONPYCACHEPREFIX": "${workspaceFolder}\.cache\pycache" },
"terminal.integrated.env.linux":   { "PYTHONPYCACHEPREFIX": "${workspaceFolder}/.cache/pycache" },
"terminal.integrated.env.osx":     { "PYTHONPYCACHEPREFIX": "${workspaceFolder}/.cache/pycache" },
"files.exclude":        { ".cache": true, ".venv": true, "**/__pycache__": true },
"search.exclude":       { ".cache": true, ".venv": true, "**/__pycache__": true },
"files.watcherExclude": { "**/.cache/**": true, "**/.venv/**": true, "**/__pycache__/**": true }
```

Merge semantics: keys are set/overridden individually; existing unrelated keys and
existing entries inside the exclude maps are preserved.

Rationale for the terminal env: `python.envFile` only feeds extension-spawned
processes; `poe`/`uv run` typed in an integrated terminal would otherwise write
`__pycache__` next to sources. This is a straggler-catcher, **in addition to** `.env`.

### 7. `.gitignore`

Ensure a `.cache/` line exists (accept existing `.cache` without slash as satisfying it).
Legacy entries (`.pytest_cache`, `.coverage`, `__pycache__/`, …) are left alone.

### 8. `.gitattributes`

Ensure these lines exist (append if missing, create file if absent):

```
* text=auto eol=lf
*.sh text eol=lf
```

## JSONC handling for `.vscode/*.json`

VS Code files may contain `//` comments and trailing commas. Approach:
- Preserve the leading comment block (lines before the first `{`) verbatim.
- Strip remaining comments/trailing commas, `json.load`, mutate, re-dump with 4-space
  indent. Inline comments elsewhere in the file are lost — acceptable; across the
  seven current projects only the boilerplate header comment exists.

## Idempotency / safety

- Running twice yields no changes on the second run.
- Never deletes files or directories.
- Never writes secrets; only touches the single `PYTHONPYCACHEPREFIX` line in env files.
- `.env` is written with the file's existing line-ending style.

## Dependencies added to `poe_tasks`

- `tomlkit` (runtime dep).

## Denied / out of scope (decided 2026-08-26)

- `poe clean` nuking `.cache/` — no; the cache should persist.
- `dist/` relocation — no.
- Docker/compose/Dockerfile edits — no; deployment env is intentional.
- Straggler report and git-tracked-cache-file report — no; not this script's job.

## Open questions (project-bootstrap scope)

See the accompanying decision list.
