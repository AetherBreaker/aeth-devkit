//! Whole-file replacement of `docker/Dockerfile`, rendered from the template inside the
//! installed `devkit_container` package, shown as a diff and applied only on consent.
//! Leftovers of the old shell entrypoint are reported, never deleted.

use anyhow::{Context as _, Result};
use similar::TextDiff;

use crate::changes::Changes;
use crate::context::ProjectContext;
use crate::docker::Consent;
use crate::packages::Venv;
use crate::templates;
use crate::vscode::protocol::Proposal;

/// The files under `docker/` this step owns, by target name (the compose file has its own
/// rule-based flow). One list, so `git::committable` stages exactly what is written here:
/// a file rendered here but not on this list would be replaced without the HEAD-reset
/// every other managed file gets, then left out of the commit.
pub const TARGETS: &[&str] = &["Dockerfile"];

/// The template's file name inside the installed `devkit_container` package.
pub const TEMPLATE_FILE: &str = "template.Dockerfile";

/// The Dockerfile as the installed devkit-container renders it for this project, or `None`
/// when the package is not in the venv. The version rendered is the version the image will
/// install, because both come from the same locked package.
pub fn render(ctx: &ProjectContext, venv: &dyn Venv) -> Result<Option<String>> {
  let Some(installed) = venv.installed(&ctx.root, &crate::packages::CONTAINER) else {
    return Ok(None);
  };
  let path = installed.dir.join(TEMPLATE_FILE);
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
  Ok(Some(templates::substitute(&text, ctx, templates::Escape::None)))
}

/// The text as diffed and as VS Code shows it: LF line endings, no byte-order mark. Neither
/// is drift (`.gitattributes` owns line endings, and Windows editors add a BOM the
/// templates never carry), and a BOM would otherwise show as a phantom hunk on the first
/// line. What gets written keeps the file's own endings (see `Proposal`).
pub fn normalize_newlines(s: &str) -> String {
  s.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n")
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

pub fn apply(ctx: &ProjectContext, venv: &dyn Venv, consent: &Consent, changes: &mut Changes) -> Result<()> {
  for target in TARGETS {
    let rel = format!("docker/{target}");
    let path = ctx.root.join("docker").join(target);
    let original = crate::read_optional(&path)?;
    // On a plain run the package step has already installed the package, so `None` here
    // means a dry run on a project that has not adopted it yet.
    let Some(rendered) = render(ctx, venv)? else {
      changes.notes.push(format!(
        "{rel} was not rendered: devkit-container is not installed in this venv yet; a plain run installs it and renders the file."
      ));
      if let Some(original) = &original {
        changes.record_optional(&path, Some(original), original, vec![])?;
      }
      continue;
    };
    let Some(original) = original else {
      changes.record_optional(&path, None, &rendered, vec!["created from template".into()])?;
      continue;
    };
    if normalize_newlines(&original) == normalize_newlines(&rendered) {
      // Managed, unchanged. CRLF-only drift is not drift: .gitattributes owns line endings.
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      continue;
    }
    // Written in the file's own line endings (the template is LF).
    let rendered = if original.contains("\r\n") && !rendered.contains("\r\n") {
      rendered.replace('\n', "\r\n")
    } else {
      rendered
    };
    println!("{}", unified_diff(&rel, &original, &rendered));
    let proposal = Proposal::new(&rel, format!("Replace {rel}?"), &original, &rendered);
    let decision = consent.decide(&proposal, true)?;
    let detail = decision.detail("replaced with the devkit template");
    match decision.text(&proposal) {
      Some(text) => changes.record_optional(&path, Some(&original), &text, vec![detail])?,
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
  fn render_substitutes_python_dir_from_the_installed_package() {
    let site = tempfile::tempdir().unwrap();
    let pkg = site.path().join("devkit_container");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join(TEMPLATE_FILE), "RUN mv /tmp/repo/{python_dir} /app/{python_dir}\n").unwrap();
    let mut map = std::collections::HashMap::new();
    map.insert(
      "devkit_container".to_string(),
      crate::packages::Installed {
        dir: pkg,
        version: "1.4.0".into(),
      },
    );
    let venv = crate::packages::StubVenv(map);
    let ctx = ProjectContext {
      root: std::path::PathBuf::from("/p"),
      package: "proj".into(),
      dependencies: Default::default(),
      has_docker: true,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec!["proj".into()],
      docker_legacy_keys: vec![],
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "python".into(),
      has_rust: true,
      publish_index: None,
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
    };
    assert_eq!(render(&ctx, &venv).unwrap().unwrap(), "RUN mv /tmp/repo/python /app/python\n");
    assert_eq!(render(&ctx, &crate::packages::StubVenv::default()).unwrap(), None);
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
