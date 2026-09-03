//! Where a release publishes, its credentials, and the package name.
//!
//! Everything is derived from `pyproject.toml`: the `[[tool.uv.index]]` entry that has a
//! `publish-url` is the publish target, and its `name` decides which credential variables
//! to read (`UV_INDEX_<NAME>_USERNAME` / `_PASSWORD`). No publish index means PyPI, which
//! the workflow reaches through trusted publishing — no credentials at all. Nothing
//! project-specific remains in code.
//!
//! The credential variables are uv's for *reading* from an index, which a developer already
//! has set in order to install from it. The workflow reads the same names from repository
//! secrets and hands them to `uv publish` as `UV_PUBLISH_USERNAME` / `_PASSWORD`.

use anyhow::{Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::pyproject::{self, index_env_key, normalize_dist_name};

/// PyPI's simple index, used for the existing-version probe and the post-CI check when no
/// private index is configured.
pub const PYPI_SIMPLE: &str = "https://pypi.org/simple";

/// Where the workflow publishes, as resolved from `pyproject.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishTarget {
  /// The sole `[[tool.uv.index]]` with a `publish-url`. `url` is its simple index (what
  /// readers query), `publish_url` its devpi root (what pre-flight removal talks to).
  Index {
    name: String,
    url: String,
    publish_url: String,
    username: String,
    password: String,
  },
  /// No publish index: PyPI, via trusted publishing in CI.
  Pypi,
}

impl PublishTarget {
  /// Name for messages and the artefact table.
  pub fn label(&self) -> &str {
    match self {
      PublishTarget::Index { name, .. } => name,
      PublishTarget::Pypi => "PyPI",
    }
  }

  /// The simple index to read versions from.
  pub fn simple_url(&self) -> &str {
    match self {
      PublishTarget::Index { url, .. } => url,
      PublishTarget::Pypi => PYPI_SIMPLE,
    }
  }
}

/// Everything the release needs to know about the package and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
  pub package: String,
  pub target: PublishTarget,
}

/// `SFTPyPI` → (`UV_INDEX_SFTPYPI_USERNAME`, `UV_INDEX_SFTPYPI_PASSWORD`).
pub fn env_var_names(index_name: &str) -> (String, String) {
  let key = index_env_key(index_name);
  (format!("UV_INDEX_{key}_USERNAME"), format!("UV_INDEX_{key}_PASSWORD"))
}

/// The devpi REST URL for one release: `<publish-url>/<normalized package>/<version>`.
pub fn devpi_url(publish_url: &str, package: &str, version: &str) -> String {
  // `trim_end_matches('/')` avoids a double slash whether or not the configured URL ends
  // with one; the package name is PEP 503-normalized because that is how devpi keys it.
  format!("{}/{}/{version}", publish_url.trim_end_matches('/'), normalize_dist_name(package))
}

/// Build the [`Config`] from the parsed `pyproject.toml`, an optional `--index` override,
/// and an environment lookup.
///
/// `env` is a parameter (`&dyn Fn(&str) -> Option<String>`) rather than a direct
/// `std::env::var` call so tests can supply variables without touching the real process
/// environment, which is shared between test threads and therefore racy to mutate.
///
/// `--index` must name an index with a `publish-url`; without the flag, one publish index
/// selects it, none selects PyPI, and several is an error (the workflow publishes to one
/// place, and guessing which would be wrong half the time).
pub fn resolve(doc: &DocumentMut, index: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config> {
  let package = pyproject::project_name(doc)?;
  let all = pyproject::publish_indexes(doc)?;
  let chosen = match index {
    Some(want) => {
      // `publish_index` produces the precise "no such index" / "no publish-url" errors.
      let named = pyproject::publish_index(doc, Some(want))?;
      all.into_iter().find(|i| i.name == named.name)
    }
    None => match all.len() {
      0 => None,
      1 => all.into_iter().next(),
      _ => bail!(
        "several indexes have a publish-url ({}); pass --index",
        all.iter().map(|i| i.name.as_str()).collect::<Vec<_>>().join(", ")
      ),
    },
  };
  let Some(idx) = chosen else {
    return Ok(Config {
      package,
      target: PublishTarget::Pypi,
    });
  };
  let (user_var, pass_var) = env_var_names(&idx.name);
  let (username, password) = (env(&user_var), env(&pass_var));
  // Names of variables that are unset *or empty*, so the error lists all of them at once.
  let missing: Vec<&str> = [(&user_var, &username), (&pass_var, &password)]
    .into_iter()
    .filter(|(_, v)| v.as_deref().is_none_or(str::is_empty))
    .map(|(k, _)| k.as_str())
    .collect();
  if !missing.is_empty() {
    bail!("required environment variables are not set:\n  - {}", missing.join("\n  - "));
  }
  Ok(Config {
    package,
    target: PublishTarget::Index {
      name: idx.name,
      url: idx.url,
      publish_url: idx.publish_url,
      // Safe: the `missing` check above proved both are `Some` and non-empty.
      username: username.unwrap(),
      password: password.unwrap(),
    },
  })
}

impl Config {
  /// The devpi URL of `version` on the private index; `None` for PyPI, which has no such
  /// endpoint (and whose releases cannot be deleted anyway).
  pub fn devpi_url(&self, version: &str) -> Option<String> {
    match &self.target {
      PublishTarget::Index { publish_url, .. } => Some(devpi_url(publish_url, &self.package, version)),
      PublishTarget::Pypi => None,
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const DOC: &str = "[project]\nname = \"Aeth_DevKit\"\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";

  fn env(k: &str) -> Option<String> {
    match k {
      "UV_INDEX_SFTPYPI_USERNAME" => Some("u".to_string()),
      "UV_INDEX_SFTPYPI_PASSWORD" => Some("p".to_string()),
      _ => None,
    }
  }

  #[test]
  fn env_names_follow_uv_convention() {
    assert_eq!(
      env_var_names("SFTPyPI"),
      ("UV_INDEX_SFTPYPI_USERNAME".into(), "UV_INDEX_SFTPYPI_PASSWORD".into())
    );
    assert_eq!(env_var_names("my-index").0, "UV_INDEX_MY_INDEX_USERNAME");
  }

  #[test]
  fn devpi_url_joins_and_normalizes() {
    assert_eq!(
      devpi_url("https://x/user/internal/", "Aeth_DevKit", "7.0.3"),
      "https://x/user/internal/aeth-devkit/7.0.3"
    );
  }

  #[test]
  fn resolves_an_index_target_from_doc_and_env() {
    let doc = DOC.parse().unwrap();
    let c = resolve(&doc, None, &env).unwrap();
    assert_eq!(c.package, "Aeth_DevKit");
    assert_eq!(
      c.target,
      PublishTarget::Index {
        name: "SFTPyPI".into(),
        url: "https://x/+simple".into(),
        publish_url: "https://x/user/internal/".into(),
        username: "u".into(),
        password: "p".into(),
      }
    );
    assert_eq!(c.target.label(), "SFTPyPI");
    assert_eq!(c.target.simple_url(), "https://x/+simple");
    assert_eq!(c.devpi_url("1.0").as_deref(), Some("https://x/user/internal/aeth-devkit/1.0"));
  }

  #[test]
  fn no_publish_index_means_pypi() {
    for doc in [
      "[project]\nname = \"demo\"\n",
      "[project]\nname = \"demo\"\n\n[[tool.uv.index]]\nname = \"Ro\"\nurl = \"https://x/+simple\"\n",
    ] {
      let c = resolve(&doc.parse().unwrap(), None, &|_| None).unwrap();
      assert_eq!(c.target, PublishTarget::Pypi, "{doc}");
      assert_eq!(c.target.label(), "PyPI");
      assert_eq!(c.target.simple_url(), PYPI_SIMPLE);
      assert_eq!(c.devpi_url("1.0"), None);
    }
  }

  #[test]
  fn explicit_index_must_have_a_publish_url() {
    let doc: DocumentMut = "[project]\nname = \"demo\"\n\n[[tool.uv.index]]\nname = \"Ro\"\nurl = \"https://x/+simple\"\n"
      .parse()
      .unwrap();
    let e = resolve(&doc, Some("Ro"), &|_| None).unwrap_err().to_string();
    assert!(e.contains("no publish-url"), "{e}");
    assert!(resolve(&doc, Some("Nope"), &|_| None).is_err());
  }

  #[test]
  fn several_publish_indexes_is_an_error() {
    let doc: DocumentMut =
      format!("{DOC}\n[[tool.uv.index]]\nname = \"B\"\nurl = \"https://b/+simple\"\npublish-url = \"https://b/\"\n")
        .parse()
        .unwrap();
    let e = resolve(&doc, None, &env).unwrap_err().to_string();
    assert!(e.contains("SFTPyPI, B") && e.contains("--index"), "{e}");
    assert_eq!(resolve(&doc, Some("B"), &|_| Some("x".into())).unwrap().target.label(), "B");
  }

  #[test]
  fn missing_env_lists_both_names() {
    let doc = DOC.parse().unwrap();
    let e = resolve(&doc, None, &|_| None).unwrap_err().to_string();
    assert!(
      e.contains("UV_INDEX_SFTPYPI_USERNAME") && e.contains("UV_INDEX_SFTPYPI_PASSWORD"),
      "{e}"
    );
  }
}
