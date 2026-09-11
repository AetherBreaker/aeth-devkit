//! Docker standardisation: the Dockerfile (from the installed container package) replaced
//! whole and the compose file edited in place — each only with the user's consent, given per
//! file or once for all.

pub mod compose_rules;
pub mod hunks;
pub mod scaffold;
pub mod static_files;

use std::cell::Cell;

use anyhow::{Context as _, Result};

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::prompt::Prompt;

use crate::changes::Changes;
use crate::context::ProjectContext;
use crate::gate::Gates;
use crate::packages::Venv;
use crate::vscode::protocol::{Proposal, Response, Reviewer};

/// How consent questions are answered for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  /// Standard input is attached: ask per file.
  Ask,
  /// The user answered `replace all`: the shown diffs that follow are replaced, an add is
  /// still asked.
  ReplaceAll,
  /// `--yes`: every proposal accepted, nothing asked.
  Yes,
  /// `--dry-run`: every intended edit is recorded, nothing is asked or written.
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
}

impl<'a> Consent<'a> {
  pub fn new(prompt: &'a dyn Prompt, reviewer: Option<&'a dyn Reviewer>, mode: Mode) -> Self {
    Self {
      prompt,
      reviewer: Cell::new(reviewer),
      mode: Cell::new(mode),
    }
  }

  /// Decide one proposal whose diff was just printed: VS Code first when a reviewer is
  /// present, then the terminal. `dismissed` falls back to the terminal for this file
  /// only; an error or a malformed answer retires the reviewer for the run.
  ///
  /// `offer_replace_all` is whether a typed `replace all` is offered for and covers this
  /// proposal. Adding a listed-but-absent service is not: a typo in pyproject must not grow
  /// the compose file without someone reading the service name. `--yes` accepts it like
  /// everything else, since that flag is the user's blanket answer.
  pub fn decide(&self, p: &Proposal, offer_replace_all: bool) -> Result<Decision> {
    match self.mode.get() {
      // A dry run's intended edit, shown like every other; `--yes` needs no diff read.
      Mode::DryRun | Mode::Yes => return Ok(Decision::Replace),
      Mode::ReplaceAll if offer_replace_all => return Ok(Decision::Replace),
      Mode::Ask | Mode::ReplaceAll => {}
    }
    if let Some(r) = self.reviewer.get() {
      match r.review(p, offer_replace_all) {
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
    let keywords = if offer_replace_all {
      "[replace / replace all / anything else keeps it]:"
    } else {
      "[replace / anything else keeps it]:"
    };
    Ok(match self.prompt.ask(&format!("{} {keywords}", p.question))?.as_str() {
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

/// Everything Docker: the Dockerfile from the installed container package first, then the
/// compose file from the same package, then advisories.
pub fn apply(ctx: &ProjectContext, deps: &crate::Deps, gates: &Gates, changes: &mut Changes) -> Result<()> {
  let docker = &deps.docker;
  let consent = Consent::new(docker.prompt, docker.reviewer, docker.mode);
  static_files::apply(ctx, deps.venv, &consent, gates, changes)?;
  compose(ctx, deps.venv, docker.runner, &consent, gates, changes)
}

/// The compose file: created whole from the scaffold when absent; otherwise one diff per
/// listed service (its rule-engine edits, or its scaffold block when the file lacks it)
/// and one for the top-level keys. Each diff is computed against the text as accepted so
/// far, so a hunk never straddles two services and a partial answer leaves later line
/// numbers valid.
fn compose(
  ctx: &ProjectContext,
  venv: &dyn Venv,
  runner: &dyn Runner,
  consent: &Consent,
  gates: &Gates,
  changes: &mut Changes,
) -> Result<()> {
  use aeth_devkit_core::compose::find_compose_file;
  use aeth_devkit_core::compose::tree::{self, Edit};

  let sc = scaffold::load(ctx, venv, gates)?;
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
  // An include-only file, a placeholder, or an unrelated file the tree walk found first:
  // not a reason to abort a run whose Dockerfile is already on disk, so the compose step
  // steps aside and says so.
  match tree::top_level(&tree::split_lines(&original), "services") {
    Some(s) if !s.is_inline() => {}
    found => {
      // Still recorded as managed (the gitignore advisory in `lib` reads that list).
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      if found.is_some() {
        changes.errors.push(format!(
          "{rel} writes `services:` inline, which the compose step cannot edit; switch it to the block form."
        ));
      } else {
        // An `include:`-only aggregator is a supported Compose layout: the services live
        // in the included files, and defining one here would conflict with them rather
        // than override. Nothing to fix, so this warns instead of being an `error:`.
        changes.warnings.push(format!(
          "{rel} has no top-level `services:` key, so the compose step left it alone. If it only `include:`s other files, devkit cannot manage the services they define; add the app service here by hand, or remove the file to get a scaffold."
        ));
      }
      return Ok(());
    }
  }
  let mut text = original.clone();
  let mut details: Vec<String> = Vec::new();
  // One diff and one decision; `text` advances only on replace or partial. A closure
  // (not a fn) so it can share `consent`, `tag`, `rel` and `details` without a struct.
  let mut ask = |text: &mut String,
                 what: &str,
                 question: String,
                 edits: &[Edit],
                 edit_details: Vec<String>,
                 offer_replace_all: bool|
   -> Result<()> {
    if edits.is_empty() {
      return Ok(());
    }
    let proposed = tag.fill(&tree::apply_edits(text, edits));
    let title = format!("{rel}: {what}");
    println!("{}", static_files::unified_diff(&title, text, &proposed));
    let proposal = Proposal::new(&title, question, text, &proposed);
    match consent.decide(&proposal, offer_replace_all)? {
      Decision::Keep => println!("Kept {title}."),
      Decision::Replace => {
        *text = proposal.proposed;
        details.extend(edit_details);
      }
      Decision::Partial { text: t, accepted, total } => {
        *text = t;
        details.push(format!("{what} ({accepted} of {total} hunks)"));
      }
    }
    Ok(())
  };
  for name in &ctx.docker_services {
    let lines = tree::split_lines(&text);
    // Block-form above, and every accepted edit adds under it or beside it.
    let services = tree::top_level(&lines, "services").expect("services: survives the edits");
    // The scaffold block for *this* service, parsed as its own little document so the
    // rule engine can look keys up in it exactly like in the project file.
    let sc_doc = tree::split_lines(&format!("services:\n{}", scaffold::service_block(&sc, name)));
    let sc_services = tree::top_level(&sc_doc, "services").expect("scaffold starts with services:");
    let sc_svc = tree::child(&sc_doc, &sc_services, name).expect("scaffold block names the service");
    match tree::child(&lines, &services, name) {
      // `app: {image: x}`: nothing can be inserted under it.
      Some(svc) if svc.is_inline() => {
        changes.errors.push(format!(
          "{rel}: service {name} is written inline, which the compose step cannot edit; switch it to the block form."
        ));
      }
      Some(svc) => {
        let o = compose_rules::service_edits(&lines, &svc, &sc_doc, &sc_svc, name, &sc.rules);
        changes.errors.extend(o.errors);
        ask(
          &mut text,
          &format!("service {name}"),
          format!("Apply the {name} edits to {rel}?"),
          &o.edits,
          o.details,
          true,
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
        // Never pre-answered, so `replace all` is not on offer (see `Consent::decide`).
        ask(
          &mut text,
          &format!("new service {name}"),
          format!("Add service {name} to {rel}?"),
          &[edit],
          vec![format!("added service {name}")],
          false,
        )?;
      }
    }
  }
  let lines = tree::split_lines(&text);
  let o = compose_rules::top_level_edits(&lines, &tree::split_lines(&sc.tail));
  changes.errors.extend(o.errors);
  ask(
    &mut text,
    "top level",
    format!("Apply the top-level edits to {rel}?"),
    &o.edits,
    o.details,
    true,
  )?;
  if text == original {
    changes.record_optional(&path, Some(&original), &original, vec![])?;
  } else {
    changes.record_optional(&path, Some(&original), &text, details)?;
  }
  // The note explains the value in the diffs the user just saw, whichever way they answered.
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
    assert_eq!(c.decide(&proposal("a"), true).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("b"), true).unwrap(), Decision::Replace, "no second question");
    assert_eq!(p.asked.borrow().len(), 1);
  }

  #[test]
  fn anything_but_the_keywords_keeps() {
    let p = ScriptedPrompt::new(&["replace", "y", ""]);
    let c = Consent::new(&p, None, Mode::Ask);
    assert_eq!(c.decide(&proposal("a"), true).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("b"), true).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("c"), true).unwrap(), Decision::Keep);
  }

  #[test]
  fn dry_run_yes_and_replace_all_never_ask() {
    let p = ScriptedPrompt::new(&[]);
    let dry = Consent::new(&p, None, Mode::DryRun);
    assert_eq!(dry.decide(&proposal("a"), true).unwrap(), Decision::Replace);
    assert_eq!(
      dry.decide(&proposal("b"), false).unwrap(),
      Decision::Replace,
      "an add is intended drift too"
    );
    let yes = Consent::new(&p, None, Mode::Yes);
    assert_eq!(yes.decide(&proposal("a"), true).unwrap(), Decision::Replace);
    assert_eq!(
      yes.decide(&proposal("add b"), false).unwrap(),
      Decision::Replace,
      "--yes covers an add"
    );
    let all = Consent::new(&p, None, Mode::ReplaceAll);
    assert_eq!(all.decide(&proposal("a"), true).unwrap(), Decision::Replace);
    assert!(p.asked.borrow().is_empty());
  }

  #[test]
  fn running_out_of_answers_is_an_error_not_a_keep() {
    // The scripted stand-in for stdin ending: the run cancels rather than defaulting.
    let p = ScriptedPrompt::new(&["replace"]);
    let c = Consent::new(&p, None, Mode::Ask);
    assert_eq!(c.decide(&proposal("a"), true).unwrap(), Decision::Replace);
    assert!(c.decide(&proposal("b"), true).is_err());
  }

  #[test]
  fn replace_all_covers_offered_proposals_only() {
    // An add is asked whatever the mode; `replace all` typed there still sticks for the
    // shown diffs that follow.
    let p = ScriptedPrompt::new(&["", "replace all", ""]);
    let all = Consent::new(&p, None, Mode::ReplaceAll);
    assert_eq!(all.decide(&proposal("add a"), false).unwrap(), Decision::Keep);
    assert_eq!(all.decide(&proposal("file"), true).unwrap(), Decision::Replace);
    assert_eq!(p.asked.borrow().len(), 1, "only the add asked");
    let ask = Consent::new(&p, None, Mode::Ask);
    assert_eq!(ask.decide(&proposal("add b"), false).unwrap(), Decision::Replace);
    assert_eq!(
      ask.decide(&proposal("add c"), false).unwrap(),
      Decision::Keep,
      "still asked after replace all"
    );
    assert_eq!(p.asked.borrow().len(), 3);
  }

  #[test]
  fn reviewer_answers_first_and_dismissed_falls_back_per_file() {
    let p = ScriptedPrompt::new(&["replace"]);
    let r = ScriptedReviewer::new(vec![Response::Keep, Response::Dismissed, Response::ReplaceAll]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(c.decide(&proposal("a"), true).unwrap(), Decision::Keep);
    assert_eq!(
      c.decide(&proposal("b"), true).unwrap(),
      Decision::Replace,
      "dismissed, terminal said replace"
    );
    assert_eq!(c.decide(&proposal("c"), true).unwrap(), Decision::Replace);
    assert_eq!(
      c.decide(&proposal("d"), true).unwrap(),
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
      c.decide(&prop, true).unwrap(),
      Decision::Partial {
        text: "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n".into(),
        accepted: 1,
        total: 2
      }
    );
    assert_eq!(c.decide(&prop, true).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&prop, true).unwrap(), Decision::Keep);
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
    assert_eq!(c.decide(&proposal("a"), true).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("b"), true).unwrap(), Decision::Keep);
    assert_eq!(r.reviewed.borrow().len(), 1, "not consulted again");
    assert_eq!(p.asked.borrow().len(), 2);
    let bad = ScriptedReviewer::new(vec![Response::Partial { accepted: vec![7] }]);
    let c = Consent::new(&p, Some(&bad), Mode::Ask);
    assert!(
      c.decide(&proposal("a"), true).is_err(),
      "prompt queue is empty, so the fallback prompt errors: proves the reviewer was retired"
    );
  }
}
