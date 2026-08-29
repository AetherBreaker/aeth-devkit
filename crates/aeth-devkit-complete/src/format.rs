//! Render the resolved table in the exact text formats poe's completion scripts consume.
//!
//! Byte-compatibility matters: the generated shell scripts parse these with `-split`,
//! `read -r`, and friends, so any drift from `poe _list_tasks` / `poe _describe_task_args`
//! would show up as broken completions rather than as an error.

use crate::resolve::Task;

/// Help longer than this is cut with an ellipsis; a completion popup has no room for more.
const MAX_HELP: usize = 60;

/// `poe _list_tasks`: names on one space-separated line.
pub fn list_tasks(tasks: &[Task]) -> String {
  let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
  format!("{}\n", names.join(" "))
}

/// `poe _describe_task_args`: one tab-separated `options\ttype\thelp\tchoices` line per
/// argument. `_` stands in for "no choices" because `read` collapses consecutive tabs.
pub fn describe_task_args(task: &Task) -> String {
  let mut out = String::new();
  for arg in &task.args {
    let choices: Vec<String> = arg.choices.iter().filter(|c| !c.is_empty()).map(|c| escape_choice(c)).collect();
    let choices = if choices.is_empty() { "_".to_string() } else { choices.join(" ") };
    // Positionals print their single placeholder name; options print all spellings.
    let options = arg.options.join(",");
    out.push_str(&format!("{options}\t{}\t{}\t{choices}\n", arg.kind, format_help(&arg.help)));
  }
  out
}

/// Mirror of poe's `_format_help`: first line only, truncated with `...` at 60 chars, with
/// `\`, `:` and tab escaped (`:` is zsh's description separator).
fn format_help(text: &str) -> String {
  let line = text.lines().next().unwrap_or("").trim();
  if line.is_empty() {
    // A single space keeps the column present; an empty field can confuse `_describe`.
    return " ".to_string();
  }
  let truncated: String = if line.chars().count() > MAX_HELP {
    let head: String = line.chars().take(MAX_HELP - 3).collect();
    format!("{}...", head.trim_end())
  } else {
    line.to_string()
  };
  truncated.replace('\\', "\\\\").replace(':', "\\:").replace('\t', " ")
}

/// Mirror of poe's `_escape_choice`: single-quote a value containing shell-significant
/// characters, splicing embedded single quotes as `'\''` (end quote, escaped quote, reopen).
fn escape_choice(value: &str) -> String {
  const SPECIAL: [char; 8] = [' ', '\t', '\n', '"', '\'', '\\', '$', '`'];
  if value.contains(SPECIAL) {
    format!("'{}'", value.replace('\'', "'\\''"))
  } else {
    value.to_string()
  }
}
