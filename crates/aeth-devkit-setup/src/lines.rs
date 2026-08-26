//! Line-oriented merges: `.env` upsert, `.gitignore` replace-or-prepend, plain line union.

use std::collections::HashSet;

fn line_ending(original: Option<&str>) -> &'static str {
  match original {
    Some(s) if s.contains("\r\n") => "\r\n",
    _ => "\n",
  }
}

fn split_lines(s: &str) -> Vec<String> {
  s.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect()
}

fn join(lines: &[String], eol: &str) -> String {
  let mut out = lines.join(eol);
  if !out.ends_with(eol) {
    out.push_str(eol);
  }
  out
}

/// Key of an env line (`export FOO=…` / `FOO=…`), if it is one.
fn env_key(line: &str) -> Option<&str> {
  let l = line.trim_start();
  let l = l.strip_prefix("export ").map(str::trim_start).unwrap_or(l);
  let (key, _) = l.split_once('=')?;
  let key = key.trim();
  if key.is_empty() || key.starts_with('#') { None } else { Some(key) }
}

/// Replace each template `KEY=…` line in place, or append it. Everything else is preserved.
pub fn upsert_env(original: Option<&str>, template: &str, log: &mut Vec<String>) -> String {
  let eol = line_ending(original);
  let mut lines = match original {
    Some(s) => split_lines(s),
    None => Vec::new(),
  };
  // Drop the single trailing empty element produced by a final newline so we can append cleanly.
  let had_trailing_newline = lines.last().is_some_and(|l| l.is_empty());
  if had_trailing_newline {
    lines.pop();
  }
  for tline in template.lines() {
    let Some(key) = env_key(tline) else { continue };
    match lines.iter().position(|l| env_key(l) == Some(key)) {
      Some(i) => {
        if lines[i] != tline {
          log.push(format!("set {key}"));
          lines[i] = tline.to_string();
        }
      }
      None => {
        log.push(format!("added {key}"));
        lines.push(tline.to_string());
      }
    }
  }
  if lines.is_empty() {
    return String::new();
  }
  join(&lines, eol)
}

/// Normalized form used for "same ignore rule" comparisons. `uv init` writes `*.py[oc]`,
/// which GitHub's template spells `*.py[codz]`; treat them as the same rule.
fn norm(line: &str) -> String {
  let t = line.trim().trim_end_matches('/');
  match t {
    "*.py[oc]" => "*.py[codz]".to_string(),
    _ => t.to_string(),
  }
}

fn is_rule(line: &str) -> bool {
  let t = line.trim();
  !t.is_empty() && !t.starts_with('#')
}

/// If the existing file has no rules outside the template, replace it. Otherwise prepend
/// the template and drop duplicated rules (and orphaned comments) from the remainder.
pub fn merge_gitignore(original: Option<&str>, template: &str, log: &mut Vec<String>) -> String {
  let eol = line_ending(original);
  let template_lines = split_lines(template.trim_end_matches(['\n', '\r']));
  let template_rules: HashSet<String> = template_lines.iter().filter(|l| is_rule(l)).map(|l| norm(l)).collect();

  let Some(original) = original else {
    log.push("created from template".into());
    return join(&template_lines, eol);
  };
  let orig_lines = split_lines(original);
  let extra: Vec<&String> = orig_lines
    .iter()
    .filter(|l| is_rule(l) && !template_rules.contains(&norm(l)))
    .collect();
  if extra.is_empty() {
    if split_lines(original.trim_end_matches(['\n', '\r'])) == template_lines {
      return original.to_string();
    }
    log.push("replaced with template (no project-specific rules)".into());
    return join(&template_lines, eol);
  }

  // Prepend the template; keep only non-duplicate rules from the original.
  let mut remainder: Vec<String> = Vec::new();
  for l in &orig_lines {
    if is_rule(l) && template_rules.contains(&norm(l)) {
      continue;
    }
    remainder.push(l.clone());
  }
  // Drop comment lines not followed (before the next blank/EOF) by any rule, and collapse blank runs.
  let mut cleaned: Vec<String> = Vec::new();
  for (i, l) in remainder.iter().enumerate() {
    let t = l.trim();
    if t.starts_with('#') {
      let has_rule_after = remainder[i + 1..].iter().take_while(|n| !n.trim().is_empty()).any(|n| is_rule(n));
      if !has_rule_after {
        continue;
      }
    }
    if t.is_empty() && cleaned.last().is_none_or(|p| p.trim().is_empty()) {
      continue;
    }
    cleaned.push(l.clone());
  }
  while cleaned.last().is_some_and(|l| l.trim().is_empty()) {
    cleaned.pop();
  }

  // Already in the prepended form? Then the file is unchanged.
  let already: Vec<String> = split_lines(original.trim_end_matches(['\n', '\r']));
  if already.starts_with(&template_lines) {
    let tail: Vec<String> = already[template_lines.len()..]
      .iter()
      .skip_while(|l| l.trim().is_empty())
      .cloned()
      .collect();
    if tail == cleaned {
      return original.to_string();
    }
  }

  log.push(format!("prepended template; kept {} project-specific rule(s)", extra.len()));
  let mut out = template_lines;
  out.push(String::new());
  out.push("# ---- project-specific ----".into());
  out.extend(cleaned);
  join(&out, eol)
}

/// Append template lines not already present. Never removes or reorders.
pub fn line_union(original: Option<&str>, template: &str, log: &mut Vec<String>) -> String {
  let eol = line_ending(original);
  let Some(original) = original else {
    log.push("created from template".into());
    return join(&split_lines(template.trim_end_matches(['\n', '\r'])), eol);
  };
  let existing: HashSet<String> = original.lines().map(norm).collect();
  let missing: Vec<String> = template
    .lines()
    .filter(|l| is_rule(l) && !existing.contains(&norm(l)))
    .map(str::to_string)
    .collect();
  if missing.is_empty() {
    return original.to_string();
  }
  log.push(format!("added: {}", missing.join(", ")));
  let mut lines = split_lines(original.trim_end_matches(['\n', '\r']));
  lines.extend(missing);
  join(&lines, eol)
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn env_upsert_replaces_in_place_and_appends() {
    let orig = "A=1\r\nPYTHONPYCACHEPREFIX=\"old\"\r\nB=2\r\n";
    let mut log = vec![];
    let out = upsert_env(Some(orig), "PYTHONPYCACHEPREFIX=\"new\"\nNEW=x\n", &mut log);
    assert_eq!(out, "A=1\r\nPYTHONPYCACHEPREFIX=\"new\"\r\nB=2\r\nNEW=x\r\n");
    assert_eq!(log, vec!["set PYTHONPYCACHEPREFIX", "added NEW"]);
    let mut log2 = vec![];
    assert_eq!(upsert_env(Some(&out), "PYTHONPYCACHEPREFIX=\"new\"\nNEW=x\n", &mut log2), out);
    assert!(log2.is_empty());
  }

  #[test]
  fn env_upsert_creates() {
    let mut log = vec![];
    assert_eq!(upsert_env(None, "K=v\n", &mut log), "K=v\n");
  }

  #[test]
  fn gitignore_replaces_when_subset() {
    let tpl = "# Python\n__pycache__/\n.venv\n.cache\n";
    let mut log = vec![];
    let out = merge_gitignore(Some("__pycache__/\n.venv\n"), tpl, &mut log);
    assert_eq!(out, tpl);
  }

  #[test]
  fn gitignore_prepends_and_dedups() {
    let tpl = "# Python\n__pycache__/\n.venv\n.cache\n";
    let orig = "# generated\n__pycache__/\n.venv\n\n# mine\nsecrets/\n.cache/\n";
    let mut log = vec![];
    let out = merge_gitignore(Some(orig), tpl, &mut log);
    assert_eq!(
      out,
      "# Python\n__pycache__/\n.venv\n.cache\n\n# ---- project-specific ----\n# mine\nsecrets/\n"
    );
    let mut log2 = vec![];
    assert_eq!(merge_gitignore(Some(&out), tpl, &mut log2), out);
    assert!(log2.is_empty(), "{log2:?}");
  }

  #[test]
  fn union_appends_missing_only() {
    let mut log = vec![];
    let out = line_union(Some("a\nb\n"), "b\nc\n", &mut log);
    assert_eq!(out, "a\nb\nc\n");
    let mut log2 = vec![];
    assert_eq!(line_union(Some(&out), "b\nc\n", &mut log2), out);
  }
}
