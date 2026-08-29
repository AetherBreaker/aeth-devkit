# Coding-agent config in `setup-project` — survey + brainstorm

Status: **SURVEY / BRAINSTORM — nothing here is approved.** Written 2026-08-28 by a Claude session
started in `aeth_ext`; handed off so the work can continue from this repo. The user has made **no
design decisions yet**; the "Open questions" section is where the conversation stopped.

## Problem

`poe setup-project` standardizes tool/editor/repo config but skips coding-agent config
(`CLAUDE.md`, `AGENTS.md`, `.github/copilot-instructions.md`, `.claude/settings.json`, hooks,
`.mcp.json`, `claude.yml`). Some of that is prose ("prompt chunks"), which the existing merge modes
don't handle. Question raised: how much of the prose can move into real config files instead?

## Survey of the fleet (2026-08-28)

### Prose files — seven copies of one document, all drifting

| Project | Files | State |
| --- | --- | --- |
| aeth_ext | `.claude/CLAUDE.md`, `AGENTS.md`, `.github/copilot-instructions.md` | `CLAUDE.md` is the newest; the other two are stale (old Annotation section, still tell agents to trust `.claude/plans/`, missing the "Plan Docs Do Not Define Intent" and "Abstraction Conventions" sections) |
| ScheduledReportAggregator | `.claude/CLAUDE.md`, copilot-instructions | Snapshot of an older aeth_ext `CLAUDE.md`, header swapped |
| IMAPReportCollector | copilot-instructions | Titled `# aeth_ext Project Conventions` — verbatim copy |
| ScheduledInvoiceProcessor | copilot-instructions | 14 lines |
| aeth-devkit | copilot-instructions | 6 lines (poethepoet docs pointer) |

### Hard config — exists only in aeth_ext (hooks also in SRA)

- `aeth_ext/.claude/settings.json`: hooks wiring (PreToolUse: `pre_edit_protect`, `pre_bash_protect_deps`;
  Stop: `stop_ruff`, `stop_pyright`, `stop_clean`), `enabledMcpjsonServers`, `enabledPlugins`,
  `extraKnownMarketplaces`, `permissions.allow`, `permissions.additionalDirectories`.
  - `enabledPlugins`/`extraKnownMarketplaces` duplicate `~/.claude/settings.json` exactly. Claude
    *unions* these arrays across scopes, so a project copy can only add, never remove → belongs at
    user level only.
  - `permissions.allow` has leaked session junk (scratchpad paths containing a session UUID,
    `mkdir -p ".claude/hooks"`); `additionalDirectories` is an absolute path. Not portable as-is —
    a template needs a curated allowlist, not the live file.
- `aeth_ext/.claude/hooks/*.py` (5 files, ~25 lines each): SRA's copies differ only by a `-> None`
  annotation. `stop_clean.py` assumes a `clean` poe task exists. `stop_ruff.py` runs
  `ruff check --fix --unfixable F401 .` (reasoning comment inside is worth keeping).
- `aeth_ext/.mcp.json` (github via `${GITHUB_PERSONAL_ACCESS_TOKEN}`, context7 via npx) and
  `.github/workflows/claude.yml` (stock `claude-code-action@v1` template): fully generic.
- `.claude/settings.local.json`, `.claude/plans/`, `scheduled_tasks.lock`: local/project-specific,
  never template.
- SRA `.gitignore` line 239 is `*.json` → `settings.json`/`.mcp.json` cannot be tracked there
  today. Per-project fix, not a devkit concern (but `setup-project` will silently create
  ignored files there).
- User level: `~/.claude/settings.json` has plugins, marketplaces, model, effort. **No**
  `~/.claude/CLAUDE.md`, **no** `~/.claude/rules/`, no user-level hooks.

### Mechanisms verified against docs (2026-08-28)

- Claude Code (`code.claude.com/docs/en/memory.md`, `settings.md`, `settings-reference.md`):
  - `CLAUDE.md` supports `@path` imports (relative to the containing file, `@~/` ok, 4 hops, skipped
    inside code blocks). Does **not** read `AGENTS.md` natively → `CLAUDE.md` body `@AGENTS.md` is
    the bridge.
  - `.claude/rules/*.md` (optional `paths:` frontmatter) and `~/.claude/rules/` exist.
  - `hooks`, `permissions.*`, `enabledPlugins` arrays concatenate+dedupe across user/project scopes;
    scalars: highest precedence wins. `$CLAUDE_PROJECT_DIR` is set for user-level hooks too.
  - Project `settings.json` supports an `env` block applied to every subprocess.
- Copilot in VS Code (`code.visualstudio.com/docs/agent-customization/custom-instructions`):
  - Reads repo-root `AGENTS.md` when `chat.useAgentsMdFile: true` (default false) — a
    `.vscode/settings.json` key, which `setup-project` already merges.
  - `.github/instructions/*.instructions.md` with `applyTo` globs supported.
  - `github.copilot.chat.commitMessageGeneration.instructions` and the review-instructions keys
    accept `[{text}]`/`[{file}]` arrays in settings.
  - Copilot coding agent (cloud) and Copilot code review read `AGENTS.md` and
    `copilot-instructions.md`.
- Codex CLI reads nearest `AGENTS.md`; Gemini CLI can be pointed at it via
  `.gemini/settings.json` `context.fileName`.

Conclusion: `AGENTS.md` can be the single canonical prose file; `CLAUDE.md` = `@AGENTS.md` (+ any
Claude-only lines); `copilot-instructions.md` becomes unnecessary once `chat.useAgentsMdFile` is on.

## Prose → hard config candidates

Each row: current `CLAUDE.md` section → replacement → what prose remains.

| Section | Replacement | Prose left |
| --- | --- | --- |
| `PYTHONPYCACHEPREFIX` | already in `.env`/launch.json/tasks.json via setup-project; add `env` block to `.claude/settings.json` | none |
| No `from __future__ import annotations` | Ruff `[tool.ruff.lint.flake8-tidy-imports.banned-api] "__future__.annotations".msg = "<the why>"` — `stop_ruff` delivers the message only when violated ("lint message as prompt"); verify TID251 catches `from __future__ import annotations` before relying on it | none |
| Google docstrings | `pydocstyle convention = "google"` + `D` selection in the pyproject template (aeth_ext has it at pyproject line ~246; the devkit template does not) | the "dense comments" paragraph |
| Commit conventions | Copilot: settings key. Everyone: a `commit-msg` git hook (`conventional-pre-commit`/commitizen) | ≤3 lines |
| Secrets / `uv add` protection | hooks already enforce | 1 line each |
| Copilot "skip `tests/` in review" (copilot-instructions only) | review-instructions settings key | none |
| Commands | identical across projects because devkit standardizes the tasks | template text |
| PEP 758; Tests/Plans-don't-define-intent; Testing workflow; Abstraction conventions | genuine universal policy | stays, in the template |
| IsPydantic | applies to any project depending on `aeth-ext` | template, gated by existing `if-dep aeth-ext` |
| Textual dev tasks; `shutdown.py` as docstring exemplar | aeth_ext-specific | outside the managed part |

## Sketch of what `setup-project` would grow

1. **`AGENTS.md`** — new merge mode: managed block (`<!-- devkit:begin -->…<!-- devkit:end -->`)
   replaced wholesale; text outside is the project's. `if-dep` gates sections. Create-if-missing.
   (Section-union keyed on `## headings` was considered; managed block is simpler and honest about
   ownership.)
2. **`CLAUDE.md`** — create-if-missing, body `@AGENTS.md`. Location undecided: root vs `.claude/`
   (aeth_ext uses `.claude/CLAUDE.md`; `@../AGENTS.md` from there).
3. **`.claude/settings.json`** — JSON deep-merge (array union is right for hooks/permissions).
   Template: hooks + `env` + `enabledMcpjsonServers` + curated allowlist. **Not** plugins/marketplaces.
4. **Hooks** — instead of copying five `.py` files per repo, ship them in the package
   (`devkit hook stop-ruff` subcommands, or `python -m aeth_devkit.hooks.stop_ruff`). Then
   `settings.json` is the only per-repo artifact and `poe lock` upgrades hook behaviour fleet-wide.
   `stop_clean` should be conditional on a `clean` task existing.
5. **`.mcp.json`** — JSON deep-merge. **`.github/workflows/claude.yml`** — create-if-missing.
6. **`.vscode/settings.json`** additions — `chat.useAgentsMdFile: true`, Copilot commit + review
   instruction keys. Zero new mechanism.
7. **pyproject template** — Ruff `banned-api` for `__future__.annotations`, pydocstyle google.
8. `copilot-instructions.md`: setup-project never deletes; either leave, or report as obsolete.

## Open questions (conversation stopped at #1)

1. **Where does canonical prose live?** (a) devkit-managed block in each repo's `AGENTS.md` —
   travels with the repo, seen by Copilot cloud/code-review and the `claude.yml` Action; costs a
   new Markdown merge mode. Recommended. (b) user-level `~/.claude/CLAUDE.md` + `~/.claude/rules/` —
   zero repo churn, no new merge code, but only local Claude Code sees it.
2. Hooks: package-shipped subcommands vs copied files.
3. `CLAUDE.md` at root or `.claude/`.
4. Which universal-policy sections survive the "prose → config" pass, and whether aeth_ext's
   current `CLAUDE.md` text is the seed for the template.
5. Whether `setup-project` should refuse/warn when a target is gitignored (SRA `*.json`).
