//! Choosing and validating the version to pin.
//!
//! The complete-release rule and version resolution are one computation: every required
//! source (GitHub tags when origin is on GitHub, every publish index when any are
//! configured) must hold the version. "Latest" is the highest stable version common to all
//! sources, and an explicit version must be a member of each — so an interrupted release
//! (tag without index upload, or the reverse) can never be pinned. The GitHub *release*
//! (not just the tag) is verified separately for whichever version wins.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use pep440_rs::Version;

use aeth_devkit_core::github;
use aeth_devkit_core::pyproject::PinIndex;
use aeth_devkit_core::version::{contains, latest_stable_common, parse_lenient};

use crate::Deps;

/// The version that will be pinned, plus the exact tag name the remote spells it with
/// (present whenever GitHub was a source — required for `GIT_TAG` targets).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
  pub version: Version,
  pub tag_spelling: Option<String>,
}

/// Resolve the target version and prove the release is complete.
///
/// `gh_repo` is `Some("owner/repo")` when origin is a GitHub remote; `need_tag` says a
/// `GIT_TAG` target exists, which makes GitHub mandatory rather than optional.
pub fn resolve_version(
  deps: &Deps,
  root: &Path,
  package: &str,
  gh_repo: Option<&str>,
  indexes: &[PinIndex],
  explicit: Option<&str>,
  need_tag: bool,
) -> Result<Resolved> {
  if need_tag && gh_repo.is_none() {
    bail!("a GIT_TAG pin requires the origin remote to be a GitHub repository");
  }
  if gh_repo.is_none() && indexes.is_empty() {
    bail!("nothing to resolve against: origin is not GitHub and no [[tool.uv.index]] has a publish-url");
  }

  // Gather every required source's version list, remembering names for error messages.
  let mut sources: Vec<(String, Vec<String>)> = Vec::new();
  let tags: Option<Vec<String>> = match gh_repo {
    Some(repo) => {
      let t = github::list_tags(deps.runner, root, repo)?;
      sources.push((format!("GitHub tags ({repo})"), t.clone()));
      Some(t)
    }
    None => None,
  };
  for idx in indexes {
    let versions = deps
      .index
      .versions(&idx.url, package)
      .with_context(|| format!("querying index {}", idx.name))?;
    sources.push((format!("index {}", idx.name), versions));
  }

  let version = match explicit {
    Some(s) => {
      let want = parse_lenient(s).with_context(|| format!("{s} is not a valid version"))?;
      let missing: Vec<&str> = sources
        .iter()
        .filter(|(_, list)| !contains(list.iter().map(String::as_str), &want))
        .map(|(name, _)| name.as_str())
        .collect();
      if !missing.is_empty() {
        bail!(
          "version {want} looks like an incomplete release — it is missing from:\n  - {}",
          missing.join("\n  - ")
        );
      }
      want
    }
    None => {
      let lists: Vec<Vec<String>> = sources.iter().map(|(_, l)| l.clone()).collect();
      latest_stable_common(&lists).with_context(|| {
        format!(
          "no stable version is present on every source ({})",
          sources.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ")
        )
      })?
    }
  };

  // The exact remote spelling for GIT_TAG targets (and the release check below).
  let tag_spelling = tags.as_ref().map(|t| {
    t.iter()
      .find(|name| parse_lenient(name).as_ref() == Some(&version))
      .cloned()
      .expect("version was proven to be in the tag list")
  });

  // A tag alone is the signature of an interrupted `devkit release`; require the release.
  if let Some(tag) = &tag_spelling
    && !github::release_exists(deps.runner, root, tag)?
  {
    bail!("tag {tag} exists but has no GitHub release — the release looks incomplete; finish or rescind it first");
  }

  Ok(Resolved { version, tag_spelling })
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::index::StubIndexClient;
  use aeth_devkit_core::process::RecordingRunner;

  fn idx(name: &str) -> PinIndex {
    PinIndex {
      name: name.into(),
      url: format!("https://x/{name}/+simple"),
      publish_url: format!("https://x/{name}/"),
    }
  }

  fn deps<'a>(runner: &'a RecordingRunner, index: &'a StubIndexClient) -> Deps<'a> {
    Deps { runner, index }
  }

  #[test]
  fn latest_is_the_intersection_of_tags_and_index() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v2.0.0\nv1.0.0\n");
    r.script("gh", &["release", "view", "v1.0.0"], 0, "url\n");
    // Index only has 1.0.0 → 2.0.0 is a half-release and must not win.
    let s = StubIndexClient {
      versions: vec!["1.0.0".into()],
    };
    let d = deps(&r, &s);
    let got = resolve_version(&d, Path::new("."), "pkg", Some("o/r"), &[idx("A")], None, true).unwrap();
    assert_eq!(got.version.to_string(), "1.0.0");
    assert_eq!(got.tag_spelling.as_deref(), Some("v1.0.0"));
  }

  #[test]
  fn explicit_version_missing_from_a_source_names_it() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v1.0.0\n");
    let s = StubIndexClient { versions: vec![] };
    let d = deps(&r, &s);
    let err = resolve_version(&d, Path::new("."), "pkg", Some("o/r"), &[idx("A")], Some("1.0.0"), true)
      .unwrap_err()
      .to_string();
    assert!(err.contains("incomplete release") && err.contains("index A"), "{err}");
  }

  #[test]
  fn tag_without_release_is_incomplete() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v1.0.0\n");
    r.script_err("gh", &["release", "view"], 1, "release not found");
    let s = StubIndexClient {
      versions: vec!["1.0.0".into()],
    };
    let d = deps(&r, &s);
    let err = resolve_version(&d, Path::new("."), "pkg", Some("o/r"), &[idx("A")], None, true)
      .unwrap_err()
      .to_string();
    assert!(err.contains("no GitHub release"), "{err}");
  }

  #[test]
  fn pypi_only_project_skips_github_entirely() {
    let r = RecordingRunner::new(1); // any gh call would fail loudly
    let s = StubIndexClient {
      versions: vec!["1.0.0".into(), "2.0.0".into(), "3.0.0a1".into()],
    };
    let d = deps(&r, &s);
    let got = resolve_version(&d, Path::new("."), "pkg", None, &[idx("A")], None, false).unwrap();
    assert_eq!(got.version.to_string(), "2.0.0");
    assert_eq!(got.tag_spelling, None);
    assert!(r.calls_for("gh").is_empty());
  }

  #[test]
  fn explicit_prerelease_matches_across_spellings() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v1.2.0-alpha1\n");
    r.script("gh", &["release", "view", "v1.2.0-alpha1"], 0, "url\n");
    let s = StubIndexClient {
      versions: vec!["1.2.0a1".into()],
    };
    let d = deps(&r, &s);
    let got = resolve_version(&d, Path::new("."), "pkg", Some("o/r"), &[idx("A")], Some("1.2.0a1"), true).unwrap();
    assert_eq!(got.version.to_string(), "1.2.0a1");
    assert_eq!(got.tag_spelling.as_deref(), Some("v1.2.0-alpha1"), "remote spelling kept");
  }

  #[test]
  fn git_target_without_github_origin_is_an_error() {
    let r = RecordingRunner::new(0);
    let s = StubIndexClient { versions: vec![] };
    let d = deps(&r, &s);
    assert!(resolve_version(&d, Path::new("."), "pkg", None, &[], None, true).is_err());
  }
}
