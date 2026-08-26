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

/// `name[extras]` followed by everything else (specifiers, then an optional `; marker`).
static REQ_HEAD: LazyLock<Regex> =
  LazyLock::new(|| Regex::new(r"^(?P<head>[A-Za-z0-9][A-Za-z0-9._-]*(?:\s*\[[^\]]*\])?)(?P<rest>.*)$").unwrap());

/// One version clause: surrounding whitespace, operator, whitespace, version.
static CLAUSE: LazyLock<Regex> = LazyLock::new(|| {
  Regex::new(r"^(?P<pre>\s*)(?P<op>===|==|~=|!=|<=|>=|<|>)(?P<mid>\s*)(?P<ver>[0-9][0-9A-Za-z.+!-]*)(?P<post>\s*)$").unwrap()
});

struct Clause<'a> {
  pre: &'a str,
  op: &'a str,
  mid: &'a str,
  ver: &'a str,
  post: &'a str,
}

impl Clause<'_> {
  fn render(&self, ver: &str) -> String {
    format!("{}{}{}{}{}", self.pre, self.op, self.mid, ver, self.post)
  }
}

fn parse_clause(s: &str) -> Option<Clause<'_>> {
  let c = CLAUSE.captures(s)?;
  Some(Clause {
    pre: c.name("pre")?.as_str(),
    op: c.name("op")?.as_str(),
    mid: c.name("mid")?.as_str(),
    ver: c.name("ver")?.as_str(),
    post: c.name("post")?.as_str(),
  })
}

/// Numeric release segments of a version string (`7.0.0` → `[7, 0, 0]`); `None` if it
/// contains anything but dot-separated integers.
fn release_parts(ver: &str) -> Option<Vec<u64>> {
  ver.split('.').map(|p| p.parse().ok()).collect()
}

/// Rewrite the version(s) of a requirement so it targets `version`, keeping name, extras,
/// operators, whitespace, clause order, and marker. Supported shapes:
///
/// - a single pin `OP version` with `OP` one of `>=`, `==`, `===`, `~=` → the version is
///   replaced;
/// - a range of exactly one major version, `>=A, <B` (either order) where `B` is a bare
///   major boundary (`7`, `7.0`, `7.0.0`) equal to `A`'s major plus one → the lower bound
///   becomes `version` and the upper bound becomes `version`'s major plus one, keeping
///   the same number of components.
///
/// Returns `None` for anything else — no version, exclusions, other bounds, wildcards,
/// or other ranges — since a bump there would change the requirement's meaning.
pub fn set_requirement_version(spec: &str, version: &str) -> Option<String> {
  let caps = REQ_HEAD.captures(spec)?;
  let head = &caps["head"];
  let rest = &caps["rest"];
  let (specifiers, marker) = match rest.find(';') {
    Some(i) => (&rest[..i], &rest[i..]),
    None => (rest, ""),
  };
  let clauses: Vec<Clause> = specifiers.split(',').map(parse_clause).collect::<Option<_>>()?;

  let rewritten = match clauses.as_slice() {
    [pin] if matches!(pin.op, ">=" | "==" | "===" | "~=") => pin.render(version),
    [a, b] => {
      let lower_first = match (a.op, b.op) {
        (">=", "<") => true,
        ("<", ">=") => false,
        _ => return None,
      };
      let (lower, upper) = if lower_first { (a, b) } else { (b, a) };
      let lower_major = *release_parts(lower.ver)?.first()?;
      let upper_parts = release_parts(upper.ver)?;
      let is_major_boundary = upper_parts.iter().skip(1).all(|&p| p == 0);
      if !is_major_boundary || upper_parts[0] != lower_major + 1 {
        return None;
      }
      let new_major = *release_parts(version)?.first()?;
      let new_upper = std::iter::once((new_major + 1).to_string())
        .chain(upper_parts.iter().skip(1).map(|_| "0".to_string()))
        .collect::<Vec<_>>()
        .join(".");
      let (new_lower, new_upper) = (lower.render(version), upper.render(&new_upper));
      if lower_first {
        format!("{new_lower},{new_upper}")
      } else {
        format!("{new_upper},{new_lower}")
      }
    }
    _ => return None,
  };
  Some(format!("{head}{rewritten}{marker}"))
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
    assert_eq!(set_requirement_version("x===1.2", "1.3").unwrap(), "x===1.3");
    assert!(set_requirement_version("numpy", "2.0").is_none());
  }

  #[test]
  fn refuses_specifiers_where_a_bump_would_change_meaning() {
    // Exclusions and upper bounds are not pins.
    assert!(set_requirement_version("pkg!=1.0", "3.0").is_none());
    assert!(set_requirement_version("pkg<2", "3.0").is_none());
    assert!(set_requirement_version("pkg<=2", "3.0").is_none());
    assert!(set_requirement_version("pkg>1", "3.0").is_none());
    // Ranges that are not "one whole major version" cannot be bumped safely.
    assert!(set_requirement_version("pkg >= 1, != 1.5", "3.0").is_none());
    assert!(set_requirement_version("pkg>=1,<=2", "3.0").is_none());
    assert!(
      set_requirement_version("pkg>=6,<6.5", "7.0").is_none(),
      "upper bound is not a major boundary"
    );
    assert!(set_requirement_version("pkg>=6,<8", "7.0").is_none(), "spans two majors");
    assert!(set_requirement_version("pkg>=6,<7,!=6.1", "7.0").is_none());
    // Wildcards are ranges too.
    assert!(set_requirement_version("pkg==1.*", "3.0").is_none());
  }

  #[test]
  fn bumps_a_range_that_covers_one_major_version() {
    assert_eq!(set_requirement_version("pkg>=6.0.2,<7", "7.1.0").unwrap(), "pkg>=7.1.0,<8");
    // Upper bound keeps its component count; whitespace and clause order are preserved.
    assert_eq!(
      set_requirement_version("pkg >= 6.0.2, < 7.0", "7.1.0").unwrap(),
      "pkg >= 7.1.0, < 8.0"
    );
    assert_eq!(set_requirement_version("pkg<7.0.0,>=6", "7.1.0").unwrap(), "pkg<8.0.0,>=7.1.0");
    // Same major: only the lower bound moves.
    assert_eq!(set_requirement_version("pkg>=6.0.2,<7", "6.3.0").unwrap(), "pkg>=6.3.0,<7");
    // Extras and markers survive.
    assert_eq!(
      set_requirement_version("pkg[x]>=6.0.2,<7 ; sys_platform == 'win32'", "7.1.0").unwrap(),
      "pkg[x]>=7.1.0,<8 ; sys_platform == 'win32'"
    );
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
