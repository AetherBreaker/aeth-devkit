//! The live view of the release workflow run: what `gh run watch` shows, laid out in
//! columns so it fits on screen.
//!
//! `gh run watch` prints one line per step under one line per job, which for a matrix
//! release runs well past a terminal's height — and it draws that in the alternate screen
//! buffer, which has no scrollback, so the overflow is simply unreachable. Here the jobs
//! sit side by side instead, each column holding its own steps, so the frame is as tall as
//! the *longest* job rather than the sum of all of them. That fits, which is what lets
//! [`crate::repaint`] draw each refresh over the last one in the normal buffer.
//!
//! The data comes from `gh run view --json`, not from parsing `gh run watch`'s output: the
//! same information, in a shape that does not change with `gh`'s formatting.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use serde::Deserialize;

use crate::Deps;
use crate::ci::{POLL_INTERVAL, SETTLE_TIMEOUT};
use crate::repaint::{self, Repaint, Size};

/// The narrowest column worth drawing: a two-space indent, a glyph, and enough of a step
/// name to tell steps apart. Below this the view falls back to a single column.
const MIN_COL: usize = 24;
/// Blank columns between one job and the next, so two names never touch.
const GUTTER: usize = 2;
/// Width assumed when stdout is not a terminal (a redirected release log).
const OFFSCREEN_COLS: usize = 100;

/// A run as `gh run view --json status,conclusion,jobs` reports it. `gh` renders JSON
/// nulls as empty strings, so `conclusion` is `""` until the run or job completes.
#[derive(Debug, Deserialize)]
pub struct RunView {
  pub status: String,
  pub conclusion: String,
  pub jobs: Vec<Job>,
}

#[derive(Debug, Deserialize)]
pub struct Job {
  pub name: String,
  pub status: String,
  pub conclusion: String,
  pub steps: Vec<Step>,
}

#[derive(Debug, Deserialize)]
pub struct Step {
  pub name: String,
  pub status: String,
  pub conclusion: String,
}

/// `&["a", "b"]` -> `vec!["a".to_string(), "b".to_string()]`, for the `Runner` API.
fn s(args: &[&str]) -> Vec<String> {
  args.iter().map(|a| a.to_string()).collect()
}

/// Read the run's jobs and their steps.
pub fn view(deps: &Deps, root: &Path, id: &str) -> Result<RunView> {
  let out = deps
    .runner
    .run_capture("gh", &s(&["run", "view", id, "--json", "status,conclusion,jobs"]), root)?;
  if !out.success() {
    bail!("gh run view {id} failed: {}", out.stderr.trim());
  }
  serde_json::from_str(&out.stdout).with_context(|| format!("parsing the jobs of run {id}"))
}

/// The glyph for a `(status, conclusion)` pair. Only `completed` carries a conclusion; every
/// other status is a state of its own.
fn glyph(status: &str, conclusion: &str) -> char {
  match (status, conclusion) {
    ("completed", "success") => '✓',
    ("completed", "skipped") => '·',
    // cancelled, timed_out, action_required and whatever GitHub adds later: not a success,
    // and the caller reports the run's own conclusion, so one glyph covers them all.
    ("completed", _) => '✗',
    ("in_progress", _) => '●',
    // queued, waiting, pending, requested.
    _ => '○',
  }
}

/// `text` cut to `width` columns and padded back out to it, so cells line up. Counted in
/// terminal columns, not characters: an emoji in a step name is two columns wide, and
/// padding it as one shifts every column to its right.
fn cell(text: &str, width: usize) -> String {
  let cut = repaint::truncate(text, width.saturating_sub(GUTTER));
  let pad = width.saturating_sub(repaint::columns(cut));
  format!("{cut}{:pad$}", "")
}

/// One frame of the view: a heading, then the jobs in bands of side-by-side columns.
///
/// `cols` is the terminal width. A wider terminal gets more columns, never narrower than
/// [`MIN_COL`]; a run with more jobs than fit across is drawn as several bands, which is
/// still far shorter than a line per step.
pub fn frame(id: &str, run: &RunView, cols: usize) -> Vec<String> {
  let done = run.jobs.iter().filter(|j| j.status == "completed").count();
  let state = if run.conclusion.is_empty() {
    run.status.replace('_', " ")
  } else {
    run.conclusion.clone()
  };
  let mut out = vec![format!("  run {id} · {state} · {done}/{} jobs", run.jobs.len())];
  if run.jobs.is_empty() {
    out.push("  no jobs yet".into());
    return out;
  }
  let per_band = (cols / MIN_COL).clamp(1, run.jobs.len());
  let width = (cols / per_band).max(MIN_COL);
  for band in run.jobs.chunks(per_band) {
    out.push(String::new());
    let rows = band.iter().map(|j| j.steps.len()).max().unwrap_or(0) + 1;
    for row in 0..rows {
      let mut line = String::new();
      for job in band {
        let text = if row == 0 {
          format!("{} {}", glyph(&job.status, &job.conclusion), job.name)
        } else {
          match job.steps.get(row - 1) {
            Some(step) => format!("  {} {}", glyph(&step.status, &step.conclusion), step.name),
            None => String::new(),
          }
        };
        line.push_str(&cell(&text, width));
      }
      out.push(line.trim_end().to_string());
    }
  }
  out
}

/// The jobs that did not succeed, named in full: the frame's columns are narrow enough to
/// have truncated the step name that actually failed.
pub fn failures(run: &RunView) -> Vec<String> {
  let bad = |status: &str, conclusion: &str| status == "completed" && conclusion != "success" && conclusion != "skipped";
  run
    .jobs
    .iter()
    .filter(|j| bad(&j.status, &j.conclusion))
    .map(|j| match j.steps.iter().find(|s| bad(&s.status, &s.conclusion)) {
      Some(step) => format!("  ✗ {} — {} ({})", j.name, step.name, j.conclusion),
      None => format!("  ✗ {} ({})", j.name, j.conclusion),
    })
    .collect()
}

/// Poll the run until GitHub reports it completed, repainting the view each time. Returns
/// the run's conclusion (`success`, `failure`, `cancelled`, …).
///
/// A read that fails is not fatal on its own — an API blip must not cancel a live release —
/// but reads that keep failing for [`SETTLE_TIMEOUT`] mean the run is no longer being
/// watched, which is the caller's cue to stop it. Ctrl-C returns immediately for the same
/// reason: the run has to be dealt with, not abandoned.
pub fn watch(deps: &Deps, root: &Path, id: &str, out: &mut dyn Write, size: Size) -> Result<String> {
  let mut painter = Repaint::new(size);
  let mut blind = Duration::ZERO;
  loop {
    deps.check_interrupt()?;
    match view(deps, root, id) {
      Ok(run) => {
        blind = Duration::ZERO;
        let cols = size().map_or(OFFSCREEN_COLS, |(cols, _)| cols);
        for (i, line) in frame(id, &run, cols).iter().enumerate() {
          painter.line(out, line, i == 0);
        }
        if run.status == "completed" {
          for line in failures(&run) {
            let _ = writeln!(out, "{line}");
          }
          return Ok(run.conclusion);
        }
      }
      Err(e) if blind >= SETTLE_TIMEOUT => return Err(e),
      Err(_) => blind += POLL_INTERVAL,
    }
    (deps.sleep)(POLL_INTERVAL);
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn step(name: &str, status: &str, conclusion: &str) -> Step {
    Step {
      name: name.into(),
      status: status.into(),
      conclusion: conclusion.into(),
    }
  }

  fn job(name: &str, status: &str, conclusion: &str, steps: Vec<Step>) -> Job {
    Job {
      name: name.into(),
      status: status.into(),
      conclusion: conclusion.into(),
      steps,
    }
  }

  /// Two finished jobs, one running, one not started: every glyph the view can draw.
  fn run() -> RunView {
    RunView {
      status: "in_progress".into(),
      conclusion: String::new(),
      jobs: vec![
        job(
          "Rust (ubuntu-latest)",
          "completed",
          "success",
          vec![
            step("Set up job", "completed", "success"),
            step("Clippy", "completed", "success"),
            step("Publish coverage", "completed", "skipped"),
          ],
        ),
        job(
          "Rust (windows-latest)",
          "in_progress",
          "",
          vec![step("Set up job", "completed", "success"), step("Clippy", "in_progress", "")],
        ),
        job(
          "Wheel build",
          "completed",
          "failure",
          vec![step("Build wheel", "completed", "failure")],
        ),
        job("Publish", "queued", "", vec![]),
      ],
    }
  }

  #[test]
  fn jobs_are_drawn_side_by_side_with_their_own_steps() {
    let out = frame("77", &run(), 100);
    assert_eq!(out[0], "  run 77 · in progress · 2/4 jobs");
    assert_eq!(out[1], "");
    assert_eq!(
      out[2],
      "✓ Rust (ubuntu-latest)   ● Rust (windows-latest)  ✗ Wheel build            ○ Publish"
    );
    assert_eq!(out[3], "  ✓ Set up job             ✓ Set up job             ✗ Build wheel");
    assert_eq!(out[4], "  ✓ Clippy                 ● Clippy");
    // The tallest job sets the height; shorter columns simply end.
    assert_eq!(out[5], "  · Publish coverage");
    assert_eq!(out.len(), 6);
  }

  #[test]
  fn a_narrow_terminal_falls_back_to_one_column_per_band() {
    let out = frame("77", &run(), 30);
    // One job per band, each band separated by a blank line: still four rows of steps at
    // most, never the sum of every job's steps.
    assert_eq!(out[2], "✓ Rust (ubuntu-latest)");
    assert_eq!(out[6], "");
    assert_eq!(out[7], "● Rust (windows-latest)");
  }

  #[test]
  fn a_run_with_no_jobs_yet_says_so() {
    let empty = RunView {
      status: "queued".into(),
      conclusion: String::new(),
      jobs: vec![],
    };
    assert_eq!(frame("77", &empty, 100), ["  run 77 · queued · 0/0 jobs", "  no jobs yet"]);
  }

  #[test]
  fn the_heading_reports_the_conclusion_once_there_is_one() {
    let mut done = run();
    done.status = "completed".into();
    done.conclusion = "failure".into();
    assert_eq!(frame("77", &done, 100)[0], "  run 77 · failure · 2/4 jobs");
  }

  #[test]
  fn failures_name_the_step_the_columns_had_to_truncate() {
    assert_eq!(failures(&run()), ["  ✗ Wheel build — Build wheel (failure)"]);
    // A job that failed before any step ran still gets named.
    let mut none = run();
    none.jobs[2].steps.clear();
    assert_eq!(failures(&none), ["  ✗ Wheel build (failure)"]);
  }

  #[test]
  fn a_skipped_job_is_not_a_failure() {
    let skipped = RunView {
      status: "completed".into(),
      conclusion: "success".into(),
      jobs: vec![job("Publish", "completed", "skipped", vec![])],
    };
    assert!(failures(&skipped).is_empty());
  }
}
