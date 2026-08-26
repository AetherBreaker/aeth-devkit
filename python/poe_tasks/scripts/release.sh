#!/usr/bin/env bash
# release.sh — Bump version, commit, tag, build, and publish to GitHub and SFTPyPI.
# Usage: bash scripts/release.sh [--force|-f] [bump_type ...] ["quoted notes"]
#   Positional args: one or more bump types (major, minor, patch, stable, alpha, beta, rc, post, dev)
#   Notes (optional): a quoted multi-word string as the last positional arg
# Typically invoked via: poe release [-f] [bump_type ...] ["notes"]
# Examples:
#   poe release patch
#   poe release major alpha
#   poe release minor "initial minor release"
#   poe release                         # publish current version without bumping
#
# On any error, all steps that were completed are rolled back:
#   - GitHub release deleted
#   - Package removed from SFTPyPI
#   - Remote tag deleted
#   - Local tag deleted
#   - Version-bump commit reset
#   - pyproject.toml and uv.lock restored
#   - Remote branch force-pushed back to the pre-bump state

set -euo pipefail

# ---------------------------------------------------------------------------
# Helper: return 0 if $1 is a valid uv version --bump keyword
# ---------------------------------------------------------------------------
is_bump_type() {
  case "${1:-}" in
  major | minor | patch | stable | alpha | beta | rc | post | dev) return 0 ;;
  *) return 1 ;;
  esac
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
# poe injects --force/-f as env var 'force=True' (type=boolean); all other
# positional args arrive as $@ (via $POE_EXTRA_ARGS expansion in the poe cmd).
# We also handle --force/-f flags if the script is invoked directly.
#
# Detection rules (applied to remaining args after stripping --force/-f):
#   - Leading args matching a bump type    → collected into BUMP_TYPES
#   - Once a non-bump-type word is seen, all remaining args form the notes candidate
#   - 0 trailing non-bump-type words       → no notes
#   - 1 trailing non-bump-type word        → error (assumed typo, not a note)
#   - 2+ trailing non-bump-type words      → joined with spaces as release notes
_force_env="${force:-}"
force=false
BUMP_TYPES=()
_tail_words=()

for arg in "$@"; do
  case "${arg}" in
  --force | -f) force=true ;;
  *)
    if is_bump_type "${arg}" && ((${#_tail_words[@]} == 0)); then
      BUMP_TYPES+=("${arg}")
    else
      _tail_words+=("${arg}")
    fi
    ;;
  esac
done
[[ "${_force_env}" == "True" ]] && force=true
unset _force_env

notes_text=""
if ((${#_tail_words[@]} == 1)); then
  if [[ "${_tail_words[0]}" == *" "* ]]; then
    # Single arg with spaces — shell quoting was preserved; treat as notes
    notes_text="${_tail_words[0]}"
  else
    # Single word that isn't a bump type — assume typo
    echo "ERROR: '${_tail_words[0]}' is not a valid bump type." >&2
    echo "       Valid bump types: major, minor, patch, stable, alpha, beta, rc, post, dev" >&2
    echo "       (Notes must be multiple words — single-word notes are not supported.)" >&2
    exit 1
  fi
elif ((${#_tail_words[@]} >= 2)); then
  notes_text="${_tail_words[*]}"
fi
unset _tail_words

# ---------------------------------------------------------------------------
# Pre-flight: verify required environment variables are present
# ---------------------------------------------------------------------------
missing_vars=()
[[ -z "${UV_INDEX_SFTPYPI_USERNAME:-}" ]] && missing_vars+=("UV_INDEX_SFTPYPI_USERNAME")
[[ -z "${UV_INDEX_SFTPYPI_PASSWORD:-}" ]] && missing_vars+=("UV_INDEX_SFTPYPI_PASSWORD")
if ((${#missing_vars[@]} > 0)); then
  echo "ERROR: The following required environment variables are not set:" >&2
  for var in "${missing_vars[@]}"; do
    echo "  - ${var}" >&2
  done
  exit 1
fi

# ---------------------------------------------------------------------------
# Guard: warn if the working tree has uncommitted changes
# ---------------------------------------------------------------------------
if [[ -n "$(git status --porcelain)" ]]; then
  echo "WARNING: You have uncommitted changes:" >&2
  git status --short >&2
  if $force; then
    echo "WARNING: Proceeding anyway (--force)." >&2
  else
    printf "Continue with release anyway? [y/N] " >&2
    read -r _response </dev/tty
    case "${_response}" in
    [yY] | [yY][eE][sS]) echo "Continuing..." ;;
    *)
      echo "Aborting." >&2
      exit 1
      ;;
    esac
  fi
fi

CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
COMMITTED=false
TAGGED=false
PUSHED=false
PUSHED_BRANCH=false
PUBLISHED_PYPI=false
RELEASED_GITHUB=false
NEW_VERSION=""
SNAPSHOT_DIR=""
PYPROJECT_SNAPSHOT=""
UVLOCK_SNAPSHOT=""
UVLOCK_WAS_PRESENT=false
DIST_SNAPSHOT_DIR=""

cleanup() {
  # SC2155: declare and assign separately so `local` does not mask $?
  local exit_code
  exit_code=$?
  # Disable errexit and nounset so every rollback step runs regardless of failures
  set +eu

  if [ "${exit_code}" -eq 0 ]; then
    if [ -n "${SNAPSHOT_DIR:-}" ] && [ -d "${SNAPSHOT_DIR}" ]; then
      rm -rf "${SNAPSHOT_DIR}" 2>/dev/null || true
    fi
    return 0
  fi

  echo ""
  echo "ERROR: Release failed (exit code: ${exit_code}). Rolling back changes..."

  # 1. Delete GitHub release (--cleanup-tag also removes the remote tag)
  if $RELEASED_GITHUB; then
    echo "  -> Deleting GitHub release v${NEW_VERSION} (and its remote tag)..."
    gh release delete "v${NEW_VERSION}" --yes --cleanup-tag 2>/dev/null ||
      echo "     WARNING: Could not delete GitHub release. Run manually: gh release delete v${NEW_VERSION} --yes --cleanup-tag"
  fi

  # 2. Remove package from SFTPyPI via devpi REST API
  if $PUBLISHED_PYPI; then
    echo "  -> Removing ${PACKAGE_NAME}==${NEW_VERSION} from SFTPyPI..."
    curl -s -o /dev/null \
      -u "${UV_INDEX_SFTPYPI_USERNAME}:${UV_INDEX_SFTPYPI_PASSWORD}" \
      -X DELETE "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/${PACKAGE_NAME}/${NEW_VERSION}" ||
      echo "     WARNING: Could not remove from SFTPyPI. Manual cleanup needed."
  fi

  # 3. Delete remote tag (only if push succeeded but GitHub release cleanup did not run)
  if $PUSHED && ! $RELEASED_GITHUB; then
    echo "  -> Deleting remote tag v${NEW_VERSION}..."
    git push origin --delete "v${NEW_VERSION}" 2>/dev/null ||
      echo "     WARNING: Could not delete remote tag v${NEW_VERSION}."
  fi

  # 4. Delete local tag
  if $TAGGED; then
    git tag -d "v${NEW_VERSION}" 2>/dev/null || true
  fi

  # 5. Reset local version bump commit (mixed: keeps working tree, restores index)
  if $COMMITTED; then
    echo "  -> Resetting local version bump commit..."
    git reset HEAD~1 ||
      echo "     WARNING: Could not reset local commit. Run manually: git reset HEAD~1"
  fi

  # 6. Restore pyproject.toml and uv.lock to their exact pre-run contents.
  if [ -n "${PYPROJECT_SNAPSHOT:-}" ] && [ -f "${PYPROJECT_SNAPSHOT}" ]; then
    cp "${PYPROJECT_SNAPSHOT}" pyproject.toml 2>/dev/null || true
  fi
  if $UVLOCK_WAS_PRESENT; then
    if [ -n "${UVLOCK_SNAPSHOT:-}" ] && [ -f "${UVLOCK_SNAPSHOT}" ]; then
      cp "${UVLOCK_SNAPSHOT}" uv.lock 2>/dev/null || true
    fi
  else
    rm -f uv.lock 2>/dev/null || true
  fi

  # 7. Force-push the rolled-back state to remote (local HEAD is now the pre-bump commit)
  if $PUSHED_BRANCH; then
    echo "  -> Force-pushing rollback to remote ${CURRENT_BRANCH}..."
    git push --force origin "${CURRENT_BRANCH}" 2>/dev/null ||
      echo "     WARNING: Could not force-push rollback. Run manually: git push --force origin ${CURRENT_BRANCH}"
  fi

  rm -f dist/*.whl dist/*.tar.gz 2>/dev/null || true
  if [ -n "${DIST_SNAPSHOT_DIR:-}" ] && [ -d "${DIST_SNAPSHOT_DIR}" ]; then
    cp "${DIST_SNAPSHOT_DIR}"/* dist/ 2>/dev/null || true
  fi
  if [ -n "${SNAPSHOT_DIR:-}" ] && [ -d "${SNAPSHOT_DIR}" ]; then
    rm -rf "${SNAPSHOT_DIR}" 2>/dev/null || true
  fi
  echo ""
  echo "Rollback complete."
}

trap cleanup EXIT

# ---------------------------------------------------------------------------
# Snapshot pre-run state so cleanup can restore files on failure
# ---------------------------------------------------------------------------
SNAPSHOT_DIR=$(mktemp -d 2>/dev/null || mktemp -d -t release-snapshot)
PYPROJECT_SNAPSHOT="${SNAPSHOT_DIR}/pyproject.toml"
UVLOCK_SNAPSHOT="${SNAPSHOT_DIR}/uv.lock"
DIST_SNAPSHOT_DIR="${SNAPSHOT_DIR}/dist"
cp pyproject.toml "${PYPROJECT_SNAPSHOT}"
if [ -f uv.lock ]; then
  UVLOCK_WAS_PRESENT=true
  cp uv.lock "${UVLOCK_SNAPSHOT}"
fi
mkdir -p "${DIST_SNAPSHOT_DIR}" dist
for artifact in dist/*.whl dist/*.tar.gz; do
  [ -f "${artifact}" ] || continue
  cp "${artifact}" "${DIST_SNAPSHOT_DIR}/"
done

if ((${#BUMP_TYPES[@]} > 0)); then
  # ---------------------------------------------------------------------------
  # Bump mode: bump version, commit, tag, and push
  # ---------------------------------------------------------------------------
  _uv_bump_args=()
  for _bt in "${BUMP_TYPES[@]}"; do
    _uv_bump_args+=("--bump" "${_bt}")
  done
  UV_VERSION_OUTPUT=$(uv version "${_uv_bump_args[@]}")
  unset _uv_bump_args _bt
  # Extract package name (normalising underscores to dashes) and the new version
  # from uv's output in a single awk pass instead of two separate pipelines.
  read -r PACKAGE_NAME NEW_VERSION < <(awk '{gsub(/_/, "-", $1); print $1, $NF}' <<<"${UV_VERSION_OUTPUT}")
  # Projects that also ship a Rust binary (maturin) keep Cargo.toml's version in step
  if [ -f Cargo.toml ]; then
    sed -i -E "0,/^version = \"[^\"]+\"/s//version = \"${NEW_VERSION}\"/" Cargo.toml
    [ -f Cargo.lock ] && cargo update -q --workspace 2>/dev/null || true
  fi
  uv sync
  git add pyproject.toml uv.lock
  [ -f Cargo.toml ] && git add Cargo.toml Cargo.lock 2>/dev/null
  git commit -m "Bump version to ${NEW_VERSION}"
  COMMITTED=true
  git tag -a "v${NEW_VERSION}" -m "Version ${NEW_VERSION}"
  TAGGED=true
  git push --follow-tags
  PUSHED=true
  PUSHED_BRANCH=true

  # -------------------------------------------------------------------------
  # Build distribution artefacts
  # -------------------------------------------------------------------------
  rm -f dist/*.whl dist/*.tar.gz
  uv build

  # -------------------------------------------------------------------------
  # Publish to SFTPyPI
  # -------------------------------------------------------------------------
  # Remove any pre-existing version from the index before publishing to avoid
  # checksum-mismatch errors when re-releasing the same version number.
  echo "Checking SFTPyPI for a pre-existing v${NEW_VERSION}..."
  pre_delete_status=$(
    curl -s -o /dev/null -w "%{http_code}" \
      -u "${UV_INDEX_SFTPYPI_USERNAME}:${UV_INDEX_SFTPYPI_PASSWORD}" \
      -X DELETE "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/${PACKAGE_NAME}/${NEW_VERSION}"
  )
  case "$pre_delete_status" in
  200 | 204) echo "  -> Removed pre-existing v${NEW_VERSION} from SFTPyPI index." ;;
  404) echo "  -> No pre-existing v${NEW_VERSION} found on SFTPyPI index." ;;
  *) echo "  -> WARNING: Unexpected status ${pre_delete_status} when checking SFTPyPI for pre-existing version." ;;
  esac
  # Publish to devpi (SFTPyPI). Pass credentials explicitly — devpi does not
  # support OIDC/trusted publishing, so uv must receive them via CLI flags.
  uv publish --index SFTPyPI \
    --username "${UV_INDEX_SFTPYPI_USERNAME}" \
    --password "${UV_INDEX_SFTPYPI_PASSWORD}"
  PUBLISHED_PYPI=true

  # -------------------------------------------------------------------------
  # Create GitHub release
  # -------------------------------------------------------------------------
  if [ -n "${notes_text}" ]; then
    gh release create "v${NEW_VERSION}" dist/* --title "v${NEW_VERSION}" --notes "${notes_text}"
  else
    gh release create "v${NEW_VERSION}" dist/* --title "v${NEW_VERSION}" --generate-notes
  fi
  RELEASED_GITHUB=true

else
  # ---------------------------------------------------------------------------
  # No-bump mode: publish the current version without bumping
  # ---------------------------------------------------------------------------
  # Get current version and package name
  UV_VERSION_OUTPUT=$(uv version)
  read -r PACKAGE_NAME NEW_VERSION < <(awk '{gsub(/_/, "-", $1); print $1, $NF}' <<<"${UV_VERSION_OUTPUT}")

  # Abort if a local git tag already exists for this version
  if git rev-parse "v${NEW_VERSION}" >/dev/null 2>&1; then
    echo "ERROR: Local git tag v${NEW_VERSION} already exists." >&2
    exit 1
  fi
  # Abort if a remote git tag already exists for this version
  if git ls-remote --tags origin "refs/tags/v${NEW_VERSION}" | grep -q .; then
    echo "ERROR: Remote git tag v${NEW_VERSION} already exists." >&2
    exit 1
  fi
  # Abort if a GitHub release already exists for this version
  if gh release view "v${NEW_VERSION}" >/dev/null 2>&1; then
    echo "ERROR: GitHub release v${NEW_VERSION} already exists." >&2
    exit 1
  fi
  # Abort if SFTPyPI already has a package at this version
  _sftpypi_check_status=$(
    curl -s -o /dev/null -w "%{http_code}" \
      -u "${UV_INDEX_SFTPYPI_USERNAME}:${UV_INDEX_SFTPYPI_PASSWORD}" \
      "https://pypi.sweetfiretobacco.com/jacob.ogden/internal/${PACKAGE_NAME}/${NEW_VERSION}"
  )
  if [[ "${_sftpypi_check_status}" == "200" ]]; then
    echo "ERROR: Package ${PACKAGE_NAME}==${NEW_VERSION} already exists on SFTPyPI." >&2
    exit 1
  fi
  unset _sftpypi_check_status

  # -------------------------------------------------------------------------
  # Build distribution artefacts
  # -------------------------------------------------------------------------
  rm -f dist/*.whl dist/*.tar.gz
  uv build

  # -------------------------------------------------------------------------
  # Publish to SFTPyPI
  # -------------------------------------------------------------------------
  # Publish to devpi (SFTPyPI). Pass credentials explicitly — devpi does not
  # support OIDC/trusted publishing, so uv must receive them via CLI flags.
  uv publish --index SFTPyPI \
    --username "${UV_INDEX_SFTPYPI_USERNAME}" \
    --password "${UV_INDEX_SFTPYPI_PASSWORD}"
  PUBLISHED_PYPI=true

  # -------------------------------------------------------------------------
  # Tag and push
  # -------------------------------------------------------------------------
  git tag -a "v${NEW_VERSION}" -m "Version ${NEW_VERSION}"
  TAGGED=true
  git push origin "v${NEW_VERSION}"
  PUSHED=true

  # -------------------------------------------------------------------------
  # Create GitHub release
  # -------------------------------------------------------------------------
  if [ -n "${notes_text}" ]; then
    gh release create "v${NEW_VERSION}" dist/* --title "v${NEW_VERSION}" --notes "${notes_text}"
  else
    gh release create "v${NEW_VERSION}" dist/* --title "v${NEW_VERSION}" --generate-notes
  fi
  RELEASED_GITHUB=true

fi

# ---------------------------------------------------------------------------
# Restore development dependencies
# ---------------------------------------------------------------------------
uv sync --all-extras
