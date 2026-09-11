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
    let mut gates = Gates::default();
    let exprs = gates.extend(templates, doc, facts)?;
    if let Some(head) = head {
      let at_head = verdicts_for(&exprs, head, facts)?;
      for (name, e) in &exprs {
        if gates.verdicts.get(e) != at_head.get(e) {
          bail!(
            "pyproject.toml is not committed: the gate `{e}` in {name} evaluates differently against HEAD; commit that change, then rerun setup-project"
          );
        }
      }
    }
    Ok(gates)
  }

  /// Sweep `templates` for expressions the table lacks and evaluate them against `doc`; one
  /// already present was evaluated against the same document and is left alone. For the
  /// templates a run can only read after its package step has installed them (the container
  /// package's), swept once, later, against the document the first sweep saw (2.5). Returns
  /// the `(template, expression)` pairs added.
  pub fn extend(&mut self, templates: &[(String, String)], doc: &DocumentMut, facts: &Facts) -> Result<Vec<(String, String)>> {
    let mut exprs: Vec<(String, String)> = Vec::new();
    for (name, text) in templates {
      for e in expressions(text, Format::for_target(name)).map_err(|e| anyhow!("template {name}: {e:#}"))? {
        if !self.verdicts.contains_key(&e) && !exprs.iter().any(|(_, x)| x == &e) {
          exprs.push((name.clone(), e));
        }
      }
    }
    self.verdicts.extend(verdicts_for(&exprs, doc, facts)?);
    Ok(exprs)
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

/// One open block while rendering.
#[derive(Debug)]
enum Frame {
  Explicit {
    name: String,
    keep: bool,
    line: usize,
  },
  /// A structural block: no end line; it closes when `end` (exclusive line index) is reached.
  Structural {
    keep: bool,
    end: usize,
  },
}

impl Frame {
  fn keep(&self) -> bool {
    match self {
      Frame::Explicit { keep, .. } | Frame::Structural { keep, .. } => *keep,
    }
  }
}

impl Gates {
  /// Render `text`: resolve every block against the swept verdicts and strip every marker
  /// (2.2, 2.3). Line endings come out as LF; the caller restores CRLF where a file has it.
  pub fn apply(&self, text: &str, format: Format, name: &str) -> Result<String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    let mut frames: Vec<Frame> = Vec::new();
    let at = |i: usize| format!("{name} line {}", i + 1);
    for (i, line) in lines.iter().enumerate() {
      // Structural units that end here close first; an explicit block still open inside one
      // is an error rather than a silently extended unit.
      if let Some(pos) = frames.iter().position(|f| matches!(f, Frame::Structural { end, .. } if *end == i))
        && let Some(Frame::Explicit { name: n, line, .. }) = frames.get(pos + 1)
      {
        bail!(
          "{}: block `{n}` opened at line {} must close before the structural unit ends",
          at(i),
          line + 1
        );
      }
      while matches!(frames.last(), Some(Frame::Structural { end, .. }) if *end == i) {
        frames.pop();
      }
      let suppressed = frames.iter().any(|f| !f.keep());
      let Some(m) = find_marker(line, format).map_err(|e| anyhow!("{}: {e:#}", at(i)))? else {
        if !suppressed {
          out.push_str(line);
          out.push('\n');
        }
        continue;
      };
      match parse_body(m.body).map_err(|e| anyhow!("{}: {e:#}", at(i)))? {
        Body::PassThrough => {
          if !suppressed {
            out.push_str(line);
            out.push('\n');
          }
        }
        Body::If { expr, label, block } => {
          let keep = self.verdict(&expr).map_err(|e| anyhow!("{}: {e:#}", at(i)))?;
          if m.content.is_empty() {
            if !block {
              bail!("{}: `!if` on its own line needs a trailing colon", at(i));
            }
            if m.structural {
              let start = (i + 1..lines.len())
                .find(|&j| !lines[j].trim().is_empty() && find_marker(lines[j], format).ok().flatten().is_none())
                .with_context(|| format!("{}: nothing follows the structural gate", at(i)))?;
              let end = unit_end(format, &lines, start).map_err(|e| anyhow!("{}: {e:#}", at(i)))?;
              frames.push(Frame::Structural { keep, end });
            } else {
              frames.push(Frame::Explicit {
                name: label.unwrap_or(expr),
                keep,
                line: i,
              });
            }
          } else {
            if m.structural {
              bail!("{}: a trailing gate cannot be structural", at(i));
            }
            if block {
              bail!("{}: a trailing `!if` gates one line and takes no colon", at(i));
            }
            if !suppressed && keep {
              out.push_str(m.content);
              out.push('\n');
            }
          }
        }
        Body::End(target) => {
          if !m.content.is_empty() && !suppressed {
            out.push_str(m.content);
            out.push('\n');
          }
          match target {
            None => match frames.last() {
              Some(Frame::Explicit { .. }) => {
                frames.pop();
              }
              Some(Frame::Structural { .. }) => bail!("{}: `!end` cannot close a structural block", at(i)),
              None => bail!("{}: `!end` with no open block", at(i)),
            },
            Some(n) => loop {
              match frames.pop() {
                Some(Frame::Explicit { name: open, .. }) if open == n => break,
                Some(Frame::Explicit { .. }) => {}
                Some(Frame::Structural { .. }) => bail!("{}: `!end {n}` would close a structural block", at(i)),
                None => bail!("{}: no open block named `{n}`", at(i)),
              }
            },
          }
        }
      }
    }
    if let Some(Frame::Explicit { name: n, line, .. }) = frames.iter().find(|f| matches!(f, Frame::Explicit { .. })) {
      bail!("{name}: block `{n}` opened at line {} is not closed", line + 1);
    }
    Ok(out)
  }
}

/// The exclusive end of the structural unit starting at `start` (2.3).
fn unit_end(format: Format, lines: &[&str], start: usize) -> Result<usize> {
  let n = lines.len();
  let is_marker = |l: &str| find_marker(l, format).ok().flatten().is_some();
  match format {
    Format::Toml => {
      let mut j = start + 1;
      while j < n && !lines[j].trim_start().starts_with('[') {
        j += 1;
      }
      Ok(before_decor(lines, start, j, |l| l.trim_start().starts_with('#')))
    }
    Format::Markdown => {
      let level = heading_level(lines[start]).with_context(|| "a structural gate in markdown must precede a heading")?;
      let mut fence: Option<String> = None;
      let mut j = start + 1;
      while j < n {
        let token = fence_delimiter(lines[j]);
        match (&fence, token) {
          (None, Some(t)) => fence = Some(t),
          (Some(open), Some(t)) if t.starts_with(open.as_str()) => fence = None,
          _ => {}
        }
        if fence.is_none() && heading_level(lines[j]).is_some_and(|l| l <= level) {
          break;
        }
        j += 1;
      }
      Ok(before_decor(lines, start, j, is_marker))
    }
    Format::Yaml => {
      let indent = indent_of(lines[start]);
      let mut j = start + 1;
      while j < n && (lines[j].trim().is_empty() || indent_of(lines[j]) > indent) {
        j += 1;
      }
      Ok(j)
    }
    Format::Dockerfile => {
      let mut j = start;
      while j < n && lines[j].trim_end().ends_with('\\') {
        j += 1;
      }
      Ok((j + 1).min(n))
    }
    Format::Jsonc | Format::Plain => bail!("structural gates are not defined for {format:?} files"),
  }
}

/// Where a unit ends when its successor is at `next` (or `next == lines.len()`, the end of
/// the file): the decor lines (`is_decor`) directly above the successor are its own, and
/// blank lines above those still belong to the unit. Without a successor nothing is decor,
/// so a trailing marker stays inside the unit and a stray `!end` there is diagnosed.
fn before_decor(lines: &[&str], start: usize, next: usize, is_decor: impl Fn(&str) -> bool) -> usize {
  if next == lines.len() {
    return next;
  }
  let mut k = next;
  while k > start + 1 && (lines[k - 1].trim().is_empty() || is_decor(lines[k - 1])) {
    k -= 1;
  }
  (k..next).find(|&x| is_decor(lines[x])).unwrap_or(next)
}

fn indent_of(line: &str) -> usize {
  line.len() - line.trim_start().len()
}

/// The run of ``` or ~~~ opening or closing a fenced code block, if this line is one. A fence
/// closes only on a run at least as long as the one that opened it.
pub(crate) fn fence_delimiter(line: &str) -> Option<String> {
  let t = line.trim_start();
  for c in ['`', '~'] {
    let n = t.chars().take_while(|&x| x == c).count();
    if n >= 3 {
      return Some(c.to_string().repeat(n));
    }
  }
  None
}

/// `Some(n)` for an ATX heading line with `n` leading `#`s, `None` otherwise.
pub(crate) fn heading_level(line: &str) -> Option<usize> {
  let hashes = line.chars().take_while(|&c| c == '#').count();
  (1..=6).contains(&hashes).then_some(hashes).filter(|&n| line[n..].starts_with(' '))
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

#[cfg(test)]
mod apply_tests {
  use super::*;

  /// Gates with fixed verdicts: `t` true, `f` false.
  fn gates() -> Gates {
    let mut verdicts = HashMap::new();
    verdicts.insert("t".to_string(), true);
    verdicts.insert("f".to_string(), false);
    verdicts.insert("t or f".to_string(), true);
    Gates { verdicts }
  }

  fn apply(text: &str, format: Format) -> String {
    gates().apply(text, format, "x").unwrap()
  }

  fn err(text: &str, format: Format) -> String {
    gates().apply(text, format, "x").unwrap_err().to_string()
  }

  #[test]
  fn explicit_blocks_keep_or_drop_their_lines_and_markers_never_survive() {
    let tpl = "a\n# !if t:\nb\n# !end\n  # !if f:\n  c\n  # !end\nd\n";
    assert_eq!(apply(tpl, Format::Yaml), "a\nb\nd\n");
    assert_eq!(apply("a\n# !if f:\nb\n# !end\n", Format::Yaml), "a\n");
  }

  #[test]
  fn one_liners_gate_their_own_line() {
    let tpl = "- a  # !if t\n- b  # !if f\n- c\n";
    assert_eq!(apply(tpl, Format::Yaml), "- a\n- c\n");
  }

  #[test]
  fn trailing_ends_keep_the_line_and_close_the_block() {
    let tpl = "# !if t:\na\nb  # !end\nc\n";
    assert_eq!(apply(tpl, Format::Yaml), "a\nb\nc\n");
    let tpl = "# !if f as x:\na\nb  # !end x\nc\n";
    assert_eq!(apply(tpl, Format::Yaml), "c\n");
  }

  #[test]
  fn nesting_three_deep_with_a_named_end_unwinding_two() {
    let tpl = "# !if t:\n1\n  # !if t as mid:\n  2\n    # !if t or f:\n    3\n  # !end mid\n4\n# !end\n5\n";
    assert_eq!(apply(tpl, Format::Yaml), "1\n  2\n    3\n4\n5\n");
    // The same shape with the middle block false drops 2 and 3 but not 4.
    let tpl = "# !if t:\n1\n  # !if f as mid:\n  2\n    # !if t:\n    3\n  # !end mid\n4\n# !end\n";
    assert_eq!(apply(tpl, Format::Yaml), "1\n4\n");
    // A block is named by its expression when it has no label.
    let tpl = "# !if t:\n1\n# !if f:\n2\n# !end f\n# !end t\n";
    assert_eq!(apply(tpl, Format::Yaml), "1\n");
  }

  #[test]
  fn structural_toml_unit_runs_to_the_next_header_minus_its_comment_block() {
    let tpl = "[a]\nx = 1\n\n# S!if f:\n[b]\ny = 2\n\n# a comment above c\n# S!if t:\n[c]\nz = 3\n";
    assert_eq!(apply(tpl, Format::Toml), "[a]\nx = 1\n\n# a comment above c\n[c]\nz = 3\n");
    // A key-level gate in TOML is an explicit block or a one-liner, never structural.
    let tpl = "[a]\nx = 1  # !if f\ny = 2\n";
    assert_eq!(apply(tpl, Format::Toml), "[a]\ny = 2\n");
  }

  #[test]
  fn structural_markdown_unit_is_the_section_and_fences_hide_headings() {
    let tpl = "## Always\n\na\n\n<!-- S!if f: -->\n## Gated\n\n```bash\n# not a heading\n```\n\n### Sub\n\nb\n\n<!-- S!if t: -->\n## Kept\n\nc\n";
    assert_eq!(apply(tpl, Format::Markdown), "## Always\n\na\n\n## Kept\n\nc\n");
    let e = err("<!-- S!if t: -->\nnot a heading\n", Format::Markdown);
    assert!(e.contains("heading"), "{e}");
  }

  #[test]
  fn structural_yaml_unit_is_the_next_node_and_dockerfile_the_next_instruction() {
    let tpl = "svc:\n  # S!if f:\n  environment:\n    - A=1\n    - B=2\n  networks:\n    - x\n";
    assert_eq!(apply(tpl, Format::Yaml), "svc:\n  networks:\n    - x\n");
    // A rule line between the marker and the node stays with the node.
    let tpl = "svc:\n  # S!if t:\n  # !rule presence\n  cap_add:\n    - NET_ADMIN\n  x: 1\n";
    assert_eq!(
      apply(tpl, Format::Yaml),
      "svc:\n  # !rule presence\n  cap_add:\n    - NET_ADMIN\n  x: 1\n"
    );
    let tpl = "FROM x\n# S!if f:\nRUN a \\\n  && b\nRUN c\n";
    assert_eq!(apply(tpl, Format::Dockerfile), "FROM x\nRUN c\n");
  }

  #[test]
  fn compose_annotations_pass_through_untouched() {
    let tpl = "services:\n# !service-block:\n  {service}:\n    # !rule exact\n    a: 1\n# !end service-block\n";
    assert_eq!(apply(tpl, Format::Yaml), tpl);
  }

  #[test]
  fn malformed_structure_is_an_error_naming_the_line() {
    for (tpl, needle) in [
      ("# !if t:\na\n", "not closed"),
      ("a\n# !end\n", "no open block"),
      ("# !if t:\n# !end nope\n", "no open block named"),
      ("# !if t\na\n# !end\n", "colon"),
      ("a  # !if t:\n", "colon"),
      ("[a]\n# S!if t:\n[b]\n# !end\n", "structural"),
      // An explicit block opened inside a unit cannot outlive it. (A marker directly above the
      // next header is that header's decor, so it opens outside the unit.)
      (
        "[a]\n# S!if t:\n[b]\n# !if t:\ny = 1\n[c]\nx = 1\n# !end\n",
        "before the structural unit",
      ),
      ("# !bogus\n", "unknown marker"),
      ("a  # S!if t:\n", "structural"),
      ("# S!if t:\n", "nothing follows"),
    ] {
      let e = err(tpl, Format::Toml);
      assert!(e.contains(needle), "{tpl:?}: {e}");
    }
    let e = err("# S!if t:\na\n", Format::Plain);
    assert!(e.contains("not defined for"), "{e}");
  }

  #[test]
  fn lines_keep_their_indentation_and_crlf_is_normalised_to_lf() {
    assert_eq!(apply("  a\r\n  # !if t:\r\n    b\r\n  # !end\r\n", Format::Yaml), "  a\n    b\n");
  }
}
