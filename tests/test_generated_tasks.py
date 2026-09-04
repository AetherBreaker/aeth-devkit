"""Guards for the generated task table.

`crates/aeth-devkit/build.rs` bakes the TaskCollection-built task table into
`aeth_devkit/_tasks_generated.py` so the runtime never imports poethepoet_tasks. The
staleness check lives in `test_build_regenerates.py`, since regenerating means building;
what is checked here is the content of the baked file.
"""

# Standard library imports
import json
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
GENERATED = REPO / "python" / "aeth_devkit" / "_tasks_generated.py"
PYPROJECT = REPO / "pyproject.toml"

# Kept in step with PLACEHOLDER in crates/aeth-devkit/build.rs.
PLACEHOLDER = "@@AETH_DEVKIT_SCRIPTS@@"


def test_importing_aeth_devkit_does_not_import_poethepoet_tasks():
  """The entire point of the codegen: poethepoet_tasks costs ~24 ms of import time."""
  probe = "import aeth_devkit, sys; print('poethepoet_tasks' in sys.modules)"
  out = subprocess.run([sys.executable, "-c", probe], capture_output=True, encoding="utf-8", check=True)
  assert out.stdout.strip() == "False", "poethepoet_tasks was imported at runtime"


def test_poethepoet_tasks_is_a_build_dependency_only():
  """Consumers should not install a package only the build imports.

  Note this is poethepoet-tasks (the TaskCollection helper), not poethepoet (the runner) —
  the latter stays a runtime dependency, and the `clean` task's `poethepoet.scripts:rm`
  comes from it.
  """
  config = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
  runtime = " ".join(config["project"]["dependencies"])
  build = " ".join(config["build-system"]["requires"])

  assert "poethepoet-tasks" not in runtime, "poethepoet-tasks is not imported at runtime"
  assert "poethepoet-tasks" in build, "the build imports it, so it must stay a build requirement"
  assert "poethepoet>" in runtime or "poethepoet=" in runtime, "poe itself runs the tasks"


def test_generated_file_holds_no_absolute_paths():
  r"""Script paths must be a placeholder in the file, resolved from __file__ at import.

  Baking the generating machine's absolute paths would ship dead paths to every consumer,
  since the package installs into each project's own site-packages.

  Matched against this checkout's actual scripts directory rather than a pattern for what an
  absolute path looks like. A drive-letter heuristic (`":/"`, `":\\"`) passes silently on
  Linux and macOS, where the path that would leak contains no colon at all — so it would have
  guarded nothing on exactly the platforms it was not developed on.
  """
  text = GENERATED.read_text(encoding="utf-8")
  scripts_dir = REPO / "python" / "aeth_devkit" / "scripts"

  # Both separator styles: _script_path() emits forward slashes, but a future change might not.
  for form in {str(scripts_dir), scripts_dir.as_posix()}:
    assert form not in text, f"absolute script path frozen into {GENERATED.name}: {form}"

  # The converse: if substitution stopped happening entirely there would be nothing to leak
  # and the check above would pass vacuously.
  assert PLACEHOLDER in text, f"no {PLACEHOLDER} in {GENERATED.name}; did placeholding stop running?"


def test_script_paths_resolve_next_to_the_installed_package():
  # First party imports
  import aeth_devkit

  pkg_scripts = (Path(aeth_devkit.__file__).parent / "scripts").as_posix()
  blob = json.dumps(aeth_devkit.tasks())
  assert pkg_scripts in blob, f"no task references the installed scripts dir {pkg_scripts}"


def test_generated_tasks_match_the_task_collection_source():
  """The bake must reproduce exactly what the live TaskCollection would have returned."""
  sys.path.insert(0, str(REPO / "python"))
  # First party imports
  import aeth_devkit
  from aeth_devkit import _tasks_source

  assert aeth_devkit.tasks()["tasks"] == _tasks_source.tasks()["tasks"]
