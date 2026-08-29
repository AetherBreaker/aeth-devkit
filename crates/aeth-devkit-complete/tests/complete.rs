use std::path::Path;

use aeth_devkit_complete::resolve::{Task, resolve};
use aeth_devkit_core::process::RecordingRunner;

fn project(pyproject: &str) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
  dir
}

fn write(root: &Path, rel: &str, content: &str) {
  let p = root.join(rel);
  std::fs::create_dir_all(p.parent().unwrap()).unwrap();
  std::fs::write(p, content).unwrap();
}

fn names(tasks: &[Task]) -> Vec<&str> {
  tasks.iter().map(|t| t.name.as_str()).collect()
}

#[test]
fn tasks_come_from_tool_poe_tasks_in_order_without_hidden_ones() {
  let dir = project("[tool.poe.tasks]\ntest = \"pytest\"\n_hidden = \"x\"\nlint.cmd = \"ruff check\"\n");
  let r = resolve(dir.path(), &RecordingRunner::new(0)).unwrap();
  assert_eq!(names(&r.tasks), ["test", "lint"]);
}

// ---- include (TOML) ----------------------------------------------------------------------

#[test]
fn included_files_add_tasks_after_the_projects_own_and_never_override() {
  let dir = project("[tool.poe]
include = \"extra.toml\"
[tool.poe.tasks]
build = \"x\"
");
  write(dir.path(), "extra.toml", "[tool.poe.tasks]
build = \"OVERRIDE\"
deploy = \"y\"
");
  let r = resolve(dir.path(), &RecordingRunner::new(0)).unwrap();
  assert_eq!(names(&r.tasks), ["build", "deploy"]);
  assert!(r.sources.iter().any(|p| p.ends_with("extra.toml")), "{:?}", r.sources);
}

#[test]
fn include_items_may_be_a_list_of_strings_or_tables_resolved_against_the_including_file() {
  let dir = project("[tool.poe]
include = [\"conf/a.toml\", { path = \"conf/b.toml\" }]
");
  write(dir.path(), "conf/a.toml", "[tool.poe]
include = \"nested.toml\"
[tool.poe.tasks]
a = \"1\"
");
  write(dir.path(), "conf/nested.toml", "[tool.poe.tasks]
nested = \"1\"
");
  write(dir.path(), "conf/b.toml", "[tool.poe.tasks]
b = \"1\"
");
  let r = resolve(dir.path(), &RecordingRunner::new(0)).unwrap();
  assert_eq!(names(&r.tasks), ["a", "nested", "b"]);
}

#[test]
fn cyclic_includes_terminate_and_recursive_false_is_honoured() {
  let dir = project("[tool.poe]
include = \"a.toml\"
");
  write(dir.path(), "a.toml", "[tool.poe]
include = \"b.toml\"
[tool.poe.tasks]
a = \"1\"
");
  write(dir.path(), "b.toml", "[tool.poe]
include = \"a.toml\"
[tool.poe.tasks]
b = \"1\"
");
  let r = resolve(dir.path(), &RecordingRunner::new(0)).unwrap();
  assert_eq!(names(&r.tasks), ["a", "b"]);

  let dir = project("[tool.poe]
include = { path = \"a.toml\", recursive = false }
");
  write(dir.path(), "a.toml", "[tool.poe]
include = \"b.toml\"
[tool.poe.tasks]
a = \"1\"
");
  write(dir.path(), "b.toml", "[tool.poe.tasks]
b = \"1\"
");
  let r = resolve(dir.path(), &RecordingRunner::new(0)).unwrap();
  assert_eq!(names(&r.tasks), ["a"]);
}

#[test]
fn a_missing_include_is_skipped_not_fatal() {
  let dir = project("[tool.poe]
include = \"nope.toml\"
[tool.poe.tasks]
x = \"1\"
");
  assert_eq!(names(&resolve(dir.path(), &RecordingRunner::new(0)).unwrap().tasks), ["x"]);
}

// ---- include_script ----------------------------------------------------------------------

const SCRIPT_CFG: &str = "[tool.poe]
include_script = [{ script = \"mypkg:tasks\", executor = { type = \"uv\" } }]
[tool.poe.tasks]
local = \"1\"
";

fn scripted(out: &str) -> RecordingRunner {
  let r = RecordingRunner::new(0);
  r.script("python", &["-c"], 0, out);
  r
}

#[test]
fn include_script_runs_poes_one_liner_against_the_venv_python_when_present() {
  let dir = project(SCRIPT_CFG);
  write(dir.path(), ".venv/Scripts/python.exe", "");
  let exe = dir.path().join(".venv").join("Scripts").join("python.exe");
  let runner = RecordingRunner::new(0);
  runner.script(&exe.to_string_lossy(), &["-c"], 0, r#"{"tasks": {"gen": {"cmd": "x"}}}"#);
  let r = resolve(dir.path(), &runner).unwrap();
  assert_eq!(names(&r.tasks), ["local", "gen"]);
  let calls = runner.calls.borrow();
  let py = calls.iter().find(|c| c.args.first().map(String::as_str) == Some("-c")).expect("python -c call");
  assert!(py.program.replace('\\', "/").ends_with(".venv/Scripts/python.exe"), "{}", py.program);
  let script = &py.args[1];
  assert!(script.contains("_i('mypkg')") && script.contains("json.dumps(_m.tasks())"), "{script}");
  assert_eq!(py.cwd, dir.path());
}

#[test]
fn include_script_output_shapes_are_all_normalized() {
  for out in [
    r#"{"tool": {"poe": {"tasks": {"gen": "x"}}}}"#,
    r#"{"tool.poe": {"tasks": {"gen": "x"}}}"#,
    r#"{"tasks": {"gen": "x"}, "config_path": "C:/x/mypkg/__init__.py"}"#,
    // poe accepts a JSON string containing JSON, too.
    r#""{\"tasks\": {\"gen\": \"x\"}}""#,
  ] {
    let dir = project(SCRIPT_CFG);
    let r = resolve(dir.path(), &scripted(out)).unwrap();
    assert_eq!(names(&r.tasks), ["local", "gen"], "for {out}");
  }
}

#[test]
fn include_script_config_path_is_a_cache_source() {
  let dir = project(SCRIPT_CFG);
  let r = resolve(dir.path(), &scripted(r#"{"tasks": {}, "config_path": "C:/x/mypkg/__init__.py"}"#)).unwrap();
  assert!(r.sources.iter().any(|p| p.to_string_lossy().ends_with("__init__.py")), "{:?}", r.sources);
}

#[test]
fn include_script_failures_fall_back_to_the_toml_tasks() {
  let dir = project(SCRIPT_CFG);
  let failing = RecordingRunner::new(1);
  assert_eq!(names(&resolve(dir.path(), &failing).unwrap().tasks), ["local"]);
  assert_eq!(names(&resolve(dir.path(), &scripted("not json")).unwrap().tasks), ["local"]);
}

// ---- args ----------------------------------------------------------------------------------

#[test]
fn task_args_are_normalized_like_poes_argspec() {
  let dir = project(
    "[tool.poe.tasks.t]
cmd = \"x\"
args = [
  \"plain\",
  { name = \"_priv\", options = [\"--priv\", \"-p\"], type = \"boolean\", help = \"Line one\\nline two\" },
  { name = \"flavor\", choices = [\"vanilla\", \"choc chip\"] },
  { name = \"_who\", positional = true },
  { name = \"where\", positional = \"WHERE\" },
]
[tool.poe.tasks.d]
cmd = \"y\"
[tool.poe.tasks.d.args]
verbose = { type = \"boolean\" }
",
  );
  let r = resolve(dir.path(), &RecordingRunner::new(0)).unwrap();
  let t = &r.tasks[0].args;
  assert_eq!((t[0].name.as_str(), &t[0].options[..], t[0].kind.as_str()), ("plain", &["--plain".to_string()][..], "string"));
  assert_eq!((&t[1].options[..], t[1].kind.as_str(), t[1].help.as_str()), (&["--priv".to_string(), "-p".to_string()][..], "boolean", "Line one\nline two"));
  assert_eq!(t[2].choices, ["vanilla", "choc chip"]);
  assert_eq!((&t[3].options[..], t[3].kind.as_str()), (&["priv".replace("priv", "who")][..], "positional"));
  assert_eq!(&t[4].options[..], &["WHERE".to_string()][..]);
  let d = &r.tasks[1].args;
  assert_eq!((d[0].name.as_str(), &d[0].options[..], d[0].kind.as_str()), ("verbose", &["--verbose".to_string()][..], "boolean"));
}
