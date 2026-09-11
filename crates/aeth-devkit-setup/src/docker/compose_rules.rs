//! The compose standard as a rule table read from the scaffold's `# !rule <kind>`
//! annotations (spec 2.6). Every standard *value* comes from the rendered scaffold block for
//! the same service, so the template is the single source of truth: it says which keys are
//! enforced, which kind of check each gets, and what the standard value is.

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::compose::tree::{self, Edit, Node};
use aeth_devkit_core::github::normalize_repo;

/// How a key is compared with the scaffold (see the spec's rule table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
  /// Rewritten when the scalar differs; inserted when missing.
  Exact,
  /// Inserted (with its scaffold subtree) when missing; never changed.
  Presence,
  /// `GIT_REPO`: compared through docker-pin's normaliser; rewritten to the scaffold form.
  Repo,
  /// `volumes`: some entry must mount `/app/persisted_data`; the scaffold entry is appended.
  VolumeTarget,
  /// `environment`: each scaffold `KEY=` must be present; missing ones are appended.
  EnvKeys,
  /// `healthcheck.test`: the list must equal the scaffold's; else the subtree is replaced.
  ExactList,
}

impl Kind {
  fn parse(s: &str) -> Option<Kind> {
    Some(match s {
      "exact" => Kind::Exact,
      "presence" => Kind::Presence,
      "repo" => Kind::Repo,
      "volume-target" => Kind::VolumeTarget,
      "env-keys" => Kind::EnvKeys,
      "exact-list" => Kind::ExactList,
      _ => return None,
    })
  }
}

/// One `# !rule <kind>` annotation: the key it sits above, as a path relative to the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
  pub path: Vec<String>,
  pub kind: Kind,
}

pub const RULE_MARKER: &str = "# !rule ";

/// Strip the rule lines from a scaffold block and return them keyed by the annotated key's
/// path, `skip` leading mapping levels dropped (`services`, the service). A rule line must
/// be followed by a mapping key (spec 2.6).
pub fn parse_rules(lines: &[String], skip: usize) -> Result<(Vec<String>, Vec<Rule>)> {
  let mut out = Vec::with_capacity(lines.len());
  let mut rules = Vec::new();
  let mut pending: Option<(Kind, usize)> = None;
  let mut stack: Vec<(usize, String)> = Vec::new();
  for (i, line) in lines.iter().enumerate() {
    let trimmed = line.trim();
    if let Some(kind) = trimmed.strip_prefix(RULE_MARKER) {
      if let Some((_, at)) = pending {
        bail!("line {}: a rule line follows another rule line (line {})", i + 1, at + 1);
      }
      let kind = Kind::parse(kind.trim()).with_context(|| format!("line {}: unknown rule kind `{}`", i + 1, kind.trim()))?;
      pending = Some((kind, i));
      continue;
    }
    out.push(line.clone());
    let key = (!trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("- "))
      .then(|| trimmed.split_once(':').map(|(k, _)| k.trim().to_string()))
      .flatten();
    let Some(key) = key else {
      if let Some((_, at)) = pending {
        bail!("line {}: `# !rule` must be followed by a key, not `{trimmed}`", at + 1);
      }
      continue;
    };
    let indent = line.len() - line.trim_start().len();
    while stack.last().is_some_and(|(d, _)| *d >= indent) {
      stack.pop();
    }
    stack.push((indent, key));
    if let Some((kind, _)) = pending.take() {
      rules.push(Rule {
        path: stack.iter().skip(skip).map(|(_, k)| k.clone()).collect(),
        kind,
      });
    }
  }
  if let Some((_, at)) = pending {
    bail!("line {}: `# !rule` at the end of the block annotates nothing", at + 1);
  }
  Ok((out, rules))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
  pub edits: Vec<Edit>,
  /// One human line per edit, for the change report.
  pub details: Vec<String>,
  /// Drift the engine saw but would not edit: a YAML shape it does not model (flow style,
  /// a list where the standard has a mapping). Splicing block lines into those is not
  /// YAML, so the user is told instead.
  pub errors: Vec<String>,
}

/// The scaffold subtree rooted at `sc_node`, re-indented to sit under `parent`.
fn subtree_under(lines: &[String], parent: &Node, sc_lines: &[String], sc_node: &Node) -> Vec<String> {
  tree::re_indent(
    &sc_lines[sc_node.line..sc_node.end],
    sc_node.indent,
    tree::child_indent(lines, parent),
  )
}

/// Edits bringing `svc` (in `lines`) up to `sc_svc` (the same service's scaffold block in
/// `sc_lines`). Rules whose path the scaffold lacks (a gated-out `environment`) are skipped.
pub fn service_edits(lines: &[String], svc: &Node, sc_lines: &[String], sc_svc: &Node, name: &str, rules: &[Rule]) -> Outcome {
  let mut out = Outcome::default();
  // Dotted prefixes inserted or replaced wholesale; deeper rules under them are already
  // satisfied and must not add a second copy.
  let mut settled: Vec<String> = Vec::new();
  for Rule { path, kind } in rules {
    let path: Vec<&str> = path.iter().map(String::as_str).collect();
    let path = path.as_slice();
    let kind = *kind;
    let dotted = path.join(".");
    if settled.iter().any(|p| dotted.starts_with(&format!("{p}."))) {
      continue;
    }
    let Some(std) = tree::descend(sc_lines, sc_svc, path) else {
      continue;
    };
    // Walk down the target, stopping at the first missing or scalar-valued segment.
    let mut parent = svc.clone();
    let mut found: Option<Node> = None;
    for (depth, key) in path.iter().enumerate() {
      let prefix = path[..=depth].join(".");
      let sc_node = tree::descend(sc_lines, sc_svc, &path[..=depth]).expect("scaffold holds every rule path");
      match tree::child(lines, &parent, key) {
        Some(n) if depth + 1 == path.len() => found = Some(n),
        // `build: .` (or `build: {…}`) where a block mapping is needed: replace the line
        // with the scaffold block. An anchored block (`build: &b` + children) walks on.
        Some(n) if n.is_inline() => {
          out.edits.push(Edit::Replace {
            from: n.line,
            to: n.end,
            lines: tree::re_indent(&sc_lines[sc_node.line..sc_node.end], sc_node.indent, n.indent),
          });
          out.details.push(format!("{name}: replaced {prefix}"));
          settled.push(prefix);
          break;
        }
        Some(n) => parent = n,
        None => {
          // `args:` holding `- GIT_TAG=v1`: the parent is a sequence, and a mapping line
          // spliced into it is not YAML. Settled so sibling rules do not repeat the error.
          if !tree::list_items(lines, &parent).is_empty() {
            let at = path[..depth].join(".");
            out.errors.push(format!(
              "{name}: {at} is written as a list, so {prefix} was not added; switch it to the mapping form or add the key by hand"
            ));
            settled.push(at);
            break;
          }
          out.edits.push(Edit::Insert {
            at: parent.end,
            lines: subtree_under(lines, &parent, sc_lines, &sc_node),
          });
          out.details.push(format!("{name}: added {prefix}"));
          settled.push(prefix);
          break;
        }
      }
    }
    let Some(node) = found else { continue };
    match kind {
      Kind::Presence => {}
      Kind::Exact => {
        if node.value != std.value {
          out.edits.push(Edit::SetValue {
            line: node.line,
            value: std.value.clone(),
          });
          out.details.push(format!("{name}: set {dotted} = {}", std.value));
        }
      }
      Kind::Repo => {
        // No origin → empty scaffold value → nothing to compare against.
        if !std.value.is_empty() && normalize_repo(&node.value) != normalize_repo(&std.value) {
          out.edits.push(Edit::SetValue {
            line: node.line,
            value: std.value.clone(),
          });
          out.details.push(format!("{name}: set {dotted} = {}", std.value));
        }
      }
      Kind::VolumeTarget => {
        let sc_items = tree::list_items(sc_lines, &std);
        let want = sc_items
          .first()
          .and_then(|it| tree::item_child(sc_lines, it, "target"))
          .map(|t| t.value)
          .unwrap_or_default();
        // Short form `source:target[:mode]`; long form has `target` inside the item.
        let short_form = |text: &str| text.split(':').nth(1) == Some(want.as_str());
        // Flow style (`volumes: [...]`): block items appended under it are not YAML, so
        // the entries are judged in place and a missing mount is left to the user.
        if node.is_inline() {
          let mounted = tree::flow_entries(&node.value).map(|entries| {
            entries.iter().any(|e| match tree::flow_entries(e) {
              Some(pairs) => pairs
                .iter()
                .any(|p| p.split_once(':').is_some_and(|(k, v)| k.trim() == "target" && v.trim() == want)),
              None => short_form(e),
            })
          });
          if mounted != Some(true) {
            out.errors.push(format!(
              "{name}: {dotted} is written inline and mounts nothing at {want}; add the bind mount by hand or switch to the block form"
            ));
          }
          continue;
        }
        let items = tree::list_items(lines, &node);
        let mounted = items
          .iter()
          .any(|it| tree::item_child(lines, it, "target").is_some_and(|t| t.value == want) || short_form(&it.text));
        if !mounted {
          let indent = items.first().map_or(tree::child_indent(lines, &node), |it| it.indent);
          let sc_item = &sc_items[0];
          out.edits.push(Edit::Insert {
            at: node.end,
            lines: tree::re_indent(&sc_lines[sc_item.line..sc_item.end], sc_item.indent, indent),
          });
          out.details.push(format!("{name}: added the {want} bind mount"));
        }
      }
      Kind::EnvKeys => {
        let sc_items = tree::list_items(sc_lines, &std);
        // `KEY=value` list items; a bare `KEY` has an empty value.
        let sc_pairs: Vec<(&str, &str)> = sc_items
          .iter()
          .map(|it| it.text.split_once('=').unwrap_or((&it.text, "")))
          .collect();
        // Flow style (`environment: {...}` or `[...]`): same rule as volumes — the entry
        // keys are compared exactly, as `K: v` pairs or `K=v` strings.
        if node.is_inline() {
          let keys: Vec<String> = tree::flow_entries(&node.value)
            .unwrap_or_default()
            .iter()
            .map(|e| e.split_once([':', '=']).map_or(e.as_str(), |(k, _)| k).trim().to_string())
            .collect();
          let missing: Vec<&str> = sc_pairs
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !keys.iter().any(|have| have == k))
            .collect();
          if !missing.is_empty() {
            out.errors.push(format!(
              "{name}: {dotted} is written inline and lacks {}; add them by hand or switch to the block form",
              missing.join(", ")
            ));
          }
          continue;
        }
        let items = tree::list_items(lines, &node);
        let map_form = items.is_empty() && !tree::children(lines, &node).is_empty();
        let present =
          |key: &str| items.iter().any(|it| it.text.starts_with(&format!("{key}="))) || tree::child(lines, &node, key).is_some();
        let indent = items.first().map_or(tree::child_indent(lines, &node), |it| it.indent);
        let mut added: Vec<String> = Vec::new();
        let mut new_lines: Vec<String> = Vec::new();
        for (it, (key, value)) in sc_items.iter().zip(&sc_pairs) {
          if present(key) {
            continue;
          }
          new_lines.push(if map_form {
            // Always double-quoted: a plain scalar could read as a sequence (`["a"]`), a
            // boolean (`no`), a number (`0800`), or a comment (` #`), and quoting is never
            // wrong for an environment value. Same escapes as `templates::substitute`.
            let quoted = format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""));
            format!("{}{key}: {quoted}", " ".repeat(indent))
          } else {
            format!("{}- {}", " ".repeat(indent), it.text)
          });
          added.push(key.to_string());
        }
        if !new_lines.is_empty() {
          out.edits.push(Edit::Insert {
            at: node.end,
            lines: new_lines,
          });
          out.details.push(format!("{name}: added environment {}", added.join(", ")));
        }
      }
      Kind::ExactList => {
        let have: Vec<String> = tree::list_items(lines, &node).into_iter().map(|i| i.text).collect();
        let want: Vec<String> = tree::list_items(sc_lines, &std).into_iter().map(|i| i.text).collect();
        // An inline form (`test: ["CMD", …]`) has a value and no items: always a mismatch.
        if have != want || node.is_inline() {
          out.edits.push(Edit::Replace {
            from: node.line,
            to: node.end,
            lines: tree::re_indent(&sc_lines[std.line..std.end], std.indent, node.indent),
          });
          out.details.push(format!("{name}: set {dotted} to the standard heartbeat check"));
        }
      }
    }
  }
  out
}

/// Top level: `networks.coolify.external: true`, inserted at whatever depth is missing.
pub fn top_level_edits(lines: &[String], sc_tail: &[String]) -> Outcome {
  let mut out = Outcome::default();
  let Some(sc_networks) = tree::top_level(sc_tail, "networks") else {
    return out;
  };
  let Some(networks) = tree::top_level(lines, "networks") else {
    // Append at the end, separated from whatever came before by one blank line.
    let mut new: Vec<String> = Vec::new();
    if lines.last().is_some_and(|l| !l.trim().is_empty()) {
      new.push(String::new());
    }
    new.extend(sc_tail[sc_networks.line..sc_networks.end].iter().cloned());
    out.edits.push(Edit::Insert {
      at: lines.len(),
      lines: new,
    });
    out.details.push("added networks.coolify".into());
    return out;
  };
  let sc_coolify = tree::child(sc_tail, &sc_networks, "coolify").expect("template tail has coolify");
  let sc_external = tree::child(sc_tail, &sc_coolify, "external").expect("template tail has external");
  // `external: true` inside a flow mapping (`{external: true}`).
  let flow_external = |value: &str| {
    tree::flow_entries(value).is_some_and(|pairs| {
      pairs.iter().any(|p| {
        p.split_once(':')
          .is_some_and(|(k, v)| k.trim() == "external" && v.trim() == sc_external.value)
      })
    })
  };
  // `networks: {coolify: {external: true}}`: nothing can be inserted under an inline
  // value, so it is judged in place and anything missing is left to the user.
  if networks.is_inline() {
    let ok = tree::flow_entries(&networks.value).is_some_and(|entries| {
      entries
        .iter()
        .any(|e| e.split_once(':').is_some_and(|(k, v)| k.trim() == "coolify" && flow_external(v)))
    });
    if !ok {
      out
        .errors
        .push("networks is written inline and lacks coolify with external: true; add it by hand or switch to the block form".into());
    }
    return out;
  }
  let Some(coolify) = tree::child(lines, &networks, "coolify") else {
    out.edits.push(Edit::Insert {
      at: networks.end,
      lines: subtree_under(lines, &networks, sc_tail, &sc_coolify),
    });
    out.details.push("added networks.coolify".into());
    return out;
  };
  if coolify.is_inline() {
    if !flow_external(&coolify.value) {
      out
        .errors
        .push("networks.coolify is written inline without external: true; set it by hand or switch to the block form".into());
    }
    return out;
  }
  match tree::child(lines, &coolify, "external") {
    None => {
      out.edits.push(Edit::Insert {
        at: coolify.end,
        lines: subtree_under(lines, &coolify, sc_tail, &sc_external),
      });
      out.details.push("added networks.coolify.external".into());
    }
    Some(e) if e.value != sc_external.value => {
      out.edits.push(Edit::SetValue {
        line: e.line,
        value: sc_external.value.clone(),
      });
      out.details.push(format!("set networks.coolify.external = {}", sc_external.value));
    }
    Some(_) => {}
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::compose::tree::{apply_edits, child, split_lines, top_level};

  /// The rendered scaffold block for service `app` (what the template produces for an
  /// aeth_ext user with origin o/r), parsed as its own document.
  const STD: &str = "\
services:
  app:
    # !rule exact
    container_name: app
    build:
      # !rule exact
      context: .
      # !rule exact
      dockerfile: docker/Dockerfile
      args:
        # !rule repo
        GIT_REPO: https://github.com/o/r.git
        # !rule presence
        GIT_TAG: {git_tag}
    # !rule presence
    restart: no
    # !rule volume-target
    volumes:
      - type: bind
        source: /data/app_files
        target: /app/persisted_data
    # !rule env-keys
    environment:
      - ALERTS_EMAIL=info@sweetfiretobacco.com
      - ALERTS_EMAIL_PWD=${ALERTS_EMAIL_PWD:?}
      - ALERTS_RECIPIENTS=[\"jacob.ogden@sweetfiretobacco.com\"]
    # !rule presence
    networks:
      - coolify
    healthcheck:
      # !rule exact-list
      test:
        - CMD-SHELL
        - bash -ec 'heartbeat'
      # !rule exact
      interval: 30s
      # !rule exact
      timeout: 5s
      # !rule exact
      retries: 3
      # !rule exact
      start_period: 15s
";
  const TAIL: &str = "\nnetworks:\n  coolify:\n    external: true\n";

  fn run(doc: &str) -> (String, Vec<String>) {
    let (out, o) = run_full(doc);
    (out, o.details)
  }

  fn run_full(doc: &str) -> (String, Outcome) {
    let lines = split_lines(doc);
    let (sc, rules) = parse_rules(&split_lines(STD), 2).unwrap();
    let sc_svc = child(&sc, &top_level(&sc, "services").unwrap(), "app").unwrap();
    let svc = child(&lines, &top_level(&lines, "services").unwrap(), "app").unwrap();
    let mut o = service_edits(&lines, &svc, &sc, &sc_svc, "app", &rules);
    let t = top_level_edits(&lines, &split_lines(TAIL));
    o.edits.extend(t.edits);
    o.details.extend(t.details);
    o.errors.extend(t.errors);
    (apply_edits(doc, &o.edits), o)
  }

  #[test]
  fn a_compliant_service_with_extras_needs_no_edits() {
    // aeth_ext's real shape: on-failure restart, map-form networks with aliases, labels,
    // expose, ssh-form GIT_REPO, extra env, comments — none of it is the standard's business.
    let doc = "\
services:

  app:
    container_name: app
    build:
      context: .
      dockerfile: docker/Dockerfile
      args:
        GIT_TAG: v8.0.8
        GIT_REPO: git@github.com:O/R.git
    restart: on-failure:3
    expose:
      - 8080
    volumes:
      - type: bind
        source: /data/central_log_server_files
        target: /app/persisted_data
    environment:
      # alerts
      - ALERTS_EMAIL=info@sweetfiretobacco.com
      - ALERTS_EMAIL_PWD=${ALERTS_EMAIL_PWD:?}
      - ALERTS_RECIPIENTS=[\"jacob.ogden@sweetfiretobacco.com\"]
      - EXTRA=1
    networks:
      coolify:
        aliases:
          - app
    labels:
      - \"traefik.x=y\"
    healthcheck:
      test:
        - CMD-SHELL
        - bash -ec 'heartbeat'
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 15s

networks:
  coolify:
    external: true
";
    let (out, details) = run(doc);
    assert_eq!(out, doc, "{details:?}");
    assert!(details.is_empty());
  }

  #[test]
  fn each_rule_kind_edits_exactly_its_key() {
    let doc = "\
services:
  app:
    container_name: other
    build:
      context: ./x
      args:
        GIT_REPO: https://github.com/someone/else.git
    volumes:
      - /tmp/a:/app/scratch
    environment:
      - ALERTS_EMAIL=info@sweetfiretobacco.com
      - KEEP=1
    healthcheck:
      test: [\"CMD\", \"true\"]
      interval: 60s
      timeout: 5s
      retries: 3
      start_period: 15s
";
    let (out, details) = run(doc);
    let want = "\
services:
  app:
    container_name: app
    build:
      context: .
      args:
        GIT_REPO: https://github.com/o/r.git
        GIT_TAG: {git_tag}
      dockerfile: docker/Dockerfile
    volumes:
      - /tmp/a:/app/scratch
      - type: bind
        source: /data/app_files
        target: /app/persisted_data
    environment:
      - ALERTS_EMAIL=info@sweetfiretobacco.com
      - KEEP=1
      - ALERTS_EMAIL_PWD=${ALERTS_EMAIL_PWD:?}
      - ALERTS_RECIPIENTS=[\"jacob.ogden@sweetfiretobacco.com\"]
    healthcheck:
      test:
        - CMD-SHELL
        - bash -ec 'heartbeat'
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 15s
    restart: no
    networks:
      - coolify

networks:
  coolify:
    external: true
";
    assert_eq!(out, want, "{details:?}");
    // container_name, context, dockerfile, GIT_REPO, GIT_TAG, restart, volume, environment,
    // test, interval, networks, top-level networks.
    assert_eq!(details.len(), 12, "{details:?}");
  }

  #[test]
  fn a_missing_intermediate_is_inserted_whole_and_a_scalar_intermediate_is_replaced() {
    let doc = "services:\n  app:\n    build: .\nnetworks:\n  other: {}\n";
    let (out, details) = run(doc);
    assert!(
      out.contains("    build:\n      context: .\n      dockerfile: docker/Dockerfile\n      args:\n"),
      "{out}"
    );
    assert!(out.contains("      GIT_TAG: {git_tag}\n"), "{out}");
    assert!(out.contains("networks:\n  other: {}\n  coolify:\n    external: true\n"), "{out}");
    assert!(details.iter().any(|d| d == "app: replaced build"), "{details:?}");
  }

  // A service whose only non-standard trait is the shape under test; every other key is
  // already compliant so the assertions see one rule at a time.
  fn service_with(volumes: &str, environment: &str, args: &str) -> String {
    format!(
      "services:
  app:
    container_name: app
    build:
      context: .
      dockerfile: docker/Dockerfile
      args:
{args}
    restart: no
    volumes:{volumes}
    environment:{environment}
    networks:
      - coolify
    healthcheck:
      test:
        - CMD-SHELL
        - bash -ec 'heartbeat'
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 15s

networks:
  coolify:
    external: true
"
    )
  }
  const ARGS_OK: &str = "        GIT_REPO: https://github.com/o/r.git
        GIT_TAG: v1";
  const ENV_OK: &str = " {ALERTS_EMAIL: a, ALERTS_EMAIL_PWD: b, ALERTS_RECIPIENTS: c}";

  #[test]
  fn inline_volumes_and_environment_are_judged_by_their_entries_and_never_edited() {
    // Compliant flow style, in every spelling: no edit, no error, no false drift.
    for (volumes, environment) in [
      (" [\"/data/x:/app/persisted_data\"]", ENV_OK),
      (
        " [{type: bind, source: /d, target: /app/persisted_data}]",
        " {ALERTS_EMAIL : a, ALERTS_EMAIL_PWD : b, ALERTS_RECIPIENTS : c}",
      ),
      (
        " ['/tmp/a:/app/scratch', /d:/app/persisted_data:ro]",
        " [ALERTS_EMAIL=a, \"ALERTS_EMAIL_PWD=b\", ALERTS_RECIPIENTS=c]",
      ),
    ] {
      let doc = service_with(volumes, environment, ARGS_OK);
      let (out, o) = run_full(&doc);
      assert_eq!(out, doc, "{volumes} / {environment}: {:?}", o.details);
      assert!(o.errors.is_empty(), "{volumes} / {environment}: {:?}", o.errors);
    }
    // Non-compliant flow style: still no edit (block items under a scalar are not YAML),
    // but an error naming what is missing. Substrings do not count: `/app/persisted_data_old`
    // is not the target, `OLD_ALERTS_EMAIL` is not `ALERTS_EMAIL`.
    let doc = service_with(
      " [\"/tmp/a:/app/persisted_data_old\", \"/app/persisted_data:/backup\"]",
      " {OLD_ALERTS_EMAIL: a, ALERTS_EMAIL_PWD: b}",
      ARGS_OK,
    );
    let (out, o) = run_full(&doc);
    assert_eq!(out, doc, "{:?}", o.details);
    assert_eq!(o.errors.len(), 2, "{:?}", o.errors);
    assert!(
      o.errors[0].contains("volumes") && o.errors[0].contains("/app/persisted_data"),
      "{:?}",
      o.errors
    );
    assert!(
      o.errors[1].contains("environment") && o.errors[1].contains("lacks ALERTS_EMAIL, ALERTS_RECIPIENTS"),
      "{:?}",
      o.errors
    );
    // An anchor or alias is not a flow collection either: noted, never edited.
    let doc = service_with(" *shared", " *shared", ARGS_OK);
    let (out, o) = run_full(&doc);
    assert_eq!(out, doc, "{:?}", o.details);
    assert_eq!(o.errors.len(), 2, "{:?}", o.errors);
  }

  #[test]
  fn an_anchored_block_is_a_block() {
    // `volumes: &v` + items and `build: &b` + children are walked like any block; the
    // anchor survives and only the real gaps are filled.
    let doc = service_with(
      " &vols\n      - /data/x:/app/persisted_data",
      " &env\n      - ALERTS_EMAIL=a\n      - ALERTS_EMAIL_PWD=b\n      - ALERTS_RECIPIENTS=c",
      ARGS_OK,
    )
    .replace("    build:\n", "    build: &b\n");
    let (out, o) = run_full(&doc);
    assert_eq!(out, doc, "{:?} {:?}", o.details, o.errors);
    let doc = doc.replace("      - ALERTS_RECIPIENTS=c\n", "");
    let (out, o) = run_full(&doc);
    assert!(out.contains("    environment: &env\n"), "{out}");
    assert!(
      out.contains("      - ALERTS_EMAIL_PWD=b\n      - ALERTS_RECIPIENTS=[\"jacob.ogden@sweetfiretobacco.com\"]\n"),
      "{out}"
    );
    assert!(o.errors.is_empty(), "{:?}", o.errors);
  }

  #[test]
  fn inline_networks_are_judged_in_place() {
    let block = "\n      - /data/x:/app/persisted_data";
    for (tail, ok) in [
      ("networks: {coolify: {external: true}}\n", true),
      ("networks: {other: {}, coolify: {external: true}}\n", true),
      ("networks: {coolify: {external: false}}\n", false),
      ("networks: {other: {external: true}}\n", false),
      ("networks:\n  coolify: {external: true}\n", true),
      ("networks:\n  coolify: {internal: true}\n", false),
    ] {
      let doc = service_with(block, ENV_OK, ARGS_OK).replace("networks:\n  coolify:\n    external: true\n", tail);
      assert!(doc.ends_with(tail), "{doc}");
      let (out, o) = run_full(&doc);
      assert_eq!(out, doc, "{tail}: {:?}", o.details);
      assert_eq!(o.errors.is_empty(), ok, "{tail}: {:?}", o.errors);
    }
  }

  #[test]
  fn a_list_form_parent_gets_one_error_instead_of_mapping_lines() {
    let doc = service_with(
      "
      - /data/x:/app/persisted_data",
      ENV_OK,
      "        - GIT_TAG=v1",
    );
    let (out, o) = run_full(&doc);
    assert_eq!(out, doc, "{:?}", o.details);
    assert_eq!(o.errors.len(), 1, "one error for args, not one per key: {:?}", o.errors);
    assert!(o.errors[0].contains("build.args is written as a list"), "{:?}", o.errors);
  }

  #[test]
  fn map_form_environment_values_are_always_quoted() {
    let doc = service_with(
      "
      - /data/x:/app/persisted_data",
      "
      FOO: bar",
      ARGS_OK,
    );
    let (out, details) = run(&doc);
    assert!(
      out.contains(
        "    environment:
      FOO: bar
      ALERTS_EMAIL: \"info@sweetfiretobacco.com\"
      ALERTS_EMAIL_PWD: \"${ALERTS_EMAIL_PWD:?}\"
      ALERTS_RECIPIENTS: \"[\\\"jacob.ogden@sweetfiretobacco.com\\\"]\"
"
      ),
      "{out}
{details:?}"
    );
  }

  #[test]
  fn the_repo_rule_skips_itself_without_an_origin() {
    let lines = split_lines("services:\n  app:\n    build:\n      args:\n        GIT_REPO: x\n");
    let (sc, rules) = parse_rules(&split_lines(&STD.replace("https://github.com/o/r.git", "")), 2).unwrap();
    let sc_svc = child(&sc, &top_level(&sc, "services").unwrap(), "app").unwrap();
    let svc = child(&lines, &top_level(&lines, "services").unwrap(), "app").unwrap();
    let o = service_edits(&lines, &svc, &sc, &sc_svc, "app", &rules);
    assert!(!o.details.iter().any(|d| d.contains("GIT_REPO")), "{:?}", o.details);
  }

  #[test]
  fn rules_are_read_from_annotations_and_stripped() {
    let block = split_lines(
      "  app:\n    # !rule exact\n    container_name: app\n    build:\n      # !rule exact\n      context: .\n      args:\n        # !rule repo\n        GIT_REPO: x\n    # !rule volume-target\n    volumes:\n      - type: bind\n        target: /app/persisted_data\n    healthcheck:\n      # !rule exact-list\n      test:\n        - CMD\n      # !rule exact\n      interval: 30s\n    plain: 1\n",
    );
    let (clean, rules) = parse_rules(&block, 1).unwrap();
    assert!(clean.iter().all(|l| !l.contains("!rule")), "{clean:?}");
    assert_eq!(clean.len(), block.len() - 6);
    let paths: Vec<(String, Kind)> = rules.iter().map(|r| (r.path.join("."), r.kind)).collect();
    assert_eq!(
      paths,
      vec![
        ("container_name".to_string(), Kind::Exact),
        ("build.context".to_string(), Kind::Exact),
        ("build.args.GIT_REPO".to_string(), Kind::Repo),
        ("volumes".to_string(), Kind::VolumeTarget),
        ("healthcheck.test".to_string(), Kind::ExactList),
        ("healthcheck.interval".to_string(), Kind::Exact),
      ]
    );
    for (bad, needle) in [
      ("  app:\n    # !rule exact\n    - item\n", "followed by a key"),
      ("  app:\n    # !rule exact\n", "annotates nothing"),
      ("  app:\n    # !rule bogus\n    x: 1\n", "unknown rule kind"),
      ("  app:\n    # !rule exact\n    # !rule exact\n    x: 1\n", "another rule line"),
    ] {
      let e = parse_rules(&split_lines(bad), 1).unwrap_err().to_string();
      assert!(e.contains(needle), "{bad:?}: {e}");
    }
  }
}
