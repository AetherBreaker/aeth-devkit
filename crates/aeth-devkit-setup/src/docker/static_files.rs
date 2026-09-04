//! Whole-file replacement of the templated `docker/` files (everything except the compose
//! file), shown as a diff and applied only on consent. Files the template stopped
//! shipping are reported, never deleted.

use std::path::Path;

use anyhow::{Context as _, Result};
use similar::TextDiff;

use crate::changes::Changes;
use crate::context::ProjectContext;
use crate::docker::Consent;
use crate::templates;

/// Target file name for a template file name, or `None` for files this step does not own:
/// `template.Dockerfile` → `Dockerfile`, `entrypoint.template.sh` → `entrypoint.sh`;
/// the compose file (`compose.template.yaml`) has its own rule-based flow.
pub fn target_name(template_file: &str) -> Option<String> {
  if template_file == "compose.template.yaml" {
    return None;
  }
  if let Some(rest) = template_file.strip_prefix("template.") {
    return Some(rest.to_string());
  }
  // `x.template.ext` → `x.ext`: split at the *last* `.template.` so a stem containing
  // the word keeps it.
  let (stem, ext) = template_file.rsplit_once(".template.")?;
  Some(format!("{stem}.{ext}"))
}

pub fn normalize_newlines(s: &str) -> String {
  s.replace("\r\n", "\n")
}

/// Three lines of context, both sides labelled so the user can tell which is theirs.
pub fn unified_diff(rel: &str, old: &str, new: &str) -> String {
  TextDiff::from_lines(old, new)
    .unified_diff()
    .context_radius(3)
    .header(&format!("{rel} (project)"), &format!("{rel} (devkit template)"))
    .to_string()
}

pub fn apply(ctx: &ProjectContext, templates_dir: &Path, consent: &Consent, changes: &mut Changes) -> Result<()> {
  let dir = templates_dir.join("docker");
  let mut targets: Vec<String> = std::fs::read_dir(&dir)
    .with_context(|| format!("reading {}", dir.display()))?
    .filter_map(|e| e.ok())
    .filter(|e| e.path().is_file())
    .filter_map(|e| target_name(&e.file_name().to_string_lossy()))
    .collect();
  targets.sort(); // deterministic prompt order
  for target in targets {
    let rel = format!("docker/{target}");
    let rendered = templates::load(templates_dir, &rel, ctx, templates::Escape::None)?;
    let path = ctx.root.join("docker").join(&target);
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
  fn template_names_map_back_to_targets() {
    assert_eq!(target_name("template.Dockerfile").as_deref(), Some("Dockerfile"));
    assert_eq!(target_name("entrypoint.template.sh").as_deref(), Some("entrypoint.sh"));
    assert_eq!(target_name("compose.template.yaml"), None, "the compose file has its own flow");
    assert_eq!(target_name("README.md"), None);
  }

  #[test]
  fn diff_names_both_sides_and_ignores_crlf_only_drift() {
    let d = unified_diff("docker/Dockerfile", "a\nb\n", "a\nc\n");
    assert!(d.contains("--- docker/Dockerfile (project)"), "{d}");
    assert!(d.contains("+++ docker/Dockerfile (devkit template)"), "{d}");
    assert!(d.contains("-b\n+c\n"), "{d}");
    assert_eq!(normalize_newlines("a\r\nb\r\n"), normalize_newlines("a\nb\n"));
  }
}
