#!/usr/bin/env bash
# Build and package the extension as aeth-devkit-vscode-N.vsix, manifest version N.0.0.
# The repo's package.json stays at 0.0.0: the version is stamped here only.
set -euo pipefail
n="${1:?usage: package.sh N}"
cd "$(dirname "$0")/.."
npm run build
npx @vscode/vsce package "$n.0.0" \
  --no-dependencies --no-git-tag-version --no-update-package-json \
  --allow-missing-repository \
  --out "aeth-devkit-vscode-$n.vsix"
