# TODO

Item tracking for `aeth-devkit`. Keep entries short; link the spec when a
design exists. Check items off in place; delete them once released.

## setup-project

- [ ] Label the entries setup-project owns inside partially-managed files. The merges that
      add without overwriting (`.claude/settings*.json` hooks and permissions, `.mcp.json`
      servers, `.vscode/settings.json` keys, `launch.json`/`tasks.json` configurations)
      cannot tell a template entry from the project's own, so an entry the template drops
      stays in every project for good (the `PreToolUse` hooks, b90d71a). Mark what the
      run writes, entry by entry, in whatever the format allows (the `# setup-project:`
      markers in `pyproject.toml` and the `AGENTS.md` block are the precedents), so a run
      can update a managed entry, remove one that has left the template, and leave an
      unmarked one alone.
- [ ] `template.env` writes `PYTHONPYCACHEPREFIX` unquoted; the value has spaces and
      backslashes, which poe's envfile loader accepts but uv's `--env-file` parser
      rejects (`Failed to parse environment file .env at position 4`, the rest of the
      file still loads). Quote the value so both readers agree.
- [ ] Completion shell detection follows the launching shell's `PATH`: from PowerShell,
      Git Bash's `bash.exe` (`Gitin`, not on `PATH`) is not seen, so bash completion is
      only installed from a Git Bash run, silently. Probe Git's install directory too, or say
      which shells were skipped.
- [ ] Marker validation and stripping reach a brand-new table's top-level keys only; a
      `# setup-project:` marker nested inside a sub-table of a table the project lacks, or
      inside a template array, would ship into the project's file. No template line does
      this today.
- [ ] Sister-project Docker migration (after the aeth-devkit major that lands the
      `devkit-container` split, PR #16): in each of aeth_ext, IMAPReportCollector,
      ScheduledInvoiceProcessor, ScheduledReportAggregator — add
      `[tool.docker].services = ["<service>"]`, commit it, `poe lock` (takes the new devkit),
      `poe setup-project` (adds `devkit-container` to the lock and venv; answer `replace` for
      the Dockerfile, review the compose diff), fold `chown_paths` into
      `required_persisted_dirs`, delete `chown_paths`/`mkdirs`, delete `docker/entrypoint.sh`
      and `docker/scripts/`, then `poe docker-pin` and push. Coolify redeploys on the
      `docker/` change; watch the first build of each pull `devkit-container` from SFTPyPI.
      ScheduledInvoiceProcessor and ScheduledReportAggregator first move `file_holding` /
      `timeclock_playground` to temp dirs (on their own TODO lists, high priority).
- [ ] IMAPReportCollector: `[tool.docker].mkdirs = [""]` is a data bug (would have chowned
      `/app`); goes away with the migration above.
- [ ] Dockerfile customisation: `docker/Dockerfile` is devkit-owned and `docker-pin` replaces
      drift without asking, so a hunk kept in `setup-project` does not survive the next pin.
      Consider a mechanism for project-specific Dockerfile edits, or a `[tool.devkit]` setting
      that opts a project out of Dockerfile management.
- [x] `if-docker` conditional marker for template tables (mirrors `if-dep`; drives the
      `[tool.docker]` item above). Done on `feat/agent-config`.
- [ ] Vendored gitignore refresh: a `poe` task or script that re-fetches
      `Python.gitignore` / `Rust.gitignore` from GitHub into the templates.
- [ ] Consider a `--python-dir` override for projects whose Python package is neither in
      `src/` nor `python/`.

## Release / packaging

- [ ] **Complete-release rule for every version check** (policy agreed 2026-09-09; design it
      with full attention before implementing). A version counts as released only when all
      three indicators exist: the git tag on the remote, the GitHub release, and the package on
      SFTPyPI (the last only for targets that publish there). Any subset is the signature of an
      interrupted release or an interrupted rescind, so every resolver must warn that the
      version looks incomplete, name what is missing, and fall back to the newest version that
      passes all three. Sites in this repo: `docker-pin` (`crates/aeth-devkit-pin/src/resolve.rs`,
      already three-way but errors on a tag without a release instead of falling back), the
      VS Code extension installer (`crates/aeth-devkit-setup/src/vscode/install.rs`, reads
      releases with the asset, no tag check, no index), `devkit lock`
      (`crates/aeth-devkit-lock`, index only), the `{latest}` package step
      (`crates/aeth-devkit-setup/src/packages.rs`, uv chooses; steer it with a
      `<=newest-complete,!=incomplete` specifier on `--upgrade-package`), the update nag
      (`crates/aeth-devkit-core/src/update.rs`, index only, cached), and `devkit release`'s
      post-run verification. Open design points: how devkit learns a package's GitHub
      repository (a fixed table of its own packages vs. `[project.urls]` metadata; `docker-pin`
      uses the project's origin), `gh` vs. anonymous HTTP for the GitHub side, and whether an
      explicit `--version` that is incomplete errors (today) or warns and falls back. Other
      repos: `devkit-vscode` (tag push then release; no index), `devkit-container` and the
      later wheel repos (all three indicators), the sister projects' `docker-pin` runs that
      consume them, and `rescind-release.sh` (a rollback must remove all three, in the reverse
      order, so a half-rescind is caught the same way).
- [ ] Release 7.0.0 (`aeth-devkit`), then migrate downstream projects per README.
- [ ] **TUI for the release watch** (shelved 2026-09-04; work committed, unpushed, on
      `feat/release-watch-repaint`). That branch dropped `gh run watch` for our own column view
      (`watch.rs`) repainted in the terminal's normal buffer (`repaint.rs`), which sidesteps the
      alternate screen having no scrollback but keeps two compromises of our own: a frame must
      stay shorter than the terminal, and one that does not fit is appended rather than
      repainted. Full-screen ratatui removes both outright, and is worth more than this one
      view — a real UI layer is a toolbox for tools a line-oriented CLI cannot do.
  - Shape: keep `watch::{view, frame, failures}` and their tests as they are. `frame` already
    produces the lines, so ratatui only adds a scrollable `Paragraph` + `Scrollbar` over them and
    the non-TTY path keeps printing exactly what it prints today. `repaint.rs` is deleted.
  - Cost, measured against the branch: about +30 production lines and +10 test (-101/-55 for
    `repaint.rs`; +40 lifecycle and restore guard, +70 event loop, +20 scroll state).
  - **Do not use `ratatui::init*` or `crossterm::enable_raw_mode`.** Both clear
    `ENABLE_PROCESSED_INPUT` (Windows) / `ISIG` (Unix), so Ctrl-C stops reaching the handler in
    `aeth-devkit-release/src/lib.rs` that sets `INTERRUPTED` — the rollback's trigger. Clear only
    `ENABLE_LINE_INPUT | ENABLE_ECHO_INPUT` / `ICANON | ECHO` (~20 lines, two `cfg` branches) and
    signals survive. This matters most while the loop is blocked in `gh run view`: a key event
    would not be read until that returns, a signal fires regardless.
  - New failure class to guard: a shell left in alt-screen/raw modes. `Drop` guard plus a panic
    hook; a `SIGCONT` re-assert for ^Z on Unix (does not arise on Windows).
  - Open decisions: whether the mode setting lives in a core `term` module or release-local; a
    key map where `q` (detach) is visibly distinct from Ctrl-C (cancel and roll back); whether a
    finished run stays in the alt screen or drops back with a summary in the normal buffer.
- [ ] setup-project: when VS Code does not pick up a consent request (no ack within 5 s)
      the run falls back to the typed terminal prompt; a terminal diff viewer built on the
      shelved release-watch TUI could take that fallback instead (idea raised 2026-09-06).
- [ ] setup-project: the VS Code diff (and the terminal consent behind it) covers only the
      Docker files; every change setup-project proposes (pyproject, gitignore, launch/tasks,
      ...) was meant to go through the same review (raised 2026-09-06).
- [ ] VS Code extension: support `code-insiders` and `cursor` launchers (each has its own
      URI scheme, `argv.json` location and extensions dir); only `code` works today.
- [ ] devkit-vscode: a stronger release workflow. Today a pushed `vN` tag builds, tests,
      packages and publishes; there is no version-bump command, no changelog, and nothing
      waits for or verifies the release the way `devkit release` does for wheels. Consider a
      `devkit release`-shaped flow for non-wheel artefacts.
- [ ] `[tool.devkit] release-workflow = false` is honoured by `setup-project` but not yet by the
      rest of devkit: `git::committable()` still stages `.github/workflows/release.yml` for the
      run (harmless except in a double-Ctrl-C abort, which drops uncommitted edits to it), and
      `devkit release` refuses on the missing publish step with advice to run `setup-project`,
      which cannot help there; it should say the project opts out.
- [ ] Per-key opt-out for the pyproject merge, removed 2026-09-09: `[tool.setup-project].keep`
      listed dotted key paths the merge never touched. Nothing used it, so it went with the
      bloat. If a project ever needs to hold a template-managed key, re-add it as a
      `[tool.devkit]` setting once the core merge is ironclad, not before.
- [ ] Now that `vscode-extension-v1` has shipped: delete `.vscode/extension/` and
      `install.ps1` from aeth_ext and aeth_ext-2, and the
      `~/.vscode/extensions/local.[drekker-]add-to-runtime-base-*` junction (setup-project
      prints a note while they exist).
- [ ] Fix system-level `init.defaultBranch = master` in
      `C:\Program Files\Git\etc\gitconfig` (needs an elevated shell; user config already
      overrides it to `main`).

## Script migration to Rust

Planned order (each command is its own crate under `crates/`; the `devkit` binary
dispatches):

- [x] `lock.sh` → `devkit lock` (7.0.0)
- [x] `docker-pin-latest.sh` → `devkit docker-pin`. Agreed requirements:
  - [x] **Rename** — command and poe task become `docker-pin` (it pins any version, not just
        latest); `release-and-pin` keeps its name.
  - [x] **Crate layout** — thin `crates/aeth-devkit-pin` (clap `Args` + orchestration, `run_real`
        dispatched from `devkit`); reusable pieces (compose discovery, version resolution,
        GitHub tags client, pin edit) live in `aeth-devkit-core` so a future in-process
        `release-and-pin` can call the raw functions directly (no subprocess).
  - [x] **Index config from pyproject** — kill the hardcoded SFTPyPI URL. Query *every*
        `[[tool.uv.index]]` with a `publish-url` via its simple `url` (existing `IndexClient`).
        Explicit version must exist on ALL of them (missing from any = failed release, name
        the index); latest = `latest_stable` over the *intersection* of version sets.
  - [x] **GitHub via `gh` CLI** through the `Runner` trait (`gh api ... --paginate`): picks up
        auth, no rate-limit issues, no 100-tag cap, testable with `RecordingRunner`.
  - [x] **Block-aware compose edit** — format-preserving line edit scoped to service blocks
        that build *this* project: `PACKAGE_NAME` (normalized) == `project.name`, or
        `GIT_REPO` == origin remote (normalized https/ssh/.git/case). All matching blocks
        move together, each change reported; no match = error listing what was found.
        Mode (git/pypi) follows from which match kind hits, not key order in the file.
  - [x] **Preflights before any edit** — resolve → validate → behind-check → edit → commit → push:
    - Complete-release check (mode-independent): remote tag `v<ver>` AND GitHub release AND
      present on every publish index. Index check skipped only when no publish index is
      configured; GitHub checks skipped only when origin is not a GitHub remote. Applies to
      resolved-latest as well as explicit versions; failure lists exactly what is missing.
    - Behind-origin check (`fetch` + `behind_count`) when pushing; fail before touching files.
  - [x] **Commit only the compose path** (`commit_paths`, not bare `git commit` which sweeps
        the user's staged files). Message: `chore: pin <name> to <version>`.
  - [x] **Dirty compose file** — apply the pin to the HEAD blob and commit via
        `commit_files_on_head` (user's index/worktree untouched), then write the 3-way merge
        (worktree over base + pin) back to the worktree so the user's uncommitted changes ride
        on top. Merge conflict = abort before committing anything.
  - [x] **Flags** — `--version/-V`, `--dry-run`, `--no-commit` (edit only, implies no push),
        `--no-push`, `--compose-file <path>`.
  - [x] **Compose discovery** — anchored at the git repo root; walk shallowest-first with
        Docker name precedence (`compose.yaml` > `compose.yml` > `docker-compose.yaml` >
        `docker-compose.yml`) within each directory; first hit wins (single compose file
        assumed; extend later if ever needed). Skip known-irrelevant dirs (`.git`, `.venv`,
        `.cache`, `__pycache__`, `.mypy_cache`, `.pytest_cache`, `.ruff_cache`,
        `node_modules`, root Cargo `target/`). Always print the chosen file.
  - [x] **Version handling via `pep440_rs` end to end** — explicit input accepted with or
        without `v` prefix, parsed on entry (error if unparseable); all membership checks
        (indexes, git tags) use parsed equality, not string equality; latest via
        `latest_stable` (no hand-rolled regex filters). Written form: `GIT_TAG` = `v` + the
        actual tag spelling found on the remote; `PACKAGE_VERSION` = PEP 440 normalized.
  - [x] **Poe wiring + script removal** — the poe task becomes `docker-pin` running
        `devkit docker-pin` with declared poe args mirroring the flags (lock-task
        style); delete `docker-pin-latest.sh`. README: move the command out of the
        shell-script table and add its per-command Rust feature-reference bullets.
  - [x] **Migrate `release-and-pin` in the same pass** (both constituents are then Rust):
        a thin `ReleaseAndPin` subcommand in the `devkit` binary crate composing
        `aeth_devkit_release` + pin lib in-process (no subprocesses). Release lib entry
        point grows a structured outcome (released version + released/aborted) so the
        composition knows what to pin; `devkit release` behavior unchanged. `--dry-run`
        runs release's dry-run then *skips* the pin step ("dry run: skipping docker pin" —
        an unpublished version cannot pass pin's preflights). All other args forward to
        release verbatim; the pin step runs with the released version and full preflights
        (free post-release verification). Poe task: `devkit release-and-pin $POE_EXTRA_ARGS`.
- [x] `release.sh` → `devkit release` (spec: `docs/specs/2026-08-26-devkit-release-design.md`)
- [ ] `rescind-release.sh`

## Housekeeping

- [ ] `uv run ruff format python` — `python/aeth_devkit/__init__.py` has pre-existing
      formatting drift now visible with the inlined ruff config.
- [ ] IMAPReportCollector: `tool.coverage.run.source_pkgs` still lists
      `scheduled_invoice_processor` (copy-paste leftover); remove after `setup-project`
      unions in the correct name.
- [ ] Rename remaining `master` default branches if desired: `ScheduledReportAggregator`,
      `apscheduler-stubs`.
