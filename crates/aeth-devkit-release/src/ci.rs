//! Step 8: waiting for the release workflow the GitHub release triggered, and proving that
//! what it published is installable.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::version::{contains, parse_lenient};
use aeth_devkit_core::{git, github};

use crate::Deps;
use crate::config::Config;

/// The workflow setup-project installs; the release requires it at `HEAD`.
pub const WORKFLOW_FILE: &str = ".github/workflows/release.yml";
/// How long the release event may take to produce a run before we give up. GitHub usually
/// starts one within seconds; two minutes covers a queue backlog without hiding a workflow
/// that is never going to start (file missing on the default branch, Actions disabled).
pub const RUN_START_TIMEOUT: Duration = Duration::from_secs(120);
/// Spacing between `gh run list` polls.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// `&["a", "b"]` → `vec!["a".to_string(), "b".to_string()]`, for the `Runner` API.
fn s(args: &[&str]) -> Vec<String> {
  args.iter().map(|a| a.to_string()).collect()
}

/// The id of the release workflow run triggered by `tag`, once it exists. For a `release`
/// event `gh` reports the tag as the run's head branch, which is what selects it.
pub fn find_run(runner: &dyn Runner, root: &Path, tag: &str) -> Result<Option<String>> {
  let jq = format!(".[] | select(.headBranch == \"{tag}\") | .databaseId");
  let args = s(&[
    "run",
    "list",
    "--workflow",
    "release.yml",
    "--event",
    "release",
    "--json",
    "databaseId,headBranch",
    "--jq",
    &jq,
  ]);
  let out = runner.run_capture("gh", &args, root)?;
  if !out.success() {
    bail!("gh run list failed: {}", out.stderr.trim());
  }
  Ok(out.stdout.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string))
}

/// Poll until the run exists (bounded by [`RUN_START_TIMEOUT`]), then `gh run watch` it to
/// completion with inherited stdio so the user sees the job progress. Returns the run URL.
///
/// `sleep` is injected so tests can count the waits instead of serving them.
pub fn wait_for_run(deps: &Deps, root: &Path, tag: &str, sleep: &mut dyn FnMut(Duration)) -> Result<String> {
  let mut waited = Duration::ZERO;
  let id = loop {
    deps.check_interrupt()?;
    if let Some(id) = find_run(deps.runner, root, tag)? {
      break id;
    }
    if waited >= RUN_START_TIMEOUT {
      bail!(
        "no release workflow run for {tag} started within {}s; is {WORKFLOW_FILE} on the default branch and Actions enabled?",
        RUN_START_TIMEOUT.as_secs()
      );
    }
    sleep(POLL_INTERVAL);
    waited += POLL_INTERVAL;
  };
  println!("  run {id} started; watching...");
  match deps.runner.run_inherit("gh", &s(&["run", "watch", &id, "--exit-status"]), root)? {
    Some(0) => {}
    Some(code) => bail!("release workflow run {id} failed (gh run watch exited with {code})"),
    None => bail!("gh run watch was terminated by a signal"),
  }
  // The URL is informational: the run already succeeded, so a failed lookup (auth or API
  // blip) must not turn a good release into a rollback. Fall back to naming the run.
  let out = deps
    .runner
    .run_capture("gh", &s(&["run", "view", &id, "--json", "url", "--jq", ".url"]), root)?;
  let url = out.stdout.trim();
  Ok(if out.success() && !url.is_empty() {
    url.to_string()
  } else {
    format!("run {id}")
  })
}

/// A green run is not the same as an installable release: the version must be listed on
/// the publish target and the GitHub release must still exist. This is the rule
/// `docker-pin` applies before pinning, checked here so `Released` means what it says.
pub fn verify_published(deps: &Deps, root: &Path, cfg: &Config, version: &str) -> Result<()> {
  let want = parse_lenient(version).with_context(|| format!("{version} is not a PEP 440 version"))?;
  let versions = deps
    .index
    .versions(cfg.target.simple_url(), &cfg.package)
    .with_context(|| format!("querying {}", cfg.target.label()))?;
  if !contains(versions.iter().map(String::as_str), &want) {
    bail!(
      "the release workflow succeeded but {}=={version} is not on {}; inspect the publish job's log",
      cfg.package,
      cfg.target.label()
    );
  }
  if !github::release_exists(deps.runner, root, &format!("v{version}"))? {
    bail!("v{version} has no GitHub release any more; it was deleted while the workflow ran");
  }
  Ok(())
}

/// The Actions page for the release workflow, when `origin` is on GitHub. Printed by
/// `--no-wait` in place of the run URL, which may not exist yet.
pub fn actions_url(root: &Path) -> Option<String> {
  let origin = git::origin_url(root).ok().flatten()?;
  let repo = github::github_repo_path(&origin)?;
  Some(format!("https://github.com/{repo}/actions/workflows/release.yml"))
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::cell::Cell;
  use std::sync::atomic::AtomicBool;

  use aeth_devkit_core::devpi::StubDevpiClient;
  use aeth_devkit_core::index::StubIndexClient;
  use aeth_devkit_core::process::{CapturedOutput, RecordingRunner};

  use crate::config::PublishTarget;
  use crate::prompt::ScriptedPrompt;

  /// Answers `gh run list` empty for the first `empty_polls` calls, then like `inner`.
  struct LateRun {
    inner: RecordingRunner,
    empty_polls: usize,
    polls: Cell<usize>,
  }

  impl Runner for LateRun {
    fn run_inherit_env(&self, program: &str, args: &[String], cwd: &Path, env: &[(&str, &str)]) -> Result<Option<i32>> {
      self.inner.run_inherit_env(program, args, cwd, env)
    }
    fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
      if program == "gh" && args.starts_with(&["run".to_string(), "list".to_string()]) {
        let n = self.polls.get();
        self.polls.set(n + 1);
        if n < self.empty_polls {
          self.inner.run_capture(program, args, cwd)?; // recorded, but answered empty
          return Ok(CapturedOutput {
            code: Some(0),
            ..Default::default()
          });
        }
      }
      self.inner.run_capture(program, args, cwd)
    }
  }

  fn scripted() -> RecordingRunner {
    let r = RecordingRunner::new(0);
    r.script("gh", &["run", "list"], 0, "123456\n");
    r.script("gh", &["run", "view"], 0, "https://github.com/o/r/actions/runs/123456\n");
    r
  }

  fn deps<'a>(runner: &'a dyn Runner, index: &'a StubIndexClient, flag: &'a AtomicBool) -> Deps<'a> {
    // Leaked stubs keep the test short; these tests own tiny amounts of memory.
    let devpi: &'static StubDevpiClient = Box::leak(Box::new(StubDevpiClient::new(false)));
    let prompt: &'static ScriptedPrompt = Box::leak(Box::new(ScriptedPrompt::new(&[])));
    Deps {
      runner,
      devpi,
      index,
      prompt,
      env: &|_| None,
      interrupted: flag,
    }
  }

  #[test]
  fn find_run_selects_the_run_for_the_tag() {
    let r = scripted();
    assert_eq!(find_run(&r, Path::new("."), "v1.2.3").unwrap().as_deref(), Some("123456"));
    let call = &r.calls_for("gh")[0];
    assert_eq!(
      call[..6].to_vec(),
      vec!["run", "list", "--workflow", "release.yml", "--event", "release"]
    );
    assert!(call.iter().any(|a| a.contains(r#"select(.headBranch == "v1.2.3")"#)), "{call:?}");
    let empty = RecordingRunner::new(0);
    assert_eq!(find_run(&empty, Path::new("."), "v1.2.3").unwrap(), None);
    let broken = RecordingRunner::new(1);
    assert!(find_run(&broken, Path::new("."), "v1.2.3").is_err());
  }

  #[test]
  fn waits_for_the_run_to_appear_then_watches_it() {
    let late = LateRun {
      inner: scripted(),
      empty_polls: 2,
      polls: Cell::new(0),
    };
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&late, &index, &flag);
    let mut slept = Vec::new();
    let url = wait_for_run(&d, Path::new("."), "v1.2.3", &mut |dur| slept.push(dur)).unwrap();
    assert_eq!(url, "https://github.com/o/r/actions/runs/123456");
    assert_eq!(slept, vec![POLL_INTERVAL, POLL_INTERVAL]);
    let gh = late.inner.calls_for("gh");
    assert!(gh.iter().any(|c| c == &["run", "watch", "123456", "--exit-status"]), "{gh:?}");
  }

  #[test]
  fn a_failed_url_lookup_does_not_fail_a_green_run() {
    let r = scripted();
    r.script_err("gh", &["run", "view"], 1, "HTTP 502");
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag);
    assert_eq!(wait_for_run(&d, Path::new("."), "v1.2.3", &mut |_| {}).unwrap(), "run 123456");
  }

  #[test]
  fn gives_up_when_no_run_starts_in_time() {
    let r = RecordingRunner::new(0); // `gh run list` always empty
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag);
    let mut total = Duration::ZERO;
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &mut |dur| total += dur).unwrap_err();
    assert!(err.to_string().contains("no release workflow run for v1.2.3"), "{err}");
    assert!(total >= RUN_START_TIMEOUT, "{total:?}");
    assert!(r.calls_for("gh").iter().all(|c| c[1] != "watch"));
  }

  #[test]
  fn a_failed_run_is_an_error() {
    let r = scripted();
    r.script("gh", &["run", "watch"], 1, "");
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &mut |_| {}).unwrap_err();
    assert!(err.to_string().contains("run 123456 failed"), "{err}");
  }

  #[test]
  fn an_interrupt_between_polls_stops_waiting() {
    let r = RecordingRunner::new(0);
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(true);
    let d = deps(&r, &index, &flag);
    assert!(wait_for_run(&d, Path::new("."), "v1", &mut |_| {}).is_err());
    assert!(r.calls_for("gh").is_empty());
  }

  #[test]
  fn verify_requires_the_version_on_the_target_index_and_the_release() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["release", "view"], 0, "url\n");
    let flag = AtomicBool::new(false);
    let cfg = Config {
      package: "demo".into(),
      target: PublishTarget::Pypi,
    };
    let present = StubIndexClient {
      versions: vec!["1.0.0".into()],
    };
    assert!(verify_published(&deps(&r, &present, &flag), Path::new("."), &cfg, "1.0.0").is_ok());
    let absent = StubIndexClient { versions: vec![] };
    let err = verify_published(&deps(&r, &absent, &flag), Path::new("."), &cfg, "1.0.0").unwrap_err();
    assert!(err.to_string().contains("demo==1.0.0 is not on PyPI"), "{err}");
    let no_release = RecordingRunner::new(0);
    no_release.script_err("gh", &["release", "view"], 1, "release not found");
    let err = verify_published(&deps(&no_release, &present, &flag), Path::new("."), &cfg, "1.0.0").unwrap_err();
    assert!(err.to_string().contains("no GitHub release"), "{err}");
  }
}
