# Review everything but Docker

Date: 2026-09-11. Status: design approved in discussion; implementation plan next.

## 1. Why

`setup-project` reviews the wrong files. Only the Docker files go through consent (a printed diff
and a terminal question, or per-hunk Accept/Reject in VS Code); every other managed file is merged
and written silently. Docker management is meant to be all or nothing: the Dockerfile and compose
standard are devkit's, and picking hunks out of them only produces drift the next run re-proposes.
The other files are shared with the project (its own pyproject keys, settings, ignore rules), which
is where a per-hunk choice earns its keep. This swaps the two, and closes the TODO item "every change
setup-project proposes … was meant to go through the same review".

## 2. Which files, which treatment

| Treatment | Files |
| --- | --- |
| **Reviewed** | `pyproject.toml` (the template merge only), `.vscode/settings.json`, `.vscode/extensions.json`, `.vscode/launch.json`, `.vscode/tasks.json`, `.gitignore`, `.gitattributes`, `AGENTS.md`, `.claude/CLAUDE.md`, `.github/workflows/claude.yml`, `.github/workflows/release.yml`, `.claude/settings.json`, `.claude/settings.local.json`, `.mcp.json` |
| **Always written** | `docker/Dockerfile`; the compose file when created from the scaffold, and its per-service rule edits and top-level edits; `.dockerignore`; the package step's edits to `pyproject.toml` (the `devkit-templates` bootstrap, version floors) and `uv.lock`; tombi formatting; `.env` and every env file `launch.json` names |
| **Still asked** | Adding a service `[tool.docker].services` lists but the compose file lacks |

- **Env files** skip review because a diff prints the lines around each change and the VS Code
  review copies the whole file into the devkit cache; both would expose secrets. Writing them
  unreviewed is safe: the merge only sets the keys in devkit's env template and touches no other line.
- **The service add** stays asked so a typo in `services` cannot grow the compose file unseen. It
  keeps today's rules: one hunk, `-y` accepts it, `replace all` does not cover it.
- **Always-written Docker files** print no diff; their report lines say what changed, and `--dry-run`
  still shows them in the multi-diff review.

## 3. Reviewing one file

Unchanged from today's Docker flow, applied per reviewed file in run order:

- The unified diff prints. In a VS Code terminal the diff opens with Accept/Reject per hunk and the
  whole-file buttons (`Apply accepted hunks`, `Accept all hunks`, `Replace file`, `Replace all`,
  `Keep file`); closing it or Ctrl-C falls back to the terminal for that file.
- The terminal asks `<question> [replace / replace all / anything else keeps it]:`. The question is
  `Apply the devkit changes to <rel>?`, or `Create <rel>?` for a new file.
- `replace all` (terminal or VS Code) accepts every reviewed file for the rest of the run.
- `-y` accepts everything unasked; `--dry-run` asks nothing and ends with the multi-diff review,
  which keeps covering every file, always-written ones included.
- **A new file** is one hunk, the whole file. Keep means it is not created, and it is not listed as
  managed (so no gitignore advisory for a file that does not exist). The next run asks again.
- **Report lines.** Replace keeps the merge's own log lines. A partial answer replaces them with
  `<k> of <n> hunks accepted`, since the log describes changes that may have been rejected. Keep
  records nothing.

## 4. Edge rules

### 4.1 A rejected devkit package

The pyproject merge is what lists `devkit-claude-hooks`, `devkit-poe-complete` and, for Docker
projects, `devkit-container`. If the pyproject as accepted lacks one of them, the package step skips
it for this run with a note naming it (`<name> was not added to pyproject.toml, so it was not
locked or installed; the next run proposes it again`). Without this rule the package step fails the
run (`not in uv.lock after locking`).

A skipped `devkit-container` also skips the Dockerfile and compose steps, with a note: both read
their templates from the installed package, and the compose step otherwise fails the run.
`.dockerignore` does not depend on the package and is still written.

A dry run never rejects, so this rule only applies to plain runs.

### 4.2 A partial answer that breaks the file

Hunks are line ranges, so a mix of accepted and rejected hunks can leave a file that no longer
parses (a dangling comma, a key without its table). After assembling a partial answer the CLI
parses the result: `toml_edit` for `pyproject.toml`, the JSONC parser `json_merge` already uses for
the `.vscode`, `.claude` and `.mcp.json` files. On a parse error it prints
`the accepted hunks leave <rel> unparseable (<error>); answer again` and reviews the same proposal
again. YAML, markdown and the line-based files have no parser in the crate and are not checked.
Only VS Code can answer partially, so the terminal never loops.

### 4.3 Committing runs

A committing run resets the committable managed files to HEAD before merging and replays the user's
uncommitted edits after the commit. The diff's current side is therefore HEAD's copy, not what the
editor shows. The README says so.

### 4.4 Repeat runs, pipes, no stdin

- Anything kept is proposed again on the next run. The README's idempotence line becomes: a second
  run with every proposal accepted is a byte-for-byte no-op.
- A pipe that scripted answers for the Docker questions now meets different questions.
- A run with no stdin is still refused unless `-y` or `--dry-run`.

## 5. Code shape

- **A top-level review module.** The consent machinery leaves `docker/`: `Mode`, `Decision`,
  `Consent`, the hunk split and reassembly (`docker/hunks.rs`), and the diff printer and newline
  normaliser from `docker/static_files.rs`. `Consent::decide` keeps its `offer_replace_all`
  parameter, now used only by the service add.
- **Collaborators.** `runner`, `prompt`, `reviewer` and `mode` move from `docker::Deps` to
  `crate::Deps`; `docker::Deps` goes away. `run_with` builds one `Consent` for the run and hands it
  to every reviewed step and to the Docker step (for the service add).
- **One review-and-record function** replaces `changes.record_optional` at each reviewed call site
  in `lib.rs` (about 14): print the diff, decide, validate a partial (4.2), record the chosen text
  with the report lines from section 3, and return the text now on disk (or that would be, in a dry
  run). The pyproject step uses that return value for 4.1.
- **Docker.** `static_files::apply` writes the rendered Dockerfile directly. The compose step applies
  each service's rule edits and the top-level edits straight onto the text, and keeps the consent
  call only for the service add.
- **Package step.** `packages::advance` takes the list of packages to advance after dropping those
  missing from the accepted pyproject; `run_with` emits the 4.1 notes and gates the Docker step.
- **Extension protocol.** No change: the request carries a title, two texts and hunks, and nothing in
  it is Docker-specific. Doc comments in `vscode/protocol.rs` and `vscode/session.rs` that name the
  Docker step are updated.

## 6. Docs

- `README.md`: the `setup-project` flags paragraph (which files prompt), the **Docker** bullet (no
  questions except the service add; drop the replace/replace-all text), a sentence on review in the
  per-file bullets or one shared paragraph, the post-apply/idempotence wording (4.4), the committing
  run's HEAD-side diff (4.3), and **VS Code extension** ("each Docker change" becomes each reviewed
  file).
- `TODO.md`: remove the "only the Docker files" review item and the "a kept Dockerfile hunk does not
  survive the next `docker-pin`" item, which no longer arises.
- `devkit-vscode/README.md` line 5 ("shows each Docker change"): a separate commit in that repository.

## 7. Testing

- The review function: replace, keep, partial, `replace all` carrying over, a new file kept (not
  written, not managed), a partial that breaks TOML or JSON being asked again.
- `run_with`: every file in the reviewed row goes through the reviewer and nothing in the
  always-written row does; a pyproject partial that drops `devkit-container` skips the package and
  the Docker steps with their notes.
- The Docker integration tests (`tests/docker.rs`) lose their consent scripts except for the service
  add; the existing CRLF and partial-assembly coverage moves to a non-Docker file.

## 8. Out of scope

- Per-hunk choice in the terminal.
- Reviewing env files with masked values.
- Validating YAML or markdown after a partial answer.
- Any change to the extension or its protocol.
