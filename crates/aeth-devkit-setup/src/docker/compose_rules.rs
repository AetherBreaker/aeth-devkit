//! The compose standard as a rule table. Every standard *value* comes from the rendered
//! scaffold block for the same service, so the template is the single source of truth and
//! the rules only say which kind of check each key gets.

use aeth_devkit_core::compose::tree::{self, Edit, Node};
use aeth_devkit_core::github::normalize_repo;

/// How a key is compared with the scaffold (see the spec's rule table).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
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

const RULES: &[(&[&str], Kind)] = &[
  (&["container_name"], Kind::Exact),
  (&["build", "context"], Kind::Exact),
  (&["build", "dockerfile"], Kind::Exact),
  (&["build", "args", "GIT_REPO"], Kind::Repo),
  (&["build", "args", "GIT_TAG"], Kind::Presence),
  (&["restart"], Kind::Presence),
  (&["volumes"], Kind::VolumeTarget),
  (&["environment"], Kind::EnvKeys),
  (&["networks"], Kind::Presence),
  (&["healthcheck", "test"], Kind::ExactList),
  (&["healthcheck", "interval"], Kind::Exact),
  (&["healthcheck", "timeout"], Kind::Exact),
  (&["healthcheck", "retries"], Kind::Exact),
  (&["healthcheck", "start_period"], Kind::Exact),
];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Outcome {
  pub edits: Vec<Edit>,
  /// One human line per edit, for the change report.
  pub details: Vec<String>,
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
pub fn service_edits(lines: &[String], svc: &Node, sc_lines: &[String], sc_svc: &Node, name: &str) -> Outcome {
  let mut out = Outcome::default();
  // Dotted prefixes inserted or replaced wholesale; deeper rules under them are already
  // satisfied and must not add a second copy.
  let mut settled: Vec<String> = Vec::new();
  for (path, kind) in RULES {
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
        // `build: .` where a mapping is needed: replace the line with the scaffold block.
        Some(n) if !n.value.is_empty() => {
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
        let want = tree::list_items(sc_lines, &std)
          .first()
          .and_then(|it| tree::item_child(sc_lines, it, "target"))
          .map(|t| t.value)
          .unwrap_or_default();
        let items = tree::list_items(lines, &node);
        let mounted = items.iter().any(|it| {
          tree::item_child(lines, it, "target").is_some_and(|t| t.value == want)
            // Short form `source:target[:mode]`.
            || it.text.split(':').nth(1) == Some(want.as_str())
        });
        if !mounted {
          let indent = items.first().map_or(tree::child_indent(lines, &node), |it| it.indent);
          let sc_items = tree::list_items(sc_lines, &std);
          let sc_item = &sc_items[0];
          out.edits.push(Edit::Insert {
            at: node.end,
            lines: tree::re_indent(&sc_lines[sc_item.line..sc_item.end], sc_item.indent, indent),
          });
          out.details.push(format!("{name}: added the {want} bind mount"));
        }
      }
      Kind::EnvKeys => {
        let items = tree::list_items(lines, &node);
        let map_form = items.is_empty() && !tree::children(lines, &node).is_empty();
        let present =
          |key: &str| items.iter().any(|it| it.text.starts_with(&format!("{key}="))) || tree::child(lines, &node, key).is_some();
        let indent = items.first().map_or(tree::child_indent(lines, &node), |it| it.indent);
        let mut added: Vec<String> = Vec::new();
        let mut new_lines: Vec<String> = Vec::new();
        for it in tree::list_items(sc_lines, &std) {
          let (key, value) = it.text.split_once('=').unwrap_or((&it.text, ""));
          if present(key) {
            continue;
          }
          new_lines.push(if map_form {
            format!("{}{key}: {value}", " ".repeat(indent))
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
        if have != want || !node.value.is_empty() {
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
  let Some(coolify) = tree::child(lines, &networks, "coolify") else {
    out.edits.push(Edit::Insert {
      at: networks.end,
      lines: subtree_under(lines, &networks, sc_tail, &sc_coolify),
    });
    out.details.push("added networks.coolify".into());
    return out;
  };
  let sc_external = tree::child(sc_tail, &sc_coolify, "external").expect("template tail has external");
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
    container_name: app
    build:
      context: .
      dockerfile: docker/Dockerfile
      args:
        GIT_REPO: https://github.com/o/r.git
        GIT_TAG: {git_tag}
    restart: no
    volumes:
      - type: bind
        source: /data/app_files
        target: /app/persisted_data
    environment:
      - ALERTS_EMAIL=info@sweetfiretobacco.com
      - ALERTS_EMAIL_PWD=${ALERTS_EMAIL_PWD:?}
      - ALERTS_RECIPIENTS=[\"jacob.ogden@sweetfiretobacco.com\"]
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
";
  const TAIL: &str = "\nnetworks:\n  coolify:\n    external: true\n";

  fn run(doc: &str) -> (String, Vec<String>) {
    let lines = split_lines(doc);
    let sc = split_lines(STD);
    let sc_svc = child(&sc, &top_level(&sc, "services").unwrap(), "app").unwrap();
    let svc = child(&lines, &top_level(&lines, "services").unwrap(), "app").unwrap();
    let mut o = service_edits(&lines, &svc, &sc, &sc_svc, "app");
    let t = top_level_edits(&lines, &split_lines(TAIL));
    o.edits.extend(t.edits);
    o.details.extend(t.details);
    (apply_edits(doc, &o.edits), o.details)
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

  #[test]
  fn the_repo_rule_skips_itself_without_an_origin() {
    let lines = split_lines("services:\n  app:\n    build:\n      args:\n        GIT_REPO: x\n");
    let sc = split_lines(&STD.replace("https://github.com/o/r.git", ""));
    let sc_svc = child(&sc, &top_level(&sc, "services").unwrap(), "app").unwrap();
    let svc = child(&lines, &top_level(&lines, "services").unwrap(), "app").unwrap();
    let o = service_edits(&lines, &svc, &sc, &sc_svc, "app");
    assert!(!o.details.iter().any(|d| d.contains("GIT_REPO")), "{:?}", o.details);
  }
}
