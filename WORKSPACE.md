# Mirroring the devkit workspace on another machine

Every repository lives beside the others under one folder; the paths below assume
`D:\SFT Software Projects`. Each is devkit-managed: `uv sync` installs the tooling and
`poe setup-project` renders the standard configuration.

## Clone

```bash
cd "/d/SFT Software Projects"
gh repo clone AetherBreaker/aeth-devkit
gh repo clone AetherBreaker/devkit-container
gh repo clone AetherBreaker/devkit-vscode
gh repo clone AetherBreaker/devkit-claude-hooks
gh repo clone AetherBreaker/devkit-poe-complete
```

## Publishing credentials

The release workflows read the SFTPyPI credentials from repository secrets; locally, only the
`release`, `release-and-pin` and `rescind-release` poe tasks read them, from `.env` (index
queries and `uv` need none). Copy `aeth-devkit/.env` (never committed) into every other
repository, `devkit-vscode` included: nothing there needs the credentials, but one `.env` in
every checkout is the deliberate convention, and its repository secrets are pre-provisioned
the same way, unused so far.

```bash
for r in devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete; do cp aeth-devkit/.env "$r/.env"; done
```

## Bring each repository up

`setup-project` prompts and commits, so it needs a real terminal: run this from a shell, not
from a tool with piped stdin.

```bash
for r in aeth-devkit devkit-container devkit-vscode devkit-claude-hooks devkit-poe-complete; do
  (cd "$r" && uv sync && uv run poe setup-project)
done
```

`devkit-vscode` also needs `npm ci`; its `poe setup-project` needs `aeth-devkit>=12.1.0` in its
venv, which `uv sync` provides. `devkit-claude-hooks` and `devkit-poe-complete` build their
binaries into the venv through maturin during `uv sync`, so they need the Rust toolchain.

`setup-project` installs the VS Code extension, the Claude Code hook lines and the shell
completion for this machine as part of that run.
