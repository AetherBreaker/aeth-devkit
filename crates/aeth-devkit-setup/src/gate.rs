//! The template language (spec section 2): `# !` markers, explicit and structural blocks,
//! Python gate expressions, and the `Gates` verdict table every template is rendered through.

use std::collections::HashMap;

use anyhow::{Context as _, Result, bail};
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
