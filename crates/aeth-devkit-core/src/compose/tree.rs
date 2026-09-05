//! Nested key lookup and line-level edits for compose files.
//!
//! Same philosophy as the parent module: every function works on the file's own lines and
//! never re-serialises YAML, so comments, ordering, quoting and blank lines survive. Only
//! the small YAML subset compose files use is understood: block mappings, block sequences
//! (`- item`, including zero-indented ones), and inline scalars.

use std::cmp::Reverse;

use super::{indent_of, key_value, replace_value, unquote};

/// A `key: value` mapping line and the extent of its subtree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
  pub key: String,
  /// The inline value (quotes and trailing ` # comment` stripped); empty for a block.
  pub value: String,
  /// 0-based line index of the `key:` line.
  pub line: usize,
  pub indent: usize,
  /// Exclusive end of the subtree. Trailing blank and comment lines are left outside so an
  /// insertion at `end` lands directly under the last real child.
  pub end: usize,
}

impl Node {
  /// The whole value sits on the key's line — a scalar or a flow collection (`[…]`,
  /// `{…}`) — so nothing can be inserted beneath it. An anchored block (`volumes: &v`
  /// followed by items) is not inline: its value is the anchor, its content the block.
  pub fn is_inline(&self) -> bool {
    !self.value.is_empty() && self.end == self.line + 1
  }
}

/// One `- ` entry of a block sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListItem {
  pub line: usize,
  pub indent: usize,
  /// What follows the dash, unquoted, trailing comment stripped.
  pub text: String,
  /// Exclusive end: continuation lines (`  source: …` under `- type: bind`) belong to it.
  pub end: usize,
}

/// One line-level change. Positions refer to the *original* text; [`apply_edits`] orders
/// them so they never invalidate each other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
  /// Rewrite the value on a `key: value` line (see [`replace_value`]).
  SetValue { line: usize, value: String },
  /// Insert already-indented lines before `at` (`at == len` appends).
  Insert { at: usize, lines: Vec<String> },
  /// Replace lines `[from, to)` with already-indented lines.
  Replace { from: usize, to: usize, lines: Vec<String> },
}

pub fn split_lines(text: &str) -> Vec<String> {
  // `str::lines` splits on `\n` and strips a trailing `\r`, so CRLF files parse the same.
  text.lines().map(str::to_string).collect()
}

fn is_blank_or_comment(line: &str) -> bool {
  let t = line.trim();
  t.is_empty() || t.starts_with('#')
}

fn is_list_item(line: &str) -> bool {
  let t = line.trim_start();
  t == "-" || t.starts_with("- ")
}

/// Exclusive end of the block starting at `line` with `indent`. A later content line at
/// `indent` or less ends it — except a list item *at* `indent` when `same_indent_items`
/// is set, which YAML allows as a zero-indented sequence under a mapping key.
fn block_end(lines: &[String], line: usize, indent: usize, same_indent_items: bool) -> usize {
  let mut end = line + 1;
  // `skip` starts the enumeration after the block's own line; `enumerate` first so the
  // indices are still the original line numbers.
  for (i, l) in lines.iter().enumerate().skip(line + 1) {
    if is_blank_or_comment(l) {
      continue; // decided by the next content line, so blanks never extend `end`
    }
    let ind = indent_of(l);
    if ind < indent || (ind == indent && !(same_indent_items && is_list_item(l))) {
      break;
    }
    end = i + 1;
  }
  end
}

fn node_at(lines: &[String], i: usize) -> Option<Node> {
  let l = &lines[i];
  if is_blank_or_comment(l) || is_list_item(l) {
    return None;
  }
  let (key, value) = key_value(l)?;
  let indent = indent_of(l);
  Some(Node {
    key: key.to_string(),
    value,
    line: i,
    indent,
    end: block_end(lines, i, indent, true),
  })
}

/// Mapping children found in `[start, end)` at exactly `indent`.
fn children_in(lines: &[String], start: usize, end: usize, indent: usize) -> Vec<Node> {
  let mut out = Vec::new();
  let mut i = start;
  while i < end {
    match node_at(lines, i) {
      // Jump past the child's whole subtree so a deeper key with the same name is not
      // mistaken for a sibling.
      Some(n) if n.indent == indent => {
        i = n.end;
        out.push(n);
      }
      _ => i += 1,
    }
  }
  out
}

pub fn top_level(lines: &[String], key: &str) -> Option<Node> {
  children_in(lines, 0, lines.len(), 0).into_iter().find(|n| n.key == key)
}

/// Indent of `node`'s first content line, or `node.indent + 2` for an empty block (the
/// step every sister compose file uses).
pub fn child_indent(lines: &[String], node: &Node) -> usize {
  lines[node.line + 1..node.end]
    .iter()
    .find(|l| !is_blank_or_comment(l))
    .map(|l| indent_of(l))
    .unwrap_or(node.indent + 2)
}

pub fn children(lines: &[String], node: &Node) -> Vec<Node> {
  children_in(lines, node.line + 1, node.end, child_indent(lines, node))
}

pub fn child(lines: &[String], node: &Node, key: &str) -> Option<Node> {
  children(lines, node).into_iter().find(|n| n.key == key)
}

/// Follow `path` down from `node`; `None` as soon as one segment is missing.
pub fn descend(lines: &[String], node: &Node, path: &[&str]) -> Option<Node> {
  // `try_fold` threads the current node through each key and stops at the first `None`.
  path.iter().try_fold(node.clone(), |n, k| child(lines, &n, k))
}

/// The `- ` items directly under `node`.
pub fn list_items(lines: &[String], node: &Node) -> Vec<ListItem> {
  let ind = child_indent(lines, node);
  let mut out = Vec::new();
  let mut i = node.line + 1;
  while i < node.end {
    let l = &lines[i];
    if !is_blank_or_comment(l) && indent_of(l) == ind && is_list_item(l) {
      // An item's continuation lines sit deeper than the dash; the next item at `ind`
      // ends it (hence `same_indent_items = false`).
      let end = block_end(lines, i, ind, false);
      // Everything after the dash: `- type: bind` → `type: bind`.
      let text = unquote(l.trim_start()[1..].trim_start());
      out.push(ListItem {
        line: i,
        indent: ind,
        text,
        end,
      });
      i = end;
    } else {
      i += 1;
    }
  }
  out
}

/// `key` inside a mapping item: the first pair sits on the dash line (`- type: bind`),
/// the rest two columns deeper (`  source: …`).
pub fn item_child(lines: &[String], item: &ListItem, key: &str) -> Option<Node> {
  // A let-chain: bind the pair *and* test the key in one condition.
  if let Some((k, v)) = key_value(&item.text)
    && k == key
  {
    return Some(Node {
      key: k.to_string(),
      value: v,
      line: item.line,
      indent: item.indent + 2,
      end: item.end,
    });
  }
  children_in(lines, item.line + 1, item.end, item.indent + 2)
    .into_iter()
    .find(|n| n.key == key)
}

/// The entries of a flow collection: `[a, "b"]` → `a`, `b`; `{k: v, k2: {x: y}}` → `k: v`,
/// `k2: {x: y}`. Split at top-level commas (quotes and nesting respected), each unquoted
/// and trimmed, so a nested collection comes back as text this same function can parse.
/// `None` when `value` is not a flow collection (a plain scalar, an anchor, an alias).
pub fn flow_entries(value: &str) -> Option<Vec<String>> {
  let v = value.trim();
  let inner = v
    .strip_prefix('[')
    .and_then(|s| s.strip_suffix(']'))
    .or_else(|| v.strip_prefix('{').and_then(|s| s.strip_suffix('}')))?;
  let mut out = Vec::new();
  let (mut depth, mut quote, mut start) = (0usize, None::<char>, 0usize);
  for (i, c) in inner.char_indices() {
    match (quote, c) {
      (Some(q), c) if c == q => quote = None,
      (Some(_), _) => {}
      (None, '"' | '\'') => quote = Some(c),
      (None, '[' | '{') => depth += 1,
      (None, ']' | '}') => depth = depth.saturating_sub(1),
      (None, ',') if depth == 0 => {
        out.push(&inner[start..i]);
        start = i + 1;
      }
      _ => {}
    }
  }
  out.push(&inner[start..]);
  Some(out.iter().map(|e| e.trim()).filter(|e| !e.is_empty()).map(unquote).collect())
}

/// Shift every non-blank line's indentation by `to - from` (either direction).
pub fn re_indent(lines: &[String], from: usize, to: usize) -> Vec<String> {
  lines
    .iter()
    .map(|l| {
      if l.trim().is_empty() {
        return String::new();
      }
      // Signed arithmetic through `isize` so a leftward shift cannot underflow; a line
      // shallower than `from` (impossible for a well-formed subtree) clamps at zero.
      let ind = (indent_of(l) as isize + to as isize - from as isize).max(0) as usize;
      format!("{}{}", " ".repeat(ind), l.trim_start())
    })
    .collect()
}

/// Apply `edits` to `text`. Line endings and the presence of a final newline are kept.
///
/// Edits are applied from the highest line downwards so earlier indices stay valid. At
/// one position: in-place edits (`SetValue`, `Replace`) go before insertions; among
/// insertions the *shallower*-indented one is applied first, so it ends up below (it
/// belongs to the enclosing block, which closes last — think `args` and `build` ending on
/// the same line); at equal indent the later-registered one is applied first, which
/// leaves the earlier one above it, i.e. registration order is document order.
pub fn apply_edits(text: &str, edits: &[Edit]) -> String {
  let mut lines = split_lines(text);
  let position = |e: &Edit| match e {
    Edit::SetValue { line, .. } => *line,
    Edit::Insert { at, .. } => *at,
    Edit::Replace { from, .. } => *from,
  };
  let rank = |e: &Edit| match e {
    Edit::SetValue { .. } => 0,
    Edit::Replace { .. } => 1,
    Edit::Insert { .. } => 2,
  };
  let indent = |e: &Edit| match e {
    // Only insertions compete for one slot; their first non-blank line decides.
    Edit::Insert { lines, .. } => lines.iter().find(|l| !l.trim().is_empty()).map_or(0, |l| indent_of(l)),
    _ => 0,
  };
  let mut ordered: Vec<(usize, &Edit)> = edits.iter().enumerate().collect();
  // `Reverse` flips the ordering of a key; sort keys are compared lexicographically.
  ordered.sort_by_key(|(i, e)| (Reverse(position(e)), rank(e), indent(e), Reverse(*i)));
  for (_, e) in ordered {
    match e {
      Edit::SetValue { line, value } => {
        if let Some(l) = lines.get_mut(*line) {
          *l = replace_value(l, value);
        }
      }
      Edit::Insert { at, lines: new } => {
        let at = (*at).min(lines.len());
        // `splice` with an empty range inserts without removing anything.
        lines.splice(at..at, new.iter().cloned());
      }
      Edit::Replace { from, to, lines: new } => {
        let to = (*to).min(lines.len());
        lines.splice(*from..to, new.iter().cloned());
      }
    }
  }
  let nl = if text.contains("\r\n") { "\r\n" } else { "\n" };
  let mut out = lines.join(nl);
  if text.ends_with('\n') {
    out.push_str(nl);
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  const DOC: &str = "\
services:

  app:
    container_name: app
    build:
      context: .
      args:
        GIT_TAG: v1  # pinned
    volumes:
      - type: bind
        source: /data/app
        target: /app/persisted_data
      - /tmp/x:/app/scratch
    environment:
      # a comment
      - A=1
    networks:
    - coolify
  side:
    image: x

networks:
  coolify:
    external: true
";

  fn lines() -> Vec<String> {
    split_lines(DOC)
  }

  #[test]
  fn top_level_and_children_with_subtree_ends() {
    let l = lines();
    let services = top_level(&l, "services").unwrap();
    assert_eq!((services.line, services.indent), (0, 0));
    // Ends before the blank line that precedes `networks:`.
    assert_eq!(l[services.end - 1].trim(), "image: x");
    let kids = children(&l, &services);
    assert_eq!(kids.iter().map(|n| n.key.as_str()).collect::<Vec<_>>(), ["app", "side"]);
    let app = &kids[0];
    assert_eq!(l[app.end - 1].trim(), "- coolify", "a zero-indented list still belongs to its key");
    assert!(top_level(&l, "nope").is_none());
  }

  #[test]
  fn descend_and_values() {
    let l = lines();
    let app = child(&l, &top_level(&l, "services").unwrap(), "app").unwrap();
    let tag = descend(&l, &app, &["build", "args", "GIT_TAG"]).unwrap();
    assert_eq!(tag.value, "v1", "trailing comment stripped");
    assert_eq!(descend(&l, &app, &["build", "context"]).unwrap().value, ".");
    assert!(descend(&l, &app, &["build", "dockerfile"]).is_none());
    assert_eq!(child_indent(&l, &app), 4);
    let nets = child(&l, &app, "networks").unwrap();
    assert_eq!(child_indent(&l, &nets), 4, "zero-indented sequence");
    let build = child(&l, &app, "build").unwrap();
    assert!(build.value.is_empty());
  }

  #[test]
  fn list_items_and_item_children() {
    let l = lines();
    let app = child(&l, &top_level(&l, "services").unwrap(), "app").unwrap();
    let vols = child(&l, &app, "volumes").unwrap();
    let items = list_items(&l, &vols);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "type: bind");
    assert_eq!(item_child(&l, &items[0], "target").unwrap().value, "/app/persisted_data");
    assert_eq!(item_child(&l, &items[0], "type").unwrap().value, "bind", "the dash line itself");
    assert!(item_child(&l, &items[0], "nope").is_none());
    assert_eq!(items[1].text, "/tmp/x:/app/scratch");
    let env = child(&l, &app, "environment").unwrap();
    assert_eq!(list_items(&l, &env).iter().map(|i| i.text.as_str()).collect::<Vec<_>>(), ["A=1"]);
    let empty = child(&l, &app, "container_name").unwrap();
    assert!(list_items(&l, &empty).is_empty());
  }

  #[test]
  fn re_indent_shifts_every_nonblank_line() {
    let src = split_lines("    a:\n      b: 1\n\n      c: 2");
    assert_eq!(re_indent(&src, 4, 2), ["  a:", "    b: 1", "", "    c: 2"]);
    assert_eq!(re_indent(&src, 4, 6), ["      a:", "        b: 1", "", "        c: 2"]);
  }

  #[test]
  fn apply_edits_keeps_indices_stable_and_orders_same_position_edits() {
    let text = "a\nb\nc\n";
    let edits = [
      Edit::Insert {
        at: 1,
        lines: vec!["x".into()],
      },
      Edit::SetValue {
        line: 1,
        value: "B".into(),
      }, // applies to the original line 1 (`b`)
      Edit::Insert {
        at: 1,
        lines: vec!["y".into()],
      },
      Edit::Replace {
        from: 2,
        to: 3,
        lines: vec!["C1".into(), "C2".into()],
      },
    ];
    // `b` has no colon, so SetValue leaves it alone — use a mapping line to see it.
    let out = apply_edits(&text.replace("b", "k: b"), &edits);
    assert_eq!(out, "a\nx\ny\nk: B\nC1\nC2\n");
  }

  #[test]
  fn deeper_inserts_at_one_position_land_above_shallower_ones() {
    // Two blocks closing on the same line: `args` (inner) and `build` (outer). A new key
    // for each is inserted at the same index; the deeper one belongs to the inner block
    // and must come first, whatever order the rules registered them in.
    let text = "build:\n  args:\n    A: 1\nnext: x\n";
    let edits = [
      Edit::Insert {
        at: 3,
        lines: vec!["  dockerfile: d".into()],
      },
      Edit::Insert {
        at: 3,
        lines: vec!["    B: 2".into()],
      },
    ];
    assert_eq!(
      apply_edits(text, &edits),
      "build:\n  args:\n    A: 1\n    B: 2\n  dockerfile: d\nnext: x\n"
    );
  }

  #[test]
  fn flow_entries_split_at_top_level_commas_only() {
    assert_eq!(
      flow_entries("[\"/d:/app/x\", '/e:/app/y', {type: bind, target: /app/z}]").unwrap(),
      ["/d:/app/x", "/e:/app/y", "{type: bind, target: /app/z}"]
    );
    assert_eq!(
      flow_entries("{A: 1, B: \"x, y\", C: [1, 2]}").unwrap(),
      ["A: 1", "B: \"x, y\"", "C: [1, 2]"]
    );
    assert_eq!(flow_entries("[]").unwrap(), Vec::<String>::new());
    assert!(flow_entries("&shared").is_none() && flow_entries("*shared").is_none() && flow_entries("no").is_none());
    let lines = split_lines("a: [1]\nb: &x\n  - 1\nc:\n  - 1\nd: no\n");
    let node = |k| top_level(&lines, k).unwrap();
    assert!(node("a").is_inline() && node("d").is_inline());
    assert!(!node("b").is_inline() && !node("c").is_inline());
  }

  #[test]
  fn apply_edits_preserves_crlf_and_missing_final_newline() {
    let out = apply_edits(
      "a\r\nb: 1\r\n",
      &[Edit::SetValue {
        line: 1,
        value: "2".into(),
      }],
    );
    assert_eq!(out, "a\r\nb: 2\r\n");
    let out = apply_edits(
      "a\nb",
      &[Edit::Insert {
        at: 2,
        lines: vec!["c".into()],
      }],
    );
    assert_eq!(out, "a\nb\nc");
  }
}
