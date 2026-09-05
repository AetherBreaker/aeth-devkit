# TODO

Item tracking for `aeth-devkit`. Keep entries short; link the spec when a
design exists. Check items off in place; delete them once released.

## setup-project

- [ ] Sister-project Docker migration (after the first devkit release that ships
      `devkit-container`): in each of aeth_ext, IMAPReportCollector, ScheduledInvoiceProcessor,
      ScheduledReportAggregator — add `[tool.docker].services = ["<service>"]`, run
      `poe setup-project` (answer `replace` for the Dockerfile, review the compose diff),
      fold `chown_paths` into `required_persisted_dirs`, delete `chown_paths`/`mkdirs`,
      delete `docker/entrypoint.sh` and `docker/scripts/`, then `poe docker-pin`.
      ScheduledInvoiceProcessor and ScheduledReportAggregator first move `file_holding` /
      `timeclock_playground` to temp dirs (on their own TODO lists, high priority).
- [ ] IMAPReportCollector: `[tool.docker].mkdirs = [""]` is a data bug (would have chowned
      `/app`); goes away with the migration above.
- [x] `release.rust.template.yml` builds `aeth-devkit-container` only behind the
      `if-container-crate` gate (`crates/aeth-devkit-container/Cargo.toml` present).
- [x] `if-docker` conditional marker for template tables (mirrors `if-dep`; drives the
      `[tool.docker]` item above). Done on `feat/agent-config`.
- [ ] Vendored gitignore refresh: a `poe` task or script that re-fetches
      `Python.gitignore` / `Rust.gitignore` from GitHub into the templates.
- [ ] Consider a `--python-dir` override for projects whose Python package is neither in
      `src/` nor `python/`.

## Release / packaging

- [ ] Release 7.0.0 (`aeth-devkit`), then migrate downstream projects per README.
- [ ] The VS Code extension design adds a `vsix` job to the Rust release workflow's matrix.
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

- [ ] **Auto-commit `stop-ruff`'s safe fixes** — `stop-ruff` (`crates/aeth-devkit-hooks/src/stop.rs`)
      runs `ruff check --fix` and leaves whatever it changes uncommitted and unreported: a
      clean fix exits 0, so the hook says nothing and the diff just sits in the tree until
      someone notices `git status`. Auto-commit those changes instead. Constraints found
      while scoping this:
  - The commit must happen inside the same invocation that ran `--fix`, before the
    pass/fail branch — `stop_hook_active` skips the whole hook on a continued turn, so a
    turn where ruff still had unfixable complaints would never get a later chance to
    commit the fixes it already made.
  - On `main`/`master`, `scope()` runs project-wide, so the commit must stage only the
    paths ruff actually touched (diff `git status` around the `--fix` call, or otherwise
    track ruff's fixed-file list) — never a blanket `git add -A`/`git commit -a`, since
    the tree can hold unrelated uncommitted work (a design doc mid-edit, etc.) at Stop
    time that must not get swept in.
  - `stop-pyright` never fixes anything (report-only) and `stop-clean` only deletes
    generated files, so neither is in scope for this — only `stop-ruff` applies.
- [ ] `uv run ruff format python` — `python/aeth_devkit/__init__.py` has pre-existing
      formatting drift now visible with the inlined ruff config.
- [ ] IMAPReportCollector: `tool.coverage.run.source_pkgs` still lists
      `scheduled_invoice_processor` (copy-paste leftover); remove after `setup-project`
      unions in the correct name.
- [ ] Rename remaining `master` default branches if desired: `ScheduledReportAggregator`,
      `apscheduler-stubs`.
