//! Looking up published versions on a PEP 503 / PEP 691 simple index.

use anyhow::{Context as _, Result, bail};
use regex::Regex;
use std::sync::LazyLock;

use crate::pyproject::normalize_dist_name;

pub const PEP691_JSON: &str = "application/vnd.pypi.simple.v1+json";

/// Something that can list the versions published for a package.
pub trait IndexClient {
  /// Every version the index lists for `package`. A project page that does not exist yet
  /// (PyPI before the first upload, devpi for a never-published name) is an empty list,
  /// not an error — a first release must be able to pass the "already exists?" probe.
  fn versions(&self, simple_url: &str, package: &str) -> Result<Vec<String>>;
}

/// `<simple_url>/<normalized package>/`.
pub fn project_url(simple_url: &str, package: &str) -> String {
  format!("{}/{}/", simple_url.trim_end_matches('/'), normalize_dist_name(package))
}

/// Versions found in wheel (`name-ver-….whl`) and sdist (`name-ver.tar.gz` / `.zip`) file
/// names belonging to `package`. Order follows the input; duplicates are kept.
pub fn versions_from_filenames<'a>(package: &str, filenames: impl IntoIterator<Item = &'a str>) -> Vec<String> {
  let want = normalize_dist_name(package);
  let mut out = Vec::new();
  for f in filenames {
    let version = if let Some(s) = f.strip_suffix(".whl") {
      // name-version(-build)?-python-abi-platform
      let mut parts = s.splitn(3, '-');
      let (Some(name), Some(ver)) = (parts.next(), parts.next()) else {
        continue;
      };
      (normalize_dist_name(name) == want).then(|| ver.to_string())
    } else if let Some(s) = f.strip_suffix(".tar.gz").or_else(|| f.strip_suffix(".zip")) {
      // sdist names may use '-' inside the project name; the version follows the last '-'
      // whose prefix normalizes to the package name.
      s.rmatch_indices('-')
        .find(|(i, _)| normalize_dist_name(&s[..*i]) == want)
        .map(|(i, _)| s[i + 1..].to_string())
    } else {
      None
    };
    if let Some(v) = version {
      out.push(v);
    }
  }
  out
}

/// PEP 691 JSON project page: use `versions` when present, else derive from `files`.
pub fn parse_simple_json(body: &str, package: &str) -> Result<Vec<String>> {
  let v: serde_json::Value = serde_json::from_str(body).context("parsing simple-index JSON")?;
  if let Some(arr) = v.get("versions").and_then(|x| x.as_array()) {
    return Ok(arr.iter().filter_map(|x| x.as_str().map(str::to_string)).collect());
  }
  let files = v
    .get("files")
    .and_then(|x| x.as_array())
    .map(|a| {
      a.iter()
        .filter_map(|f| f.get("filename").and_then(|n| n.as_str()))
        .collect::<Vec<_>>()
    })
    .unwrap_or_default();
  Ok(versions_from_filenames(package, files))
}

static ANCHOR_TEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<a\b[^>]*>(.*?)</a>").unwrap());

/// PEP 503 HTML project page: the text of each `<a>` is a file name.
pub fn parse_simple_html(body: &str, package: &str) -> Vec<String> {
  let names: Vec<&str> = ANCHOR_TEXT.captures_iter(body).map(|c| c.get(1).unwrap().as_str().trim()).collect();
  versions_from_filenames(package, names)
}

/// Fetches the project page over HTTP, preferring PEP 691 JSON and falling back to HTML.
/// `timeout` bounds the whole request; `None` means ureq's default (no limit).
#[derive(Default)]
pub struct HttpIndexClient {
  pub timeout: Option<std::time::Duration>,
}

impl HttpIndexClient {
  pub fn with_timeout(timeout: std::time::Duration) -> Self {
    Self { timeout: Some(timeout) }
  }
}

impl IndexClient for HttpIndexClient {
  fn versions(&self, simple_url: &str, package: &str) -> Result<Vec<String>> {
    let url = project_url(simple_url, package);
    // 4xx/5xx come back as responses (not errors) so the 404 below can be told apart.
    let agent: ureq::Agent = ureq::Agent::config_builder()
      .timeout_global(self.timeout)
      .http_status_as_error(false)
      .build()
      .into();
    let mut resp = agent
      .get(&url)
      .header("Accept", &format!("{PEP691_JSON}, text/html;q=0.1"))
      .call()
      .with_context(|| format!("fetching {url}"))?;
    match resp.status().as_u16() {
      200 => {}
      404 => return Ok(Vec::new()),
      s => bail!("unexpected HTTP {s} from GET {url}"),
    }
    let content_type = resp
      .headers()
      .get("content-type")
      .and_then(|v| v.to_str().ok())
      .unwrap_or("")
      .to_ascii_lowercase();
    let body = resp.body_mut().read_to_string().with_context(|| format!("reading {url}"))?;
    if content_type.contains("json") {
      parse_simple_json(&body, package)
    } else {
      Ok(parse_simple_html(&body, package))
    }
  }
}

/// Returns a fixed list; for tests.
pub struct StubIndexClient {
  pub versions: Vec<String>,
}

impl IndexClient for StubIndexClient {
  fn versions(&self, _simple_url: &str, _package: &str) -> Result<Vec<String>> {
    Ok(self.versions.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn builds_project_url() {
    assert_eq!(project_url("https://x/+simple", "Aeth_DevKit"), "https://x/+simple/aeth-devkit/");
    assert_eq!(project_url("https://x/simple/", "a"), "https://x/simple/a/");
  }

  #[test]
  fn extracts_versions_from_wheel_and_sdist_names() {
    let files = [
      "aeth_devkit-6.0.2-py3-none-any.whl",
      "aeth_devkit-6.0.2.tar.gz",
      "aeth_devkit-7.0.0a1-cp314-cp314-win_amd64.whl",
      "aeth-devkit-5.1.0.zip",
      "other_pkg-1.0.0.tar.gz",
      "README.txt",
    ];
    let mut v = versions_from_filenames("aeth-devkit", files);
    v.sort();
    v.dedup();
    assert_eq!(v, vec!["5.1.0", "6.0.2", "7.0.0a1"]);
  }

  #[test]
  fn parses_pep691_json() {
    let body =
      r#"{"name":"aeth-devkit","versions":["6.0.2","6.1.0"],"files":[{"filename":"aeth_devkit-6.0.2-py3-none-any.whl","url":"x"}]}"#;
    let mut v = parse_simple_json(body, "aeth-devkit").unwrap();
    v.sort();
    v.dedup();
    assert_eq!(v, vec!["6.0.2", "6.1.0"]);
    let body =
      r#"{"name":"aeth-devkit","files":[{"filename":"aeth_devkit-6.0.2.tar.gz"},{"filename":"aeth_devkit-6.0.3-py3-none-any.whl"}]}"#;
    assert_eq!(parse_simple_json(body, "aeth-devkit").unwrap(), vec!["6.0.2", "6.0.3"]);
  }

  #[test]
  fn parses_simple_html() {
    let body = r#"<html><body><h1>Links for aeth-devkit</h1>
<a href="../../packages/aeth_devkit-6.0.2-py3-none-any.whl#sha256=abc">aeth_devkit-6.0.2-py3-none-any.whl</a><br/>
<a href="../../packages/aeth_devkit-6.1.0.tar.gz">aeth_devkit-6.1.0.tar.gz</a>
</body></html>"#;
    assert_eq!(parse_simple_html(body, "aeth-devkit"), vec!["6.0.2", "6.1.0"]);
  }

  #[test]
  fn stub_client_returns_versions() {
    let c = StubIndexClient {
      versions: vec!["1.0".into()],
    };
    assert_eq!(c.versions("u", "p").unwrap(), vec!["1.0"]);
  }
}
