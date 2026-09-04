"""Guards for the build-time regeneration of the baked task table.

`crates/aeth-devkit/build.rs` bakes the task table before the crate compiles, so any
build — `cargo build`, `uv build`, `uv sync`, `pip install` — ships a table matching its own
source. maturin drives cargo, so this covers the packaging paths too without a custom PEP 517
backend.
"""

# Standard library imports
import subprocess
import tomllib
from pathlib import Path

# Third party imports
import pytest

REPO = Path(__file__).resolve().parent.parent
GENERATED = REPO / "python" / "aeth_devkit" / "_tasks_generated.py"
SOURCE = REPO / "python" / "aeth_devkit" / "_tasks_source.py"
BUILD_RS = REPO / "crates" / "aeth-devkit" / "build.rs"
PYPROJECT = REPO / "pyproject.toml"


@pytest.fixture
def restore_generated():
  """Put the generated file back byte-for-byte however the test ends.

  Read and written as bytes: text mode would translate the file's LF endings to CRLF on
  Windows when restoring it, quietly rewriting every line of the very file this guards.
  """
  original = GENERATED.read_bytes()
  yield original
  GENERATED.write_bytes(original)


def test_cargo_build_regenerates_a_stale_task_table(restore_generated: bytes) -> None:
  """The whole point: building must bake a fresh table, with nothing to remember.

  build.rs reruns when `_tasks_source.py` changes, so the source is touched to trigger it —
  that is the real-world sequence (edit tasks, build) this guards.
  """
  GENERATED.write_bytes(b"# stale\n")
  SOURCE.touch()

  result = subprocess.run(
    ["cargo", "build", "-p", "aeth-devkit"],
    cwd=REPO,
    capture_output=True,
    encoding="utf-8",
    errors="replace",
    check=False,
  )

  assert result.returncode == 0, f"cargo build failed:\n{result.stderr[-2000:]}"
  # Compared as bytes so a line-ending regression counts as a difference, not a silent pass.
  assert GENERATED.read_bytes() == restore_generated, "cargo build did not regenerate the task table"


def test_build_rs_is_wired_into_the_crate():
  """A build.rs only runs if cargo can see it; a rename would silently disable regeneration."""
  assert BUILD_RS.is_file(), f"{BUILD_RS} is missing, so nothing regenerates at build time"


def test_generation_needs_no_loose_files_outside_the_crate():
  """build.rs owns the whole bake, so nothing under python/ has to be packaged for it.

  A helper script beside the package would have to be added to `[tool.maturin] include` or
  source builds would break; keeping the generator inside build.rs means the crate carries it
  and the sdist gets it for free.
  """
  assert not (REPO / "python" / "gen_tasks.py").exists(), "generator escaped back out of build.rs"

  config = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
  included = {e["path"] for e in config["tool"]["maturin"].get("include", []) if isinstance(e, dict)}
  assert "python/gen_tasks.py" not in included, "stale include for a generator that no longer exists"


def test_task_source_is_excluded_from_the_wheel():
  """`_tasks_source.py` imports poethepoet_tasks, which consumers no longer install.

  Nothing imports it at runtime, but shipping a module that raises ImportError on import is
  a trap for anything that walks the package. The sdist still needs it — that is what a
  source build regenerates from — so the exclusion is wheel-only.

  Asserted against config rather than a built wheel because `uv build` runs cargo.
  """
  config = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
  excluded = {(e["path"], e.get("format")) for e in config["tool"]["maturin"].get("exclude", []) if isinstance(e, dict)}
  assert ("python/aeth_devkit/_tasks_source.py", "wheel") in excluded, (
    "_tasks_source.py must be excluded from the wheel but kept in the sdist"
  )


def test_build_backend_is_plain_maturin():
  """No custom PEP 517 shim: build.rs is the whole mechanism."""
  config = tomllib.loads(PYPROJECT.read_text(encoding="utf-8"))
  assert config["build-system"]["build-backend"] == "maturin"
  assert "backend-path" not in config["build-system"]
