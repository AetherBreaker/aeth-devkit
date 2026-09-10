# Extract devkit-templates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move every template under `python/aeth_devkit/templates/` out of `aeth-devkit` into a new repository `AetherBreaker/devkit-templates`, shipped as a pure-Python wheel on SFTPyPI whose only content is a `devkit_templates` package holding the templates as package data; make `setup-project` read templates from the project's own venv (bootstrapping `devkit-templates` into a project that lacks it) instead of from the bundled directory; remove `--check`; and give the templates repo its own CI that renders the working tree through the released devkit. This is step 4 of the split spec.

**Architecture:** The templates directory leaves by `git filter-repo` with its history, renamed into `python/devkit_templates/templates/`, and is packaged with the `uv_build` backend (no code, just `__init__.py` plus package data). In `aeth-devkit`, a new bootstrap phase ensures `devkit-templates` is in the project's dev group and venv before any template is read; `templates::locate` gains the project root and resolves the templates from the venv's `devkit_templates` package (the source-tree fallback is deleted); `devkit-templates` joins the always-active package list (4.0); the pyproject template lists it; `--check` and its exit-1 path are removed; the setup crate's end-to-end tests source templates from the venv (honoring `DEVKIT_TEMPLATES`); the Rust CI job gains `uv sync`. devkit-templates declares `aeth-devkit>=13.0.0` and raises that floor only when a template first uses a new language feature.

**Tech Stack:** Rust 2024 (the setup/packages/templates crates), `uv_build` pure-Python backend, uv 0.11+, maturin unaffected, GitHub Actions, `gh`, `git filter-repo` via `uvx`, Python 3.14.

**Spec:** `docs/superpowers/specs/2026-09-08-devkit-split-design.md`, sections 2, 3, 4.0, 4.2, 4.5, 5, 6, 7 (step 4 and the "Repository creation" paragraph) and 9. Read it first. The three previous steps' plans (`2026-09-08-extract-devkit-container.md`, `2026-09-09-extract-devkit-vscode.md`, `2026-09-09-extract-hooks-and-completion.md`, each with an Execution notes section) are the recipe this plan repeats; their notes record every quirk hit so far, including the ones re-stated below.

## Global Constraints

- Distribution name `devkit-templates`; import/package name `devkit_templates`; repository `AetherBreaker/devkit-templates`; starts at version `1.0.0`, public, default branch `main`, created by the plan (spec 7), cloned to `D:\SFT Software Projects\SFT Workspace\devkit-templates`.
- The package has **no code**: `python/devkit_templates/__init__.py` is empty and every template lives under `python/devkit_templates/templates/`, preserving the current sub-paths (`vscode/…`, `docker/…`, `github/workflows/…`). `template.Dockerfile` does **not** move here; it already lives in `devkit-container` (step 1).
- Build backend is `uv_build` (`[build-system] requires = ["uv_build>=0.9,<0.10"]`, `build-backend = "uv_build"`), with `[tool.uv.build-backend] module-root = "python"` and `module-name = "devkit_templates"`. Non-`.py` files under the module root ride into the wheel automatically; there is no manifest to maintain.
- `devkit-templates` copies `aeth-devkit`'s `[[tool.uv.index]]` block verbatim (`publish-url` included: this repo publishes) and is released with `devkit release` through the **non-Rust** release workflow template (`release.template.yml`), which builds `dist/*` with `uv build` and publishes wheel + sdist.
- `devkit-templates` declares `aeth-devkit>=13.0.0` as an ordinary dev dependency at creation (13.0.0 is the newest released devkit and still carries bundled templates, so its `setup-project` can manage this repo). Its floor rises to `>=14.0.0` only in the same commit a template first uses a feature that only the step-4 devkit supports; no template content changes in this step, so it stays at `>=13.0.0`.
- Publication order (spec 7): `devkit-templates==1.0.0` is on SFTPyPI **before** the `aeth-devkit` release whose pyproject template references it. That release is a **major** (`14.0.0`), because `--check` (a documented flag) is removed and a project on 14.0.0 now requires `devkit-templates` in its venv to render anything. Its note says: run `poe lock`, then `poe setup-project`, in every project before the next use.
- `setup-project` for `devkit-templates` does what spec 4.0 says for a devkit package: adds the dev-group requirement when missing, locks under the `aeth-devkit==<running>` constraint, writes the `>=<locked>` floor, syncs. It is, additionally, the one package that must be present **before any template is read** (spec 4.0 "Order within a run"), so a bootstrap phase ensures it by its constant name ahead of the pyproject merge.
- `--templates-dir` and `DEVKIT_TEMPLATES` remain, as the override that renders a directory instead of the venv package: a sibling checkout for editing templates and `setup` together, and the templates repo's own CI. The source-tree fallback in `locate` (the `CARGO_MANIFEST_DIR/../../python/aeth_devkit/templates` branch) is deleted.
- `.env` holds live credentials: never print it, never commit it. The SFTPyPI secrets are set on the new repo with `gh secret set`, piped from `aeth_devkit/.env`, and `.env` is copied into the clone (standing decision from step 2).
- `devkit setup-project` refuses a non-terminal stdin only when there is **no** stdin at all; a pipe or terminal is fine, and `-y/--yes` accepts every prompt (this landed with step 3). The executor may run `setup-project` directly (it no longer needs a human at a terminal), but every `uv lock` / `poe lock` is still the user's to run unless the Bash dependency-guard hook is disabled for the session — ask before disabling it.
- `aeth-devkit` conventions (`AGENTS.md`): Conventional Commits; a `fix` body states bug, cause and fix; comments carry reasoning densely; no single-use helpers of four lines or fewer; tests carry no docstrings and never define intent; on a feature branch run only the named tests while iterating and the full suite once at the end. PRs are rebase-merged. End every commit body with the `Co-Authored-By: Claude <noreply@anthropic.com>` trailer, substituting the running model's name and version (for example `Claude Opus 4.8`).

## Context for the executor

### Where things are

- Workspace folder: `D:\SFT Software Projects\SFT Workspace` (Git Bash: `/d/SFT Software Projects/SFT Workspace`), holding `aeth_devkit` (underscore) plus `devkit-container`, `devkit-vscode`, `devkit-claude-hooks`, `devkit-poe-complete`. `$WS` below is that folder.
- `aeth_devkit` `main` at the time of writing: version `13.0.0` released (the hooks/completion split merged). The bundled templates still exist at `python/aeth_devkit/templates/` and 13.0.0 reads them from there; that is what lets 13.0.0 set up the new `devkit-templates` repo.
- The templates to move (22 files): run `git ls-files python/aeth_devkit/templates` to see them. `template.Dockerfile` is **not** among them (already moved in step 1).
- The template machinery in the setup crate:
  - `crates/aeth-devkit-setup/src/templates.rs` — `locate(explicit)`, `load`/`load_optional`/`substitute`, `gate`, `template_file_name`, `hook_bin`. `locate` currently tries the override, then `DEVKIT_TEMPLATES`, then the `aeth_devkit` package beside the devkit binary (`packages::probe(py, &DEVKIT)` then `found.dir.join("templates")`), then the source tree (`CARGO_MANIFEST_DIR/../../python/aeth_devkit/templates`).
  - `crates/aeth-devkit-setup/src/packages.rs` — `DevkitPackage`, the `CONTAINER`/`HOOKS`/`COMPLETE`/`DEVKIT` consts, `active(ctx)`, `latest_requested(template)`, `advance(ctx, deps, dry_run, latest, changes)`, `environment(root)`, `SystemVenv`, `probe(python, package)`, `StubVenv`, `locked_version`, `locked_registry_version`, `RUNNING_DEVKIT`.
  - `crates/aeth-devkit-setup/src/lib.rs` — `run(root, templates_dir, dry_run)` and `run_with(ctx, templates_dir, dry_run, deps)`; the numbered step list (1, 1b, 2…14) reads templates by passing `templates_dir` to `templates::load`. The package step is `1b` (`packages::advance`), currently **after** the pyproject merge (step 1).
  - `crates/aeth-devkit-setup/src/cli.rs` — `Args` (with `check`), `run_reject_headless`, `run`. `run` calls `templates::locate(args.templates_dir.as_deref())` then discovers ctx then stages bases then applies.
  - `crates/aeth-devkit-setup/src/docker/scaffold.rs` — `load(templates_dir, ctx)` reads `docker/compose.yaml` from the templates dir; unchanged except it now receives the venv-resolved dir.
- The tests that read the real templates directory: `crates/aeth-devkit-setup/tests/apply.rs` and `tests/docker.rs`, both via a `templates()` helper returning `CARGO_MANIFEST_DIR/../../python/aeth_devkit/templates`. `tests/packages.rs` uses inline pyproject strings and `tests/fixtures/docker/` (no template dir). The unit tests in `toml_merge.rs`, `json_merge.rs`, `lines.rs`, `md_block.rs` use inline string templates and are unaffected.
- `--check` surface to remove: `Args.check` and its clap arg (`cli.rs`); the `args.check` reads in `run_reject_headless` (exemption), `run` (`dry_run = args.dry_run || args.check`, the vs-code skip condition, the two `if args.check { ExitCode::from(1) }` returns); the doc comments in `changes.rs` that mention `--check`; `tests/apply.rs` (the `a_run_without_standard_input_…` test's `--check` forms, `check_fails_on_a_compose_file_the_engine_cannot_edit`, the `args` helper's `check:` field); `tests/docker.rs` comments; `README.md` (the `### devkit setup-project` flag paragraph and the Docker bullet); `python/aeth_devkit/_tasks_source.py` help text (then regenerate `_tasks_generated.py`).
- CI: `.github/workflows/ci.yml` has a `rust` job (no Python env), a `tests` job (has uv + pytest), a `wheel` job. The `rust` job runs `cargo test --workspace`; after this step its `apply`/`docker` integration tests need `devkit_templates` in a venv, so the `rust` job gains `astral-sh/setup-uv` + `uv sync`.
- `devkit-container` is the reference satellite for a wheel repo's shape, but its build backend is maturin; `devkit-templates` is pure-Python, so its `pyproject.toml` uses `uv_build` and its CI has no Rust toolchain and no binary-in-wheel check — it has a render-through-devkit check instead.

### Tools the plan assumes

`gh` authenticated as `AetherBreaker`, `git`, `uv` (0.11.16 here), `uvx` for `git filter-repo`, Rust stable with `rustfmt` and `clippy`, Python 3.14 reachable by uv. Docker is not needed. The GitHub MCP `create_repository` tool may be refused by the classifier (it was in step 3); `gh repo create AetherBreaker/devkit-templates --public --description "…"` (no `--push`) is the fallback and worked in step 3.

### What only the user runs, and the hook

- Every `uv lock` / `poe lock` and any command that runs `uv add`/`uv remove`/`uv lock` is blocked by the project's Bash dependency-guard hook (`.claude/settings.local.json`, `pre-bash-protect-deps`). In step 3 the user authorized disabling that hook for the session so the assistant could run `uv lock`; do not assume that standing — ask before disabling it, and restore it at the end of the session from a scratchpad backup. `uv sync` is always allowed.
- Merging the `aeth-devkit` PR and running `poe release` followed the user's standing "merge and release once reviews are clean" from step 2; ask again only if that may have changed.

### Lessons from steps 1-3 that apply here

- `git filter-repo` under Git Bash: set `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'` or the `old:new` rename arguments are mangled. It re-points tags; delete them all before the first push. Windows checkouts are CRLF: write `.gitattributes` first, then `git add --renormalize .` and re-checkout so the working tree is LF.
- `gh repo create --push` is refused by the tool permission layer; `gh repo create <name> --public --description …` (no `--push`) then `git remote add origin` + `git push -u origin main` gives the same end state.
- Write multi-line files with the Write tool or a Python script written to the scratchpad; heredocs and `sed` mangle escapes and quotes, and inline Python in a Bash heredoc breaks on backslash/quote escapes (write the script to the scratchpad and run it; read and write files with `encoding="utf-8", newline="\n"`).
- `poe release` words cannot start with a dash (clap reads them as flags) and an apostrophe is mangled by poe's re-quoting; word the release note plainly (step 3's note hit both).
- A satellite whose venv devkit cannot run `setup-project` for it is set up by the **current released** devkit instead: here `devkit-templates` is set up by 13.0.0, which still bundles templates, so no hand-rendering is needed (unlike step 3, where the siblings' release workflow had to be hand-rendered). Confirm 13.0.0's `setup-project` renders `devkit-templates`'s config cleanly before releasing it.
- The first `setup-project` run in a new repo has no `tombi` in the venv until it syncs; if a second "Standardize" commit appears, run `uv lock && uv sync && uv run tombi format --quiet pyproject.toml` and fold it into the lock commit.
- Reviews: a fresh reviewer after each task, and two independent extra-high-effort reviews of the PR (one on this session's model, one on another top model), each with the branch diff, the plan and the spec, and read-only access to the new repository. They found real defects in all three previous steps. Do the same.

### Decisions taken by this plan (the user may override before execution)

1. **Build backend `uv_build`.** The project is uv-managed; `uv_build` includes non-`.py` files under the module root with no manifest, and the non-Rust release workflow already uses `uv build`. `module-root = "python"`, `module-name = "devkit_templates"`, templates at `python/devkit_templates/templates/`.
2. **Templates live under a `templates/` subdirectory of the package** (`devkit_templates/templates/…`), so the venv probe returns the package dir and joins `templates`, exactly as `locate` does today for the bundled copy. The probe helper (`packages::probe`, `environment`, `SystemVenv`) is reused unchanged; a new `DevkitPackage` const `TEMPLATES` is added.
3. **A bootstrap phase before the merge.** `setup-project` ensures `devkit-templates` is in the project's dev group and venv before reading any template: if the requirement is absent it inserts a bare `devkit-templates` (plus the `[tool.uv.sources]` entry), locks under the `aeth-devkit==<running>` constraint, and syncs; then it probes the venv for the templates dir. The normal package step (`advance`, now including `TEMPLATES` in `active`) writes the `>=<locked>` floor afterwards like the other packages. The override (`--templates-dir`/`DEVKIT_TEMPLATES`) skips the bootstrap entirely.
4. **`locate` gains the project root and loses the source-tree fallback.** New signature resolves: the explicit override, else `DEVKIT_TEMPLATES`, else the project venv's `devkit_templates` package dir joined with `templates`. The old "beside the devkit binary" and "source tree" branches are deleted; devkit no longer ships templates beside itself.
5. **A dry run on a project without `devkit-templates` installed does not install it.** It reports that a plain run would add `devkit-templates` and renders nothing further (the same shape as the container "not adopted yet" dry-run note), and exits 0. A real run bootstraps. devkit's own tests and CI use the override, so they never hit this path.
6. **`--check` is removed** (spec 4.0). `--dry-run` stays and exits 0; idempotence is the acceptance test. The nag test already uses `docker-pin --dry-run`, so it is unaffected.
7. **aeth-devkit dev-depends on `devkit-templates`** (added to its own pyproject in this step), and the setup crate's `apply`/`docker` integration tests resolve the template directory from the venv's `devkit_templates` (honoring `DEVKIT_TEMPLATES` first). The Rust CI job gains `uv sync` so that venv exists. The authoritative render-of-real-content guard becomes `devkit-templates`'s CI.
8. **A major, `14.0.0`.** `--check` removal and the templates-from-venv contract change are breaking. The release note names the `poe lock` + `poe setup-project` remedy.
9. **The templates repo's CI renders through the released devkit**, not the branch's: `uvx --from aeth-devkit devkit setup-project --templates-dir . --dry-run --no-vscode -y` against three scratch projects (pure Python, Rust, Docker), failing on a render error or a leftover `{placeholder}`. This is the compatibility guard that exists before any release.
10. **The README section for templates moves**: `aeth-devkit`'s README keeps a short "templates live in devkit-templates" pointer (the way it points at the other satellites); the template-authoring detail moves to the new repo's README.
11. **Consumers are not migrated** by this plan (the user's standing instruction): only the six devkit repos take 14.0.0 in Part C. Sister projects migrate once the whole spec is done.

### The order that matters

Part A creates `devkit-templates` and publishes the wheel, set up by the current 13.0.0. Part B changes `aeth-devkit` on one branch and releases 14.0.0. Part C is the rollout. Part B must not be released before the wheel exists; Part C must not start before Part B is released. The bootstrap and `locate` changes are what make a project on 14.0.0 able to find templates at all, so their tests (Tasks 7-8) gate the release.

---

## File structure

**New repository `devkit-templates`** (Part A):
- `python/devkit_templates/__init__.py` — empty.
- `python/devkit_templates/templates/**` — the 22 template files, moved with history.
- `pyproject.toml` — `uv_build` backend, `devkit-templates` 1.0.0, dev group with `aeth-devkit>=13.0.0` and the poe include, `[[tool.uv.index]]` copied from aeth-devkit, `[tool.uv.sources] aeth-devkit = { index = "SFTPyPI" }`.
- `.gitignore`, `.gitattributes`, `README.md`, `TODO.md`, `.github/workflows/ci.yml` (render-through-devkit), and after `setup-project`: `.github/workflows/release.yml`, `.github/workflows/claude.yml`, `.claude/…`, `AGENTS.md`, `.mcp.json`, `uv.lock`.

**`aeth-devkit`** (Part B, branch `feat/extract-templates`):
- Delete `python/aeth_devkit/templates/` entirely.
- `crates/aeth-devkit-setup/src/packages.rs` — add `TEMPLATES` const; add it to `active`; add `ensure_templates` (bootstrap).
- `crates/aeth-devkit-setup/src/templates.rs` — rewrite `locate` (root-aware, venv probe, no source fallback).
- `crates/aeth-devkit-setup/src/lib.rs` — thread the resolved templates dir; call the bootstrap before step 1; add `devkit-templates` handling to the package step's `active`.
- `crates/aeth-devkit-setup/src/cli.rs` — remove `--check`; resolve templates via the bootstrap; pass root to `locate`.
- `crates/aeth-devkit-setup/src/changes.rs` — drop `--check` wording.
- `crates/aeth-devkit-setup/tests/apply.rs`, `tests/docker.rs` — `templates()` reads from the venv/override; drop `--check` assertions.
- `python/aeth_devkit/templates/pyproject.template.toml` **(moved to devkit-templates, but edited there)** — add `devkit-templates>={latest}` to the dev group and a `[tool.uv.sources]` entry. (Edited in the devkit-templates repo, then that repo re-released; see Task 10.)
- `.github/workflows/ci.yml` — `rust` job gains `uv sync`.
- `pyproject.toml` — aeth-devkit dev-depends on `devkit-templates`.
- `README.md`, `TODO.md`, `WORKSPACE.md`, `_tasks_source.py` (+ regenerate `_tasks_generated.py`).

---

## Part A: the repository and the wheel

### Task 1: Extract `python/aeth_devkit/templates` with its history

**Files:**
- Create: `$WS/devkit-templates` (a filtered clone), `.gitattributes` in it.

**Interfaces:**
- Produces: a local repository on branch `main`, no remote, no tags, whose tree is `python/devkit_templates/templates/**` (the former `python/aeth_devkit/templates/**`), LF throughout.

- [ ] **Step 1: Confirm `aeth_devkit` is current and clean**

```bash
WS="/d/SFT Software Projects/SFT Workspace"
cd "$WS/aeth_devkit" && git switch main && git pull --ff-only && git status --short --branch | head -3 && git log --oneline -1
git ls-files python/aeth_devkit/templates | wc -l
test ! -e "$WS/devkit-templates" && echo "target absent"
```

Expected: `## main...origin/main`, a clean tree, 22 template files listed, "target absent".

- [ ] **Step 2: Filter a throwaway clone down to the templates, renamed into the package**

```bash
cd "$WS" && git clone --no-local aeth_devkit devkit-templates && cd devkit-templates
MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*' uvx --from git-filter-repo git-filter-repo --force \
  --path python/aeth_devkit/templates/ \
  --path-rename python/aeth_devkit/templates/:python/devkit_templates/templates/
git log --oneline | wc -l
git ls-files | sed -n '1,30p'
git remote -v; git tag -l | wc -l
```

Expected: the templates' history (more than a handful of commits — templates change often); every path now under `python/devkit_templates/templates/`; no remote; some re-pointed tags.

- [ ] **Step 3: Delete inherited tags, confirm the branch, add the empty package and LF**

Write `python/devkit_templates/__init__.py` as an empty file. Write `.gitattributes` with exactly:

```
* text=auto eol=lf
*.sh text eol=lf
```

Then:

```bash
cd "$WS/devkit-templates"
git tag -l | xargs -r git tag -d >/dev/null; git tag -l | wc -l; git branch --show-current
git add .gitattributes python/devkit_templates/__init__.py
git add --renormalize .
git commit -q -m "chore: add .gitattributes, the package __init__, LF checkouts

Co-Authored-By: Claude <noreply@anthropic.com>"
git rm -rq --cached . && git reset -q --hard HEAD
git ls-files --eol | awk '{print $1, $2}' | sort | uniq -c
```

Expected: `0` tags; branch `main` (else `git branch -m main`); every file `i/lf w/lf` after the re-checkout; `python/devkit_templates/__init__.py` present and empty.

---

### Task 2: Stand `devkit-templates` up as its own package

**Files:**
- Create: `$WS/devkit-templates/pyproject.toml`, `.gitignore`, `README.md`, `TODO.md`, `.github/workflows/ci.yml`.

**Interfaces:**
- Consumes: the filtered tree from Task 1.
- Produces: a pyproject that builds a wheel carrying `devkit_templates/templates/**`, a CI that renders the templates through the released devkit, no dependency on any devkit Rust crate.

- [ ] **Step 1: Write `pyproject.toml`**

Copy `aeth_devkit`'s `[[tool.uv.index]]` block verbatim (read it from `$WS/aeth_devkit/pyproject.toml`). Write:

```toml
[project]
  name            = "devkit-templates"
  version         = "1.0.0"
  description     = "Project-configuration templates for devkit-managed projects, rendered by devkit setup-project"
  readme          = "README.md"
  requires-python = ">=3.14"
  dependencies    = []

[dependency-groups]
  dev = ["aeth-devkit>=13.0.0", "poethepoet>=0.46.0"]

[build-system]
  requires      = ["uv_build>=0.9,<0.10"]
  build-backend = "uv_build"

[tool.uv.build-backend]
  module-root = "python"
  module-name = "devkit_templates"

[tool.poe]
  include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]

[tool.uv.sources]
  aeth-devkit = { index = "SFTPyPI" }

[[tool.uv.index]]
  # (verbatim copy of aeth-devkit's block, publish-url included)
```

The remaining tooling tables (`[tool.coverage]`, `[tool.ruff]`, `[tool.pyright]`, `[tool.pytest.ini_options]`, `[tool.tombi]`) are added by `setup-project` in Task 4 — do not hand-write them.

- [ ] **Step 2: Write `.gitignore`, `README.md`, `TODO.md`**

`.gitignore`:

```
/.venv/
/dist/
/.cache/
/.env
```

`README.md` (short; the detail the setup crate needs about the template language stays in aeth-devkit's source comments):

```markdown
# devkit-templates

The project-configuration templates `devkit setup-project` renders into every
devkit-managed project: `pyproject.toml`, the VS Code files, `.gitignore`,
`.gitattributes`, `.dockerignore`, the compose scaffold, the GitHub workflows,
`AGENTS.md`, the Claude settings and `.mcp.json`. No code — a `devkit_templates`
package whose only content is `templates/`.

## How it reaches a project

A wheel on SFTPyPI, a dev dependency of every project. `setup-project` reads the
templates from the project's own venv; `uv.lock` is the pin. A content change needs
only a release. A change to the template *language* (a new placeholder, line gate,
table marker, file, or merge shape) is an aeth-devkit change first — the language
lives in aeth-devkit's `setup` crate — then a release here that raises the
`aeth-devkit>=` floor and uses the feature. The floor is how a project that has not
updated devkit is kept from rendering a template it cannot understand.

## Editing templates and `setup` together

Point devkit at a working tree instead of the venv with `--templates-dir .` (or
`DEVKIT_TEMPLATES`). CI renders the working tree this way against scratch projects.
```

`TODO.md`: a single heading `# devkit-templates TODO` with no entries yet (or move any template-specific entries the executor finds in aeth-devkit's `TODO.md` here; see Task 9).

- [ ] **Step 3: Write `.github/workflows/ci.yml` — render through the released devkit**

The guard of record. Render the working-tree templates through the latest released devkit against three scratch projects and fail on a render error or a leftover placeholder. Write the scratch projects inline so CI needs nothing but uv:

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
    name: Render templates through the released devkit
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"
      - name: Render against scratch projects
        shell: bash
        env:
          UV_INDEX_SFTPYPI_USERNAME: ${{ secrets.UV_INDEX_SFTPYPI_USERNAME }}
          UV_INDEX_SFTPYPI_PASSWORD: ${{ secrets.UV_INDEX_SFTPYPI_PASSWORD }}
        run: |
          set -euo pipefail
          tpl="$PWD/python/devkit_templates/templates"
          # A scratch project per layout; --templates-dir renders the working tree, not a
          # released package. -y accepts every prompt; --no-commit/--no-vscode keep it local;
          # a dry run writes nothing and still counts drift, so a render error or a leftover
          # placeholder fails the step.
          run() {
            local dir="$1"; shift
            ( cd "$dir" && uvx --from "aeth-devkit" --index "$DEVKIT_INDEX" devkit setup-project \
                --templates-dir "$tpl" --dry-run --no-vscode -y ) 2>&1 | tee out.txt
            ! grep -q '{[a-z_]*}' "$dir/out.txt"
          }
          # …create pyproject.toml for pure-Python, Rust (add a Cargo.toml), and Docker
          #   ([tool.docker].services) scratch dirs, each with the SFTPyPI index block…
```

The exact `uvx --from aeth-devkit` invocation and `DEVKIT_INDEX` wiring are the plan's to finalize against how the released devkit is fetched (it is a wheel on SFTPyPI, so `uvx --index <SFTPyPI simple URL> --from aeth-devkit devkit …` with the credentials in the env). Keep the three scratch `pyproject.toml` files minimal: name, version, `requires-python`, an empty dependency list, the `[[tool.uv.index]]` block, and for the Docker case a `[tool.docker] services = ["app"]` table. The check is: every run exits 0 and no line of its output contains an unresolved `{placeholder}`.

- [ ] **Step 4: Verify the wheel carries the templates (local)**

```bash
cd "$WS/devkit-templates"
uv build --out-dir dist 2>&1 | tail -2
uv run --no-project python -c "import zipfile,glob; z=zipfile.ZipFile(glob.glob('dist/*.whl')[0]); names=z.namelist(); assert 'devkit_templates/__init__.py' in names, names; assert any(n.startswith('devkit_templates/templates/') and n.endswith('pyproject.template.toml') for n in names), [n for n in names if 'templates' in n][:5]; print('ok', len([n for n in names if n.startswith('devkit_templates/templates/')]), 'template files')"
rm -rf dist
```

Expected: `ok 22 template files` (or the current count), proving `uv_build` includes the non-`.py` package data under the module root.

- [ ] **Step 5: Commit**

```bash
cd "$WS/devkit-templates"
git add -A && git commit -q -m "build: package the templates as a uv_build wheel with render CI

Co-Authored-By: Claude <noreply@anthropic.com>"
git log --oneline -2
```

---

### Task 3: Create the GitHub repository, push, secrets, `.env`

**Files:** the remote repository; repository secrets; a local `.env`.

**Interfaces:**
- Produces: `https://github.com/AetherBreaker/devkit-templates`, public, default `main`, CI runnable, the SFTPyPI secrets set.

- [ ] **Step 1: Create and push**

```bash
cd "$WS/devkit-templates"
gh repo create AetherBreaker/devkit-templates --public \
  --description "Project-configuration templates for devkit-managed projects (the devkit_templates wheel)"
git remote add origin https://github.com/AetherBreaker/devkit-templates.git
git push -u origin main 2>&1 | tail -1
gh repo view AetherBreaker/devkit-templates --json name,visibility,defaultBranchRef --jq '{name,visibility,default:.defaultBranchRef.name}'
```

Expected: `{"name":"devkit-templates","visibility":"PUBLIC","default":"main"}`. If the GitHub MCP `create_repository` tool is what the executor reaches for and it is refused, fall back to the `gh repo create` line above.

- [ ] **Step 2: Secrets and `.env`**

```bash
cd "$WS/devkit-templates"
for k in UV_INDEX_SFTPYPI_USERNAME UV_INDEX_SFTPYPI_PASSWORD; do
  grep "^$k=" "$WS/aeth_devkit/.env" | cut -d= -f2- | sed 's/^"\(.*\)"$/\1/' | gh secret set "$k" --repo AetherBreaker/devkit-templates && echo "set $k"
done
gh secret list --repo AetherBreaker/devkit-templates | awk '{print $1}'
cp "$WS/aeth_devkit/.env" .env && git check-ignore -q .env && echo ".env ignored"
```

Expected: both secret names listed; `.env ignored`. Never print the values.

- [ ] **Step 3: CI green**

```bash
sleep 20; gh run watch --repo AetherBreaker/devkit-templates --exit-status \
  "$(gh run list --repo AetherBreaker/devkit-templates --workflow ci.yml --limit 1 --json databaseId --jq '.[0].databaseId')" 2>&1 | tail -3
```

Expected: the render job green (it renders the working-tree templates through the released 13.0.0 devkit, which is compatible since no template content changed). A failure here is a Task 2 or 3 problem; fix on `main`, push, wait again.

---

### Task 4: Set up and release `devkit-templates` 1.0.0

**Files:** in the repo, by `setup-project` (it renders from the **current released** 13.0.0, which still bundles templates): the standard tooling tables in `pyproject.toml`, `.github/workflows/release.yml` and `claude.yml`, `.claude/…`, `AGENTS.md`, `.mcp.json`, `uv.lock`.

**Interfaces:**
- Produces: `devkit-templates==1.0.0` on SFTPyPI, release `v1.0.0` with a wheel and an sdist. Part B's pyproject template floor depends on this existing.

- [ ] **Step 1: Sync and set up**

`setup-project` now runs without a human at the terminal (step 3's `-y`). Run it with the released devkit the repo's dev group pins (`aeth-devkit>=13.0.0`):

```bash
cd "$WS/devkit-templates"
uv sync 2>&1 | tail -2
env -u VIRTUAL_ENV uv run --env-file .env devkit --version
env -u VIRTUAL_ENV uv run --env-file .env devkit setup-project --no-vscode -y 2>&1 | tail -40
```

Expected: `devkit 13.0.0`; one "Standardize project configuration with devkit" commit adding the tooling tables, `.github/workflows/release.yml` (the **non-Rust** template — `uv build`, not maturin), `claude.yml`, the `.claude/` files, `AGENTS.md`, `.mcp.json`; a note naming the two `UV_INDEX_SFTPYPI_*` secrets (already set). The `.env` parse warning uv prints about `PYTHONPYCACHEPREFIX` is harmless (TODO in aeth-devkit). If a second "Standardize" commit would appear on a re-run, fold `uv lock && uv sync && uv run tombi format --quiet pyproject.toml` into the lock commit in the next step.

- [ ] **Step 2: Confirm the release workflow is the non-Rust one, lock, push**

```bash
cd "$WS/devkit-templates"
head -1 .github/workflows/release.yml
grep -q 'uv build --out-dir dist' .github/workflows/release.yml && echo "non-Rust release workflow" || echo "WRONG WORKFLOW"
grep -q 'maturin' .github/workflows/release.yml && echo "UNEXPECTED maturin" || echo "no maturin (correct)"
# the dev group uv lock added is the user's to commit; if the hook is disabled for the session:
uv lock && uv sync && uv run tombi format --quiet pyproject.toml
git add -A && git commit -q -m "chore: lock the dev group setup-project added

Co-Authored-By: Claude <noreply@anthropic.com>" && git push 2>&1 | tail -1
```

Expected: the header line is the devkit-owned marker; `non-Rust release workflow`; `no maturin (correct)`.

- [ ] **Step 3: Release 1.0.0**

```bash
cd "$WS/devkit-templates"
env -u VIRTUAL_ENV uv run --env-file .env devkit release --dry-run 2>&1 | tail -12
env -u VIRTUAL_ENV uv run --env-file .env devkit release --force 2>&1 | tail -8
```

`devkit release` with no bump releases the committed `1.0.0`. Run it with a long timeout (the workflow builds and publishes). Expected: `Released devkit-templates 1.0.0`, the workflow green.

- [ ] **Step 4: Verify the artefacts and the index**

```bash
gh release view v1.0.0 --repo AetherBreaker/devkit-templates --json assets --jq '[.assets[].name]'
curl -s https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple/devkit-templates/ | grep -o 'devkit_templates-1\.0\.0[^"<#]*' | sort -u
```

Expected: a wheel and an sdist in the release; `devkit_templates-1.0.0-py3-none-any.whl` and `devkit_templates-1.0.0.tar.gz` on the index.

- [ ] **Step 5: The wheel resolves and carries the templates from a scratch project**

```bash
S="$TEMP/templates-resolve" && rm -rf "$S" && mkdir -p "$S" && cd "$S"
printf '[project]\nname = "scratch"\nversion = "0"\nrequires-python = ">=3.14"\ndependencies = []\n\n[dependency-groups]\ndev = ["devkit-templates"]\n\n[tool.uv.sources]\ndevkit-templates = [{ index = "SFTPyPI" }]\n\n[[tool.uv.index]]\nname = "SFTPyPI"\nurl = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"\nexplicit = true\n' > pyproject.toml
env -u VIRTUAL_ENV uv sync --env-file "$WS/aeth_devkit/.env" 2>&1 | tail -2
uv run --no-project python -c "import devkit_templates, os; d=os.path.join(os.path.dirname(devkit_templates.__file__),'templates'); assert os.path.isfile(os.path.join(d,'pyproject.template.toml')), d; print('templates at', d)"
```

Expected: `templates at …/devkit_templates/templates`, proving the probe path Part B relies on works off the index.

---

### Review checkpoint A

Dispatch a fresh reviewer over the whole `devkit-templates` repository (a clean clone or the local tree) with the spec sections 4.2 and 6 and this plan's Part A. It checks: the wheel carries every template and nothing else; `uv_build` config is right; the release workflow is the non-Rust one; the render CI actually fails on a bad placeholder (have it confirm by reasoning about the grep); `aeth-devkit>=13.0.0` is the floor; `.env` is ignored and never committed; no leftover reference to `aeth_devkit` paths inside the moved templates. Fix the union of real findings on `main`, push, wait for CI, before starting Part B.

---

## Part B: `aeth-devkit` reads templates from the venv, and drops `--check`

All of Part B is on one branch.

### Task 5: Branch; add the `TEMPLATES` package and the bootstrap

**Files:**
- Modify: `crates/aeth-devkit-setup/src/packages.rs`.
- Test: the `packages.rs` unit tests and `tests/packages.rs`.

**Interfaces:**
- Produces: `packages::TEMPLATES` (a `DevkitPackage`), `TEMPLATES` in `active(ctx)`, and `packages::ensure_templates(ctx, deps, dry_run, changes) -> Result<Option<PathBuf>>` returning the templates directory resolved from the venv (`None` only on a dry run where the package is absent, so the caller can note it and stop).

- [ ] **Step 1: Create the branch**

```bash
cd "$WS/aeth_devkit" && git switch main && git pull --ff-only && git switch -c feat/extract-templates
git log --oneline -1
```

- [ ] **Step 2: Add the `TEMPLATES` const and put it in `active`**

In `packages.rs`, beside `CONTAINER`/`HOOKS`/`COMPLETE`:

```rust
/// The project-configuration templates `setup-project` renders. Unlike the others it must
/// be present before any template is read (spec 4.0), so the run bootstraps it by this
/// name ahead of the pyproject merge; here it is an always-active package like the hooks.
pub const TEMPLATES: DevkitPackage = DevkitPackage {
  name: "devkit-templates",
  import_name: "devkit_templates",
};
```

In `active`, add `&TEMPLATES` to the always-active vec (beside `&HOOKS, &COMPLETE`); the `retain` that drops the project's own package still applies, so `devkit-templates` set up against itself is excluded. Update the doc comment to say hooks, completion **and templates** for every project.

- [ ] **Step 3: Write `ensure_templates`**

The bootstrap. It must (a) make `devkit-templates` a dev-group requirement if absent, with its source entry, so the later merge and `advance` find it; (b) lock it under the running-devkit constraint and sync; (c) return the templates dir from the venv. It reuses the lock/sync shape `advance` uses. Real code (adapt helper names to what `advance` already factors; if `advance`'s lock and sync are inline, extract the shared piece only if a lint forces it — otherwise duplicate the few lines):

```rust
/// Ensure `devkit-templates` is in the project's dev group and venv, and return its
/// `templates/` directory. The one package that must exist before any template is read
/// (spec 4.0): on a project that has never had it, add the bare requirement and its source,
/// lock under `aeth-devkit==<running>`, sync, then probe the venv. A dry run does not
/// install: if the package is absent it returns `None` and the caller reports that a plain
/// run would add it. The `--templates-dir`/`DEVKIT_TEMPLATES` override never reaches here.
pub fn ensure_templates(ctx: &ProjectContext, deps: &crate::Deps, dry_run: bool, changes: &mut Changes) -> Result<Option<PathBuf>> {
  let root = &ctx.root;
  let installed = deps.venv.installed(root, &TEMPLATES);
  if dry_run {
    if installed.is_none() {
      changes.notes.push(
        "devkit-templates is not installed in this venv; a plain run adds it, locks it and syncs before rendering.".into(),
      );
      return Ok(None);
    }
    return Ok(Some(templates_dir(&installed.unwrap())));
  }
  // Make the requirement and source exist so the merge and `advance` manage it afterwards.
  let pyproject_path = root.join("pyproject.toml");
  let text = std::fs::read_to_string(&pyproject_path).context("reading pyproject.toml")?;
  let mut doc: DocumentMut = text.parse().context("parsing pyproject.toml")?;
  let own = normalize_dist_name(&ctx.name);
  let is_own = normalize_dist_name(TEMPLATES.name) == own; // the templates repo itself
  let mut edited = false;
  if !is_own && find_requirement(&doc, TEMPLATES.name).is_none() {
    insert_dev_requirement(&mut doc, TEMPLATES.name);   // bare name; `advance` writes the floor
    ensure_source(&mut doc, TEMPLATES.name, &ctx.devkit_index);
    edited = true;
  }
  if edited {
    std::fs::write(&pyproject_path, doc.to_string()).context("writing pyproject.toml")?;
    changes.record(&pyproject_path, &text, &doc.to_string(), vec!["added devkit-templates".into()])?;
  }
  // Lock just this package (plus the devkit pin) and sync, so the venv has it to read.
  let args = [
    "lock".to_string(),
    "--upgrade-package".into(),
    TEMPLATES.name.into(),
    "--upgrade-package".into(),
    format!("aeth-devkit=={RUNNING_DEVKIT}"),
  ];
  let out = deps.docker.runner.run_capture("uv", &args, root)?;
  if !out.success() {
    bail!("uv lock failed bootstrapping devkit-templates: {}", out.stderr.trim());
  }
  match deps.docker.runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root)? {
    Some(0) => {}
    Some(code) => bail!("uv sync --frozen exited with {code} bootstrapping devkit-templates"),
    None => bail!("uv sync --frozen was terminated by a signal"),
  }
  changes.venv_synced = true;
  let installed = deps
    .venv
    .installed(root, &TEMPLATES)
    .context("devkit-templates is not in the venv after bootstrapping it; is the environment elsewhere (UV_PROJECT_ENVIRONMENT)?")?;
  Ok(Some(templates_dir(&installed)))
}

/// The `templates/` directory inside an installed `devkit_templates` package.
fn templates_dir(installed: &Installed) -> PathBuf {
  installed.dir.join("templates")
}
```

`insert_dev_requirement` and `ensure_source` are new small helpers on the pyproject doc; if the project has no `[dependency-groups].dev` array or no `[tool.uv.sources]` table, create them. Reuse `aeth_devkit_core::pyproject` helpers where they exist (`find_requirement`, `normalize_dist_name`, `index_url_for`); only write new TOML-editing code where none exists. Keep the bootstrap's pyproject edit minimal — the normal merge re-asserts the `{latest}` form and `advance` writes the floor, so the bootstrap's bare entry is transient within the same run.

- [ ] **Step 4: Unit test the active list and the dry-run note**

Add to `packages.rs` tests: `active` on a plain project returns templates, hooks and completion (no container); on a Docker project adds the container; on a project named `devkit-templates` drops templates but keeps the rest. In `tests/packages.rs`, add a test that `ensure_templates` on a dry-run project without the package in the stub venv returns `None` and pushes the "a plain run adds it" note, and on a project whose stub venv has it returns the `templates/` subdir of the stub dir. Build only these:

```bash
cd "$WS/aeth_devkit"
cargo test -p aeth-devkit-setup --lib packages:: 2>&1 | grep -E 'test result|FAILED'
cargo test -p aeth-devkit-setup --test packages 2>&1 | grep -E 'test result|FAILED'
```

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -q -m "feat(setup): devkit-templates is an active package with a bootstrap phase

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 6: Rewrite `locate` to resolve from the venv

**Files:**
- Modify: `crates/aeth-devkit-setup/src/templates.rs`.
- Test: the `templates.rs` unit tests.

**Interfaces:**
- Consumes: `ProjectContext` (for the root and venv), `packages::ensure_templates` from Task 5.
- Produces: `templates::locate(explicit, ctx, deps, dry_run, changes) -> Result<Option<PathBuf>>` — the override first, else the venv bootstrap. `run_with`/`cli` call this.

- [ ] **Step 1: Replace `locate`**

New `locate` honors the override (explicit arg, then `DEVKIT_TEMPLATES`) and otherwise bootstraps from the venv. It no longer looks beside the binary or in the source tree:

```rust
/// Resolve the templates directory: the explicit `--templates-dir`, then `DEVKIT_TEMPLATES`
/// (both render a working tree — a sibling checkout or this repo's own CI), else the project
/// venv's `devkit_templates` package, bootstrapped in when absent (spec 4.0, 4.5). `None`
/// only on a dry run that would have to install the package first; the caller reports it.
pub fn locate(
  explicit: Option<&Path>,
  ctx: &ProjectContext,
  deps: &crate::Deps,
  dry_run: bool,
  changes: &mut Changes,
) -> Result<Option<PathBuf>> {
  if let Some(p) = explicit {
    return existing_dir(p.to_path_buf(), "--templates-dir").map(Some);
  }
  if let Ok(p) = std::env::var("DEVKIT_TEMPLATES") {
    return existing_dir(PathBuf::from(p), "DEVKIT_TEMPLATES").map(Some);
  }
  crate::packages::ensure_templates(ctx, deps, dry_run, changes)
}
```

Delete the `current_exe`/`probe(&DEVKIT)`/`join("templates")` block and the `CARGO_MANIFEST_DIR/../../python/aeth_devkit/templates` fallback and the now-unused imports. `existing_dir` stays.

- [ ] **Step 2: Fix the `locate` doc/tests**

The existing `templates.rs` unit tests cover `template_file_name` and `hook_bin`, not `locate` — confirm none call the old `locate`. The `packages.rs` probe test that asserts `found.dir.join("templates").is_dir()` for the `aeth_devkit` package (around `packages.rs:512`) is now wrong — templates no longer ship in the `aeth_devkit` package. Change it to probe `devkit_templates` in the workspace venv and assert **its** `templates/` dir, or drop the `templates` assertion and keep only the version/dir checks. Pick the former if the workspace venv will have `devkit-templates` (it will, Task 11 adds the dev dep); guard with the same `return;`-when-no-venv the test already uses.

- [ ] **Step 3: Build the crate (it will not compile yet — callers change in Task 7)**

`locate`'s signature changed, so `cli.rs` will not compile until Task 7. That is expected; do not try to make the whole crate build in this task. Check just this file's logic with:

```bash
cd "$WS/aeth_devkit" && cargo build -p aeth-devkit-setup 2>&1 | grep -E 'error\[|error:' | head
```

Expect errors only in `cli.rs` (and possibly `lib.rs`) about `locate`'s arity — those are Task 7. If there are errors **inside** `templates.rs`, fix them here.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -q -m "feat(setup): locate templates from the project venv, not the bundle

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 7: Thread the resolved directory through `run_with` and `cli`; remove `--check`

**Files:**
- Modify: `crates/aeth-devkit-setup/src/lib.rs`, `crates/aeth-devkit-setup/src/cli.rs`, `crates/aeth-devkit-setup/src/changes.rs`.
- Test: compile; the nag test is unaffected (already `docker-pin`).

**Interfaces:**
- Consumes: `locate` (Task 6), `ensure_templates` (Task 5).
- Produces: a compiling `setup` crate where the templates dir is resolved once, the bootstrap runs before step 1, `active` includes templates, and `--check` is gone.

- [ ] **Step 1: Resolve templates inside the run, after ctx and staging**

In `cli::run`, remove the early `let templates = templates::locate(args.templates_dir.as_deref())?;`. The templates dir must be resolved after `ctx` is discovered and, on a committing run, after `stage_bases` (the bootstrap edits `pyproject.toml` and `uv.lock`, which the commit machinery merges against HEAD the way `advance` already does). Resolve it at the top of the `apply` closure (which already holds `deps` and runs after staging), then pass it into `run_with`. Change `run_with` to take the resolved dir. The cleanest shape: `run_with` resolves it itself as its first action, since it already builds nothing before step 1:

```rust
pub fn run_with(ctx: &ProjectContext, explicit_templates: Option<&Path>, dry_run: bool, deps: &Deps) -> Result<Changes> {
  let mut changes = Changes::new(dry_run);
  changes.keep_previews = deps.docker.reviewer.is_some();

  // Templates come from the project venv (spec 4.0/4.5); the bootstrap adds devkit-templates
  // when absent. A dry run that would have to install it renders nothing and says so.
  let Some(templates_dir) = templates::locate(explicit_templates, ctx, deps, dry_run, &mut changes)? else {
    return Ok(changes);
  };
  let templates_dir = templates_dir.as_path();

  // 1. pyproject.toml … (unchanged below, using `templates_dir`)
```

`cli::run` (the only non-test caller, at `cli.rs:163`) then passes `args.templates_dir.as_deref()` straight into `run_with`; the test call sites pass `Some(&tpl)` (Task 8). There is no `lib::run` convenience wrapper — it was removed in step 3. Note that step 1b's `packages::advance` now also manages `devkit-templates` (it is in `active`), so the bootstrap's bare floor becomes the real `>=<locked>` floor in the same run. Confirm `advance` writing a floor for a package the bootstrap already added does not double-add — `find_requirement` finds the bootstrap's entry and `advance` only rewrites its spec.

- [ ] **Step 2: Remove `--check`**

In `cli.rs`:
- Delete the `check` field and its `#[arg(long)]`.
- `run_reject_headless`: drop `args.check` from the exemption (`args.yes || args.dry_run`).
- `run`: `let dry_run = args.dry_run;`. Remove the `|| args.check` and the `args.check` term in the vs-code skip. Delete both `if args.check { return Ok(ExitCode::from(1)); }` returns and the `if args.check { 1 } else { SUCCESS }` in the nothing-to-write branch (it becomes plain `SUCCESS`).
- Update the `run` doc comment's exit-code list (drop the `--check found drift` exit 1; exit 1 now comes only from a commit failure path if any — verify, otherwise say 0 ok, 2 error from the caller, 3 commit failed).

In `changes.rs`: reword the two doc comments that say "`--check` passes/fails" to describe the supported-vs-unsupported-layout distinction without the flag (a supported layout is never written; an unsupported one is a `problem:` reported on every run).

- [ ] **Step 3: Build the whole crate and the dispatcher**

```bash
cd "$WS/aeth_devkit"
cargo build -p aeth-devkit-setup -p aeth-devkit 2>&1 | grep -E 'error|warning:' | head -20
```

Expect clean (tests come next). Fix any remaining caller of `locate`/`run_with`/`Args.check`.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -q -m "feat(setup)!: resolve templates in-run and remove --check

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 8: The setup crate's end-to-end tests source templates from the venv

**Files:**
- Modify: `crates/aeth-devkit-setup/tests/apply.rs`, `crates/aeth-devkit-setup/tests/docker.rs`.

**Interfaces:**
- Consumes: the new `run_with` arity and the override path.
- Produces: `apply`/`docker` tests that render real templates resolved from `DEVKIT_TEMPLATES`/the workspace venv, with the `--check` assertions gone.

- [ ] **Step 1: Point the `templates()` helper at the venv or override**

Both test files have `fn templates() -> PathBuf`. The templates no longer live at `../../python/aeth_devkit/templates`. Replace the helper (shared logic — put it in one test-support spot if both use it verbatim, else keep one per file) with: honor `DEVKIT_TEMPLATES` if set, else probe the workspace venv's `devkit_templates` package for its `templates/` dir, else **skip the test** (the pattern the probe test uses) — a clear message so a missing venv is obviously the cause:

```rust
fn templates() -> Option<PathBuf> {
  if let Ok(p) = std::env::var("DEVKIT_TEMPLATES") {
    return Some(PathBuf::from(p));
  }
  let venv = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join(".venv");
  let py = ["Scripts/python.exe", "bin/python"].iter().map(|r| venv.join(r)).find(|p| p.is_file())?;
  aeth_devkit_setup::packages::probe(&py, &aeth_devkit_setup::packages::TEMPLATES).map(|i| i.dir.join("templates"))
}
```

Callers become `let Some(tpl) = templates() else { return; };`. Because these tests pass an explicit `templates_dir`, `run_with` takes the override path and never bootstraps — so the tests need no `devkit-templates` lock/sync, only the files on disk. The tests already stub the venv for the package step via `StubVenv`; that stays.

- [ ] **Step 2: Drop the `--check` assertions**

In `apply.rs`: the `a_run_without_standard_input_…` test drops the `--check` invocations (keep `--dry-run`); `check_fails_on_a_compose_file_the_engine_cannot_edit` becomes a `--dry-run` test asserting the `problem:` is reported and nothing is written (rename it, e.g. `a_compose_file_the_engine_cannot_edit_is_a_problem_reported_every_run`); the `args` helper loses its `check:` field and the `dry_run: !check` logic becomes a plain `dry_run` parameter. In `docker.rs`, fix the two `--check` comments.

- [ ] **Step 3: Run these tests with the override set to the workspace templates — which no longer exist**

The workspace no longer has `python/aeth_devkit/templates`. To run these tests locally before Task 11 adds the dev dep, point `DEVKIT_TEMPLATES` at the sibling `devkit-templates` checkout's working tree:

```bash
cd "$WS/aeth_devkit"
DEVKIT_TEMPLATES="$WS/devkit-templates/python/devkit_templates/templates" \
  cargo test -p aeth-devkit-setup --test apply --test docker 2>&1 | grep -E 'test result|FAILED|panicked' | head
```

Expected: green. Without `DEVKIT_TEMPLATES` and without the package in the workspace venv yet, they skip (printing nothing) — that is fine until Task 11; CI gets the venv in Task 9.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -q -m "test(setup): source real templates from the venv or DEVKIT_TEMPLATES

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 9: Delete the bundled templates; dev-depend on devkit-templates; CI, docs, TODO

**Files:**
- Delete: `python/aeth_devkit/templates/` (the whole directory).
- Modify: `pyproject.toml` (aeth-devkit's own dev group + sources), `.github/workflows/ci.yml` (rust job gains uv sync), `README.md`, `TODO.md`, `WORKSPACE.md`, `python/aeth_devkit/_tasks_source.py` (+ regenerate).

**Interfaces:**
- Produces: an `aeth-devkit` that ships no templates, whose own venv and CI carry `devkit-templates`, whose docs point at the new repo.

- [ ] **Step 1: Delete the templates directory**

```bash
cd "$WS/aeth_devkit"
git rm -rq python/aeth_devkit/templates
ls python/aeth_devkit
```

Expected: the `templates/` directory gone; `__init__.py`, `_tasks_source.py`, `_tasks_generated.py`, the scripts remain.

- [ ] **Step 2: aeth-devkit dev-depends on `devkit-templates`**

Add `devkit-templates>=1.0.0` to aeth-devkit's own `[dependency-groups].dev` and a `[tool.uv.sources] devkit-templates = { index = "SFTPyPI" }` entry (match the existing `aeth-devkit`/hooks/completion source shape). This is the dev dep that puts `devkit_templates` in the workspace venv so the setup tests and the `rust` CI job can render. The `uv lock` that realizes it is the user's (or the session's, if the hook is disabled) — do it in Task 11's lock, or here if the hook is already down; either way commit the pyproject edit now and the lock with it when run.

- [ ] **Step 3: Rust CI job gains a venv**

In `.github/workflows/ci.yml`, the `rust` job currently has no Python. Add `astral-sh/setup-uv@v5` (python 3.14) and a `uv sync` step before `cargo test`, and pass the SFTPyPI credentials so `devkit-templates` installs:

```yaml
      - uses: astral-sh/setup-uv@v5
        with:
          python-version: "3.14"
      - name: Sync the venv (templates for the setup tests)
        env:
          UV_INDEX_SFTPYPI_USERNAME: ${{ secrets.UV_INDEX_SFTPYPI_USERNAME }}
          UV_INDEX_SFTPYPI_PASSWORD: ${{ secrets.UV_INDEX_SFTPYPI_PASSWORD }}
        run: uv sync
```

The `apply`/`docker` tests then resolve templates from `.venv`'s `devkit_templates`. (The `tests` job already syncs; the `wheel` job is unaffected.) Confirm `aeth-devkit` CI has the SFTPyPI secrets (it publishes, so it does).

- [ ] **Step 4: README, WORKSPACE, TODO, task help**

- `README.md`: in `### devkit setup-project`, drop `--check` from the flags paragraph and the `--dry-run`/`--check` mentions in the Docker bullet; reword to `--dry-run` only. Add a short `### devkit-templates` pointer section near the other satellite pointers: the templates live in `AetherBreaker/devkit-templates`, are a dev dependency, and `setup-project` reads them from the venv. Update the Development section's layout list (drop "templates" from `python/aeth_devkit`). Fix the Placeholders/`--templates-dir` line to say it overrides the venv package.
- `WORKSPACE.md`: add `devkit-templates` to the clone list, the `.env` copy loop, and the "bring each repository up" loop. Note it needs the Rust toolchain only if... it does not (pure Python) — say so if the file distinguishes.
- `TODO.md`: drop any entry the split makes moot; move template-authoring entries (if any) to `devkit-templates`'s `TODO.md` (Task 2 step 2). Keep the `template.env` unquoted-`PYTHONPYCACHEPREFIX` entry (it is about the template content, which now lives in devkit-templates — move it there).
- `python/aeth_devkit/_tasks_source.py`: drop `--check` from the setup-project help string. Regenerate `_tasks_generated.py`:

```bash
cd "$WS/aeth_devkit" && cargo build -p aeth-devkit 2>&1 | tail -1 && git diff --stat python/aeth_devkit/_tasks_generated.py
```

- [ ] **Step 5: The full suite once**

```bash
cd "$WS/aeth_devkit"
DEVKIT_TEMPLATES="$WS/devkit-templates/python/devkit_templates/templates" cargo test --workspace 2>&1 | grep -E 'test result|FAILED' | sort | uniq -c
uv run pytest -q 2>&1 | tail -3
git diff --exit-code -- python/aeth_devkit/_tasks_generated.py && echo "task table committed-clean after regen" || echo "stage the regenerated table"
```

Expected: every `test result: ok` (with `DEVKIT_TEMPLATES` pointing at the sibling tree so the render tests run); pytest green. The `test_build_regenerates`/`test_generated_tasks` python tests must still pass (they do not touch templates).

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -q -m "feat!: templates live in devkit-templates; drop the bundled directory

Co-Authored-By: Claude <noreply@anthropic.com>"
```

---

### Task 10: Teach the template to list devkit-templates (in the devkit-templates repo)

**Files:**
- Modify (in `$WS/devkit-templates`): `python/devkit_templates/templates/pyproject.template.toml`.

**Interfaces:**
- Produces: a pyproject template that unions `devkit-templates>={latest}` into the dev group and a `[tool.uv.sources]` entry, so a project's `setup-project` carries the templates package as a managed floor.

This edit lives in the **templates repo**, not aeth-devkit (the template content moved there). It is a content change that needs no floor bump (the `{latest}` placeholder and a new source key are existing language features), so `devkit-templates` stays `aeth-devkit>=13.0.0` — but the floor it *writes* into projects resolves under the running devkit, which in Part C is 14.0.0.

- [ ] **Step 1: Add the dev-group entry and the source**

In `$WS/devkit-templates/python/devkit_templates/templates/pyproject.template.toml`, in the `dev = [ … ]` array add `"devkit-templates>={latest}",` beside the hooks and completion entries, and in `[tool.uv.sources]` add `devkit-templates = [{ index = "{devkit_index}" }]` beside the others (ungated — every project gets templates, like hooks and completion). Update the dev-group comment to name templates alongside hooks and completion.

- [ ] **Step 2: Render-check and release a templates minor**

```bash
cd "$WS/devkit-templates"
# render the working tree through the released devkit to prove the placeholder resolves
S="$TEMP/tpl-render" && rm -rf "$S" && mkdir -p "$S" && cd "$S"
printf '[project]\nname = "scratch"\nversion = "0"\nrequires-python = ">=3.14"\ndependencies = []\n\n[[tool.uv.index]]\nname = "SFTPyPI"\nurl = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"\nexplicit = true\n' > pyproject.toml
env -u VIRTUAL_ENV uv run --env-file "$WS/aeth_devkit/.env" --directory "$WS/devkit-templates" -- devkit setup-project --root "$S" --templates-dir "$WS/devkit-templates/python/devkit_templates/templates" --dry-run --no-vscode -y 2>&1 | grep -i 'devkit-templates\|{' | head
```

Expected: the dry run lists `devkit-templates` added to the dev group and no leftover `{placeholder}`. Then commit and release a minor from the templates repo (it is the ordinary "a change is meant to reach projects" release):

```bash
cd "$WS/devkit-templates"
git add -A && git commit -q -m "feat(templates): projects carry devkit-templates as a managed floor

Co-Authored-By: Claude <noreply@anthropic.com>"
git push 2>&1 | tail -1
env -u VIRTUAL_ENV uv run --env-file .env devkit release --force minor 2>&1 | tail -6
```

Expected: `Released devkit-templates 1.1.0`, on SFTPyPI.

> Ordering note: this step can happen any time after Task 4 and before Part C's rollout. It is listed here because it depends on nothing in aeth-devkit's branch — only on the template file, which now lives in devkit-templates. Part C's `setup-project` runs pick up `devkit-templates>={latest}` from this release.

---

### Task 11: Pull request, review, release 14.0.0

- [ ] **Step 1: Push and open the PR**

```bash
cd "$WS/aeth_devkit"
git push -u origin feat/extract-templates
gh pr create --title "feat!: templates move to devkit-templates, read from the venv; --check removed" --body-file - <<'EOF'
Step 4 of docs/superpowers/specs/2026-09-08-devkit-split-design.md (plan: docs/superpowers/plans/2026-09-10-extract-devkit-templates.md).

- Every template moves to AetherBreaker/devkit-templates, a pure-Python `devkit_templates` wheel on SFTPyPI (already at 1.1.0). aeth-devkit ships no templates.
- `setup-project` reads templates from the project's venv, bootstrapping `devkit-templates` into a project that lacks it before rendering; `--templates-dir`/`DEVKIT_TEMPLATES` still override with a working tree.
- `--check` is removed; `--dry-run` stays and exits 0. Idempotence is the acceptance test.
- A major: a project on this devkit must carry `devkit-templates`. The release note says to run `poe lock`, then `poe setup-project`, in every project.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
gh pr checks --watch
```

Expected: all checks green (the `rust` job now syncs the venv, so the setup tests render real templates from the installed `devkit-templates`).

- [ ] **Step 2: Two independent extra-high-effort reviews**

One on this session's model, one on another top model. Each gets the branch diff, this plan, the spec, and read-only access to `devkit-templates`. Focus them on: the bootstrap's interaction with the committing flow (does `ensure_templates` editing `pyproject.toml`/`uv.lock` before the merge survive `stage_bases` and the HEAD-merge/replay correctly, with no double-add of the floor and no lost user edits?); the dry-run-without-the-package path (renders nothing, exits 0, writes nothing); `locate`'s override precedence; that nothing still reads the deleted bundled directory (grep the whole tree); the `rust` CI job actually installing `devkit-templates`; `--check` fully gone from code, tests, docs and the task table. Verify the union of findings, fix what is real, push, wait for green.

- [ ] **Step 3: Merge and release 14.0.0**

The user's standing instruction is to merge and release once reviews are clean; confirm only if it may have changed.

```bash
cd "$WS/aeth_devkit"
gh pr merge --rebase --delete-branch
git switch main && git pull --ff-only
uv run poe release --dry-run major
uv run poe release --force major "Templates move to the devkit-templates package, which setup-project reads from each project's venv and bootstraps in when missing; the setup-project check flag is removed. In every project run poe lock, then poe setup-project, before the next use."
```

Expected: `Released aeth-devkit 14.0.0`, the workflow green, `aeth-devkit==14.0.0` on SFTPyPI, the local venv at 14.0.0. (Word the note plainly: no leading dashes, no apostrophes — clap and poe both choke on them.)

---

## Part C: rollout and verification

### Task 12: Every devkit repository takes 14.0.0

- [ ] **Step 1: Lock and set up, in order**

`aeth_devkit` first (its own pyproject gains `devkit-templates` through its own run), then the satellites. Each `poe lock` moves the `aeth-devkit` pin to 14.0.0 and pulls `devkit-templates`; each `poe setup-project` bootstraps templates from the venv and renders. `setup-project` no longer needs a terminal (`-y`), but `poe lock`/`uv lock` is gated by the dependency-guard hook — run these with the hook disabled for the session (ask first) or hand them to the user:

```bash
for r in aeth_devkit devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete devkit-templates; do
  (cd "/d/SFT Software Projects/SFT Workspace/$r" && uv run poe lock && uv run poe setup-project --no-vscode -y)
done
```

`devkit-templates` is in the list: its own `poe lock` moves it to `aeth-devkit>=14.0.0`'s lock and its `setup-project` now renders from the 14.0.0 venv (it excludes itself from the package list, so it does not depend on its own name). Expected in each: a lock commit, then a "Standardize project configuration with devkit" commit whose pyproject adds `devkit-templates>=1.1.0`.

- [ ] **Step 2: Verify, per repository**

```bash
cd "/d/SFT Software Projects/SFT Workspace/<repo>"
uv run devkit --version                       # 14.0.0
grep -n 'devkit-templates' pyproject.toml     # floor present (except in devkit-templates itself)
python -c "import devkit_templates, os; print('templates at', os.path.join(os.path.dirname(devkit_templates.__file__),'templates'))"
uv run devkit setup-project --dry-run --no-vscode 2>&1 | tail -2   # Nothing to do
uv run devkit setup-project --check 2>&1 | tail -2 || echo "exit $? (expected: --check is gone)"
```

Expected: `devkit 14.0.0`; the floor present (absent only in `devkit-templates`); the package in the venv; `Nothing to do — project already matches the templates.`; `--check` now an unknown-flag error (proving its removal).

- [ ] **Step 3: Push every repo; confirm CI**

Push each repo's two commits; confirm CI green on each. `devkit-templates`'s render CI now renders through 14.0.0.

- [ ] **Step 4: Record**

Append an "Execution notes" section to this plan with whatever differed, commit it on `aeth_devkit` `main`, and push. Restore the Bash dependency-guard hook in `.claude/settings.local.json` from the scratchpad backup if it was disabled. Remind the user that `CLAUDE_CODE_OAUTH_TOKEN` needs setting by hand on `devkit-templates` (its `claude.yml` was installed).

---

## Self-review notes

- **Spec coverage.** 4.2 (package, uv_build, SFTPyPI, non-Rust workflow, language floor, read-from-venv, CI render guard, no git/cache/snapshot): Tasks 2, 3, 4, 10, plus the render CI in Task 2 step 3. 4.0 "templates package first" bootstrap: Task 5 `ensure_templates` + Task 7 ordering. 4.0 `--check` removal: Task 7 step 2. 4.5 `locate` rewrite + one venv-probe helper: Task 6 (the helper, `packages::probe`/`environment`, already serves both). 4.5 aeth-devkit's own pyproject gains the package: Task 9 step 2 + Task 12. Section 7 publication order (wheel before the referencing release): Part A before Part B. Section 9 "a templates release flows without a devkit release": Task 10 is exactly that.
- **Self-dependency.** `active`'s `retain` and `ensure_templates`'s `is_own` guard both exclude `devkit-templates` from managing itself (Task 5).
- **The one genuinely hard interaction** is the bootstrap editing `pyproject.toml`/`uv.lock` before the merge, inside the committing flow's stage/merge/replay. Task 11's reviewers are pointed straight at it; if `stage_bases` + the bootstrap conflict, the fallback is to run the bootstrap's lock/sync against HEAD's pyproject the way `advance` already does and let the merge own the floor — the plan's `ensure_templates` mirrors `advance`'s existing pattern deliberately so the two behave identically.
- **Type consistency.** `ensure_templates` and `locate` both return `Result<Option<PathBuf>>` with `None` meaning "dry run, package absent, nothing rendered"; `run_with` early-returns the accumulated `changes` on `None`. `templates()` in the tests returns `Option<PathBuf>` and callers `return` on `None`.
- **No placeholders.** Every code block is real; the only deferred detail is the exact `uvx --from aeth-devkit` fetch URL in the templates CI (Task 2 step 3), which depends on the SFTPyPI simple-index URL already used elsewhere in this repo and is noted as the plan's to finalize against it.
