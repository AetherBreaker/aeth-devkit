#!/usr/bin/env bash
# lock.sh — Bump the poe-tasks pin in pyproject.toml to the latest stable release on
# SFTPyPI, run `uv sync` (updating uv.lock), and commit the lockfile (and pin change)
# with a standardized message. Any arguments are forwarded verbatim to `uv sync`.
# Typically invoked via: poe lock [--upgrade] [--all-extras]

set -euo pipefail

PACKAGE_NAME="poe-tasks"
PYPROJECT="pyproject.toml"

if [ ! -f "${PYPROJECT}" ]; then
  echo "ERROR: ${PYPROJECT} not found in $(pwd)" >&2
  exit 1
fi

# ---------------------------------------------------------------------------
# Update the poe-tasks pin (only if this project declares one)
if grep -Eq "^\s*\"${PACKAGE_NAME}\s*[>=~!]=?" "${PYPROJECT}"; then
  echo "Querying SFTPyPI for latest stable ${PACKAGE_NAME} version..."
  API_JSON=$(curl -sfL -H "Accept: application/json" \
    "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/${PACKAGE_NAME}")

  if [ -z "${API_JSON}" ]; then
    echo "ERROR: Empty response from SFTPyPI for package '${PACKAGE_NAME}'" >&2
    exit 1
  fi

  PYFILE=$(mktemp --suffix=.py)
  trap 'rm -f "${PYFILE}"' EXIT
  cat >"${PYFILE}" <<'PYEOF'
import sys, re, json

data = json.load(sys.stdin)
versions = [v for v in data.get("result", {}) if re.match(r'^\d+(\.\d+)*$', v)]
if not versions:
    print("ERROR: No stable release versions found in SFTPyPI response", file=sys.stderr)
    sys.exit(1)
print(max(versions, key=lambda v: tuple(int(x) for x in v.split("."))))
PYEOF
  LATEST_VERSION=$(printf '%s\n' "${API_JSON}" | uv run --no-project python "${PYFILE}")

  # Replace the version in e.g.  "poe-tasks>=4.0.0",  keeping the operator and any extras/markers
  sed -i -E "s/^(\s*\"${PACKAGE_NAME}(\[[^]]*\])?\s*[>=~!]=?\s*)[0-9][0-9A-Za-z.+!-]*/\1${LATEST_VERSION}/" "${PYPROJECT}"

  if git diff --quiet -- "${PYPROJECT}"; then
    echo "${PACKAGE_NAME} pin already at ${LATEST_VERSION}"
  else
    echo "Updated ${PACKAGE_NAME} pin to ${LATEST_VERSION}"
  fi
else
  echo "No ${PACKAGE_NAME} pin found in ${PYPROJECT}; skipping pin update"
fi

# ---------------------------------------------------------------------------
# Sync (updates uv.lock) and commit
uv sync "$@"

if git diff --quiet -- uv.lock "${PYPROJECT}"; then
  echo "uv.lock is up to date; nothing to commit"
else
  git add uv.lock "${PYPROJECT}"
  git commit -m "Update uv.lock"
fi
