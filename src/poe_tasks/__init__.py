# Standard library imports
import os

# Third party imports
from poethepoet_tasks import TaskCollection

_SCRIPTS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scripts")


def _script_path(filename: str) -> str:
  return os.path.join(_SCRIPTS_DIR, filename).replace("\\", "/")


tasks = TaskCollection()

tasks.add(
  task_name="release",
  task_config={
    "help": (
      "Bump version, commit, tag, build, and publish to GitHub and SFTPyPI. "
      "Pass one or more bump types as free positional args; "
      "valid values: major, minor, patch, stable, alpha, beta, rc, post, dev. "
      "To include release notes, append a multi-word string as the final arg "
      "(single-word trailing args are treated as a typo and raise an error). "
      "Omit all bump types to publish the current version without bumping. "
      "Pass --force / -f to skip the uncommitted-changes prompt. "
      "Examples: "
      "poe release patch | "
      "poe release major alpha | "
      "poe release minor 'first minor release' | "
      "poe release 'publish notes'"
    ),
    "envfile": ".env",
    "cmd": f'bash "{_script_path("release.sh")}" $POE_EXTRA_ARGS',
  },
)

tasks.add(
  task_name="fix-bash",
  task_config={
    "help": "Windows-only: configure this workspace so all VS Code terminals prefer Git Bash without changing global PATH",
    "cmd": f'powershell -NoProfile -ExecutionPolicy Bypass -File "{_script_path("fix-git-bash-workspace.ps1")}"',
  },
)

tasks.add(
  task_name="docker-pin-latest",
  task_config={
    "help": (
      "Auto-detect the docker compose file in the project root, resolve the version to pin "
      "(--version or latest stable from SFTPyPI), and update PACKAGE_VERSION in place."
    ),
    "cmd": f'bash "{_script_path("docker-pin-latest.sh")}" "${{version}}"',
    "args": [
      {
        "name": "version",
        "options": ["--version", "-V"],
        "default": "",
        "help": (
          "Pin to this exact version (supports pre-release versions such as 1.2.0a1). "
          "If omitted, the latest stable release is fetched from SFTPyPI."
        ),
      },
    ],
  },
)

tasks.add(
  task_name="release-and-pin",
  task_config={
    "help": (
      "Bump version, commit, tag, build, and publish to GitHub and SFTPyPI, "
      "then pin the docker-compose package version. "
      "Pass one or more bump types as free positional args; "
      "valid values: major, minor, patch, stable, alpha, beta, rc, post, dev. "
      "To include release notes, append a multi-word string as the final arg "
      "(single-word trailing args are treated as a typo and raise an error). "
      "Pass --force / -f to skip the uncommitted-changes prompt. "
      "Examples: "
      "poe release-and-pin patch | "
      "poe release-and-pin major alpha | "
      "poe release-and-pin minor 'first minor release'"
    ),
    "envfile": ".env",
    "shell": (
      f'bash "{_script_path("release.sh")}" $POE_EXTRA_ARGS && bash "{_script_path("docker-pin-latest.sh")}" "$(uv version --short)"'
    ),
    "interpreter": "bash",
  },
)

tasks.add(
  task_name="rescind-release",
  task_config={
    "help": (
      "Fully rescind a release: removes the package from SFTPyPI, deletes the GitHub release, "
      "and removes the Git tag (local and remote). Defaults to the most recent release; "
      "when defaulting, also rewinds the local branch to the previous release commit "
      "(all changes from the release are kept in the working tree). "
      "Usage: poe rescind-release [version]"
    ),
    "envfile": ".env",
    "cmd": f'bash "{_script_path("rescind-release.sh")}" "${{version}}"',
    "args": [
      {
        "name": "version",
        "positional": True,
        "default": "",
        "help": "Version to rescind (e.g. 1.2.3). Defaults to the most recent release.",
      },
    ],
  },
)


tasks.add(
  task_name="clean",
  task_config={
    "script": """
          poethepoet.scripts:rm(
            ".coverage",
            ".ruff_cache",
            ".mypy_cache",
            ".pytest_cache",
            "./**/__pycache__",
            "dist",
            "htmlcov",
            verbosity=environ.get('POE_VERBOSITY'),
            dry_run=_dry_run
          )
        """,
    "help": "Remove generated files",
  },
)


tasks.add(
  task_name="lock",
  task_config={
    "help": (
      "Run uv sync (updating uv.lock) and commit the lockfile with a standardized message. "
      "Skips the commit if the lockfile is unchanged. "
      "Pass --upgrade / --all-extras to forward the same flags to uv sync."
    ),
    "shell": (
      "uv sync ${upgrade:+--upgrade} ${all_extras:+--all-extras} && "
      "if git diff --quiet -- uv.lock; then echo 'uv.lock is up to date; nothing to commit'; "
      'else git add uv.lock && git commit -m "Update uv.lock"; fi'
    ),
    "interpreter": "bash",
    "args": [
      {
        "name": "upgrade",
        "options": ["--upgrade", "-U"],
        "type": "boolean",
        "help": "Allow package upgrades, ignoring pinned versions in uv.lock (uv sync --upgrade)",
      },
      {
        "name": "all_extras",
        "options": ["--all-extras"],
        "type": "boolean",
        "help": "Include all optional dependencies (uv sync --all-extras)",
      },
    ],
  },
)
