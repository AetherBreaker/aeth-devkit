# `devkit release` — design spec

Status: READY FOR IMPLEMENTATION

## Purpose

Port `python/aeth_devkit/scripts/release.sh` to Rust as `devkit release`, in its own crate
`aeth-devkit-release`, following the workspace conventions from
`2026-08-26-devkit-workspace-and-lock-design.md`. The port fixes known defects in the script,
reorders the steps so remote mutation happens last, and turns the ad-hoc boolean rollback
into an explicit undo journal.

Out of scope: `rescind-release.sh` and `docker-pin-latest.sh` (they stay as shell scripts;
the shared remote-cleanup primitives added to `aeth-devkit-core` in this pass are written so
`rescind-release` can reuse them later).

### Learning requirement

The Rust migration exists so the author can learn Rust. All Rust written for this pass is
**densely commented as teaching material**: comments around nearly every line explaining
both the Rust syntax in use (ownership, borrowing, `?`, `Option`/`Result`, enums with data,
traits and trait objects, closures, iterators, lifetimes, `impl` blocks, pattern matching)
and the logic the line performs, plus *why* an idiom was chosen over the alternative.
`///` doc comments for items; `//` line comments inside bodies. This applies to test code
too. Code review of this pass should reject under-commented files.

## Defects in the current script that this pass fixes

| # | Defect | Fix |
| --- | --- | --- |
| 1 | `sed 's/^version = …/'` never matches this repo's indented `Cargo.toml`; `Cargo.toml` is still `7.0.0` while pyproject is `7.0.2`. | Edit with `toml_edit` (`workspace.package.version`, else `package.version`). Pre-flight refuses if Cargo and pyproject versions disagree. |
| 2 | `git commit -m` commits the whole index, sweeping in unrelated staged files when the tree is dirty. | `git::commit_paths` (commits only the named files). |
| 3 | Rollback `git reset HEAD~1` is unguarded. | Reset only if `HEAD` equals the recorded bump-commit SHA. |
| 4 | Rollback `git push --force` can clobber a concurrent push. | `git push --force-with-lease=<branch>:<bump-sha>`. |
| 5 | Bump mode silently `DELETE`s any pre-existing devpi version. | Existence report + typed `force` confirmation (see Pre-flight §6). |
| 6 | Push happens before build/publish, so a local build failure force-pushes the remote. | Reorder: build and commit locally first; push and GitHub release last. |
| 7 | devpi URL and env-var names hard-coded. | Derived from `[[tool.uv.index]]`. |
| 8 | Trailing `uv sync --all-extras` needed because `uv sync` after the bump stripped extras. | Use `uv lock` (does not touch the venv); no trailing sync. |

## CLI

```text
devkit release [OPTIONS] [BUMP]... ["multi-word notes"]

Arguments:
  [BUMP]...   Leading positionals from: major minor patch stable alpha beta rc post dev.
              None → publish the current version without bumping.
  notes       Remaining positionals after the bump words (see heuristic).

Options:
  -f, --force        Answer both confirmation prompts as if `force` had been typed.
      --dry-run      Run every pre-flight check, print the existence report and the plan,
                     mutate nothing (no files, no commits/tags/index, no network writes).
                     The one read-flavoured side effect is the `git fetch` that the
                     behind-upstream check needs (it refreshes remote-tracking refs).
      --index NAME   `[[tool.uv.index]]` to publish to. Default: the single index that has
                     a `publish-url`; error if none or more than one.
      --root DIR     Project root (default `.`).
```

### Notes heuristic (unchanged from the script)

Strip `--force`/`-f`. Walk the positionals: while the word is a bump type **and no
non-bump word has been seen yet**, it is a bump; everything from the first non-bump word on
is the tail.

- 0 tail words → no notes (`gh release create --generate-notes`).
- 1 tail word containing a space → notes (shell quoting preserved).
- 1 tail word without a space → error: `'x' is not a valid bump type … (Notes must be
  multiple words.)`
- 2+ tail words → joined with single spaces as notes.

Implemented in a pure function `parse_positionals(&[String]) -> Result<Parsed>` so it is
unit-testable without clap. Clap collects all positionals into one plain `Vec<String>`
(recognized flags like `--force`/`--dry-run` still parse wherever they appear on the
line), and `run` calls the parser.

### Index and credentials

`--index NAME` (or the default described above) selects a `[[tool.uv.index]]` entry.

- Publish URL: its `publish-url` (required; error if missing).
- devpi REST URL for a version: `<publish-url without trailing '/'>/<package>/<version>`
  (matches today's hard-coded `…/internal/<pkg>/<ver>`).
- Credentials: `UV_INDEX_<NAME>_USERNAME` / `UV_INDEX_<NAME>_PASSWORD`, where `<NAME>` is
  the index name upper-cased with `-` → `_` (uv's own convention, so `uv publish --index
  NAME` finds the same variables). Both are required; missing ones are listed in one error.
- Package name: `[project].name` from pyproject, PEP 503-normalized for the devpi URL and
  reported as-is elsewhere.

## Pre-flight (read-only; both modes; all of it runs under `--dry-run`)

1. **Tools**: `git`, `uv`, `gh` respond to `--version`. Missing → one error listing them.
2. **Config**: index + credentials resolved as above; `pyproject.toml` parses.
3. **Branch**: current branch has an upstream (`@{u}`); `git fetch --quiet origin`; local
   is not behind upstream (`git rev-list --count HEAD..@{u}` = 0). Behind → error telling
   the user to pull/rebase first.
4. **Target version**: bump mode → `uv version --bump A [--bump B …] --dry-run`, parse the
   last whitespace-separated token of the output line (`name old => new`). No-bump mode →
   `uv version`, parse the same way (`name ver`). Prints `Releasing <pkg> <old> → <new>`.
5. **Cargo.toml** (if present): version read via `toml_edit` must equal pyproject's
   *current* version. Mismatch → error naming both values. (Prevents publishing a wheel
   whose `--version` disagrees with the Python package.)
6. **Dirty tree**: `git status --porcelain` non-empty → print `git status --short`, then
   prompt `Continue with a dirty tree? Type 'force' to continue:`. Anything but `force` →
   abort (exit 1). `--force` skips the prompt.
7. **Existence report**: probe the target version everywhere, then print a table:

   ```
   Existing artefacts for v7.0.3:
     local tag       v7.0.3 -> a1b2c3d (annotated)
     remote tag      none
     GitHub release  https://github.com/…/releases/tag/v7.0.3
     devpi           aeth-devkit==7.0.3 on SFTPyPI
   ```

   Probes: `git rev-parse --verify refs/tags/vX^{commit}`; `git ls-remote --tags origin
   refs/tags/vX`; `gh release view vX --json url`; `GET <devpi url>` with basic auth
   (200 = exists, 404 = absent, anything else = error). If everything is `none`, continue.
   Otherwise prompt `Remove these and continue? Type 'force':`. Typed `force` (or
   `--force`) → remove what exists, in this order: GitHub release (`gh release delete vX
   --yes --cleanup-tag`, which also removes the remote tag), remote tag (if still present),
   devpi version (`DELETE`), local tag. Commits are **never** rewound here — that is
   `rescind-release`'s job. Under `--dry-run` the report is printed and the removals are
   listed as "would remove". Removals performed here are *not* journaled: they were
   explicitly requested and the artefacts are stale by definition.

Under `--dry-run`, after the report the command prints the ordered plan (the step table
below with concrete values) and exits 0.

## Forward steps and undo journal

Each step, on success, pushes an `Undo` onto a `Vec<Undo>`. On the first `Err` the journal
is unwound in reverse. All steps run with inherited stdio so `uv`/`git`/`gh` output is
visible, except where stdout is captured for parsing.

| # | Step | Mode | Pushes |
| --- | --- | --- | --- |
| 1 | Snapshot `pyproject.toml`, `uv.lock`, `Cargo.toml`, `Cargo.lock` (each if present, remembering which were absent) and `dist/*.whl`, `dist/*.tar.gz` into a `tempfile::tempdir()`. | both | `RestoreFiles` |
| 2 | `uv version --bump …` (real); set Cargo.toml version via `toml_edit`; `cargo update --workspace --quiet` when `Cargo.lock` exists (its failure is an error, not swallowed). | bump | — (covered by 1) |
| 3 | `uv lock` | bump | — |
| 4 | Delete `dist/*.whl`, `dist/*.tar.gz`; `uv build`. | both | — |
| 5 | Commit only the release's own edits to `pyproject.toml uv.lock Cargo.toml Cargo.lock`, message `Bump version to <new>`: step 2 first reset any dirty managed file to its `HEAD` content so the tools ran on clean input; the bumped bytes are committed through a scratch index (`GIT_INDEX_FILE`), never `git add`; then the HEAD→bumped delta is three-way merged (`git merge-file`) back onto the user's working-tree copy and onto their staged copy, so their edits survive and `git status` afterwards shows exactly what it showed before. Overlapping edits are an error (→ rollback). The commit SHA is recorded once; the push step reuses it. | bump | `ResetCommit { bump_sha, pre_sha, index }` |
| 6 | `git tag -a v<new> -m "Version <new>"` | both | `DeleteLocalTag` |
| 7 | `uv publish --index NAME` (credentials via the inherited `UV_INDEX_<NAME>_USERNAME/_PASSWORD`, never argv). On a non-zero exit, probe the index and queue `DeleteDevpi` if anything landed (partial wheel/sdist upload). | both | `DeleteDevpi` |
| 8 | bump: `git push --atomic origin <branch> v<new>` (both refs or neither). no-bump: `git push --atomic origin v<new>`. | both | `DeleteRemoteTag`, plus `ForcePushBranch { branch, sha }` in bump mode |
| 9 | `gh release create v<new> dist/* --title v<new>` with `--notes "<notes>"` or `--generate-notes`. | both | `DeleteGithubRelease` |

Success: remove the snapshot dir, print `Released <pkg> <new>` and the GitHub URL.

### Undo semantics

```rust
enum Undo {
  RestoreFiles(Snapshot),                   // copy back; delete files that were absent
  ResetCommit { bump_sha, pre_sha, index }, // git reset --soft pre_sha, then restore the pre-run index entries (mode+blob) of the managed files; only if HEAD == bump_sha
  DeleteLocalTag(String),                   // git tag -d
  DeleteDevpi { package, version },         // DELETE <devpi url>; 200/204/404 all fine
  DeleteRemoteTag { tag, expected },        // lease-guarded: push :refs/tags/vX with --force-with-lease=refs/tags/vX:<expected tag object>
  ForcePushBranch { branch, sha },          // git push --force-with-lease=<branch>:<sha> origin <pre-bump-sha>:<branch>
  DeleteGithubRelease(String),              // gh release delete vX --yes --cleanup-tag
}
```

- Unwinding runs **every** entry even if earlier ones fail; each failure is collected as
  `(what, manual command)`.
- `DeleteGithubRelease` with `--cleanup-tag` already removes the remote tag, so a
  subsequent `DeleteRemoteTag` treats "tag not found" as success.
- `ForcePushBranch` pushes the *pre-bump* SHA (recorded before step 5) to the branch with
  a lease on the bump SHA. Unwinding walks the journal backwards, so it runs *before*
  `ResetCommit` (the remote is rewound first, then the local branch); both end up equal.
- `ResetCommit` refuses (and reports) if `HEAD != sha`; `RestoreFiles` then still restores
  the working files, which is safe because it only touches the four versioned files and
  `dist/`.
- `ResetCommit` uses `--soft` and then puts back the exact pre-run index entries of the
  managed files (recorded before step 2), so the user's staged state — in those files and
  everywhere else — is byte-identical after a rollback.
- If `RestoreFiles` itself fails, the snapshot directory is kept (not deleted on drop) and
  the manual command tells the user to copy from it — `git checkout` would restore `HEAD`,
  not the pre-run tree, and would not restore `dist/`.
- `push_refs` uses `git push --atomic`, so the branch and tag land together or not at all.
  A *failed* push is still not proof the remote stayed put (the response can be lost after
  the server applied it), so the failure path probes both remote refs with `ls-remote` and
  queues `DeleteRemoteTag` / `ForcePushBranch` for whatever actually landed — assuming the
  worst when the probe itself fails. Existence is not ownership: the tag is compensated
  only when the remote tag *object id* matches the tag this run created, the branch only
  when the remote points at our bump commit, and both compensations carry leases so a
  concurrently replaced ref is refused rather than destroyed. The same principle guards
  devpi: after a failed `uv publish`, the stored release files are byte-compared against
  this run's `dist/` artifacts, and `DeleteDevpi` is queued only when they are ours (an
  unanswerable probe still assumes the worst). Likewise `gh release create` failures are
  followed by a `gh release view` probe so a release that was created but failed asset
  upload still gets `DeleteGithubRelease` queued.
- After unwinding: print `Rollback complete.` if nothing failed, else a block
  `Manual cleanup required:` listing each failed step's copy-pasteable command. Exit code
  1 either way (the release failed); the original error is printed first.

### Interrupts

`ctrlc` crate handler sets an `AtomicBool`. Child processes share the console and receive
the interrupt themselves, so they exit non-zero and the normal `Err` path rolls back. The
flag is also checked before each step so an interrupt during a pure-Rust step (snapshot,
Cargo.toml edit) aborts with `Interrupted` and unwinds. Pressing Ctrl-C again during
rollback is ignored (the handler stays installed); rollback always runs to completion.

## Code layout

```text
crates/aeth-devkit-release/
  Cargo.toml          lib aeth_devkit_release; dev bin devkit-release; deps: core, anyhow, clap, toml_edit, tempfile, ctrlc
  src/main.rs         Args::parse() → run_real → exit code
  src/lib.rs          Args, run(args, &Deps), run_real; top-level orchestration
  src/args.rs         parse_positionals — bump/notes heuristic (pure)
  src/config.rs       index selection, credentials, devpi URL, package name
  src/preflight.rs    tools, branch, committed-config check, version, Cargo check, unmerged/dirty-tree, existence probes
  src/report.rs       existence-report rendering (pure)
  src/steps.rs        forward steps 1–9
  src/undo.rs         Undo enum, unwind(), manual-command rendering
  src/snapshot.rs     file/dist snapshot + restore
  tests/release.rs    integration tests (real git in tempdir + scripted runner + stub devpi)
```

`Deps` bundles the injectable collaborators: `&dyn Runner`, `&dyn DevpiClient`,
`&dyn Prompt` (reads a line from stdin; test impl returns scripted answers), and an
interrupt flag.

### `aeth-devkit-core` additions (reusable by a future `rescind-release`)

- `process`: `Runner::run_capture(program, args, cwd) -> Result<Output>` (exit code +
  stdout + stderr as `String`). `RecordingRunner` gains scripted responses: a
  `Vec<Script>` where each entry is `{ program, arg_prefix, code, stdout, stderr }`,
  matched by program plus argument prefix. The most recently registered match wins and
  scripts are never consumed (one script answers any number of calls), so tests register
  broad defaults first and override later; unmatched calls answer with a default exit
  code and empty output. A `fail_at(n)` convenience fails the Nth call regardless.
- `devpi.rs`: `trait DevpiClient { fn exists(url, user, pass) -> Result<bool>; fn delete(url, user, pass) -> Result<DeleteOutcome> }`, `HttpDevpiClient` (ureq basic auth), `StubDevpiClient` (records calls, scripted `exists` answers).
- `pyproject.rs`: `project_name(doc) -> Result<String>`, `publish_index(doc, name: Option<&str>) -> Result<PublishIndex { name, publish_url }>`.
- `cargo_toml.rs`: `read_version(&str) -> Option<String>`, `set_version(&mut DocumentMut, &str) -> bool` targeting `workspace.package.version` then `package.version`.
- `git.rs`: `head_sha`, `current_branch`, `upstream_behind_count`, `fetch`, `status_porcelain`, `tag_exists`, `tag_target`, `remote_tag_exists`, `create_annotated_tag`, `delete_tag`, `push_refs`, `delete_remote_tag`, `force_push_with_lease`, `reset_mixed_to`. All are thin `git` wrappers with captured output; errors carry stderr. The ones that
  can reach a remote (`fetch`, `upstream_behind_count`, `remote_tag_exists`, `push_refs`,
  `delete_remote_tag`, `force_push_with_lease`) take a `&dyn Runner` so tests can script
  them; the purely local ones call `git` directly like the existing helpers.

### Dispatcher, poe, docs

- `crates/aeth-devkit/src/main.rs`: `Release(aeth_devkit_release::Args)` subcommand.
- `python/aeth_devkit/__init__.py`:
  - `release`: `cmd = 'devkit release $POE_EXTRA_ARGS'`, `envfile: .env`, and *no*
    declared args: poe would otherwise reject the free positionals. `--force`/`-f` and
    `--dry-run` travel through `$POE_EXTRA_ARGS` verbatim (poe expands it in place for
    `cmd` tasks, preserving quoted multi-word notes) and clap parses them wherever they
    appear.
  - `release-and-pin`: `shell` task with `interpreter: bash`:
    `devkit release $POE_EXTRA_ARGS && bash "…docker-pin-latest.sh" "$(uv version --short)"`.
- Delete `python/aeth_devkit/scripts/release.sh`.
- README table row for `poe release` → `devkit release`; TODO: check off `release.sh`.
- Bump workspace version alongside the next release (the pre-flight Cargo check will
  otherwise refuse, since Cargo.toml is currently stale at 7.0.0 — the first run must be
  preceded by hand-fixing Cargo.toml to match pyproject).

## Testing

Unit (in-crate):
- `args::parse_positionals`: every branch of the heuristic, including `--force` stripping,
  bump words after a tail word being treated as notes, and the single-word error text.
- `report`: rendering with all-none, single leak, and everything-present inputs.
- `undo`: unwinding order is reverse of push order; a failing entry does not stop later
  ones; manual-command block lists exactly the failures; `ResetCommit` refuses on SHA
  mismatch.
- `config`: default index selection (one publish-url / none / two), env-var name mapping
  (`SFTPyPI` → `UV_INDEX_SFTPYPI_USERNAME`), devpi URL join.
- `cargo_toml::set_version` on an indented `[workspace.package]` table and on a plain
  `[package]`.
- `snapshot`: round-trip restores contents; absent files are deleted on restore; dist
  artefacts restored and new ones removed.

Integration (`tests/release.rs`, real git via `init_test_repo`, scripted `RecordingRunner`
for `uv`/`gh`, `StubDevpiClient`, scripted `Prompt`):
- Happy path in bump mode: assert the exact sequence of external invocations and that the
  repo ends with the bump commit, annotated tag, and updated files.
- Failure injected at each of steps 4–9: assert the compensating calls emitted (and their
  order) and that the repo is byte-identical to the pre-run state (`HEAD`, tags, the four
  files, `dist/`).
- No-bump mode happy path and a leaked-local-tag pre-flight with prompt answer `force`
  (tag deleted, run proceeds) and with answer `no` (abort, nothing touched).
- `--dry-run` produces no invocations beyond the read-only probes and no file changes.
- Git operations that the runner cannot fake (commit, tag, reset) are executed for real
  against the temp repo; remote operations (`push`, `ls-remote`, `fetch`) go through the
  runner so no network is needed. Therefore `git::*` helpers take the `Runner` where the
  call may reach a remote, and use `std::process::Command` directly where it is purely
  local. (The existing `commit_paths` etc. stay direct.)
- No live network or `gh` in tests.
