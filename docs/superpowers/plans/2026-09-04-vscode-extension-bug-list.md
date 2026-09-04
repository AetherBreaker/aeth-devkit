# VS Code extension: bugs found while testing

Collected during the manual smoke test of `feat/vscode-extension`; fixed together in a
later pass. Add an entry per bug: what happened, what caused it, where the fix goes.

- [x] **A UTF-8 BOM breaks the Docker step.** A `docker/compose.yaml` starting with a BOM
      (Windows PowerShell's `Set-Content -Encoding utf8` writes one) makes `tree::top_level`
      miss `services:` and the run dies with "docker/compose.yaml has no top-level
      services: key"; a BOM on `docker/Dockerfile` shows up as a phantom first hunk
      (a `-`/`+` pair on the `# syntax` line, visually identical). Fix in the setup crate: strip a
      leading `\u{FEFF}` when reading files for the compose flow and in
      `static_files::normalize_newlines`, and make the compose error name the BOM when
      one is present. Found 2026-09-04.
- [x] **Compose diffs lose syntax highlighting.** The diff documents are served as
      `aeth-devkit-proposed:/<id>/<side>/<title>` and a compose title is
      `docker/compose.yaml: new service worker`, so the URI's last segment is `worker`
      and VS Code's filename-based language detection finds nothing; only
      `docker/Dockerfile` (no suffix) highlights. Fix in `proposedDocs.ts`: build the path
      so the real file name is the last segment (`/<id>/<side>/<suffix>/<rel path>`), or
      have the CLI send the file path separately from the title and use that. Found
      2026-09-04.
