//! Docker standardisation: templated `docker/` files replaced whole and the compose file
//! edited in place — each only with the user's consent, given per file or once for all.

pub mod compose_rules;
pub mod scaffold;
pub mod static_files;

use std::cell::Cell;
use std::path::Path;

use anyhow::{Context as _, Result};

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::prompt::Prompt;

use crate::changes::Changes;
use crate::context::ProjectContext;

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
  pub mode: Mode,
  /// Whether a human can answer the add-service question (stdin is a terminal and this
  /// is not a dry run); see [`Consent::add`] for what happens without one.
  pub interactive: bool,
}

/// Consent state for one run. `Cell` because `replace all` upgrades the mode through a
/// shared reference: the same `&Consent` is handed to every step.
pub struct Consent<'a> {
  prompt: &'a dyn Prompt,
  mode: Cell<Mode>,
  interactive: bool,
  declined_silently: Cell<bool>,
}

impl<'a> Consent<'a> {
  pub fn new(prompt: &'a dyn Prompt, mode: Mode, interactive: bool) -> Self {
    Self {
      prompt,
      mode: Cell::new(mode),
      interactive,
      declined_silently: Cell::new(false),
    }
  }

  /// Whether a change whose diff was just printed may be applied.
  pub fn replace(&self, question: &str) -> Result<bool> {
    match self.mode.get() {
      Mode::ReplaceAll | Mode::DryRun => Ok(true),
      Mode::KeepAll => {
        self.declined_silently.set(true);
        Ok(false)
      }
      Mode::Ask => match self.prompt.ask(question)?.as_str() {
        "replace" => Ok(true),
        "replace all" => {
          self.mode.set(Mode::ReplaceAll);
          Ok(true)
        }
        _ => Ok(false),
      },
    }
  }

  /// Whether a listed-but-absent service may be scaffolded into the compose file. Without
  /// a human, `ReplaceAll` can only have come from `--replace-docker` (a prompt cannot
  /// have upgraded it), and the flag answers `add` so `--check` and `--replace-docker`
  /// agree in CI; a plain non-tty run records the silent skip so the run's note fires.
  pub fn add(&self, question: &str) -> Result<bool> {
    match self.mode.get() {
      Mode::DryRun => Ok(true), // an intended edit, shown like every other
      _ if self.interactive => Ok(self.prompt.ask(question)? == "add"),
      Mode::ReplaceAll => Ok(true),
      Mode::Ask | Mode::KeepAll => {
        self.declined_silently.set(true);
        Ok(false)
      }
    }
  }

  /// A change was kept only because nobody could be asked.
  pub fn kept_silently(&self) -> bool {
    self.declined_silently.get()
  }
}

/// Everything Docker: static files first, then the compose file, then advisories.
pub fn apply(ctx: &ProjectContext, templates_dir: &Path, deps: &Deps, changes: &mut Changes) -> Result<()> {
  let consent = Consent::new(deps.prompt, deps.mode, deps.interactive);
  static_files::apply(ctx, templates_dir, &consent, changes)?;
  compose(ctx, templates_dir, deps.runner, &consent, changes)?;
  if consent.kept_silently() {
    changes
      .notes
      .push("Docker files were left alone because no terminal was available to confirm; pass --replace-docker to apply them.".into());
  }
  Ok(())
}

/// The compose file: created whole from the scaffold when absent; otherwise every listed
/// service is checked against the rule table and all edits are shown as one diff behind
/// one prompt. A listed service the file lacks is offered as a scaffold block.
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
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
  let lines = tree::split_lines(&text);
  // An include-only file, a placeholder, or an unrelated file the tree walk found first:
  // not a reason to abort a run whose Dockerfile is already on disk, so the compose step
  // steps aside and says so.
  let services = match tree::top_level(&lines, "services") {
    Some(s) if !s.is_inline() => s,
    found => {
      // Still recorded as managed (the gitignore advisory in `lib` reads that list).
      changes.record_optional(&path, Some(&text), &text, vec![])?;
      changes.notes.push(if found.is_some() {
        format!("{rel} writes `services:` inline, which the compose step cannot edit; switch it to the block form.")
      } else {
        format!(
          "{rel} has no top-level `services:` key, so the compose step left it alone; add the app service there by hand, or remove the file to get a scaffold."
        )
      });
      return Ok(());
    }
  };
  let present = tree::children(&lines, &services);
  let mut edits: Vec<Edit> = Vec::new();
  let mut details: Vec<String> = Vec::new();
  for name in &ctx.docker_services {
    // The scaffold block for *this* service, parsed as its own little document so the
    // rule engine can look keys up in it exactly like in the project file.
    let sc_doc = tree::split_lines(&format!("services:\n{}", scaffold::service_block(&sc, name)));
    let sc_services = tree::top_level(&sc_doc, "services").expect("scaffold starts with services:");
    let sc_svc = tree::child(&sc_doc, &sc_services, name).expect("scaffold block names the service");
    match present.iter().find(|n| &n.key == name) {
      // `app: {image: x}`: nothing can be inserted under it.
      Some(svc) if svc.is_inline() => {
        changes.notes.push(format!(
          "{rel}: service {name} is written inline, which the compose step cannot edit; switch it to the block form."
        ));
      }
      Some(svc) => {
        let o = compose_rules::service_edits(&lines, svc, &sc_doc, &sc_svc, name);
        edits.extend(o.edits);
        details.extend(o.details);
        changes.notes.extend(o.notes);
      }
      None => {
        let found = present.iter().map(|n| n.key.as_str()).collect::<Vec<_>>().join(", ");
        let q = format!("Service \"{name}\" is not in {rel} (found: {found}). Add it? [add / anything else skips]:");
        if consent.add(&q)? {
          let indent = tree::child_indent(&lines, &services);
          let mut block = tree::re_indent(&sc_doc[sc_svc.line..sc_svc.end], sc_svc.indent, indent);
          // One blank line between service blocks, matching the sister files.
          if services.end > 0 && !lines[services.end - 1].trim().is_empty() {
            block.insert(0, String::new());
          }
          edits.push(Edit::Insert {
            at: services.end,
            lines: block,
          });
          details.push(format!("added service {name}"));
        } else {
          println!("Skipped service \"{name}\".");
        }
      }
    }
  }
  let o = compose_rules::top_level_edits(&lines, &tree::split_lines(&sc.tail));
  edits.extend(o.edits);
  details.extend(o.details);
  changes.notes.extend(o.notes);
  if edits.is_empty() {
    changes.record_optional(&path, Some(&text), &text, vec![])?;
    return Ok(());
  }
  let new_text = tag.fill(&tree::apply_edits(&text, &edits));
  println!("{}", static_files::unified_diff(&rel, &text, &new_text));
  if consent.replace(&format!(
    "Apply these edits to {rel}? [replace / replace all / anything else keeps it]:"
  ))? {
    changes.record_optional(&path, Some(&text), &new_text, details)?;
  } else {
    changes.record_optional(&path, Some(&text), &text, vec![])?;
    println!("Kept {rel}.");
  }
  // The note explains the value in the diff the user just saw, whichever way they answered.
  changes.notes.extend(tag.note());
  Ok(())
}

#[cfg(test)]
mod consent_tests {
  use super::*;
  use aeth_devkit_core::prompt::ScriptedPrompt;

  #[test]
  fn replace_all_sticks_for_the_rest_of_the_run() {
    let p = ScriptedPrompt::new(&["replace all"]);
    let c = Consent::new(&p, Mode::Ask, true);
    assert!(c.replace("a?").unwrap());
    assert!(c.replace("b?").unwrap(), "no second question");
    assert_eq!(p.asked.borrow().len(), 1);
  }

  #[test]
  fn anything_but_the_keywords_keeps() {
    let p = ScriptedPrompt::new(&["replace", "y", "", "add", "no"]);
    let c = Consent::new(&p, Mode::Ask, true);
    assert!(c.replace("a?").unwrap());
    assert!(!c.replace("b?").unwrap());
    assert!(!c.replace("c?").unwrap());
    assert!(c.add("d?").unwrap());
    assert!(!c.add("e?").unwrap());
  }

  #[test]
  fn dry_run_and_keep_all_never_ask() {
    let p = ScriptedPrompt::new(&[]);
    let dry = Consent::new(&p, Mode::DryRun, false);
    assert!(dry.replace("a?").unwrap() && dry.add("b?").unwrap());
    let keep = Consent::new(&p, Mode::KeepAll, false);
    assert!(!keep.replace("a?").unwrap() && !keep.add("b?").unwrap());
    assert!(keep.kept_silently());
    // --replace-docker without a terminal: the flag stands in for every answer.
    let all = Consent::new(&p, Mode::ReplaceAll, false);
    assert!(all.replace("a?").unwrap());
    assert!(all.add("b?").unwrap());
    assert!(!all.kept_silently());
    assert!(p.asked.borrow().is_empty());
  }
}
