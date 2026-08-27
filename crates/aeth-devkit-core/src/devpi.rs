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
