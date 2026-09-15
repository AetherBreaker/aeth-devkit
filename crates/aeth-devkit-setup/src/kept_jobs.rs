//! The kept release jobs (hub design 3.7): every job named in
//! `[tool.devkit].release-workflow-jobs` is copied out of the project's `release.yml` into
//! the rendered one, under `jobs` after the template's own, so the devkit-owned file can
//! carry a job the project wrote.

use anyhow::{Result, bail};

use aeth_devkit_core::compose::tree::{self, Edit};

#[derive(Debug)]
pub struct Kept {
  pub text: String,
  /// One change-log detail per job copied.
  pub details: Vec<String>,
  /// One advisory per named job the existing file does not hold.
  pub notes: Vec<String>,
}

/// `rendered` with the named jobs of `existing` spliced under its `jobs`, in the order named.
/// Both are read as line trees (the compose engine's): a job is its `<name>:` line, the
/// comment lines directly above it at the same indent, and every deeper line up to the next
/// job, re-indented to the rendered file's job indent with one blank line above.
pub fn splice(rendered: &str, existing: &str, names: &[String]) -> Result<Kept> {
  let ours = tree::split_lines(rendered);
  let Some(jobs) = tree::top_level(&ours, "jobs") else {
    bail!("the rendered release workflow has no top-level `jobs:` key");
  };
  let theirs = tree::split_lines(existing);
  let their_jobs = tree::top_level(&theirs, "jobs");
  let indent = tree::child_indent(&ours, &jobs);
  let mut details = Vec::new();
  let mut notes = Vec::new();
  let mut block: Vec<String> = Vec::new();
  for name in names {
    if tree::child(&ours, &jobs, name).is_some() {
      bail!("[tool.devkit].release-workflow-jobs names `{name}`, a job of the devkit template's own; rename the project's job");
    }
    let Some(job) = their_jobs.as_ref().and_then(|j| tree::child(&theirs, j, name)) else {
      notes.push(format!(
        ".github/workflows/release.yml has no job `{name}` yet; [tool.devkit].release-workflow-jobs keeps it once it is written."
      ));
      continue;
    };
    let mut start = job.line;
    while start > 0 {
      let above = &theirs[start - 1];
      if !(above.trim_start().starts_with('#') && above.len() - above.trim_start().len() == job.indent) {
        break;
      }
      start -= 1;
    }
    block.push(String::new());
    block.extend(tree::re_indent(&theirs[start..job.end], job.indent, indent));
    details.push(format!("kept job {name}"));
  }
  let text = if block.is_empty() {
    rendered.to_string()
  } else {
    // `jobs.end` excludes trailing blank lines, so the block lands under the last job.
    tree::apply_edits(
      rendered,
      &[Edit::Insert {
        at: jobs.end,
        lines: block,
      }],
    )
  };
  Ok(Kept { text, details, notes })
}

#[cfg(test)]
mod tests {
  use super::*;

  const RENDERED: &str = "# header\nname: Release\non:\n  release:\n    types: [published]\n\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: build\n\n  publish:\n    needs: build\n    runs-on: ubuntu-latest\n    steps:\n      - run: publish\n";
  const PEERS: &str = "  # The hub's bundle.\n  peers:\n    runs-on: ubuntu-latest\n    permissions:\n      contents: write\n    steps:\n      - run: peers\n";

  fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
  }

  #[test]
  fn a_named_job_is_copied_under_jobs_with_its_comment_and_nested_lines() {
    let existing = format!("{RENDERED}\n{PEERS}\n");
    let k = splice(RENDERED, &existing, &names(&["peers"])).unwrap();
    assert_eq!(k.text, format!("{RENDERED}\n{PEERS}"));
    assert_eq!(k.details, vec!["kept job peers"]);
    assert!(k.notes.is_empty());
    // Idempotent: the result spliced again is the same text.
    assert_eq!(splice(RENDERED, &k.text, &names(&["peers"])).unwrap().text, k.text);
    // Found after other jobs and shifted to the rendered job indent when the existing file
    // used four spaces; the lines below keep their own step (a shift, not a rescale).
    let four = "jobs:\n    other:\n        runs-on: x\n    peers:\n        runs-on: y\n        steps:\n            - run: z\n";
    let k = splice(RENDERED, four, &names(&["peers"])).unwrap();
    assert!(
      k.text
        .ends_with("      - run: publish\n\n  peers:\n      runs-on: y\n      steps:\n          - run: z\n"),
      "{}",
      k.text
    );
    // Two names keep their order; one present and one absent give one detail and one note.
    let k = splice(RENDERED, &existing, &names(&["docs", "peers"])).unwrap();
    assert_eq!(k.details, vec!["kept job peers"]);
    assert_eq!(k.notes.len(), 1);
    assert!(k.notes[0].contains("no job `docs` yet"), "{}", k.notes[0]);
  }

  #[test]
  fn a_missing_job_is_a_note_and_a_template_job_name_is_refused() {
    let k = splice(RENDERED, "name: mine\n", &names(&["peers"])).unwrap();
    assert_eq!(k.text, RENDERED);
    assert!(k.details.is_empty());
    assert!(k.notes[0].contains("no job `peers` yet"), "{:?}", k.notes);
    assert_eq!(splice(RENDERED, "", &names(&["peers"])).unwrap().text, RENDERED);
    let e = splice(RENDERED, RENDERED, &names(&["publish"])).unwrap_err().to_string();
    assert!(e.contains("`publish`") && e.contains("template"), "{e}");
    let e = splice("name: x\n", "", &names(&["peers"])).unwrap_err().to_string();
    assert!(e.contains("jobs"), "{e}");
  }
}
