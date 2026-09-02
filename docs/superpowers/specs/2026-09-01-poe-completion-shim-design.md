# poe completion: thin shim design

Date: 2026-09-01
Status: implemented (PR #8)
Supersedes the shell-heavy half of the original poe-completion work in
`crates/aeth-devkit-complete`.

## Problem

`devkit complete` requires a **global** devkit install for two unrelated reasons, and
conflates them into a single hard requirement:

1. **Tab time.** Both generated scripts call bare `devkit` (`& devkit complete args ...`
   in PowerShell, `devkit complete tasks "$target_path"` in bash), so the binary is
   resolved from `PATH`.
2. **Shell startup.** `install --powershell` writes
   `devkit complete script --powershell | Out-String | Invoke-Expression` into `$PROFILE`,
   so *every new shell* runs devkit once purely to obtain ~400 lines of script text.

`install::preflight` then hard-`bail!`s when no devkit is on `PATH`, instructing the user
to `uv tool install aeth-devkit` globally, and *warns* when the devkit it finds lives in a
`.venv` — even though a venv-resident devkit is the normal, expected case for this project.

Requirement 2 is the one that genuinely cannot be satisfied per-project: at shell startup
there is no project context. Requirement 1 can be, trivially. Treating them as one
requirement is the defect.

## Decisions

| # | Decision | Rationale |
| --- | -------- | --------- |
| 1 | Move completion logic into Rust; shells keep a thin permanent shim | Removes the startup devkit call entirely, so requirement 2 disappears rather than being worked around |
| 2 | Resolve `devkit` from `PATH` only | Venv activation is the single source of truth. The editor activates the project venv at shell startup, so `PATH` already resolves to the venv's devkit. Wanting completion in an *unactivated* shell is precisely the case where a global install is the correct answer |
| 3 | Rust returns a sentinel for file/dir completion; the shell performs it | Inherits each shell's correct quoting, escaping, tilde expansion and trailing-separator behaviour instead of reimplementing them |
| 4 | Per-shell input contract: bash sends the raw line, PowerShell sends its parsed AST elements | Each shell sends what it does *well*. bash's `COMP_WORDS` splits on `=` (hence the current ~20-line re-merging hack); PowerShell's `$commandAst.CommandElements` is already an authoritative parse that would be discarded by re-parsing raw text |
| 5 | Repair a drifted shim at Tab time, atomically and idempotently | Self-healing without a startup cost; matches the project's "automate, don't refuse" principle |
| 6 | One manually bumped integer, the **shim version**, sent on every call | A protocol change is always a shim change and a shim bugfix is always a shim change, so one number covers both. It is decoupled from the package version: an ordinary devkit release that does not touch shim code changes nothing |
| 7 | On version mismatch: answer best-effort, repair for the next shell | An already-open shell holds the old shim in memory permanently and would otherwise lose completion entirely until restarted — even for a cosmetic change |

## Architecture

Three components, one job each:

- **Shim** (per shell, ~10-15 lines, permanent). Registers the completer, forwards the
  command line to `devkit`, prints results, acts on the file-completion sentinel. Contains
  no knowledge of poe.
- **Engine** (new module in `aeth-devkit-complete`). Pure: `(shell, words, cursor) ->
  completions`. No I/O and no shell awareness beyond an input-shape enum. Purity is what
  makes the behaviour testable without spawning a shell.
- **Installer** (reworked `install.rs`). Writes shim artifacts, patches `$PROFILE`,
  migrates existing installs.

## Wire protocol

One subcommand, shell-tagged, so both input shapes converge on one code path:

```sh
# bash - raw line, split on the Rust side
devkit complete query --shell bash --shim-version <SHIM_VERSION> \
  --line "$COMP_LINE" --point "$COMP_POINT"

# PowerShell - already-parsed elements, no splitting needed
devkit complete query --shell powershell --shim-version <SHIM_VERSION> \
  --cword N -- <elements...>
```

bash's raw line is split with an off-the-shelf POSIX splitter rather than hand-rolled
parsing. Use **`shell-words`**: POSIX.1-2008 Shell Command Language, no dependencies, no
advisory history. `shlex` is an equally acceptable alternative with broader shell testing
— its RUSTSEC-2024-0006 advisory concerns the `quote`/`join` API, which this design never
calls, and is fixed in >= 1.3.0 regardless.

### Cursor semantics

The two shells locate the cursor differently, and each sends what it can state
authoritatively:

- **bash** sends `--point`, the byte offset of the cursor within `--line`. devkit splits
  the line and takes the word whose span contains that offset. If the offset falls on
  whitespace, a new empty word is being started at that position. The prefix to filter on
  is the text from the word's start up to the cursor — so a cursor mid-word filters on the
  left portion only and ignores the text to its right.
- **PowerShell** sends `--cword`, computed by the shim from `CommandElements` extents as
  the index of the element whose extent contains `$cursorPosition`; when the cursor lies
  beyond the last element (a fresh word is being started), `--cword` equals the element
  count. The shim additionally sends `--word-to-complete` from PowerShell's own
  `$wordToComplete`, which is authoritative for the prefix and removes any need for devkit
  to re-derive it.

### Response format

Response is line-oriented with a directive header:

```text
items
build<TAB>build<TAB>Compile the crate<TAB>command
--mode<TAB>--mode<TAB>Build profile<TAB>param
```

The requests above show the version symbolically: it is `scripts::SHIM_VERSION`, which
bumps whenever shim text changes, so pinning a literal here would go stale.

The header is exactly one of `items`, `dirs` or `files`; the latter two are the
file-completion sentinel and carry no item lines. The header carries **no version** — the
shim version is negotiated on the request side, and duplicating it here would imply the
response format bumps whenever the shim text changes cosmetically, which it does not.

Item columns are: value to insert, display text, tooltip, result type. bash reads column 1
and ignores the rest. PowerShell uses all four, preserving today's tooltips and
`Command`/`ParameterName`/`ParameterValue` typing.

Directives live only in the header, never in item lines, so no completion value can be
mistaken for a sentinel.

## Logic moving into Rust

Everything currently duplicated across the two scripts:

- Locating the task within the command line
- `-C` / `--directory` / `--root` extraction, including the inline `=` forms
- The global option list and its mutual-exclusion table (`-v` suppressing `--quiet`, etc.)
- `--` pass-through detection
- Used-option filtering with equivalence groups (`-v` and `--verbose` suppress each other)
- Positional-argument indexing, skipping option values correctly
- Choice lookup for options and positionals

All fed by the existing `cache::resolve_cached`.

Staying in shell: registering the completer, and acting on the `dirs`/`files` sentinel
(`_filedir` in bash, `Get-ChildItem` in PowerShell).

## Install artifacts and migration

**bash** keeps its current locations — `~/bash_completion.d/poe.bash` and
`~/.local/share/bash-completion/completions/poe` — because bash already installs a *file*;
it simply becomes much shorter. The existing `FileAction::RefuseForeign` guard is retained
unchanged.

**PowerShell** gains a file at `~/.local/share/devkit/poe-completion.ps1`, matching the
`~/.local/share` convention already used for bash. `$PROFILE` receives one permanent,
content-free line:

```powershell
$c = "$HOME/.local/share/devkit/poe-completion.ps1"; if (Test-Path $c) { . $c }
```

This line is deliberately content-free so that it never needs to change. It is the only
user-owned artifact touched, and it is touched only when the user explicitly runs
`devkit complete install` — never from a Tab press.

**Migration.** `patch_profile` already strips poe's own registration line; it gains one
more rule to strip the previous devkit line
(`devkit complete script --powershell | ... | Invoke-Expression`).

**`preflight` is deleted.** Its sole purpose was proving that a fresh shell could resolve
`devkit` for the startup call. There is no startup call any more, so there is nothing to
check: both the hard `bail!` and the `.venv` warning go.

## Version drift and repair

The `$PROFILE` line is permanent by construction, so the only artifact that can drift is
the devkit-owned shim file.

On a Tab press whose `--shim-version` does not match the binary's:

1. Answer the request best-effort anyway (decision 7).
2. If the on-disk shim differs from the current text, rewrite it via temp file + rename,
   so a concurrent Tab press in another shell never observes a torn script.
3. If the on-disk shim already matches, write nothing. This keeps the repair idempotent:
   an open shell holding an old shim in memory reports a stale version on *every* press,
   and must not trigger a write each time.

The next new shell picks up the repaired file.

Backward-version support is deliberately **not** built. Version 1 has no predecessor, so
there is nothing to be compatible with. Revisit only if a version 2 ships.

## Failure modes

A completer that errors breaks the shell, so every failure path yields empty output and a
zero exit: devkit absent from `PATH`, non-zero exit, malformed output, unreadable
`pyproject.toml`, cache miss with no venv. bash guards with `command -v devkit` and
`2>/dev/null`; PowerShell retains `try`/`catch` and `2>$null`. `query` never exits
non-zero, matching existing `tasks` / `args` behaviour.

## Testing

- **Engine table tests**: `(shell, line, cursor) -> expected completions`, no shell
  process. Port the behaviours currently only verifiable by hand: `-C=dir` inline values,
  cursor mid-word, `--` pass-through, mutually exclusive globals, positional indexing past
  used options, choice lookup.
- **Snapshot test** pinning shim text and shim-version constant together, so changing the
  shim text without bumping the constant fails the build. This is what makes decision 6's
  discipline mechanical rather than aspirational.
- **Repair tests**: mismatch with differing file writes once; mismatch with matching file
  writes nothing; write is atomic.
- **Migration tests**: a `$PROFILE` holding the old `Invoke-Expression` line is converted;
  poe's own line is still stripped; a foreign bash file is still refused.
- Existing `tests/complete.rs` and `tests/cli.rs` are retained.

## Non-goals

- No zsh or fish support.
- No completion for any command other than `poe`.
- No change to the cache design.
- `tasks`, `args` and `script` subcommands stay, for already-installed scripts.

## Known loose end

`crates/aeth-devkit-complete/src/lib.rs` cites
`docs/specs/2026-08-28-poe-completion-design.md`, which does not exist in the repository.
Update that reference to point at this document during implementation.
