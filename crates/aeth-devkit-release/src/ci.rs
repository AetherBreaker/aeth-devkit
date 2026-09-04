//! Step 8: waiting for the release workflow the GitHub release triggered, and proving that
//! what it published is installable.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::{git, github};

use crate::Deps;
use crate::config::Config;
use crate::preflight::target_has_version;

/// The workflow setup-project installs; the release requires it at `HEAD`.
pub const WORKFLOW_FILE: &str = ".github/workflows/release.yml";
/// How long the release event may take to produce a run before we give up. GitHub usually
/// starts one within seconds; two minutes covers a queue backlog without hiding a workflow
/// that is never going to start (file missing on the default branch, Actions disabled).
pub const RUN_START_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a green run's upload may take to show on the index before the release is
/// declared failed. PyPI's CDN and devpi both lag a little behind an upload, and a single
/// request can hit a transient 5xx; neither is a reason to roll a published release back.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);
/// How long a cancelled run may take to report itself completed after its watcher died.
/// GitHub honours a cancel within seconds; past this the run's state is unknown and the
/// error says so.
pub const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Spacing between polls, for the run start, the index and a settling run.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// `&["a", "b"]` → `vec!["a".to_string(), "b".to_string()]`, for the `Runner` API.
fn s(args: &[&str]) -> Vec<String> {
  args.iter().map(|a| a.to_string()).collect()
}

/// One release workflow run, as `gh run list` reports it. `status` is GitHub's
/// (`queued`, `in_progress`, `completed`, …); only `completed` is terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
  pub id: String,
  pub status: String,
}

/// Every release workflow run for `tag`, newest first. For a `release` event `gh` reports
/// the tag as the run's head branch, which is what selects them. Runs of an earlier,
/// since-deleted release of the same tag are included — deleting a release does not delete
/// its runs — which is why callers snapshot this list before creating the release and wait
/// for an id that was not in it.
pub fn list_runs(runner: &dyn Runner, root: &Path, tag: &str) -> Result<Vec<Run>> {
  let jq = format!(".[] | select(.headBranch == \"{tag}\") | \"\\(.databaseId) \\(.status)\"");
  let args = s(&[
    "run",
    "list",
    "--workflow",
    "release.yml",
    "--event",
    "release",
    "--json",
    "databaseId,headBranch,status",
    "--jq",
    &jq,
  ]);
  let out = runner.run_capture("gh", &args, root)?;
  if !out.success() {
    bail!("gh run list failed: {}", out.stderr.trim());
  }
  out
    .stdout
    .lines()
    .map(str::trim)
    .filter(|l| !l.is_empty())
    .map(|l| {
      let (id, status) = l.split_once(' ').ok_or_else(|| anyhow!("unexpected `gh run list` line: {l:?}"))?;
      Ok(Run {
        id: id.into(),
        status: status.into(),
      })
    })
    .collect()
}

/// Pre-flight: a run of an earlier release of the same tag that is still queued or in
/// progress would attach to and publish against the release about to be created, before
/// the new run even starts; deleting that earlier release does not stop its run. Refuse
/// until every such run has finished.
pub fn check_no_active_run(runner: &dyn Runner, root: &Path, tag: &str) -> Result<()> {
  let active: Vec<String> = list_runs(runner, root, tag)?
    .into_iter()
    .filter(|r| r.status != "completed")
    .map(|r| r.id)
    .collect();
  if !active.is_empty() {
    bail!(
      "release workflow run(s) {} for {tag} are still active (from an earlier release of this tag); wait for them or `gh run cancel <id>` first",
      active.join(", ")
    );
  }
  Ok(())
}

/// The error [`wait_for_run`] fails with when run `id` was cancelled but never reported
/// itself completed: its state is unknown, so it may still be about to publish, and the
/// caller must *not* unwind the journal (that would delete the release and tag under a
/// run that is still going, and cannot un-publish what it uploads). Everything is left in
/// place for the user to settle by hand.
#[derive(Debug)]
pub struct Unsettled {
  pub id: String,
}

impl std::fmt::Display for Unsettled {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "release workflow run {} was cancelled but has not stopped within {}s",
      self.id,
      SETTLE_TIMEOUT.as_secs()
    )
  }
}

impl std::error::Error for Unsettled {}

/// `(status, conclusion)` of a run; the conclusion is empty until it completes.
fn run_state(runner: &dyn Runner, root: &Path, id: &str) -> Result<(String, String)> {
  let args = s(&[
    "run",
    "view",
    id,
    "--json",
    "status,conclusion",
    "--jq",
    r#".status + " " + (.conclusion // "")"#,
  ]);
  let out = runner.run_capture("gh", &args, root)?;
  if !out.success() {
    bail!("gh run view {id} failed: {}", out.stderr.trim());
  }
  let line = out.stdout.trim();
  let (status, conclusion) = line.split_once(' ').unwrap_or((line, ""));
  Ok((status.into(), conclusion.into()))
}

/// The watcher of run `id` died (`why`). That is not proof the run stopped: Ctrl-C and
/// API/network failures end the watch while the run carries on — into `gh release upload`
/// and `uv publish`, which the rollback that follows cannot undo. So: read the run's state;
/// a run still going is cancelled and polled until GitHub reports it completed, bounded by
/// [`SETTLE_TIMEOUT`]. `Ok` when the run turns out to have succeeded — on its own, or
/// finishing before the cancel landed: either way it published, so the release stands.
/// A run that never settles is [`Unsettled`], which the caller must not unwind. The
/// interrupt flag is deliberately not consulted: after Ctrl-C it is already set, and
/// stopping the run comes before honouring it.
fn settle(deps: &Deps, root: &Path, id: &str, why: &str) -> Result<()> {
  let mut waited = Duration::ZERO;
  let mut cancelled = false;
  loop {
    if let Ok((status, conclusion)) = run_state(deps.runner, root, id)
      && status == "completed"
    {
      if conclusion == "success" {
        println!("  run {id} succeeded although its watcher did not ({why}); continuing");
        return Ok(());
      }
      bail!("release workflow run {id} failed: {conclusion} ({why})");
    }
    if !cancelled {
      // Failure is fine: the run may have completed since, which the next poll sees.
      println!("  {why}; cancelling run {id} so nothing is published after the rollback...");
      let _ = deps.runner.run_capture("gh", &s(&["run", "cancel", id]), root);
      cancelled = true;
    }
    if waited >= SETTLE_TIMEOUT {
      return Err(Unsettled { id: id.into() }.into());
    }
    (deps.sleep)(POLL_INTERVAL);
    waited += POLL_INTERVAL;
  }
}

/// Poll until a run not in `known` (the ids [`list_runs`] returned before the release was
/// created) exists, bounded by [`RUN_START_TIMEOUT`], then `gh run watch` it to completion
/// with inherited stdio so the user sees the job progress. Returns the run URL.
pub fn wait_for_run(deps: &Deps, root: &Path, tag: &str, known: &[String]) -> Result<String> {
  let mut waited = Duration::ZERO;
  let id = loop {
    deps.check_interrupt()?;
    if let Some(run) = list_runs(deps.runner, root, tag)?.into_iter().find(|r| !known.contains(&r.id)) {
      break run.id;
    }
    if waited >= RUN_START_TIMEOUT {
      bail!(
        "no release workflow run for {tag} started within {}s; is {WORKFLOW_FILE} on the default branch and Actions enabled?",
        RUN_START_TIMEOUT.as_secs()
      );
    }
    (deps.sleep)(POLL_INTERVAL);
    waited += POLL_INTERVAL;
  };
  println!("  run {id} started; watching...");
  match deps.runner.run_inherit("gh", &s(&["run", "watch", &id, "--exit-status"]), root)? {
    Some(0) => {}
    Some(code) => settle(deps, root, &id, &format!("gh run watch exited with {code}"))?,
    None => settle(deps, root, &id, "gh run watch was terminated by a signal")?,
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
///
/// The target is polled for up to [`VERIFY_TIMEOUT`] through the same authenticated,
/// target-specific check as the pre-flight probe: a request error or an absent version
/// right after the upload is propagation, not failure, and the rollback this gates is
/// destructive (for PyPI it cannot even undo the upload).
pub fn verify_published(deps: &Deps, root: &Path, cfg: &Config, version: &str) -> Result<()> {
  let mut waited = Duration::ZERO;
  loop {
    deps.check_interrupt()?;
    let last_error = match target_has_version(deps, cfg, version) {
      Ok(true) => break,
      Ok(false) => None,
      Err(e) => Some(e),
    };
    if waited >= VERIFY_TIMEOUT {
      let why = match last_error {
        Some(e) => format!("the last query failed: {e:#}"),
        None => "the index does not list it".into(),
      };
      bail!(
        "the release workflow succeeded but {}=={version} is not on {} after {}s ({why}); inspect the publish job's log",
        cfg.package,
        cfg.target.label(),
        VERIFY_TIMEOUT.as_secs()
      );
    }
    (deps.sleep)(POLL_INTERVAL);
    waited += POLL_INTERVAL;
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
  use std::cell::{Cell, RefCell};
  use std::sync::atomic::AtomicBool;

  use aeth_devkit_core::devpi::StubDevpiClient;
  use aeth_devkit_core::index::{IndexClient, StubIndexClient};
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

  /// An index that errors for the first `failures` queries and lists nothing for the next
  /// `empty` ones before finally listing `versions` — propagation, as a release sees it.
  struct SlowIndex {
    failures: usize,
    empty: usize,
    versions: Vec<String>,
    queries: Cell<usize>,
  }

  impl IndexClient for SlowIndex {
    fn versions(&self, _simple_url: &str, _package: &str) -> Result<Vec<String>> {
      let n = self.queries.get();
      self.queries.set(n + 1);
      if n < self.failures {
        bail!("HTTP 502");
      }
      if n < self.failures.saturating_add(self.empty) {
        return Ok(Vec::new());
      }
      Ok(self.versions.clone())
    }
  }

  fn scripted() -> RecordingRunner {
    let r = RecordingRunner::new(0);
    r.script("gh", &["run", "list"], 0, "123456 completed\n");
    r.script("gh", &["run", "view"], 0, "https://github.com/o/r/actions/runs/123456\n");
    r.script(
      "gh",
      &["run", "view", "123456", "--json", "status,conclusion"],
      0,
      "completed failure\n",
    );
    r.script("gh", &["release", "view"], 0, "url\n");
    r
  }

  /// Reports the run in progress for the first `live` state queries, then completed with
  /// `conclusion`; everything else like `inner`.
  struct Settling {
    inner: RecordingRunner,
    live: usize,
    conclusion: &'static str,
    queries: Cell<usize>,
  }

  impl Runner for Settling {
    fn run_inherit_env(&self, program: &str, args: &[String], cwd: &Path, env: &[(&str, &str)]) -> Result<Option<i32>> {
      self.inner.run_inherit_env(program, args, cwd, env)
    }
    fn run_capture(&self, program: &str, args: &[String], cwd: &Path) -> Result<CapturedOutput> {
      if program == "gh" && args.starts_with(&s(&["run", "view", "123456", "--json", "status,conclusion"])) {
        let n = self.queries.get();
        self.queries.set(n + 1);
        self.inner.run_capture(program, args, cwd)?; // recorded
        let stdout = if n < self.live {
          "in_progress \n".to_string()
        } else {
          format!("completed {}\n", self.conclusion)
        };
        return Ok(CapturedOutput {
          code: Some(0),
          stdout,
          ..Default::default()
        });
      }
      self.inner.run_capture(program, args, cwd)
    }
  }

  fn deps<'a>(runner: &'a dyn Runner, index: &'a dyn IndexClient, flag: &'a AtomicBool, sleep: &'a dyn Fn(Duration)) -> Deps<'a> {
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
      sleep,
    }
  }

  const NO_SLEEP: &dyn Fn(Duration) = &|_| {};

  fn cfg() -> Config {
    Config {
      package: "demo".into(),
      target: PublishTarget::Pypi,
    }
  }

  fn private_cfg() -> Config {
    Config {
      package: "demo".into(),
      target: PublishTarget::Index {
        name: "Private".into(),
        url: "https://x/+simple".into(),
        publish_url: "https://x/user/internal/".into(),
        username: "u".into(),
        password: "p".into(),
      },
    }
  }

  #[test]
  fn list_runs_selects_the_runs_for_the_tag() {
    let r = scripted();
    assert_eq!(
      list_runs(&r, Path::new("."), "v1.2.3").unwrap(),
      vec![Run {
        id: "123456".into(),
        status: "completed".into()
      }]
    );
    let call = &r.calls_for("gh")[0];
    assert_eq!(
      call[..8].to_vec(),
      vec![
        "run",
        "list",
        "--workflow",
        "release.yml",
        "--event",
        "release",
        "--json",
        "databaseId,headBranch,status"
      ]
    );
    assert!(call.iter().any(|a| a.contains(r#"select(.headBranch == "v1.2.3")"#)), "{call:?}");
    let empty = RecordingRunner::new(0);
    assert!(list_runs(&empty, Path::new("."), "v1.2.3").unwrap().is_empty());
    let broken = RecordingRunner::new(1);
    assert!(list_runs(&broken, Path::new("."), "v1.2.3").is_err());
  }

  #[test]
  fn runs_that_existed_before_the_release_are_not_the_new_one() {
    // Newest first, as `gh run list` prints: the re-release's run 777 sits above the
    // earlier release's 123456, which is still there because deleting a release keeps its runs.
    let r = RecordingRunner::new(0);
    r.script("gh", &["run", "list"], 0, "777 completed\n123456 completed\n");
    r.script("gh", &["run", "view"], 0, "https://github.com/o/r/actions/runs/777\n");
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag, NO_SLEEP);
    let known = vec!["123456".to_string()];
    let url = wait_for_run(&d, Path::new("."), "v1.2.3", &known).unwrap();
    assert!(url.ends_with("/777"), "{url}");
    assert!(r.calls_for("gh").iter().any(|c| c == &["run", "watch", "777", "--exit-status"]));
    // Only the old run exists: keep waiting rather than watch it.
    let stale = RecordingRunner::new(0);
    stale.script("gh", &["run", "list"], 0, "123456 completed\n");
    let d = deps(&stale, &index, &flag, NO_SLEEP);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &known).unwrap_err();
    assert!(err.to_string().contains("no release workflow run"), "{err}");
    assert!(stale.calls_for("gh").iter().all(|c| c[1] != "watch"));
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
    let slept = RefCell::new(Vec::new());
    let sleep = |d| slept.borrow_mut().push(d);
    let d = deps(&late, &index, &flag, &sleep);
    let url = wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap();
    assert_eq!(url, "https://github.com/o/r/actions/runs/123456");
    assert_eq!(*slept.borrow(), vec![POLL_INTERVAL, POLL_INTERVAL]);
    let gh = late.inner.calls_for("gh");
    assert!(gh.iter().any(|c| c == &["run", "watch", "123456", "--exit-status"]), "{gh:?}");
  }

  #[test]
  fn a_failed_url_lookup_does_not_fail_a_green_run() {
    let r = scripted();
    r.script_err("gh", &["run", "view"], 1, "HTTP 502");
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag, NO_SLEEP);
    assert_eq!(wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap(), "run 123456");
  }

  #[test]
  fn gives_up_when_no_run_starts_in_time() {
    let r = RecordingRunner::new(0); // `gh run list` always empty
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let total = Cell::new(Duration::ZERO);
    let sleep = |d| total.set(total.get() + d);
    let d = deps(&r, &index, &flag, &sleep);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap_err();
    assert!(err.to_string().contains("no release workflow run for v1.2.3"), "{err}");
    assert!(total.get() >= RUN_START_TIMEOUT, "{:?}", total.get());
    assert!(r.calls_for("gh").iter().all(|c| c[1] != "watch"));
  }

  #[test]
  fn a_failed_run_is_an_error() {
    let r = scripted();
    r.script("gh", &["run", "watch"], 1, "");
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag, NO_SLEEP);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap_err();
    assert!(err.to_string().contains("run 123456 failed"), "{err}");
    // The run had already concluded: nothing to cancel.
    assert!(r.calls_for("gh").iter().all(|c| c[1] != "cancel"));
  }

  #[test]
  fn a_dead_watcher_cancels_a_live_run_and_waits_for_it_to_stop() {
    let r = Settling {
      inner: scripted(),
      live: 3,
      conclusion: "cancelled",
      queries: Cell::new(0),
    };
    r.inner.script("gh", &["run", "watch"], 130, ""); // as after Ctrl-C
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let slept = RefCell::new(Vec::new());
    let sleep = |d| slept.borrow_mut().push(d);
    let d = deps(&r, &index, &flag, &sleep);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap_err();
    assert!(err.to_string().contains("run 123456 failed: cancelled"), "{err}");
    let gh = r.inner.calls_for("gh");
    assert_eq!(gh.iter().filter(|c| c[..] == ["run", "cancel", "123456"]).count(), 1, "{gh:?}");
    // Three live polls, each followed by a wait; the fourth reads "completed".
    assert_eq!(slept.borrow().len(), 3);
    assert_eq!(r.queries.get(), 4);

    // A run that never settles is still an error, with the window spent.
    let stuck = Settling {
      inner: scripted(),
      live: usize::MAX,
      conclusion: "cancelled",
      queries: Cell::new(0),
    };
    stuck.inner.script("gh", &["run", "watch"], 1, "");
    let total = Cell::new(Duration::ZERO);
    let sleep = |d| total.set(total.get() + d);
    let d = deps(&stuck, &index, &flag, &sleep);
    let err = wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap_err();
    assert!(err.downcast_ref::<Unsettled>().is_some_and(|u| u.id == "123456"), "{err}");
    assert!(total.get() >= SETTLE_TIMEOUT);

    // The cancel lost the race with a normal completion: the run published, so it is a
    // success, not a rollback.
    let won = Settling {
      inner: scripted(),
      live: 1,
      conclusion: "success",
      queries: Cell::new(0),
    };
    won.inner.script("gh", &["run", "watch"], 1, "");
    let d = deps(&won, &index, &flag, NO_SLEEP);
    wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap();
    assert!(won.inner.calls_for("gh").iter().any(|c| c[..] == ["run", "cancel", "123456"]));
  }

  #[test]
  fn a_run_that_succeeded_despite_its_watcher_is_a_success() {
    let r = scripted();
    r.script("gh", &["run", "watch"], 1, ""); // API blip after the run finished
    r.script(
      "gh",
      &["run", "view", "123456", "--json", "status,conclusion"],
      0,
      "completed success\n",
    );
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(false);
    let d = deps(&r, &index, &flag, NO_SLEEP);
    assert_eq!(
      wait_for_run(&d, Path::new("."), "v1.2.3", &[]).unwrap(),
      "https://github.com/o/r/actions/runs/123456"
    );
    assert!(r.calls_for("gh").iter().all(|c| c[1] != "cancel"));
  }

  #[test]
  fn an_active_run_of_an_earlier_release_is_refused() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["run", "list"], 0, "777 in_progress\n123456 completed\n");
    let err = check_no_active_run(&r, Path::new("."), "v1.2.3").unwrap_err();
    assert!(err.to_string().contains("777") && !err.to_string().contains("123456"), "{err}");
    let done = scripted();
    check_no_active_run(&done, Path::new("."), "v1.2.3").unwrap();
    let none = RecordingRunner::new(0);
    check_no_active_run(&none, Path::new("."), "v1.2.3").unwrap();
  }

  #[test]
  fn an_interrupt_between_polls_stops_waiting() {
    let r = RecordingRunner::new(0);
    let index = StubIndexClient { versions: vec![] };
    let flag = AtomicBool::new(true);
    let d = deps(&r, &index, &flag, NO_SLEEP);
    assert!(wait_for_run(&d, Path::new("."), "v1", &[]).is_err());
    assert!(r.calls_for("gh").is_empty());
  }

  #[test]
  fn verify_waits_out_propagation_before_passing() {
    let r = scripted();
    let flag = AtomicBool::new(false);
    // Two transient errors, then two empty pages, then the version: a pass, after four
    // polls' worth of sleeping.
    let index = SlowIndex {
      failures: 2,
      empty: 2,
      versions: vec!["1.0.0".into()],
      queries: Cell::new(0),
    };
    let slept = RefCell::new(Vec::new());
    let sleep = |d| slept.borrow_mut().push(d);
    let d = deps(&r, &index, &flag, &sleep);
    verify_published(&d, Path::new("."), &cfg(), "1.0.0").unwrap();
    assert_eq!(slept.borrow().len(), 4);
    assert_eq!(index.queries.get(), 5);
  }

  #[test]
  fn verify_fails_once_the_window_is_spent() {
    let r = scripted();
    let flag = AtomicBool::new(false);
    let absent = StubIndexClient { versions: vec![] };
    let total = Cell::new(Duration::ZERO);
    let sleep = |d| total.set(total.get() + d);
    let d = deps(&r, &absent, &flag, &sleep);
    let err = verify_published(&d, Path::new("."), &cfg(), "1.0.0").unwrap_err();
    assert!(err.to_string().contains("demo==1.0.0 is not on PyPI"), "{err}");
    assert!(err.to_string().contains("does not list it"), "{err}");
    assert!(total.get() >= VERIFY_TIMEOUT);
    // A persistent request failure is reported as such, not as "not listed".
    let broken = SlowIndex {
      failures: usize::MAX,
      empty: 0,
      versions: vec![],
      queries: Cell::new(0),
    };
    let d = deps(&r, &broken, &flag, NO_SLEEP);
    let err = verify_published(&d, Path::new("."), &cfg(), "1.0.0").unwrap_err();
    assert!(err.to_string().contains("HTTP 502"), "{err}");
  }

  #[test]
  fn verify_asks_a_private_index_through_its_authenticated_endpoint() {
    let r = scripted();
    let flag = AtomicBool::new(false);
    // The simple index is never consulted: a private one needs the login the devpi
    // endpoint gets, and this stub would fail the check if it were queried.
    let broken = SlowIndex {
      failures: usize::MAX,
      empty: 0,
      versions: vec![],
      queries: Cell::new(0),
    };
    let devpi = StubDevpiClient::new(true);
    let mut d = deps(&r, &broken, &flag, NO_SLEEP);
    d.devpi = &devpi;
    verify_published(&d, Path::new("."), &private_cfg(), "1.0.0").unwrap();
    assert_eq!(broken.queries.get(), 0);
    assert_eq!(devpi.calls.borrow().as_slice(), ["GET https://x/user/internal/demo/1.0.0"]);

    let absent = StubDevpiClient::new(false);
    let mut d = deps(&r, &broken, &flag, NO_SLEEP);
    d.devpi = &absent;
    let err = verify_published(&d, Path::new("."), &private_cfg(), "1.0.0").unwrap_err();
    assert!(err.to_string().contains("demo==1.0.0 is not on Private"), "{err}");
  }

  #[test]
  fn verify_requires_the_release_to_still_exist() {
    let present = StubIndexClient {
      versions: vec!["1.0.0".into()],
    };
    let flag = AtomicBool::new(false);
    let no_release = RecordingRunner::new(0);
    no_release.script_err("gh", &["release", "view"], 1, "release not found");
    let d = deps(&no_release, &present, &flag, NO_SLEEP);
    let err = verify_published(&d, Path::new("."), &cfg(), "1.0.0").unwrap_err();
    assert!(err.to_string().contains("no GitHub release"), "{err}");
  }
}
