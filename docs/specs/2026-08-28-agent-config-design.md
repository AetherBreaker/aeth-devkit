# Coding-agent config in `setup-project` — design

Status: **APPROVED 2026-08-28.** Supersedes the open questions in
`2026-08-28-agent-config-survey.md` (that document remains the record of the fleet survey and
the mechanism research; this one is the decision).

Implementation note: by explicit request there is **no separate implementation plan**. This
spec is written to be implemented directly, so it states file paths, function signatures, and
template bodies rather than leaving them to a planning pass.

## Problem

`poe setup-project` standardizes tool/editor/repo config but skips coding-agent config, so
copies of one conventions document have drifted across every project in the fleet, and the
hard config (hooks, MCP servers, permissions) exists only in `aeth_ext`. See the survey for
the per-project evidence.

## Decisions

The survey's five open questions, resolved:

1. **Canonical prose lives in a devkit-managed block in each repo's `AGENTS.md`.** It travels
   with the repo, so Copilot cloud, Copilot code review, Codex CLI, and the `claude.yml`
   Action all see it — a user-level `~/.claude/CLAUDE.md` would be visible only to local
   Claude Code. Cost: one new Markdown merge mode.
2. **Hooks ship as a Rust crate** exposing `devkit hook <name>`, not as copied `.py` files.
   Measured: a Rust binary costs ~16 ms per invocation against ~33 ms for a bare Python
   interpreter start, before any imports. The `PreToolUse` hooks fire on every Edit/Write/Bash,
   so this is the hot path. `poe lock` then upgrades hook behaviour fleet-wide.
3. **`CLAUDE.md` lives at `.claude/CLAUDE.md`** (matching `aeth_ext` today), with body
   `@../AGENTS.md`.
4. **Section split** — see "AGENTS.md template body" below. The `Commands` section is dropped
   entirely except for a note that Python is uv-managed and must be run under `uv run`.
5. **Gitignored targets are written, and the fixing negation line is appended to
   `.gitignore` automatically.** Consistent with the project's standing preference that devkit
   tools resolve awkward state rather than bail.

Two defects found while designing, folded into scope (G below), and one cleanup note (I).

## Scope

| # | Item |
| --- | --- |
| A | New crate `aeth-devkit-hooks` → `devkit hook <name>` |
| B | `AGENTS.md` managed-block merge mode |
| C | `.claude/CLAUDE.md`, `.claude/settings.json`, `.mcp.json`, `.github/workflows/claude.yml`, `.vscode/settings.json` |
| D | `pyproject.template.toml` additions |
| E | Gitignored-target handling |
| F | Testing |
| G | Docker gating defects |
| I | Cleanup reporting |

H is deliberately absent: it was `devkit complete` (poe task completion in Rust) during
design, and is now specified separately in `2026-08-28-poe-completion-design.md`. It shares no
code with this work. The letters are kept stable so references to "G" and "I" from the design
conversation still resolve.

---

## A. `aeth-devkit-hooks` crate

New workspace member `crates/aeth-devkit-hooks`, wired into `crates/aeth-devkit/src/main.rs`
as `Command::Hook(aeth_devkit_hooks::Args)` alongside the existing `SetupProject` / `Lock` /
`Release` arms. Add `aeth-devkit-hooks = { path = "crates/aeth-devkit-hooks" }` to
`[workspace.dependencies]` in `Cargo.toml`.

Per this repo's convention, the crate carries line-level teaching comments: the Rust migration
is study material, not just a port.

### Subcommands

`devkit hook <name>`, where name is one of:

| name | event | behaviour |
| --- | --- | --- |
| `pre-edit-protect` | PreToolUse (Edit/Write) | deny when `tool_input.file_path` basename is `.env` or `uv.lock` |
| `pre-bash-protect-deps` | PreToolUse (Bash) | deny when `tool_input.command` matches the `uv add/remove/lock` regex |
| `stop-ruff` | Stop | run `ruff check --fix --unfixable F401 .`, report output |
| `stop-pyright` | Stop | run `pyright`, report output |
| `stop-clean` | Stop | run `poe clean`, report output |

Each reads the hook payload as JSON on stdin, writes a decision object on stdout, exits 0.
Exiting non-zero or writing malformed JSON would surface as a hook error in every session, so
every failure path (unparseable stdin, missing key, tool not found) exits 0 silently.

### Shared shape

```rust
#[derive(Deserialize)]
struct Payload {
  #[serde(default)] tool_input: ToolInput,
}
#[derive(Deserialize, Default)]
struct ToolInput {
  #[serde(default)] file_path: String,
  #[serde(default)] command: String,
}
```

Deny output (PreToolUse):

```json
{"hookSpecificOutput": {"hookEventName": "PreToolUse",
 "permissionDecision": "deny", "permissionDecisionReason": "..."}}
```

Context output (Stop):

```json
{"hookSpecificOutput": {"hookEventName": "Stop", "additionalContext": "..."}}
```

Both mirror the current Python hooks exactly, including truncating captured output to 4000
bytes (truncate on a char boundary — the Python `output[:4000]` slices by character, and Rust
byte-slicing would panic on multibyte output).

### Behaviour details carried over from the Python originals

- **`pre-bash-protect-deps`** keeps the regex
  `(?:^|[;&|\n]|\bthen\b)\s*uv\s+(?:add|remove|lock)\b` so a banned command is caught after a
  separator, not only at the start of the line.
- **`pre-edit-protect`** denies on basename, not path, so nested `.env` files are covered.
- **`stop-ruff`** keeps `--fix --unfixable F401` and the reasoning comment: ruff should sort
  and format imports and apply other safe fixes, but an unused import must be *reported*
  rather than deleted, because an import added in one edit and first used in a later one would
  otherwise be stripped in between.
- All three Stop hooks resolve `CLAUDE_PROJECT_DIR` (falling back to the cwd) as the working
  directory, and report only when the tool exits non-zero *and* produced output.

### Tool resolution — the speed-relevant part

The Python hooks invoke `uv run <tool>`. `uv run` costs roughly 140 ms of resolution work per
call before the tool starts. Instead, resolve in this order and use the first hit:

1. `<project_dir>/.venv/Scripts/<tool>.exe` (Windows)
2. `<project_dir>/.venv/bin/<tool>` (Unix)
3. `uv run <tool>` as a fallback

If none resolve, exit 0 silently — a project mid-`uv sync` must not spam hook errors.

### `stop-clean` availability

**Do not** gate this on the project's `[tool.poe.tasks]`. `clean` is not a project-defined
task: it is contributed by `aeth_devkit` itself through
`include_script = "aeth_devkit:tasks"` (see `python/aeth_devkit/__init__.py`), so a scan of
the project's own task table finds nothing and the hook would disable itself everywhere.

Any project whose `.claude/settings.json` was written by `setup-project` has devkit, and
therefore has `clean`. So `stop-clean` simply runs, and falls back to silence if `poe` cannot
be resolved by the rules above.

## B. `AGENTS.md` — managed-block merge mode

New module `crates/aeth-devkit-setup/src/md_block.rs`.

```rust
/// Replace the devkit-managed block in `original` with `block`, or append it when absent.
/// Returns the merged document. `original: None` creates the file.
pub fn merge_managed_block(original: Option<&str>, block: &str, log: &mut Vec<String>) -> String;

/// Drop `<!-- setup-project: if-dep NAME -->` sections whose dep is absent, and strip the
/// markers from the sections that survive.
pub fn apply_if_dep(template: &str, ctx: &ProjectContext, log: &mut Vec<String>) -> String;
```

### Rules

- Markers: `<!-- devkit:begin -->` and `<!-- devkit:end -->`. Everything between them is
  devkit's and is replaced wholesale; everything outside is the project's and is never touched.
- File missing → create it containing just the block.
- File exists **with** markers → replace between them, preserving surrounding text and the
  text's original line endings.
- File exists **without** markers → **append** the block at the end, preceded by a blank line.
  Appending rather than prepending keeps a project's own title and description (e.g.
  `aeth_ext`'s) at the top of the file where a reader expects it.
- Missing `end` marker after a `begin` marker is a hard error, not a silent rewrite: it means
  a human edited the file mid-block and guessing would destroy their work.
- Idempotent: merging a document that already contains the current block is a no-op, which
  `Changes::record_optional` already detects by string equality.

### Section gating

`if-dep` mirrors the existing TOML marker at `toml_merge.rs:9`
(`const IF_DEP_MARKER: &str = "setup-project: if-dep ";`), but in HTML-comment form so it is
invisible in rendered Markdown:

```markdown
<!-- setup-project: if-dep aeth-ext -->
## Pydantic Dataclass Conventions
...
```

The marker governs from its position to the next heading of the same level or the end of the
block. Reuse `ProjectContext::has_dependency`, which already normalizes per PEP 503.

### AGENTS.md template body

New template `python/aeth_devkit/templates/AGENTS.template.md`
(`templates::template_file_name("AGENTS.md")` already yields `AGENTS.template.md` — no change
needed to that function).

Seeded from `aeth_ext/.claude/CLAUDE.md`, with these sections, in order:

1. **Environment** (new, replaces `Commands`) — one short paragraph: this project is
   uv-managed; run Python and anything that depends on it under `uv run`. No task list: the
   task list is identical across projects precisely because devkit standardizes it, so
   restating it in every repo is pure drift surface.
2. **Exception Handling (PEP 758, Python 3.14+)** — verbatim.
3. **Tests Do Not Define Intent** — verbatim.
4. **Plan Docs Do Not Define Intent Either** — verbatim.
5. **Testing Workflow** — verbatim.
6. **Comment density** — the second paragraph of the current "Docstring and Comment
   Conventions" section only (the "carry reasoning, but stay dense" material). The Google-style
   paragraph is dropped; see D.
7. **Abstraction Conventions** — verbatim.
8. **Commit Message Conventions** — trimmed to three lines: Conventional Commits, the type
   list, and the requirement that a `fix` body says what broke, why, and how it is fixed.
9. **Secrets** — one line: `.env` holds live credentials; never print, commit, or suggest
   committing it.
10. **Pydantic Dataclass Conventions** — gated `if-dep aeth-ext`, verbatim.

Dropped from the prose because config now enforces them (see D and A): the
`from __future__ import annotations` ban, Google docstring style, `PYTHONPYCACHEPREFIX`, and
the `uv add` protection. Left out of the managed block as project-specific: `aeth_ext`'s
header/description, its Textual dev tasks, and its `shutdown.py` docstring exemplar.

## C. Claude and Copilot config files

Added to `run()` in `crates/aeth-devkit-setup/src/lib.rs` as steps 9–12, after the existing
`.dockerignore` step: `AGENTS.md`, `.claude/CLAUDE.md`, `.claude/settings.json`, `.mcp.json`,
and `.github/workflows/claude.yml`. The `.vscode/settings.json` additions below are not a new
step — they are keys added to a template the existing step 2 already merges.

Templates loaded as JSON use `templates::Escape::Json`, which is what makes the backslashes in
the `{project_root}` substitution survive into valid JSON.

### `.claude/CLAUDE.md` — create-if-missing

Body is `@../AGENTS.md` plus a one-line comment saying Claude-only additions go below. Never
rewritten once it exists: it is the project's hook for Claude-specific text, and the shared
content lives one import away.

### `.claude/settings.json` — deep merge with a keyed hooks merge

Template `python/aeth_devkit/templates/claude/settings.template.jsonc`. Contents:

- `hooks` — the five entries from A.
- `env` — `{"PYTHONPYCACHEPREFIX": "{project_root}\\.cache\\pycache"}`, matching
  `template.env`. This is the `settings.json` `env` block that applies to every subprocess
  Claude spawns, which is what removes the prose instruction to export it manually.
- `enabledMcpjsonServers` — `["github", "context7"]`.
- `permissions.allow` — a curated list: `Bash(uv run pytest *)`, `Bash(uv run ruff check *)`,
  `Bash(uv run pyright *)`, `Bash(uv sync *)`, `Bash(git diff *)`, `Bash(git status *)`,
  `Bash(git log *)`, `WebSearch`.

**Deliberately excluded**, per the survey's findings: `enabledPlugins` and
`extraKnownMarketplaces` (Claude unions these across scopes, so a project copy can only ever
add, never remove — they belong at user level only), `permissions.additionalDirectories` (an
absolute path, not portable), and the session-junk entries in `aeth_ext`'s live file
(scratchpad paths carrying a session UUID, `mkdir -p ".claude/hooks"`).

**Hook command placeholder.** New `{devkit_bin}` substitution in `templates::substitute`:
the detected `.venv/Scripts/devkit.exe` or `.venv/bin/devkit` path (expressed with
`$CLAUDE_PROJECT_DIR` so it stays relocatable), falling back to `uv run devkit` when no venv
is present. Entries keep `"shell": "bash"` and the current timeouts (30/60/30 s for
ruff/pyright/clean).

**Keyed hooks merge.** `json_merge::deep_merge` unions arrays by whole-value equality
(`json_merge.rs:96`, `if !t.contains(v)`). That is correct for `permissions.allow`, but wrong
for `hooks`: change a `timeout` by hand and the template's entry no longer compares equal, so
it is appended and the hook then runs twice per event. Add:

```rust
/// Merge the `hooks` object: groups keyed by `matcher` (absent matcher is its own key),
/// entries within a group keyed by the `hook <name>` token in the command.
pub fn merge_hooks(target: &mut Value, template: &Value, log: &mut Vec<String>);
```

Devkit-owned entries — recognized by the `hook <name>` token, which is stable across binary
paths — are updated in place. Any entry without that token is a hand-written hook and is left
strictly alone. Note the coupling this creates: renaming a hook orphans its old entry in every
repo that has already been set up, so hook names are effectively public API.

Everything except `hooks` goes through the existing `deep_merge`.

### `.mcp.json` — deep merge

Template `mcp.template.jsonc`, matching `aeth_ext/.mcp.json`: `github` over http at
`https://api.githubcopilot.com/mcp/` with an
`Authorization: Bearer ${GITHUB_PERSONAL_ACCESS_TOKEN}` header, and `context7` via
`npx -y @upstash/context7-mcp`. Fully generic; no substitution needed.

### `.github/workflows/claude.yml` — create-if-missing

Template `github/claude.template.yml`, the stock `anthropics/claude-code-action@v1` workflow
`aeth_ext` already uses. Create-if-missing rather than merged: a workflow a project has since
customized must not be reverted on a routine `setup-project` run.

### `.vscode/settings.json` — template additions only

No new mechanism; these are keys added to the existing merged template:

- `chat.useAgentsMdFile: true` — this is what makes VS Code Copilot read the repo-root
  `AGENTS.md`. It defaults to false, which is why the fleet needed
  `copilot-instructions.md` at all.
- `github.copilot.chat.commitMessageGeneration.instructions` — `[{"file": "AGENTS.md"}]`.
- `github.copilot.chat.reviewSelection.instructions` — carries the "skip `tests/` in review"
  rule that currently exists only as prose in `copilot-instructions.md`.

## D. `pyproject.template.toml` additions

- **`src`** — `src = ["./{python_dir}", "../*/src", "../*/python"]`.

  Verified against ruff 0.15.22: `ruff config src` states "This field supports globs", and an
  A/B run in `ScheduledReportAggregator` confirmed the behaviour. With
  `known-first-party` reduced to the project itself, `from aeth_ext.types import StrEnum` was
  reclassified `# Third party imports`; adding the two globs restored `# First party` with no
  name list at all. Sister packages become first-party by existing on disk.

  `known-first-party` stays `["{package}"]`. Note for anyone reading the fleet's existing
  config: entries like `"**/aeth_ext"` do work, but only because globset's `**` matches an
  empty prefix — the field matches *module names*, not paths, so those entries are a
  roundabout spelling of the bare name. The union merge leaves them in place harmlessly.

  Accepted limitation: a sister directory that is not a Python project but contains `src/`
  is also scanned. Ruff only classifies an import as first-party when the *named* module is
  found there, so the worst case is a stray `src/foo.py` in a non-Python sibling.

- **Banned `from __future__ import annotations`**:

  ```toml
  [tool.ruff.lint.flake8-tidy-imports.banned-api]
    "__future__.annotations".msg = "Python 3.14 evaluates all annotations lazily (PEP 649) with or without this import. What it adds is PEP 563 stringification, which breaks pydantic, dataclasses.fields() introspection, and typing.get_type_hints."
  ```

  Verified: ruff 0.15.22 reports
  `TID251 __future__.annotations is banned: <msg>` on `from __future__ import annotations`,
  so the message is delivered by `stop-ruff` exactly when violated — a lint message acting as
  a prompt, costing zero context until it fires. Requires `TID` in the select list.

- **Google docstrings** — `[tool.ruff.lint.pydocstyle] convention = "google"` plus `D` in the
  select list. `aeth_ext` already has this at its pyproject line ~246; the devkit template
  does not.

## E. Gitignored targets

Before writing each managed path, test it with `git check-ignore -q -- <rel>`. The helper
already exists in spirit at `crates/aeth-devkit-setup/src/git.rs:28` inside `trackable()`;
lift it to a reusable `fn is_ignored(root: &Path, rel: &str) -> bool`.

When a managed path is ignored, write the file **and** append a negation line (e.g.
`!.claude/settings.json`) to `.gitignore`, recorded in the change log like any other edit.
This is the concrete case from the survey: `ScheduledReportAggregator`'s `.gitignore` line 239
is `*.json`, so `settings.json` and `.mcp.json` cannot be tracked there today and
`setup-project` would otherwise silently create files git ignores.

This makes `.gitignore` a file `setup-project` *appends to* for a new reason. It is already
merged by `lines::merge_gitignore`, so the mechanism exists; what is new is that a negation
line can be added as a consequence of a different file's write. Order matters: the `.gitignore`
step currently runs at step 6, before the new agent-config steps, so the negation append must
happen as a second pass after the agent-config files are known.

Env files are exempt: `.env` is ignored by design and must stay that way.

## F. Testing

TDD throughout, per the project workflow.

- `md_block.rs` unit tests: create-from-missing; append when no markers; replace between
  markers; preserve text outside; error on unterminated block; idempotent second merge;
  CRLF preservation; `if-dep` section kept and dropped.
- `json_merge::merge_hooks` unit tests: fresh insert; idempotent re-merge; a hand-edited
  `timeout` updates in place instead of duplicating; a hand-written hook with no
  `hook <name>` token survives untouched.
- `aeth-devkit-hooks` unit tests per subcommand, driving payload JSON in and asserting the
  decision JSON out: deny and allow paths for both PreToolUse hooks; Stop hooks reporting only
  on non-zero exit with output; malformed stdin exits 0 silently; 4000-byte truncation lands
  on a char boundary.
- `crates/aeth-devkit-setup/tests/apply.rs`: an end-to-end case over a fixture project
  asserting every new file is created, and a second run reports no changes. Extend the
  existing Docker fixture (`apply.rs:45` writes `docker/Dockerfile`) with a docker-less
  counterpart for G.

## G. Docker gating defects

Both found while designing this change; both are in `setup-project` today.

1. **`[tool.docker]` is merged unconditionally — confirmed live.** A `--dry-run` against a
   fresh scratch project containing only a two-line `pyproject.toml` and an empty package
   reported `- added [tool.docker]`. This is why `aeth-devkit`'s own `pyproject.toml` carries
   a `chown_paths = ["persisted_data"]` block for a project with no Docker setup.

   Fix: add an `if-docker` marker for template tables, mirroring `IF_DEP_MARKER` in
   `toml_merge.rs`, and gate `[tool.docker]` with it. This is the TODO.md item
   "`if-docker` conditional marker for template tables".

2. **`has_docker` is too permissive.** `context.rs:79` sets it from
   `root.join("docker").is_dir()`, so *any* directory named `docker` — empty or stray — turns
   on `.dockerignore` creation and the Docker-only template paths.

   Fix: require real Docker content — a `Dockerfile*` at the root, or a `docker/` directory
   that actually contains a `Dockerfile` or a `compose.y*ml`.

   This is the most likely explanation for the `.dockerignore` that commit `f98063e`
   ("Standardize project configuration with devkit") added to `aeth-devkit`, a repo with no
   Docker setup: the current guard at `lib.rs:112` is correct and a dry-run no longer offers
   to create the file, so either an older devkit lacked the guard or a stray `docker/`
   directory was present at the time. For the record, nothing in the Rust crates writes under
   `docker/`; that commit added only `.dockerignore`, `.gitignore`, and `pyproject.toml`.

## I. Cleanup reporting

`setup-project` never deletes. Two obsolete artifacts should therefore be *reported*, not
removed:

- `[tool.docker]` already merged into projects that have no Docker setup (this repo included).
- `.github/copilot-instructions.md`, made unnecessary by `chat.useAgentsMdFile` plus
  `AGENTS.md`.

Emit a `note:` line naming each so the user can delete it by hand. Silently rewriting files
the tool did not create, or deleting config a project may still depend on, is out of scope.

## Out of scope

- Docker scaffolding flags (`--docker`), tracked separately in TODO.md.
- Migrating the fleet. This spec delivers the mechanism; running `setup-project` across the
  six projects and reconciling each `AGENTS.md` is follow-up work.
- Removing `aeth_ext`'s `.claude/CLAUDE.md` content in favour of the import — the
  create-if-missing rule means an existing file is left alone, so that migration is manual and
  deliberate.
