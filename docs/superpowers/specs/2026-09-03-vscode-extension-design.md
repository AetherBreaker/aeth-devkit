# VS Code extension — design

Status: reviewed 2026-09-04; ready for an implementation plan. Builds on
[the CI release workflow design](2026-09-03-ci-release-workflow-design.md) and the Docker
standardization work on `feat/docker-standardization` (whose terminal consent prompt this
replaces when VS Code is present). Neither depends on this one. Implementation branches
from `feat/docker-standardization`, not `main`, until that branch merges.

## Goal

One devkit-owned VS Code extension, `aeth.aeth-devkit`, that:

1. gives `devkit setup-project` an in-editor consent flow: a native diff of each proposed
   Docker change with persistent accept/reject controls, per hunk and per file, in place
   of the typed `replace` prompt;
2. gives `setup-project --dry-run` an in-editor review of every change it would make,
   across all managed files;
3. absorbs the existing `drekker-add-to-runtime-base-classes` extension that lives in
   `aeth_ext/.vscode/extension/` (three untyped files installed by a junction), so there
   is one extension to install and version.

The extension is **never published to the VS Code marketplace or any registry**. The
`.vsix` attached to its own GitHub release is its only distribution channel; the manifest
carries `"private": true` and no workflow has a `vsce publish` step.

`devkit lock` integration is shelved (2026-09-03) and out of scope.

## Source layout

`vscode-extension/` in the devkit repo, TypeScript, bundled with esbuild (no `tsc`
output shipped; `vscode` API typings via `@types/vscode`):

```text
vscode-extension/
  package.json          # name aeth-devkit, publisher aeth, private: true, version 0.0.0
  tsconfig.json
  esbuild.mjs           # src/extension.ts -> dist/extension.js, single file, minified
  src/
    extension.ts        # activate: URI handler, commands, providers
    consent.ts          # request/response protocol, hunk state, decision writing
    proposedDocs.ts     # TextDocumentContentProvider for scheme aeth-devkit-proposed
    lenses.ts           # CodeLens provider for proposed documents
    review.ts           # dry-run multi-file review (vscode.changes)
    runtimeBaseClasses.ts  # the ported Drekker command, unchanged behaviour
  test/                 # vitest unit tests for the pure modules
```

`package.json` contributes:

- commands `aeth-devkit.acceptHunk`, `rejectHunk`, `applyAccepted`, `acceptAllHunks`,
  `replaceFile`, `replaceAll`, `keepFile`, `addToRuntimeBaseClasses`;
- `editor/context` entry for `addToRuntimeBaseClasses` (`editorLangId == python`), as
  today;
- `editor/content` entries for `replaceFile` and `keepFile`, `when: resourceScheme ==
  aeth-devkit-proposed`, under `enabledApiProposals: ["contribEditorContentMenu"]`;
- `editor/title` entries for the same two commands, `when: resourceScheme ==
  aeth-devkit-proposed && !aeth-devkit.contentMenu` — the fallback when the proposal is
  not enabled on the machine;
- a `uriHandler` activation event (`onUri`);
- `aethDevkit.protocol`: the consent protocol version the build speaks (see Versioning).

The version in `package.json` stays `0.0.0` in the repo; the extension's own release
workflow stamps it at package time.

### The proposed-API decision

`editor/content` is the floating in-editor button VS Code's git extension uses for
"Resolve in Merge Editor". It has been a proposed API (`contribEditorContentMenu`) since
2022-08-22, unchanged since, with no finalisation scheduled (tracking: vscode#190013). It
is a menu contribution with no API surface, and the built-in git extension depends on it,
so removal is unlikely. Stable VS Code grants it to a non-built-in extension only when
`~/.vscode/argv.json` lists the extension in `enable-proposed-api`; without the entry the
contribution is dropped with an extension-host warning and everything else works. That is
why the title-bar icons exist behind the `aeth-devkit.contentMenu` context key: exactly
one of the two whole-file controls is visible.

There is no API to ask whether the grant is active, so `content_menu` in the request is a
best guess (below). The accepted failure is one run, before the restart, in which neither
whole-file control shows and only the line-0 CodeLens offers `Replace file` / `Keep file`.

To verify during implementation:

- `vsce package` accepts a manifest with `enabledApiProposals` (publishing is refused,
  which is irrelevant here). If it does not, the package step uses
  `--allow-proposed-apis` or falls back to zipping the folder with the same layout; a
  `.vsix` is a zip with a manifest.
- CodeLens in the diff editor is governed by `diffEditor.codeLens`, which defaults to
  `false`. The extension sets it at user level once if it is unset, with a note in the
  README; a user who has set it to `false` deliberately is warned and gets the title /
  content controls only.

## Versioning and distribution

### Extension version

The extension has no consumers other than devkit, so it carries a single incrementing
integer `N` as its version, stamped into the manifest as `N.0.0` (vsce requires semver).
Its git tag is `vscode-extension-vN` and its release asset is `aeth-devkit-vscode-N.vsix`.

Compatibility is a separate number, the **protocol version**: an integer in every request
(`"protocol": 1`) and in the manifest's `aethDevkit.protocol`. devkit carries a
`MIN_EXTENSION_VERSION` constant (the first `N` that speaks its protocol). The extension
answers a request whose protocol it does not speak with
`{ "decision": "error", "message": "…" }`, and the CLI falls back to the terminal with
that message. Both numbers change only when the request or response shape changes.

### Extension release workflow

A devkit-repo-only workflow, `.github/workflows/vscode-extension.yml` (not a
setup-project template), triggered by the same `release: published` event as
`release.yml`, so `devkit release` stays untouched:

1. checkout `github.sha`; find the highest existing `vscode-extension-v*` tag (none on
   the first run);
2. `git diff --quiet <that tag> <github.sha> -- vscode-extension/`; no diff → exit 0,
   nothing released;
3. `N = previous + 1` (1 on the first run); `npm ci`, `npm run build`,
   `npx @vscode/vsce package N.0.0 --no-dependencies --out aeth-devkit-vscode-N.vsix`;
4. create the tag `vscode-extension-vN` on `github.sha` and a GitHub release for it with
   the `.vsix` attached (`contents: write` on this job only).

The `vsix` job proposed for the Rust release workflow template is dropped, and the
TODO.md entry for it is replaced by this workflow.

### setup-project

New module in the setup crate, `vscode.rs`, run once per invocation before any consent
prompt. It engages only when stdin is a terminal (the existing `Mode::Ask` condition):
with no stdin there is no diff review, and `--check` never opens an editor.

1. **Detect VS Code**: `TERM_PROGRAM == "vscode"`. Otherwise the module is inert and the
   terminal flow applies. `--no-vscode` forces the terminal flow; `--vscode` skips the
   detection (for a VS Code launched terminal that lost the variable).
2. **Find the launcher**: `code` on `PATH`. On Windows the launcher is a `.cmd` shim,
   and `std::process::Command` does not apply `PATHEXT`, so the lookup tries `code.cmd`
   explicitly. Not found → note, terminal flow. `code-insiders` and `cursor` (different
   URI schemes, `argv.json` locations and extension dirs) are a TODO.md entry.
3. **Ensure the extension**: `code --list-extensions --show-versions`; parse
   `aeth.aeth-devkit@N.0.0`. If absent or `N < MIN_EXTENSION_VERSION`: resolve the newest
   `vscode-extension-v*` tag through the GitHub API (`git/matching-refs/tags/`), download
   its `.vsix` into the devkit cache dir (the HTTP client core already uses for indexes),
   and run `code --install-extension <path> --force`. A fresh install is usable
   immediately. An upgrade over an already-loaded extension is not: the CLI prints
   `reload the VS Code window, then run setup-project again` and exits without changing
   anything. Any failure (offline, no tag yet, install error) → note, terminal flow.
4. **Ensure the proposal grant**: `~/.vscode/argv.json` (JSON with comments; edited with
   the setup crate's comment-preserving JSON merge) gets `aeth.aeth-devkit` appended to
   `enable-proposed-api`. When the entry was added this run, print
   `note: restart VS Code once to enable the in-editor devkit buttons` and pass
   `content_menu: false`; a run that finds the entry pre-existing passes `true`.
5. **Retire the junction**: if `~/.vscode/extensions/local.drekker-add-to-runtime-base-*`
   exists, print a note to delete it (never deleted automatically; it is a junction into
   a sister project's working tree). `.vscode/extension/` in a project is reported as
   stray by the same note.

`--dry-run` performs steps 1–3 with the install disabled (no download, no argv edit) and
uses the extension for review only if a compatible one is already installed. `--check`
performs none of them.

## Consent protocol

The CLI owns every decision and every byte written. The extension is a view that reports
what the user chose; it never produces file content.

### What gets a diff

Every Docker change is a diff request:

- each templated `docker/` file (whole-file replacement), one request per file;
- the compose file, one request **per service**: an existing listed service gets a diff
  of its rule-engine edits (as many hunks as `similar` produces, but a hunk never
  straddles two services: hunks are split at service boundaries); a listed-but-absent
  service gets a diff of exactly one add hunk. The former terminal `add` question is
  gone; adding a service is accepting that hunk. Compose top-level edits are one more
  request.

Hunk ranges come from the same `similar` diff that prints the terminal diff, so the two
views always agree. A missing file (fresh scaffold) is never a consent request; it is
created without prompting.

The current side of every diff is the text devkit actually read, which is the `HEAD`
version: setup-project already resets managed files to `HEAD` before running and merges
the user's uncommitted edits back after the commit. The CLI writes that snapshot to the
cache and the extension shows it through its own content provider, so an unsaved editor
buffer for the same file can never shift the hunk numbering.

### Request

`<cache>/consent/<id>.request.json`:

```json
{
  "protocol": 1, "id": "…",
  "title": "docker/Dockerfile" | "docker/compose.yaml: service web",
  "current_path": "<cache>/consent/<id>.current",
  "proposed_path": "<cache>/consent/<id>.proposed",
  "hunks": [{ "current": [start, end], "proposed": [start, end] }],
  "offer_replace_all": true,
  "content_menu": true,
  "response_path": "<cache>/consent/<id>.response.json"
}
```

The CLI then runs `code --open-url "vscode://aeth.aeth-devkit/consent?id=<id>"`
and polls `response_path` every 250 ms. The terminal still prints the unified diff and a
one-line `waiting for VS Code (Ctrl-C to answer here instead)…`. Requests are sequential:
one diff tab is open at a time.

### Extension behaviour

1. The URI handler accepts only a well-formed id (`<pid>-<n>` or `review-<pid>`) and
   reads `<cache>/consent/<id>.request.json`, so a link from any web page can name
   nothing outside the cache; it refuses a protocol it does not speak
   with an `error` response, registers both texts under
   `aeth-devkit-proposed:/<id>/…` and opens a diff of the two titled
   `devkit: <title>` via `vscode.diff`.
2. CodeLens on the proposed document: above each hunk `Accept` / `Reject` (toggling;
   accepted by default, the lens label shows the state); at line 0
   `Apply accepted (n of m)` · `Accept all hunks` · `Replace all` (only when offered) ·
   `Keep file`.
3. The `editor/content` button (or title icons) offers `Replace file` / `Keep file`.
4. A decision writes the response (temp file + rename, so the poller never reads a
   partial file), marks the request as answered, then closes the diff tab. Closing the
   tab without a decision (`window.tabGroups.onDidChangeTabs`, ignored for an answered
   request) writes `dismissed`.

### Response

```json
{ "decision": "replace" | "replace_all" | "keep" | "partial" | "dismissed" | "error",
  "accepted": [0, 2], "message": "…" }
```

- `replace`: the CLI writes its own proposed text. `Apply accepted` with every hunk
  accepted, and `Accept all hunks`, both send `replace`.
- `partial` carries the accepted hunk indices; the CLI assembles the text itself from the
  hunk table (accepted hunks take the proposed side, rejected the current side) and
  records it as a replace. `Apply accepted` with no hunk accepted sends `keep`.
- `replace_all` is the run-wide decision: every later Docker diff, including new-service
  add hunks, is accepted without being shown, as `--replace-docker` does. The per-diff
  `Accept all hunks` affects only the open diff.
- `dismissed` is not a decision: the CLI asks the terminal prompt for that file only; the
  next file opens in VS Code again.
- `error`: the CLI prints the message and uses the terminal prompt for the rest of the
  run.

### Interrupts and fallback

No timeout: a review takes as long as it takes, and a closed VS Code window simply leaves
the CLI waiting. The first Ctrl-C while waiting cancels the request (the CLI writes
`<id>.cancel`, which the extension honours by closing the tab without a response) and asks
the terminal prompt for that file; a second Ctrl-C at the terminal prompt ends the
process exactly as an unhandled Ctrl-C does today. Any protocol error (unparseable
response, missing file) falls back to the terminal prompt.

The CLI empties `<cache>/consent/` when a run starts and when it ends, so a killed run
leaves nothing behind for longer than the next run.

## Review mode

Under `--dry-run` with a compatible extension installed, the CLI writes one request
(`review-<pid>.request.json`) listing every file it would change — each entry carries
`path`, `label`, `current_path` (null for a created file) and `proposed_path` — opened via
`vscode://aeth.aeth-devkit/review?id=review-<pid>`, and the extension shows the multi-diff
editor with `vscode.changes("devkit setup-project (dry run)", [[path, current, proposed],
…])`. Read-only; no response is awaited. The printed report is unchanged.

## Ported command

`addToRuntimeBaseClasses` is a straight port of `aeth_ext/.vscode/extension/extension.js`
to TypeScript with its three pure helpers (`insertIntoRuntimeBaseClasses`,
`ensureRuntimeBaseClassesArray`, `computeModulePath`) unit-tested. Command id changes
from `drekker.addToRuntimeBaseClasses`; title and menu placement stay.

## Testing

- Extension: vitest for `consent.ts` (hunk toggling, request parsing and path
  validation, protocol refusal, response writing), `runtimeBaseClasses.ts` helpers; an
  `.vscode/launch.json` in devkit runs the extension development host for manual smoke
  tests.
- Setup crate: launcher resolution including the `.cmd` case, `--list-extensions`
  parsing and the `MIN_EXTENSION_VERSION` comparison, argv.json editing (absent file,
  existing array, already present), per-service hunk splitting, partial text assembly
  from accepted indices, response polling against a fake responder, every fallback path,
  that `--dry-run` never installs or edits argv, and that `--check` and a non-TTY stdin
  never touch the module.
- Workflow: the no-diff early exit and the first-release case are exercised by hand on
  the first devkit release after merge.

## Documentation

README: a `VS Code extension` section (what it does, that it is release-asset only with
its own `vscode-extension-vN` tags, the one-time restart note, the `diffEditor.codeLens`
setting, `--vscode` / `--no-vscode`), and the consent bullets in the `setup-project`
feature reference. TODO.md: remove the extension folder and `install.ps1` from `aeth_ext`
and `aeth_ext-2` once devkit ships it; delete the junction; `code-insiders` / `cursor`
support; the `{vscode_extension}` gate entry is dropped.

## Out of scope

`devkit lock` integration (shelved), consent for non-Docker managed files (they remain
apply-without-prompt), `--check` opening anything, the merge editor, marketplace
publishing, tracking the proposal's finalisation, editors other than `code`.
