"""The poe task table, baked from `_tasks_source.py` by `crates/aeth-devkit/build.rs`.

Do not edit by hand, and do not import poethepoet_tasks from here: this file exists so that
importing `aeth_devkit` costs ~0.8 ms rather than ~24 ms, which poe pays on every task run.
Edit `_tasks_source.py` and build.
"""

import os

# Absolute script paths cannot be baked: this package installs into each consuming project's
# own site-packages, so they are resolved from __file__ at import instead.
_SCRIPTS_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scripts").replace("\\", "/")
_PLACEHOLDER = "@@AETH_DEVKIT_SCRIPTS@@"

_RAW = {'env': {},
 'envfile': [],
 'tasks': {'release': {'help': 'Bump version, commit, tag, build, and publish to GitHub and the package '
                               'index. Pass one or more bump types as free positional args; valid values: '
                               'major, minor, patch, stable, alpha, beta, rc, post, dev. To include release '
                               'notes, append a multi-word string as the final arg (single-word trailing '
                               'args are treated as a typo and raise an error). Omit all bump types to '
                               'publish the current version without bumping. Pass --force / -f to skip the '
                               'confirmation prompts; --dry-run to only print the plan. Examples: poe '
                               "release patch | poe release major alpha | poe release minor 'first minor "
                               "release' | poe release 'publish notes'",
                       'envfile': '.env',
                       'cmd': 'devkit release $POE_EXTRA_ARGS'},
           'fix-bash': {'help': 'Windows-only: configure this workspace so all VS Code terminals prefer Git '
                                'Bash without changing global PATH',
                        'cmd': 'powershell -NoProfile -ExecutionPolicy Bypass -File '
                               '"@@AETH_DEVKIT_SCRIPTS@@/fix-git-bash-workspace.ps1"'},
           'docker-pin-latest': {'help': 'Auto-detect the docker compose file in the project root, resolve '
                                         'the version to pin (--version or latest stable release), and '
                                         'update PACKAGE_VERSION in place.',
                                 'cmd': 'bash "@@AETH_DEVKIT_SCRIPTS@@/docker-pin-latest.sh" "${version}"',
                                 'args': [{'name': 'version',
                                           'options': ['--version', '-V'],
                                           'default': '',
                                           'help': 'Pin to this exact version (supports pre-release versions '
                                                   'such as 1.2.0a1). If omitted, the latest stable release '
                                                   'is fetched from the package index.'}]},
           'release-and-pin': {'help': 'Bump version, commit, tag, build, and publish to GitHub and the '
                                       'package index, then pin the docker-compose package version. Pass one '
                                       'or more bump types as free positional args; valid values: major, '
                                       'minor, patch, stable, alpha, beta, rc, post, dev. To include release '
                                       'notes, append a multi-word string as the final arg (single-word '
                                       'trailing args are treated as a typo and raise an error). Pass '
                                       '--force / -f to skip the confirmation prompts. Examples: poe '
                                       'release-and-pin patch | poe release-and-pin major alpha | poe '
                                       "release-and-pin minor 'first minor release'",
                               'envfile': '.env',
                               'shell': 'devkit release $POE_EXTRA_ARGS && case " $POE_EXTRA_ARGS " in *" '
                                        '--dry-run "*) echo "Dry run: skipping docker pin." ;; *) bash '
                                        '"@@AETH_DEVKIT_SCRIPTS@@/docker-pin-latest.sh" "$(uv version '
                                        '--short)" ;; esac',
                               'interpreter': 'bash'},
           'rescind-release': {'help': 'Fully rescind a release: removes the package from the package index, '
                                       'deletes the GitHub release, and removes the Git tag (local and '
                                       'remote). Defaults to the most recent release; when defaulting, also '
                                       'rewinds the local branch to the previous release commit (all changes '
                                       'from the release are kept in the working tree). Usage: poe '
                                       'rescind-release [version]',
                               'envfile': '.env',
                               'cmd': 'bash "@@AETH_DEVKIT_SCRIPTS@@/rescind-release.sh" "${version}"',
                               'args': [{'name': 'version',
                                         'positional': True,
                                         'default': '',
                                         'help': 'Version to rescind (e.g. 1.2.3). Defaults to the most '
                                                 'recent release.'}]},
           'clean': {'script': '\n'
                               '          poethepoet.scripts:rm(\n'
                               '            ".coverage",\n'
                               '            ".ruff_cache",\n'
                               '            ".mypy_cache",\n'
                               '            ".pytest_cache",\n'
                               '            "./**/__pycache__",\n'
                               '            "dist",\n'
                               '            "htmlcov",\n'
                               "            verbosity=environ.get('POE_VERBOSITY'),\n"
                               '            dry_run=_dry_run\n'
                               '          )\n'
                               '        ',
                     'help': 'Remove generated files'},
           'lock': {'help': 'Update the aeth-devkit pin in pyproject.toml to the latest stable release on '
                            'its index, run uv sync (updating uv.lock), and commit the lockfile and pin '
                            'change with a standardized message. Skips the commit if nothing changed. Pass '
                            '--upgrade / --all-extras to forward the same flags to uv sync.',
                    'shell': 'devkit lock ${dry_run:+--dry-run} ${no_commit:+--no-commit} '
                             '${package:+--package "$package"} -- ${upgrade:+--upgrade} '
                             '${all_extras:+--all-extras}',
                    'interpreter': 'bash',
                    'args': [{'name': 'package',
                              'options': ['--package', '-p'],
                              'default': '',
                              'help': 'Dependency pin to bump instead of aeth-devkit'},
                             {'name': 'dry_run',
                              'options': ['--dry-run'],
                              'type': 'boolean',
                              'help': 'Report what would change without writing, syncing, or committing'},
                             {'name': 'no_commit',
                              'options': ['--no-commit'],
                              'type': 'boolean',
                              'help': 'Do not commit uv.lock / pyproject.toml after syncing'},
                             {'name': 'upgrade',
                              'options': ['--upgrade', '-U'],
                              'type': 'boolean',
                              'help': 'Allow package upgrades, ignoring pinned versions in uv.lock (uv sync '
                                      '--upgrade)'},
                             {'name': 'all_extras',
                              'options': ['--all-extras'],
                              'type': 'boolean',
                              'help': 'Include all optional dependencies (uv sync --all-extras)'}]},
           'setup-project': {'help': "Standardize this project's configuration from the templates shipped "
                                     'with aeth-devkit (cache dirs under .cache/, PYTHONPYCACHEPREFIX in '
                                     '.env and VS Code, inlined ruff/pyright config, '
                                     '.gitignore/.gitattributes/.dockerignore). Idempotent. Extra args are '
                                     'passed to devkit setup-project: --dry-run, --check, --no-commit, '
                                     '--templates-dir PATH.',
                             'cmd': 'devkit setup-project'}}}


# Annotated as `object` rather than via typing: importing typing would cost ~5 ms, most of
# what this file exists to save.
def _resolve(value: object) -> object:
  """Substitute the real scripts directory into every string, at any nesting depth."""
  if isinstance(value, str):
    return value.replace(_PLACEHOLDER, _SCRIPTS_DIR)
  if isinstance(value, dict):
    return {k: _resolve(v) for k, v in value.items()}
  if isinstance(value, list):
    return [_resolve(v) for v in value]
  return value


def tasks(include_tags: object = (), exclude_tags: object = ()) -> dict:
  """The task table poe loads via `include_script = "aeth_devkit:tasks"`.

  `include_tags`/`exclude_tags` are accepted for signature parity with TaskCollection but
  ignored: no task here declares tags, and poe itself has no notion of them — filtering was
  always the callable's own job. Add it here if a consumer ever needs it.
  """
  resolved = _resolve(_RAW)
  # Narrowing for the type checker; _RAW is a dict literal, so this always holds.
  assert isinstance(resolved, dict)
  return {**resolved, "config_path": __file__}
