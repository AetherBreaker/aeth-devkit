use std::path::Path;

use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_hooks::{Hook, run};
use serde_json::{Value, json};

fn decide(hook: Hook, payload: Value) -> Option<Value> {
  let runner = RecordingRunner::new(0);
  run(hook, &payload.to_string(), Path::new("."), &runner).map(|s| serde_json::from_str(&s).expect("hook output must be JSON"))
}

#[test]
fn pre_edit_protect_denies_env_file() {
  let out = decide(Hook::PreEditProtect, json!({"tool_input": {"file_path": "d:/proj/sub/.env"}})).expect("must deny");
  let h = &out["hookSpecificOutput"];
  assert_eq!(h["hookEventName"], "PreToolUse");
  assert_eq!(h["permissionDecision"], "deny");
  assert!(h["permissionDecisionReason"].as_str().unwrap().contains(".env"));
}

#[test]
fn pre_edit_protect_allows_other_files() {
  assert_eq!(
    decide(Hook::PreEditProtect, json!({"tool_input": {"file_path": "src/main.py"}})),
    None
  );
}

#[test]
fn pre_bash_protect_deps_denies_uv_add() {
  let out = decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": "uv add requests"}})).expect("must deny");
  let h = &out["hookSpecificOutput"];
  assert_eq!(h["permissionDecision"], "deny");
  assert!(h["permissionDecisionReason"].as_str().unwrap().contains("uv add requests"));
}

#[test]
fn pre_bash_protect_deps_denies_after_separator() {
  assert!(decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": "cd x && uv lock"}})).is_some());
  assert!(
    decide(
      Hook::PreBashProtectDeps,
      json!({"tool_input": {"command": "if true; then uv remove foo; fi"}})
    )
    .is_some()
  );
}

#[test]
fn pre_bash_protect_deps_allows_other_uv_commands() {
  assert_eq!(
    decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": "uv run pytest"}})),
    None
  );
  assert_eq!(
    decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": "uv sync"}})),
    None
  );
  assert_eq!(
    decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": "echo uv-add"}})),
    None
  );
}

#[test]
fn malformed_stdin_is_silent() {
  let runner = RecordingRunner::new(0);
  assert_eq!(run(Hook::PreEditProtect, "not json", Path::new("."), &runner), None);
  assert_eq!(run(Hook::PreBashProtectDeps, "", Path::new("."), &runner), None);
}

// ---- Stop hooks ------------------------------------------------------------------------

fn context(out: Option<String>) -> Option<String> {
  out.map(|s| {
    let v: Value = serde_json::from_str(&s).unwrap();
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "Stop");
    v["hookSpecificOutput"]["additionalContext"].as_str().unwrap().to_string()
  })
}

#[test]
fn stop_ruff_reports_output_on_failure() {
  let runner = RecordingRunner::new(1);
  runner.script("uv", &["run", "ruff"], 1, "src/x.py:1:1: E501 too long\n");
  let ctx = context(run(Hook::StopRuff, "{}", Path::new("."), &runner)).expect("must report");
  assert_eq!(ctx, "ruff check (project-wide) reported issues:\nsrc/x.py:1:1: E501 too long");
}

#[test]
fn stop_hooks_fall_back_to_uv_run_with_the_right_args_and_cwd() {
  let dir = tempfile::tempdir().unwrap();
  let runner = RecordingRunner::new(0);
  run(Hook::StopRuff, "{}", dir.path(), &runner);
  run(Hook::StopPyright, "{}", dir.path(), &runner);
  run(Hook::StopClean, "{}", dir.path(), &runner);
  let uv = runner.calls_for("uv");
  assert_eq!(uv[0], ["run", "ruff", "check", "--fix", "--unfixable", "F401", "."]);
  assert_eq!(uv[1], ["run", "pyright"]);
  assert_eq!(uv[2], ["run", "poe", "clean"]);
  assert!(runner.calls.borrow().iter().all(|c| c.cwd == dir.path()));
}

#[test]
fn stop_hooks_prefer_the_venv_binary() {
  let dir = tempfile::tempdir().unwrap();
  let scripts = dir.path().join(".venv").join("Scripts");
  std::fs::create_dir_all(&scripts).unwrap();
  let exe = scripts.join("ruff.exe");
  std::fs::write(&exe, "").unwrap();
  let runner = RecordingRunner::new(0);
  run(Hook::StopRuff, "{}", dir.path(), &runner);
  let calls = runner.calls.borrow();
  assert_eq!(calls.len(), 1);
  assert_eq!(calls[0].program, exe.to_string_lossy());
  assert_eq!(calls[0].args, ["check", "--fix", "--unfixable", "F401", "."]);
}

#[test]
fn stop_hooks_are_silent_on_success_or_empty_output() {
  let ok = RecordingRunner::new(0);
  ok.script("uv", &["run"], 0, "all good\n");
  assert_eq!(run(Hook::StopPyright, "{}", Path::new("."), &ok), None);
  let quiet_failure = RecordingRunner::new(1);
  assert_eq!(run(Hook::StopClean, "{}", Path::new("."), &quiet_failure), None);
}

#[test]
fn stop_hooks_concatenate_stdout_and_stderr() {
  let runner = RecordingRunner::new(1);
  runner.script_err("uv", &["run", "pyright"], 1, "  boom  ");
  let ctx = context(run(Hook::StopPyright, "{}", Path::new("."), &runner)).unwrap();
  assert_eq!(ctx, "pyright (project-wide) reported issues:\nboom");
}

#[test]
fn stop_output_is_truncated_to_4000_chars_on_a_char_boundary() {
  let runner = RecordingRunner::new(1);
  let long = "é".repeat(5000); // 2 bytes each: byte-slicing at 4000 would split a char
  runner.script("uv", &["run", "ruff"], 1, &long);
  let ctx = context(run(Hook::StopRuff, "{}", Path::new("."), &runner)).unwrap();
  let body = ctx.split_once('\n').unwrap().1;
  assert_eq!(body.chars().count(), 4000);
}

/// A runner whose spawn itself fails — the tool is not installed at all.
struct FailingRunner;
impl aeth_devkit_core::process::Runner for FailingRunner {
  fn run_inherit(&self, _: &str, _: &[String], _: &Path) -> anyhow::Result<Option<i32>> {
    anyhow::bail!("program not found")
  }
  fn run_capture(&self, _: &str, _: &[String], _: &Path) -> anyhow::Result<aeth_devkit_core::process::CapturedOutput> {
    anyhow::bail!("program not found")
  }
}

#[test]
fn stop_hooks_are_silent_when_the_tool_cannot_be_spawned() {
  assert_eq!(run(Hook::StopRuff, "{}", Path::new("."), &FailingRunner), None);
}
