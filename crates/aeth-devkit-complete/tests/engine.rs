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
  let req = Request { prefix: "b".to_string(), ..base(&project) };
  assert_eq!(values(run(&req)), ["build", "bench"]);
}

#[test]
fn an_unreadable_project_yields_no_completions_rather_than_an_error() {
  // A directory with no pyproject at all. A completer that errored here would print over
  // the user's prompt, so the contract is "empty, quietly".
  let dir = tempfile::tempdir().unwrap();
  let req = Request { root: root_of(&dir).to_path_buf(), ..base(&fixture_project(&[])) };
  assert_eq!(values(run(&req)), Vec::<String>::new());
}
