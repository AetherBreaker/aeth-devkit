# Extract devkit-templates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every template under `python/aeth_devkit/templates/` into its own repository, `AetherBreaker/devkit-templates`, published as a pure-Python wheel on SFTPyPI whose only content is a `devkit_templates` package carrying the templates as package data; make `devkit setup-project` read templates from the project's own environment, installing `devkit-templates` into a project that lacks it before anything renders; remove `--check`; give the templates repository a CI that renders its working tree through released devkits. Step 4 of the split spec.

**Architecture:** The directory leaves by `git filter-repo` with its history, renamed to `python/devkit_templates/templates/`, and is packaged with `uv_build` (no code: an empty `__init__.py` and the files). In `aeth-devkit`, `run_with` gains a step 0 that resolves the templates directory: an override (`--templates-dir`, `DEVKIT_TEMPLATES`, or a new `[tool.devkit].templates-dir`) renders a working tree; otherwise a bootstrap adds `devkit-templates` to the dev group by its constant name when absent, advances it under the `aeth-devkit==<running>` constraint (the existing package step, now callable for one package), syncs, and reads from the venv's `devkit_templates`. The bundled directory and both `locate` fallbacks go; the setup crate's end-to-end tests render a fixture snapshot of the templates instead. `--check` and its exit-1 paths go. `devkit lock` learns to move the dev-group pin rather than a `[project].dependencies` one, so the templates package's compatibility floor (`aeth-devkit>=X`, a runtime dependency by spec 4.2) is raised only by hand. The devkit release is a major, 14.0.0.

**Tech Stack:** Rust 2024 (`toml_edit` 0.25, clap 4), `uv_build` 0.11, uv 0.11+, GitHub Actions, `gh`, `git filter-repo` via `uvx`, Python 3.14.

**Spec:** `docs/superpowers/specs/2026-09-08-devkit-split-design.md`, sections 2, 3, 4.0, 4.2, 4.5, 5, 6, 7 (step 4 and "Repository creation") and 9. Read it first. The three previous plans (`2026-09-08-extract-devkit-container.md`, `2026-09-09-extract-devkit-vscode.md`, `2026-09-09-extract-hooks-and-completion.md`, each with an Execution notes section) are the recipe this plan repeats.

## Global Constraints

- Distribution name `devkit-templates`; import name `devkit_templates`; repository `AetherBreaker/devkit-templates`, public, default branch `main`, version `1.0.0` at creation; cloned to `D:\SFT Software Projects\SFT Workspace\devkit-templates`.
- The package has no code. `python/devkit_templates/__init__.py` is empty; every template lives under `python/devkit_templates/templates/` at its current relative path (`vscode/…`, `docker/…`, `github/workflows/…`, `claude/…`). `template.Dockerfile` is not among them: it lives in `devkit-container` since step 1.
- Build backend `uv_build`: `[build-system] requires = ["uv_build>=0.11,<0.12"]`, `build-backend = "uv_build"`, `[tool.uv.build-backend] module-name = "devkit_templates"`, `module-root = "python"`. `uv_build` ships every file under the module directory (verified in its docs: "all data files must either be under the module root or in the appropriate data directory"), so there is no manifest. Task 2 verifies the wheel listing.
- The package's compatibility floor is `[project].dependencies = ["aeth-devkit>=13.0.0"]` (spec 4.2: "an ordinary dependency"; a dependency group would not reach the wheel's metadata). It is raised by hand in the commit that first uses a newer template-language feature. No template content changes in this step, so `>=13.0.0` is true: the 13.0.0 engine renders these files.
- `devkit-templates` copies `aeth-devkit`'s `[[tool.uv.index]]` block verbatim (`publish-url` included) and is released with `devkit release` through the non-Rust release workflow template (`uv build`, wheel and sdist).
- Publication order (spec 7): `devkit-templates==1.0.0` is on SFTPyPI before the devkit release whose bootstrap names it. That release is a **major**, `14.0.0`: `--check` is removed, and a project on 14.0.0 renders nothing until `devkit-templates` is in its environment (the bootstrap does that in the first `setup-project` run). The release note says: run `poe lock`, then `poe setup-project`, in every project.
- Override precedence for the templates directory: `--templates-dir`, then `DEVKIT_TEMPLATES`, then `[tool.devkit].templates-dir` (relative to the project root), then the venv's `devkit_templates`. Each override must be an existing directory. An override skips the bootstrap entirely.
- A dry run never installs anything: on a project whose environment lacks `devkit-templates` it exits 2 with a message saying a plain run adds it. `--dry-run` otherwise exits 0; `problem:` lines are findings for a hand edit, not an exit code. Idempotence is the acceptance test (a second plain run reports nothing to do).
- The project's own name is never added as a package (spec 4.0): `devkit-templates` renders its own tree through `[tool.devkit].templates-dir` and depends on itself nowhere.
- `.env` holds live credentials: never print it, never commit it. The SFTPyPI secrets go onto the new repository with `gh secret set` piped from `aeth_devkit/.env`; `.env` is copied into the clone. The index is anonymously readable (aeth-devkit's CI `uv sync` pulls the hooks and completion wheels with no credentials), so CI needs no secrets to render.
- `setup-project` runs headless with `-y`; the executor runs every `setup-project` and every `uv` command (`uv lock` included) itself. Nothing is handed to the user.
- Consumers (`aeth_ext`, `IMAPReportCollector`, `ScheduledInvoiceProcessor`, `ScheduledReportAggregator`, `timeclock_entry_processor`, any other) are **not** migrated by this plan: the user's standing instruction is that consumers move once every step of the spec is done. Part C touches only `aeth_devkit`, `devkit-container`, `devkit-vscode`, `devkit-claude-hooks`, `devkit-poe-complete` and `devkit-templates`.
- `aeth-devkit` conventions (`AGENTS.md`): Conventional Commits with the trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`; a `fix` body states bug, cause and fix; comments carry reasoning densely; no single-use helpers of four lines or fewer; tests carry no docstrings and never define intent; on a feature branch run only the named tests while iterating and the full suite once at the end. PRs are rebase-merged. Keep `REMOVAL-CANDIDATES.md` in the repo root current with anything found along the way that no longer pays rent.

## Context for the executor

### Where things are

- Workspace: `D:\SFT Software Projects\SFT Workspace` (Git Bash `/d/SFT Software Projects/SFT Workspace`), `$WS` below, holding `aeth_devkit` (underscore), `devkit-container`, `devkit-vscode`, `devkit-claude-hooks`, `devkit-poe-complete`.
- `aeth_devkit` `main`: version `13.0.0` released, every devkit repo on it. The bundled templates are at `python/aeth_devkit/templates/` (22 files; `git ls-files python/aeth_devkit/templates`), and 13.0.0 reads them from beside its binary, which is what lets 13.0.0 set the new repository up in Part A.
- The template machinery, all in `crates/aeth-devkit-setup/src/`:
  - `templates.rs`: `locate(explicit)` (override, `DEVKIT_TEMPLATES`, the `aeth_devkit` package beside the binary via `packages::probe(py, &DEVKIT)` joined with `templates`, then the source tree), `load`, `load_optional`, `substitute`, `gate`, `template_file_name`, `hook_bin`, `existing_dir`.
  - `packages.rs`: `DevkitPackage`, `CONTAINER`/`HOOKS`/`COMPLETE`/`DEVKIT`, `active(ctx)`, `Installed`, `Venv`/`SystemVenv`/`StubVenv`, `environment(root)`, `probe(python, package)`, `locked_version`, `locked_registry_version`, `latest_requested(template)`, `advance(ctx, deps, dry_run, latest, changes)`, `resync_after_replay`, `RUNNING_DEVKIT`. `advance` locks every package in `active(ctx)` with one `uv lock --upgrade-package …` call, writes the `>=<locked>` floors for the names in `latest` (a bare requirement becomes `name>=<locked>`), re-locks after writing floors, syncs when the lock moved or the venv lags, and records `uv.lock`. Its first check refuses a lock whose `aeth-devkit` entry is not the running version (a `problem:` on a dry run).
  - `context.rs`: `ProjectContext` (`root`, `name`, `has_rust`, `has_docker`, `devkit_index`, `release_workflow`, …), `DEVKIT_KEYS = ["release-workflow"]` with the unknown-key refusal, `find_package_in` (a package dir is one with `__init__.py`; `python/` wins over `src/`).
  - `lib.rs`: `run_with(ctx, templates_dir: &Path, dry_run, deps)`, the numbered steps 1 (pyproject merge), 1b (`packages::advance`), 2–15. `templates_dir` is passed into every `templates::load` and into `docker::apply`/`scaffold::load`. There is no other `run`.
  - `cli.rs`: `Args` (`root`, `templates_dir`, `dry_run`, `check`, `no_commit`, `yes`, `vscode`, `no_vscode`), `run_reject_headless`, `run` (calls `templates::locate` first, then discovers `ctx`, stages bases when committing, applies through a closure that builds `Deps` and calls `run_with`).
  - `changes.rs`: `Changes::record`/`record_optional` (write unless dry run; one entry per path, details merged), `notes`, `warnings`, `problems`, `venv_synced`.
- `crates/aeth-devkit-lock/src/lib.rs`: `bump_pin(doc, pkg, index)` rewrites the first requirement `pyproject::find_requirement` returns, whose table order is `project.dependencies`, `project.optional-dependencies.*`, `dependency-groups.*` (`crates/aeth-devkit-core/src/pyproject.rs`, `requirement_tables`).
- Tests that read the real templates: `crates/aeth-devkit-setup/tests/apply.rs` and `tests/docker.rs`, each through a `templates()` helper returning `CARGO_MANIFEST_DIR/../../python/aeth_devkit/templates`; `tests/packages.rs` uses inline pyproject strings, pre-written locks and `tests/fixtures/docker/` (no template dir); the unit tests in `toml_merge.rs`, `json_merge.rs`, `lines.rs`, `md_block.rs`, `scaffold.rs` use inline templates. `packages.rs`'s `probe` unit test asserts `found.dir.join("templates").is_dir()` for the `aeth_devkit` package.
- `--check` surface: `Args.check`; `run_reject_headless`'s exemption; in `run`: `dry_run = args.dry_run || args.check`, the VS Code skip condition, `Ok(if args.check { 1 } else { SUCCESS })` in the nothing-to-write branch, `if args.check { return Ok(ExitCode::from(1)) }` after the report; comments in `changes.rs` (two), `docker/mod.rs` (`Mode::DryRun` doc, the include-only warning), `packages.rs` (the lock-mismatch comment), `lib.rs` (`keep_previews`); tests in `tests/apply.rs` (`a_run_without_standard_input_is_refused_unless_nothing_will_be_asked`, `check_fails_on_a_compose_file_the_engine_cannot_edit`, the `Args` literals); comments in `tests/docker.rs`; `README.md` (the flags paragraph, the Docker bullets, the VS Code section); `python/aeth_devkit/_tasks_source.py` help text (regenerated into `_tasks_generated.py` by `cargo build -p aeth-devkit`).
- CI (`.github/workflows/ci.yml`): `rust` (no Python; `cargo fmt/clippy/test --workspace` on both OSes), `tests` (`uv sync`, `pytest`, the baked-table check), `wheel` (`maturin develop`, `devkit --version`, `setup-project --help`, `poe lock --dry-run`). The `rust` job stays venv-less: the setup tests render a fixture snapshot.
- Placeholders the engine substitutes (`substitute`): `{project_root}`, `{package}`, `{python_dir}`, `{hook_bin}`, `{publish_index}`, `{publish_index_key}`, `{devkit_index}`, `{git_repo}`; the scaffold fills `{service}` and `{git_tag}` (falling back to `v<version>` with a note when there is no GitHub origin); the merger and package step handle `{latest}`. The only other `{…}` text in the templates is `${file}` in `vscode/launch.template.jsonc`, VS Code's own variable; the CI scan below excludes `$`-prefixed braces for that reason.

### Tools the plan assumes

`gh` authenticated as `AetherBreaker`, `git`, `uv` 0.11.16, `uvx`, Rust stable with `rustfmt` and `clippy`, Python 3.14 via uv. No Docker.

### Lessons from steps 1–3 that apply

- `git filter-repo` under Git Bash needs `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'` or the `old:new` rename is mangled. It re-points every tag and turns the clone's remote-tracking branches into local ones (`feat/release-watch-repaint` appeared in step 3): delete all tags and every branch but `main` before the first push. Windows checkouts are CRLF: write `.gitattributes`, `git add --renormalize .`, commit, then re-checkout so the working tree is LF.
- The GitHub MCP `create_repository` tool was refused by the classifier in step 3; `gh repo create AetherBreaker/<name> --public --description "…"` (no `--push`) then `git remote add origin` and `git push -u origin main` worked.
- Write multi-line files with the Write tool, and edit scripts as Python files in the scratchpad run with `python "$S/x.py"`; inline Python in a Bash heredoc breaks on backslash escapes. Read and write with `encoding="utf-8", newline="\n"`.
- `poe release` words cannot start with a dash (clap reads them as flags) and an apostrophe is mangled by poe's re-quoting: word release notes plainly.
- `uv run --env-file .env` warns that it cannot parse the `PYTHONPYCACHEPREFIX` line `template.env` writes (spaces, unquoted); the rest of the file loads. Cosmetic; a TODO in aeth-devkit that Task 8 moves to the templates repository.
- In a satellite the venv devkit runs `setup-project`; credentials come from `.env` through poe, or through `uv run --env-file .env` when calling `devkit` directly (no poe tasks before the first run).
- Reviews: a fresh reviewer after each task and two independent extra-high-effort reviews of the PR (one on this model, one on another) found real defects in every previous step. Do the same.

### Decisions taken by this plan (the user may override before execution)

1. **`uv_build`.** The repo is uv-managed and has no code; `uv_build` needs no include list for package data under the module root, and the non-Rust release workflow already runs `uv build`.
2. **Templates under `devkit_templates/templates/`**, so the venv probe (`probe` → the package directory) is joined with `templates` exactly as `locate` joins the bundled copy today, and `__init__.py` sits beside them rather than among them.
3. **A bootstrap that reuses `advance`.** `advance` gains a `packages` parameter (which packages to lock and floor); step 0 calls it for `TEMPLATES` alone, after inserting a bare `devkit-templates` requirement and its `[tool.uv.sources]` entry when the project lacks them; step 1b calls it for `active(ctx)` (hooks, completion, container). The pyproject template does **not** list `devkit-templates`: the bootstrap owns that entry and its floor, so the template's union never has to reconcile with it and no templates release is needed for the entry to appear. Cost: two `uv lock --upgrade-package` calls per plain run (one more index round trip) instead of one.
4. **`[tool.devkit].templates-dir`** joins the override sources, third in precedence. It is how the templates repository renders its own tree under 14.0.0 (its environment never holds `devkit_templates`, by the self-dependency rule), and it is a project setting, so it belongs in the table spec 4.0 reserves. `DEVKIT_KEYS` gains it; an unknown key is still refused, so the setting is added to the templates repo only after 14.0.0 is in its environment (Part C).
5. **A dry run without the package is an error**, not a note: a note followed by "Nothing to do" would read as a clean project. Exit 2 with the remedy.
6. **A fixture snapshot for the setup crate's tests** (`git mv python/aeth_devkit/templates crates/aeth-devkit-setup/tests/fixtures/templates`, so blame survives). The engine's tests are hermetic and the `rust` CI job keeps no venv; the released templates are rendered through the working-tree binary by a new `render` CI job (dry run, so no `aeth-devkit==<unreleased version>` constraint is ever resolved). Template content evolves in its own repository, whose CI renders it through released devkits.
7. **`devkit lock` prefers a dependency-group requirement** over a `[project].dependencies` one. Only the templates package names `aeth-devkit` at runtime (its compatibility floor); `poe lock` must move its tooling pin, not that floor. A five-line change in core plus `bump_pin`.
8. **The templates CI does a plain run into scratch projects under `$RUNNER_TEMP`** (`-y --no-commit --no-vscode`), not `--dry-run` as spec 4.2 words it: files on disk are what a placeholder scan can read, and the package step then exercises the 4.0 constraint against the real index. Matrix: three project kinds × two devkits (the declared floor, and the newest on the index).
9. **The `aeth_devkit` render job uses a throwaway venv** for the newest `devkit-templates` (`uv venv` + `uv pip install`), not `uv sync`: the job needs no editable build of devkit, only the templates.
10. **A major, `14.0.0`.**
11. **Consumers stay put** (Global Constraints).

### The order that matters

Part A publishes `devkit-templates==1.0.0`, set up by 13.0.0. Part B changes `aeth-devkit` on one branch and releases 14.0.0; nothing in Part B touches `aeth_devkit`'s own `pyproject.toml` (its bootstrap adds the package in Part C). Part C moves the six devkit repositories; `devkit-templates` last and by a different route (Task 10 step 2).

---

## File structure

**`devkit-templates`** (Part A): `python/devkit_templates/__init__.py` (empty); `python/devkit_templates/templates/**` (moved with history); `pyproject.toml`; `.gitignore`; `.gitattributes`; `README.md`; `TODO.md`; `ci/render.sh`; `.github/workflows/ci.yml`. After its first `setup-project` run: `.github/workflows/release.yml`, `claude.yml`, `.claude/`, `.vscode/`, `AGENTS.md`, `.mcp.json`, `uv.lock`, the tooling tables in `pyproject.toml`.

**`aeth-devkit`** (Part B, branch `feat/extract-templates`):
- Move: `python/aeth_devkit/templates/` → `crates/aeth-devkit-setup/tests/fixtures/templates/`.
- `crates/aeth-devkit-setup/src/context.rs`: `templates_dir: Option<PathBuf>` from `[tool.devkit].templates-dir`.
- `crates/aeth-devkit-setup/src/templates.rs`: `override_dir(explicit, ctx)` replaces `locate`.
- `crates/aeth-devkit-setup/src/packages.rs`: `TEMPLATES`, `ensure_templates`, `add_bare_requirement`, `advance` with a `packages` parameter.
- `crates/aeth-devkit-setup/src/lib.rs`: step 0; `run_with(ctx, templates_override: Option<&Path>, …)`.
- `crates/aeth-devkit-setup/src/cli.rs`: no `--check`; the override resolved after `ctx`.
- `crates/aeth-devkit-setup/src/changes.rs`, `docker/mod.rs`: `--check` wording.
- `crates/aeth-devkit-core/src/pyproject.rs`: `find_requirement_in_groups`.
- `crates/aeth-devkit-lock/src/lib.rs`: `bump_pin` prefers the group requirement.
- Tests: `tests/apply.rs`, `tests/docker.rs`, `tests/packages.rs`, `context.rs`, `templates.rs`, `packages.rs`, the lock crate's tests.
- `.github/workflows/ci.yml`: a `render` job.
- `README.md`, `WORKSPACE.md`, `TODO.md`, `REMOVAL-CANDIDATES.md`, `python/aeth_devkit/_tasks_source.py` (+ regenerated `_tasks_generated.py`).

---

## Part A: the repository and the wheel

### Task 1: Extract `python/aeth_devkit/templates` with its history

**Files:**
- Create: `$WS/devkit-templates` (a filtered clone), `.gitattributes` and `python/devkit_templates/__init__.py` in it.

**Interfaces:**
- Produces: a local repository on `main`, no remote, no tags, no other branch, whose tree is `python/devkit_templates/templates/**` plus the empty `__init__.py`, LF throughout.

- [x] **Step 1: Confirm `aeth_devkit` is current and clean**

```bash
WS="/d/SFT Software Projects/SFT Workspace"
cd "$WS/aeth_devkit" && git switch main && git pull --ff-only && git status --short --branch | head -3 && git log --oneline -1
git ls-files python/aeth_devkit/templates | wc -l
test ! -e "$WS/devkit-templates" && echo "target absent"
```

Expected: `## main...origin/main`, a clean tree, `22`, "target absent".

- [x] **Step 2: Filter a throwaway clone down to the templates, renamed into the package**

```bash
cd "$WS" && git clone --no-local aeth_devkit devkit-templates && cd devkit-templates
MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' uvx --from git-filter-repo git-filter-repo --force \
  --path python/aeth_devkit/templates/ \
  --path-rename python/aeth_devkit/templates/:python/devkit_templates/templates/
git log --oneline | wc -l
git ls-files | grep -vc '^python/devkit_templates/templates/' ; git ls-files | wc -l
git remote -v; git tag -l | wc -l; git branch
```

Expected: many commits (templates change often; only history under this path survives, which spec 7 accepts); `0` files outside the renamed directory and `22` in it; no remote; some re-pointed tags; possibly stray branches beside `main`.

- [x] **Step 3: Tags and stray branches gone, the empty package, LF**

Write `python/devkit_templates/__init__.py` empty and `.gitattributes` with exactly:

```text
* text=auto eol=lf
*.sh text eol=lf
```

Then:

```bash
cd "$WS/devkit-templates"
git tag -l | xargs -r git tag -d >/dev/null; git tag -l | wc -l
git branch --show-current; git branch | grep -v '^\* main$' | sed 's/^..//' | xargs -r git branch -D
git add .gitattributes python/devkit_templates/__init__.py && git add --renormalize . && git commit -q -m "chore: add .gitattributes and the empty devkit_templates package

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git rm -rq --cached . && git reset -q --hard HEAD
git ls-files --eol | awk '{print $1, $2}' | sort | uniq -c
```

Expected: `0` tags; branch `main` (else `git branch -m main` first); no other branch; every file `i/lf w/lf`.

---

### Task 2: Stand `devkit-templates` up as a package with a render CI

**Files:**
- Create: `pyproject.toml`, `.gitignore`, `README.md`, `TODO.md`, `ci/render.sh`, `.github/workflows/ci.yml` in `$WS/devkit-templates`.

**Interfaces:**
- Consumes: Task 1's tree.
- Produces: a wheel `devkit_templates-1.0.0-py3-none-any.whl` carrying `devkit_templates/templates/**`; a CI that renders the working tree through the declared-floor devkit and the newest devkit into three scratch projects and fails on a render error or an unresolved placeholder.

- [x] **Step 1: `pyproject.toml`**

Copy the `[[tool.uv.index]]` block from `$WS/aeth_devkit/pyproject.toml` verbatim into the place marked below. Write:

```toml
[project]
  name            = "devkit-templates"
  version         = "1.0.0"
  description     = "The project-configuration templates devkit setup-project renders into every devkit-managed project"
  readme          = "README.md"
  requires-python = ">=3.14"
  # The compatibility floor (spec 4.2): the oldest devkit whose template language these files
  # use. Raised by hand in the commit that first uses a newer placeholder, gate, marker, file
  # or merge shape. `poe lock` moves the dev-group pin below, never this.
  dependencies = ["aeth-devkit>=13.0.0"]

[dependency-groups]
  dev = ["aeth-devkit>=13.0.0"]

[build-system]
  requires      = ["uv_build>=0.11,<0.12"]
  build-backend = "uv_build"

[tool.uv.build-backend]
  module-name = "devkit_templates"
  module-root = "python"

[tool.poe]
  include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]

[tool.uv.sources]
  aeth-devkit = [{ index = "SFTPyPI" }]

# [[tool.uv.index]] block copied from aeth-devkit here, publish-url included.
```

Do not add `[tool.devkit]`: 13.0.0 refuses a key it does not know, and 13.0.0 runs this repository's first `setup-project` (Task 3). The tooling tables (`[tool.coverage]`, `[tool.ruff]`, `[tool.pyright]`, `[tool.pytest.ini_options]`, `[tool.tombi]`) arrive from that run.

- [x] **Step 2: `.gitignore`, `README.md`, `TODO.md`**

`.gitignore` (the run's template prepends the standard rules later):

```text
/.venv/
/dist/
/.cache/
/.env
```

`README.md`:

```markdown
# devkit-templates

The project-configuration templates `devkit setup-project` renders into every
devkit-managed project: `pyproject.toml`, the VS Code files, `.gitignore`, `.gitattributes`,
`.dockerignore`, the compose scaffold, the GitHub workflows, `AGENTS.md`, the Claude
settings and `.mcp.json`. No code: a `devkit_templates` package whose only content is
`templates/`.

## How a change reaches projects

A wheel on SFTPyPI, a dev dependency of every project; `setup-project` reads the
templates from the project's own environment, and `uv.lock` is the pin. Push freely;
release (`poe release`) when a change is meant to reach projects. Nothing else moves it.

## The floor

`[project].dependencies` names `aeth-devkit>=X`: the oldest devkit whose template
language these files use. The placeholders, the `# setup-project:` line gates, the table
and value markers, the compose service block, the AGENTS.md block and the inventory of
files and merge shapes are implemented by aeth-devkit's `setup` crate. A content change
needs nothing. A change that needs a new feature of that language is an aeth-devkit change
first (add the feature, release it), then a commit here that raises the floor and uses
it. Under `setup-project`'s constraint (`aeth-devkit==<the running devkit>`) a project on
an older devkit is held at the last release its devkit accepts, with a warning; it never
renders a template it cannot understand. `poe lock` moves the dev-group pin and leaves
this floor alone.

## Editing

`poe setup-project` in this repository renders its own tree (`[tool.devkit].templates-dir`
points at it). To render a checkout of this repository into another project, pass
`--templates-dir <path to python/devkit_templates/templates>` or set `DEVKIT_TEMPLATES`.

CI (`ci/render.sh`) renders the working tree through the declared floor devkit and the
newest devkit on the index into three scratch projects (pure Python, Rust, Docker) and
fails on a render error or a `{word}` left in any rendered file. A literal `{word}` in a
template is read as an unresolved placeholder by that scan; write it another way.
```

`TODO.md`:

```markdown
# devkit-templates TODO

- [ ] `template.env` writes `PYTHONPYCACHEPREFIX` unquoted; the value has spaces and
      backslashes, which poe's envfile loader accepts but uv's `--env-file` parser rejects
      (`Failed to parse environment file .env at position 4`; the rest of the file still
      loads). Quote the value so both readers agree.
```

(That entry moves here from aeth-devkit's `TODO.md`, which Task 8 drops it from.)

- [x] **Step 3: `ci/render.sh`**

One script, runnable locally, that builds a scratch project of a kind, installs the requested devkit into it, runs a plain `setup-project` from the working-tree templates, and scans every rendered file:

```bash
#!/usr/bin/env bash
# Render this tree's templates through a released devkit into a scratch project and fail on
# a render error or a placeholder left unresolved.
#   $1  python | rust | docker      the project layout to render into
#   $2  the aeth-devkit requirement to render with: "aeth-devkit==13.0.0", or "aeth-devkit"
#       for the newest release on the index
set -euo pipefail
kind="$1"; devkit="$2"
tpl="$(cd "$(dirname "$0")/.." && pwd)/python/devkit_templates/templates"
root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/render-$kind"
rm -rf "$root" && mkdir -p "$root"

# A package directory under python/ marks a mixed Rust/Python layout; src/ otherwise.
pkg_dir=src; [ "$kind" = rust ] && pkg_dir=python
mkdir -p "$root/$pkg_dir/scratch_app" && : > "$root/$pkg_dir/scratch_app/__init__.py"
{
  printf '[project]\nname = "scratch-app"\nversion = "0.1.0"\nrequires-python = ">=3.14"\ndependencies = []\n\n'
  printf '[dependency-groups]\ndev = ["%s"]\n\n' "$devkit"
  if [ "$kind" = docker ]; then printf '[tool.docker]\nservices = ["scratch-app"]\n\n'; fi
  printf '[tool.uv.sources]\naeth-devkit = [{ index = "SFTPyPI" }]\n\n'
  printf '[[tool.uv.index]]\nname = "SFTPyPI"\nurl = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"\nexplicit = true\n'
} > "$root/pyproject.toml"
if [ "$kind" = rust ]; then
  printf '[package]\nname = "scratch-app"\nversion = "0.1.0"\nedition = "2024"\n\n[lib]\npath = "src/lib.rs"\n' > "$root/Cargo.toml"
  mkdir -p "$root/src" && : > "$root/src/lib.rs"
fi

cd "$root"
uv sync 2>&1 | tail -1
uv run devkit --version
# A plain run, not a dry run: files on disk are what the scan reads, and the package step
# then exercises the 4.0 constraint against the real index. The scratch dir is not a git
# repository, so nothing is committed; --no-commit says so explicitly.
uv run devkit setup-project --templates-dir "$tpl" -y --no-vscode --no-commit
# `${file}` in launch.json is VS Code's own variable; a bare `{name}` is a placeholder nobody
# substituted, including {latest} if no floor was written and {service}/{git_tag} if the
# scaffold missed a block.
if grep -rnE '(^|[^$])\{[a-z_]+\}' . --exclude-dir=.venv --exclude=uv.lock; then
  echo "unresolved placeholder(s) above" >&2
  exit 1
fi
echo "render ok: $kind through $(uv run devkit --version)"
```

Then `chmod +x ci/render.sh` (and `git update-index --chmod=+x ci/render.sh` on Windows, where the bit is not otherwise recorded).

- [x] **Step 4: `.github/workflows/ci.yml`**

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
  render:
    name: Render (${{ matrix.kind }}, ${{ matrix.devkit }} devkit)
    runs-on: ubuntu-latest
    strategy:
      fail-fast: false
      matrix:
        kind: [python, rust, docker]
        devkit: [floor, newest]
    steps:
      - uses: actions/checkout@v4

      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"

      # The floor is the compatibility contract: what this tree claims to render under. The
      # newest devkit catches a feature this tree uses that no release supports yet.
      - name: Render through the ${{ matrix.devkit }} devkit
        shell: bash
        run: |
          if [ "${{ matrix.devkit }}" = floor ]; then
            req="$(grep -oE 'aeth-devkit>=[0-9.]+' pyproject.toml | head -1 | sed 's/>=/==/')"
          else
            req="aeth-devkit"
          fi
          bash ci/render.sh "${{ matrix.kind }}" "$req"
```

No secrets: the index is anonymously readable (see Global Constraints).

- [x] **Step 5: The wheel carries the templates; the render script runs locally**

```bash
cd "$WS/devkit-templates"
uv build --out-dir dist 2>&1 | tail -2
uv run --no-project python -c "import zipfile,glob; n=zipfile.ZipFile(glob.glob('dist/*.whl')[0]).namelist(); t=[x for x in n if x.startswith('devkit_templates/templates/')]; assert 'devkit_templates/__init__.py' in n, n; assert 'devkit_templates/templates/pyproject.template.toml' in t and 'devkit_templates/templates/.mcp.template.jsonc' in t and 'devkit_templates/templates/github/workflows/release.rust.template.yml' in t, t; print('ok', len(t), 'template files')"
rm -rf dist
RUNNER_TEMP="$TEMP" bash ci/render.sh python "aeth-devkit==13.0.0" 2>&1 | tail -3
```

Expected: `ok 22 template files` (the dotfile and the nested paths included — if the dotfile is missing, `uv_build` skipped it and the fallback is hatchling with `[tool.hatch.build.targets.wheel] packages = ["python/devkit_templates"]`; record that in the execution notes); `render ok: python through devkit 13.0.0`. The local render needs the index (anonymous) and takes a minute (it locks and syncs the scratch project).

- [x] **Step 6: Commit**

```bash
cd "$WS/devkit-templates"
git add -A && git commit -q -m "build: package the templates with uv_build; CI renders the tree through released devkits

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git log --oneline -2 && git status --short
```

---

### Task 3: Create the repository, publish 1.0.0

**Files:** the remote repository, its secrets, a local `.env`; in the repo by 13.0.0's `setup-project`: the tooling tables, `.github/workflows/release.yml` (non-Rust), `claude.yml`, `.claude/`, `.vscode/`, `AGENTS.md`, `.mcp.json`, `uv.lock`.

**Interfaces:**
- Produces: `https://github.com/AetherBreaker/devkit-templates`, CI green, release `v1.0.0`, `devkit-templates==1.0.0` on SFTPyPI. Part B's bootstrap resolves this name.

- [x] **Step 1: Create, push, secrets, `.env`**

```bash
cd "$WS/devkit-templates"
gh repo create AetherBreaker/devkit-templates --public --description "Project-configuration templates for devkit-managed projects, the devkit_templates wheel devkit setup-project renders from"
git remote add origin https://github.com/AetherBreaker/devkit-templates.git && git push -u origin main 2>&1 | tail -1
gh repo view AetherBreaker/devkit-templates --json name,visibility,defaultBranchRef --jq '{name,visibility,default:.defaultBranchRef.name}'
for k in UV_INDEX_SFTPYPI_USERNAME UV_INDEX_SFTPYPI_PASSWORD; do
  grep "^$k=" "$WS/aeth_devkit/.env" | cut -d= -f2- | sed 's/^"\(.*\)"$/\1/' | gh secret set "$k" --repo AetherBreaker/devkit-templates && echo "set $k"
done
gh secret list --repo AetherBreaker/devkit-templates | awk '{print $1}'
cp "$WS/aeth_devkit/.env" .env && git check-ignore -q .env && echo ".env ignored"
```

Expected: `{"name":"devkit-templates","visibility":"PUBLIC","default":"main"}`; both secret names; `.env ignored`.

- [x] **Step 2: CI green**

```bash
sleep 20; gh run watch --repo AetherBreaker/devkit-templates --exit-status "$(gh run list --repo AetherBreaker/devkit-templates --workflow ci.yml --limit 1 --json databaseId --jq '.[0].databaseId')" 2>&1 | tail -3
```

Expected: all six matrix jobs green (both devkit entries resolve to 13.0.0 today). A failure is a Task 2 problem; fix on `main`, push, wait again.

- [x] **Step 3: `setup-project` by 13.0.0**

13.0.0 renders from its own bundled templates, which are this tree's files, and adds the hooks and completion packages (not this project's own name). It runs headless with `-y`:

```bash
cd "$WS/devkit-templates"
uv sync 2>&1 | tail -1
env -u VIRTUAL_ENV uv run --env-file .env devkit --version
env -u VIRTUAL_ENV uv run --env-file .env devkit setup-project --no-vscode -y 2>&1 | grep -v 'Failed to parse environment' | tail -40
```

Expected: `devkit 13.0.0`; one "Standardize project configuration with devkit" commit; `pyproject.toml` gains the tooling tables, `devkit-claude-hooks>=1.0.0`, `devkit-poe-complete>=1.0.0` and their sources; `.github/workflows/release.yml` is the non-Rust template (`grep -c 'uv build --out-dir dist' .github/workflows/release.yml` is `1`, `grep -c maturin` is `0`); the note names the two `UV_INDEX_SFTPYPI_*` secrets (already set).

- [x] **Step 4: Lock committed, dry-run clean, push**

The run's package step re-locked after its `pyproject.toml` edits; `uv.lock` is still untracked. Lock once more, format, commit:

```bash
cd "$WS/devkit-templates"
git status --short
uv lock 2>&1 | tail -1 && uv run tombi format --quiet pyproject.toml; git status --short
git add -A && git commit -q -m "chore: lock the dev group setup-project added

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>" ; git push 2>&1 | tail -1
env -u VIRTUAL_ENV uv run --env-file .env devkit setup-project --dry-run --no-vscode 2>&1 | grep -v 'Failed to parse' | tail -1
```

Expected: `Nothing to do — project already matches the templates.` (a second run is a no-op); `## main...origin/main`.

- [x] **Step 5: Release 1.0.0 and verify**

```bash
cd "$WS/devkit-templates"
env -u VIRTUAL_ENV uv run --env-file .env devkit release --dry-run 2>&1 | grep -v 'Failed to parse' | tail -10
env -u VIRTUAL_ENV uv run --env-file .env devkit release --force 2>&1 | grep -v 'Failed to parse' | tail -6
gh release view v1.0.0 --repo AetherBreaker/devkit-templates --json assets --jq '[.assets[].name]'
curl -s https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple/devkit-templates/ | grep -o 'devkit_templates-1\.0\.0[^"<#]*' | sort -u
```

Run the release with a long timeout (the workflow builds and publishes). Expected: `Released devkit-templates 1.0.0`; the release holds `devkit_templates-1.0.0-py3-none-any.whl` and `devkit_templates-1.0.0.tar.gz`; both names on the index.

- [x] **Step 6: The wheel resolves off the index and has the templates where the probe looks**

```bash
S="$TEMP/templates-resolve" && rm -rf "$S" && mkdir -p "$S" && cd "$S"
printf '[project]\nname = "scratch"\nversion = "0"\nrequires-python = ">=3.14"\ndependencies = []\n\n[dependency-groups]\ndev = ["devkit-templates"]\n\n[tool.uv.sources]\ndevkit-templates = [{ index = "SFTPyPI" }]\n\n[[tool.uv.index]]\nname = "SFTPyPI"\nurl = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"\nexplicit = true\n' > pyproject.toml
env -u VIRTUAL_ENV uv sync 2>&1 | tail -1
env -u VIRTUAL_ENV uv run python -c "import importlib.metadata as m, os, devkit_templates; d=os.path.join(os.path.dirname(devkit_templates.__file__),'templates'); assert os.path.isfile(os.path.join(d,'pyproject.template.toml')), d; print(m.version('devkit-templates'), d)"
```

Expected: `1.0.0 …/site-packages/devkit_templates/templates`. This is exactly what `packages::probe` will compute in Part B (`dir` = the package directory, then `templates`).

---

### Review checkpoint A

A fresh reviewer over the `devkit-templates` repository with spec 4.2 and 6 and Part A: the wheel's contents; the `uv_build` settings; the floor and its comment; the render script's scan (does it catch each placeholder class, does it wrongly catch `${file}`); the non-Rust release workflow; `.env` never committed; no path inside a template still says `aeth_devkit/templates`. Fix the union of real findings on `main`, push, wait for green, then start Part B.

---

## Part B: `aeth-devkit` reads templates from the environment; `--check` goes

One branch, `feat/extract-templates`. Every task ends with a compiling workspace and its own named tests green.

### Task 4: The fixture snapshot replaces the bundle

**Files:**
- Move: `python/aeth_devkit/templates/` → `crates/aeth-devkit-setup/tests/fixtures/templates/`.
- Modify: `crates/aeth-devkit-setup/tests/apply.rs`, `tests/docker.rs` (`templates()`), `crates/aeth-devkit-setup/src/packages.rs` (one test assertion).

**Interfaces:**
- Produces: a workspace with no bundled templates whose setup tests render the snapshot. The binary can find templates only through `--templates-dir`/`DEVKIT_TEMPLATES` until Task 5; the tests already pass them.

- [x] **Step 1: Branch and move**

```bash
cd "$WS/aeth_devkit" && git switch main && git pull --ff-only && git switch -c feat/extract-templates
git mv python/aeth_devkit/templates crates/aeth-devkit-setup/tests/fixtures/templates
git status --short | head -3; ls python/aeth_devkit; ls crates/aeth-devkit-setup/tests/fixtures/templates | head -5
```

Expected: `python/aeth_devkit` holds `__init__.py`, `_tasks_source.py`, `_tasks_generated.py` and the scripts; the fixture directory holds the 22 files (renames, so blame follows).

- [x] **Step 2: Point the test helpers at the snapshot**

In `tests/apply.rs`, `templates()` becomes `fixtures().join("templates")`. In `tests/docker.rs`, `templates()` becomes `fixtures().join("templates")` too — its `fixtures()` currently returns `tests/fixtures/docker`; change that helper to return `tests/fixtures` and update its two uses (`fixtures().join("docker")` for the container package dir in `package_dirs`, and any other). The module doc of `apply.rs` ("apply the real templates") becomes "apply the template snapshot under `tests/fixtures/templates` (the templates live in devkit-templates; the engine's tests are hermetic)".

- [x] **Step 3: The `probe` test no longer expects templates inside `aeth_devkit`**

In `packages.rs`'s probe test, replace `assert!(found.dir.join("templates").is_dir(), …)` with `assert!(found.dir.join("__init__.py").is_file(), "{}", found.dir.display());`.

- [x] **Step 4: Run the setup crate's tests**

```bash
cd "$WS/aeth_devkit" && cargo test -p aeth-devkit-setup 2>&1 | grep -E 'test result|FAILED|panicked' | head
```

Expected: every `test result: ok`. (The binary-level tests pass `--templates-dir "$(templates())"`; nothing exercises `locate` without an override.)

- [x] **Step 5: Commit**

```bash
git add -A && git commit -q -m "test(setup): the templates are a fixture snapshot; the bundle is gone

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Templates come from the environment

**Files:**
- Modify: `crates/aeth-devkit-setup/src/context.rs`, `src/templates.rs`, `src/packages.rs`, `src/lib.rs`, `src/cli.rs`.
- Test: `context.rs` and `packages.rs` unit tests, `tests/packages.rs`, `tests/apply.rs`.

**Interfaces:**
- Consumes: `packages::advance`, `probe`, `Venv`, `Changes::record`, `pyproject::find_requirement`.
- Produces: `ProjectContext::templates_dir: Option<PathBuf>`; `templates::override_dir(explicit, ctx) -> Result<Option<PathBuf>>`; `packages::TEMPLATES`; `packages::advance(ctx, deps, dry_run, packages, latest, changes)`; `packages::ensure_templates(ctx, deps, dry_run, changes) -> Result<PathBuf>`; `run_with(ctx, templates_override: Option<&Path>, dry_run, deps)`.

- [x] **Step 1: `[tool.devkit].templates-dir` in `context.rs`**

`DEVKIT_KEYS` becomes `&["release-workflow", "templates-dir"]`. Add the field and its parsing beside `release_workflow`:

```rust
  /// `[tool.devkit].templates-dir`: a directory to render instead of the environment's
  /// `devkit_templates` package, relative to the project root. The templates repository
  /// rendering its own tree; also any project that keeps a checkout beside it.
  pub templates_dir: Option<PathBuf>,
```

```rust
    let templates_dir = match devkit.and_then(|d| d.get("templates-dir")) {
      None => None,
      Some(item) => {
        let rel = item
          .as_str()
          .with_context(|| format!("[tool.devkit].templates-dir must be a string, got {}", item.to_string().trim()))?;
        let dir = root.join(rel);
        if !dir.is_dir() {
          bail!("[tool.devkit].templates-dir: {} is not a directory", dir.display());
        }
        Some(dir)
      }
    };
```

Tests (beside `release_workflow_is_on_unless_tool_devkit_turns_it_off`): the key is read relative to the root; a non-string or a missing directory is an error naming the key.

- [x] **Step 2: `templates::override_dir` replaces `locate`**

Delete `locate` whole (the override branches, the beside-the-binary probe, the source-tree fallback). Add:

```rust
/// The directory that renders instead of the environment's `devkit_templates`, if any:
/// `--templates-dir`, else `DEVKIT_TEMPLATES`, else `[tool.devkit].templates-dir` (validated
/// at discovery). Each is a working tree: a checkout beside the project, the templates
/// repository itself, or its CI. `None` means the package in the venv, which
/// `packages::ensure_templates` installs first when the project lacks it.
pub fn override_dir(explicit: Option<&Path>, ctx: &ProjectContext) -> Result<Option<PathBuf>> {
  if let Some(p) = explicit {
    return existing_dir(p.to_path_buf(), "--templates-dir").map(Some);
  }
  if let Ok(p) = std::env::var("DEVKIT_TEMPLATES") {
    return existing_dir(PathBuf::from(p), "DEVKIT_TEMPLATES").map(Some);
  }
  Ok(ctx.templates_dir.clone())
}
```

Remove imports that only `locate` used. A unit test covers the flag winning over the pyproject setting (build a `ProjectContext` with `templates_dir = Some(a)`, pass `Some(b)`, expect `b`) and a missing flag directory erroring with `--templates-dir` in the message; the env-var branch is covered at binary level in step 8 (a unit test would race other tests on the process environment).

- [x] **Step 3: `packages.rs`: `TEMPLATES`, `advance` takes its package list, the bootstrap**

Add the const beside the others:

```rust
/// The templates `setup-project` renders. The one package that must be in the environment
/// before any template is read (spec 4.0 "order within a run"), so the run bootstraps it by
/// this name ahead of the pyproject merge (`ensure_templates`); it is not in `active`.
pub const TEMPLATES: DevkitPackage = DevkitPackage {
  name: "devkit-templates",
  import_name: "devkit_templates",
};
```

`advance` gains `packages: &[&DevkitPackage]` after `dry_run` and uses it where it now calls `active(ctx)`; its doc says which packages. Because it now runs twice per run, guard the lock-mismatch `problem` push with `if !changes.problems.contains(&message)`, and reword the "is not in uv.lock after locking" context to `"{} is not in uv.lock after locking; pyproject.toml should list it (the merge or the templates bootstrap adds it)"`. Then the bootstrap:

```rust
/// The templates directory from the project's environment, installing `devkit-templates`
/// first when the project lacks it (spec 4.0): a bare requirement and its source by this
/// name, since no template is readable yet, then [`advance`] for this one package (the
/// lock under `aeth-devkit==<running>`, the `>=<locked>` floor, the sync). The templates
/// repository never carries itself; it renders its own tree through an override.
pub fn ensure_templates(ctx: &ProjectContext, deps: &crate::Deps, dry_run: bool, changes: &mut Changes) -> Result<PathBuf> {
  let root = &ctx.root;
  if normalize_dist_name(&ctx.name) == normalize_dist_name(TEMPLATES.name) {
    bail!(
      "{} is the templates package itself; render its own tree with --templates-dir, DEVKIT_TEMPLATES or [tool.devkit].templates-dir",
      ctx.name
    );
  }
  let pyproject_path = root.join("pyproject.toml");
  let text = std::fs::read_to_string(&pyproject_path).context("reading pyproject.toml")?;
  let doc: DocumentMut = text.parse().context("parsing pyproject.toml")?;
  let listed = find_requirement(&doc, TEMPLATES.name).is_some();
  if dry_run && (!listed || deps.venv.installed(root, &TEMPLATES).is_none()) {
    bail!(
      "devkit-templates is not in this project's environment; a plain run adds it to the dev group, locks it under aeth-devkit=={RUNNING_DEVKIT} and syncs, then renders. A dry run cannot."
    );
  }
  if !listed {
    add_bare_requirement(ctx, &pyproject_path, &text, doc, changes)?;
  }
  advance(ctx, deps, dry_run, &[&TEMPLATES], &[TEMPLATES.name.to_string()], changes)?;
  let installed = deps
    .venv
    .installed(root, &TEMPLATES)
    .context("devkit-templates is not in the project's environment after locking and syncing; is it elsewhere (UV_PROJECT_ENVIRONMENT)?")?;
  Ok(installed.dir.join("templates"))
}

/// `devkit-templates` into the dev group and `[tool.uv.sources]`, bare: `advance` writes the
/// floor right after. Intermediate tables are implicit so no empty `[tool]` header appears.
fn add_bare_requirement(ctx: &ProjectContext, path: &Path, original: &str, mut doc: DocumentMut, changes: &mut Changes) -> Result<()> {
  use toml_edit::{Array, InlineTable, Table, Value};
  let mut log = Vec::new();
  let groups = doc
    .entry("dependency-groups")
    .or_insert(Item::Table(Table::new()))
    .as_table_mut()
    .context("[dependency-groups] must be a table")?;
  let dev = groups
    .entry("dev")
    .or_insert(Item::Value(Value::Array(Array::new())))
    .as_array_mut()
    .context("[dependency-groups].dev must be an array")?;
  dev.push(TEMPLATES.name);
  log.push(format!("dependency-groups.dev: added \"{}\"", TEMPLATES.name));
  let mut implicit = Table::new();
  implicit.set_implicit(true);
  let tool = doc.entry("tool").or_insert(Item::Table(implicit.clone())).as_table_mut().context("[tool] must be a table")?;
  let uv = tool.entry("uv").or_insert(Item::Table(implicit)).as_table_mut().context("[tool.uv] must be a table")?;
  let sources = uv
    .entry("sources")
    .or_insert(Item::Table(Table::new()))
    .as_table_like_mut()
    .context("[tool.uv.sources] must be a table")?;
  if !sources.contains_key(TEMPLATES.name) {
    let mut entry = InlineTable::new();
    entry.insert("index", Value::from(ctx.devkit_index.as_str()));
    let mut arr = Array::new();
    arr.push(Value::InlineTable(entry));
    sources.insert(TEMPLATES.name, Item::Value(Value::Array(arr)));
    log.push(format!("added tool.uv.sources.{}", TEMPLATES.name));
  }
  changes.record(path, original, &doc.to_string(), log)
}
```

(`changes.record` writes the file; tombi formats it at the end of the run, so layout here only has to be valid.)

- [x] **Step 4: Step 0 in `run_with`; `cli` resolves the override after discovery**

`run_with`'s signature becomes `run_with(ctx: &ProjectContext, templates_override: Option<&Path>, dry_run: bool, deps: &Deps)`. Its first action after `changes.keep_previews`:

```rust
  // 0. The templates: an override renders a working tree; otherwise the environment's
  //    devkit-templates, installed first when the project lacks it, since nothing renders
  //    without it (spec 4.0). Before the merge, whose template this reads.
  let templates_dir = match templates_override {
    Some(dir) => dir.to_path_buf(),
    None => packages::ensure_templates(ctx, deps, dry_run, &mut changes)?,
  };
  let templates_dir = templates_dir.as_path();
```

Step 1b becomes `packages::advance(ctx, deps, dry_run, &packages::active(ctx), &packages::latest_requested(&pyproject_template), &mut changes)?;`. Update `lib.rs`'s top doc ("from the templates shipped with aeth-devkit" → "from the devkit-templates package in the project's environment") and the `keep_previews` comment (drop `--check`).

In `cli::run`: delete the `templates::locate` line at the top; after `ctx` is discovered, `let templates_override = crate::templates::override_dir(args.templates_dir.as_deref(), &ctx)?;` and pass `templates_override.as_deref()` into `run_with`. `Args.templates_dir`'s doc: "Render this directory instead of the environment's devkit-templates package (DEVKIT_TEMPLATES and [tool.devkit].templates-dir also set it, in that order of precedence)."

- [x] **Step 5: Build**

```bash
cd "$WS/aeth_devkit" && cargo build --workspace 2>&1 | grep -E '^(error|warning)' -A6 | head -40
```

Expected: clean. `tests/packages.rs`'s `advance` harness and every `run_with` call in `tests/apply.rs` and `tests/docker.rs` will not compile yet — step 6 and 7.

- [x] **Step 6: Package-step tests**

In `tests/packages.rs`: the harness's `packages::advance(&ctx, &deps, dry_run, latest, &mut changes)` becomes `packages::advance(&ctx, &deps, dry_run, &packages::active(&ctx), latest, &mut changes)`. Add a lock variant naming `devkit-templates` (extend `lock_with` with a `devkit-templates` 1.0.0 registry entry, or add `lock_with_templates()`), a `StubVenv` entry `"devkit_templates"` → `Installed { dir: fixtures_root(), version: "1.0.0" }` where `fixtures_root()` is `tests/fixtures` (so `dir.join("templates")` is the snapshot), and:

```rust
#[test]
fn the_bootstrap_adds_the_bare_requirement_and_source_and_returns_the_venv_templates_dir() {
  let dir = project(PLAIN_PYPROJECT, Some(&lock_with_templates("1.0.0")));
  let root = dir.path();
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec!["1.0.0".into()] };
  let (out, changes) = ensure(root, &runner, &index, &venv_with_templates("1.0.0"), false).unwrap();
  assert_eq!(out, fixtures_root().join("templates"));
  let py = fs::read_to_string(root.join("pyproject.toml")).unwrap();
  assert!(py.contains("\"devkit-templates>=1.0.0\""), "{py}");
  assert!(py.contains("devkit-templates = [{ index = \"SFTPyPI\" }]"), "{py}");
  let calls = runner.calls_for("uv");
  assert_eq!(calls[0], ["lock", "--upgrade-package", "devkit-templates", "--upgrade-package", &format!("aeth-devkit=={RUNNING_DEVKIT}")], "{calls:?}");
  assert_eq!(calls[1], ["lock"], "the re-lock after the floor");
  assert!(calls.iter().all(|c| c[0] != "sync"), "the stub venv already holds 1.0.0");
  assert!(changes.files.iter().any(|f| f.path.ends_with("pyproject.toml") && f.details.iter().any(|d| d.contains("added \"devkit-templates\""))), "{changes:?}");
}

#[test]
fn a_dry_run_without_the_templates_package_is_an_error_naming_the_remedy() {
  let dir = project(PLAIN_PYPROJECT, None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let err = ensure(dir.path(), &runner, &index, &venv(None), true).unwrap_err().to_string();
  assert!(err.contains("a plain run adds it"), "{err}");
  assert!(runner.calls_for("uv").is_empty());
}

#[test]
fn the_templates_repository_never_bootstraps_itself() {
  let dir = project(&PLAIN_PYPROJECT.replace("name = \"p\"", "name = \"devkit-templates\""), None);
  let runner = RecordingRunner::new(0);
  let index = StubIndexClient { versions: vec![] };
  let err = ensure(dir.path(), &runner, &index, &venv(None), false).unwrap_err().to_string();
  assert!(err.contains("templates-dir"), "{err}");
}
```

with an `ensure(...)` helper shaped like the file's `advance(...)` helper that calls `packages::ensure_templates` and returns `(PathBuf, Changes)`. Also assert, in the existing `a_project_without_docker_still_gets_the_hooks_and_completion`, that `active` does not contain `devkit-templates` (the bootstrap's package is not the merge's).

- [x] **Step 7: Apply and Docker harnesses compile against the new arity; one end-to-end bootstrap test**

In `tests/apply.rs` and `tests/docker.rs`, every `run_with(&ctx, &templates(), …)` becomes `run_with(&ctx, Some(&templates()), …)` (the override path: no bootstrap, no `uv` calls beyond what the package step already records). Add to `tests/apply.rs` one test through the venv path: the fixture project (whose pyproject has no `devkit-templates`), a `StubVenv` whose `devkit_templates` entry points at `fixtures()` (so the templates dir is the snapshot) at version `1.0.0`, and a `uv.lock` that names `devkit-templates 1.0.0` beside the three packages (extend `devkit_lock()`); run `run_with(&ctx, None, false, &deps)` and assert the run's `pyproject.toml` carries `devkit-templates>=1.0.0` and the source entry, that the rendered files equal a second run with `Some(&templates())` on an identical project (same `Changes::report`), and that a following `run_with(&ctx, None, false, &deps)` is empty (idempotent).

- [x] **Step 8: Binary-level override precedence**

In `tests/apply.rs`, beside the existing binary tests (`CARGO_BIN_EXE_devkit-setup`), a test that runs the binary with `.env("DEVKIT_TEMPLATES", templates())` and no `--templates-dir` on the fixture project with `--dry-run --no-vscode` and gets exit 0 and `Would change:` (the env override works without a venv), and one with `DEVKIT_TEMPLATES` pointing at a non-directory expecting exit 2 and `DEVKIT_TEMPLATES` in stderr.

- [x] **Step 9: Run the crate's tests and commit**

```bash
cd "$WS/aeth_devkit" && cargo fmt --all && cargo clippy -p aeth-devkit-setup --all-targets -- -D warnings 2>&1 | grep -E '^(warning|error)' -A6 | head -30
cargo test -p aeth-devkit-setup 2>&1 | grep -E 'test result|FAILED|panicked' | head
git add -A && git commit -q -m "feat(setup): templates come from the project environment, bootstrapped in when missing

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: Remove `--check`

**Files:**
- Modify: `crates/aeth-devkit-setup/src/cli.rs`, `src/changes.rs`, `src/docker/mod.rs`, `src/packages.rs` (comments), `tests/apply.rs`, `tests/docker.rs`, `python/aeth_devkit/_tasks_source.py`, `python/aeth_devkit/_tasks_generated.py` (regenerated).

**Interfaces:**
- Produces: a `setup-project` whose only dry form is `--dry-run`, exit 0.

- [x] **Step 1: `cli.rs`**

Delete the `check` field and its `#[arg(long)]` doc. `run_reject_headless`: `!(args.yes || args.dry_run)`; its message ends "or use --dry-run". In `run`: `let dry_run = args.dry_run;`; the VS Code skip is `args.no_vscode || !tty || args.yes` (a dry run still opens the review, as today); the nothing-to-write branch returns `ExitCode::SUCCESS` unconditionally (keep the "Nothing to write; the problem(s) above need a hand edit." line); delete `if args.check { return Ok(ExitCode::from(1)); }` after the report. The `run` doc: "Exit codes: 0 ok (a `problem:` line is a finding for a hand edit, not a failure), 3 commit failed (the template changes were rolled back). Errors bubble up for the caller to print (exit 2)."

- [x] **Step 2: Comments elsewhere**

`changes.rs`: the two `--check` sentences in the `notes`/`problems` docs become "a supported layout is never written; an unsupported one is a `problem:` reported on every run until it is fixed by hand". `docker/mod.rs`: `Mode::DryRun`'s doc says `--dry-run` only; the include-only warning comment says "so this warns instead of being a `problem:`". `packages.rs`: the lock-mismatch comment says "a dry run reports it as a problem: it must not read as clean". `lib.rs` `keep_previews` comment already changed in Task 5.

- [x] **Step 3: Tests**

`tests/apply.rs`: in `a_run_without_standard_input_is_refused_unless_nothing_will_be_asked`, replace the two `--check` runs with `--dry-run` ones (both exit 0; after deleting `.dockerignore` assert the output contains `Would change:`). Rewrite `check_fails_on_a_compose_file_the_engine_cannot_edit` as `an_unsupported_compose_shape_is_a_problem_reported_on_every_dry_run`: the `Args` literal loses `check`, `dry_run: true`; assert `changes.problems.len() == 1` and `cli::run` returns `SUCCESS` for the inline-services shape, and that the include-only shape is a warning with no problem. Remove `check: false` from every `Args` literal. `tests/docker.rs`: the three comments mentioning `--check` say `--dry-run`.

- [x] **Step 4: The task help text, regenerated**

In `_tasks_source.py`, the setup-project help: "Extra args are passed to devkit setup-project: --dry-run, --no-commit, -y/--yes, --templates-dir PATH." and "from the templates shipped with aeth-devkit" → "from the devkit-templates package". Then:

```bash
cd "$WS/aeth_devkit" && cargo build -p aeth-devkit 2>&1 | tail -1 && git diff --stat python/aeth_devkit/_tasks_generated.py
cargo test -p aeth-devkit-setup --test apply 2>&1 | grep -E 'test result|FAILED' ; cargo test -p aeth-devkit 2>&1 | grep -E 'test result|FAILED'
git add -A && git commit -q -m "feat(setup)!: remove --check; a dry run exits 0

--check was --dry-run plus an exit code and a suppressed VS Code review, and nothing
invoked it (spec 4.0). problem: lines stay findings for a hand edit.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: `devkit lock` moves the dev-group pin, not a runtime floor

**Files:**
- Modify: `crates/aeth-devkit-core/src/pyproject.rs`, `crates/aeth-devkit-lock/src/lib.rs`.
- Test: the lock crate's tests.

**Interfaces:**
- Produces: `pyproject::find_requirement_in_groups(doc, name) -> Option<Requirement>`; `bump_pin` prefers it.

- [x] **Step 1: Core helper**

Beside `find_requirement`:

```rust
/// [`find_requirement`] restricted to `dependency-groups.*`: the tooling pins, which are
/// the ones `devkit lock` moves. A `[project].dependencies` requirement for a devkit
/// package is a compatibility floor (devkit-templates), raised by hand.
pub fn find_requirement_in_groups(doc: &DocumentMut, name: &str) -> Option<Requirement> {
  let want = normalize_dist_name(name);
  requirement_tables(doc)
    .into_iter()
    .filter(|t| t.starts_with("dependency-groups."))
    .find_map(|table| requirement_in(doc, &table, &want))
}
```

If `find_requirement`'s body is not already factored as a per-table lookup (`requirement_in` above), factor the shared loop into one; that is a lint-free refactor with two callers, not a new small helper.

- [x] **Step 2: `bump_pin`**

```rust
  let Some(req) = pyproject::find_requirement_in_groups(doc, pkg).or_else(|| pyproject::find_requirement(doc, pkg)) else {
```

with a comment above: "The tooling pin first: a project that also names the package under `[project].dependencies` (devkit-templates, whose floor is its compatibility contract) keeps that requirement." A lock-crate test: a pyproject with `dependencies = ["aeth-devkit>=13.0.0"]` and `dev = ["aeth-devkit>=13.0.0"]`, a stub index at 14.0.0 — after `bump_pin` the dev entry is `>=14.0.0` and the runtime entry is still `>=13.0.0`; and a pyproject with only the runtime entry still gets it bumped (today's behaviour).

- [x] **Step 3: Test and commit**

```bash
cd "$WS/aeth_devkit" && cargo test -p aeth-devkit-lock -p aeth-devkit-core 2>&1 | grep -E 'test result|FAILED'
git add -A && git commit -q -m "feat(lock): move the dev-group pin, never a runtime floor

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: CI render job, docs, TODO, removal candidates

**Files:**
- Modify: `.github/workflows/ci.yml`, `README.md`, `WORKSPACE.md`, `TODO.md`, `REMOVAL-CANDIDATES.md`.

- [x] **Step 1: The `render` job**

Append to `.github/workflows/ci.yml` (beside `wheel`): the working-tree binary renders the newest released templates, as a dry run, into two scratch projects:

```yaml
  # The engine against the released templates: a change here that breaks a template shape
  # is caught before a release. A dry run, so no `aeth-devkit==<this version>` is resolved
  # (a plain run's package step would, and would fail on the commit that bumps the version
  # before its wheel exists).
  render:
    name: Render the released templates through this tree's devkit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable

      - uses: Swatinem/rust-cache@v2

      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"

      - name: Dry-run scratch projects
        shell: bash
        run: |
          set -euo pipefail
          cargo build -p aeth-devkit
          uv venv "$RUNNER_TEMP/tpl" --python 3.14
          uv pip install --python "$RUNNER_TEMP/tpl/bin/python" --index https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple devkit-templates
          tpl="$("$RUNNER_TEMP/tpl/bin/python" -c 'import devkit_templates, os; print(os.path.join(os.path.dirname(devkit_templates.__file__), "templates"))')"
          for kind in python docker; do
            root="$RUNNER_TEMP/scratch-$kind"
            mkdir -p "$root/src/scratch_app" && : > "$root/src/scratch_app/__init__.py"
            printf '[project]\nname = "scratch-app"\nversion = "0.1.0"\nrequires-python = ">=3.14"\ndependencies = []\n' > "$root/pyproject.toml"
            if [ "$kind" = docker ]; then printf '\n[tool.docker]\nservices = ["scratch-app"]\n' >> "$root/pyproject.toml"; fi
            target/debug/devkit setup-project --root "$root" --templates-dir "$tpl" --dry-run --no-vscode | tee "$root/out.txt"
            grep -q '^Would change:' "$root/out.txt"
          done
```

- [x] **Step 2: README**

In `### devkit setup-project`: the flags sentence becomes "Flags: `--root`, `--templates-dir` (or `DEVKIT_TEMPLATES`, or `[tool.devkit].templates-dir`; a working tree rendered instead of the environment's `devkit-templates` package), `--dry-run`, `--no-commit`, `-y`/`--yes`." Drop `--check` from the stdin sentence ("`-y` or `--dry-run`"). Add a bullet after Project discovery: "**Templates** - read from the project's environment, the `devkit_templates` package (`AetherBreaker/devkit-templates`); a project that lacks it gets `devkit-templates` added to its dev group, locked under the running devkit and synced before anything renders, and `--dry-run` on such a project is an error saying so. Templates are versioned by `uv.lock` like the other devkit packages." In the Docker bullets, replace "`--check` exits 1 on …" with "a `problem:` line on every run for …" and "does not fail `--check`" with "is a warning, not a problem", and "`--dry-run`/`--check` print everything" with "`--dry-run` prints everything". In the VS Code section, "neither `--check` nor `-y`" becomes "not `-y`". Add a short `### devkit-templates` section beside the hooks/completion one: what it is, that `setup-project` installs and reads it, where it lives. In Development: the layout line drops "templates" from `python/aeth_devkit`, and a sentence: "The setup crate's tests render the snapshot under `crates/aeth-devkit-setup/tests/fixtures/templates`; to render a templates checkout instead, pass `--templates-dir` or set `DEVKIT_TEMPLATES`."

- [x] **Step 3: WORKSPACE.md, TODO.md, REMOVAL-CANDIDATES.md**

`WORKSPACE.md`: add `gh repo clone AetherBreaker/devkit-templates` to the clone list, `devkit-templates` to the `.env` copy loop and to the bring-up loop, and after the bring-up block: "`devkit-templates` renders its own tree (`[tool.devkit].templates-dir`), so it needs no `devkit_templates` in its environment; its `[project].dependencies` floor on `aeth-devkit` is the compatibility contract and is raised by hand, while `poe lock` moves only its dev-group pin."

`TODO.md`: drop the `template.env` entry (now in devkit-templates's `TODO.md`).

`REMOVAL-CANDIDATES.md`, under aeth-devkit: `packages::DEVKIT` (after `locate`'s beside-the-binary probe went, only the `probe` unit test names it); `Changes::problems` as a distinct list from `warnings` now that no exit code depends on it (it still changes the wording "need a hand edit"; keep or fold, the user's call).

- [x] **Step 4: The full suite once, and commit**

```bash
cd "$WS/aeth_devkit"
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings 2>&1 | grep -E '^(warning|error)' -A6 | head -20
cargo test --workspace 2>&1 | grep -E 'test result|FAILED' | sort | uniq -c
uv run pytest -q 2>&1 | tail -2
git diff --exit-code -- python/aeth_devkit/_tasks_generated.py && echo "task table unchanged"
git add -A && git commit -q -m "docs: templates live in devkit-templates; the render CI job

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

Expected: every `test result: ok`, pytest green, the task table unchanged since Task 6's regeneration.

---

### Task 9: Pull request, reviews, merge, release 14.0.0

- [x] **Step 1: Push and open the PR**

```bash
cd "$WS/aeth_devkit" && git push -u origin feat/extract-templates
gh pr create --title "feat!: templates move to devkit-templates, read from the environment; --check removed" --body-file - <<'EOF'
Step 4 of docs/superpowers/specs/2026-09-08-devkit-split-design.md (plan: docs/superpowers/plans/2026-09-10-extract-devkit-templates.md).

- Every template now lives in `AetherBreaker/devkit-templates`, a pure-Python `devkit_templates` wheel on SFTPyPI (1.0.0). aeth-devkit ships none.
- `setup-project` reads templates from the project's environment; a project that lacks `devkit-templates` gets it added, locked under the running devkit and synced before anything renders. `--templates-dir`, `DEVKIT_TEMPLATES` and a new `[tool.devkit].templates-dir` render a working tree instead.
- `--check` is removed; `--dry-run` exits 0.
- `devkit lock` moves a dev-group pin in preference to a `[project].dependencies` requirement, so the templates package's compatibility floor is raised only by hand.
- A major: a project on 14.0.0 renders nothing until its first `setup-project` run installs the templates package. The note says to run `poe lock`, then `poe setup-project`, in every project.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
gh pr checks --watch 2>&1 | tail -6
```

Expected: `rust` (both OSes), `tests` (both), `wheel`, `render` green.

- [x] **Step 2: Two independent extra-high-effort reviews**

One on this session's model, one on another top model, each with the branch diff, this plan, the spec, and read-only access to `devkit-templates`. Point them at: the bootstrap inside the committing flow (the pyproject and lock edits happen after `stage_bases`, exactly where `advance`'s already do — confirm no double entry for `devkit-templates`, no lost user edit on replay, one `pyproject.toml` entry in the report); the dry-run error path writes nothing; override precedence and the unknown-key refusal for `[tool.devkit]`; that nothing still reads `python/aeth_devkit/templates` (grep the tree); `--check` gone from code, tests, docs and the task table; `bump_pin` on the two-requirement case; the `render` job's dry run on a project with no environment (the package step's dry-run notes, the container "not adopted" note). Verify the union of findings, fix what is real, push, wait for green.

- [x] **Step 3: Merge and release**

The standing instruction (step 2) is to merge and release once the reviews are clean; confirm only if that may have changed.

```bash
cd "$WS/aeth_devkit"
gh pr merge --rebase --delete-branch 2>&1 | tail -2; git switch main; git pull --ff-only
uv run poe release --dry-run major 2>&1 | tail -12
uv run poe release --force major "The templates now come from the devkit-templates package, which setup-project installs into every project and reads from its environment; the check flag is gone and a dry run always exits 0. In every project run poe lock, then poe setup-project, before the next session." 2>&1 | tail -6
uv run devkit --version
```

Expected: `Released aeth-devkit 14.0.0`, the workflow green, the local venv at `devkit 14.0.0`. (No dash-led words, no apostrophes in the note.)

---

## Part C: rollout and verification

### Task 10: The six devkit repositories take 14.0.0

- [ ] **Step 1: The five that carry `devkit_templates`**

In order, `aeth_devkit` first (its own pyproject gains `devkit-templates` through its bootstrap, spec 4.5). `poe lock` moves the `aeth-devkit` pin to 14.0.0 (`devkit-vscode`, `devkit-claude-hooks` and `devkit-poe-complete` have poe tasks since step 3); `poe setup-project -y` bootstraps the templates package, then renders:

```bash
for r in aeth_devkit devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete; do
  (cd "/d/SFT Software Projects/SFT Workspace/$r" && env -u VIRTUAL_ENV uv run poe lock && env -u VIRTUAL_ENV uv run poe setup-project --no-vscode -y)
done
```

Expected per repo: an "Update uv.lock" commit, then a "Standardize project configuration with devkit" commit whose `pyproject.toml` adds `"devkit-templates"` (then pinned `>=1.0.0`) and `devkit-templates = [{ index = "SFTPyPI" }]`, whose `uv.lock` locks `devkit-templates 1.0.0`, and whose report lists `pyproject.toml` once. `.claude/settings.local.json` keeps its two `PreToolUse` entries in the four satellites: the hooks merge adds and updates, never removes (b90d71a took them out of the template; taking them out of projects is the later hook rework's job, not this plan's).

- [ ] **Step 2: `devkit-templates` itself, by the other route**

Its environment must hold 14.0.0 before `[tool.devkit].templates-dir` can be read (13.0.0 refuses the key), and `poe lock` on 13.0.0 would move the runtime floor (Task 7 lands the preference in 14.0.0), so the lock moves by hand first:

```bash
cd "/d/SFT Software Projects/SFT Workspace/devkit-templates"
env -u VIRTUAL_ENV uv lock --upgrade-package aeth-devkit 2>&1 | tail -2      # the lock to 14.0.0; both pins untouched
env -u VIRTUAL_ENV uv sync 2>&1 | tail -1
env -u VIRTUAL_ENV uv run devkit --version
```

Then add to its `pyproject.toml`:

```toml
[tool.devkit]
  templates-dir = "python/devkit_templates/templates"
```

and:

```bash
git add -A && git commit -q -m "chore: setup-project renders this tree; take aeth-devkit 14.0.0

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
env -u VIRTUAL_ENV uv run poe setup-project --no-vscode -y 2>&1 | grep -v 'Failed to parse' | tail -12
env -u VIRTUAL_ENV uv run poe lock 2>&1 | tail -3
grep -n 'aeth-devkit' pyproject.toml
```

Expected: `devkit 14.0.0`; the run renders from the tree with no bootstrap and adds nothing named `devkit-templates`; `poe lock` (14.0.0) reports the dev-group pin at 14.0.0 and `[project].dependencies` still says `aeth-devkit>=13.0.0`.

- [ ] **Step 3: Verify, per repository**

```bash
cd "/d/SFT Software Projects/SFT Workspace/<repo>"
env -u VIRTUAL_ENV uv run devkit --version
grep -n 'devkit-templates' pyproject.toml
env -u VIRTUAL_ENV uv run devkit setup-project --dry-run --no-vscode 2>&1 | tail -1
env -u VIRTUAL_ENV uv run devkit setup-project --check 2>&1 | tail -1
```

Expected: `devkit 14.0.0`; the floor and the source in every repo but `devkit-templates` (which has only `templates-dir`); `Nothing to do — project already matches the templates.`; `error: unexpected argument '--check' found`. Then push every repo and confirm CI green (the templates repo's render CI now renders through 14.0.0 in its `newest` entry and 13.0.0 in its `floor` entry — both must pass, which is the compatibility guard working).

- [ ] **Step 4: Record**

Append an "Execution notes" section to this plan with whatever differed; commit it on `aeth_devkit` `main` and push. Add anything found along the way to `REMOVAL-CANDIDATES.md`. Remind the user that `CLAUDE_CODE_OAUTH_TOKEN` needs setting by hand on `devkit-templates`. Consumers stay where they are.

---

## Self-review notes

- **Spec coverage.** 4.2: package and backend (Task 2), SFTPyPI via the non-Rust workflow (Task 3), the floor and its rule (Task 2, the README, Task 7), read-from-venv with the override kept (Task 5), the render CI (Task 2 steps 3–4), no git/tag/cache/snapshot (nothing here fetches anything but the wheel). 4.0 "order within a run" (Task 5 step 0 before the merge, `advance` for the rest at 1b), `--check` removal (Task 6), `[tool.devkit]` as the settings table (Task 5 step 1). 4.5: `locate` becomes override-else-venv and the source-tree fallback goes (Task 5 step 2), one venv helper serves both packages (`probe`, unchanged), aeth-devkit's own pyproject gains the package through its own run (Task 10). Section 7: wheel before the referencing release (Part A before Part B); repository creation as prescribed (Tasks 1, 3). Section 9: a templates release flows with no devkit release (the templates repo's `poe release`); the floor guard's stop and throttle are `advance`'s existing behaviour under the constraint, exercised by the CI matrix's `floor`/`newest` pair.
- **Placeholder scan.** Every code block is complete; the one open contingency (uv_build skipping the dotfile) names its fallback.
- **Type consistency.** `override_dir` returns `Result<Option<PathBuf>>`; `ensure_templates` returns `Result<PathBuf>`; `run_with(ctx, Option<&Path>, bool, &Deps)`; `advance(ctx, deps, dry_run, &[&DevkitPackage], &[String], &mut Changes)`; `find_requirement_in_groups` returns `Option<Requirement>` like `find_requirement`.
- **The hard interaction** is the bootstrap's `pyproject.toml`/`uv.lock` edits inside the committing flow. They happen at the same point `advance`'s edits already happen (after `stage_bases`, recorded through `Changes`, merged against HEAD and replayed by `commit_changes`), which is why the bootstrap reuses `advance` rather than locking on its own. Task 9's reviewers are aimed at it.
