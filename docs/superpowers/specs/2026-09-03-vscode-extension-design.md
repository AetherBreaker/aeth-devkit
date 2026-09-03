# VS Code extension — design

Status: draft for review. Builds on
[the CI release workflow design](2026-09-03-ci-release-workflow-design.md) (the `.vsix`
is a release asset) and [the Docker standardization design](2026-09-03-docker-standardization-design.md)
(whose terminal consent prompt this replaces when VS Code is present). Neither depends on
this one.

## Goal

One devkit-owned VS Code extension, `aeth.aeth-devkit`, that:

1. gives `devkit setup-project` an in-editor consent flow: a native diff of each proposed
   Docker change with persistent accept/reject controls, per hunk and per file, in place
   of the typed `replace` prompt;
2. gives `setup-project --dry-run` and `--check` an in-editor review of every change they
   would make, across all managed files;
3. absorbs the existing `drekker-add-to-runtime-base-classes` extension that lives in
   `aeth_ext/.vscode/extension/` (three untyped files installed by a junction), so there
   is one extension to install and version.

The extension is **never published to the VS Code marketplace or any registry**. The
`.vsix` attached to devkit's GitHub release is its only distribution channel; the manifest
carries `"private": true` and the workflow has no `vsce publish` step.

`devkit lock` integration is shelved (2026-09-03) and out of scope.

## Source layout

`vscode-extension/` in the devkit repo, TypeScript, bundled with esbuild (no `tsc`
output shipped; `vscode` API typings via `@types/vscode`):

```
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

- commands `aeth-devkit.acceptHunk`, `rejectHunk`, `applyAccepted`, `replaceFile`,
  `replaceAll`, `keepFile`, `addToRuntimeBaseClasses`;
- `editor/context` entry for `addToRuntimeBaseClasses` (`editorLangId == python`), as
  today;
- `editor/content` entries for `replaceFile` and `keepFile`, `when: resourceScheme ==
  aeth-devkit-proposed`, under `enabledApiProposals: ["contribEditorContentMenu"]`;
- `editor/title` entries for the same two commands, `when: resourceScheme ==
  aeth-devkit-proposed && !aeth-devkit.contentMenu` — the fallback when the proposal is
  not enabled on the machine;
- a `uriHandler` activation event (`onUri`).

The version in `package.json` stays `0.0.0`; CI stamps the release version at package
time (see Distribution) so `devkit release` needs no knowledge of the extension.

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

To verify during implementation: `vsce package` accepts a manifest with
`enabledApiProposals` (publishing is refused, which is irrelevant here). If it does not,
the package step uses `--allow-proposed-apis` or falls back to zipping the folder with
the same layout; a `.vsix` is a zip with a manifest.

## Distribution and installation

### CI

The Rust release workflow template gains a `vsix` job on `ubuntu-latest`:
`actions/setup-node` (LTS), `npm ci` in `vscode-extension/`, `npm run build`, then
`npx @vscode/vsce package "${TAG#v}" --no-dependencies --out aeth-devkit-${TAG#v}.vsix`,
uploaded as artifact `vsix`. The publish job attaches it to the release with the wheels
and container binaries. Like the container step, the job is unconditional in the Rust
variant for now (only devkit is Rust); a `{vscode_extension}` gate joins the
`{container_crate}` note in TODO.md.

### setup-project

New module in the setup crate, `vscode.rs`, run once per invocation before any consent
prompt:

1. **Detect VS Code**: `TERM_PROGRAM == "vscode"`. Otherwise the module is inert and the
   terminal flow applies. `--no-vscode` forces the terminal flow; `--vscode` skips the
   detection (for a VS Code launched terminal that lost the variable).
2. **Find the launcher**: first of `code`, `code-insiders`, `cursor` on `PATH`. On
   Windows the launchers are `.cmd` shims, and `std::process::Command` does not apply
   `PATHEXT`, so the lookup tries `<name>.cmd` explicitly. None found → note, terminal
   flow.
3. **Ensure the extension**: `code --list-extensions --show-versions`; if
   `aeth.aeth-devkit@<devkit version>` is absent, download
   `https://github.com/AetherBreaker/aeth-devkit/releases/download/v<version>/aeth-devkit-<version>.vsix`
   into the devkit cache dir (the HTTP client core already uses for indexes) and run
   `code --install-extension <path> --force`. Failure → note, terminal flow. VS Code
   picks up an installed extension without a restart.
4. **Ensure the proposal grant**: `~/.vscode/argv.json` (JSON with comments; edited with
   the setup crate's comment-preserving JSON merge) gets `aeth.aeth-devkit` appended to
   `enable-proposed-api`. When the entry was added this run, print
   `note: restart VS Code once to enable the in-editor devkit buttons`, and pass
   `content_menu: false` in requests until a later run finds the entry pre-existing.
5. **Retire the junction**: if `~/.vscode/extensions/local.drekker-add-to-runtime-base-*`
   exists, print a note to delete it (never deleted automatically; it is a junction into
   a sister project's working tree). `.vscode/extension/` in a project is reported as
   stray by the same note.

`--dry-run` and `--check` perform steps 1–2 only (no install, no argv edit) and use the
extension for review if it is already installed.

## Consent protocol

The CLI owns every decision; the extension is a view that reports what the user chose.

### Request

For each Docker file (or the compose file's combined edit) the CLI writes
`<cache>/consent/<id>.request.json`:

```json
{
  "id": "…", "devkit_version": "8.3.0",
  "title": "docker/Dockerfile",
  "current_path": "D:/…/docker/Dockerfile",
  "proposed_path": "<cache>/consent/<id>.proposed",
  "hunks": [{ "current": [start, end], "proposed": [start, end] }],
  "offer_replace_all": true,
  "content_menu": true,
  "response_path": "<cache>/consent/<id>.response.json"
}
```

Hunk ranges come from the same `similar` diff that prints the terminal diff, so the two
views always agree. A missing `current_path` file (fresh scaffold) is never a consent
request; it is created without prompting per the Docker design.

The CLI then runs `code --open-url "vscode://aeth.aeth-devkit/consent?request=<path>"`
and polls `response_path` every 250 ms. The terminal still prints the unified diff and a
one-line `waiting for VS Code (Ctrl-C to fall back to the terminal prompt)…`.

### Extension behaviour

1. The URI handler reads the request, registers the proposed text under
   `aeth-devkit-proposed:/<id>/<title>` (content provider), and opens
   `vscode.diff(current, proposed, "devkit: <title>")`.
2. CodeLens on the proposed document: above each hunk `Accept` / `Reject` (toggling;
   accepted by default, the lens label shows the state); at line 0
   `Apply accepted (n of m)` · `Replace all` (only when offered) · `Keep file`.
3. The `editor/content` button (or title icons) offers `Replace file` / `Keep file`.
4. A decision writes the response and closes the diff tab. Closing the tab without a
   decision (tracked via `window.tabGroups.onDidChangeTabs`) writes `keep`.

### Response

```json
{ "decision": "replace" | "replace_all" | "keep" | "partial", "text": "…" }
```

`partial` carries the proposed document with rejected hunks reverted to the current
text, computed by the extension from the hunk table; the CLI writes it verbatim. Every
decision maps onto the Docker design's existing outcomes: `replace_all` stops further
prompting for the run; `partial` is a replace whose content is the returned text.

### Timeouts and fallback

No fixed timeout (a review can take as long as it takes). Ctrl-C while waiting cancels
the request (the CLI writes a `cancel` marker the extension honours by closing the tab)
and asks the terminal prompt instead. Any protocol error (unparseable response, missing
file, extension version mismatch reported by the extension) falls back the same way.

## Review mode

Under `--dry-run` or `--check` with the extension available, the CLI writes one request
listing every file it would change (path, proposed text path), and the extension opens
the multi-diff editor with `vscode.changes("devkit setup-project", [[label, current,
proposed], …])`. Read-only; no response is awaited. The printed report is unchanged.

## Ported command

`addToRuntimeBaseClasses` is a straight port of `aeth_ext/.vscode/extension/extension.js`
to TypeScript with its three pure helpers (`insertIntoRuntimeBaseClasses`,
`ensureRuntimeBaseClassesArray`, `computeModulePath`) unit-tested. Command id changes
from `drekker.addToRuntimeBaseClasses`; title and menu placement stay.

## Testing

- Extension: vitest for `consent.ts` (hunk toggling, partial text assembly, request
  parsing), `runtimeBaseClasses.ts` helpers; an `.vscode/launch.json` in devkit runs the
  extension development host for manual smoke tests.
- Setup crate: launcher resolution including the `.cmd` case, `--list-extensions`
  parsing, argv.json editing (absent file, existing array, already present), response
  polling against a fake responder, every fallback path, and that `--dry-run` never
  installs or edits argv.

## Documentation

README: a `VS Code extension` section (what it does, that it is release-asset only,
the one-time restart note, `--vscode` / `--no-vscode`), and the consent bullets in the
`setup-project` feature reference. TODO.md: remove the extension folder and `install.ps1`
from `aeth_ext` and `aeth_ext-2` once devkit ships it; delete the junction; the
`{vscode_extension}` gate.

## Out of scope

`devkit lock` integration (shelved), consent for non-Docker managed files (they remain
apply-without-prompt), the merge editor, marketplace publishing, tracking the proposal's
finalisation.
