#!/usr/bin/env bash
# docker-pin-latest.sh — Resolve the version to pin (explicit arg or latest release),
# and update the version field in the compose file in place.
# Automatically detects the compose file in the project root.
# Supports two pinning modes, detected automatically from the compose file:
#   - GIT_TAG mode  : updates GIT_TAG (e.g. v1.2.3) using the GitHub Releases API,
#                     requires GIT_REPO to be present in the compose build args.
#   - PyPI mode     : updates PACKAGE_VERSION using PACKAGE_NAME against SFTPyPI.
# Typically invoked via: poe docker-pin-latest [--version <ver>]

set -euo pipefail

# Optional explicit version argument (e.g. passed by release-and-pin after a fresh release).
# When provided, the version is used as-is and supports pre-release versions.
# When omitted, the latest release is fetched from the appropriate source.
VERSION_ARG="${1:-}"

# ---------------------------------------------------------------------------
# Auto-detect docker compose file (cwd first, then subdirectories)
# Priority matches Docker Compose's own file-name precedence:
#   compose.yaml > compose.yml > docker-compose.yaml > docker-compose.yml
# ---------------------------------------------------------------------------
COMPOSE_FILE=""
for candidate in compose.yaml compose.yml docker-compose.yaml docker-compose.yml; do
  if [ -f "${candidate}" ]; then
    COMPOSE_FILE="${candidate}"
    break
  fi
  found=$(find . -name "${candidate}" -not -path './.git/*' | sort | head -1)
  if [ -n "${found}" ]; then
    COMPOSE_FILE="${found}"
    break
  fi
done

if [ -z "${COMPOSE_FILE}" ]; then
  echo "ERROR: No docker compose file found under $(pwd)." >&2
  echo "       Expected one of: compose.yaml, compose.yml, docker-compose.yaml, docker-compose.yml" >&2
  exit 1
fi

echo "Compose  : ${COMPOSE_FILE}"

# ---------------------------------------------------------------------------
# Detect pinning mode: GIT_TAG/GIT_REPO  vs  PACKAGE_NAME/PACKAGE_VERSION
# ---------------------------------------------------------------------------
GIT_REPO=$(grep -oP 'GIT_REPO:\s*\K\S+' "${COMPOSE_FILE}" | head -1 || true)
PACKAGE_NAME=$(grep -oP 'PACKAGE_NAME:\s*\K\S+' "${COMPOSE_FILE}" | head -1 || true)

if [ -n "${GIT_REPO}" ]; then
  MODE="git"
  echo "Mode     : git (GIT_TAG / GIT_REPO)"
  echo "Repo     : ${GIT_REPO}"
elif [ -n "${PACKAGE_NAME}" ]; then
  MODE="pypi"
  echo "Mode     : pypi (PACKAGE_NAME / PACKAGE_VERSION)"
  echo "Package  : ${PACKAGE_NAME}"
else
  echo "ERROR: Could not find GIT_REPO or PACKAGE_NAME in ${COMPOSE_FILE}" >&2
  echo "       Add one of these build args to select a pinning mode." >&2
  exit 1
fi

# In git mode, derive owner/repo once — used by all API calls and the commit message
GITHUB_REPO_PATH=""
if [ "${MODE}" = "git" ]; then
  GITHUB_REPO_PATH=$(printf '%s\n' "${GIT_REPO}" | sed 's|https://github.com/||;s|\.git$||')
fi

# ---------------------------------------------------------------------------
# Determine the version to pin
# ---------------------------------------------------------------------------
if [ -n "${VERSION_ARG}" ]; then
  LATEST_VERSION="${VERSION_ARG}"
  echo "Version  : ${LATEST_VERSION} (explicitly provided)"

  # Validate the explicit version exists on the remote before proceeding
  if [ "${MODE}" = "git" ]; then
    # Normalise to v-prefixed form for the tag lookup
    TAG_TO_CHECK="v${LATEST_VERSION#v}"
    echo "Validating tag ${TAG_TO_CHECK} exists on GitHub..."
    HTTP_STATUS=$(curl -sL -o /dev/null -w "%{http_code}" \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "https://api.github.com/repos/${GITHUB_REPO_PATH}/git/ref/tags/${TAG_TO_CHECK}")
    if [ "${HTTP_STATUS}" != "200" ]; then
      echo "ERROR: Tag '${TAG_TO_CHECK}' does not exist in ${GIT_REPO} (HTTP ${HTTP_STATUS})" >&2
      exit 1
    fi
    echo "Confirmed: ${TAG_TO_CHECK} exists on remote"
  else
    # PyPI mode — verify the version exists in SFTPyPI
    echo "Validating version ${LATEST_VERSION} exists on SFTPyPI..."
    VALIDATE_JSON=$(curl -sL -H "Accept: application/json" \
      "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/${PACKAGE_NAME}")
    if [ -z "${VALIDATE_JSON}" ]; then
      echo "ERROR: Empty response from SFTPyPI for package '${PACKAGE_NAME}'" >&2
      exit 1
    fi
    EXISTS=$(printf '%s\n' "${VALIDATE_JSON}" | uv run python -c "
import sys, json
data = json.load(sys.stdin)
target = sys.argv[1]
print('yes' if target in data.get('result', {}) else 'no')
" "${LATEST_VERSION}")
    if [ "${EXISTS}" != "yes" ]; then
      echo "ERROR: Version '${LATEST_VERSION}' does not exist for package '${PACKAGE_NAME}' on SFTPyPI" >&2
      exit 1
    fi
    echo "Confirmed: ${LATEST_VERSION} exists on SFTPyPI"
  fi

elif [ "${MODE}" = "git" ]; then
  # Query the GitHub Tags API for the latest stable version tag
  echo "Querying GitHub Tags API for latest stable tag..."

  # Fetch up to 100 tags (most repos won't exceed this; stable latest will be among them)
  TAGS_JSON=$(curl -sL \
    -H "Accept: application/vnd.github+json" \
    -H "X-GitHub-Api-Version: 2022-11-28" \
    "https://api.github.com/repos/${GITHUB_REPO_PATH}/tags?per_page=100")

  if [ -z "${TAGS_JSON}" ]; then
    echo "ERROR: Empty response from GitHub Tags API for repo '${GITHUB_REPO_PATH}'" >&2
    exit 1
  fi

  LATEST_VERSION=$(printf '%s\n' "${TAGS_JSON}" | uv run python -c "
import sys, re, json

tags = json.load(sys.stdin)
names = [t['name'] for t in tags]

# Keep only stable semver tags: v1.2.3 — no pre-release suffixes (a/b/rc, -, etc.)
stable = [n for n in names if re.match(r'^v\d+(\.\d+)*$', n)]
if not stable:
    print('ERROR: No stable version tags found in GitHub Tags API response', file=sys.stderr)
    sys.exit(1)

def version_key(tag):
    return tuple(int(x) for x in tag.lstrip('v').split('.'))

print(max(stable, key=version_key))
")

else
  # PyPI mode — query SFTPyPI for the latest stable release
  echo "Querying SFTPyPI for latest stable version..."

  API_JSON=$(curl -sL -H "Accept: application/json" \
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

# Keep only PEP 440 stable release versions (no pre/post/dev; no metadata keys like "+doc")
versions = [
    v for v in data.get("result", {})
    if re.match(r'^\d+(\.\d+)*$', v)
]
if not versions:
    print("ERROR: No stable release versions found in SFTPyPI response", file=sys.stderr)
    sys.exit(1)

def version_key(v):
    return tuple(int(x) for x in v.split("."))

print(max(versions, key=version_key))
PYEOF

  LATEST_VERSION=$(printf '%s\n' "${API_JSON}" | uv run python "${PYFILE}")
fi

# ---------------------------------------------------------------------------
# Update the version pin in the compose file
# ---------------------------------------------------------------------------
if [ "${MODE}" = "git" ]; then
  # Normalize: ensure the stored tag always has the "v" prefix
  TAG_VERSION="v${LATEST_VERSION#v}"

  CURRENT_VERSION=$(grep -oP 'GIT_TAG:\s*\K\S+' "${COMPOSE_FILE}" | head -1 || true)
  echo "Current  : ${CURRENT_VERSION:-<not set>}"
  echo "Latest   : ${TAG_VERSION}"

  if [ "${CURRENT_VERSION}" = "${TAG_VERSION}" ]; then
    echo "Already pinned to ${TAG_VERSION}. No changes made."
    exit 0
  fi

  # Replace the GIT_TAG value; the tag may or may not carry a "v" prefix currently
  sed -i -E "s/^([[:space:]]*GIT_TAG:[[:space:]]*)v?[0-9]+([.][0-9]+)*((a|b|rc)[0-9]+)?([.]post[0-9]+)?([.]dev[0-9]+)?/\1${TAG_VERSION}/" "${COMPOSE_FILE}"

  echo "Updated GIT_TAG: ${CURRENT_VERSION:-<not set>} -> ${TAG_VERSION} in ${COMPOSE_FILE}"

  COMMIT_MSG="chore: pin ${GITHUB_REPO_PATH##*/} to ${TAG_VERSION}"

else
  # Match any PEP 440 version string: release, pre-release (a/b/rc), post, dev
  CURRENT_VERSION=$(grep -oP 'PACKAGE_VERSION:\s*\K\d+(?:\.\d+)*(?:(?:a|b|rc)\d+)?(?:\.post\d+)?(?:\.dev\d+)?' "${COMPOSE_FILE}" | head -1 || true)

  echo "Current  : ${CURRENT_VERSION:-<not set>}"
  echo "Latest   : ${LATEST_VERSION}"

  if [ "${CURRENT_VERSION}" = "${LATEST_VERSION}" ]; then
    echo "Already pinned to ${LATEST_VERSION}. No changes made."
    exit 0
  fi

  # Replace any full PEP 440 version string (release, pre-release a/b/rc, post, dev)
  sed -i -E "s/^([[:space:]]*PACKAGE_VERSION:[[:space:]]*)[0-9]+([.][0-9]+)*((a|b|rc)[0-9]+)?([.]post[0-9]+)?([.]dev[0-9]+)?/\1${LATEST_VERSION}/" "${COMPOSE_FILE}"

  echo "Updated PACKAGE_VERSION: ${CURRENT_VERSION:-<not set>} -> ${LATEST_VERSION} in ${COMPOSE_FILE}"

  COMMIT_MSG="chore: pin ${PACKAGE_NAME} to ${LATEST_VERSION}"
fi

# ---------------------------------------------------------------------------
# Commit the change and push to remote
# ---------------------------------------------------------------------------
git add "${COMPOSE_FILE}"
git commit -m "${COMMIT_MSG}"
git push

echo "Committed and pushed: ${COMPOSE_FILE}"
