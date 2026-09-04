//! Getting a compatible extension into VS Code: the newest `vscode-extension-vN` release
//! is fetched from GitHub (no auth: the repo is public and this runs once per install)
//! and handed to `code --install-extension`. A fresh install is live at once; an upgrade
//! over a loaded extension needs a window reload, which the caller reports and stops on.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;

use super::protocol::{EXTENSION_ID, MIN_EXTENSION_VERSION};

pub const REPO: &str = "AetherBreaker/aeth-devkit";
pub const TAG_PREFIX: &str = "vscode-extension-v";

pub fn refs_url() -> String {
  format!("https://api.github.com/repos/{REPO}/git/matching-refs/tags/{TAG_PREFIX}")
}

pub fn vsix_url(n: u32) -> String {
  format!("https://github.com/{REPO}/releases/download/{TAG_PREFIX}{n}/aeth-devkit-vscode-{n}.vsix")
}

/// Two HTTP verbs behind a trait so the install flow is testable without a network.
pub trait Fetch {
  fn get_text(&self, url: &str) -> Result<String>;
  fn download(&self, url: &str, dest: &Path) -> Result<()>;
}

pub struct HttpFetch;

impl HttpFetch {
  fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
      .timeout_global(Some(std::time::Duration::from_secs(60)))
      .http_status_as_error(false)
      .build()
      .into()
  }
}

impl Fetch for HttpFetch {
  fn get_text(&self, url: &str) -> Result<String> {
    // GitHub's API rejects requests without a User-Agent.
    let mut resp = Self::agent()
      .get(url)
      .header("User-Agent", "aeth-devkit")
      .header("Accept", "application/vnd.github+json")
      .call()
      .with_context(|| format!("fetching {url}"))?;
    if resp.status().as_u16() != 200 {
      bail!("HTTP {} from GET {url}", resp.status());
    }
    resp.body_mut().read_to_string().with_context(|| format!("reading {url}"))
  }

  fn download(&self, url: &str, dest: &Path) -> Result<()> {
    // Release assets redirect to a storage host; ureq follows redirects by default.
    let mut resp = Self::agent()
      .get(url)
      .header("User-Agent", "aeth-devkit")
      .call()
      .with_context(|| format!("downloading {url}"))?;
    if resp.status().as_u16() != 200 {
      bail!("HTTP {} from GET {url}", resp.status());
    }
    // ureq caps body reads at 10 MB by default; a bundled extension can pass that.
    let bytes = resp
      .body_mut()
      .with_config()
      .limit(256 * 1024 * 1024)
      .read_to_vec()
      .with_context(|| format!("reading {url}"))?;
    if let Some(parent) = dest.parent() {
      std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(dest, bytes).with_context(|| format!("writing {}", dest.display()))
  }
}

/// Canned bodies per URL; downloads write a stub file and are recorded. For tests.
#[derive(Default)]
pub struct StubFetch {
  pub bodies: HashMap<String, String>,
  pub downloads: RefCell<Vec<(String, PathBuf)>>,
}

impl Fetch for StubFetch {
  fn get_text(&self, url: &str) -> Result<String> {
    self.bodies.get(url).cloned().ok_or_else(|| anyhow::anyhow!("stub: no body for {url}"))
  }

  fn download(&self, url: &str, dest: &Path) -> Result<()> {
    self.downloads.borrow_mut().push((url.to_string(), dest.to_path_buf()));
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(dest, b"vsix")?;
    Ok(())
  }
}

/// The highest `N` among `refs/tags/vscode-extension-vN` in a matching-refs response.
pub fn latest_tag_number(refs_json: &str) -> Result<Option<u32>> {
  let refs: Vec<serde_json::Value> = serde_json::from_str(refs_json).context("parsing the extension tag list")?;
  Ok(
    refs
      .iter()
      .filter_map(|r| r["ref"].as_str())
      .filter_map(|r| r.strip_prefix("refs/tags/")?.strip_prefix(TAG_PREFIX))
      .filter_map(|n| n.parse().ok())
      .max(),
  )
}

/// `N` from the `aeth.aeth-devkit@N.0.0` line of `code --list-extensions --show-versions`.
pub fn installed_version(list_output: &str) -> Option<u32> {
  list_output.lines().find_map(|l| {
    let (id, ver) = l.trim().split_once('@')?;
    if !id.eq_ignore_ascii_case(EXTENSION_ID) {
      return None;
    }
    ver.split('.').next()?.parse().ok()
  })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ensure {
  /// A compatible extension is installed and loaded.
  Ready,
  /// A newer extension was just installed over a loaded one; VS Code must reload first.
  ReloadNeeded,
  /// No compatible extension and none could be installed; the note says why.
  Unavailable(String),
}

/// Make sure a compatible extension is installed, installing the newest release when
/// `install` is set. Never fails: every problem becomes `Unavailable`.
pub fn ensure_extension(runner: &dyn Runner, fetch: &dyn Fetch, launcher: &Path, cache: &Path, install: bool) -> Ensure {
  match ensure(runner, fetch, launcher, cache, install) {
    Ok(e) => e,
    Err(e) => Ensure::Unavailable(format!("{e:#}")),
  }
}

fn ensure(runner: &dyn Runner, fetch: &dyn Fetch, launcher: &Path, cache: &Path, install: bool) -> Result<Ensure> {
  let code = launcher.to_string_lossy();
  let out = runner.run_capture(&code, &["--list-extensions".into(), "--show-versions".into()], Path::new("."))?;
  if !out.success() {
    bail!("`code --list-extensions` failed: {}", out.stderr.trim());
  }
  let installed = installed_version(&out.stdout);
  if installed.is_some_and(|n| n >= MIN_EXTENSION_VERSION) {
    return Ok(Ensure::Ready);
  }
  if !install {
    return Ok(Ensure::Unavailable(
      "the devkit VS Code extension is not installed (a run without --dry-run installs it)".into(),
    ));
  }
  let latest = latest_tag_number(&fetch.get_text(&refs_url())?)?;
  let Some(n) = latest.filter(|n| *n >= MIN_EXTENSION_VERSION) else {
    bail!("no compatible devkit VS Code extension release exists yet (need build {MIN_EXTENSION_VERSION})");
  };
  let vsix = cache.join("vsix").join(format!("aeth-devkit-vscode-{n}.vsix"));
  fetch.download(&vsix_url(n), &vsix)?;
  let args: Vec<String> = vec!["--install-extension".into(), vsix.to_string_lossy().into_owned(), "--force".into()];
  let out = runner.run_capture(&code, &args, Path::new("."))?;
  if !out.success() {
    bail!("`code --install-extension` failed: {}", out.stderr.trim());
  }
  Ok(if installed.is_some() { Ensure::ReloadNeeded } else { Ensure::Ready })
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;

  const LIST: &[&str] = &["--list-extensions"];
  const REFS: &str = r#"[{"ref":"refs/tags/vscode-extension-v1"},{"ref":"refs/tags/vscode-extension-v3"},{"ref":"refs/tags/vscode-extension-v2"},{"ref":"refs/tags/vscode-extension-vX"}]"#;

  fn fetch_with_refs() -> StubFetch {
    let mut f = StubFetch::default();
    f.bodies.insert(refs_url(), REFS.into());
    f
  }

  #[test]
  fn parses_installed_version_and_tag_numbers() {
    assert_eq!(installed_version("ms-python.python@2024.1.0\nAeth.aeth-devkit@3.0.0\n"), Some(3));
    assert_eq!(installed_version("ms-python.python@2024.1.0\n"), None);
    assert_eq!(latest_tag_number(REFS).unwrap(), Some(3));
    assert_eq!(latest_tag_number("[]").unwrap(), None);
    assert!(latest_tag_number("nope").is_err());
    assert_eq!(
      vsix_url(3),
      "https://github.com/AetherBreaker/aeth-devkit/releases/download/vscode-extension-v3/aeth-devkit-vscode-3.vsix"
    );
  }

  #[test]
  fn ready_when_a_compatible_extension_is_installed() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "aeth.aeth-devkit@1.0.0\n");
    let f = StubFetch::default();
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(ensure_extension(&r, &f, Path::new("code"), cache.path(), true), Ensure::Ready);
    assert_eq!(r.calls_for("code").len(), 1, "no install");
    assert!(f.downloads.borrow().is_empty());
  }

  #[test]
  fn installs_the_newest_release_when_absent() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "ms-python.python@2024.1.0\n");
    let f = fetch_with_refs();
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(ensure_extension(&r, &f, Path::new("code"), cache.path(), true), Ensure::Ready);
    let vsix = cache.path().join("vsix").join("aeth-devkit-vscode-3.vsix");
    assert_eq!(f.downloads.borrow()[0], (vsix_url(3), vsix.clone()));
    assert!(vsix.is_file());
    let calls = r.calls_for("code");
    assert_eq!(calls[1], vec!["--install-extension", &vsix.to_string_lossy().into_owned(), "--force"]);
  }

  #[test]
  fn an_upgrade_over_a_loaded_extension_needs_a_reload() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "aeth.aeth-devkit@0.0.0\n");
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(
      ensure_extension(&r, &fetch_with_refs(), Path::new("code"), cache.path(), true),
      Ensure::ReloadNeeded
    );
  }

  #[test]
  fn dry_run_never_installs_and_failures_are_unavailable() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "");
    let f = fetch_with_refs();
    let cache = tempfile::tempdir().unwrap();
    assert!(matches!(
      ensure_extension(&r, &f, Path::new("code"), cache.path(), false),
      Ensure::Unavailable(m) if m.contains("not installed")
    ));
    assert!(f.downloads.borrow().is_empty());

    let offline = StubFetch::default();
    assert!(matches!(
      ensure_extension(&r, &offline, Path::new("code"), cache.path(), true),
      Ensure::Unavailable(m) if m.contains("no body")
    ));

    let mut old = StubFetch::default();
    old.bodies.insert(refs_url(), r#"[{"ref":"refs/tags/vscode-extension-v0"}]"#.into());
    assert!(matches!(
      ensure_extension(&r, &old, Path::new("code"), cache.path(), true),
      Ensure::Unavailable(m) if m.contains("no compatible")
    ));

    let failing = RecordingRunner::new(0);
    failing.script("code", LIST, 0, "");
    failing.script_err("code", &["--install-extension"], 1, "boom");
    assert!(matches!(
      ensure_extension(&failing, &f, Path::new("code"), cache.path(), true),
      Ensure::Unavailable(m) if m.contains("boom")
    ));
  }
}
