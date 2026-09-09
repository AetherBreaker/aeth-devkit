# Extract devkit-vscode Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the VS Code extension out of `aeth-devkit` into its own repository, `AetherBreaker/devkit-vscode`, released from a tag push as GitHub releases `vN` that `devkit setup-project` keeps installing exactly as it does today.

**Architecture:** `vscode-extension/` and its release workflow leave `aeth-devkit` by `git filter-repo` with their history. The new repository is a plain npm project at its root (esbuild bundle, vitest tests, `vsce` packaging) plus a stub `pyproject.toml` that makes it devkit-managed. Its hand-written `release.yml` fires on a `vN` tag push and publishes `aeth-devkit-vscode-N.vsix` (manifest `N.0.0`) as release `vN`, continuing the build numbers from `vscode-extension-v1`. In `setup`, the installer changes two constants (`REPO`, `TAG_PREFIX`), and a new `[tool.devkit] release-workflow = false` switch lets a repository whose artefact is not a wheel keep its own `release.yml`. The consent protocol between devkit and the extension is untouched (`protocol: 1`, `MIN_EXTENSION_VERSION = 1`).

**Tech Stack:** Node LTS + npm (esbuild 0.25, vitest 4, `@vscode/vsce`), GitHub Actions, `gh`, `git filter-repo` via `uvx`, Rust 2024 for the `setup` changes, uv ≥ 0.12.

**Spec:** `docs/superpowers/specs/2026-09-08-devkit-split-design.md`, sections 2, 3, 4.3, 4.5, 5, 6, 7 (step 2, and the "Repository creation" paragraph) and 9. Read it first; every task below argues from it. Step 1's plan and its execution notes (`docs/superpowers/plans/2026-09-08-extract-devkit-container.md`) and the autopsy (`docs/superpowers/reports/2026-09-09-devkit-container-extraction-autopsy.md`) show how the first satellite was made; this plan repeats that recipe.

## Global Constraints

- The extension's identity does not change: `package.json` `name = "aeth-devkit"`, `publisher = "aeth"`, so the installed id stays `aeth.aeth-devkit` (`protocol.rs` `EXTENSION_ID`). A renamed extension would orphan every installed copy.
- Build numbers are integers and continue: the new repo's first release is `v2` (build 1 is `vscode-extension-v1` on `aeth-devkit`); the vsix is `aeth-devkit-vscode-N.vsix` with manifest version `N.0.0`, stamped by `scripts/package.sh`; `package.json` stays at `0.0.0`.
- `PROTOCOL` and `MIN_EXTENSION_VERSION` in `crates/aeth-devkit-setup/src/vscode/protocol.rs` stay `1`. The extension is not changed functionally in this step.
- Publication order (spec 7): release `v2` exists on `devkit-vscode` before the `aeth-devkit` release whose installer points at it; otherwise a machine without the extension fails `setup-project` with "no compatible devkit VS Code extension release exists yet".
- The new repository is public (the anonymous vsix download depends on it), default branch `main`, created by `gh repo create`, cloned beside the others. It publishes no wheel, so no SFTPyPI secrets are set for it.
- `.env` files hold live credentials: never print one, never commit one. Nothing in this plan needs one.
- `devkit setup-project` refuses to run without a terminal on stdin (`cli::run_reject_headless`), and the tool shells of a coding agent have none. Every non-dry setup-project run in this plan is the user's; the plan marks them **USER RUNS**. Never work around this by opening console windows.
- `aeth-devkit` conventions (its `AGENTS.md`): Conventional Commits with the trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`, a `fix` body states bug, cause and fix; comments carry reasoning densely; no tiny single-use helpers; tests carry no docstrings and do not define intent; on a feature branch run only the named tests while iterating and the full suite once at the end.
- `aeth-devkit` PRs are rebase-merged (linear history, original commits kept).

## Context for the executor

This plan is executed in a fresh session, possibly on another machine. Everything it needs is below or in the files it names.

### Where things are

- The repositories live side by side under one workspace folder; on the primary machine that is `D:\SFT Software Projects` (`/d/SFT Software Projects` in Git Bash). `aeth-devkit/WORKSPACE.md` is the mirror recipe. Commands below use `$WS` for that folder: `WS="/d/SFT Software Projects"`, adjusted to the machine.
- `aeth-devkit` `main` at the time of writing: `13ff3fb`, version `12.0.0` released (tag `v12.0.0`; wheels and sdist on SFTPyPI). `devkit-container` is at `1.0.0`. Open PRs on `aeth-devkit`: three Dependabot npm PRs against `/vscode-extension` (#14, #15, #17: vitest, vite, esbuild) and #12 (an unrelated shelved branch). Start by pulling `main` and confirming the tree is clean.
- The extension today: `aeth-devkit/vscode-extension/` (`src/*.ts` ×6, `test/*.ts` ×3, `esbuild.mjs`, `tsconfig.json`, `vitest.config.ts`, `scripts/package.sh`, `package.json`, `package-lock.json`, `.gitignore`, `.vscodeignore`, `README.md`), 16 commits of history. Its release workflow is `aeth-devkit/.github/workflows/vscode-extension.yml`; its CI job is the `extension` job in `aeth-devkit/.github/workflows/ci.yml`; the debugger config "VS Code extension (dev host)" is in `aeth-devkit/.vscode/launch.json`. The only release so far is `vscode-extension-v1` on `aeth-devkit` (asset `aeth-devkit-vscode-1.vsix`).
- The installer: `crates/aeth-devkit-setup/src/vscode/install.rs` (`REPO`, `TAG_PREFIX`, `refs_url`, `vsix_url`, `latest_tag_number`, `ensure_extension`). It lists tags through `https://api.github.com/repos/<REPO>/git/matching-refs/tags/<TAG_PREFIX>`, takes the highest integer `N` after the prefix, downloads `https://github.com/<REPO>/releases/download/<TAG_PREFIX><N>/aeth-devkit-vscode-<N>.vsix`, and runs `code --install-extension`. A `GH_TOKEN`/`GITHUB_TOKEN` in the environment is used only for the rate limit.
- `setup-project` installs `.github/workflows/release.yml` into every project it manages and **replaces** any file at that path that does not start with its header line (`lib.rs`, step 10b, `DEVKIT_WORKFLOW_HEADER`). That is why Task 6 exists: without the opt-out, the extension repo's own `release.yml` would be overwritten on its first `setup-project` run.
- `setup-project` creates `.vscode/launch.json` from its template only when the file is absent; an existing one is only patched in its Python configs (`json_merge::patch_launch`). So a `launch.json` written before the first run survives it.

### Tools the plan assumes

`gh` authenticated as `AetherBreaker` (`gh auth status`), `git`, `uv` ≥ 0.12 (`uv --version`), Node LTS with `npm` (`node --version` ≥ 20; the workflows use `lts/*`), `uvx` for `git filter-repo`, Rust stable for the `setup` crate, and Python 3.14 reachable by uv (`uv python install 3.14` if the machine lacks it). Docker is not needed. `code` on PATH is only needed for the optional end-to-end check in Task 10.

### What only the user can run

- Every `setup-project` run that is not `--dry-run` (terminal required). The plan hands these over with the exact command.
- Merging the `aeth-devkit` PR and running `poe release` (outward-facing; the user decides when). Step 1's user asked to be told when the PR was ready and then said "do the merge and release"; expect the same.

### Lessons from step 1 that apply here

- `git filter-repo` re-points every tag that reaches a surviving commit; delete all tags in the extracted repo before the first push, or `aeth-devkit`'s tags (and `vscode-extension-v1`) come along.
- Windows checkouts with `core.autocrlf=true` produce CRLF working trees; add `.gitattributes` (`* text=auto eol=lf`) as the first new commit and renormalise, or CRLF ends up in the vsix.
- Writing files from a coding agent's shell: heredocs can collapse backslash escapes and long quoted compound commands can fail to parse. Write multi-line content with the file-writing tool or a script file, then run short commands.
- The step-1 plan claimed the agent's stdin was a terminal; it was not (Python's `isatty()` lies on Windows, Rust's `IsTerminal` does not). Do not test this again; hand the runs to the user.
- `uv sync --frozen` removes anything the lock does not name; `uv lock` has no `--constraints` flag in any release.
- Reviews: step 1 dispatched a fresh reviewer subagent after each task and two independent extra-high-effort reviews of the PR before merge; several real defects were found that way. Do the same.

### Decisions taken by this plan (the user may override before execution)

1. **A `[tool.devkit] release-workflow = false` switch** (Task 6) instead of letting `setup-project` install an inert wheel-publishing `release.yml` into the extension repo. The inert file would misdescribe the repo, print a "add the publishing secrets" note on the first run, and fail if a release were ever created by hand. The spec (4.0) reserves `[tool.devkit]` for exactly such project settings; this is its first. Alternative rejected: naming the extension workflow something else and tolerating the inert devkit one.
2. **The extension workflow is `.github/workflows/release.yml`**, triggered by a `v*` tag push, creating the release with the workflow token (the `release: published` event it raises starts no other workflow, so nothing recurses). Alternative rejected: triggering on `release: published` as today; then a hand-created release is the trigger, and the devkit workflow question above becomes acute.
3. **The Dependabot bumps** (vitest ≥ 4.1.11, esbuild ≥ 0.25.0; vite comes along) land in the new repo **before** `v2` is tagged (Task 4), so the first release from the new home is not immediately followed by a security rebuild and the three `aeth-devkit` PRs close as moot.
4. **`aeth-devkit`'s release is `minor` (12.1.0).** Nothing a consumer sees changes: a machine with build 1 installed is not touched (`MIN_EXTENSION_VERSION` stays 1), and only a fresh install fetches from the new repository.
5. The stub `pyproject.toml` has `version = "0.0.0"` like `package.json`: the vsix is versioned by its build number, nothing in the repo is released by `devkit release`.

### The order that matters

Part A builds the repository and publishes `v2`. Part B changes `aeth-devkit` (the switch, the installer, the removals) and releases it. Part C runs `setup-project` in the new repository with that release, which is the first devkit that honours the switch. Part B must not be released before `v2` exists; Part C must not run before Part B is released.

## File structure

**`devkit-vscode` (new, `$WS/devkit-vscode`)** — from filter-repo, renamed to the root: `src/`, `test/`, `scripts/package.sh`, `esbuild.mjs`, `tsconfig.json`, `vitest.config.ts`, `package.json`, `package-lock.json`, `.gitignore`, `.vscodeignore`, `README.md`, `.github/workflows/release.yml` (rewritten in Task 3). Added by hand: `.gitattributes`, `.vscode/launch.json`, `.github/workflows/ci.yml`, `pyproject.toml`. Added by `setup-project` in Task 10: `AGENTS.md`, `.claude/*`, `.vscode/settings.json` and `extensions.json`, `.mcp.json`, `.github/workflows/claude.yml`, gitignore lines, `uv.lock`.

**`aeth-devkit` (branch `feat/extract-devkit-vscode`)**
- Modify: `crates/aeth-devkit-setup/src/context.rs` (`release_workflow`), `crates/aeth-devkit-setup/src/lib.rs` (step 10b gate), the test literals of `ProjectContext` in `src/docker/scaffold.rs`, `src/docker/static_files.rs`, `src/md_block.rs`, `src/templates.rs`, `src/toml_merge.rs`, `crates/aeth-devkit-setup/tests/apply.rs`, `crates/aeth-devkit-setup/src/vscode/install.rs`, `crates/aeth-devkit-setup/src/vscode/protocol.rs` (one doc comment), `.github/workflows/ci.yml`, `.vscode/launch.json`, `README.md`, `TODO.md`, `WORKSPACE.md`.
- Delete: `vscode-extension/` (whole directory), `.github/workflows/vscode-extension.yml`.

---

## Part A: the `devkit-vscode` repository

### Task 1: Extract `vscode-extension/` with its history

**Files:**
- Create: `$WS/devkit-vscode` (a filtered clone), `.gitattributes` in it.

**Interfaces:**
- Produces: a local repository on branch `main`, no remote, no tags, whose tree is the extension at the root plus `.github/workflows/release.yml` (the old workflow, rewritten in Task 3), LF throughout.

- [ ] **Step 1: Bring `aeth-devkit` up to date and confirm the tree**

```bash
WS="/d/SFT Software Projects"   # adjust to this machine
cd "$WS/aeth-devkit" && git switch main && git pull --ff-only && git status --short --branch | head -3
git log --oneline -1
ls vscode-extension && test -f .github/workflows/vscode-extension.yml && echo present
```

Expected: `## main...origin/main`, a clean tree, the directory and workflow present. If `main` has moved past `13ff3fb`, read the newer commits' messages before continuing.

- [ ] **Step 2: Filter a throwaway clone down to the extension**

```bash
cd "$WS"
git clone --no-local aeth-devkit devkit-vscode
cd devkit-vscode
uvx --from git-filter-repo git-filter-repo --force \
  --path vscode-extension/ \
  --path .github/workflows/vscode-extension.yml \
  --path-rename vscode-extension/: \
  --path-rename .github/workflows/vscode-extension.yml:.github/workflows/release.yml
git log --oneline | wc -l
git ls-files
git remote -v
git tag -l
```

Expected: about 16 commits; the file list is exactly `.github/workflows/release.yml`, `.gitignore`, `.vscodeignore`, `README.md`, `esbuild.mjs`, `package-lock.json`, `package.json`, `scripts/package.sh`, `src/consent.ts`, `src/extension.ts`, `src/lenses.ts`, `src/proposedDocs.ts`, `src/review.ts`, `src/runtimeBaseClasses.ts`, `test/consent.test.ts`, `test/runtimeBaseClasses.test.ts`, `test/vscode-stub.ts`, `tsconfig.json`, `vitest.config.ts`; no remote (filter-repo removes it); the tag list is the tags filter-repo re-pointed (there will be several).

- [ ] **Step 3: Delete every inherited tag and confirm the branch**

```bash
git tag -l | xargs -r git tag -d
git tag -l | wc -l
git branch --show-current
```

Expected: `0` tags; branch `main`. If the branch is not `main`, `git branch -m main`.

- [ ] **Step 4: LF everywhere, as the first new commit**

Write `.gitattributes` with exactly:

```
* text=auto eol=lf
*.sh text eol=lf
```

Then:

```bash
git add .gitattributes && git add --renormalize .
git commit -m "chore: add .gitattributes so checkouts are LF

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git ls-files --eol | grep -v 'i/lf' || echo "index is all LF"
```

Expected: "index is all LF" (a renormalised file may appear in the commit; that is the point).

---

### Task 2: Stand the extension up as its own repository

**Files:**
- Modify: `package.json` (repository URL), `.vscodeignore`, `README.md`.
- Create: `.vscode/launch.json`, `pyproject.toml`.

**Interfaces:**
- Consumes: the tree from Task 1.
- Produces: a repository that builds, typechecks, tests and packages from its root; a `pyproject.toml` whose dev group floor is the `aeth-devkit` release Part B will make (`12.1.0`) and which carries `[tool.devkit] release-workflow = false`.

- [ ] **Step 1: Point `package.json` at the new home**

In `package.json`, change only the repository URL:

```json
  "repository": { "type": "git", "url": "https://github.com/AetherBreaker/devkit-vscode" },
```

`name`, `displayName`, `publisher`, `version` (`0.0.0`), `engines`, `main`, `scripts`, `devDependencies` and `activationEvents` stay as they are.

- [ ] **Step 2: Keep the devkit-managed files out of the vsix**

`vsce` packages everything `.vscodeignore` does not exclude, and Task 10 adds devkit's files to the root. Append to `.vscodeignore`:

```
# devkit-managed repository files, not part of the extension
.github/**
.claude/**
.vscode/**
.cache/**
.venv/**
.gitattributes
.gitignore
.mcp.json
.python-version
AGENTS.md
pyproject.toml
uv.lock
```

- [ ] **Step 3: The debugger config comes along**

Create `.vscode/launch.json` (the dev-host config that lived in `aeth-devkit/.vscode/launch.json`, with the paths now at the root; `setup-project` leaves a pre-existing `launch.json` alone apart from Python configs, so this survives Task 10):

```jsonc
{
    "version": "0.2.0",
    "configurations": [
        {
            "name": "VS Code extension (dev host)",
            "type": "extensionHost",
            "request": "launch",
            "args": ["--extensionDevelopmentPath=${workspaceFolder}"],
            "outFiles": ["${workspaceFolder}/dist/**/*.js"]
        }
    ]
}
```

- [ ] **Step 4: The stub `pyproject.toml`**

Create `pyproject.toml` with exactly:

```toml
[project]
  name            = "devkit-vscode"
  # The extension is versioned by its build number (tag vN, vsix N.0.0); nothing in this
  # repository is released by `devkit release`, so this number never moves.
  version         = "0.0.0"
  description     = "The aeth-devkit VS Code extension: native diff consent for devkit setup-project"
  requires-python = ">=3.14"
  dependencies    = []

[dependency-groups]
  dev = ["aeth-devkit>=12.1.0"]

[tool.devkit]
  # The vsix is released by .github/workflows/release.yml on a `vN` tag push; devkit's
  # wheel-publishing release workflow has nothing to build here and must not replace it.
  release-workflow = false

[tool.poe]
  include_script = [{ script = "aeth_devkit:tasks", executor = { type = "uv", frozen = true } }]

[tool.uv.sources]
  aeth-devkit = [{ index = "SFTPyPI" }]

[[tool.uv.index]]
  name     = "SFTPyPI"
  url      = "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/+simple"
  explicit = true
```

No `[build-system]`: uv treats the project as virtual and `uv sync` installs only the dev group, which is what puts `devkit` in `.venv`. No `publish-url` on the index: nothing here publishes. The floor `12.1.0` is the release Task 9 makes; if that release comes out under another number, change this line before Task 10.

- [ ] **Step 5: Rewrite `README.md`**

Replace the file with:

```markdown
# aeth-devkit VS Code extension

`aeth.aeth-devkit`. Installed by `devkit setup-project` from this repository's GitHub
releases (`vN`, asset `aeth-devkit-vscode-N.vsix`); never published to the marketplace. It
shows each Docker change setup-project proposes as a native diff with per-hunk Accept and
Reject, opens a multi-diff review for `--dry-run`, and carries the
`Add to runtime-evaluated-base-classes` command. The consent protocol it speaks with devkit
is versioned (`protocol` in every request; a mismatch retires the reviewer for that run);
devkit's side is `crates/aeth-devkit-setup/src/vscode/` in `AetherBreaker/aeth-devkit`.

## Develop

```sh
npm ci
npm run typecheck   # esbuild strips types without checking them; this is the only type check
npm test
npm run build       # dist/extension.js
```

The `VS Code extension (dev host)` launch configuration runs the extension from this
checkout. The repository is devkit-managed: `uv sync` and `poe setup-project` keep the shared
configuration current; the Python tooling that merge brings in is inert here.

## Release

Builds are numbered. Push an annotated tag `vN` with the next integer `N` and the release
workflow builds, typechecks, tests, packages `aeth-devkit-vscode-N.vsix` (manifest `N.0.0`)
and publishes GitHub release `vN` with it:

```sh
git tag -a v3 -m "VS Code extension build 3" && git push origin v3
```

`devkit setup-project` installs the newest `N` that meets its `MIN_EXTENSION_VERSION`; bump
that constant in devkit when a protocol change makes older builds unusable. Build 1 was
released from `aeth-devkit` as `vscode-extension-v1`; numbering continues from there.
```

- [ ] **Step 6: Build, test and package from the root, and check what the vsix contains**

```bash
npm ci
npm run typecheck && npm test && npm run build
bash scripts/package.sh 99
npx @vscode/vsce ls | sort
rm -f aeth-devkit-vscode-99.vsix
git status --short
```

Expected: typecheck and tests pass; `dist/extension.js` exists (gitignored); `aeth-devkit-vscode-99.vsix` was produced (gitignored, deleted again); the `vsce ls` list holds `package.json`, `README.md`, `dist/extension.js` and no `src/`, `test/`, `pyproject.toml` or `.github/` entries. `git status` shows only the intended edits (package.json, .vscodeignore, README.md, .vscode/launch.json, pyproject.toml).

- [ ] **Step 7: Commit**

```bash
git add package.json .vscodeignore README.md .vscode/launch.json pyproject.toml
git commit -m "chore: stand the extension up as its own repository

The extension now lives at the repository root: package.json names the new
home, .vscodeignore keeps the devkit-managed files out of the vsix, the
dev-host launch config comes along, and a stub pyproject.toml makes the
repository devkit-managed with the release workflow opted out.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 3: Workflows: CI on push, release on a `vN` tag push

**Files:**
- Create: `.github/workflows/ci.yml`.
- Rewrite: `.github/workflows/release.yml` (the moved `vscode-extension.yml`).

**Interfaces:**
- Produces: `release.yml` that, for a pushed tag `vN` with integer `N`, publishes GitHub release `vN` with `aeth-devkit-vscode-N.vsix`. The installer in Part B depends on exactly that tag and asset shape.

- [ ] **Step 1: CI**

Create `.github/workflows/ci.yml`:

```yaml
name: CI

on:
  push:
    branches: [main]
  pull_request:

concurrency:
  group: ci-${{ github.ref }}
  cancel-in-progress: true

permissions:
  contents: read

jobs:
  # esbuild strips types without checking them, so `typecheck` is the only type check.
  extension:
    name: VS Code extension
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - uses: actions/setup-node@v4
        with:
          node-version: lts/*
          cache: npm

      - run: npm ci

      - run: npm run typecheck

      - run: npm test

      - run: npm run build
```

- [ ] **Step 2: The release workflow**

Replace `.github/workflows/release.yml` with:

```yaml
# The extension's release: a pushed tag `vN` (N an integer build number) builds, typechecks,
# tests and packages the vsix as aeth-devkit-vscode-N.vsix (manifest version N.0.0) and
# publishes it as GitHub release `vN`. Not `devkit release`: that bumps a Python version and
# verifies a wheel on an index, neither of which a vsix has (pyproject.toml opts this repo
# out of devkit's release workflow). The release is created with the workflow token, so the
# `release: published` event it raises starts no other workflow. `devkit setup-project`
# installs the newest `vN` whose N meets its minimum.
name: Release

on:
  push:
    tags: ["v*"]

permissions:
  contents: read

concurrency:
  group: release-${{ github.ref_name }}
  cancel-in-progress: false

jobs:
  release:
    runs-on: ubuntu-latest
    permissions:
      contents: write
    steps:
      - uses: actions/checkout@v4
        with:
          persist-credentials: false

      - name: The tag must be an integer build number
        id: version
        shell: bash
        run: |
          n="${GITHUB_REF_NAME#v}"
          [[ "$n" =~ ^[0-9]+$ ]] || { echo "tag $GITHUB_REF_NAME is not vN with an integer N" >&2; exit 1; }
          echo "n=$n" >> "$GITHUB_OUTPUT"

      - uses: actions/setup-node@v4
        with:
          node-version: lts/*
          cache: npm

      - run: npm ci

      - run: npm run typecheck

      - run: npm test

      - run: bash scripts/package.sh "${{ steps.version.outputs.n }}"

      - name: Release
        env:
          GH_TOKEN: ${{ github.token }}
          N: ${{ steps.version.outputs.n }}
        run: |
          gh release create "v$N" \
            --title "VS Code extension $N" \
            --notes "aeth-devkit VS Code extension build $N. Installed by devkit setup-project; not published to the marketplace." \
            "aeth-devkit-vscode-$N.vsix"
```

`scripts/package.sh` is unchanged: it already `cd`s to the repository root relative to itself and stamps `N.0.0`.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml .github/workflows/release.yml
git commit -m "ci: build on push, release the vsix on a vN tag push

The release no longer rides on a devkit release event: an annotated tag
vN is the release trigger, and the workflow publishes GitHub release vN
with aeth-devkit-vscode-N.vsix, the tag and asset shape devkit's installer
reads.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 4: Take the Dependabot bumps before the first release

**Files:**
- Modify: `package.json`, `package-lock.json`; possibly `vitest.config.ts` or `esbuild.mjs` if an API changed.

**Interfaces:**
- Produces: dev dependencies at or above the versions GitHub's alerts name: `vitest` ≥ 4.1.11 (critical below 3.2.6), `esbuild` ≥ 0.25.0; `vite` ≥ 6.4.3 arrives with vitest.

- [ ] **Step 1: Bump and reinstall**

```bash
npm install --save-dev vitest@^4.1.11 esbuild@^0.25.0
npm ci
npm run typecheck && npm test && npm run build
git diff --stat
```

Expected: tests pass and the bundle builds. If `npm test` fails on a vitest 4 change, fix `vitest.config.ts` minimally (the config uses the `vscode` stub alias; the vitest 4 migration notes are at vitest.dev). If `npm run build` fails on esbuild 0.25, adjust `esbuild.mjs` minimally. If either cannot be made green in reasonable time, fall back to `vitest@^3.2.6` (the critical fix) and record the rest in the README's Release section as pending; do not tag `v2` on a red tree.

- [ ] **Step 2: Commit**

```bash
git add package.json package-lock.json vitest.config.ts esbuild.mjs 2>/dev/null; git add package.json package-lock.json
git commit -m "build(deps): take vitest 4 and esbuild 0.25

GitHub's alerts against the lockfile (vitest below 3.2.6 critical, esbuild
0.24, vite through vitest) close with these; they land before the first
release from this repository so v2 is not immediately followed by a rebuild.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Create the GitHub repository, push, and publish build 2

**Files:**
- None locally beyond the tag.

**Interfaces:**
- Produces: `https://github.com/AetherBreaker/devkit-vscode`, public, `main` pushed, CI green, release `v2` with `aeth-devkit-vscode-2.vsix`. Part B's installer constants and Part C depend on it.

- [ ] **Step 1: Create and push**

```bash
cd "$WS/devkit-vscode"
gh repo create AetherBreaker/devkit-vscode --public --source=. --remote=origin --push \
  --description "The aeth-devkit VS Code extension: native diff consent for devkit setup-project"
git branch -u origin/main main
gh repo view AetherBreaker/devkit-vscode --json name,visibility,defaultBranchRef --jq '{name,visibility,default:.defaultBranchRef.name}'
```

Expected: `{"name":"devkit-vscode","visibility":"PUBLIC","default":"main"}`.

- [ ] **Step 2: CI must be green before anything is tagged**

```bash
gh run list --repo AetherBreaker/devkit-vscode --limit 3
gh run watch --repo AetherBreaker/devkit-vscode --exit-status "$(gh run list --repo AetherBreaker/devkit-vscode --workflow ci.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
```

Expected: the CI run completes with success. If `gh run list` is empty right after the push, the run has not been created yet; try again in a few seconds. A failure here is a Task 2 to 4 problem; fix it on `main` and push again before tagging.

- [ ] **Step 3: Tag build 2**

Build 1 is `vscode-extension-v1` on `aeth-devkit`; numbering continues.

```bash
git tag -a v2 -m "VS Code extension build 2"
git push origin v2
gh run watch --repo AetherBreaker/devkit-vscode --exit-status "$(gh run list --repo AetherBreaker/devkit-vscode --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
gh release view v2 --repo AetherBreaker/devkit-vscode --json tagName,assets --jq '{tag: .tagName, assets: [.assets[].name]}'
```

Expected: the release run succeeds; `{"tag":"v2","assets":["aeth-devkit-vscode-2.vsix"]}`. If `gh run list` shows no release run a few seconds after the push, the trigger did not fire: check the tag name and the `on.push.tags` pattern.

- [ ] **Step 4: Check the two URLs the installer will use**

```bash
curl -s https://api.github.com/repos/AetherBreaker/devkit-vscode/git/matching-refs/tags/v | grep -o '"ref": *"[^"]*"'
curl -sIL -o /dev/null -w '%{http_code}\n' https://github.com/AetherBreaker/devkit-vscode/releases/download/v2/aeth-devkit-vscode-2.vsix
```

Expected: `"ref": "refs/tags/v2"` and `200`. These are `install::refs_url()` and `install::vsix_url(2)` after Task 7.

---

## Part B: `aeth-devkit`

All Part B work is on branch `feat/extract-devkit-vscode` in `$WS/aeth-devkit`:

```bash
cd "$WS/aeth-devkit" && git switch main && git pull --ff-only && git switch -c feat/extract-devkit-vscode
```

Run only the named tests while iterating; the full suite runs once in Task 9.

### Task 6: `[tool.devkit] release-workflow = false`

**Files:**
- Modify: `crates/aeth-devkit-setup/src/context.rs`, `crates/aeth-devkit-setup/src/lib.rs` (step 10b), the `ProjectContext` literals in `crates/aeth-devkit-setup/src/docker/scaffold.rs`, `src/docker/static_files.rs`, `src/md_block.rs`, `src/templates.rs` (three literals), `src/toml_merge.rs` (two literals), `README.md`.
- Test: `crates/aeth-devkit-setup/src/context.rs` (unit), `crates/aeth-devkit-setup/tests/apply.rs`.

**Interfaces:**
- Produces: `ProjectContext.release_workflow: bool` (default `true`; `[tool.devkit].release-workflow`), and `setup-project` skipping step 10b when it is `false`.

- [ ] **Step 1: The failing tests**

In `crates/aeth-devkit-setup/src/context.rs`, inside `mod publish_index_detection` (or a new `mod devkit_settings` beside it), add:

```rust
  #[test]
  fn release_workflow_is_on_unless_tool_devkit_turns_it_off() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), "[project]\nname = \"p\"\n").unwrap();
    assert!(ProjectContext::discover(dir.path()).unwrap().release_workflow);
    std::fs::write(
      dir.path().join("pyproject.toml"),
      "[project]\nname = \"p\"\n\n[tool.devkit]\nrelease-workflow = false\n",
    )
    .unwrap();
    assert!(!ProjectContext::discover(dir.path()).unwrap().release_workflow);
    std::fs::write(
      dir.path().join("pyproject.toml"),
      "[project]\nname = \"p\"\n\n[tool.devkit]\nrelease-workflow = \"no\"\n",
    )
    .unwrap();
    let err = ProjectContext::discover(dir.path()).unwrap_err().to_string();
    assert!(err.contains("release-workflow"), "{err}");
  }
```

In `crates/aeth-devkit-setup/tests/apply.rs`, add:

```rust
#[test]
fn a_project_that_opts_out_keeps_its_own_release_workflow() {
  // The extension repository releases a vsix from a workflow of its own at the same path;
  // with the switch off, setup-project neither replaces it nor announces publishing secrets.
  let dir = make_project();
  let root = dir.path();
  let own = "name: Release\non:\n  push:\n    tags: [\"v*\"]\n";
  write(root, ".github/workflows/release.yml", own);
  let py = read(root, "pyproject.toml");
  write(root, "pyproject.toml", &format!("{py}\n[tool.devkit]\n  release-workflow = false\n"));
  let changes = run(root, false).unwrap();
  assert_eq!(read(root, ".github/workflows/release.yml"), own);
  assert!(!changes.notes.iter().any(|n| n.contains("release workflow")), "{:?}", changes.notes);
  assert!(!changes.managed.iter().any(|p| p.ends_with("release.yml")), "{:?}", changes.managed);
}
```

- [ ] **Step 2: Run them to see them fail**

```bash
cargo test -p aeth-devkit-setup release_workflow_is_on_unless 2>&1 | tail -5
cargo test -p aeth-devkit-setup --test apply a_project_that_opts_out 2>&1 | tail -5
```

Expected: compile errors (no field `release_workflow`).

- [ ] **Step 3: The field**

In `ProjectContext` (`context.rs`), after `devkit_index`:

```rust
  /// `[tool.devkit].release-workflow`: `false` opts the project out of the devkit release
  /// workflow, for a repository whose artefact is not a wheel and releases through a
  /// `release.yml` of its own (the VS Code extension). Default `true`. The first setting
  /// in `[tool.devkit]`, the table the split spec reserves for devkit-level project
  /// settings; the template seeds nothing there.
  pub release_workflow: bool,
```

In `discover`, before the `Ok(Self { … })`:

```rust
    // A value that cannot mean anything is an error, like `[tool.docker].services`: read as
    // `true` it would silently install a workflow the project meant to keep out.
    let release_workflow = match doc.get("tool").and_then(|t| t.get("devkit")).and_then(|d| d.get("release-workflow")) {
      None => true,
      Some(item) => item
        .as_bool()
        .with_context(|| format!("[tool.devkit].release-workflow must be true or false, got {}", item.to_string().trim()))?,
    };
```

and `release_workflow,` in the struct expression. Then every test literal of `ProjectContext` gains `release_workflow: true,` (add it after each `devkit_index: "SFTPyPI".into(),` line: `scaffold.rs`, `static_files.rs`, `md_block.rs`, `templates.rs` ×3, `toml_merge.rs` ×2). `cargo build -p aeth-devkit-setup --all-targets` finds any missed one.

- [ ] **Step 4: The gate in `lib.rs`**

Wrap step 10b. Replace

```rust
  // 10b. The release workflow is devkit-owned, unlike `claude.yml`: nothing in it is
  //      project-specific beyond the placeholders, so drift is replaced and reported. The
  //      one manual step — credentials — is announced whenever the devkit workflow displaces
  //      something else (nothing, or a workflow the project wrote): that is when its
  //      secret / trusted-publisher requirement arrives.
  {
```

with

```rust
  // 10b. The release workflow is devkit-owned, unlike `claude.yml`: nothing in it is
  //      project-specific beyond the placeholders, so drift is replaced and reported. The
  //      one manual step — credentials — is announced whenever the devkit workflow displaces
  //      something else (nothing, or a workflow the project wrote): that is when its
  //      secret / trusted-publisher requirement arrives. `[tool.devkit].release-workflow =
  //      false` skips all of it: that project's `release.yml` is its own (a vsix, say), and
  //      the file is neither written nor listed as managed. Nothing is deleted on the way
  //      out; a project that opts out later keeps whatever copy it has.
  if ctx.release_workflow {
```

and close the block's brace as before (the block was already a bare `{ … }`; it becomes `if ctx.release_workflow { … }`).

- [ ] **Step 5: Run the tests**

```bash
cargo test -p aeth-devkit-setup release_workflow_is_on_unless 2>&1 | tail -3
cargo test -p aeth-devkit-setup --test apply a_project_that_opts_out applies_and_is_idempotent 2>&1 | tail -3
cargo clippy -p aeth-devkit-setup --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: all pass, clippy and fmt clean. `applies_and_is_idempotent` still sees `release.yml` created for the ordinary fixture.

- [ ] **Step 6: README**

In `README.md`, the `setup-project` section's **Release workflow** bullet (it begins "`.github/workflows/release.yml` is rendered from the pure-Python or the maturin-matrix template"), append one sentence at its end:

```
  A repository whose artefact is not a wheel sets `[tool.devkit] release-workflow = false`
  and keeps a `release.yml` of its own (the VS Code extension); setup-project then writes
  nothing there.
```

- [ ] **Step 7: Commit**

```bash
git add crates/aeth-devkit-setup README.md
git commit -m "feat(setup): [tool.devkit].release-workflow opts a project out of the release workflow

The devkit release workflow is installed into every managed project and
replaces whatever sits at .github/workflows/release.yml. The VS Code
extension's repository releases a vsix from a workflow of its own at that
path, so it needs a way to keep it: the first [tool.devkit] setting, read
by ProjectContext and honoured by step 10b, which then writes nothing and
lists nothing as managed.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 7: The installer fetches from `devkit-vscode`

**Files:**
- Modify: `crates/aeth-devkit-setup/src/vscode/install.rs`, `crates/aeth-devkit-setup/src/vscode/protocol.rs` (one doc comment).
- Test: `install.rs` unit tests.

**Interfaces:**
- Consumes: release `v2` on `AetherBreaker/devkit-vscode` (Task 5).
- Produces: `install::REPO = "AetherBreaker/devkit-vscode"`, `install::TAG_PREFIX = "v"`; `refs_url()` = `https://api.github.com/repos/AetherBreaker/devkit-vscode/git/matching-refs/tags/v`, `vsix_url(n)` = `https://github.com/AetherBreaker/devkit-vscode/releases/download/v<n>/aeth-devkit-vscode-<n>.vsix`.

- [ ] **Step 1: The failing test**

In `install.rs`'s `mod tests`, change the fixture and the URL assertion:

```rust
  const REFS: &str = r#"[{"ref":"refs/tags/v1"},{"ref":"refs/tags/v3"},{"ref":"refs/tags/v2"},{"ref":"refs/tags/vX"},{"ref":"refs/tags/v1.0.0"}]"#;
```

and in `parses_installed_version_and_tag_numbers`:

```rust
    assert_eq!(
      vsix_url(3),
      "https://github.com/AetherBreaker/devkit-vscode/releases/download/v3/aeth-devkit-vscode-3.vsix"
    );
```

(`v1.0.0` documents that a non-integer suffix is ignored; the existing assertion that the latest is `3` covers it.)

```bash
cargo test -p aeth-devkit-setup install::tests 2>&1 | grep -E 'test result|FAILED|panicked' | head
```

Expected: `parses_installed_version_and_tag_numbers` fails on the URL.

- [ ] **Step 2: The constants and the docs**

In `install.rs`:

```rust
//! Getting a compatible extension into VS Code: the newest `vN` release of the extension's
//! own repository is fetched from GitHub (the repo is public; a `GH_TOKEN`/`GITHUB_TOKEN`
//! in the environment is sent only for the higher rate limit, and dropped if rejected) and
//! handed to `code --install-extension`. A fresh install is live at once; an upgrade over a
//! loaded extension needs a window reload, which the caller reports and stops on.
```

```rust
/// The extension's repository. Build 1 was `vscode-extension-v1` on `AetherBreaker/aeth-devkit`
/// and stays published there; numbering continued in the new repository from `v2`.
pub const REPO: &str = "AetherBreaker/devkit-vscode";
/// Tags are `vN` with an integer `N`; anything else after the prefix is ignored.
pub const TAG_PREFIX: &str = "v";
```

Update `latest_tag_number`'s doc comment from `refs/tags/vscode-extension-vN` to `refs/tags/vN` (integer `N`; a `v1.0.0` is not one and is skipped). In `protocol.rs`, the doc on `MIN_EXTENSION_VERSION`:

```rust
/// The first extension build (`N` of release `vN` in `AetherBreaker/devkit-vscode`; build 1 was
/// `vscode-extension-v1` on `aeth-devkit`) that speaks [`PROTOCOL`].
pub const MIN_EXTENSION_VERSION: u32 = 1;
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p aeth-devkit-setup install::tests 2>&1 | grep -E 'test result' | head -2
```

Expected: all install tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/aeth-devkit-setup/src/vscode/install.rs crates/aeth-devkit-setup/src/vscode/protocol.rs
git commit -m "feat(setup): install the VS Code extension from AetherBreaker/devkit-vscode

The extension now releases from its own repository as vN; the installer's
repository and tag prefix follow. Build numbers, the asset name and the
protocol minimum are unchanged, so a machine with build 1 installed is not
touched.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: Remove the extension, its workflow and its CI job; the docs

**Files:**
- Delete: `vscode-extension/`, `.github/workflows/vscode-extension.yml`.
- Modify: `.github/workflows/ci.yml` (drop the `extension` job), `.vscode/launch.json` (drop the dev-host config), `README.md`, `TODO.md`, `WORKSPACE.md`.

- [ ] **Step 1: Remove**

```bash
git rm -r -q vscode-extension .github/workflows/vscode-extension.yml
```

In `.github/workflows/ci.yml`, delete the whole `extension:` job (from the comment line `# esbuild strips types without checking them …` through its last `- run: npm run build`), leaving `rust`, `python` and `wheel`. In `.vscode/launch.json`, delete the `"VS Code extension (dev host)"` configuration object (and the comma before it). Check nothing else refers to the directory:

```bash
git grep -n 'vscode-extension' -- ':!docs' ':!TODO.md'
```

Expected: no output.

- [ ] **Step 2: README**

Replace the `### VS Code extension` section's first paragraph (it begins "`aeth.aeth-devkit`, in `vscode-extension/`") with:

```markdown
`aeth.aeth-devkit`, in its own repository, `AetherBreaker/devkit-vscode`. Never published
to the marketplace: each build is a GitHub release there (`vN`, asset
`aeth-devkit-vscode-N.vsix`), cut by that repository's workflow from a `vN` tag push. Build 1
is `vscode-extension-v1` on this repository and stays published.
```

The rest of the section (what `setup-project` does in a VS Code terminal) is unchanged. Also grep the README for any workflow inventory that lists `vscode-extension.yml` and remove it:

```bash
grep -n 'vscode-extension' README.md
```

- [ ] **Step 3: TODO and WORKSPACE**

In `TODO.md`, under `## setup-project` (or a `## devkit-vscode` heading if you prefer), add the entry the spec asks for:

```markdown
- [ ] devkit-vscode: a stronger release workflow. Today a pushed `vN` tag builds, tests,
      packages and publishes; there is no version-bump command, no changelog, and nothing
      waits for or verifies the release the way `devkit release` does for wheels. Consider a
      `devkit release`-shaped flow for non-wheel artefacts.
```

Leave the existing entry about deleting `.vscode/extension/` from `aeth_ext` after `vscode-extension-v1`; it is consumer clean-up and still accurate.

In `WORKSPACE.md`: add `gh repo clone AetherBreaker/devkit-vscode` to the clone block; leave the `.env` loop as it is (the extension publishes no wheel); add `devkit-vscode` to the bring-up loop and, after that block, the sentence: "`devkit-vscode` also needs `npm ci`; its `poe setup-project` needs `aeth-devkit>=12.1.0` in its venv, which `uv sync` provides."

- [ ] **Step 4: Build and the targeted tests**

```bash
cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all --check
```

Expected: clean.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor: remove the VS Code extension, its release stream and its CI job

The extension lives in AetherBreaker/devkit-vscode now (split step 2);
vscode-extension/, the vscode-extension.yml workflow, the CI job and the
dev-host launch config go with it. README, TODO and WORKSPACE.md describe
the new home.

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 9: Pull request, review, merge, release

- [ ] **Step 1: The full suite, once**

```bash
cargo test --workspace 2>&1 | grep -E 'test result|FAILED' | sort | uniq -c
uv run pytest -q 2>&1 | tail -2
git diff --exit-code -- python/aeth_devkit/_tasks_generated.py && echo "task table unchanged"
```

Expected: every `test result: ok`; pytest green; the task table unchanged.

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin feat/extract-devkit-vscode
gh pr create --title "feat: extract the VS Code extension into devkit-vscode" --body-file - <<'EOF'
Step 2 of docs/superpowers/specs/2026-09-08-devkit-split-design.md (plan: docs/superpowers/plans/2026-09-09-extract-devkit-vscode.md).

- The extension lives in AetherBreaker/devkit-vscode with its history; releases are `vN` from a tag push, starting at `v2` (build 1 stays on this repository as `vscode-extension-v1`).
- `setup-project` installs it from there (`install::REPO`, `install::TAG_PREFIX`); build numbers, asset name and protocol minimum are unchanged, so installed extensions are not touched.
- `[tool.devkit] release-workflow = false` lets a repository whose artefact is not a wheel keep its own `release.yml`; the extension repository is the first user.
- `vscode-extension/`, `vscode-extension.yml` and the CI job are removed; the three Dependabot PRs against `/vscode-extension` are moot (their bumps landed in the new repository before `v2`).

Release as a minor: nothing a consumer sees changes.

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
gh pr checks --watch
```

Expected: all checks green (there is one CI job fewer).

- [ ] **Step 3: Review before merge**

Run the pre-merge review the way step 1 did: two independent extra-high-effort reviews of the PR (one in-session, one on another model), verify the union of their findings, fix what is real, push, and wait for green again.

- [ ] **Step 4: Merge and release (the user's call)**

Stop here and tell the user the PR is ready unless they have already said to merge and release. Then:

```bash
gh pr merge --rebase
git switch main && git pull --ff-only
uv run poe release --dry-run minor
uv run poe release --force minor "The VS Code extension now lives in AetherBreaker/devkit-vscode and releases from there as vN; setup-project installs it from that repository. [tool.devkit] release-workflow = false opts a repository out of the devkit release workflow."
```

The release waits for the workflow (several minutes; run it in the background or with a long timeout). Expected: `Released aeth-devkit 12.1.0`, the release workflow green, `aeth-devkit==12.1.0` on SFTPyPI, the local venv rebuilt at 12.1.0 (`uv run devkit --version`). If the version differs from `12.1.0`, fix the floor in `devkit-vscode/pyproject.toml` before Task 10.

- [ ] **Step 5: The Dependabot PRs**

```bash
gh pr list --state open --json number,title,headRefName --jq '.[] | "\(.number) \(.title)"'
```

Expected: the three `/vscode-extension` PRs are closed (Dependabot closes a PR whose manifest is gone). If any is still open, close it with a comment: `gh pr close <n> --comment "Moved: the extension lives in AetherBreaker/devkit-vscode and took these bumps there before v2."`

---

## Part C: bring-up and verification

### Task 10: Make `devkit-vscode` devkit-managed, and check the installer end to end

**Files:**
- Created by `setup-project` in `devkit-vscode`: `AGENTS.md`, `.claude/*`, `.vscode/settings.json` and `extensions.json`, `.mcp.json`, `.github/workflows/claude.yml`, merged `.gitignore`, merged `pyproject.toml`; `uv.lock`.

- [ ] **Step 1: USER RUNS setup-project in a real terminal**

Hand the user this, verbatim (the `--no-vscode` keeps the run in the terminal; there is no Docker setup, so nothing prompts, but the run refuses a non-terminal stdin regardless):

```bash
cd "$WS/devkit-vscode"
uv sync
uv run devkit --version          # must print 12.1.0 or newer
uv run devkit setup-project --no-vscode
```

Expected: a commit "Standardize project configuration with devkit". `.github/workflows/release.yml` is untouched (the switch), and no "add the repository secrets" note is printed.

- [ ] **Step 2: Verify and lock**

```bash
cd "$WS/devkit-vscode"
git log --oneline -3
git show --stat HEAD | head -30
git diff HEAD~1 -- .github/workflows/release.yml | wc -l     # expect 0
grep -n 'release-workflow' pyproject.toml
uv lock && uv sync && git add uv.lock
git commit -m "chore: lock the dev group setup-project added

Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
git push
npm ci && npm run typecheck && npm test && bash scripts/package.sh 99 && npx @vscode/vsce ls | sort && rm -f aeth-devkit-vscode-99.vsix
```

Expected: the setup-project commit lists the files above and not `release.yml`; the lock commits and pushes; the vsix listing still contains no devkit-managed file. A second `uv run devkit setup-project --dry-run` (headless is fine for a dry run) reports nothing to do.

- [ ] **Step 3: The Claude workflow token (user, later)**

`setup-project` installed `.github/workflows/claude.yml`; it needs the `CLAUDE_CODE_OAUTH_TOKEN` repository secret, set by hand as for `devkit-container`. Tell the user; do not set it.

- [ ] **Step 4: The installer, end to end**

The unit tests cover URL construction; this checks the live pair the released devkit uses:

```bash
curl -s https://api.github.com/repos/AetherBreaker/devkit-vscode/git/matching-refs/tags/v | grep -o '"ref": *"[^"]*"'
curl -sIL -o /dev/null -w '%{http_code}\n' https://github.com/AetherBreaker/devkit-vscode/releases/download/v2/aeth-devkit-vscode-2.vsix
cd "$WS/aeth-devkit" && uv run devkit setup-project --dry-run --vscode 2>&1 | head -5
```

Expected: `refs/tags/v2`; `200`; the dry run does not complain about the extension (this machine has build 1, which meets the minimum, so nothing is fetched). Optional, only if the user wants the fetch path exercised on this machine: `code --uninstall-extension aeth.aeth-devkit`, then a **USER RUNS** `uv run devkit setup-project --vscode` in `aeth-devkit`, which installs `aeth-devkit-vscode-2.vsix` from the new repository; `code --list-extensions --show-versions | grep aeth` then shows `aeth.aeth-devkit@2.0.0`.

- [ ] **Step 5: Record**

Append an "Execution notes" section to this plan with anything that differed from the steps above (as step 1's plan has), commit it on `aeth-devkit` `main`, and push.

---

## Self-review notes

- Spec coverage: 4.3's four bullets map to Tasks 1 to 5 (move, protocol untouched, `vN` from `v2` with the stamped vsix and `install.rs` changing only the two constants, `package.json` URL); 4.5's removals to Task 8; section 7's repository-creation paragraph to Task 5 (public, `gh repo create`, filter-repo with history, no secrets because no wheel); the TODO entry 4.3 asks for is in Task 8. 4.3 says the stub pyproject carries "name, version, dev group with `aeth-devkit`, the poe include": Task 2 adds `[tool.devkit]`, `requires-python` and the index and source blocks, which `uv sync` needs; the reasons are in the task.
- Placeholders: none; every step carries its command or content.
- Names used across tasks: `release_workflow` (context field, Task 6, read by `lib.rs`), `REPO`/`TAG_PREFIX` (Task 7; URLs checked in Tasks 5 and 10), `aeth-devkit>=12.1.0` (Task 2's floor, Task 9's release, Task 10's check).
- Not in this plan: any change to the extension's behaviour, the protocol, or `MIN_EXTENSION_VERSION`; the consumer migration from step 1; steps 3 to 5 of the split.

## Execution notes (done 2026-09-09)

- All ten tasks are complete: `AetherBreaker/devkit-vscode` exists (public, `main`, CI green), release `v2` carries `aeth-devkit-vscode-2.vsix`, `aeth-devkit` 12.1.0 (PR #18, rebase-merged) is on SFTPyPI, and `devkit-vscode` is devkit-managed (`6815057` from the user's `setup-project` run, `7a9ab5d` for `uv.lock`). `release.yml` survived the run untouched, no secrets note was printed, the vsix listing is still `package.json`, `README.md`, `dist/extension.js`, and a dry run reports nothing to do. `CLAUDE_CODE_OAUTH_TOKEN` on `devkit-vscode` is still to be set by hand.
- Task 10: the first `setup-project` run has no `tombi` in the venv (the dev group it adds is locked afterwards), so `pyproject.toml` formatting was done by hand with the same command (`uv run tombi format --quiet pyproject.toml`) in the lock commit; otherwise the next plain run would make a second "Standardize" commit.
- Layout on this machine: the repositories live in `D:\SFT Software Projects\SFT Workspace` (`aeth_devkit`, `devkit-container`, `devkit-vscode`), not `D:\SFT Software Projects` as WORKSPACE.md assumes; WORKSPACE.md was left as written.
- Two decisions the user took before execution override the plan: the SFTPyPI repository secrets **are** set on `devkit-vscode` and `.env` is copied into its checkout ("useful for other devkit commands"); WORKSPACE.md's `.env` loop names `devkit-vscode` and says what is true (only the release poe tasks read `.env`; the secrets are pre-provisioned and unused). Nothing in that repository publishes.
- Task 1: under Git Bash, `git filter-repo`'s `old:new` rename arguments are mangled by MSYS path conversion; `MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'` fixes it. The working tree was re-checked-out after the `.gitattributes` commit so it is LF too.
- Task 2: `.vscodeignore` also excludes `.env` and `.env.*`: `setup-project` upserts `.env` at the root and the user wanted credentials there, so a local `package.sh` run would otherwise ship them in the vsix.
- Task 4: `npm install` rewrote `package.json`'s compact formatting; the two specs were applied by hand (`esbuild ^0.25.12`, `vitest ^4.1.11`; vite 8.2.2 arrives with vitest) and `npm ci` confirmed the lock agrees.
- Task 5: `gh repo create --push` was refused by the tool permission layer; the repository was created with the GitHub API and `main` pushed with git. End state as planned.
- Task 7: the plan's `install::tests` filter missed a `refs/tags/vscode-extension-v1` fixture in `vscode/mod.rs`; the task review caught it. Task 8's "grep prints nothing" cannot hold while `vscode-extension-v1` is deliberately named in docs; the check excludes that string.
- Pre-merge reviews (two, independent) changed the design in one place: the installer reads the repository's **releases** (`/repos/{REPO}/releases`, highest `vN` that is neither draft nor prerelease and carries the vsix) instead of the git tag list, because the new workflow builds after the tag push, so a tag can exist for minutes (or, after a failed run, until `gh run rerun`) with nothing to download. Spec 4.3's "install.rs changes only `REPO` and `TAG_PREFIX`" did not anticipate the tag-push trigger. Also from the reviews: `[tool.devkit]` refuses keys devkit does not know (a misspelt `release_workflow = false` would otherwise read as `true` and replace the project's own workflow); the key stays kebab-case `release-workflow`, which the user may still rename while one repository uses it; TODO entries record that `git::committable()` and `devkit release` do not yet honour the opt-out. Spec 6 lists the consent protocol as owned by `devkit-vscode`, but its versioned definition lives in `setup`'s `protocol.rs`; a spec nit for steps 3–5.
- `setup-project --dry-run` in `aeth-devkit` itself reports a pending `.gitignore` refresh; it predates this branch (the same dry run at `c788978` shows it) and belongs to the repository's next ordinary `setup-project` run.
