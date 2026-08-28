//! Talking to a devpi index's REST API: does `<package>/<version>` exist, and delete it.
//!
//! devpi exposes each release at `<index-root>/<package>/<version>`. A `GET` there answers
//! 200 when the version exists and 404 when it does not; a `DELETE` removes it (200/204) or
//! reports 404 if it was already gone. That is the whole protocol this module speaks.
//!
//! As with [`crate::process::Runner`], the network side hides behind a trait so the release
//! command can be tested without an index: [`HttpDevpiClient`] does real HTTP, while
//! [`StubDevpiClient`] answers from a flag and records what it was asked.

// Interior mutability again (see `process.rs`): the stub's trait methods take `&self`, yet
// it must remember calls and flip its "exists" flag when asked to delete.
use std::cell::{Cell, RefCell};

// `bail!` is anyhow's "return Err(anyhow!(…)) right now" macro — handy for early exits.
use anyhow::{Context as _, Result, bail};
// The `Engine` trait provides `.encode()` on base64 engines; imported anonymously because we
// only need its methods, not its name.
use base64::Engine as _;

/// What happened when we asked devpi to delete a version.
///
/// A two-variant enum is clearer than a `bool` at the call site: `DeleteOutcome::NotFound`
/// reads unambiguously where `false` would not. `Copy` is fine because it carries no data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeleteOutcome {
  Deleted,
  NotFound,
}

/// Something that can query and delete versions on a devpi index.
pub trait DevpiClient {
  /// `true` for HTTP 200, `false` for 404, `Err` for anything else (auth failures, outages).
  fn exists(&self, url: &str, username: &str, password: &str) -> Result<bool>;
  /// `Deleted` for 200/204, `NotFound` for 404, `Err` otherwise.
  fn delete(&self, url: &str, username: &str, password: &str) -> Result<DeleteOutcome>;
  /// The release files the index stores for a version, as `(filename, download URL)`
  /// pairs; `None` when the version does not exist. Existence alone cannot prove *who*
  /// uploaded a version — this is what lets a caller compare the stored artifacts with
  /// its own before deleting anything.
  ///
  /// The default implementation (used by simple stubs) answers from [`exists`](Self::exists)
  /// with an empty file list, which callers treat as "no evidence against ownership".
  fn files(&self, url: &str, username: &str, password: &str) -> Result<Option<Vec<(String, String)>>> {
    // `bool::then` maps `true` → `Some(value)` and `false` → `None` in one step.
    Ok(self.exists(url, username, password)?.then(Vec::new))
  }
  /// The raw bytes of one stored file (an `href` from [`files`](Self::files)).
  fn fetch(&self, href: &str, username: &str, password: &str) -> Result<Vec<u8>> {
    // Stubs that never return files from `files` never reach this; a real client overrides.
    let _ = (username, password);
    bail!("fetching {href} is not supported by this client")
  }
}

/// Parse a devpi version page (JSON form) into `(filename, href)` pairs. devpi answers
/// `GET <index>/<package>/<version>` with `{"result": {"+links": [{"rel": "releasefile",
/// "href": …}, …]}}` when asked for `application/json`. Split out as a pure function so it
/// can be unit-tested without a server.
pub fn parse_version_links(body: &str) -> Result<Vec<(String, String)>> {
  let v: serde_json::Value = serde_json::from_str(body).context("parsing devpi version JSON")?;
  // Each `and_then` peels one layer; any missing layer yields an empty iterator below.
  let links = v.get("result").and_then(|r| r.get("+links")).and_then(|l| l.as_array());
  let mut out = Vec::new();
  // `into_iter().flatten()` iterates the array when present and nothing when `None`.
  for link in links.into_iter().flatten() {
    // Only actual release files; devpi also lists e.g. `doczip` links under other `rel`s.
    if link.get("rel").and_then(|r| r.as_str()) != Some("releasefile") {
      continue;
    }
    let Some(href) = link.get("href").and_then(|h| h.as_str()) else {
      continue;
    };
    // The filename is the last path segment, with any `#hash=…` fragment stripped.
    let clean = href.split('#').next().unwrap_or(href);
    let name = clean.rsplit('/').next().unwrap_or(clean).to_string();
    out.push((name, clean.to_string()));
  }
  Ok(out)
}

/// The value of an HTTP `Authorization` header for basic auth: `Basic base64("user:pass")`.
pub fn basic_auth_header(username: &str, password: &str) -> String {
  // `format!` builds an owned `String`; `{username}` captures the variable directly.
  let raw = format!("{username}:{password}");
  // `STANDARD` is the RFC 4648 alphabet with `=` padding, which is what HTTP expects.
  format!("Basic {}", base64::engine::general_purpose::STANDARD.encode(raw))
}

/// Real HTTP client built on `ureq`.
pub struct HttpDevpiClient;

impl HttpDevpiClient {
  /// A ureq agent that returns 4xx/5xx responses as `Ok` so we can inspect the status code
  /// ourselves. By default ureq turns those into `Err`, which would hide the 404 we rely on.
  fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().new_agent()
  }

  /// A `GET` with basic auth (and an optional `Accept` header), returning the full
  /// response so callers can read the body — `status` below only needs the code.
  fn get(url: &str, username: &str, password: &str, accept: Option<&str>) -> Result<ureq::http::Response<ureq::Body>> {
    let agent = Self::agent();
    let auth = basic_auth_header(username, password);
    let mut req = agent.get(url).header("Authorization", &auth);
    // The builder is rebound (`req = …`) because each `header` call consumes and returns it.
    if let Some(a) = accept {
      req = req.header("Accept", a);
    }
    req.call().with_context(|| format!("GET {url}"))
  }

  /// Shared request plumbing: issue `method` to `url` with basic auth and return the status.
  fn status(method: &str, url: &str, username: &str, password: &str) -> Result<u16> {
    let agent = Self::agent();
    let auth = basic_auth_header(username, password);
    // `match` on the method string chooses the request builder; both arms produce the same
    // response type so the `match` itself has a single type.
    let resp = match method {
      "GET" => agent.get(url).header("Authorization", &auth).call(),
      "DELETE" => agent.delete(url).header("Authorization", &auth).call(),
      // Programmer error, not a run-time condition: `unreachable!` documents that.
      _ => unreachable!("unsupported method {method}"),
    }
    .with_context(|| format!("{method} {url}"))?;
    // `StatusCode` → plain `u16` so callers can match on numbers.
    Ok(resp.status().as_u16())
  }
}

impl DevpiClient for HttpDevpiClient {
  fn exists(&self, url: &str, username: &str, password: &str) -> Result<bool> {
    match Self::status("GET", url, username, password)? {
      200 => Ok(true),
      404 => Ok(false),
      // Bind the unexpected code to `s` so the error message can show it.
      s => bail!("unexpected HTTP {s} from GET {url}"),
    }
  }

  fn delete(&self, url: &str, username: &str, password: &str) -> Result<DeleteOutcome> {
    match Self::status("DELETE", url, username, password)? {
      // `|` matches either value in one arm.
      200 | 204 => Ok(DeleteOutcome::Deleted),
      404 => Ok(DeleteOutcome::NotFound),
      s => bail!("unexpected HTTP {s} from DELETE {url}"),
    }
  }

  fn files(&self, url: &str, username: &str, password: &str) -> Result<Option<Vec<(String, String)>>> {
    // Same URL as `exists`, but asking for JSON gets the structured version page instead
    // of HTML.
    let mut resp = Self::get(url, username, password, Some("application/json"))?;
    match resp.status().as_u16() {
      200 => {}
      404 => return Ok(None),
      s => bail!("unexpected HTTP {s} from GET {url}"),
    }
    let body = resp.body_mut().read_to_string().with_context(|| format!("reading {url}"))?;
    parse_version_links(&body).map(Some)
  }

  fn fetch(&self, href: &str, username: &str, password: &str) -> Result<Vec<u8>> {
    let mut resp = Self::get(href, username, password, None)?;
    let status = resp.status().as_u16();
    if status != 200 {
      bail!("unexpected HTTP {status} from GET {href}");
    }
    // ureq caps body reads at 10 MB by default; artifacts can be bigger, so raise it.
    resp
      .body_mut()
      .with_config()
      .limit(512 * 1024 * 1024)
      .read_to_vec()
      .with_context(|| format!("reading {href}"))
  }
}

/// Answers from a flag and records every call; for tests.
///
/// `calls` holds strings like `"GET <url>"` / `"DELETE <url>"`, which keeps assertions in
/// tests short and readable.
pub struct StubDevpiClient {
  pub exists: Cell<bool>,
  pub calls: RefCell<Vec<String>>,
}

impl StubDevpiClient {
  pub fn new(exists: bool) -> Self {
    Self {
      exists: Cell::new(exists),
      calls: RefCell::new(Vec::new()),
    }
  }
}

impl DevpiClient for StubDevpiClient {
  // Leading underscores tell the compiler (and readers) these parameters are intentionally
  // unused — the stub ignores credentials.
  fn exists(&self, url: &str, _username: &str, _password: &str) -> Result<bool> {
    self.calls.borrow_mut().push(format!("GET {url}"));
    Ok(self.exists.get())
  }

  fn delete(&self, url: &str, _username: &str, _password: &str) -> Result<DeleteOutcome> {
    self.calls.borrow_mut().push(format!("DELETE {url}"));
    // `Cell::replace` stores the new value and returns the old one in a single step, which
    // models "delete succeeds exactly once, then the version is gone".
    if self.exists.replace(false) {
      Ok(DeleteOutcome::Deleted)
    } else {
      Ok(DeleteOutcome::NotFound)
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn basic_auth_header_encodes_user_and_password() {
    assert_eq!(basic_auth_header("jacob", "s3cret"), "Basic amFjb2I6czNjcmV0");
  }

  #[test]
  fn parses_version_links_keeping_only_release_files() {
    let body = r#"{"result": {"name": "demo", "version": "1.0.1", "+links": [
      {"rel": "releasefile", "href": "https://x/f/demo-1.0.1-py3-none-any.whl#sha256=abc"},
      {"rel": "releasefile", "href": "https://x/f/demo-1.0.1.tar.gz"},
      {"rel": "doczip", "href": "https://x/f/demo-1.0.1.doc.zip"}
    ]}}"#;
    let links = parse_version_links(body).unwrap();
    assert_eq!(
      links,
      vec![
        (
          "demo-1.0.1-py3-none-any.whl".to_string(),
          "https://x/f/demo-1.0.1-py3-none-any.whl".to_string()
        ),
        ("demo-1.0.1.tar.gz".to_string(), "https://x/f/demo-1.0.1.tar.gz".to_string()),
      ]
    );
    // No links at all is an empty list, not an error.
    assert_eq!(parse_version_links(r#"{"result": {}}"#).unwrap(), vec![]);
    assert!(parse_version_links("not json").is_err());
  }

  #[test]
  fn stub_default_files_answers_from_exists() {
    // The trait's default `files` lets simple stubs participate in ownership checks: an
    // existing version reports an empty file list (no evidence against ownership).
    let s = StubDevpiClient::new(true);
    assert_eq!(s.files("u/p/1", "a", "b").unwrap(), Some(vec![]));
    s.exists.set(false);
    assert_eq!(s.files("u/p/1", "a", "b").unwrap(), None);
    assert!(s.fetch("u/p/f.whl", "a", "b").is_err());
  }

  #[test]
  fn stub_records_calls_and_flips_on_delete() {
    let s = StubDevpiClient::new(true);
    assert!(s.exists("u/p/1", "a", "b").unwrap());
    // `matches!` is a boolean pattern test — nicer than deriving `PartialEq` just for this.
    assert!(matches!(s.delete("u/p/1", "a", "b").unwrap(), DeleteOutcome::Deleted));
    assert!(!s.exists("u/p/1", "a", "b").unwrap());
    assert!(matches!(s.delete("u/p/1", "a", "b").unwrap(), DeleteOutcome::NotFound));
    assert_eq!(*s.calls.borrow(), vec!["GET u/p/1", "DELETE u/p/1", "GET u/p/1", "DELETE u/p/1"]);
  }
}
