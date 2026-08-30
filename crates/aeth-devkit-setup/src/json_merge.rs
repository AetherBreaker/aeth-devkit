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

/// `.mcp.json`: servers the project already defines are left exactly as they are; only
/// missing ones are added.
///
/// A key-by-key `deep_merge` is wrong here because a server definition is a *unit* whose
/// fields only make sense together. A project running the GitHub server over stdio
/// (`command`/`args`) merged against the template's `type: "http"` form came out as a hybrid
/// carrying both — an `http` server that still has a `command`, which is not a valid config
/// under either transport. Whole-entry semantics also matches intent: a project that has
/// already configured a server has made a choice devkit should not second-guess.
pub fn merge_mcp_file(original: Option<&str>, template: &str, log: &mut Vec<String>) -> Result<String> {
  let tpl = parse(template)?;
  let Some(original) = original else {
    log.push("created from template".into());
    return render(&tpl, &header_comments(template), None);
  };
  let mut doc = parse(original)?;

  // Everything outside `mcpServers` still deep-merges: those are ordinary scalar settings.
  let mut tpl_rest = tpl.clone();
  let tpl_servers = tpl_rest.as_object_mut().and_then(|t| t.remove("mcpServers"));
  deep_merge(&mut doc, &tpl_rest, "", log);

  if let Some(Value::Object(servers)) = tpl_servers {
    let slot = doc
      .as_object_mut()
      .map(|d| d.entry("mcpServers").or_insert_with(|| Value::Object(Map::new())));
    // A non-object `mcpServers` is the user's; skip rather than replace it.
    if let Some(target) = slot.and_then(Value::as_object_mut) {
      for (name, def) in servers {
        if target.contains_key(&name) {
          continue;
        }
        target.insert(name.clone(), def);
        log.push(format!("mcpServers: added {name}"));
      }
    }
  }

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

/// Merge the `hooks` object of `.claude/settings.json`.
///
/// The invariant this maintains is per *event*, not per group: for each hook the template
/// owns, exactly one entry exists anywhere under that event. Reconciling only within the
/// group whose `matcher` matches is not enough — a project can spread one hook per group
/// object, and `"Write|Edit"` is the same matcher as the template's `"Edit|Write"` written
/// the other way round. Either shape used to leave the old entry in a second group, so the
/// hook ran twice on every event, and re-running never healed it.
pub fn merge_hooks(target: &mut Value, template: &Value, log: &mut Vec<String>) {
  let Some(tmpl) = template.as_object() else { return };
  // A hand-written non-object here is the user's, not ours to replace. The sibling branches
  // below deliberately skip rather than clobber; this one used to overwrite it with `{}`,
  // silently dropping whatever they had written.
  if !target.is_object() {
    log.push("hooks: left alone (not an object)".into());
    return;
  }
  let tobj = target.as_object_mut().expect("checked above");
  for (event, groups) in tmpl {
    let Some(groups) = groups.as_array() else { continue };
    let existing = tobj.entry(event.clone()).or_insert_with(|| Value::Array(Vec::new()));
    // A hand-written non-array here is the user's problem to notice; never clobber it.
    let Some(arr) = existing.as_array_mut() else { continue };
    let mut emptied = false;
    for group in groups {
      // `matcher` is absent for events like Stop; `Option<&Value>` equality treats two
      // absences as equal, which is exactly "the one group with no matcher".
      let matcher = group.get("matcher").cloned();
      let Some(entries) = group.get("hooks").and_then(Value::as_array) else {
        continue;
      };
      for entry in entries {
        // Only entries carrying a `hook <name>` token are ours to manage.
        let Some(key) = hook_key(entry) else { continue };
        if reconcile_hook(arr, matcher.as_ref(), entry, &key, event, log) {
          emptied = true;
        }
      }
    }
    // Only tidy up when a duplicate was actually removed: an empty group the project wrote
    // itself is theirs to keep.
    if emptied {
      arr.retain(|g| g.get("hooks").and_then(Value::as_array).is_none_or(|h| !h.is_empty()));
    }
  }
}

/// Place exactly one copy of `entry` under `event`, removing any other entry with the same
/// key from *any* group. Returns whether a duplicate was removed.
fn reconcile_hook(
  arr: &mut Vec<Value>,
  matcher: Option<&Value>,
  entry: &Value,
  key: &str,
  event: &str,
  log: &mut Vec<String>,
) -> bool {
  // Every (group index, entry index) whose entry means this same hook — including the legacy
  // `.claude/hooks/<snake>.py` spelling, which is the same hook by another name.
  let mut found: Vec<(usize, usize)> = Vec::new();
  for (gi, g) in arr.iter().enumerate() {
    let Some(hooks) = g.get("hooks").and_then(Value::as_array) else {
      continue;
    };
    for (ei, e) in hooks.iter().enumerate() {
      if matches_key(e, key) {
        found.push((gi, ei));
      }
    }
  }

  match found.split_first() {
    // Not present anywhere: add it to the group carrying this matcher, starting one if the
    // project has no such group yet (which is the whole story for a fresh settings.json).
    None => {
      if !arr.iter().any(|g| g.get("matcher") == matcher) {
        let mut fresh = Map::new();
        if let Some(m) = matcher {
          fresh.insert("matcher".to_string(), m.clone());
        }
        fresh.insert("hooks".to_string(), Value::Array(Vec::new()));
        arr.push(Value::Object(fresh));
      }
      let slot = arr
        .iter_mut()
        .find(|g| g.get("matcher") == matcher)
        .and_then(|g| g.as_object_mut())
        .map(|g| g.entry("hooks").or_insert_with(|| Value::Array(Vec::new())));
      if let Some(hooks) = slot.and_then(Value::as_array_mut) {
        hooks.push(entry.clone());
        log.push(format!("hooks.{event}: added {key}"));
      }
      false
    }
    Some((&(gi, ei), rest)) => {
      // Update the first occurrence in place, so the project's own group and matcher survive.
      if let Some(e) = arr
        .get_mut(gi)
        .and_then(|g| g.get_mut("hooks"))
        .and_then(Value::as_array_mut)
        .and_then(|h| h.get_mut(ei))
        && e != entry
      {
        *e = entry.clone();
        log.push(format!("hooks.{event}: updated {key}"));
      }
      // Drop every other occurrence. Removing back-to-front keeps the earlier indices valid.
      let mut extras = rest.to_vec();
      extras.sort_unstable_by(|a, b| b.cmp(a));
      let mut removed = false;
      for (g, e) in extras {
        if let Some(hooks) = arr.get_mut(g).and_then(|g| g.get_mut("hooks")).and_then(Value::as_array_mut)
          && e < hooks.len()
        {
          hooks.remove(e);
          removed = true;
          log.push(format!("hooks.{event}: removed duplicate {key}"));
        }
      }
      removed
    }
  }
}

/// Whether an existing entry is the same hook as template key `key`.
///
/// The legacy spelling is only ever compared *against a template key*, which is what stops
/// the migration from claiming a script it does not own: a project's own
/// `.claude/hooks/my_custom_check.py` matches no template key and is left alone, and a
/// future template hook cannot retroactively adopt an unrelated script either.
fn matches_key(entry: &Value, key: &str) -> bool {
  hook_key(entry).as_deref() == Some(key) || legacy_hook_key(entry).as_deref() == Some(key)
}

/// The `<name>` in a `… hook <name> …` command, which identifies a devkit-owned entry
/// regardless of which binary path precedes it.
fn hook_key(entry: &Value) -> Option<String> {
  let cmd = entry.get("command")?.as_str()?;
  let (_, rest) = cmd.split_once(" hook ")?;
  rest.split_whitespace().next().map(str::to_string)
}

/// The pre-devkit wiring copied the hooks in as `.claude/hooks/<snake_name>.py` scripts, in
/// several spellings depending on the shell and platform they were written for. Recognising
/// them is what makes migration an update in place rather than a second, duplicate entry.
fn legacy_hook_key(entry: &Value) -> Option<String> {
  let cmd = entry.get("command")?.as_str()?;
  // Accept both separators and both variable syntaxes: `$CLAUDE_PROJECT_DIR/.claude/hooks/`,
  // `%CLAUDE_PROJECT_DIR%\.claude\hooks\`, and a bare relative `.claude/hooks/`.
  let normalized = cmd.replace('\\', "/");
  let (_, file) = normalized.rsplit_once(".claude/hooks/")?;
  let stem = file.split(['"', '\'', ' ']).next()?.strip_suffix(".py")?;
  Some(stem.replace('_', "-"))
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
  fn a_users_own_hook_script_is_not_claimed_by_the_fallback() {
    let mine = json!({"type": "command", "command": "python \"$CLAUDE_PROJECT_DIR/.claude/hooks/my_custom_check.py\"", "timeout": 5});
    let mut target = json!({"Stop": [{"hooks": [mine.clone()]}]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    let stop = target["Stop"][0]["hooks"].as_array().unwrap();
    assert_eq!(stop[0], mine, "a non-devkit hook script must survive untouched");
    assert_eq!(stop.len(), 3, "template entries added alongside it: {stop:?}");
  }

  #[test]
  fn legacy_python_hook_entries_are_replaced_not_duplicated() {
    // The fleet's pre-devkit wiring: `python "$CLAUDE_PROJECT_DIR/.claude/hooks/stop_ruff.py"`.
    let mut target = json!({"Stop": [{"hooks": [
      {"type": "command", "command": "python \"$CLAUDE_PROJECT_DIR/.claude/hooks/stop_ruff.py\"", "shell": "bash", "timeout": 30},
      {"type": "command", "command": "python \"$CLAUDE_PROJECT_DIR/.claude/hooks/stop_pyright.py\"", "shell": "bash", "timeout": 60}
    ]}]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    let stop = target["Stop"][0]["hooks"].as_array().unwrap();
    assert_eq!(stop.len(), 2, "{stop:?}");
    assert_eq!(stop[0]["command"], "\"$D/devkit\" hook stop-ruff");
    assert_eq!(stop[1]["command"], "\"$D/devkit\" hook stop-pyright");
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

  #[test]
  fn a_legacy_entry_in_a_later_group_is_migrated_not_left_to_run_twice() {
    // One hook per group object is an ordinary hand-written shape. Reconciling only the
    // first matcher-matching group left the second one holding a legacy script, so the hook
    // ran twice on every Stop — and re-running never healed it, because the state was stable.
    let mut target = json!({"Stop": [
      {"hooks": [{"type": "command", "command": "python \"$CLAUDE_PROJECT_DIR/.claude/hooks/stop_ruff.py\""}]},
      {"hooks": [{"type": "command", "command": "python \"$CLAUDE_PROJECT_DIR/.claude/hooks/stop_pyright.py\""}]}
    ]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    let commands: Vec<String> = target["Stop"]
      .as_array()
      .unwrap()
      .iter()
      .flat_map(|g| g["hooks"].as_array().cloned().unwrap_or_default())
      .map(|e| e["command"].as_str().unwrap_or("").to_string())
      .collect();
    assert_eq!(commands.iter().filter(|c| c.contains("stop-pyright")).count(), 1, "{commands:?}");
    assert!(
      !commands.iter().any(|c| c.contains(".claude/hooks/")),
      "legacy left behind: {commands:?}"
    );
  }

  #[test]
  fn an_equivalent_matcher_written_the_other_way_round_does_not_get_a_second_group() {
    // `"Write|Edit"` is the template's `"Edit|Write"`; a per-group merge saw them as
    // different and added a second group, so both the user's hook and ours fired.
    let mut target = json!({"PreToolUse": [
      {"matcher": "Write|Edit", "hooks": [{"type": "command", "command": "\"$D/devkit\" hook pre-edit-protect", "shell": "bash"}]}
    ]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    assert_eq!(target["PreToolUse"].as_array().unwrap().len(), 1, "{target}");
    assert_eq!(target["PreToolUse"][0]["matcher"], "Write|Edit", "the project's matcher survives");
  }

  #[test]
  fn a_users_own_script_that_collides_with_a_template_name_is_still_theirs() {
    // `stop_clean.py` maps onto the devkit key `stop-clean`, but the template here does not
    // define that hook — so nothing may claim it. The legacy spelling is only ever compared
    // against keys the template actually owns.
    let mine = json!({"type": "command", "command": "python \"$CLAUDE_PROJECT_DIR/.claude/hooks/stop_clean.py\"", "timeout": 5});
    let mut target = json!({"Stop": [{"hooks": [mine.clone()]}]});
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    let stop = target["Stop"][0]["hooks"].as_array().unwrap();
    assert_eq!(stop[0], mine, "an unowned script must survive untouched: {stop:?}");
  }

  #[test]
  fn legacy_entries_migrate_in_every_spelling() {
    // Backslashes and `%VAR%` are what a Windows-authored settings.json holds; a bare
    // relative path is what someone writes by hand. Each used to duplicate instead of migrate.
    for cmd in [
      r#"python "$CLAUDE_PROJECT_DIR/.claude/hooks/stop_ruff.py""#,
      r#"python $CLAUDE_PROJECT_DIR/.claude/hooks/stop_ruff.py"#,
      r#"python "%CLAUDE_PROJECT_DIR%\.claude\hooks\stop_ruff.py""#,
      r#"python .claude/hooks/stop_ruff.py"#,
      r#"py -3 .claude\hooks\stop_ruff.py"#,
    ] {
      let mut target = json!({"Stop": [{"hooks": [{"type": "command", "command": cmd}]}]});
      let mut log = vec![];
      merge_hooks(&mut target, &tpl(), &mut log);
      let stop = target["Stop"][0]["hooks"].as_array().unwrap();
      assert_eq!(stop.len(), 2, "must migrate in place, not duplicate: {cmd} -> {stop:?}");
      assert_eq!(stop[0]["command"], "\"$D/devkit\" hook stop-ruff", "{cmd}");
    }
  }

  #[test]
  fn a_non_object_hooks_value_is_left_alone_not_discarded() {
    // The sibling branches deliberately skip rather than clobber; this one used to replace
    // the user's value with `{}` silently, with no log line and no error.
    let mut target = json!([{"note": "my hand-written thing"}]);
    let mut log = vec![];
    merge_hooks(&mut target, &tpl(), &mut log);
    assert_eq!(target, json!([{"note": "my hand-written thing"}]));
    assert!(log.iter().any(|l| l.contains("left alone")), "must say so: {log:?}");
  }

  #[test]
  fn mcp_merge_never_half_overwrites_a_users_own_server() {
    // A server definition is a unit: merging the template's `type: "http"` GitHub server
    // key-by-key into a project's stdio one produced an `http` server that still carried
    // `command`/`args`, which is not valid under either transport.
    let template = r#"{"mcpServers": {"github": {"type": "http", "url": "https://api.githubcopilot.com/mcp/"}, "context7": {"command": "npx", "args": ["-y", "@upstash/context7-mcp"]}}}"#;
    let original =
      r#"{"mcpServers": {"github": {"type": "stdio", "command": "docker", "args": ["run", "ghcr.io/github/github-mcp-server"]}}}"#;
    let mut log = vec![];
    let out = merge_mcp_file(Some(original), template, &mut log).unwrap();
    let v: Value = serde_json::from_str(&out).unwrap();
    let github = &v["mcpServers"]["github"];
    assert_eq!(github["type"], "stdio", "the project's transport must win: {out}");
    assert_eq!(github["command"], "docker");
    assert!(github.get("url").is_none(), "no leftover http fields: {out}");
    // A server the project does not define is still added.
    assert_eq!(v["mcpServers"]["context7"]["command"], "npx");
  }

  #[test]
  fn mcp_merge_is_idempotent() {
    let template = r#"{"mcpServers": {"context7": {"command": "npx", "args": ["-y", "@upstash/context7-mcp"]}}}"#;
    let mut log = vec![];
    let once = merge_mcp_file(None, template, &mut log).unwrap();
    let mut log2 = vec![];
    let twice = merge_mcp_file(Some(&once), template, &mut log2).unwrap();
    assert_eq!(once, twice);
    assert!(log2.is_empty(), "{log2:?}");
  }
}
