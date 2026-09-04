//! Docker standardisation: templated `docker/` files replaced whole and the compose file
//! edited in place — each only with the user's consent, given per file or once for all.

pub mod compose_rules;
pub mod hunks;
pub mod scaffold;
pub mod static_files;

use std::cell::Cell;
use std::path::Path;

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::prompt::Prompt;

use crate::changes::Changes;
use crate::context::ProjectContext;
use crate::vscode::protocol::{Proposal, Response, Reviewer};

/// How consent questions are answered for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  /// A terminal is attached: ask per file.
  Ask,
  /// `--replace-docker`, or the user answered `replace all`.
  ReplaceAll,
  /// No terminal and no flag: show diffs, change nothing.
  KeepAll,
  /// `--dry-run` / `--check`: every intended edit is recorded, nothing is asked or written.
  DryRun,
}

/// The injectable collaborators, in the style of the release and pin crates.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub prompt: &'a dyn Prompt,
  /// The VS Code reviewer when one is available; consulted before the terminal prompt.
  pub reviewer: Option<&'a dyn Reviewer>,
  pub mode: Mode,
}

/// What the user decided about one [`Proposal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
  Keep,
  Replace,
  /// The proposed text with the rejected hunks reverted, assembled by the CLI.
  Partial {
    text: String,
    accepted: usize,
    total: usize,
  },
}

impl Decision {
  /// The text to write, or `None` to keep the file as it is.
  pub fn text(self, proposal: &Proposal) -> Option<String> {
    match self {
      Decision::Keep => None,
      Decision::Replace => Some(proposal.proposed.clone()),
      Decision::Partial { text, .. } => Some(text),
    }
  }

  /// The change-report line: `full` for a replace, with the hunk count for a partial.
  pub fn detail(&self, full: &str) -> String {
    match self {
      Decision::Partial { accepted, total, .. } => format!("{full} ({accepted} of {total} hunks)"),
      _ => full.to_string(),
    }
  }
}

/// Consent state for one run. `Cell` because `replace all` upgrades the mode through a
/// shared reference: the same `&Consent` is handed to every step. The reviewer is dropped
/// after a transport error so a broken VS Code costs one note, not one per file.
pub struct Consent<'a> {
  prompt: &'a dyn Prompt,
  reviewer: Cell<Option<&'a dyn Reviewer>>,
  mode: Cell<Mode>,
  declined_silently: Cell<bool>,
}

impl<'a> Consent<'a> {
  pub fn new(prompt: &'a dyn Prompt, reviewer: Option<&'a dyn Reviewer>, mode: Mode) -> Self {
    Self {
      prompt,
      reviewer: Cell::new(reviewer),
      mode: Cell::new(mode),
      declined_silently: Cell::new(false),
    }
  }

  /// Decide one proposal whose diff was just printed: VS Code first when a reviewer is
  /// present, then the terminal. `dismissed` falls back to the terminal for this file
  /// only; an error or a malformed answer retires the reviewer for the run.
  pub fn decide(&self, p: &Proposal) -> Result<Decision> {
    match self.mode.get() {
      Mode::ReplaceAll | Mode::DryRun => return Ok(Decision::Replace),
      Mode::KeepAll => {
        self.declined_silently.set(true);
        return Ok(Decision::Keep);
      }
      Mode::Ask => {}
    }
    if let Some(r) = self.reviewer.get() {
      match r.review(p, true) {
        Ok(Response::Replace) => return Ok(Decision::Replace),
        Ok(Response::ReplaceAll) => {
          self.mode.set(Mode::ReplaceAll);
          return Ok(Decision::Replace);
        }
        Ok(Response::Keep) => return Ok(Decision::Keep),
        Ok(Response::Partial { accepted }) => match partial(p, &accepted) {
          Ok(d) => return Ok(d),
          Err(e) => self.retire_reviewer(&format!("{e:#}")),
        },
        Ok(Response::Dismissed) => println!("Diff closed in VS Code; answer here instead."),
        Ok(Response::Error { message }) => self.retire_reviewer(&message),
        Err(e) => self.retire_reviewer(&format!("{e:#}")),
      }
    }
    Ok(match self.prompt.ask(&p.question)?.as_str() {
      "replace" => Decision::Replace,
      "replace all" => {
        self.mode.set(Mode::ReplaceAll);
        Decision::Replace
      }
      _ => Decision::Keep,
    })
  }

  fn retire_reviewer(&self, why: &str) {
    self.reviewer.set(None);
    println!("note: VS Code review unavailable ({why}); using the terminal prompt for the rest of the run.");
  }

  /// A change was kept only because nobody could be asked.
  pub fn kept_silently(&self) -> bool {
    self.declined_silently.get()
  }
}

/// A partial answer with every hunk is a replace and with none a keep, so the report and
/// the terminal flow see the same three outcomes.
fn partial(p: &Proposal, accepted: &[usize]) -> Result<Decision> {
  let mut accepted = accepted.to_vec();
  accepted.sort_unstable();
  accepted.dedup();
  let text = hunks::assemble(&p.current, &p.proposed, &p.hunks, &accepted)?;
  Ok(match accepted.len() {
    0 => Decision::Keep,
    n if n == p.hunks.len() => Decision::Replace,
    n => Decision::Partial {
      text,
      accepted: n,
      total: p.hunks.len(),
    },
  })
}

/// Everything Docker: static files first, then the compose file, then advisories.
pub fn apply(ctx: &ProjectContext, templates_dir: &Path, deps: &Deps, changes: &mut Changes) -> Result<()> {
  let consent = Consent::new(deps.prompt, deps.reviewer, deps.mode);
  static_files::apply(ctx, templates_dir, &consent, changes)?;
  compose(ctx, templates_dir, deps.runner, &consent, changes)?;
  if consent.kept_silently() {
    changes
      .notes
      .push("Docker files were left alone because no terminal was available to confirm; pass --replace-docker to apply them.".into());
  }
  Ok(())
}

/// The compose file: created whole from the scaffold when absent; otherwise one diff per
/// listed service (its rule-engine edits, or its scaffold block when the file lacks it)
/// and one for the top-level keys. Each diff is computed against the text as accepted so
/// far, so a hunk never straddles two services and a partial answer leaves later line
/// numbers valid.
fn compose(ctx: &ProjectContext, templates_dir: &Path, runner: &dyn Runner, consent: &Consent, changes: &mut Changes) -> Result<()> {
  use aeth_devkit_core::compose::find_compose_file;
  use aeth_devkit_core::compose::tree::{self, Edit};

  let sc = scaffold::load(templates_dir, ctx)?;
  let tag = scaffold::GitTag::new(runner, ctx);
  let Some(path) = find_compose_file(&ctx.root)? else {
    let text = tag.fill(&scaffold::render_file(&sc, &ctx.docker_services));
    changes.record_optional(
      &ctx.root.join("docker").join("compose.yaml"),
      None,
      &text,
      vec!["created from template".into()],
    )?;
    changes.notes.extend(tag.note());
    return Ok(());
  };
  let rel = path
    .strip_prefix(&ctx.root)
    .map(|p| p.to_string_lossy().replace('\\', "/"))
    .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
  // A BOM would hide `services:` from the line parser; drop it (the rewrite omits it).
  let original = std::fs::read_to_string(&path)
    .with_context(|| format!("reading {rel}"))?
    .trim_start_matches('\u{feff}')
    .to_string();
  let mut text = original.clone();
  let mut details: Vec<String> = Vec::new();
  // One diff and one decision; `text` advances only on replace or partial. A closure
  // (not a fn) so it can share `consent`, `tag`, `rel` and `details` without a struct.
  let mut ask = |text: &mut String, what: &str, question: String, edits: &[Edit], edit_details: Vec<String>| -> Result<()> {
    if edits.is_empty() {
      return Ok(());
    }
    let proposed = tag.fill(&tree::apply_edits(text, edits));
    let title = format!("{rel}: {what}");
    println!("{}", static_files::unified_diff(&title, text, &proposed));
    let proposal = Proposal::new(&title, question, text, &proposed);
    match consent.decide(&proposal)? {
      Decision::Keep => println!("Kept {title}."),
      Decision::Replace => {
        *text = proposal.proposed;
        details.extend(edit_details);
      }
      Decision::Partial { text: t, accepted, total } => {
        *text = t;
        details.push(format!("{what}: {accepted} of {total} hunks applied"));
      }
    }
    Ok(())
  };
  let keywords = "[replace / replace all / anything else keeps it]:";
  for name in &ctx.docker_services {
    let lines = tree::split_lines(&text);
    let Some(services) = tree::top_level(&lines, "services") else {
      bail!("{rel} has no top-level `services:` key");
    };
    // The scaffold block for *this* service, parsed as its own little document so the
    // rule engine can look keys up in it exactly like in the project file.
    let sc_doc = tree::split_lines(&format!("services:\n{}", scaffold::service_block(&sc, name)));
    let sc_services = tree::top_level(&sc_doc, "services").expect("scaffold starts with services:");
    let sc_svc = tree::child(&sc_doc, &sc_services, name).expect("scaffold block names the service");
    match tree::child(&lines, &services, name) {
      Some(svc) => {
        let o = compose_rules::service_edits(&lines, &svc, &sc_doc, &sc_svc, name);
        ask(
          &mut text,
          &format!("service {name}"),
          format!("Apply the {name} edits to {rel}? {keywords}"),
          &o.edits,
          o.details,
        )?;
      }
      None => {
        let indent = tree::child_indent(&lines, &services);
        let mut block = tree::re_indent(&sc_doc[sc_svc.line..sc_svc.end], sc_svc.indent, indent);
        // One blank line between service blocks, matching the sister files.
        if services.end > 0 && !lines[services.end - 1].trim().is_empty() {
          block.insert(0, String::new());
        }
        let edit = Edit::Insert {
          at: services.end,
          lines: block,
        };
        ask(
          &mut text,
          &format!("new service {name}"),
          format!("Add service {name} to {rel}? {keywords}"),
          &[edit],
          vec![format!("added service {name}")],
        )?;
      }
    }
  }
  let lines = tree::split_lines(&text);
  let o = compose_rules::top_level_edits(&lines, &tree::split_lines(&sc.tail));
  ask(
    &mut text,
    "top level",
    format!("Apply the top-level edits to {rel}? {keywords}"),
    &o.edits,
    o.details,
  )?;
  if text == original {
    changes.record_optional(&path, Some(&original), &original, vec![])?;
    return Ok(());
  }
  changes.record_optional(&path, Some(&original), &text, details)?;
  changes.notes.extend(tag.note());
  Ok(())
}

#[cfg(test)]
mod consent_tests {
  use super::*;
  use crate::vscode::protocol::ScriptedReviewer;
  use aeth_devkit_core::prompt::ScriptedPrompt;

  fn proposal(title: &str) -> Proposal {
    Proposal::new(title, format!("{title}?"), "a\nb\n", "a\nc\n")
  }

  #[test]
  fn replace_all_sticks_for_the_rest_of_the_run() {
    let p = ScriptedPrompt::new(&["replace all"]);
    let c = Consent::new(&p, None, Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Replace, "no second question");
    assert_eq!(p.asked.borrow().len(), 1);
  }

  #[test]
  fn anything_but_the_keywords_keeps() {
    let p = ScriptedPrompt::new(&["replace", "y", ""]);
    let c = Consent::new(&p, None, Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("c")).unwrap(), Decision::Keep);
  }

  #[test]
  fn dry_run_and_keep_all_never_ask() {
    let p = ScriptedPrompt::new(&[]);
    let dry = Consent::new(&p, None, Mode::DryRun);
    assert_eq!(dry.decide(&proposal("a")).unwrap(), Decision::Replace);
    let keep = Consent::new(&p, None, Mode::KeepAll);
    assert_eq!(keep.decide(&proposal("a")).unwrap(), Decision::Keep);
    assert!(keep.kept_silently());
    let all = Consent::new(&p, None, Mode::ReplaceAll);
    assert_eq!(all.decide(&proposal("a")).unwrap(), Decision::Replace);
    assert!(p.asked.borrow().is_empty());
  }

  #[test]
  fn reviewer_answers_first_and_dismissed_falls_back_per_file() {
    let p = ScriptedPrompt::new(&["replace"]);
    let r = ScriptedReviewer::new(vec![Response::Keep, Response::Dismissed, Response::ReplaceAll]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Keep);
    assert_eq!(
      c.decide(&proposal("b")).unwrap(),
      Decision::Replace,
      "dismissed, terminal said replace"
    );
    assert_eq!(c.decide(&proposal("c")).unwrap(), Decision::Replace);
    assert_eq!(
      c.decide(&proposal("d")).unwrap(),
      Decision::Replace,
      "replace all from VS Code sticks"
    );
    assert_eq!(p.asked.borrow().len(), 1);
    assert_eq!(*r.reviewed.borrow(), vec!["a", "b", "c"]);
  }

  #[test]
  fn partial_assembles_text_and_collapses_to_replace_or_keep() {
    let p = ScriptedPrompt::new(&[]);
    let cur = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\n";
    let prop = Proposal::new("t", "q", cur, new);
    assert_eq!(prop.hunks.len(), 2);
    let r = ScriptedReviewer::new(vec![
      Response::Partial { accepted: vec![1, 1] },
      Response::Partial { accepted: vec![0, 1] },
      Response::Partial { accepted: vec![] },
    ]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(
      c.decide(&prop).unwrap(),
      Decision::Partial {
        text: "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n".into(),
        accepted: 1,
        total: 2
      }
    );
    assert_eq!(c.decide(&prop).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&prop).unwrap(), Decision::Keep);
    assert_eq!(
      Decision::Partial {
        text: String::new(),
        accepted: 1,
        total: 2
      }
      .detail("replaced"),
      "replaced (1 of 2 hunks)"
    );
  }

  #[test]
  fn a_broken_reviewer_is_retired_after_one_note() {
    let p = ScriptedPrompt::new(&["", ""]);
    let r = ScriptedReviewer::new(vec![Response::Error {
      message: "protocol 9".into(),
    }]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Keep);
    assert_eq!(r.reviewed.borrow().len(), 1, "not consulted again");
    assert_eq!(p.asked.borrow().len(), 2);
    let bad = ScriptedReviewer::new(vec![Response::Partial { accepted: vec![7] }]);
    let c = Consent::new(&p, Some(&bad), Mode::Ask);
    assert!(
      c.decide(&proposal("a")).is_err(),
      "prompt queue is empty, so the fallback prompt errors: proves the reviewer was retired"
    );
  }
}
