# Mirroring the devkit workspace on another machine

Every repository lives beside the others under one folder; the paths below assume
`D:\SFT Software Projects`. Each is devkit-managed: `uv sync` installs the tooling and
`poe setup-project` renders the standard configuration.

## Clone

```bash
cd "/d/SFT Software Projects"
gh repo clone AetherBreaker/aeth-devkit
gh repo clone AetherBreaker/devkit-container
```

## Publishing credentials

The release workflows read the SFTPyPI credentials from repository secrets, but `poe release`
and the local index queries read them from `.env`. Copy `aeth-devkit/.env` (never committed)
into every repository that publishes a wheel:

```bash
for r in devkit-container; do cp aeth-devkit/.env "$r/.env"; done
```

## Bring each repository up

`setup-project` prompts and commits, so it needs a real terminal: run this from a shell, not
from a tool with piped stdin.

```bash
for r in aeth-devkit devkit-container; do
  (cd "$r" && uv sync && uv run poe setup-project)
done
```

`setup-project` installs the VS Code extension, the Claude Code hook lines and the shell
completion for this machine as part of that run.
