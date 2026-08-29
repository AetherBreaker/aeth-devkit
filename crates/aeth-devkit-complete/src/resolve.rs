//! Resolve the poe task table the way poe does: `[tool.poe.tasks]`, then `include`d TOML
//! files, then `include_script` output.
//!
//! The first two are plain config and are mirrored here in Rust. The third is not
//! mirrorable — the task table is the *return value of a Python program* — so we run the
//! exact one-liner poe runs, but against the venv's interpreter directly, skipping the
//! poethepoet framework import and the `uv run` executor that account for most of poe's
//! ~200 ms per call.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use aeth_devkit_core::process::Runner;

/// One argument of a task, in the shape `poe _describe_task_args` prints.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskArg {
  pub name: String,
  pub options: Vec<String>,
  /// `boolean`, `string`, `integer`, `float`, or `positional`.
  pub kind: String,
  pub help: String,
  pub choices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Task {
  pub name: String,
  pub args: Vec<TaskArg>,
}

/// The resolved task table plus every file it was derived from, so a cache can be keyed on
/// their modification times.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Resolved {
  pub tasks: Vec<Task>,
  pub sources: Vec<PathBuf>,
}

/// Every task poe would offer, in definition order, hidden (`_`-prefixed) ones excluded.
pub fn resolve(root: &Path, runner: &dyn Runner) -> Result<Resolved> {
  let pyproject = root.join("pyproject.toml");
  let text = std::fs::read_to_string(&pyproject).with_context(|| format!("reading {}", pyproject.display()))?;
  let doc: toml_edit::DocumentMut = text.parse().context("parsing pyproject.toml")?;
  // `tool.poe` may be absent entirely; an empty object keeps the rest of the code uniform.
  let poe = doc
    .get("tool")
    .and_then(|t| t.get("poe"))
    .map(toml_to_json)
    .unwrap_or(Value::Object(Default::default()));

  let mut out = Resolved {
    tasks: Vec::new(),
    sources: vec![pyproject],
  };
  let mut seen = HashSet::new();
  add_tasks(&mut out.tasks, &poe, &mut seen);

  // `include` — TOML files, recursively. Paths resolve against the including file's
  // directory, exactly as poe does, so nested config directories work.
  let mut visited: HashSet<PathBuf> = HashSet::new();
  for item in include_items(&poe) {
    load_include(root, root, &item, true, &mut out, &mut seen, &mut visited);
  }

  // `include_script` — Python. Each failure degrades to "no tasks from this script".
  for item in include_script_items(&poe) {
    if let Some(json) = run_include_script(root, &item, runner) {
      let (cfg, config_path) = normalize_script_output(json);
      if let Some(p) = config_path {
        out.sources.push(p);
      }
      add_tasks(&mut out.tasks, &cfg, &mut seen);
    }
  }
  Ok(out)
}

/// Append every task under `poe["tasks"]` not yet seen. Hidden `_`-prefixed tasks are
/// skipped the way `poe _list_tasks` skips them.
fn add_tasks(tasks: &mut Vec<Task>, poe: &Value, seen: &mut HashSet<String>) {
  let Some(table) = poe.get("tasks").and_then(Value::as_object) else {
    return;
  };
  for (name, def) in table {
    if name.is_empty() || name.starts_with('_') || !seen.insert(name.clone()) {
      continue;
    }
    tasks.push(Task {
      name: name.clone(),
      args: def.get("args").map(normalize_args).unwrap_or_default(),
    });
  }
}

/// Mirror of poe's `ArgSpec.normalize` plus `_get_arg_options_list`: `args` may be a list of
/// names or tables, or a table keyed by name.
fn normalize_args(args: &Value) -> Vec<TaskArg> {
  let mut out = Vec::new();
  let mut push = |name: &str, params: &Value| {
    let stripped = name.trim_start_matches('_');
    let positional = params.get("positional").cloned().unwrap_or(Value::Bool(false));
    let (options, kind) = match &positional {
      // `positional = "WHERE"` names the placeholder; `positional = true` uses the name.
      Value::String(s) => (vec![s.clone()], "positional".to_string()),
      Value::Bool(true) => (
        vec![if stripped.is_empty() { name } else { stripped }.to_string()],
        "positional".to_string(),
      ),
      _ => {
        let options = match params.get("options").and_then(Value::as_array) {
          Some(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
          None => vec![format!("--{stripped}")],
        };
        let kind = params.get("type").and_then(Value::as_str).unwrap_or("string").to_string();
        (options, kind)
      }
    };
    out.push(TaskArg {
      name: name.to_string(),
      options,
      kind,
      help: params.get("help").and_then(Value::as_str).unwrap_or("").to_string(),
      choices: params
        .get("choices")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(json_scalar_string).collect())
        .unwrap_or_default(),
    });
  };
  match args {
    Value::Array(items) => {
      for item in items {
        match item {
          Value::String(name) => push(name, &Value::Object(Default::default())),
          Value::Object(o) => {
            if let Some(name) = o.get("name").and_then(Value::as_str) {
              push(name, item);
            }
          }
          _ => {}
        }
      }
    }
    Value::Object(table) => {
      for (name, params) in table {
        push(name, params);
      }
    }
    _ => {}
  }
  out
}

/// `include` normalized to `[{path, cwd?, recursive?}]` like poe's `_normalize_included_config_path`.
fn include_items(poe: &Value) -> Vec<Value> {
  match poe.get("include") {
    Some(Value::String(s)) => vec![serde_json::json!({ "path": s })],
    Some(Value::Object(_)) => vec![poe["include"].clone()],
    Some(Value::Array(a)) => a
      .iter()
      .filter_map(|i| match i {
        Value::String(s) => Some(serde_json::json!({ "path": s })),
        Value::Object(_) => Some(i.clone()),
        _ => None,
      })
      .collect(),
    _ => Vec::new(),
  }
}

/// Load one included file and, unless `recursive = false`, its own includes. `ancestors`
/// is the cycle guard: a file already on the current include chain is skipped.
fn load_include(
  root: &Path,
  source_dir: &Path,
  item: &Value,
  parent_recursive: bool,
  out: &mut Resolved,
  seen: &mut HashSet<String>,
  ancestors: &mut HashSet<PathBuf>,
) {
  let Some(rel) = item.get("path").and_then(Value::as_str) else {
    return;
  };
  // `${POE_ROOT}` and `${POE_CONF_DIR}` are the two path variables poe expands here.
  let rel = rel
    .replace("${POE_ROOT}", &root.to_string_lossy())
    .replace("${POE_CONF_DIR}", &source_dir.to_string_lossy());
  let path = source_dir.join(rel);
  let key = path.canonicalize().unwrap_or(path.clone());
  if !ancestors.insert(key.clone()) {
    return;
  }
  // A missing include is a warning in poe, not an error: keep going with what we have.
  if let Ok(text) = std::fs::read_to_string(&path)
    && let Ok(doc) = text.parse::<toml_edit::DocumentMut>()
  {
    out.sources.push(path.clone());
    let poe = doc
      .get("tool")
      .and_then(|t| t.get("poe"))
      .map(toml_to_json)
      .unwrap_or(Value::Object(Default::default()));
    add_tasks(&mut out.tasks, &poe, seen);
    let recursive = item.get("recursive").and_then(Value::as_bool).unwrap_or(true);
    if parent_recursive && recursive {
      let dir = path.parent().unwrap_or(source_dir).to_path_buf();
      for child in include_items(&poe) {
        load_include(root, &dir, &child, true, out, seen, ancestors);
      }
    }
  }
  ancestors.remove(&key);
}

/// `include_script` normalized to `[{script, cwd?}]`.
fn include_script_items(poe: &Value) -> Vec<Value> {
  match poe.get("include_script") {
    Some(Value::String(s)) => vec![serde_json::json!({ "script": s })],
    Some(Value::Array(a)) => a
      .iter()
      .filter_map(|i| match i {
        Value::String(s) => Some(serde_json::json!({ "script": s })),
        Value::Object(_) => Some(i.clone()),
        _ => None,
      })
      .collect(),
    _ => Vec::new(),
  }
}

/// The interpreter to run `include_script` with: the venv's own, else `python` on PATH.
fn python_for(root: &Path) -> String {
  let venv = root.join(".venv");
  // Join component-by-component so the result uses native separators throughout; joining
  // a `/`-containing string onto a Windows path would yield a mixed-separator string.
  for p in [venv.join("Scripts").join("python.exe"), venv.join("bin").join("python")] {
    if p.is_file() {
      return p.to_string_lossy().into_owned();
    }
  }
  "python".to_string()
}

/// Run poe's own include_script one-liner and return its stdout as JSON, or `None` on any
/// failure. The script text is kept identical to poe's so the two can never disagree about
/// what an `include_script` *means* — only about how fast it is obtained.
fn run_include_script(root: &Path, item: &Value, runner: &dyn Runner) -> Option<Value> {
  let spec = item.get("script")?.as_str()?;
  let (module, call) = spec.split_once(':')?;
  // `pkg:tasks` means call `tasks()`; `pkg:tasks(x)` is already a call expression.
  let call = if call.contains('(') {
    call.to_string()
  } else {
    format!("{call}()")
  };
  let src = root.join("src");
  let src_append = if src.is_dir() {
    format!("sys.path.append({});", py_repr(&src.to_string_lossy()))
  } else {
    String::new()
  };
  let script = format!(
    "import os,sys,json;environ=os.environ;from importlib import import_module as _i;{src_append}\
     _o=sys.stdout;sys.stdout=sys.stderr;_m = _i('{module}');sys.stdout=_o;print(json.dumps(_m.{call}));"
  );
  let cwd = match item.get("cwd").and_then(Value::as_str) {
    Some(c) => root.join(c),
    None => root.to_path_buf(),
  };
  let out = runner.run_capture(&python_for(root), &["-c".to_string(), script], &cwd).ok()?;
  if !out.success() {
    return None;
  }
  let mut json: Value = serde_json::from_str(out.stdout.trim()).ok()?;
  // poe tolerates a JSON *string* containing JSON; so do we.
  if let Value::String(inner) = &json {
    json = serde_json::from_str(inner).ok()?;
  }
  Some(json)
}

/// Python's `repr()` for a str, enough for a path: single-quoted with `\` and `'` escaped.
fn py_repr(s: &str) -> String {
  format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// poe's three accepted shapes for script output, reduced to the `tool.poe` body, plus the
/// `config_path` the script reported (the module file, for cache invalidation).
fn normalize_script_output(mut json: Value) -> (Value, Option<PathBuf>) {
  let config_path = json
    .as_object_mut()
    .and_then(|o| o.remove("config_path"))
    .and_then(|v| v.as_str().map(PathBuf::from));
  let body = if json.get("tool").and_then(|t| t.get("poe")).is_some() {
    json["tool"]["poe"].clone()
  } else if let Some(v) = json.get("tool.poe") {
    v.clone()
  } else {
    json
  };
  (body, config_path)
}

/// Convert a `toml_edit` item to `serde_json::Value` so TOML and script JSON share one path.
fn toml_to_json(item: &toml_edit::Item) -> Value {
  use toml_edit::Item;
  match item {
    Item::None => Value::Null,
    Item::Value(v) => toml_value_to_json(v),
    Item::Table(t) => Value::Object(t.iter().map(|(k, v)| (k.to_string(), toml_to_json(v))).collect()),
    Item::ArrayOfTables(a) => Value::Array(a.iter().map(|t| toml_to_json(&Item::Table(t.clone()))).collect()),
  }
}

fn toml_value_to_json(v: &toml_edit::Value) -> Value {
  use toml_edit::Value as T;
  match v {
    T::String(s) => Value::String(s.value().clone()),
    T::Integer(i) => Value::from(*i.value()),
    T::Float(f) => serde_json::Number::from_f64(*f.value()).map(Value::Number).unwrap_or(Value::Null),
    T::Boolean(b) => Value::Bool(*b.value()),
    T::Datetime(d) => Value::String(d.value().to_string()),
    T::Array(a) => Value::Array(a.iter().map(toml_value_to_json).collect()),
    T::InlineTable(t) => Value::Object(t.iter().map(|(k, v)| (k.to_string(), toml_value_to_json(v))).collect()),
  }
}

/// A choice may be a number in TOML; the completion text is its string form.
fn json_scalar_string(v: &Value) -> String {
  match v {
    Value::String(s) => s.clone(),
    other => other.to_string(),
  }
}
