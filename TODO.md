# TODO

Item tracking for `aeth-devkit`. Keep entries short; link the spec when a
design exists. Check items off in place; delete them once released.

## setup-project

- [ ] Template `ci.yml` in devkit-templates and have `setup-project` install a test-running
      workflow when it detects the project runs tests: pytest for a Python project, cargo test
      for a Rust one, both for a mixed tree. Names follow the AGENTS.md workflow-naming
      convention (`Verify: ...` workflow, `<Area>: <what runs>` jobs, the command as the step).
- [ ] Label the entries setup-project owns inside partially-managed files. The merges that
      add without overwriting (`.claude/settings*.json` hooks and permissions, `.mcp.json`
      servers, `.vscode/settings.json` keys, `launch.json`/`tasks.json` configurations)
      cannot tell a template entry from the project's own, so an entry the template drops
      stays in every project for good (the `PreToolUse` hooks, b90d71a). Mark what the
      run writes, entry by entry, in whatever the format allows (the `# setup-project:`
      markers in `pyproject.toml` and the `AGENTS.md` block are the precedents), so a run
      can update a managed entry, remove one that has left the template, and leave an
      unmarked one alone.
- [ ] A committable file that is untracked but present before a committing run (a fresh
      repository's `uv.lock` after a first `uv sync`) is captured as the user's uncommitted
      state by `git::stage_bases`, so the run's version stays on disk but never reaches the
      commit, while the report still says `uv.lock: updated` (seen creating
      `devkit-templates`; every satellite needed its first lock committed by hand). Commit
      it as the managed file it is, or say it was left untracked.
- [ ] Completion shell detection follows the launching shell's `PATH`: from PowerShell,
      Git Bash's `bash.exe` (`Gitin`, not on `PATH`) is not seen, so bash completion is
      only installed from a Git Bash run, silently. Probe Git's install directory too, or say
      which shells were skipped.
- [ ] Marker validation and stripping reach a brand-new table's top-level keys only; a
      `# setup-project:` marker nested inside a sub-table of a table the project lacks, or
      inside a template array, would ship into the project's file. No template line does
      this today.
- [ ] Dockerfile customisation: `docker/Dockerfile` is devkit-owned and `docker-pin` replaces
      drift without asking, so a hunk kept in `setup-project` does not survive the next pin.
      Consider a mechanism for project-specific Dockerfile edits, or a `[tool.devkit]` setting
      that opts a project out of Dockerfile management.
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
- [ ] Fix system-level `init.defaultBranch = master` in
      `C:\Program Files\Git\etc\gitconfig` (needs an elevated shell; user config already
      overrides it to `main`).

## Script migration to Rust

The one shell script left; it becomes its own crate under `crates/` and the `devkit`
binary dispatches, like the others did.

- [ ] `rescind-release.sh`
