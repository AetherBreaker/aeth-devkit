use std::path::Path;

use aeth_devkit_complete::cache;
use aeth_devkit_complete::format::{describe_task_args, list_tasks};
use aeth_devkit_complete::resolve::{Resolved, Task, TaskArg, resolve};
use aeth_devkit_core::process::{RecordingRunner, SystemRunner};

fn arg(name: &str, options: &[&str], kind: &str, help: &str, choices: &[&str]) -> TaskArg {
  TaskArg {
    name: name.into(),
    options: options.iter().map(|s| s.to_string()).collect(),
    kind: kind.into(),
    help: help.into(),
    choices: choices.iter().map(|s| s.to_string()).collect(),
  }
}

// ---- format: byte-compatible with `poe _list_tasks` / `poe _describe_task_args` ----------

#[test]
fn list_tasks_is_one_space_separated_line() {
  let tasks = vec![Task { name: "a".into(), args: vec![] }, Task { name: "b-c".into(), args: vec![] }];
  assert_eq!(list_tasks(&tasks), "a b-c\n");
  assert_eq!(list_tasks(&[]), "\n");
}

#[test]
fn describe_task_args_uses_poes_tab_separated_format() {
  let task = Task {
    name: "t".into(),
    args: vec![
      arg("greeting", &["--greeting", "-g"], "string", "The greeting to use", &[]),
      arg("verbose", &["--verbose", "-v"], "boolean", "", &[]),
      arg("flavor", &["--flavor"], "string", "Flavor", &["vanilla", "choc chip"]),
      arg("name", &["name"], "positional", "The name", &[]),
    ],
  };
  let out = describe_task_args(&task);
  let lines: Vec<&str> = out.lines().collect();
  assert_eq!(lines[0], "--greeting,-g\tstring\tThe greeting to use\t_");
  // Empty help is a single space, so shells that split on tabs keep the column.
  assert_eq!(lines[1], "--verbose,-v\tboolean\t \t_");
  // A choice containing a space is single-quoted, like poe's _escape_choice.
  assert_eq!(lines[2], "--flavor\tstring\tFlavor\tvanilla 'choc chip'");
  assert_eq!(lines[3], "name\tpositional\tThe name\t_");
  assert_eq!(lines.len(), 4);
}

#[test]
fn help_is_first_line_only_truncated_and_escaped() {
  let long = "x".repeat(70);
  let task = Task {
    name: "t".into(),
    args: vec![
      arg("a", &["--a"], "string", "first line\nsecond line", &[]),
      arg("b", &["--b"], "string", &long, &[]),
      arg("c", &["--c"], "string", "path: C:\\x\tdone", &[]),
    ],
  };
  let out = describe_task_args(&task);
  let help: Vec<&str> = out.lines().map(|l| l.split('\t').nth(2).unwrap()).collect();
  assert_eq!(help[0], "first line");
  assert_eq!(help[1], format!("{}...", "x".repeat(57)));
  assert_eq!(help[2], "path\\: C\\:\\\\x done");
}

#[test]
fn choices_with_quotes_use_shell_quote_splicing() {
  let task = Task {
    name: "t".into(),
    args: vec![arg("q", &["--q"], "string", "", &["it's", "plain", "a$b"])],
  };
  let out = describe_task_args(&task);
  let choices = out.lines().next().unwrap().split('\t').nth(3).unwrap();
  assert_eq!(choices, "'it'\\''s' plain 'a$b'");
}

#[test]
fn a_task_with_no_args_describes_as_nothing() {
  assert_eq!(describe_task_args(&Task { name: "t".into(), args: vec![] }), "");
}

// ---- cache ---------------------------------------------------------------------------------

fn project_with_tasks(names: &[&str]) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  let body: String = names.iter().map(|n| format!("{n} = \"x\"\n")).collect();
  std::fs::write(dir.path().join("pyproject.toml"), format!("[tool.poe.tasks]\n{body}")).unwrap();
  dir
}

fn cached_names(root: &Path, runner: &RecordingRunner, bypass: bool) -> Vec<String> {
  cache::resolve_cached(root, runner, bypass).unwrap().tasks.into_iter().map(|t| t.name).collect()
}

#[test]
fn cold_run_populates_the_cache_file_and_warm_run_reads_it() {
  let dir = project_with_tasks(&["a"]);
  let runner = RecordingRunner::new(0);
  assert_eq!(cached_names(dir.path(), &runner, false), ["a"]);
  let cache_file = dir.path().join(".cache").join("devkit-completions.json");
  assert!(cache_file.is_file());

  // Tamper with the cache to prove the warm path reads it rather than re-resolving.
  let mut stored: cache::Stored = serde_json::from_str(&std::fs::read_to_string(&cache_file).unwrap()).unwrap();
  stored.resolved.tasks[0].name = "from-cache".into();
  std::fs::write(&cache_file, serde_json::to_string(&stored).unwrap()).unwrap();
  assert_eq!(cached_names(dir.path(), &runner, false), ["from-cache"]);
}

#[test]
fn editing_pyproject_invalidates() {
  let dir = project_with_tasks(&["a"]);
  let runner = RecordingRunner::new(0);
  cached_names(dir.path(), &runner, false);
  // A different size guarantees invalidation even if the mtime granularity is coarse.
  std::fs::write(dir.path().join("pyproject.toml"), "[tool.poe.tasks]\na = \"x\"\nbb = \"y\"\n").unwrap();
  assert_eq!(cached_names(dir.path(), &runner, false), ["a", "bb"]);
}

#[test]
fn editing_an_include_target_invalidates() {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(dir.path().join("pyproject.toml"), "[tool.poe]\ninclude = \"x.toml\"\n").unwrap();
  std::fs::write(dir.path().join("x.toml"), "[tool.poe.tasks]\ni = \"1\"\n").unwrap();
  let runner = RecordingRunner::new(0);
  assert_eq!(cached_names(dir.path(), &runner, false), ["i"]);
  std::fs::write(dir.path().join("x.toml"), "[tool.poe.tasks]\ni = \"1\"\njj = \"2\"\n").unwrap();
  assert_eq!(cached_names(dir.path(), &runner, false), ["i", "jj"]);
}

#[test]
fn a_different_devkit_version_invalidates() {
  let dir = project_with_tasks(&["a"]);
  let runner = RecordingRunner::new(0);
  cached_names(dir.path(), &runner, false);
  let cache_file = dir.path().join(".cache").join("devkit-completions.json");
  let mut stored: cache::Stored = serde_json::from_str(&std::fs::read_to_string(&cache_file).unwrap()).unwrap();
  stored.resolved.tasks[0].name = "stale".into();
  stored.fingerprint.devkit_version = "0.0.0".into();
  std::fs::write(&cache_file, serde_json::to_string(&stored).unwrap()).unwrap();
  assert_eq!(cached_names(dir.path(), &runner, false), ["a"]);
}

#[test]
fn bypass_ignores_and_rewrites_the_cache() {
  let dir = project_with_tasks(&["a"]);
  let runner = RecordingRunner::new(0);
  cached_names(dir.path(), &runner, false);
  let cache_file = dir.path().join(".cache").join("devkit-completions.json");
  let mut stored: cache::Stored = serde_json::from_str(&std::fs::read_to_string(&cache_file).unwrap()).unwrap();
  stored.resolved.tasks[0].name = "stale".into();
  std::fs::write(&cache_file, serde_json::to_string(&stored).unwrap()).unwrap();
  assert_eq!(cached_names(dir.path(), &runner, true), ["a"]);
  let after: cache::Stored = serde_json::from_str(&std::fs::read_to_string(&cache_file).unwrap()).unwrap();
  assert_eq!(after.resolved.tasks[0].name, "a");
}

#[test]
fn a_corrupt_cache_file_is_ignored() {
  let dir = project_with_tasks(&["a"]);
  std::fs::create_dir_all(dir.path().join(".cache")).unwrap();
  std::fs::write(dir.path().join(".cache").join("devkit-completions.json"), "{not json").unwrap();
  assert_eq!(cached_names(dir.path(), &RecordingRunner::new(0), false), ["a"]);
}

// ---- parity with the real poe, on this repo ---------------------------------------------

/// The one guard against the Rust resolver silently diverging from poe. Skips (with a
/// note) when this repo's venv has no `poe`, so it never fails for environmental reasons.
#[test]
fn resolved_tasks_match_poe_list_tasks_for_this_repo() {
  let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
  let poe = root.join(".venv").join("Scripts").join("poe.exe");
  let poe = if poe.is_file() { poe } else { root.join(".venv").join("bin").join("poe") };
  if !poe.is_file() {
    eprintln!("skipping parity test: no venv poe at {}", poe.display());
    return;
  }
  let out = std::process::Command::new(&poe).arg("_list_tasks").current_dir(&root).output().unwrap();
  let mut expected: Vec<String> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::to_string).collect();
  expected.sort();
  assert!(!expected.is_empty(), "poe listed no tasks");

  let r: Resolved = resolve(&root, &SystemRunner).unwrap();
  let mut actual: Vec<String> = r.tasks.into_iter().map(|t| t.name).collect();
  actual.sort();
  assert_eq!(actual, expected);
}
