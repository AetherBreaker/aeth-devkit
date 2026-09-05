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

/// The files under `docker/` this step owns, by target name (the compose file has its own
/// rule-based flow). One list, so `git::committable` stages exactly what is written here:
/// a file added to the templates but not to this list would be replaced without the
/// HEAD-reset every other managed file gets, then left out of the commit.
pub const TARGETS: &[&str] = &["Dockerfile"];

/// The repository whose `container-v<N>` releases ship the entrypoint binary the
/// Dockerfile fetches; the tag stream is devkit's, whatever project is being set up.
pub const DEVKIT_REPO: &str = "AetherBreaker/aeth-devkit";

/// The `N` of a `container-v<N>` pin in a Dockerfile's download URL, if it has one.
pub fn pinned_container_version(dockerfile: &str) -> Option<u64> {
  let re = regex::Regex::new(r"releases/download/container-v(\d+)/").expect("static regex");
  re.captures(dockerfile).and_then(|c| c[1].parse().ok())
}

/// `{container_version}` for a Dockerfile without a pin: the newest `container-v*` tag on
/// devkit's repository, or provisionally `1` (the tag the first container release makes)
/// with a note when the tags cannot be read. An existing pin never reaches here: it is
/// kept as is, and advancing it is a separate command's job (see TODO.md), so a routine
/// run never talks to `gh` for the Dockerfile.
fn resolve_container_version(runner: &dyn Runner, root: &Path, notes: &mut Vec<String>) -> String {
  let fall_back = |why: String, notes: &mut Vec<String>| {
    notes.push(format!(
      "devkit-container pinned to container-v1 provisionally ({why}); the next devkit release creates that tag."
    ));
    "1".to_string()
  };
  let tags = match github::list_tags(runner, root, DEVKIT_REPO) {
    Ok(t) => t,
    Err(e) => return fall_back(format!("{e:#}"), notes),
  };
  match tags.iter().filter_map(|t| t.strip_prefix("container-v")?.parse::<u64>().ok()).max() {
    Some(n) => n.to_string(),
    None => fall_back("no container-v* tag on the devkit repository yet".into(), notes),
  }
}

pub fn normalize_newlines(s: &str) -> String {
  s.replace("\r\n", "\n")
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
    if rendered.contains("{container_version}") {
      let version = match original.as_deref().and_then(pinned_container_version) {
        Some(n) => n.to_string(),
        None => resolve_container_version(runner, &ctx.root, &mut changes.notes),
      };
      rendered = rendered.replace("{container_version}", &version);
    }
    let Some(original) = original else {
      changes.record_optional(&path, None, &rendered, vec!["created from template".into()])?;
      continue;
    };
    if normalize_newlines(&original) == normalize_newlines(&rendered) {
      // Managed, unchanged. CRLF-only drift is not drift: .gitattributes owns line endings.
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      continue;
    }
    println!("{}", unified_diff(&rel, &original, &rendered));
    if consent.replace(&format!("Replace {rel}? [replace / replace all / anything else keeps it]:"))? {
      changes.record_optional(&path, Some(&original), &rendered, vec!["replaced with the devkit template".into()])?;
    } else {
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      println!("Kept {rel}.");
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
  fn the_pin_is_read_from_the_download_url_only() {
    assert_eq!(
      pinned_container_version("ADD https://x/releases/download/container-v12/devkit-container-x86_64-unknown-linux-musl /app/d\n"),
      Some(12)
    );
    assert_eq!(
      pinned_container_version("ADD https://x/releases/download/v9.0.0/devkit-container /app/d\n"),
      None
    );
    assert_eq!(pinned_container_version("# container-v3 in a comment is not a pin\n"), None);
  }

  #[test]
  fn a_missing_pin_takes_the_newest_container_tag_or_one_provisionally() {
    use aeth_devkit_core::process::RecordingRunner;
    let root = std::path::Path::new(".");
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v9.1.0\ncontainer-v3\ncontainer-v10\nv9.0.0\n");
    let mut notes = Vec::new();
    assert_eq!(resolve_container_version(&r, root, &mut notes), "10", "numeric, not lexical");
    assert!(notes.is_empty());
    let args = &r.calls_for("gh")[0];
    assert!(args.iter().any(|a| a == "repos/AetherBreaker/aeth-devkit/tags"), "{args:?}");
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v9.0.0\n");
    assert_eq!(resolve_container_version(&r, root, &mut notes), "1");
    assert!(
      notes[0].contains("provisionally") && notes[0].contains("no container-v* tag"),
      "{notes:?}"
    );
    let r = RecordingRunner::new(1);
    notes.clear();
    assert_eq!(resolve_container_version(&r, root, &mut notes), "1");
    assert!(notes[0].contains("gh api"), "{notes:?}");
  }

  #[test]
  fn diff_names_both_sides_and_ignores_crlf_only_drift() {
    let d = unified_diff("docker/Dockerfile", "a\nb\n", "a\nc\n");
    assert!(d.contains("--- docker/Dockerfile (project)"), "{d}");
    assert!(d.contains("+++ docker/Dockerfile (devkit template)"), "{d}");
    assert!(d.contains("-b\n+c\n"), "{d}");
    assert_eq!(normalize_newlines("a\r\nb\r\n"), normalize_newlines("a\nb\n"));
    let d = unified_diff("f", "a\r\nb\r\n", "a\nc\n");
    assert!(d.contains("-b\n+c\n") && !d.contains("-a"), "CRLF must not show as drift: {d}");
  }
}
