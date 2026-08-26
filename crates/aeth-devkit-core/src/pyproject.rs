//! Reading and editing dependency pins and index configuration in `pyproject.toml`.

use regex::Regex;
use std::sync::LazyLock;
use toml_edit::{Array, DocumentMut, Item, Value};

/// PEP 503 normalization: lowercase, runs of `-_.` → `-`.
pub fn normalize_dist_name(name: &str) -> String {
  let mut out = String::with_capacity(name.len());
  let mut last_sep = false;
  for c in name.chars() {
    if c == '-' || c == '_' || c == '.' {
      if !last_sep {
        out.push('-');
      }
      last_sep = true;
    } else {
      out.push(c.to_ascii_lowercase());
      last_sep = false;
    }
  }
  out
}

/// Distribution name at the start of a PEP 508 requirement string, normalized.
pub fn requirement_name(spec: &str) -> String {
  let end = spec
    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
    .unwrap_or(spec.len());
  normalize_dist_name(&spec[..end])
}

/// Where a dependency pin lives: the dotted table path, the array index, and the spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
  pub table: String,
  pub index: usize,
  pub spec: String,
}

fn array_at<'a>(doc: &'a DocumentMut, path: &str) -> Option<&'a Array> {
  let mut item: &Item = doc.as_item();
  for key in path.split('.') {
    item = item.get(key)?;
  }
  item.as_array()
}

fn array_at_mut<'a>(doc: &'a mut DocumentMut, path: &str) -> Option<&'a mut Array> {
  let mut item: &mut Item = doc.as_item_mut();
  for key in path.split('.') {
    item = item.get_mut(key)?;
  }
  item.as_array_mut()
}

/// Every table path that can hold requirement strings, in document order.
fn requirement_tables(doc: &DocumentMut) -> Vec<String> {
  let mut out = vec!["project.dependencies".to_string()];
  if let Some(t) = doc
    .get("project")
    .and_then(|p| p.get("optional-dependencies"))
    .and_then(Item::as_table_like)
  {
    out.extend(t.iter().map(|(k, _)| format!("project.optional-dependencies.{k}")));
  }
  if let Some(t) = doc.get("dependency-groups").and_then(Item::as_table_like) {
    out.extend(t.iter().map(|(k, _)| format!("dependency-groups.{k}")));
  }
  out
}

/// Find the first requirement string for `name` (normalized comparison) across
/// `project.dependencies`, `project.optional-dependencies.*`, and `dependency-groups.*`.
/// Non-string array entries (e.g. `{ include-group = … }`) are skipped.
pub fn find_requirement(doc: &DocumentMut, name: &str) -> Option<Requirement> {
  let want = normalize_dist_name(name);
  for table in requirement_tables(doc) {
    let Some(arr) = array_at(doc, &table) else { continue };
    for (index, v) in arr.iter().enumerate() {
      if let Some(spec) = v.as_str()
        && requirement_name(spec) == want
      {
        return Some(Requirement {
          table,
          index,
          spec: spec.to_string(),
        });
      }
    }
  }
  None
}

static VERSION_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
  // name, optional extras, optional whitespace, operator, optional whitespace, then the version.
  Regex::new(r"^(?P<head>[A-Za-z0-9][A-Za-z0-9._-]*(?:\s*\[[^\]]*\])?\s*(?:===|==|~=|!=|<=|>=|<|>)\s*)(?P<ver>[0-9][0-9A-Za-z.+!*-]*)")
    .unwrap()
});

/// Replace the version token that directly follows the first comparison operator, keeping
/// the name, extras, operator, whitespace, and anything after (markers, further clauses).
/// Returns `None` when the spec has no version to replace.
pub fn set_requirement_version(spec: &str, version: &str) -> Option<String> {
  let caps = VERSION_TOKEN.captures(spec)?;
  let whole = caps.get(0)?;
  Some(format!("{}{}{}", &caps["head"], version, &spec[whole.end()..]))
}

/// Overwrite the spec at `req` with `new_spec`, keeping the element's surrounding whitespace.
pub fn replace_requirement(doc: &mut DocumentMut, req: &Requirement, new_spec: &str) {
  if let Some(arr) = array_at_mut(doc, &req.table)
    && let Some(cur) = arr.get(req.index)
  {
    let mut v = Value::from(new_spec);
    *v.decor_mut() = cur.decor().clone();
    arr.replace(req.index, v);
  }
}

/// Index name declared for `name` under `tool.uv.sources` (a table, inline table, or an
/// array of either).
fn source_index_name(doc: &DocumentMut, want: &str) -> Option<String> {
  let sources = doc.get("tool")?.get("uv")?.get("sources")?.as_table_like()?;
  let (_, source) = sources.iter().find(|(k, _)| normalize_dist_name(k) == want)?;
  let from_table = |t: &dyn toml_edit::TableLike| t.get("index").and_then(Item::as_str).map(str::to_string);
  match source {
    Item::Value(Value::InlineTable(t)) => from_table(t),
    Item::Value(Value::Array(a)) => a.iter().find_map(|v| v.as_inline_table().and_then(|t| from_table(t))),
    Item::Table(t) => from_table(t),
    Item::ArrayOfTables(a) => a.iter().find_map(|t| from_table(t)),
    _ => None,
  }
}

/// The simple-index URL uv is told to use for `name`: `tool.uv.sources.<name>` names an
/// index, and `[[tool.uv.index]]` maps that name to a URL.
pub fn index_url_for(doc: &DocumentMut, name: &str) -> Option<String> {
  let index_name = source_index_name(doc, &normalize_dist_name(name))?;
  let url_of = |t: &dyn toml_edit::TableLike| -> Option<String> {
    (t.get("name").and_then(Item::as_str) == Some(index_name.as_str()))
      .then(|| t.get("url").and_then(Item::as_str).map(str::to_string))
      .flatten()
  };
  match doc.get("tool")?.get("uv")?.get("index")? {
    Item::ArrayOfTables(a) => a.iter().find_map(|t| url_of(t)),
    Item::Value(Value::Array(a)) => a.iter().find_map(|v| v.as_inline_table().and_then(|t| url_of(t))),
    _ => None,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  const DOC: &str = r#"
[project]
  name = "demo"
  dependencies = ["requests>=2", "aeth-ext[sftp, async]>=8.0.2 ; sys_platform == 'win32'"]

  [project.optional-dependencies]
    extra = ["numpy"]

[dependency-groups]
  dev = [
    "pyright>=1.1",
    { include-group = "extra" },
    "aeth_devkit >= 6.0.2",
  ]
  extra = ["pytest"]

[tool.uv]
  [tool.uv.sources]
    aeth-devkit = { index = "Private" }
    aeth-ext = [{ index = "Private", marker = "sys_platform == 'linux'" }]

[[tool.uv.index]]
  name     = "Private"
  url      = "https://pypi.example.com/user/internal/+simple"
  explicit = true
"#;

  fn doc() -> DocumentMut {
    DOC.parse().unwrap()
  }

  #[test]
  fn finds_requirements_in_every_table() {
    let d = doc();
    let r = find_requirement(&d, "aeth-devkit").unwrap();
    assert_eq!(
      (r.table.as_str(), r.index, r.spec.as_str()),
      ("dependency-groups.dev", 2, "aeth_devkit >= 6.0.2")
    );
    let r = find_requirement(&d, "AETH_EXT").unwrap();
    assert_eq!(r.table, "project.dependencies");
    assert_eq!(r.index, 1);
    let r = find_requirement(&d, "numpy").unwrap();
    assert_eq!(r.table, "project.optional-dependencies.extra");
    assert!(find_requirement(&d, "nope").is_none());
  }

  #[test]
  fn rewrites_only_the_version_token() {
    assert_eq!(
      set_requirement_version("aeth-devkit>=6.0.2", "7.0.0").unwrap(),
      "aeth-devkit>=7.0.0"
    );
    assert_eq!(
      set_requirement_version("aeth_devkit >= 6.0.2", "7.0.0").unwrap(),
      "aeth_devkit >= 7.0.0"
    );
    assert_eq!(set_requirement_version("x[a, b]==1.0a1", "1.0").unwrap(), "x[a, b]==1.0");
    assert_eq!(
      set_requirement_version("aeth-ext[sftp]>=8.0.2 ; sys_platform == 'win32'", "9.0.0").unwrap(),
      "aeth-ext[sftp]>=9.0.0 ; sys_platform == 'win32'"
    );
    assert_eq!(set_requirement_version("x~=1.2", "1.3").unwrap(), "x~=1.3");
    assert!(set_requirement_version("numpy", "2.0").is_none());
  }

  #[test]
  fn replaces_in_document_preserving_formatting() {
    let mut d = doc();
    let r = find_requirement(&d, "aeth-devkit").unwrap();
    replace_requirement(&mut d, &r, "aeth_devkit >= 7.0.0");
    let out = d.to_string();
    assert!(out.contains("    \"aeth_devkit >= 7.0.0\",\n"), "{out}");
    assert!(out.contains("{ include-group = \"extra\" }"));
  }

  #[test]
  fn resolves_index_url_from_sources() {
    let d = doc();
    assert_eq!(
      index_url_for(&d, "aeth-devkit").as_deref(),
      Some("https://pypi.example.com/user/internal/+simple")
    );
    assert_eq!(
      index_url_for(&d, "aeth-ext").as_deref(),
      Some("https://pypi.example.com/user/internal/+simple")
    );
    assert!(index_url_for(&d, "requests").is_none());
  }
}
