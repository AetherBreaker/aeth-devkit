# Standard library imports
from collections.abc import Callable, Collection, Generator
from typing import Any

type TaskGenerator = Callable[..., Generator[tuple[str, dict[str, Any]]]]

class TaskCollection:
  def __init__(
    self,
    env: dict[str, str] | None = ...,
    envfile: list[str] | None = ...,
  ) -> None: ...
  @property
  def env(self) -> dict[str, str]: ...
  @property
  def envfile(self) -> list[str]: ...
  def add(
    self,
    task_name: str,
    task_config: dict[str, Any],
    tags: Collection[str] = (),
  ) -> TaskCollection: ...
  def remove(
    self,
    task_name: str,
    tags: Collection[str] = ...,
  ) -> TaskCollection: ...
  def include(self, other: TaskCollection) -> TaskCollection: ...
  def generate(self, source: TaskGenerator) -> TaskCollection: ...
  def __call__(
    self,
    include_tags: Collection[str] = ...,
    exclude_tags: Collection[str] = ...,
  ) -> dict[str, Any]: ...
  def script(
    self,
    func: Callable[..., Any] | None = ...,
    *,
    task_name: str | None = ...,
    help: str | None = ...,  # noqa: A002
    task_args: bool = ...,
    options: dict[str, Any] | None = ...,
    tags: Collection[str] = ...,
  ) -> Callable[..., Any]: ...

__all__ = ["TaskCollection"]
