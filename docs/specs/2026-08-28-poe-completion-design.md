# `devkit complete` — fast shell completion for poe tasks

Status: **APPROVED 2026-08-28.**

Implementation note: by explicit request there is **no separate implementation plan**; this
spec is written to be implemented directly.

Independent of `2026-08-28-agent-config-design.md` — different trigger, different crate, no
shared code. Either can be built first.

## Problem

Every Tab press against `poe` costs ~203 ms. That is squarely in the range where completion
feels sticky rather than instant.

The cost is not the work; it is three layers of process startup. The generated PowerShell
script (`poe _powershell_completion`, lines 87–118) calls `& poe _list_tasks` and
`& poe _describe_task_args` live on each completion. Each of those starts Python, imports the
poethepoet framework, and then — because this project uses
`include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]`
(`pyproject.toml:55`) — spawns a *second* process through `uv run` to obtain the task table.

Measured on this machine, 10 runs averaged (single runs are unusable: Windows Defender's
first-touch scan dominated an initial reading by 10x):

| path | per call |
| --- | --- |
| `poe _list_tasks` (today) | 203 ms |
| poe's own include_script one-liner, run directly against `.venv/Scripts/python.exe` | 57 ms |
| `python -c pass` | 33 ms |
| `devkit --version` (Rust floor) | 16 ms |

## What poe's resolution actually does

Read before designing, because it determines what can and cannot be mirrored. There are two
mechanisms and they are not alike.

**`include` (TOML files)** — plain config. `Config._load_included_config` resolves the path,
loads the file, merges it, recurses into child includes when `recursive` is not false, and
guards against cycles by tracking ancestors. Straightforwardly mirrorable in Rust.

**`include_script` — not mirrorable.** `Config._load_include_script` builds a Python source
string and executes it:

```python
script = ("import os,sys,json;environ=os.environ;"
          "from importlib import import_module as _i;"
          f"{src_path_append}"
          "_o=sys.stdout;sys.stdout=sys.stderr;"
          f"_m = _i('{target_module}');"
          "sys.stdout=_o;"
          f"print(json.dumps(_m.{function_call.expression}));")
subproc = await executor.execute(("python", "-c", script))
```

There is no static declaration to mirror. The task table *is* the return value of a Python
program: `python/aeth_devkit/__init__.py` builds a `TaskCollection` at import time and
interpolates absolute paths through `_script_path()`. Reimplementing that "logically" in Rust
would mean reimplementing `aeth_devkit/__init__.py` in Rust and keeping the two in lockstep
forever — the drift trap this toolkit exists to avoid.

What the script *returns*, however, is plain JSON containing everything completion needs:

```json
{"env": {}, "envfile": [], "config_path": "...", "tasks": {"release": {"help": "...", ...}, ...}}
```

So the design splits along that line: Rust owns resolution, and Python is demoted from a
framework dependency to a cached data source.

## Design

New workspace member `crates/aeth-devkit-complete`, wired into
`crates/aeth-devkit/src/main.rs` as `Command::Complete(aeth_devkit_complete::Args)`.

### 1. Rust resolves the TOML layers

Parse `[tool.poe.tasks]` from `pyproject.toml` with `toml_edit` (already a workspace
dependency), then walk the `include` chain: resolve each path relative to its source config
dir, load, merge, recurse when `recursive` is not false, and skip a path already in the
ancestor set (cycle guard), mirroring `_load_included_config`.

Most projects in the fleet stop here — no Python process at all, ~16 ms.

### 2. `include_script` runs poe's own one-liner, without poe or uv

When `[tool.poe] include_script` is present, build the same script string poe builds and run
it against the resolved interpreter directly:

1. `<root>/.venv/Scripts/python.exe` (Windows)
2. `<root>/.venv/bin/python` (Unix)
3. `python` on PATH

This skips the poethepoet framework import *and* the `uv run` executor, which is where the
203 ms goes: 57 ms measured for the same JSON.

Parse the result per poe's own normalization in `_load_include_script`: a `tool.poe` key, or
a flat `tool.poe` string key, or otherwise treat the whole object as the poe config body. Pop
`config_path` before merging.

Failure — interpreter missing, non-zero exit, unparseable JSON — degrades to the TOML-only
task list rather than erroring. A completer that prints a stack trace into the user's prompt
is worse than one that offers a short list.

### 3. Cache

Store the resolved table at `.cache/devkit-completions.json` alongside a fingerprint:

- `pyproject.toml` mtime and size
- mtime and size of every included TOML file
- mtime and size of the `include_script` target module resolved from `config_path` in the
  script output
- the devkit version string

Warm path — essentially every Tab press — is a fingerprint check plus a JSON read: ~16 ms.
Cold path, once after editing `pyproject.toml` or reinstalling devkit: ~73 ms.

**Stated limitation.** A project whose `include_script` returns *dynamic* tasks (reading env
vars, a database, the clock) will see a stale cache, because the fingerprint is file-based.
Nothing in the fleet does this today. `--no-cache` forces regeneration. This is a real
behavioural difference from poe and is documented rather than hidden.

### 4. Subcommands

| command | output |
| --- | --- |
| `devkit complete tasks [dir]` | task names on one space-separated line — replaces `poe _list_tasks` |
| `devkit complete args <task> [dir]` | that task's arguments, tab-separated — replaces `poe _describe_task_args` |
| `devkit complete script --powershell` | a completion script to add to `$PROFILE` (the default shell) |
| `devkit complete script --bash` | a completion script to source from `.bashrc` |
| `devkit complete install --powershell` | put the line in `$PROFILE`, removing poe's own registration |
| `devkit complete install --bash` | write the script to the bash completion file(s) |
| `devkit complete --no-cache …` | global modifier: bypass and rewrite the cache |

An empty `[dir]` argument — which the scripts pass when no `-C` was given — means the
current directory. Any failure prints nothing and exits 0: a completer that errors breaks
the shell.

The emitted scripts are poe's generated ones, transformed by
`crates/aeth-devkit-complete/gen_scripts.py` into `src/scripts.rs` (re-run it when
poethepoet's generator changes) — same global option list, same
option-exclusion behaviour, same `-C`/`--directory` target-path handling — with the two
`& poe _*` invocations swapped for `& devkit complete *`. They register against the `poe`
command name so they replace poe's registration in the user's profile.

### 5. `install`

Both flags may be combined; at least one is required. `--dry-run` reports without writing.
Every run is idempotent and prints exactly what it changed.

**PowerShell** — one line in `$PROFILE`, whose path is obtained by asking PowerShell
(`powershell -NoProfile -Command $PROFILE`), not by guessing the Documents folder:
`devkit complete script --powershell | Out-String | Invoke-Expression`. poe's own
`poe _powershell_completion | …` line is removed: both scripts register for the `poe`
command and the last one loaded wins, so keeping poe's would only pay its ~200 ms Python
start at every shell launch for nothing.

**bash** — a *file*, following poe's own documented install, written to both:

- `~/bash_completion.d/poe.bash` — the only user hook Git Bash has
  (`/etc/profile.d/git-prompt.sh` sources `$HOME/bash_completion.d/*.bash`). Git Bash ships
  no `bash-completion` package, which is why a file at the standard location below was
  installed on this machine on 2026-07-05 and never loaded.
- `~/.local/share/bash-completion/completions/poe` — the standard location the
  `bash-completion` package reads on Linux.

An existing file is overwritten only if its header marks it as generated (`Generated by
poethepoet`, or devkit's own `adapted by aeth-devkit`); anything else is a person's and is
left alone with a note. A file install is a snapshot: re-run `install` after a devkit
upgrade that changes the script.

**Preflight.** The profile line and the scripts call bare `devkit`, so it must resolve in a
fresh shell the way `poe` does (a `uv tool install`). `install` refuses when `devkit` is not
on PATH, printing `uv tool install aeth-devkit --index <url>` as the fix, and warns (but
proceeds) when the only `devkit` found lives inside a `.venv`, since that is on PATH only
while the venv is activated.

## Result

Measured on 2026-08-29 against this repo (which has an `include_script`), debug build,
10 runs averaged:

| path | per call |
| --- | --- |
| `poe _list_tasks` | 198 ms |
| `devkit complete --no-cache tasks` (cold: runs the Python one-liner) | 62 ms |
| `devkit complete tasks` (warm: fingerprint check + cache read) | 13 ms |

**198 ms → 13 ms warm, 62 ms cold** — 15x on the path every Tab press takes — and a project
with no `include_script` never starts Python at all.

Output parity, verified: `devkit complete tasks` prints the same task list as
`poe _list_tasks`, and `devkit complete args lock` matches `poe _describe_task_args lock`
byte-for-byte except that poe emits CRLF on Windows and devkit emits LF. LF is the safer
choice: the bash script's `read -r` would otherwise keep a stray carriage return in the last
field.
The parity test in `tests/format_cache.rs` runs against the real venv `poe` and skips (with
a note) when it is absent.

## Testing

- Resolution unit tests: `[tool.poe.tasks]` only; a single `include`; nested includes; a
  cyclic include terminating instead of hanging; `recursive = false` honoured.
- `include_script` tests against a fixture module: correct JSON parsed; each of poe's three
  config-shape normalizations; non-zero exit falls back to the TOML-only list; unparseable
  output falls back rather than erroring.
- Cache tests: cold populates; warm hits; a touched `pyproject.toml` invalidates; a touched
  include target invalidates; a changed devkit version invalidates; `--no-cache` bypasses.
- A parity test asserting `devkit complete tasks` returns the same set as `poe _list_tasks`
  for this repo — the one guard against the Rust resolver silently diverging from poe.

## Out of scope

- Completing task *values* (file paths, choices) — poe does not do this either.
- zsh/fish scripts. Add them when someone needs one.
- Replacing `poe` itself. This changes only how completion obtains its data; running a task
  still goes through poe.
