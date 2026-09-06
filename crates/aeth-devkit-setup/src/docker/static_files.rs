//! Whole-file replacement of the templated `docker/` files (everything except the compose
//! file), shown as a diff and applied only on consent. Files the template stopped
//! shipping are reported, never deleted.

use std::path::Path;

use aeth_devkit_core::github;
use aeth_devkit_core::process::Runner;
use anyhow::Result;
use similar::TextDiff;

use crate::changes::Changes;
use crate::context::ProjectContext;
use crate::docker::Consent;
use crate::templates;
use crate::vscode::protocol::Proposal;

/// The files under `docker/` this step owns, by target name (the compose file has its own
/// rule-based flow). One list, so `git::committable` stages exactly what is written here:
/// a file added to the templates but not to this list would be replaced without the
/// HEAD-reset every other managed file gets, then left out of the commit.
pub const TARGETS: &[&str] = &["Dockerfile"];

/// The repository whose `container-v<N>` releases ship the entrypoint binary the
/// Dockerfile fetches; the tag stream is devkit's, whatever project is being set up.
pub const DEVKIT_REPO: &str = "AetherBreaker/aeth-devkit";

/// The `N` of a `container-v<N>` pin in a Dockerfile's download URL, if it has one.
/// Comment lines are skipped so a commented-out `ADD` above the live one is not the pin.
pub fn pinned_container_version(dockerfile: &str) -> Option<u64> {
  let re = regex::Regex::new(r"releases/download/container-v(\d+)/").expect("static regex");
  dockerfile
    .lines()
    .filter(|l| !l.trim_start().starts_with('#'))
    .find_map(|l| re.captures(l).and_then(|c| c[1].parse().ok()))
}

/// The newest `container-v*` tag on devkit's repository, `None` before the first
/// container release. A lookup error is an error: guessing a pin here would write a real
/// but possibly stale tag that no later run revisits (an existing pin is kept as is, and
/// advancing it is a separate command's job, see TODO.md).
fn newest_container_version(runner: &dyn Runner, root: &Path) -> Result<Option<u64>> {
  let tags = github::list_tags(runner, root, DEVKIT_REPO)?;
  Ok(tags.iter().filter_map(|t| t.strip_prefix("container-v")?.parse::<u64>().ok()).max())
}

/// The text as diffed and as VS Code shows it: LF line endings, no byte-order mark. Neither
/// is drift (`.gitattributes` owns line endings, and Windows editors add a BOM the
/// templates never carry), and a BOM would otherwise show as a phantom hunk on the first
/// line. What gets written keeps the file's own endings (see `Proposal`).
pub fn normalize_newlines(s: &str) -> String {
  s.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n")
}

/// `text` with every `\n` turned into `\r\n` when `like` uses CRLF, so a template
/// rendered LF diffs and writes in the file's own convention.
pub fn match_line_endings(text: &str, like: &str) -> String {
  if like.contains("\r\n") && !text.contains("\r\n") {
    text.replace('\n', "\r\n")
  } else {
    text.to_string()
  }
}

/// Three lines of context, both sides labelled so the user can tell which is theirs. Line
/// endings are normalised first: a CRLF checkout against an LF template must show the real
/// changes, not every line.
pub fn unified_diff(rel: &str, old: &str, new: &str) -> String {
  TextDiff::from_lines(normalize_newlines(old), normalize_newlines(new))
    .unified_diff()
    .context_radius(3)
    .header(&format!("{rel} (project)"), &format!("{rel} (devkit template)"))
    .to_string()
}

pub fn apply(ctx: &ProjectContext, templates_dir: &Path, runner: &dyn Runner, consent: &Consent, changes: &mut Changes) -> Result<()> {
  for target in TARGETS {
    let rel = format!("docker/{target}");
    let mut rendered = templates::load(templates_dir, &rel, ctx, templates::Escape::None)?;
    let path = ctx.root.join("docker").join(target);
    let original = crate::read_optional(&path)?;
    // `{container_version}`: the file's own pin when it carries the template's URL shape
    // (no `gh` on a routine run); otherwise devkit's newest container tag, or `1` before
    // the first container release exists. That provisional pin is only worth a note if
    // the file is actually written below, so the note waits.
    let mut provisional = None;
    if rendered.contains("{container_version}") {
      // A Dockerfile that fetches the binary through some other URL shape (a mirror, an
      // ARG) reads as unpinned, so the template's own pin replaces it in the diff below.
      // Said out loud: silently swapping someone's pin inside a whole-file diff is the
      // one part of that diff they are least likely to look for.
      if original
        .as_deref()
        .is_some_and(|o| o.contains("devkit-container") && pinned_container_version(o).is_none())
      {
        changes.notes.push(format!(
          "{rel} fetches devkit-container through a URL devkit does not recognise, so the template's own `container-v<N>` pin is offered in its place; keep the file to stay on yours."
        ));
      }
      let version = match original.as_deref().and_then(pinned_container_version) {
        Some(n) => n,
        None => match newest_container_version(runner, &ctx.root) {
          Ok(Some(n)) => n,
          Ok(None) => {
            provisional = Some(
              "devkit-container pinned to container-v1 provisionally (no container-v* tag on the devkit repository yet); the next devkit release creates that tag.",
            );
            1
          }
          Err(e) => {
            changes.problems.push(format!(
              "{rel} was left alone: devkit's container releases could not be read to pin one ({e:#}); rerun with `gh` working."
            ));
            if let Some(original) = &original {
              changes.record_optional(&path, Some(original), original, vec![])?;
            }
            continue;
          }
        },
      };
      rendered = rendered.replace("{container_version}", &version.to_string());
    }
    let Some(original) = original else {
      changes.record_optional(&path, None, &rendered, vec!["created from template".into()])?;
      changes.notes.extend(provisional.map(str::to_string));
      continue;
    };
    if normalize_newlines(&original) == normalize_newlines(&rendered) {
      // Managed, unchanged. CRLF-only drift is not drift: .gitattributes owns line endings.
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      continue;
    }
    let rendered = match_line_endings(&rendered, &original);
    println!("{}", unified_diff(&rel, &original, &rendered));
    let proposal = Proposal::new(
      &rel,
      format!("Replace {rel}? [replace / replace all / anything else keeps it]:"),
      &original,
      &rendered,
    );
    let decision = consent.decide(&proposal, true)?;
    let detail = decision.detail("replaced with the devkit template");
    match decision.text(&proposal) {
      Some(text) => {
        changes.record_optional(&path, Some(&original), &text, vec![detail])?;
        changes.notes.extend(provisional.map(str::to_string));
      }
      None => {
        changes.record_optional(&path, Some(&original), &original, vec![])?;
        println!("Kept {rel}.");
      }
    }
  }
  // Stray leftovers of the shell-and-Python entrypoint. Reported once, never removed.
  let stray: Vec<&str> = [("docker/entrypoint.sh", false), ("docker/scripts", true)]
    .iter()
    .filter(|(rel, is_dir)| {
      let p = ctx.root.join(rel);
      if *is_dir { p.is_dir() } else { p.is_file() }
    })
    .map(|(rel, _)| *rel)
    .collect();
  if !stray.is_empty() {
    changes.notes.push(format!(
      "{} no longer used: the devkit-container binary replaced the shell entrypoint and its helper scripts; safe to delete.",
      stray.join(" and ")
    ));
  }
  Ok(())
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn every_managed_docker_file_is_committable() {
    let committable = crate::git::committable(std::path::Path::new("."));
    for t in TARGETS {
      assert!(committable.iter().any(|c| c == &format!("docker/{t}")), "{t}: {committable:?}");
    }
  }

  #[test]
  fn the_pin_is_read_from_a_live_download_url_only() {
    assert_eq!(
      pinned_container_version("ADD https://x/releases/download/container-v12/devkit-container-x86_64-unknown-linux-musl /app/d\n"),
      Some(12)
    );
    assert_eq!(
      pinned_container_version("ADD https://x/releases/download/v9.0.0/devkit-container /app/d\n"),
      None
    );
    assert_eq!(
      pinned_container_version(
        "# ADD https://x/releases/download/container-v2/d /app/d\nADD https://x/releases/download/container-v5/d /app/d\n"
      ),
      Some(5),
      "a commented-out ADD is not the pin"
    );
  }

  #[test]
  fn the_newest_container_tag_is_numeric_and_a_lookup_failure_is_an_error() {
    use aeth_devkit_core::process::RecordingRunner;
    let root = std::path::Path::new(".");
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v9.1.0\ncontainer-v3\ncontainer-v10\nv9.0.0\n");
    assert_eq!(newest_container_version(&r, root).unwrap(), Some(10), "numeric, not lexical");
    let args = &r.calls_for("gh")[0];
    assert!(args.iter().any(|a| a == "repos/AetherBreaker/aeth-devkit/tags"), "{args:?}");
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v9.0.0\n");
    assert_eq!(newest_container_version(&r, root).unwrap(), None);
    assert!(newest_container_version(&RecordingRunner::new(1), root).is_err());
  }
  #[test]
  fn an_unrecognised_pin_is_named_rather_than_swapped_silently() {
    // The whole-file diff would show the swap, but not that devkit failed to read the
    // project's own pin; only a file that fetches the binary at all is worth the note.
    for (text, noted) in [
      ("ADD https://mirror/devkit-container-x86_64-unknown-linux-musl /app/d\n", true),
      ("ADD https://x/releases/download/container-v5/devkit-container /app/d\n", false),
      ("FROM scratch\n", false),
    ] {
      let says = text.contains("devkit-container") && pinned_container_version(text).is_none();
      assert_eq!(says, noted, "{text}");
    }
  }

  #[test]
  fn diff_names_both_sides_and_ignores_crlf_only_drift() {
    let d = unified_diff("docker/Dockerfile", "a\nb\n", "a\nc\n");
    assert!(d.contains("--- docker/Dockerfile (project)"), "{d}");
    assert!(d.contains("+++ docker/Dockerfile (devkit template)"), "{d}");
    assert!(d.contains("-b\n+c\n"), "{d}");
    assert_eq!(normalize_newlines("a\r\nb\r\n"), normalize_newlines("a\nb\n"));
    assert_eq!(normalize_newlines("\u{feff}a\n"), "a\n", "a BOM is not drift either");
    let d = unified_diff("f", "a\r\nb\r\n", "a\nc\n");
    assert!(d.contains("-b\n+c\n") && !d.contains("-a"), "CRLF must not show as drift: {d}");
  }
}
