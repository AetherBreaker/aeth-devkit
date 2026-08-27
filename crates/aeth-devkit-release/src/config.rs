//! Which index to publish to, its credentials, and the package name.
//!
//! The old script hard-coded the devpi URL and the two env-var names. Here everything is
//! derived from `pyproject.toml`: the `[[tool.uv.index]]` entry that has a `publish-url` is
//! the publish target, and its `name` decides the env vars uv itself would look for
//! (`UV_INDEX_<NAME>_USERNAME` / `_PASSWORD`). Nothing project-specific remains in code.

use anyhow::{Result, bail};
use toml_edit::DocumentMut;

use aeth_devkit_core::pyproject::{self, normalize_dist_name};

/// Everything the release needs to know about where it publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
  pub package: String,
  pub index_name: String,
  pub publish_url: String,
  pub username: String,
  pub password: String,
}

/// `SFTPyPI` → (`UV_INDEX_SFTPYPI_USERNAME`, `UV_INDEX_SFTPYPI_PASSWORD`): upper-case, with
/// `-` mapped to `_`, matching uv's own convention so `uv publish --index NAME` agrees.
pub fn env_var_names(index_name: &str) -> (String, String) {
  // `chars()` iterates Unicode scalar values; `map` transforms each; `collect::<String>()`
  // (inferred here from the annotation) glues them back into a `String`.
  let key: String = index_name
    .chars()
    .map(|c| if c == '-' { '_' } else { c.to_ascii_uppercase() })
    .collect();
  // A tuple return is the lightweight way to hand back two related values.
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
/// `env` is a *function parameter* (`&dyn Fn(&str) -> Option<String>`) rather than a direct
/// `std::env::var` call so tests can supply variables without touching the real process
/// environment — which is shared between test threads and therefore racy to mutate.
pub fn resolve(doc: &DocumentMut, index: Option<&str>, env: &dyn Fn(&str) -> Option<String>) -> Result<Config> {
  let package = pyproject::project_name(doc)?;
  let idx = pyproject::publish_index(doc, index)?;
  let (user_var, pass_var) = env_var_names(&idx.name);
  let (username, password) = (env(&user_var), env(&pass_var));
  // Collect the names of any variables that are unset *or empty*, so the error can list
  // all of them at once instead of one per run. `is_none_or` treats `None` as missing and
  // applies the predicate to `Some`.
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
    index_name: idx.name,
    publish_url: idx.publish_url,
    // Safe: the `missing` check above proved both are `Some` and non-empty. `unwrap` here
    // documents an invariant we have just established rather than a hope.
    username: username.unwrap(),
    password: password.unwrap(),
  })
}

impl Config {
  /// Convenience wrapper over [`devpi_url`] using this config's index and package.
  pub fn devpi_url(&self, version: &str) -> String {
    devpi_url(&self.publish_url, &self.package, version)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const DOC: &str = "[project]\nname = \"Aeth_DevKit\"\n\n[[tool.uv.index]]\nname = \"SFTPyPI\"\nurl = \"https://x/+simple\"\npublish-url = \"https://x/user/internal/\"\n";

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
  fn resolves_from_doc_and_env() {
    let doc = DOC.parse().unwrap();
    // A closure standing in for the environment. `match` on the key returns the value.
    let env = |k: &str| match k {
      "UV_INDEX_SFTPYPI_USERNAME" => Some("u".to_string()),
      "UV_INDEX_SFTPYPI_PASSWORD" => Some("p".to_string()),
      _ => None,
    };
    let c = resolve(&doc, None, &env).unwrap();
    assert_eq!(
      (c.package.as_str(), c.index_name.as_str(), c.username.as_str(), c.password.as_str()),
      ("Aeth_DevKit", "SFTPyPI", "u", "p")
    );
    assert_eq!(c.devpi_url("1.0"), "https://x/user/internal/aeth-devkit/1.0");
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
