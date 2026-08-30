"""Shared poe task definitions for projects that install aeth-devkit.

Consuming projects get these via `include_script = "aeth_devkit:tasks"`, written into their
pyproject.toml by `devkit setup-project`.

Tasks are authored in `_tasks_source.py` against poethepoet_tasks' TaskCollection and baked
into `_tasks_generated.py` by `crates/aeth-devkit/build.rs`. Only the baked table is imported
here: poe re-runs this import on every task invocation, and pulling in poethepoet_tasks costs
~24 ms of it. Edit `_tasks_source.py`, then build.
"""

from ._tasks_generated import tasks

__all__ = ["tasks"]
