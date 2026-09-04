# VS Code extension: bugs found while testing

Collected during the manual smoke test of `feat/vscode-extension`; fixed together in a
later pass. Add an entry per bug: what happened, what caused it, where the fix goes.

- [ ] **A UTF-8 BOM breaks the Docker step.** A `docker/compose.yaml` starting with a BOM
      (Windows PowerShell's `Set-Content -Encoding utf8` writes one) makes `tree::top_level`
      miss `services:` and the run dies with `docker/compose.yaml has no top-level
      \`services:\` key`; a BOM on `docker/Dockerfile` shows up as a phantom first hunk
      (`-# syntax…` / `+# syntax…`, visually identical). Fix in the setup crate: strip a
      leading `\u{FEFF}` when reading files for the compose flow and in
      `static_files::normalize_newlines`, and make the compose error name the BOM when
      one is present. Found 2026-09-04.
