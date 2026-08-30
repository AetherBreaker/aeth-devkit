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
fn pre_edit_protect_denies_every_protected_name() {
  // Table-driven so that removing an entry from `PROTECTED_NAMES` fails a test. Previously
  // only `.env` was ever asserted, and deleting `uv.lock` from the list broke nothing.
  for path in ["d:/proj/.env", "uv.lock", "sub/dir/uv.lock", "./.env"] {
    assert!(
      decide(Hook::PreEditProtect, json!({"tool_input": {"file_path": path}})).is_some(),
      "{path} must be protected"
    );
  }
}

#[test]
fn pre_edit_protect_is_not_fooled_by_windows_filename_aliases() {
  // On Windows all of these open the *same* file as `.env`: the comparison is
  // case-insensitive, and trailing dots and spaces are stripped by the filesystem. A guard
  // that compares raw bytes is bypassed by a capital letter.
  for path in [".ENV", ".Env", "d:/proj/.env.", ".env ", "UV.LOCK", "uv.Lock"] {
    assert!(
      decide(Hook::PreEditProtect, json!({"tool_input": {"file_path": path}})).is_some(),
      "{path} resolves to a protected file and must be denied"
    );
  }
}

#[test]
fn pre_edit_protect_allows_other_files() {
  for path in ["src/main.py", ".environment", "env", "my.env.example", "uv.lock.bak"] {
    assert_eq!(
      decide(Hook::PreEditProtect, json!({"tool_input": {"file_path": path}})),
      None,
      "{path} must be allowed"
    );
  }
}

#[test]
fn pre_bash_protect_deps_denies_uv_add() {
  let out = decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": "uv add requests"}})).expect("must deny");
  let h = &out["hookSpecificOutput"];
  assert_eq!(h["permissionDecision"], "deny");
  assert!(h["permissionDecisionReason"].as_str().unwrap().contains("uv add requests"));
}

/// The whole intended boundary of the dependency guard, in one place: a change to the
/// matcher shows up here as a diff rather than as a silent hole.
#[test]
fn pre_bash_protect_deps_decision_table() {
  let deny = [
    "uv add requests",
    "uv remove foo",
    "uv lock",
    "cd x && uv lock",
    "if true; then uv remove foo; fi",
    // A multi-line script is the ordinary shape of a Bash tool call.
    "cd x\nuv add requests",
    // uv's own global options sit between the program and its subcommand.
    "uv --directory . add requests",
    "uv --project foo lock",
    // Subshells, command substitution and grouping are all just places a command can live.
    "(uv add requests)",
    "$(uv add requests)",
    "{ uv add requests; }",
    "for f in a; do uv add x; done",
    // An environment prefix does not change which program runs.
    "UV_PROJECT=. uv add requests",
    "env FOO=1 uv add requests",
    "time uv add requests",
    // An explicit shell wrapper runs the string as a command.
    "bash -c \"uv add requests\"",
    "sh -c 'uv remove foo'",
    // An absolute or venv-qualified uv is still uv.
    "./.venv/Scripts/uv.exe add requests",
    "/usr/bin/uv lock",
  ];
  for cmd in deny {
    assert!(
      decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": cmd}})).is_some(),
      "must deny: {cmd}"
    );
  }

  let allow = [
    "uv run pytest",
    "uv sync",
    "uv pip list",
    "echo uv-add",
    // The banned verb appears only inside a quoted string, so no command runs it.
    "git commit -m \"wip; uv add later\"",
    "rg \"then uv add\" docs/",
    "echo 'see the README; uv lock is user-run'",
    // A different program that merely starts with the same letters.
    "uvx ruff check",
    "uvicorn app:main",
    // `add` as an argument to something that is not uv.
    "cargo add serde",
    "npm add left-pad",
  ];
  for cmd in allow {
    assert_eq!(
      decide(Hook::PreBashProtectDeps, json!({"tool_input": {"command": cmd}})),
      None,
      "must allow: {cmd}"
    );
  }
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
  // Scripted to fail, so the not-a-git-repo path is reached the way real git reaches it
  // rather than through an empty-stdout success no git would ever produce.
  runner.script("git", &["-c"], 128, "");
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
  for (subdir, exe_name) in [("Scripts", "ruff.exe"), ("bin", "ruff")] {
    let dir = tempfile::tempdir().unwrap();
    let scripts = dir.path().join(".venv").join(subdir);
    std::fs::create_dir_all(&scripts).unwrap();
    let exe = scripts.join(exe_name);
    std::fs::write(&exe, "").unwrap();
    let runner = RecordingRunner::new(0);
    runner.script("git", &["-c"], 128, "");
    run(Hook::StopRuff, "{}", dir.path(), &runner);
    let calls = runner.calls.borrow();
    let ruff = calls
      .iter()
      .find(|c| c.program == exe.to_string_lossy())
      .unwrap_or_else(|| panic!("ruff via venv {subdir}"));
    assert_eq!(ruff.args, ["check", "--fix", "--unfixable", "F401", "."]);
  }
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

// ---- branch-diff scoping (stop-ruff only) ----------------------------------------------

/// Every git call `scope` makes carries `-c core.quotePath=false` in front of the
/// subcommand, so the scripted argument prefixes have to carry it too. One helper keeps that
/// detail in a single place.
fn git_args(rest: &[&str]) -> Vec<String> {
  let mut v = vec!["-c".to_string(), "core.quotePath=false".to_string()];
  v.extend(rest.iter().map(|s| s.to_string()));
  v
}

/// A scripted git repo *plus* a real directory holding the files it claims changed.
///
/// `scope` stats every path before handing it to ruff, so a fixture that only scripts git
/// output would describe a state the filesystem contradicts. Creating the files makes the
/// fixture honest, and lets a test deliberately name a path in git output and leave it off
/// disk — which is exactly the worktree-deletion case.
struct Repo {
  dir: tempfile::TempDir,
  runner: RecordingRunner,
}

impl Repo {
  fn new(branch: &str, base_ok: bool, committed: &str, worktree: &str, untracked: &str, on_disk: &[&str]) -> Self {
    let dir = tempfile::tempdir().unwrap();
    for f in on_disk {
      let p = dir.path().join(f);
      std::fs::create_dir_all(p.parent().unwrap()).unwrap();
      std::fs::write(&p, "").unwrap();
    }
    let r = RecordingRunner::new(0);
    let script = |rest: &[&str], code: i32, out: &str| {
      let owned = git_args(rest);
      let refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
      r.script("git", &refs, code, out);
    };
    // `--show-toplevel` is how `scope` learns what git's paths are relative to.
    script(&["rev-parse", "--show-toplevel"], 0, &format!("{}\n", dir.path().display()));
    script(&["rev-parse", "--abbrev-ref", "HEAD"], 0, &format!("{branch}\n"));
    if base_ok {
      script(&["merge-base"], 0, "abc123\n");
    } else {
      script(&["merge-base"], 1, "");
    }
    script(&["diff", "--name-only", "--diff-filter=d", "abc123...HEAD"], 0, committed);
    script(&["diff", "--name-only", "--diff-filter=d", "HEAD"], 0, worktree);
    script(&["ls-files", "--others", "--exclude-standard", "--full-name"], 0, untracked);
    Self { dir, runner: r }
  }

  /// The ordinary case: every path git names also exists on disk.
  fn all_present(branch: &str, base_ok: bool, committed: &str, worktree: &str, untracked: &str) -> Self {
    let mut on_disk: Vec<&str> = Vec::new();
    for src in [committed, worktree, untracked] {
      for line in src.lines() {
        let l = line.trim();
        if !l.is_empty() && !on_disk.contains(&l) {
          on_disk.push(l);
        }
      }
    }
    Self::new(branch, base_ok, committed, worktree, untracked, &on_disk)
  }

  fn ruff_scope(&self) -> Vec<String> {
    run(Hook::StopRuff, "{}", self.dir.path(), &self.runner);
    ruff_targets(&self.runner)
  }
}

/// The paths ruff was pointed at: everything after the fixed `check --fix --unfixable F401`
/// flags, regardless of whether the call went through `uv run` or the venv binary.
fn ruff_targets(runner: &RecordingRunner) -> Vec<String> {
  let calls = runner.calls.borrow();
  let c = calls
    .iter()
    .find(|c| c.args.contains(&"--unfixable".to_string()))
    .expect("ruff must run");
  let after = c.args.iter().position(|a| a == "F401").expect("F401 flag") + 1;
  c.args[after..].to_vec()
}

#[test]
fn on_a_branch_ruff_runs_on_the_deduped_diff() {
  let repo = Repo::all_present("feat/x", true, "a.py\nshared.py\n", "shared.py\nb.py\n", "new.py\n");
  let mut files = repo.ruff_scope();
  files.sort();
  assert_eq!(files, ["a.py", "b.py", "new.py", "shared.py"]);
}

#[test]
fn non_python_and_deleted_paths_are_excluded() {
  let repo = Repo::all_present("feat/x", true, "README.md\nsrc/a.py\nstub.pyi\n", "", "notes.txt\n");
  let mut files = repo.ruff_scope();
  files.sort();
  assert_eq!(files, ["src/a.py", "stub.pyi"]);
}

#[test]
fn a_file_deleted_in_the_worktree_is_not_sent_to_ruff() {
  // `--diff-filter=d` cannot catch this: the commit-to-commit diff still lists `gone.py` as
  // Added, because the deletion was never committed. Handing ruff a path that no longer
  // exists earns an `E902 ... (os error 2)` the model is asked to fix on every turn, with no
  // fix available, until the deletion is committed.
  let repo = Repo::new("feat/x", true, "gone.py\nkept.py\n", "", "", &["kept.py"]);
  assert_eq!(repo.ruff_scope(), ["kept.py"]);
}

#[test]
fn a_branch_whose_files_are_all_gone_skips_ruff_rather_than_widening() {
  // Every scoped file has been deleted from the worktree. That is "nothing to check", not
  // "check the whole project" — widening here would lint files the branch never touched.
  let repo = Repo::new("feat/x", true, "gone.py\n", "", "", &[]);
  assert_eq!(run(Hook::StopRuff, "{}", repo.dir.path(), &repo.runner), None);
  assert!(
    !repo
      .runner
      .calls
      .borrow()
      .iter()
      .any(|c| c.args.contains(&"--unfixable".to_string())),
    "ruff must not run at all"
  );
}

#[test]
fn a_non_ascii_path_is_still_scoped() {
  // With `core.quotePath` on — the default — git emits this as a quoted, octal-escaped
  // string that no longer ends in `.py`, and the file vanishes from the scope silently.
  // `scope` turns the setting off, so the raw path arrives and survives the suffix test.
  let repo = Repo::all_present("feat/x", true, "café_utils.py\nascii_utils.py\n", "", "");
  let mut files = repo.ruff_scope();
  files.sort();
  assert_eq!(files, ["ascii_utils.py", "café_utils.py"]);
}

#[test]
fn paths_are_named_relative_to_the_directory_ruff_runs_in() {
  // Claude opened at a subdirectory of the repo. git names `app/mod.py` from the repo root,
  // but ruff runs in `app/` and cannot resolve that, so the scope must say `mod.py`.
  let repo = Repo::new("feat/x", true, "app/mod.py\ntop.py\n", "", "", &["app/mod.py", "top.py"]);
  let app = repo.dir.path().join("app");
  run(Hook::StopRuff, "{}", &app, &repo.runner);
  // `top.py` lives above the directory ruff runs in, so it is dropped rather than mis-named.
  assert_eq!(ruff_targets(&repo.runner), ["mod.py"]);
}

#[test]
fn on_main_or_detached_head_ruff_runs_project_wide() {
  for branch in ["main", "master", "HEAD"] {
    let repo = Repo::all_present(branch, true, "a.py\n", "", "");
    assert_eq!(repo.ruff_scope(), ["."], "on {branch}");
  }
}

#[test]
fn outside_a_git_repo_ruff_runs_project_wide() {
  let runner = RecordingRunner::new(128); // every git call fails
  run(Hook::StopRuff, "{}", Path::new("."), &runner);
  assert_eq!(ruff_targets(&runner), ["."]);
}

#[test]
fn no_merge_base_falls_back_to_project_wide() {
  let repo = Repo::all_present("feat/x", false, "", "", "");
  assert_eq!(repo.ruff_scope(), ["."]);
}

#[test]
fn an_empty_branch_diff_skips_ruff_entirely() {
  let repo = Repo::all_present("feat/x", true, "", "", "");
  assert_eq!(run(Hook::StopRuff, "{}", repo.dir.path(), &repo.runner), None);
  assert!(
    !repo
      .runner
      .calls
      .borrow()
      .iter()
      .any(|c| c.args.contains(&"--unfixable".to_string())),
    "ruff must not run at all: {:?}",
    repo.runner.calls.borrow()
  );
}

#[test]
fn an_oversized_file_list_falls_back_to_project_wide() {
  // Long names rather than a huge count: the cap is on total argument characters, and this
  // fixture has to create every one of them on disk for them to count at all. 600 names of
  // ~61 characters clears MAX_ARG_CHARS (30_000) with room to spare.
  let many: String = (0..600)
    .map(|i| format!("a_very_long_module_name_that_eats_argument_characters_{i:04}.py\n"))
    .collect();
  let repo = Repo::all_present("feat/x", true, &many, "", "");
  // Length is checked first, so a regression prints a count rather than 600 paths.
  let scope = repo.ruff_scope();
  assert_eq!(scope.len(), 1, "expected a project-wide fallback, got {} paths", scope.len());
  assert_eq!(scope, ["."]);
}

#[test]
fn pyright_and_clean_are_never_scoped() {
  let repo = Repo::all_present("feat/x", true, "a.py\n", "", "");
  run(Hook::StopPyright, "{}", repo.dir.path(), &repo.runner);
  run(Hook::StopClean, "{}", repo.dir.path(), &repo.runner);
  let calls = repo.runner.calls.borrow();
  assert!(!calls.iter().any(|c| c.program == "git"), "no git calls: {calls:?}");
  assert_eq!(calls.iter().filter(|c| c.args.contains(&"run".to_string())).count(), 2);
}

#[test]
fn a_continuing_stop_hook_does_not_run_its_checker_again() {
  // `stop_hook_active` means a previous Stop hook already continued this turn. The checkers
  // are deterministic, so running them again reports the same findings and continues the
  // turn again — a loop that ends only when the findings do.
  let repo = Repo::all_present("feat/x", true, "a.py\n", "", "");
  for hook in [Hook::StopRuff, Hook::StopPyright, Hook::StopClean] {
    assert_eq!(run(hook, r#"{"stop_hook_active": true}"#, repo.dir.path(), &repo.runner), None);
  }
  assert!(
    repo.runner.calls.borrow().is_empty(),
    "nothing may be spawned: {:?}",
    repo.runner.calls.borrow()
  );
}

#[test]
fn stop_hook_active_does_not_silence_the_pre_hooks() {
  // The flag is about continuing a turn; a PreToolUse guard decides one tool call and must
  // keep denying regardless.
  let runner = RecordingRunner::new(0);
  let payload = r#"{"stop_hook_active": true, "tool_input": {"file_path": "d:/p/.env"}}"#;
  assert!(run(Hook::PreEditProtect, payload, Path::new("."), &runner).is_some());
}
