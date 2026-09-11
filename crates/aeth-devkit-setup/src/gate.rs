//! The template language (spec section 2): `# !` markers, explicit and structural blocks,
//! Python gate expressions, and the `Gates` verdict table every template is rendered through.

use std::collections::HashMap;

use anyhow::{Context as _, Result, anyhow, bail};
use toml_edit::{DocumentMut, Item};

use crate::context::normalize_dist_name;
use crate::gate_eval::{self, Value, World};

/// The file type a template renders into: it fixes the comment syntax a marker uses and
/// what a structural block's unit is (2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
  Toml,
  Yaml,
  Dockerfile,
  Markdown,
  Jsonc,
  /// `#` comments, no structural unit (`.env`, `.gitignore`, `.gitattributes`).
  Plain,
}

impl Format {
  /// By extension of a target or template file name; `Dockerfile` anywhere in the name.
  pub fn for_target(name: &str) -> Format {
    let lower = name.to_ascii_lowercase();
    if lower.contains("dockerfile") {
      Format::Dockerfile
    } else if lower.ends_with(".toml") {
      Format::Toml
    } else if lower.ends_with(".yaml") || lower.ends_with(".yml") {
      Format::Yaml
    } else if lower.ends_with(".md") {
      Format::Markdown
    } else if lower.ends_with(".json") || lower.ends_with(".jsonc") {
      Format::Jsonc
    } else {
      Format::Plain
    }
  }

  /// The comment opener a marker sits in, and the closer a markdown one needs.
  fn leader(self) -> (&'static str, &'static str) {
    match self {
      Format::Markdown => ("<!-- ", " -->"),
      Format::Jsonc => ("// ", ""),
      _ => ("# ", ""),
    }
  }
}

/// A marker on one line: the text after `!`, and the content before it when it trails a
/// content line (empty when the line is the marker alone).
#[derive(Debug)]
pub(crate) struct Marker<'a> {
  pub structural: bool,
  pub body: &'a str,
  pub content: &'a str,
}

/// The marker on `line`, if any. `Err` only for a markdown marker that never closes.
pub(crate) fn find_marker(line: &str, format: Format) -> Result<Option<Marker<'_>>> {
  let (open, close) = format.leader();
  for (bang, structural) in [("S!", true), ("!", false)] {
    let prefix = format!("{open}{bang}");
    let Some(idx) = line.find(&prefix) else { continue };
    let content = line[..idx].trim_end();
    let rest = &line[idx + prefix.len()..];
    let body = if close.is_empty() {
      rest
    } else {
      rest
        .trim_end()
        .strip_suffix(close.trim_start())
        .with_context(|| format!("marker `{}` is not closed with `{}`", line.trim(), close.trim()))?
    };
    return Ok(Some(Marker {
      structural,
      body: body.trim(),
      content,
    }));
  }
  Ok(None)
}

/// What a marker says.
#[derive(Debug)]
pub(crate) enum Body {
  /// `if <expr>[ as <label>]` with (`block`) or without a trailing colon.
  If { expr: String, label: Option<String>, block: bool },
  /// `end` or `end <name>`.
  End(Option<String>),
  /// A compose annotation (`service-block:`, `end service-block`, `rule <kind>`): not the
  /// gate pass's business, emitted unchanged for the scaffold parser (2.6).
  PassThrough,
}

pub(crate) fn parse_body(body: &str) -> Result<Body> {
  if body == "service-block:" || body == "end service-block" || body.starts_with("rule ") {
    return Ok(Body::PassThrough);
  }
  if body == "end" {
    return Ok(Body::End(None));
  }
  if let Some(name) = body.strip_prefix("end ") {
    return Ok(Body::End(Some(name.trim().to_string())));
  }
  let Some(rest) = body.strip_prefix("if ") else {
    bail!("unknown marker `!{body}`; expected if, end, service-block or rule");
  };
  let (rest, block) = match rest.strip_suffix(':') {
    Some(r) => (r, true),
    None => (rest, false),
  };
  let (expr, label) = split_label(rest);
  if expr.is_empty() {
    bail!("`!if` has no expression");
  }
  if let Some(l) = &label
    && (l.is_empty() || !l.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
  {
    bail!("label `{l}` after `as` must be one word of letters, digits, `_` or `-`");
  }
  Ok(Body::If { expr, label, block })
}

/// Split `<expr> as <label>` at the last ` as ` outside a string literal.
fn split_label(s: &str) -> (String, Option<String>) {
  let bytes = s.as_bytes();
  let mut quote: Option<u8> = None;
  let mut split: Option<usize> = None;
  let mut i = 0;
  while i < bytes.len() {
    let b = bytes[i];
    match quote {
      Some(q) => {
        if b == b'\\' {
          i += 1;
        } else if b == q {
          quote = None;
        }
      }
      None => {
        if b == b'"' || b == b'\'' {
          quote = Some(b);
        } else if s[i..].starts_with(" as ") {
          split = Some(i);
        }
      }
    }
    i += 1;
  }
  match split {
    Some(at) => (s[..at].trim().to_string(), Some(s[at + 4..].trim().to_string())),
    None => (s.trim().to_string(), None),
  }
}

/// Facts a gate can ask about that are not in `pyproject.toml`.
#[derive(Debug, Clone)]
pub struct Facts {
  /// A `Cargo.toml` at the project root.
  pub rust: bool,
  /// A Dockerfile or compose file on disk (`ProjectContext::docker_files`).
  pub docker_files: bool,
}

/// The truth value of every gate expression in the templates of one run, evaluated once
/// against the working copy's `pyproject.toml` (2.5).
#[derive(Debug, Default)]
pub struct Gates {
  verdicts: HashMap<String, bool>,
}

impl Gates {
  /// Sweep `templates` (`(name, text)` pairs; the name picks the comment syntax and names
  /// the file in errors), evaluate each distinct expression against `doc`, and, when `head`
  /// is given, against it too: a gate whose verdicts differ is the refusal (2.5). A missing
  /// HEAD file is passed as an empty document.
  pub fn build(templates: &[(String, String)], doc: &DocumentMut, head: Option<&DocumentMut>, facts: &Facts) -> Result<Gates> {
    let mut exprs: Vec<(String, String)> = Vec::new();
    for (name, text) in templates {
      for e in expressions(text, Format::for_target(name)).map_err(|e| anyhow!("template {name}: {e:#}"))? {
        if !exprs.iter().any(|(_, x)| x == &e) {
          exprs.push((name.clone(), e));
        }
      }
    }
    let verdicts = verdicts_for(&exprs, doc, facts)?;
    if let Some(head) = head {
      let at_head = verdicts_for(&exprs, head, facts)?;
      for (name, e) in &exprs {
        if verdicts.get(e) != at_head.get(e) {
          bail!(
            "pyproject.toml is not committed: the gate `{e}` in {name} evaluates differently against HEAD; commit that change, then rerun setup-project"
          );
        }
      }
    }
    Ok(Gates { verdicts })
  }

  /// The swept verdict for `expr`; an expression the sweep never saw is a bug.
  pub fn verdict(&self, expr: &str) -> Result<bool> {
    self
      .verdicts
      .get(expr)
      .copied()
      .with_context(|| format!("gate `{expr}` was not swept before rendering"))
  }
}

fn verdicts_for(exprs: &[(String, String)], doc: &DocumentMut, facts: &Facts) -> Result<HashMap<String, bool>> {
  let deps = crate::context::dependencies_of(doc);
  let own = doc
    .get("project")
    .and_then(|p| p.get("name"))
    .and_then(|n| n.as_str())
    .map(normalize_dist_name)
    .unwrap_or_default();
  let publish_index = aeth_devkit_core::pyproject::publish_indexes(doc)
    .map(|v| !v.is_empty())
    .unwrap_or(false);
  let flags = [
    ("rust", facts.rust),
    ("publish_index", publish_index),
    ("docker_files", facts.docker_files),
  ];
  let keys = |path: &str| lookup(doc, path);
  let dep = |name: &str| {
    let n = normalize_dist_name(name);
    deps.contains(&n) || own == n
  };
  let world = World {
    flags: &flags,
    keys: &keys,
    dep: &dep,
  };
  let mut out = HashMap::new();
  for (name, e) in exprs {
    let v = gate_eval::evaluate(e, &world).map_err(|e| anyhow!("template {name}: {e:#}"))?;
    out.insert(e.clone(), v);
  }
  Ok(out)
}

/// Every `!if` expression in `text`, in order, each once.
pub fn expressions(text: &str, format: Format) -> Result<Vec<String>> {
  let mut out: Vec<String> = Vec::new();
  for (i, line) in text.lines().enumerate() {
    let Some(m) = find_marker(line, format).with_context(|| format!("line {}", i + 1))? else {
      continue;
    };
    if let Body::If { expr, .. } = parse_body(m.body).with_context(|| format!("line {}", i + 1))?
      && !out.contains(&expr)
    {
      out.push(expr);
    }
  }
  Ok(out)
}

/// Every file under `dir`, recursively, as `(path relative to dir with '/', text)`.
pub fn collect_dir(dir: &std::path::Path) -> Result<Vec<(String, String)>> {
  fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) -> Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))? {
      let path = entry?.path();
      if path.is_dir() {
        walk(base, &path, out)?;
      } else {
        let rel = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
        let text = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        out.push((rel, text));
      }
    }
    Ok(())
  }
  let mut out = Vec::new();
  walk(dir, dir, &mut out)?;
  Ok(out)
}

/// The value at a dotted path, `None` when any segment is missing (2.4).
fn lookup(doc: &DocumentMut, path: &str) -> Value {
  let mut item: &Item = doc.as_item();
  for seg in path.split('.') {
    match item.as_table_like().and_then(|t| t.get(seg)) {
      Some(next) => item = next,
      None => return Value::None,
    }
  }
  to_value(item)
}

fn to_value(item: &Item) -> Value {
  match item {
    Item::None => Value::None,
    Item::Value(v) => scalar(v),
    Item::Table(t) => Value::Dict(t.iter().map(|(k, i)| (k.to_string(), to_value(i))).collect()),
    Item::ArrayOfTables(a) => Value::List(
      a.iter()
        .map(|t| Value::Dict(t.iter().map(|(k, i)| (k.to_string(), to_value(i))).collect()))
        .collect(),
    ),
  }
}

fn scalar(v: &toml_edit::Value) -> Value {
  match v {
    toml_edit::Value::String(s) => Value::Str(s.value().clone()),
    toml_edit::Value::Integer(i) => Value::Int(*i.value()),
    toml_edit::Value::Float(f) => Value::Float(*f.value()),
    toml_edit::Value::Boolean(b) => Value::Bool(*b.value()),
    toml_edit::Value::Datetime(d) => Value::Str(d.value().to_string()),
    toml_edit::Value::Array(a) => Value::List(a.iter().map(scalar).collect()),
    toml_edit::Value::InlineTable(t) => Value::Dict(t.iter().map(|(k, v)| (k.to_string(), scalar(v))).collect()),
  }
}

#[cfg(test)]
mod marker_tests {
  use super::*;

  #[test]
  fn formats_come_from_the_file_name() {
    assert_eq!(Format::for_target("pyproject.toml"), Format::Toml);
    assert_eq!(Format::for_target("pyproject.template.toml"), Format::Toml);
    assert_eq!(Format::for_target("docker/compose.yaml"), Format::Yaml);
    assert_eq!(Format::for_target("github/workflows/release.rust.template.yml"), Format::Yaml);
    assert_eq!(Format::for_target("template.Dockerfile"), Format::Dockerfile);
    assert_eq!(Format::for_target("AGENTS.md"), Format::Markdown);
    assert_eq!(Format::for_target("vscode/settings.template.jsonc"), Format::Jsonc);
    assert_eq!(Format::for_target(".mcp.json"), Format::Jsonc);
    assert_eq!(Format::for_target("env"), Format::Plain);
    assert_eq!(Format::for_target("template.gitignore"), Format::Plain);
  }

  #[test]
  fn markers_are_found_in_each_comment_syntax_at_any_indent() {
    let m = find_marker("    # !if rust:", Format::Yaml).unwrap().unwrap();
    assert!(!m.structural && m.body == "if rust:" && m.content.is_empty());
    let m = find_marker("# S!if dep(\"mypy\"):", Format::Toml).unwrap().unwrap();
    assert!(m.structural && m.body == "if dep(\"mypy\"):");
    let m = find_marker("  <!-- S!if dep(\"aeth-ext\"): -->", Format::Markdown)
      .unwrap()
      .unwrap();
    assert!(m.structural && m.body == "if dep(\"aeth-ext\"):");
    let m = find_marker("  // !end", Format::Jsonc).unwrap().unwrap();
    assert!(m.body == "end");
    // Trailing: the content before the marker is kept, trailing whitespace trimmed.
    let m = find_marker("  - FOO=bar   # !if not publish_index", Format::Yaml).unwrap().unwrap();
    assert_eq!(m.content, "  - FOO=bar");
    assert_eq!(m.body, "if not publish_index");
    let m = find_marker("Some text. <!-- !end -->", Format::Markdown).unwrap().unwrap();
    assert_eq!((m.content, m.body), ("Some text.", "end"));
  }

  #[test]
  fn shebangs_comments_and_other_syntaxes_are_not_markers() {
    assert!(find_marker("#!/bin/sh", Format::Plain).unwrap().is_none());
    assert!(find_marker("# a comment with ! in it", Format::Yaml).unwrap().is_none());
    assert!(find_marker("# setup-project: if-x", Format::Yaml).unwrap().is_none());
    assert!(find_marker("// !end", Format::Yaml).unwrap().is_none(), "wrong leader for yaml");
    assert!(
      find_marker("# !end", Format::Markdown).unwrap().is_none(),
      "wrong leader for markdown"
    );
    let err = find_marker("<!-- !end", Format::Markdown).unwrap_err().to_string();
    assert!(err.contains("-->"), "{err}");
  }

  #[test]
  fn marker_bodies_parse() {
    assert!(matches!(parse_body("if rust:").unwrap(), Body::If { expr, label: None, block: true } if expr == "rust"));
    assert!(matches!(parse_body("if rust").unwrap(), Body::If { block: false, .. }));
    match parse_body("if dep(\"a\") or keys(\"x\") as heartbeat:").unwrap() {
      Body::If { expr, label, block } => {
        assert_eq!(expr, "dep(\"a\") or keys(\"x\")");
        assert_eq!(label.as_deref(), Some("heartbeat"));
        assert!(block);
      }
      other => panic!("{other:?}"),
    }
    // ` as ` inside a string literal is not a label.
    match parse_body("if keys(\"a\") == \"x as y\":").unwrap() {
      Body::If { expr, label, .. } => {
        assert_eq!(expr, "keys(\"a\") == \"x as y\"");
        assert!(label.is_none());
      }
      other => panic!("{other:?}"),
    }
    assert!(matches!(parse_body("end").unwrap(), Body::End(None)));
    assert!(matches!(parse_body("end docker.wireguard").unwrap(), Body::End(Some(n)) if n == "docker.wireguard"));
    assert!(matches!(parse_body("service-block:").unwrap(), Body::PassThrough));
    assert!(matches!(parse_body("end service-block").unwrap(), Body::PassThrough));
    assert!(matches!(parse_body("rule presence").unwrap(), Body::PassThrough));
    for bad in ["fi rust:", "if:", "if  :", "ends", "if rust as :", "if rust as bad label:"] {
      assert!(parse_body(bad).is_err(), "{bad}");
    }
  }
}

#[cfg(test)]
mod gates_tests {
  use super::*;

  const PYPROJECT: &str = r#"
[project]
name = "Demo-App"
version = "1.2.3"
dependencies = ["aeth-ext[sftp]>=8", "requests"]

[dependency-groups]
dev = ["mypy>=1"]

[tool.docker]
services = ["demo-app", "worker"]
wireguard = true

[tool.ruff.lint.per-file-ignores]
"tests/**" = ["D1"]

[[tool.uv.index]]
name = "SFTPyPI"
url = "https://x/+simple"
publish-url = "https://x/"
"#;

  fn doc(s: &str) -> DocumentMut {
    s.parse().unwrap()
  }

  fn facts() -> Facts {
    Facts {
      rust: false,
      docker_files: true,
    }
  }

  #[test]
  fn the_sweep_collects_each_distinct_expression_once() {
    let text = "a\n# !if rust:\nb\n# !end\n# S!if dep(\"mypy\"):\n[t]\nx = 1  # !if rust\n# !rule exact\n";
    assert_eq!(
      expressions(text, Format::Toml).unwrap(),
      vec!["rust".to_string(), "dep(\"mypy\")".to_string()]
    );
    let md = "<!-- S!if dep(\"aeth-ext\"): -->\n## H\n";
    assert_eq!(expressions(md, Format::Markdown).unwrap(), vec!["dep(\"aeth-ext\")".to_string()]);
    assert!(expressions("# !bogus\n", Format::Yaml).is_err());
  }

  #[test]
  fn keys_dep_and_flags_answer_from_the_document() {
    let templates = vec![(
      "t.yaml".to_string(),
      "# !if keys(\"tool.docker.wireguard\"):\n# !end\n# !if dep(\"aeth-ext\") and dep(\"demo_app\") and dep(\"mypy\"):\n# !end\n# !if \"worker\" in keys(\"tool.docker.services\"):\n# !end\n# !if \"tests/**\" in keys(\"tool.ruff.lint.per-file-ignores\"):\n# !end\n# !if keys(\"project.version\") == \"1.2.3\":\n# !end\n# !if keys(\"tool.docker.nope\") is None:\n# !end\n# !if publish_index and docker_files and not rust:\n# !end\n# !if keys(\"tool.uv.index\")[0][\"name\"] == \"SFTPyPI\":\n# !end\n".to_string(),
    )];
    let g = Gates::build(&templates, &doc(PYPROJECT), None, &facts()).unwrap();
    for e in [
      "keys(\"tool.docker.wireguard\")",
      "dep(\"aeth-ext\") and dep(\"demo_app\") and dep(\"mypy\")",
      "\"worker\" in keys(\"tool.docker.services\")",
      "\"tests/**\" in keys(\"tool.ruff.lint.per-file-ignores\")",
      "keys(\"project.version\") == \"1.2.3\"",
      "keys(\"tool.docker.nope\") is None",
      "publish_index and docker_files and not rust",
      "keys(\"tool.uv.index\")[0][\"name\"] == \"SFTPyPI\"",
    ] {
      assert!(g.verdict(e).unwrap(), "{e}");
    }
    assert!(g.verdict("never swept").is_err());
  }

  #[test]
  fn a_gate_that_flips_between_head_and_the_working_copy_is_refused() {
    let templates = vec![(
      "t.yaml".to_string(),
      "# !if keys(\"tool.docker.wireguard\"):\n# !end\n# !if dep(\"requests\"):\n# !end\n".to_string(),
    )];
    let head = doc(&PYPROJECT.replace("wireguard = true\n", ""));
    let err = Gates::build(&templates, &doc(PYPROJECT), Some(&head), &facts())
      .unwrap_err()
      .to_string();
    assert!(
      err.contains("not committed") && err.contains("keys(\"tool.docker.wireguard\")") && err.contains("t.yaml"),
      "{err}"
    );
    // A key that changed without flipping any gate does not refuse.
    let head = doc(&PYPROJECT.replace("version = \"1.2.3\"", "version = \"1.2.4\""));
    assert!(Gates::build(&templates, &doc(PYPROJECT), Some(&head), &facts()).is_ok());
    // No pyproject at HEAD: an empty document, so a gate that is true now is refused.
    let err = Gates::build(&templates, &doc(PYPROJECT), Some(&doc("")), &facts())
      .unwrap_err()
      .to_string();
    assert!(err.contains("not committed"), "{err}");
  }

  #[test]
  fn an_expression_error_names_the_template() {
    let templates = vec![(
      "AGENTS.template.md".to_string(),
      "<!-- S!if dep(\"a\") or nope: -->\n## H\n".to_string(),
    )];
    let err = Gates::build(&templates, &doc(PYPROJECT), None, &facts()).unwrap_err().to_string();
    assert!(err.contains("AGENTS.template.md") && err.contains("nope"), "{err}");
  }

  #[test]
  fn collect_dir_reads_every_file_with_slash_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("docker")).unwrap();
    std::fs::write(dir.path().join("pyproject.template.toml"), "a").unwrap();
    std::fs::write(dir.path().join("docker").join("compose.template.yaml"), "b").unwrap();
    let mut got = collect_dir(dir.path()).unwrap();
    got.sort();
    assert_eq!(
      got,
      vec![
        ("docker/compose.template.yaml".to_string(), "b".to_string()),
        ("pyproject.template.toml".to_string(), "a".to_string())
      ]
    );
  }
}
