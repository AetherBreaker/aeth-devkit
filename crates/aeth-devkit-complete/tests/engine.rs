//! Engine tests: a completion request in, a directive out, with no shell anywhere.
//!
//! This is the payoff of keeping [`aeth_devkit_complete::engine::complete`] pure — every
//! behaviour the two shell scripts used to own can be asserted as a plain function call.

use std::path::Path;

use aeth_devkit_complete::engine::{Directive, Item, ItemKind, Request, Shell, complete};
use aeth_devkit_core::process::RecordingRunner;

// ---- fixtures ----------------------------------------------------------------------------

/// Write a pyproject declaring `tasks` as trivial commands, in the order given.
fn fixture_project(tasks: &[&str]) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  // `[tool.poe.tasks]` is the table `resolve` reads; a bare string is the simplest task form.
  let mut toml = String::from("[tool.poe.tasks]\n");
  for t in tasks {
    toml.push_str(&format!("{t} = \"echo {t}\"\n"));
  }
  std::fs::write(dir.path().join("pyproject.toml"), toml).unwrap();
  dir
}

/// A request completing a fresh word straight after `poe`, which callers then adjust with
/// struct update syntax (`Request { prefix: "b".into(), ..base(&dir) }`).
fn base(dir: &tempfile::TempDir) -> Request {
  Request {
    shell: Shell::Bash,
    words: vec!["poe".to_string()],
    // One past the last word: the user has typed a space and is starting a new word.
    cword: 1,
    prefix: String::new(),
    root: dir.path().to_path_buf(),
  }
}

/// Run the engine against a fixture. `RecordingRunner` stands in for the process runner so
/// nothing is actually spawned; `true` bypasses the on-disk cache so tests never share state.
fn run(req: &Request) -> Directive {
  complete(req, &RecordingRunner::new(0), true)
}

/// Unwrap to the item list, failing loudly if the engine asked for file completion instead.
fn items(d: Directive) -> Vec<Item> {
  match d {
    Directive::Items(items) => items,
    other => panic!("expected items, got {other:?}"),
  }
}

/// The inserted values, which is what most assertions care about.
fn values(d: Directive) -> Vec<String> {
  items(d).into_iter().map(|i| i.value).collect()
}

fn root_of(dir: &tempfile::TempDir) -> &Path {
  dir.path()
}

// ---- task names --------------------------------------------------------------------------

#[test]
fn completes_task_names_after_the_command() {
  let project = fixture_project(&["build", "test"]);
  let req = base(&project);
  let got = items(run(&req));
  assert_eq!(got.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["build", "test"]);
  // Typed as a command so PowerShell renders it with the command icon rather than as a value.
  assert!(matches!(got[0].kind, ItemKind::Command));
}

#[test]
fn filters_task_names_by_prefix() {
  let project = fixture_project(&["build", "bench", "test"]);
  let req = Request {
    prefix: "b".to_string(),
    ..base(&project)
  };
  assert_eq!(values(run(&req)), ["build", "bench"]);
}

#[test]
fn an_unreadable_project_yields_no_completions_rather_than_an_error() {
  // A directory with no pyproject at all. A completer that errored here would print over
  // the user's prompt, so the contract is "empty, quietly".
  let dir = tempfile::tempdir().unwrap();
  let req = Request {
    root: root_of(&dir).to_path_buf(),
    ..base(&fixture_project(&[]))
  };
  assert_eq!(values(run(&req)), Vec::<String>::new());
}

/// Split a display line into owned words, mirroring what the shims forward.
fn w(line: &str) -> Vec<String> {
  line.split(' ').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

// ---- global options ----------------------------------------------------------------------

#[test]
fn completes_global_options_before_the_task() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe -"),
    cword: 1,
    prefix: "-".to_string(),
    ..base(&p)
  };
  let got = items(run(&req));
  assert!(got.iter().any(|i| i.value == "--verbose"), "{:?}", values_of(&got));
  assert!(matches!(got[0].kind, ItemKind::Param));
}

#[test]
fn omits_globals_excluded_by_one_already_typed() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe --verbose -"),
    cword: 2,
    prefix: "-".to_string(),
    ..base(&p)
  };
  let got = values(run(&req));
  assert!(!got.iter().any(|v| v == "--quiet" || v == "-q"), "{got:?}");
  // Unrelated globals are still offered: only the excluded pair disappears.
  assert!(got.iter().any(|v| v == "--dry-run"), "{got:?}");
}

#[test]
fn does_not_offer_globals_once_the_task_is_typed() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe build -"),
    cword: 2,
    prefix: "-".to_string(),
    ..base(&p)
  };
  // Past the task, `-` means a task option, never a poe global.
  assert!(!values(run(&req)).iter().any(|v| v == "--verbose"));
}

// ---- directory and executor values -------------------------------------------------------

#[test]
fn requests_directory_completion_after_dash_c() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe -C"),
    cword: 2,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(run(&req), Directive::Dirs);
}

#[test]
fn requests_directory_completion_for_inline_dash_c() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe -C=sr"),
    cword: 1,
    prefix: "-C=sr".to_string(),
    ..base(&p)
  };
  assert_eq!(run(&req), Directive::Dirs);
}

#[test]
fn requests_directory_completion_for_the_long_spelling() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe --directory"),
    cword: 2,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(run(&req), Directive::Dirs);
}

#[test]
fn completes_executor_choices() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe -e"),
    cword: 2,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(values(run(&req)), ["auto", "poetry", "simple", "uv", "virtualenv"]);
}

#[test]
fn completes_inline_executor_choices_with_the_whole_word() {
  let p = fixture_project(&["build"]);
  let req = Request {
    words: w("poe --executor=u"),
    cword: 1,
    prefix: "--executor=u".to_string(),
    ..base(&p)
  };
  let got = items(run(&req));
  // The inserted text replaces the entire word; the popup shows only the value.
  assert_eq!(got[0].value, "--executor=uv");
  assert_eq!(got[0].display, "uv");
  assert!(matches!(got[0].kind, ItemKind::Value));
}

// ---- pass-through ------------------------------------------------------------------------

#[test]
fn offers_file_completion_after_the_separator() {
  let p = fixture_project(&["run"]);
  let req = Request {
    words: w("poe run --"),
    cword: 3,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(run(&req), Directive::Files);
}

/// Values of already-unwrapped items, for assertion messages.
fn values_of(items: &[Item]) -> Vec<&str> {
  items.iter().map(|i| i.value.as_str()).collect()
}

// ---- task arguments ----------------------------------------------------------------------

/// A project with one option-bearing task and one positional-bearing task.
fn fixture_args() -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(
    dir.path().join("pyproject.toml"),
    r#"
[tool.poe.tasks.build]
cmd = "echo build"

[[tool.poe.tasks.build.args]]
name = "mode"
options = ["-m", "--mode"]
help = "Build profile"
choices = ["fast", "slow"]

[[tool.poe.tasks.build.args]]
name = "force"
options = ["--force"]
type = "boolean"
help = "Skip checks"

[[tool.poe.tasks.build.args]]
name = "out"
options = ["--out"]
help = "Output path"

[tool.poe.tasks.deploy]
cmd = "echo deploy"

[[tool.poe.tasks.deploy.args]]
name = "target"
positional = true
choices = ["alpha", "beta"]

[[tool.poe.tasks.deploy.args]]
name = "env"
positional = true
choices = ["dev", "prod"]

[[tool.poe.tasks.deploy.args]]
name = "mode"
options = ["--mode"]
choices = ["fast", "slow"]
"#,
  )
  .unwrap();
  dir
}

#[test]
fn completes_a_tasks_own_options_with_help_as_tooltip() {
  let p = fixture_args();
  let req = Request {
    words: w("poe build -"),
    cword: 2,
    prefix: "-".to_string(),
    ..base(&p)
  };
  let got = items(run(&req));
  let mode = got.iter().find(|i| i.value == "--mode").expect("--mode offered");
  assert_eq!(mode.tooltip, "Build profile");
  assert!(matches!(mode.kind, ItemKind::Param));
}

#[test]
fn hides_every_spelling_of_an_already_used_option() {
  // -m and --mode are one argument; using either must hide both.
  let p = fixture_args();
  let req = Request {
    words: w("poe build -m fast -"),
    cword: 4,
    prefix: "-".to_string(),
    ..base(&p)
  };
  let got = values(run(&req));
  assert!(!got.iter().any(|v| v == "--mode" || v == "-m"), "{got:?}");
  assert!(got.iter().any(|v| v == "--force"), "{got:?}");
}

#[test]
fn positionals_are_never_offered_as_option_names() {
  let p = fixture_args();
  let req = Request {
    words: w("poe deploy -"),
    cword: 2,
    prefix: "-".to_string(),
    ..base(&p)
  };
  let got = values(run(&req));
  assert!(!got.iter().any(|v| v == "target" || v == "env"), "{got:?}");
}

#[test]
fn completes_an_options_choices() {
  let p = fixture_args();
  let req = Request {
    words: w("poe build --mode"),
    cword: 3,
    prefix: String::new(),
    ..base(&p)
  };
  let got = items(run(&req));
  assert_eq!(got.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["fast", "slow"]);
  assert!(matches!(got[0].kind, ItemKind::Value));
}

#[test]
fn completes_inline_equals_choices_replacing_the_whole_word() {
  let p = fixture_args();
  let req = Request {
    words: w("poe build --mode=f"),
    cword: 2,
    prefix: "--mode=f".to_string(),
    ..base(&p)
  };
  let got = items(run(&req));
  assert_eq!(got[0].value, "--mode=fast");
  assert_eq!(got[0].display, "fast");
}

#[test]
fn a_boolean_flag_does_not_consume_the_next_word() {
  // After a boolean the next Tab offers options again, not that flag's "value".
  let p = fixture_args();
  let req = Request {
    words: w("poe build --force -"),
    cword: 3,
    prefix: "-".to_string(),
    ..base(&p)
  };
  assert!(values(run(&req)).iter().any(|v| v == "--mode"));
}

#[test]
fn falls_back_to_files_for_a_free_form_value() {
  let p = fixture_args();
  let req = Request {
    words: w("poe build --out"),
    cword: 3,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(run(&req), Directive::Files);
}

#[test]
fn completes_positional_choices_at_the_right_index() {
  let p = fixture_args();
  let req = Request {
    words: w("poe deploy alpha"),
    cword: 3,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(values(run(&req)), ["dev", "prod"]);
}

#[test]
fn completes_the_first_positional_before_any_are_given() {
  let p = fixture_args();
  let req = Request {
    words: w("poe deploy"),
    cword: 2,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(values(run(&req)), ["alpha", "beta"]);
}

#[test]
fn positional_index_skips_options_and_their_values() {
  let p = fixture_args();
  let req = Request {
    words: w("poe deploy --mode fast alpha"),
    cword: 5,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(values(run(&req)), ["dev", "prod"]);
}

#[test]
fn an_unknown_task_offers_nothing_rather_than_erroring() {
  let p = fixture_args();
  let req = Request {
    words: w("poe nosuchtask -"),
    cword: 2,
    prefix: "-".to_string(),
    ..base(&p)
  };
  assert_eq!(values(run(&req)), Vec::<String>::new());
}

#[test]
fn positionals_beyond_the_last_fall_back_to_files() {
  let p = fixture_args();
  let req = Request {
    words: w("poe deploy alpha dev extra"),
    cword: 4,
    prefix: String::new(),
    ..base(&p)
  };
  assert_eq!(run(&req), Directive::Files);
}
