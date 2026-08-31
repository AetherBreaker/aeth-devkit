//! GitHub queries through the `gh` CLI, plus repository-URL normalization.
//!
//! `gh` is already a hard dependency of `devkit release`; going through it (rather than raw
//! HTTP) picks up the user's authentication, avoids anonymous rate limits, works with
//! private repos, and keeps everything testable through the [`Runner`] trait.

use std::path::Path;

use anyhow::{Result, bail};

use crate::process::Runner;

/// `(host lowercase, "owner/repo" original case)` for the common remote-URL spellings:
/// `https://host/owner/repo(.git)`, `http://…`, `ssh://git@host/owner/repo`, and the scp
/// form `git@host:owner/repo(.git)`. `None` for anything else (including paths that are
/// not exactly two segments deep).
fn host_and_path(url: &str) -> Option<(String, String)> {
  let u = url.trim();
  let rest = if let Some(r) = u
    .strip_prefix("https://")
    .or_else(|| u.strip_prefix("http://"))
    .or_else(|| u.strip_prefix("ssh://"))
  {
    // Drop a `user@` credential part if present.
    r.split_once('@').map_or(r, |(_, tail)| tail).to_string()
  } else if let Some((user_host, path)) = u.split_once(':')
    && user_host.contains('@')
  {
    let host = user_host.split_once('@').map_or(user_host, |(_, h)| h);
    format!("{host}/{path}")
  } else {
    return None;
  };
  let clean = rest.trim_end_matches('/').trim_end_matches(".git");
  let (host, path) = clean.split_once('/')?;
  let segments: Vec<&str> = path.split('/').collect();
  (segments.len() == 2 && segments.iter().all(|s| !s.is_empty())).then(|| (host.to_ascii_lowercase(), path.to_string()))
}

/// Canonical `host/owner/repo` (all lowercase, no `.git`, no trailing slash) for equality
/// comparison between remote URLs, or `None` for something that is not a forge URL.
pub fn normalize_repo(url: &str) -> Option<String> {
  host_and_path(url).map(|(host, path)| format!("{host}/{}", path.to_ascii_lowercase()))
}

/// `Some("owner/repo")` when `url` points at github.com; the case of owner/repo is kept as
/// written because it becomes part of `gh api` paths (GitHub treats them case-insensitively,
/// but round-tripping the user's spelling is friendlier in output).
pub fn github_repo_path(url: &str) -> Option<String> {
  host_and_path(url).and_then(|(host, path)| (host == "github.com").then_some(path))
}

/// Every tag name in the repository, newest-first as GitHub reports them.
/// `--paginate` walks all pages, so there is no 100-tag cap.
pub fn list_tags(runner: &dyn Runner, root: &Path, repo: &str) -> Result<Vec<String>> {
  let args: Vec<String> = ["api", &format!("repos/{repo}/tags"), "--paginate", "--jq", ".[].name"]
    .iter()
    .map(|s| s.to_string())
    .collect();
  let out = runner.run_capture("gh", &args, root)?;
  if !out.success() {
    bail!("gh api repos/{repo}/tags failed: {}", out.stderr.trim());
  }
  Ok(out.stdout.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

/// What `gh release view` prints on stderr for a missing release; any other failure is a
/// real error (auth, network) and must not be read as "no release".
const RELEASE_NOT_FOUND: &str = "release not found";

/// Whether a GitHub release exists for `tag` in the repo `root`'s `origin` points at.
pub fn release_exists(runner: &dyn Runner, root: &Path, tag: &str) -> Result<bool> {
  let args: Vec<String> = ["release", "view", tag].iter().map(|s| s.to_string()).collect();
  let out = runner.run_capture("gh", &args, root)?;
  if out.success() {
    Ok(true)
  } else if out.stderr.to_ascii_lowercase().contains(RELEASE_NOT_FOUND) {
    Ok(false)
  } else {
    bail!("gh release view {tag} failed: {}", out.stderr.trim());
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::process::RecordingRunner;
  use std::path::Path;

  #[test]
  fn normalizes_common_remote_spellings() {
    for u in [
      "https://github.com/Owner/Repo.git",
      "https://github.com/Owner/Repo",
      "https://github.com/Owner/Repo/",
      "git@github.com:Owner/Repo.git",
      "ssh://git@github.com/Owner/Repo",
    ] {
      assert_eq!(normalize_repo(u).as_deref(), Some("github.com/owner/repo"), "{u}");
    }
    assert_eq!(normalize_repo("not a url"), None);
    assert_eq!(normalize_repo("https://github.com/only-owner"), None);
  }

  #[test]
  fn github_repo_path_keeps_case_and_rejects_other_hosts() {
    assert_eq!(github_repo_path("https://github.com/Owner/Repo.git").as_deref(), Some("Owner/Repo"));
    assert_eq!(github_repo_path("git@github.com:o/r").as_deref(), Some("o/r"));
    assert_eq!(github_repo_path("https://gitlab.com/o/r"), None);
  }

  #[test]
  fn lists_tags_through_gh() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v2.0.0\nv1.0.0\n");
    assert_eq!(list_tags(&r, Path::new("."), "o/r").unwrap(), vec!["v2.0.0", "v1.0.0"]);
    assert_eq!(
      r.calls_for("gh")[0],
      vec!["api", "repos/o/r/tags", "--paginate", "--jq", ".[].name"]
    );
    let fail = RecordingRunner::new(1);
    assert!(list_tags(&fail, Path::new("."), "o/r").is_err());
  }

  #[test]
  fn release_exists_distinguishes_missing_from_error() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["release", "view"], 0, "url\n");
    assert!(release_exists(&r, Path::new("."), "v1.0.0").unwrap());
    let missing = RecordingRunner::new(0);
    missing.script_err("gh", &["release", "view"], 1, "release not found");
    assert!(!release_exists(&missing, Path::new("."), "v1.0.0").unwrap());
    let broken = RecordingRunner::new(0);
    broken.script_err("gh", &["release", "view"], 1, "HTTP 401");
    assert!(release_exists(&broken, Path::new("."), "v1.0.0").is_err());
  }
}
