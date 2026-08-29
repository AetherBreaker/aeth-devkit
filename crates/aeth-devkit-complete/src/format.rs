//! Render the resolved table in the exact text formats poe's completion scripts consume.

use crate::resolve::Task;

/// `poe _list_tasks`: names on one space-separated line.
pub fn list_tasks(tasks: &[Task]) -> String {
  let _ = tasks;
  String::new()
}

/// `poe _describe_task_args`: one tab-separated line per argument.
pub fn describe_task_args(task: &Task) -> String {
  let _ = task;
  String::new()
}
