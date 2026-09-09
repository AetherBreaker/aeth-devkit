//! Comment-preserving deep merge of the template `pyproject.toml` into a project's.

use anyhow::{Context as _, Result, bail};
use toml_edit::{Array, DocumentMut, Item, Table, Value};

use crate::context::{ProjectContext, dependency_name};

const ADDED_COMMENT_PREFIX: &str = " # setup-project added: ";
/// Common prefix of every marker comment, used to strip them all from a table copied whole.
const MARKER: &str = "setup-project:";
const IF_DEP_MARKER: &str = "setup-project: if-dep ";
const IF_DOCKER_MARKER: &str = "setup-project: if-docker";
/// Narrower than `if-docker`: only a project whose `[tool.docker].services` names something.
/// `if-docker` also fires on bare Docker files so the `[tool.docker]` switch gets seeded; a
/// runtime dependency must wait for the switch to be set.
const IF_DOCKER_SERVICES_MARKER: &str = "setup-project: if-docker-services";

/// In a template specifier, "the newest release this devkit can use". The merge only makes
/// sure the package is listed; `packages::advance` writes the real floor once uv has chosen
/// the version under the running-devkit constraint (spec 4.0), so the placeholder never
/// reaches a project file.
pub const LATEST: &str = "{latest}";

pub fn merge_pyproject(original: &str, template: &str, ctx: &ProjectContext, log: &mut Vec<String>) -> Result<String> {
  let mut doc: DocumentMut = original.parse().context("parsing project pyproject.toml")?;
  let tpl: DocumentMut = template.parse().context("parsing template pyproject.toml")?;
  check_markers(tpl.as_table(), "")?;

  let mut merger = Merger { ctx, log };
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

/// Every `setup-project:` marker in the template must be one the merger acts on: an
/// unknown one (a typo such as `if-docker-service`) would otherwise gate nothing, and the
/// table it was meant to guard would merge into every project. The template is devkit's own
/// file, so this is a devkit bug surfaced at the first run, not a user error.
fn check_markers(template: &Table, path: &str) -> Result<()> {
  for (key, item) in template.iter() {
    let child = if path.is_empty() {
      key.to_string()
    } else {
      format!("{path}.{key}")
    };
    for line in marker_lines(template, key) {
      let known = line == IF_DOCKER_MARKER || line == IF_DOCKER_SERVICES_MARKER || line.starts_with(IF_DEP_MARKER);
      if line.starts_with(MARKER) && !known {
        bail!("pyproject template: unknown marker `# {line}` above {child}");
      }
    }
    if let Item::Table(t) = item {
      check_markers(t, &child)?;
    }
  }
  Ok(())
}

struct Merger<'a> {
  ctx: &'a ProjectContext,
  log: &'a mut Vec<String>,
}

impl Merger<'_> {
  /// Whether the marker above `key` in `template` keeps it out of this project: `if-dep X`
  /// without the dependency, `if-docker` without Docker (or its files), `if-docker-services`
  /// without services. Applies to table headers and key-value lines alike.
  fn gated_off(&self, template: &Table, key: &str) -> bool {
    if let Some(dep) = conditional_dep(template, key)
      && !self.ctx.has_dependency(&dep)
    {
      return true;
    }
    if conditional_docker(template, key) && !(self.ctx.has_docker || self.ctx.docker_files) {
      return true;
    }
    marker_lines(template, key).iter().any(|l| l == IF_DOCKER_SERVICES_MARKER) && !self.ctx.has_docker
  }

  fn merge_table(&mut self, target: &mut Table, template: &Table, path: &str) {
    for (key, titem) in template.iter() {
      let child = if path.is_empty() {
        key.to_string()
      } else {
        format!("{path}.{key}")
      };
      if self.gated_off(template, key) {
        continue;
      }
      let tkey = template.key(key).expect("iterating template keys").clone();
      match titem {
        Item::Table(ttable) => {
          // A table the project wrote inline (`sources = { … }`) is real content: promoted
          // to a header table so the merge adds to it, where a fresh copy would replace it.
          if let Some(Item::Value(Value::InlineTable(inline))) = target.get(key) {
            target.insert(key, Item::Table(inline.clone().into_table()));
            self.log.push(format!("[{child}]: inline table rewritten as a table"));
          }
          let needs_insert = !matches!(target.get(key), Some(Item::Table(_)));
          if needs_insert {
            if ttable.is_implicit() || has_only_subtables(ttable) {
              // A missing intermediate table (e.g. `tool`) is created implicit, so no bare
              // header is emitted, and recursed into so the conditional rules still apply
              // to its children; the block after this makes it explicit when the template
              // writes the header.
              let mut t = Table::new();
              t.set_implicit(true);
              target.insert_formatted(&tkey, Item::Table(t));
            } else {
              // A brand-new leaf table: copy it so the template's formatting (indentation,
              // alignment, comments) is preserved — minus the keys the markers gate off
              // for this project and the marker lines themselves, the same treatment the
              // key-by-key merge below gives an existing table.
              let mut fresh = ttable.clone();
              strip_marker_lines(fresh.decor_mut());
              fresh.retain(|k, _| !self.gated_off(ttable, k));
              for (mut key, _) in fresh.iter_mut() {
                strip_marker_lines(key.leaf_decor_mut());
              }
              scrub_latest_table(&mut fresh);
              target.insert_formatted(&tkey, Item::Table(fresh));
              self.log.push(format!("added [{child}]"));
              continue;
            }
          }
          let sub = target.get_mut(key).and_then(Item::as_table_mut).expect("just inserted");
          // A header the template writes over sub-tables only (`[tool.coverage]`) is there
          // for tombi to nest the children under; a project whose table exists only through
          // its children (or was just created above) gets the header, with the template's
          // decor, and reports it.
          if !ttable.is_implicit() && sub.is_implicit() {
            sub.set_implicit(false);
            *sub.decor_mut() = ttable.decor().clone();
            strip_marker_lines(sub.decor_mut());
            self.log.push(format!("added [{child}]"));
          }
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
        // Carry the template key's decor (indentation) so the new line matches its
        // neighbours, minus any marker that gated it.
        let mut key = tkey.clone();
        strip_marker_lines(key.leaf_decor_mut());
        let mut fresh = tval.clone();
        if let Value::Array(a) = &mut fresh {
          scrub_latest_array(a);
        }
        target.insert_formatted(&key, Item::Value(fresh));
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

/// Drop the `setup-project:` marker lines from a decor's prefix — the comment block above a
/// table header or a key — keeping the other comments and the blank line above them. The
/// markers are instructions *to* this merger; shipped, one would read like a live directive
/// in the project's file while only ever being honoured on the template side.
fn strip_marker_lines(decor: &mut toml_edit::Decor) {
  // Build the replacement first: the immutable borrow ends before `set_prefix` needs a
  // mutable one. `split` on the newline char (not `lines()`) keeps the leading and trailing
  // empty pieces, so the blank line separating a table from the one above it survives.
  let cleaned = decor
    .prefix()
    .and_then(|p| p.as_str())
    .filter(|p| p.contains(MARKER))
    .map(|prefix| prefix.split('\n').filter(|l| !is_marker_line(l)).collect::<Vec<_>>().join("\n"));
  if let Some(cleaned) = cleaned {
    decor.set_prefix(cleaned);
  }
}

/// Whether a raw decor line is a `# setup-project: ...` marker.
fn is_marker_line(line: &str) -> bool {
  line.trim().trim_start_matches('#').trim().starts_with(MARKER)
}

/// The comment lines directly above a template table header or key-value line, with `#`
/// and whitespace stripped.
fn marker_lines(template: &Table, key: &str) -> Vec<String> {
  let prefix = match template.get(key) {
    Some(Item::Table(t)) => t.decor().prefix(),
    Some(_) => template.key(key).and_then(|k| k.leaf_decor().prefix()),
    None => None,
  };
  prefix
    .and_then(|p| p.as_str())
    .map(|p| p.lines().map(|l| l.trim().trim_start_matches('#').trim().to_string()).collect())
    .unwrap_or_default()
}

/// `# setup-project: if-dep NAME` in the comment block directly above a template table.
fn conditional_dep(template: &Table, key: &str) -> Option<String> {
  marker_lines(template, key)
    .iter()
    .find_map(|l| l.strip_prefix(IF_DEP_MARKER).map(|d| d.trim().to_string()))
}

/// `# setup-project: if-docker` above a template table: merge only into projects with a
/// Docker setup — a listed service, or Docker files on disk that seed the table.
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

/// The template's `name>={latest}` entries in an array copied into a project as is, made
/// bare names: what the union does for an existing array (see [`union_dependencies`]), so
/// the placeholder never reaches a project file whichever path adds the array.
fn scrub_latest_array(arr: &mut Array) {
  for i in 0..arr.len() {
    let Some(spec) = arr.get(i).and_then(Value::as_str) else { continue };
    if spec.contains(LATEST) {
      let mut bare = Value::from(dependency_name(spec));
      *bare.decor_mut() = arr.get(i).unwrap().decor().clone();
      arr.replace(i, bare);
    }
  }
}

/// [`scrub_latest_array`] over every array in a table copied whole, sub-tables included
/// (`[dependency-groups]` holds one array per group).
fn scrub_latest_table(t: &mut Table) {
  for (_, item) in t.iter_mut() {
    match item {
      Item::Value(Value::Array(a)) => scrub_latest_array(a),
      Item::Table(sub) => scrub_latest_table(sub),
      _ => {}
    }
  }
}

/// Dependency arrays: match by package name; replace the specifier, else append. A
/// [`LATEST`] specifier is the exception: the project's own floor stays, and a missing
/// package is added by bare name for `packages::advance` to pin.
fn union_dependencies(existing: &mut Array, template: &Array) -> Vec<String> {
  let mut added = Vec::new();
  for v in template.iter() {
    let Some(spec) = v.as_str() else { continue };
    let name = dependency_name(spec);
    let pos = existing.iter().position(|e| e.as_str().is_some_and(|s| dependency_name(s) == name));
    if spec.contains(LATEST) {
      if pos.is_none() {
        let bare = Value::from(name);
        added.push(display(&bare));
        push_like_last(existing, bare);
      }
      continue;
    }
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
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
    }
  }

  #[test]
  fn a_latest_specifier_keeps_the_project_floor_and_adds_a_bare_name() {
    let tpl = "[project]\n  dependencies = [\"devkit-container>={latest}\", \"requests>=2\"]\n";
    let mut log = vec![];
    let out = merge_pyproject(
      "[project]\n  name = \"p\"\n  dependencies = [\"devkit-container>=1.2.0\"]\n",
      tpl,
      &ctx(&[]),
      &mut log,
    )
    .unwrap();
    assert!(out.contains("\"devkit-container>=1.2.0\""), "kept: {out}");
    assert!(!out.contains("{latest}"), "{out}");
    assert!(out.contains("\"requests>=2\""));
    let out = merge_pyproject("[project]\n  name = \"p\"\n  dependencies = []\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("\"devkit-container\""), "bare name: {out}");
    assert!(!out.contains("{latest}"), "{out}");
    let out = merge_pyproject("[project]\n  name = \"p\"\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(
      out.contains("\"devkit-container\"") && !out.contains("{latest}"),
      "no array yet: {out}"
    );
    // Whole tables copied from the template are scrubbed too, sub-tables included, and the
    // template's layout survives.
    let tpl = "[project]\n  dependencies = [\"devkit-container>={latest}\"]\n[dependency-groups]\n  dev = [\n    \"ruff>=0.15\",\n    \"devkit-templates>={latest}\",\n  ]\n";
    let out = merge_pyproject("[tool.x]\n  y = 1\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(out.contains("dependencies = [\"devkit-container\"]"), "{out}");
    assert!(out.contains("    \"devkit-templates\",\n"), "layout kept, name bare: {out}");
    assert!(!out.contains("{latest}"), "{out}");
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
  fn a_conditional_table_follows_the_dependency() {
    let orig = "[tool.pyright]\n  strict = false\n";
    let tpl = "[tool.pyright]\n  strict = true\n\n# setup-project: if-dep mypy\n[tool.mypy]\n  cache_dir = \".cache/mypy\"\n";
    let out = merge_pyproject(orig, tpl, &ctx(&[]), &mut vec![]).unwrap();
    assert!(out.contains("strict = true"), "{out}");
    assert!(!out.contains("tool.mypy"), "{out}");
    let out2 = merge_pyproject(orig, tpl, &ctx(&["mypy"]), &mut vec![]).unwrap();
    assert!(out2.contains("[tool.mypy]"), "{out2}");
  }

  #[test]
  fn an_explicit_parent_header_over_subtables_is_written() {
    // `[tool.coverage]` holds no keys of its own; the header exists so tombi nests the
    // sub-tables under it. A project without it gets it, whether the sub-tables are new or
    // already there, and the implicit `tool` parent still gets no header.
    let tpl = "[tool.coverage]\n  [tool.coverage.run]\n    data_file = \".cache/.coverage\"\n";
    let mut log = vec![];
    let fresh = merge_pyproject("[project]\n  name = \"p\"\n", tpl, &ctx(&[]), &mut log).unwrap();
    assert!(fresh.contains("\n[tool.coverage]\n"), "{fresh}");
    assert!(!fresh.contains("[tool]\n"), "{fresh}");
    assert!(log.contains(&"added [tool.coverage]".to_string()), "{log:?}");
    let mut log = vec![];
    let existing = merge_pyproject(
      "[tool.coverage.run]\n  data_file = \".cache/.coverage\"\n",
      tpl,
      &ctx(&[]),
      &mut log,
    )
    .unwrap();
    assert!(existing.contains("[tool.coverage]\n"), "{existing}");
    assert!(
      existing.find("[tool.coverage]").unwrap() < existing.find("[tool.coverage.run]").unwrap(),
      "{existing}"
    );
    assert_eq!(log, vec!["added [tool.coverage]"]);
    let mut log = vec![];
    assert_eq!(merge_pyproject(&existing, tpl, &ctx(&[]), &mut log).unwrap(), existing);
    assert!(log.is_empty(), "{log:?}");
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
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
    }
  }

  #[test]
  fn a_marker_above_a_value_gates_that_key_only() {
    let tpl = "[tool.uv.sources]\n  # setup-project: if-docker-services\n  devkit-container = [{ index = \"SFTPyPI\" }]\n  devkit-claude-hooks = [{ index = \"SFTPyPI\" }]\n";
    let orig = "[project]\n  name = \"p\"\n";
    let out = merge_pyproject(orig, tpl, &ctx(false), &mut vec![]).unwrap();
    assert!(out.contains("devkit-claude-hooks = [{ index = \"SFTPyPI\" }]"), "{out}");
    assert!(!out.contains("devkit-container"), "{out}");
    assert!(!out.contains("setup-project:"), "the marker is an instruction to the merger: {out}");
    let out = merge_pyproject(orig, tpl, &ctx(true), &mut vec![]).unwrap();
    assert!(out.contains("devkit-container = [{ index = \"SFTPyPI\" }]"), "{out}");
    assert!(!out.contains("setup-project:"), "{out}");
    let bad = "[tool.uv.sources]\n  # setup-project: if-dockr\n  x = 1\n";
    let err = merge_pyproject(orig, bad, &ctx(false), &mut vec![]).unwrap_err().to_string();
    assert!(err.contains("unknown marker") && err.contains("tool.uv.sources.x"), "{err}");
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
  fn if_docker_services_needs_the_switch_not_just_docker_files() {
    const TPL2: &str = "# setup-project: if-docker\n[tool.docker]\n  services = []\n\n# setup-project: if-docker-services\n[tool.uv.sources]\n  devkit-container = [{ index = \"SFTPyPI\" }]\n";
    let mut log = vec![];
    let mut files_only = ctx(false);
    files_only.docker_files = true;
    let out = merge_pyproject("[project]\nname = \"p\"\n", TPL2, &files_only, &mut log).unwrap();
    assert!(out.contains("[tool.docker]"), "the switch is seeded: {out}");
    assert!(!out.contains("devkit-container"), "no dependency before the switch is set: {out}");
    let out = merge_pyproject("[project]\nname = \"p\"\n", TPL2, &ctx(true), &mut log).unwrap();
    assert!(out.contains("devkit-container"), "{out}");
  }

  #[test]
  fn an_inline_project_table_is_promoted_not_replaced() {
    const TPL2: &str = "# setup-project: if-docker-services\n[tool.uv.sources]\n  devkit-container = [{ index = \"SFTPyPI\" }]\n";
    let mut log = vec![];
    let out = merge_pyproject(
      "[project]\nname = \"p\"\n\n[tool.uv]\nsources = { my-lib = { git = \"https://x/y.git\" } }\n",
      TPL2,
      &ctx(true),
      &mut log,
    )
    .unwrap();
    let doc: DocumentMut = out.parse().unwrap();
    let sources = doc["tool"]["uv"]["sources"].as_table_like().unwrap();
    assert!(sources.contains_key("my-lib") && sources.contains_key("devkit-container"), "{out}");
  }

  #[test]
  fn an_unknown_marker_is_an_error() {
    let mut log = vec![];
    let err = merge_pyproject(
      "[project]\nname = \"p\"\n",
      "# setup-project: if-docker-service\n[tool.x]\n  a = 1\n",
      &ctx(true),
      &mut log,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("if-docker-service") && err.contains("tool.x"), "{err}");
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
