"""Source of truth for the shared poe task table.

This module is the one place tasks are defined, but it is *not* what consumers import at
runtime: importing `poethepoet_tasks` costs ~24 ms, most of it `inspect`, and poe pays that
on every task run and every Tab press. `crates/aeth-devkit/build.rs` calls `tasks()` here once
during the build and bakes the result into `_tasks_generated.py`, which
`aeth_devkit/__init__.py` re-exports.

Edit tasks here, then build. Nothing imports this at runtime.
"""

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
      "Bump version, commit, tag, push, create the GitHub release, then wait for the release workflow to build and publish. "
      "Pass one or more bump types as free positional args; "
      "valid values: major, minor, patch, stable, alpha, beta, rc, post, dev. "
      "To include release notes, append a multi-word string as the final arg "
      "(single-word trailing args are treated as a typo and raise an error). "
      "Omit all bump types to publish the current version without bumping. "
      "Pass --force / -f to skip the confirmation prompts; --dry-run to only print the plan. "
      "--no-wait returns as soon as the GitHub release exists. "
      "Examples: "
      "poe release patch | "
      "poe release major alpha | "
      "poe release minor 'first minor release' | "
      "poe release 'publish notes'"
    ),
    "envfile": ".env",
    # No declared args on purpose: poe would then reject the free positionals. Everything
    # (bump words, quoted notes, --force/-f, --dry-run) is forwarded verbatim and parsed by
    # `devkit release` itself.
    "cmd": "devkit release $POE_EXTRA_ARGS",
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
  task_name="docker-pin",
  task_config={
    "help": (
      "Pin the docker compose file to a released version of this project. "
      "Auto-detects the compose file and the services that build this package, verifies the "
      "release is complete (GitHub tag+release and every publish index), then updates "
      "GIT_TAG / PACKAGE_VERSION, commits, and pushes."
    ),
    "shell": (
      'devkit docker-pin ${version:+--version "$version"} ${dry_run:+--dry-run} '
      '${no_commit:+--no-commit} ${no_push:+--no-push} ${compose_file:+--compose-file "$compose_file"}'
    ),
    "interpreter": "bash",
    "args": [
      {
        "name": "version",
        "options": ["--version", "-V"],
        "default": "",
        "help": (
          "Pin to this exact version (supports pre-release versions such as 1.2.0a1). "
          "If omitted, the latest stable complete release is used."
        ),
      },
      {
        "name": "dry_run",
        "options": ["--dry-run"],
        "type": "boolean",
        "help": "Resolve and report without changing anything",
      },
      {
        "name": "no_commit",
        "options": ["--no-commit"],
        "type": "boolean",
        "help": "Edit the compose file but do not commit (implies --no-push)",
      },
      {
        "name": "no_push",
        "options": ["--no-push"],
        "type": "boolean",
        "help": "Commit locally but do not push",
      },
      {
        "name": "compose_file",
        "options": ["--compose-file", "-c"],
        "default": "",
        "help": "Compose file to edit (default: auto-discover from the repo root)",
      },
    ],
  },
)

tasks.add(
  task_name="release-and-pin",
  task_config={
    "help": (
      "Bump version, commit, tag, push, create the GitHub release, wait for the release workflow "
      "to build and publish, then pin the docker-compose package version. "
      "Pass one or more bump types as free positional args; "
      "valid values: major, minor, patch, stable, alpha, beta, rc, post, dev. "
      "To include release notes, append a multi-word string as the final arg "
      "(single-word trailing args are treated as a typo and raise an error). "
      "Pass --force / -f to skip the confirmation prompts; --dry-run stays dry (the pin step is skipped). "
      "--no-wait is refused: the pin needs the artefacts the workflow publishes. "
      "Examples: "
      "poe release-and-pin patch | "
      "poe release-and-pin major alpha | "
      "poe release-and-pin minor 'first minor release'"
    ),
    "envfile": ".env",
    # Free positionals are forwarded verbatim and parsed by `devkit release-and-pin` itself;
    # the pin step runs in-process after a completed release, never on --dry-run.
    "cmd": "devkit release-and-pin $POE_EXTRA_ARGS",
  },
)

tasks.add(
  task_name="rescind-release",
  task_config={
    "help": (
      "Fully rescind a release: removes the package from the package index, deletes the GitHub release, "
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
      "Update the aeth-devkit pin in pyproject.toml to the latest stable release on its index, "
      "run uv sync (updating uv.lock), and commit the lockfile and pin change with a "
      "standardized message. Skips the commit if nothing changed. "
      "Pass --upgrade / --all-extras to forward the same flags to uv sync."
    ),
    "shell": (
      'devkit lock ${dry_run:+--dry-run} ${no_commit:+--no-commit} ${package:+--package "$package"}'
      " -- ${upgrade:+--upgrade} ${all_extras:+--all-extras}"
    ),
    "interpreter": "bash",
    "args": [
      {
        "name": "package",
        "options": ["--package", "-p"],
        "default": "",
        "help": "Dependency pin to bump instead of aeth-devkit",
      },
      {
        "name": "dry_run",
        "options": ["--dry-run"],
        "type": "boolean",
        "help": "Report what would change without writing, syncing, or committing",
      },
      {
        "name": "no_commit",
        "options": ["--no-commit"],
        "type": "boolean",
        "help": "Do not commit uv.lock / pyproject.toml after syncing",
      },
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


tasks.add(
  task_name="setup-project",
  task_config={
    "help": (
      "Standardize this project's configuration from the templates shipped with aeth-devkit "
      "(cache dirs under .cache/, PYTHONPYCACHEPREFIX in .env and VS Code, inlined ruff/pyright "
      "config, .gitignore/.gitattributes/.dockerignore). Idempotent. "
      "Extra args are passed to devkit setup-project: --dry-run, --check, --no-commit, --templates-dir PATH."
    ),
    "cmd": "devkit setup-project",
  },
)
