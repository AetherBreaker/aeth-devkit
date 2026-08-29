//! JSON / JSONC merging for `.vscode/*.json`.

use anyhow::{Context as _, Result};
use serde_json::{Map, Value};

/// Strip `//` and `/* */` comments (string-aware) and trailing commas.
pub fn strip_jsonc(text: &str) -> String {
  let mut out = String::with_capacity(text.len());
  let chars: Vec<char> = text.chars().collect();
  let mut i = 0;
  let mut in_str = false;
  while i < chars.len() {
    let c = chars[i];
    if in_str {
      out.push(c);
      if c == '\\' && i + 1 < chars.len() {
        out.push(chars[i + 1]);
        i += 2;
        continue;
      }
      if c == '"' {
        in_str = false;
      }
      i += 1;
      continue;
    }
    match c {
      '"' => {
        in_str = true;
        out.push(c);
        i += 1;
      }
      '/' if chars.get(i + 1) == Some(&'/') => {
        while i < chars.len() && chars[i] != '\n' {
          i += 1;
        }
      }
      '/' if chars.get(i + 1) == Some(&'*') => {
        i += 2;
        while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
          i += 1;
        }
        i += 2;
      }
      _ => {
        out.push(c);
        i += 1;
      }
    }
  }
  // Trailing commas: `,` followed only by whitespace and a closing bracket.
  let re = regex::Regex::new(r",(\s*[}\]])").unwrap();
  re.replace_all(&out, "$1").into_owned()
}

/// `//` comment lines that appear before the first key (VS Code's boilerplate header).
fn header_comments(text: &str) -> Vec<String> {
  let mut out = Vec::new();
  for line in text.lines() {
    let t = line.trim();
    if t.starts_with("//") {
      out.push(t.to_string());
    } else if t.starts_with('"') {
      break;
    }
  }
  out
}

fn parse(text: &str) -> Result<Value> {
  serde_json::from_str(&strip_jsonc(text)).context("parsing JSON")
}

/// Pretty-print with 4-space indent, re-inserting the header comments after the opening brace.
fn render(value: &Value, header: &[String], original: Option<&str>) -> Result<String> {
  let mut buf = Vec::new();
  let fmt = serde_json::ser::PrettyFormatter::with_indent(b"    ");
  let mut ser = serde_json::Serializer::with_formatter(&mut buf, fmt);
  serde::Serialize::serialize(value, &mut ser)?;
  let mut text = String::from_utf8(buf)?;
  if !header.is_empty() && text.starts_with('{') {
    let mut with_header = String::from("{\n");
    for h in header {
      with_header.push_str("    ");
      with_header.push_str(h);
      with_header.push('\n');
    }
    with_header.push_str(&text[2..]);
    text = with_header;
  }
  text.push('\n');
  if original.is_some_and(|o| o.contains("\r\n")) {
    text = text.replace('\n', "\r\n");
  }
  Ok(text)
}

/// Deep merge `template` into `target`: objects recurse, arrays union, scalars replace.
pub fn deep_merge(target: &mut Value, template: &Value, path: &str, log: &mut Vec<String>) {
  match (target, template) {
    (Value::Object(t), Value::Object(s)) => {
      for (k, v) in s {
        let child = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
        match t.get_mut(k) {
          Some(existing) => deep_merge(existing, v, &child, log),
          None => {
            log.push(format!("added {child}"));
            t.insert(k.clone(), v.clone());
          }
        }
      }
    }
    (Value::Array(t), Value::Array(s)) => {
      let mut added = Vec::new();
      for v in s {
        if !t.contains(v) {
          t.push(v.clone());
          added.push(v.to_string());
        }
      }
      if !added.is_empty() {
        log.push(format!("{path}: added {}", added.join(", ")));
      }
    }
    (t, s) => {
      if t != s {
        log.push(format!("set {path}"));
        *t = s.clone();
      }
    }
  }
}

/// Merge a template into an optional existing JSONC file; returns the new file text.
pub fn merge_json_file(original: Option<&str>, template: &str, log: &mut Vec<String>) -> Result<String> {
  let tpl = parse(template)?;
  let Some(original) = original else {
    log.push("created from template".into());
    return render(&tpl, &header_comments(template), None);
  };
  let mut doc = parse(original)?;
  deep_merge(&mut doc, &tpl, "", log);
  if log.is_empty() {
    return Ok(original.to_string());
  }
  render(&doc, &header_comments(original), Some(original))
}

fn is_python_launch(cfg: &Value) -> bool {
  let ty = cfg.get("type").and_then(Value::as_str).unwrap_or("");
  let req = cfg.get("request").and_then(Value::as_str).unwrap_or("launch");
  matches!(ty, "debugpy" | "python") && req == "launch"
}

/// Keys copied from the template config's `env` block into every Python launch config.
const LAUNCH_ENV_KEYS: [&str; 3] = ["PYTHONPYCACHEPREFIX", "PYTHONUNBUFFERED", "PYTHONSAFEPATH"];

fn template_env(template: &Value) -> Map<String, Value> {
  template
    .get("configurations")
    .and_then(Value::as_array)
    .and_then(|a| a.first())
    .and_then(|c| c.get("env"))
    .and_then(Value::as_object)
    .cloned()
    .unwrap_or_default()
}

/// Create `launch.json` from the template, or patch each Python launch config's
/// `envFile` / `env`. Collects every distinct `envFile` value into `env_files`.
pub fn patch_launch(original: Option<&str>, template: &str, env_files: &mut Vec<String>, log: &mut Vec<String>) -> Result<String> {
  let tpl = parse(template)?;
  let Some(original) = original else {
    log.push("created from template".into());
    collect_env_files(&tpl, env_files);
    return render(&tpl, &header_comments(template), None);
  };
  let mut doc = parse(original)?;
  let tenv = template_env(&tpl);
  if let Some(configs) = doc.get_mut("configurations").and_then(Value::as_array_mut) {
    for cfg in configs.iter_mut().filter(|c| is_python_launch(c)) {
      let name = cfg.get("name").and_then(Value::as_str).unwrap_or("?").to_string();
      let obj = cfg.as_object_mut().unwrap();
      if !obj.contains_key("envFile") {
        obj.insert("envFile".into(), Value::String("${workspaceFolder}/.env".into()));
        log.push(format!("{name}: added envFile"));
      }
      let env = obj.entry("env").or_insert_with(|| Value::Object(Map::new()));
      let env = env.as_object_mut().context("launch config 'env' is not an object")?;
      for key in LAUNCH_ENV_KEYS {
        if let Some(v) = tenv.get(key)
          && env.get(key) != Some(v)
        {
          env.insert(key.into(), v.clone());
          log.push(format!("{name}: set env.{key}"));
        }
      }
    }
  }
  collect_env_files(&doc, env_files);
  if log.is_empty() {
    return Ok(original.to_string());
  }
  render(&doc, &header_comments(original), Some(original))
}

fn collect_env_files(doc: &Value, env_files: &mut Vec<String>) {
  if let Some(configs) = doc.get("configurations").and_then(Value::as_array) {
    for cfg in configs {
      if let Some(f) = cfg.get("envFile").and_then(Value::as_str)
        && !env_files.iter().any(|e| e == f)
      {
        env_files.push(f.to_string());
      }
    }
  }
}

/// Set `options.env.PYTHONPYCACHEPREFIX` on every task.
pub fn patch_tasks(original: &str, launch_template: &str, log: &mut Vec<String>) -> Result<String> {
  let tpl = parse(launch_template)?;
  let Some(prefix) = template_env(&tpl).get("PYTHONPYCACHEPREFIX").cloned() else {
    return Ok(original.to_string());
  };
  let mut doc = parse(original)?;
  if let Some(tasks) = doc.get_mut("tasks").and_then(Value::as_array_mut) {
    for task in tasks.iter_mut() {
      let label = task.get("label").and_then(Value::as_str).unwrap_or("?").to_string();
      let Some(obj) = task.as_object_mut() else { continue };
      let options = obj.entry("options").or_insert_with(|| Value::Object(Map::new()));
      let Some(options) = options.as_object_mut() else { continue };
      let env = options.entry("env").or_insert_with(|| Value::Object(Map::new()));
      let Some(env) = env.as_object_mut() else { continue };
      if env.get("PYTHONPYCACHEPREFIX") != Some(&prefix) {
        env.insert("PYTHONPYCACHEPREFIX".into(), prefix.clone());
        log.push(format!("{label}: set options.env.PYTHONPYCACHEPREFIX"));
      }
    }
  }
  if log.is_empty() {
    return Ok(original.to_string());
  }
  render(&doc, &header_comments(original), Some(original))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn strips_comments_and_trailing_commas() {
    let src = "{\n  // c\n  \"a\": \"x // not a comment\", /* b */\n  \"list\": [1, 2,],\n}";
    let v: Value = serde_json::from_str(&strip_jsonc(src)).unwrap();
    assert_eq!(v["a"], "x // not a comment");
    assert_eq!(v["list"].as_array().unwrap().len(), 2);
  }

  #[test]
  fn deep_merge_semantics() {
    let mut t: Value = serde_json::from_str(r#"{"a":1,"o":{"x":1},"l":["p"]}"#).unwrap();
    let s: Value = serde_json::from_str(r#"{"a":2,"o":{"y":2},"l":["p","q"],"n":true}"#).unwrap();
    let mut log = vec![];
    deep_merge(&mut t, &s, "", &mut log);
    assert_eq!(
      t,
      serde_json::from_str::<Value>(r#"{"a":2,"o":{"x":1,"y":2},"l":["p","q"],"n":true}"#).unwrap()
    );
    assert_eq!(log.len(), 4);
  }

  #[test]
  fn header_preserved() {
    let orig = "{\n    // hello\n    \"a\": 1\n}\n";
    let mut log = vec![];
    let out = merge_json_file(Some(orig), "{\"b\": 2}", &mut log).unwrap();
    assert!(out.starts_with("{\n    // hello\n    \"a\": 1,\n    \"b\": 2\n}"), "{out}");
  }
}

/// Merge the `hooks` object of `.claude/settings.json`: groups keyed by `matcher`, entries
/// within a group keyed by the `hook <name>` token in the command.
pub fn merge_hooks(target: &mut Value, template: &Value, log: &mut Vec<String>) {
  let Some(tmpl) = template.as_object() else { return };
  if !target.is_object() {
    *target = Value::Object(Map::new());
  }
  let tobj = target.as_object_mut().unwrap();
  for (event, groups) in tmpl {
    let Some(groups) = groups.as_array() else { continue };
    let existing = tobj.entry(event.clone()).or_insert_with(|| Value::Array(Vec::new()));
    // A hand-written non-array here is the user's problem to notice; never clobber it.
    let Some(arr) = existing.as_array_mut() else { continue };
    for group in groups {
      // `matcher` is absent for events like Stop; `Option<&Value>` equality treats two
      // absences as equal, which is exactly "the one group with no matcher".
      let matcher = group.get("matcher");
      let Some(existing_group) = arr.iter_mut().find(|g| g.get("matcher") == matcher) else {
        arr.push(group.clone());
        log.push(format!(
          "hooks.{event}: added {}",
          matcher.and_then(Value::as_str).unwrap_or("group")
        ));
        continue;
      };
      let Some(entries) = group.get("hooks").and_then(Value::as_array) else {
        continue;
      };
      let hooks = existing_group
        .as_object_mut()
        .map(|g| g.entry("hooks").or_insert_with(|| Value::Array(Vec::new())));
      let Some(hooks) = hooks.and_then(Value::as_array_mut) else {
        continue;
      };
      for entry in entries {
        // Only entries carrying a `hook <name>` token are ours to manage.
        let Some(key) = hook_key(entry) else { continue };
        match hooks.iter_mut().find(|e| hook_key(e) == Some(key)) {
          Some(e) if e == entry => {}
          Some(e) => {
            *e = entry.clone();
            log.push(format!("hooks.{event}: updated {key}"));
          }
          None => {
            hooks.push(entry.clone());
            log.push(format!("hooks.{event}: added {key}"));
          }
        }
      }
    }
  }
}

/// The `<name>` in a `… hook <name> …` command, which identifies a devkit-owned entry
/// regardless of which binary path precedes it.
fn hook_key(entry: &Value) -> Option<&str> {
  let (_, rest) = entry.get("command")?.as_str()?.split_once(" hook ")?;
  rest.split_whitespace().next()
}

/// `.claude/settings.json`: `hooks` via [`merge_hooks`], everything else via [`deep_merge`].
pub fn merge_claude_settings(original: Option<&str>, template: &str, log: &mut Vec<String>) -> Result<String> {
  let mut tpl = parse(template)?;
  let Some(original) = original else {
    log.push("created from template".into());
    return render(&tpl, &header_comments(template), None);
  };
  let mut doc = parse(original)?;
  // Pull `hooks` out so `deep_merge` (whole-value array union, which would duplicate a
  // hand-edited hook) never sees it.
  let hooks = tpl.as_object_mut().and_then(|t| t.remove("hooks"));
  deep_merge(&mut doc, &tpl, "", log);
  if let Some(hooks) = hooks {
    let target = doc
      .as_object_mut()
      .context("settings.json root is not an object")?
      .entry("hooks")
      .or_insert_with(|| Value::Object(Map::new()));
    merge_hooks(target, &hooks, log);
  }
  if log.is_empty() {
    return Ok(original.to_string());
  }
  render(&doc, &header_comments(original), Some(original))
}

#[cfg(test)]
mod hooks_tests {
  use super::*;
  use serde_json::json;

  fn tpl() -> Value {
    json!({
      "PreToolUse": [
        {"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "\"$D/devkit\" hook pre-edit-protect", "shell": "bash"}]}
      ],
      "Stop": [
        {"hooks": [
          {"type": "command", "command": "\"$D/devkit\" hook stop-ruff", "shell": "bash", "timeout": 30},
          {"type": "command", "command": "\"$D/devkit\" hook stop-pyright", "shell": "bash", "timeout": 60}
        ]}
      ]
    })
  }

  #[test]
  fn fresh_insert_copies_the_template() {
    let mut target = json!({});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    assert_eq!(target, tpl());
    assert!(!log.is_empty());
  }

  #[test]
  fn second_merge_is_a_no_op() {
    let mut target = tpl();
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    assert_eq!(target, tpl());
    assert!(log.is_empty(), "{log:?}");
  }

  #[test]
  fn hand_edited_entry_is_updated_in_place_not_duplicated() {
    let mut target = tpl();
    target["Stop"][0]["hooks"][0]["timeout"] = json!(99);
    target["Stop"][0]["hooks"][0]["command"] = json!("python old/stop_ruff.py hook stop-ruff");
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    assert_eq!(target["Stop"][0]["hooks"].as_array().unwrap().len(), 2);
    assert_eq!(target["Stop"][0]["hooks"][0]["timeout"], 30);
    assert_eq!(target["Stop"][0]["hooks"][0]["command"], "\"$D/devkit\" hook stop-ruff");
    assert!(log.iter().any(|l| l.contains("stop-ruff")), "{log:?}");
  }

  #[test]
  fn hand_written_hooks_survive_and_template_entries_are_added_alongside() {
    let mine = json!({"type": "command", "command": "echo mine"});
    let mut target = json!({"Stop": [{"hooks": [mine.clone()]}], "SessionStart": [{"hooks": [mine.clone()]}]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    let stop = target["Stop"][0]["hooks"].as_array().unwrap();
    assert_eq!(stop.len(), 3);
    assert_eq!(stop[0], mine);
    assert_eq!(target["SessionStart"], json!([{"hooks": [mine]}]));
    assert_eq!(target["PreToolUse"], tpl()["PreToolUse"]);
  }

  #[test]
  fn claude_settings_merges_hooks_by_key_and_the_rest_deeply() {
    let template = r#"{"permissions": {"allow": ["WebSearch"]}, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "devkit hook stop-ruff", "timeout": 30}]}]}}"#;
    let original = r#"{"permissions": {"allow": ["Bash(git diff *)"]}, "hooks": {"Stop": [{"hooks": [{"type": "command", "command": "devkit hook stop-ruff", "timeout": 99}]}]}}"#;
    let mut log = vec![];
    let out = merge_claude_settings(Some(original), template, &mut log).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["permissions"]["allow"], json!(["Bash(git diff *)", "WebSearch"]));
    assert_eq!(v["hooks"]["Stop"][0]["hooks"].as_array().unwrap().len(), 1, "{out}");
    assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["timeout"], 30);
  }
}
