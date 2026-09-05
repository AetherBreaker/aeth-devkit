//! Whole-file replacement of the templated `docker/` files (everything except the compose
//! file), shown as a diff and applied only on consent. Files the template stopped
//! shipping are reported, never deleted.

use std::path::Path;

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

pub fn apply(ctx: &ProjectContext, templates_dir: &Path, consent: &Consent, changes: &mut Changes) -> Result<()> {
  for target in TARGETS {
    let rel = format!("docker/{target}");
    let rendered = templates::load(templates_dir, &rel, ctx, templates::Escape::None)?;
    let path = ctx.root.join("docker").join(target);
    let Some(original) = crate::read_optional(&path)? else {
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
