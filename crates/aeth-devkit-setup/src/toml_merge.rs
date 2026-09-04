//! Comment-preserving deep merge of the template `pyproject.toml` into a project's.

use anyhow::{Context as _, Result};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::context::{ProjectContext, dependency_name};

const ADDED_COMMENT_PREFIX: &str = " # setup-project added: ";
/// Common prefix of every marker comment, used to strip them all on the keep path.
const MARKER: &str = "setup-project:";
const IF_DEP_MARKER: &str = "setup-project: if-dep ";
const IF_DOCKER_MARKER: &str = "setup-project: if-docker";

pub fn merge_pyproject(original: &str, template: &str, ctx: &ProjectContext, log: &mut Vec<String>) -> Result<String> {
  let mut doc: DocumentMut = original.parse().context("parsing project pyproject.toml")?;
  let tpl: DocumentMut = template.parse().context("parsing template pyproject.toml")?;

  let keep = keep_list(&doc);
  let mut merger = Merger { ctx, keep: &keep, log };
  merger.merge_table(doc.as_table_mut(), tpl.as_table(), "");
  remove_extends(&mut doc, merger.log);
  renumber_tables(doc.as_table_mut(), &mut 1);

  Ok(doc.to_string())
}

/// Assign table positions in depth-first traversal order so every sub-table is emitted
/// directly after its parent (tables copied from the template would otherwise carry the
/// template's positions and end up at the bottom of the file). Idempotent.
fn renumber_tables(table: &mut Table, next: &mut isize) {
  for (_, item) in table.iter_mut() {
    match item {
      Item::Table(t) => {
        t.set_position(Some(*next));
        *next += 1;
        renumber_tables(t, next);
      }
      Item::ArrayOfTables(a) => {
        for t in a.iter_mut() {
          t.set_position(Some(*next));
          *next += 1;
          renumber_tables(t, next);
        }
      }
      _ => {}
    }
  }
}

/// `[tool.setup-project].keep` — dotted keys that must never be touched.
fn keep_list(doc: &DocumentMut) -> Vec<String> {
  doc
    .get("tool")
    .and_then(|t| t.get("setup-project"))
    .and_then(|s| s.get("keep"))
    .and_then(Item::as_array)
    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
    .unwrap_or_default()
}

struct Merger<'a> {
  ctx: &'a ProjectContext,
  keep: &'a [String],
  log: &'a mut Vec<String>,
}

impl Merger<'_> {
  fn kept(&self, path: &str) -> bool {
    self.keep.iter().any(|k| k == path || path.starts_with(&format!("{k}.")))
  }

  fn merge_table(&mut self, target: &mut Table, template: &Table, path: &str) {
    for (key, titem) in template.iter() {
      let child = if path.is_empty() {
        key.to_string()
      } else {
        format!("{path}.{key}")
      };
      if self.kept(&child) {
        continue;
      }
      if let Some(dep) = conditional_dep(template, key)
        && !self.ctx.has_dependency(&dep)
      {
        continue;
      }
      if conditional_docker(template, key) && !self.ctx.has_docker {
        continue;
      }
      let tkey = template.key(key).expect("iterating template keys").clone();
      match titem {
        Item::Table(ttable) => {
          let needs_insert = !matches!(target.get(key), Some(Item::Table(_)));
          if needs_insert {
            if ttable.is_implicit() || has_only_subtables(ttable) {
              // A missing intermediate table (e.g. `tool`) stays implicit so no bare header
              // is emitted; recurse so conditional/keep rules still apply to its children.
              let mut t = Table::new();
              t.set_implicit(true);
              target.insert_formatted(&tkey, Item::Table(t));
            } else {
              // A brand-new leaf table: copy it so the template's formatting (indentation,
              // alignment, comments) is preserved — minus the `setup-project:` markers,
              // which are instructions *to* this merger. Shipping one would leave a comment
              // in the project's pyproject.toml that reads like a live directive but is
              // only ever honoured on the template side.
              let mut fresh = ttable.clone();
              strip_marker_comments(&mut fresh);
              target.insert_formatted(&tkey, Item::Table(fresh));
              self.log.push(format!("added [{child}]"));
              continue;
            }
          }
          let sub = target.get_mut(key).and_then(Item::as_table_mut).expect("just inserted");
          self.merge_table(sub, ttable, &child);
        }
        Item::Value(tval) => self.merge_value(target, &tkey, tval, &child),
        Item::ArrayOfTables(_) | Item::None => {}
      }
    }
  }

  fn merge_value(&mut self, target: &mut Table, tkey: &toml_edit::Key, tval: &Value, path: &str) {
    let key = tkey.get();
    match target.get_mut(key) {
      None => {
        // Carry the template key's decor (indentation) so the new line matches its neighbours.
        target.insert_formatted(tkey, Item::Value(tval.clone()));
        self.log.push(format!("added {path}"));
      }
      Some(Item::Value(Value::Array(existing))) if tval.is_array() => {
        if path == "tool.poe.include_script" {
          let removed = remove_legacy_include_scripts(existing);
          if removed > 0 {
            self.log.push(format!("{path}: removed {removed} legacy poe_tasks include"));
          }
        }
        let added = if path.starts_with("dependency-groups.") || path == "project.dependencies" {
          union_dependencies(existing, tval.as_array().unwrap())
        } else {
          union_array(existing, tval.as_array().unwrap())
        };
        if !added.is_empty() {
          existing
            .decor_mut()
            .set_suffix(format!("{ADDED_COMMENT_PREFIX}{}", added.join(", ")));
          self.log.push(format!("{path}: added {}", added.join(", ")));
        }
      }
      Some(Item::Value(existing)) => {
        if canonical(existing) != canonical(tval) {
          let mut new = tval.clone();
          // keep the project's surrounding whitespace so alignment survives
          *new.decor_mut() = existing.decor().clone();
          *existing = new;
          self.log.push(format!("set {path}"));
        }
      }
      Some(_) => {
        // Template value vs project table — leave the project's structure alone.
        self
          .log
          .push(format!("skipped {path}: project uses a table where the template has a value"));
      }
    }
  }
}

/// Drop the `setup-project:` marker lines from a table's leading comment block, keeping
/// every other comment (and the blank-line spacing) exactly as the template wrote it.
fn strip_marker_comments(t: &mut Table) {
  // Build the replacement prefix *first*. `kept.join` returns an owned `String`, so the
  // immutable borrow of `t` taken by `decor()` has ended by the time `decor_mut()` needs a
  // mutable one — doing it in one expression would fail the borrow check.
  //
  // `split` on the newline char (not `lines()`) keeps the leading and trailing empty
  // pieces, so the blank line separating this table from the one above it survives.
  let cleaned = t
    .decor()
    .prefix()
    .and_then(|p| p.as_str())
    .filter(|p| p.contains(MARKER))
    .map(|prefix| prefix.split('\n').filter(|l| !is_marker_line(l)).collect::<Vec<_>>().join("\n"));
  if let Some(cleaned) = cleaned {
    t.decor_mut().set_prefix(cleaned);
  }
}

/// Whether a raw decor line is a `# setup-project: ...` marker.
fn is_marker_line(line: &str) -> bool {
  line.trim().trim_start_matches('#').trim().starts_with(MARKER)
}

/// The comment lines directly above a template table, with `#` and whitespace stripped.
fn marker_lines(template: &Table, key: &str) -> Vec<String> {
  let Some(Item::Table(t)) = template.get(key) else {
    return Vec::new();
  };
  let Some(prefix) = t.decor().prefix().and_then(|p| p.as_str()) else {
    return Vec::new();
  };
  prefix
    .lines()
    .map(|l| l.trim().trim_start_matches('#').trim().to_string())
    .collect()
}

/// `# setup-project: if-dep NAME` in the comment block directly above a template table.
fn conditional_dep(template: &Table, key: &str) -> Option<String> {
  marker_lines(template, key)
    .iter()
    .find_map(|l| l.strip_prefix(IF_DEP_MARKER).map(|d| d.trim().to_string()))
}

/// `# setup-project: if-docker` above a template table: merge only into projects that
/// actually have a Docker setup (see `ProjectContext::has_docker`).
fn conditional_docker(template: &Table, key: &str) -> bool {
  marker_lines(template, key).iter().any(|l| l == IF_DOCKER_MARKER)
}

fn has_only_subtables(t: &Table) -> bool {
  t.iter().all(|(_, i)| matches!(i, Item::Table(_)))
}

/// Comparison key: the value rendered without any decor or whitespace.
fn canonical(v: &Value) -> String {
  let mut c = v.clone();
  strip_decor(&mut c);
  c.to_string().chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn strip_decor(v: &mut Value) {
  v.decor_mut().clear();
  match v {
    Value::Array(a) => {
      for x in a.iter_mut() {
        strip_decor(x);
      }
    }
    Value::InlineTable(t) => {
      for (mut k, x) in t.iter_mut() {
        k.leaf_decor_mut().clear();
        strip_decor(x);
      }
    }
    _ => {}
  }
}

/// Drop `include_script` entries that point at the pre-rename `poe_tasks:tasks` module so
/// the template's `aeth_devkit:tasks` entry replaces rather than joins them.
fn remove_legacy_include_scripts(existing: &mut Array) -> usize {
  let legacy = |v: &Value| -> bool {
    let script = match v {
      Value::InlineTable(t) => t.get("script").and_then(Value::as_str),
      Value::String(s) => Some(s.value().as_str()),
      _ => None,
    };
    script.is_some_and(|s| s == "poe_tasks:tasks")
  };
  let before = existing.len();
  existing.retain(|v| !legacy(v));
  before - existing.len()
}

/// Append template elements missing from `existing`; returns their rendered forms.
fn union_array(existing: &mut Array, template: &Array) -> Vec<String> {
  let have: Vec<String> = existing.iter().map(canonical).collect();
  let mut added = Vec::new();
  for v in template.iter() {
    if !have.contains(&canonical(v)) {
      push_like_last(existing, v.clone());
      added.push(display(v));
    }
  }
  added
}

/// Dependency arrays: match by package name; replace the specifier, else append.
fn union_dependencies(existing: &mut Array, template: &Array) -> Vec<String> {
  let mut added = Vec::new();
  for v in template.iter() {
    let Some(spec) = v.as_str() else { continue };
    let name = dependency_name(spec);
    let pos = existing.iter().position(|e| e.as_str().is_some_and(|s| dependency_name(s) == name));
    match pos {
      Some(i) => {
        let cur = existing.get(i).unwrap();
        let cur_str = cur.as_str().unwrap_or("?").to_string();
        if cur_str != spec {
          let mut new = Value::from(spec);
          *new.decor_mut() = cur.decor().clone();
          existing.replace(i, new);
          added.push(format!("{} (was {cur_str})", display(v)));
        }
      }
      None => {
        push_like_last(existing, v.clone());
        added.push(display(v));
      }
    }
  }
  added
}

/// Push with the same leading whitespace as the last element so multi-line arrays stay tidy.
fn push_like_last(arr: &mut Array, mut v: Value) {
  let prefix = arr
    .iter()
    .last()
    .and_then(|l| l.decor().prefix().and_then(|p| p.as_str()).map(str::to_string));
  // Multi-line arrays carry a "\n    " prefix on each element; single-line ones carry
  // nothing (the first element) or " ". A new element wants at least one space, except
  // as the first element of an (emptied) array.
  let prefix = match prefix {
    Some(p) if !p.trim_matches(' ').is_empty() => p,
    _ if arr.is_empty() => String::new(),
    _ => " ".to_string(),
  };
  v.decor_mut().set_prefix(prefix);
  v.decor_mut().set_suffix("");
  arr.push_formatted(v);
}

fn display(v: &Value) -> String {
  let mut c = v.clone();
  strip_decor(&mut c);
  c.to_string().trim().to_string()
}

/// Drop `tool.ruff.extend` / `tool.pyright.extends` that point at a parent pyproject.
fn remove_extends(doc: &mut DocumentMut, log: &mut Vec<String>) {
  for (tool, key) in [("ruff", "extend"), ("pyright", "extends")] {
    let Some(table) = doc.get_mut("tool").and_then(|t| t.get_mut(tool)).and_then(Item::as_table_mut) else {
      continue;
    };
    let points_at_parent = table.get(key).and_then(Item::as_str).is_some_and(|s| s.ends_with("pyproject.toml"));
    if points_at_parent {
      table.remove(key);
      log.push(format!("removed tool.{tool}.{key}"));
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(deps: &[&str]) -> ProjectContext {
    ProjectContext {
      root: std::path::PathBuf::from("D:/proj"),
      package: "proj".into(),
      dependencies: deps.iter().map(|d| d.to_string()).collect::<HashSet<_>>(),
      has_docker: false,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec![],
      docker_legacy_keys: vec![],
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
    }
  }

  #[test]
  fn scalars_replace_arrays_union_with_comment() {
    let orig = "[tool.ruff]\n  extend    = \"../pyproject.toml\"\n  cache-dir = \".ruff_cache\"\n\n  [tool.ruff.lint]\n    extend-select = [\n      \"A\",\n      \"B\",\n    ]\n";
    let tpl =
      "[tool.ruff]\n  cache-dir = \".cache/ruff\"\n  [tool.ruff.lint]\n    extend-select = [\"A\", \"C4\"]\n    ignore = [\"X\"]\n";
    let mut log = vec![];
    let out = merge_pyproject(orig, tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("cache-dir = \".cache/ruff\""), "{out}");
    assert!(!out.contains("extend    ="), "{out}");
    assert!(out.contains("\"C4\",\n    ] # setup-project added: \"C4\""), "{out}");
    assert!(out.contains("ignore = [\"X\"]"), "{out}");
    // idempotent
    let mut log2 = vec![];
    let again = merge_pyproject(&out, tpl, &ctx(&[]), &mut log2).unwrap();
    assert_eq!(again, out);
    assert!(log2.is_empty(), "{log2:?}");
  }

  #[test]
  fn keep_list_and_conditional_tables() {
    let orig = "[tool.setup-project]\n  keep = [\"tool.pyright.strict\"]\n[tool.pyright]\n  strict = false\n";
    let tpl = "[tool.pyright]\n  strict = true\n\n# setup-project: if-dep mypy\n[tool.mypy]\n  cache_dir = \".cache/mypy\"\n";
    let mut log = vec![];
    let out = merge_pyproject(orig, tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("strict = false"));
    assert!(!out.contains("tool.mypy"));
    let out2 = merge_pyproject(orig, tpl, &ctx(&["mypy"]), &mut vec![]).unwrap();
    assert!(out2.contains("[tool.mypy]"), "{out2}");
  }

  #[test]
  fn dependencies_match_by_name() {
    let orig = "[dependency-groups]\n  dev = [\n    \"poe-tasks>=4.0.0\",\n    \"pyright>=1.1.400\",\n  ]\n";
    let tpl = "[dependency-groups]\n  dev = [\"poethepoet>=0.46.0\", \"pyright>=1.1.411\"]\n";
    let mut log = vec![];
    let out = merge_pyproject(orig, tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("\"pyright>=1.1.411\""), "{out}");
    assert!(!out.contains("\"pyright>=1.1.400\""), "{out}");
    assert!(out.contains("\"poe-tasks>=4.0.0\""));
    assert!(out.contains("\"poethepoet>=0.46.0\""));
  }
}

#[cfg(test)]
mod docker_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(has_docker: bool) -> ProjectContext {
    ProjectContext {
      root: std::path::PathBuf::from("D:/proj"),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: if has_docker { vec!["proj".into()] } else { vec![] },
      docker_legacy_keys: vec![],
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
    }
  }

  const TPL: &str = "[tool.pyright]\n  strict = true\n\n# setup-project: if-docker\n[tool.docker]\n  mkdirs = []\n";

  #[test]
  fn if_docker_table_is_skipped_without_a_docker_setup() {
    let mut log = vec![];
    let out = merge_pyproject("[project]\nname = \"p\"\n", TPL, &ctx(false), &mut log).unwrap();
    assert!(!out.contains("[tool.docker]"), "{out}");
    assert!(out.contains("[tool.pyright]"), "{out}");
  }

  #[test]
  fn if_docker_table_is_merged_with_a_docker_setup() {
    let mut log = vec![];
    let out = merge_pyproject("[project]\nname = \"p\"\n", TPL, &ctx(true), &mut log).unwrap();
    assert!(out.contains("[tool.docker]"), "{out}");
  }

  #[test]
  fn the_marker_comment_never_reaches_the_project() {
    // The marker is an instruction to the merger. Shipping it leaves a comment in the
    // project's pyproject.toml that reads like a live directive but is only ever honoured
    // on the template side — and it would persist through every later run.
    let mut log = vec![];
    let out = merge_pyproject("[project]\nname = \"p\"\n", TPL, &ctx(true), &mut log).unwrap();
    assert!(out.contains("[tool.docker]"), "{out}");
    assert!(!out.contains("setup-project:"), "marker leaked:\n{out}");
  }

  #[test]
  fn stripping_the_marker_keeps_the_other_comments_and_spacing() {
    const TPL2: &str =
      "[tool.pyright]\n  strict = true\n\n# Keep me: explains the table.\n# setup-project: if-docker\n[tool.docker]\n  mkdirs = []\n";
    let mut log = vec![];
    let out = merge_pyproject("[project]\nname = \"p\"\n", TPL2, &ctx(true), &mut log).unwrap();
    assert!(out.contains("# Keep me: explains the table."), "{out}");
    assert!(!out.contains("setup-project:"), "{out}");
    // The blank line separating the table from its predecessor must survive the strip.
    assert!(out.contains("\n\n# Keep me"), "spacing collapsed:\n{out}");
  }
}
