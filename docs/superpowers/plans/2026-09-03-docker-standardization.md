# Docker Standardization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `devkit setup-project` owns each project's Docker configuration: a templated `docker/Dockerfile`, rule-checked compose file, and a static `devkit-container` entrypoint binary built by the release workflow, with every replacement behind explicit per-file consent.

**Architecture:** Three mostly independent pieces. (1) A new `aeth-devkit-container` crate: a clap binary with `app-extra`, `readme`, and the Unix-only `run` entrypoint (mount check, mkdir/chown, privilege drop, exec). (2) The setup crate grows a `docker` module: static-file replace with consent, a compose scaffold rendered from a template, and a rule table whose standard values are read from that same rendered scaffold; nested compose lookup and line edits live in a new `aeth_devkit_core::compose::tree` module. (3) The Rust release workflow template gains a container-binary build and upload per platform.

**Tech Stack:** Rust 2024 (workspace crates), `toml_edit`, `clap`, `similar` (unified diffs), `nix` (`user` feature, Unix only), GitHub Actions, uv.

**Spec:** `docs/superpowers/specs/2026-09-03-docker-standardization-design.md` — read it first; every task below argues from it.

## Global Constraints

- Work on a feature branch (`feat/docker-standardization`), never on `main`. Conventional Commits: `<type>(<scope>): <summary>`; a `fix` body says what/why/how.
- Rust style: `rustfmt.toml` = 2-space indent, `max_width = 135`. Run `cargo fmt --all` before every commit.
- **Rust teaching comments:** this repo's Rust is study material. Comment densely inside function bodies explaining both the Rust idiom (ownership, `?`, traits, `Cell`, let-chains, `cfg`) and the logic, in the voice of `crates/aeth-devkit-core/src/process.rs`. Doc comments (`///`) on every item. Keep each comment precise; no restating.
- Tests carry no docstrings (Python) and no `///` on test fns (Rust). Never cite a test as justification for behaviour.
- Run tests under `uv run` for Python; `cargo test -p <crate>` for Rust. On this branch run only targeted tests per task, and the full suite once at the very end (Task 13).
- Exact strings from the spec (copy verbatim):
  - uid/gid `999`, user `nonroot`; asset names `devkit-container-x86_64-unknown-linux-musl`, `devkit-container-x86_64-pc-windows-msvc.exe`; download URL `https://github.com/AetherBreaker/aeth-devkit/releases/download/v{devkit_version}/devkit-container-x86_64-unknown-linux-musl`.
  - Prompts: `Replace docker/Dockerfile? [replace / replace all / anything else keeps it]:`; `Apply these edits to docker/compose.yaml? [replace / replace all / anything else keeps it]:`; `Service "x" is not in docker/compose.yaml (found: a, b). Add it? [add / anything else skips]:`.
  - Answers: `replace`, `replace all`, `add`; anything else keeps/skips.
  - Flag `--replace-docker`. Placeholders `{devkit_version}`, `{git_repo}`, `{git_tag}`, `{service}`, `{python_dir}`.
  - `[tool.docker]` keys: `services`, `required_persisted_dirs`; legacy `chown_paths`, `mkdirs` are only ever reported.
  - Entrypoint entry validation: empty, `.`, `..`, absolute, or escaping `/app` is a hard error.
- Dependencies: `toml_edit` (existing), `similar = "3.2.0"` (new, setup crate), `nix = { version = "0.31.3", features = ["user"] }` (new, container crate, `cfg(unix)` only — `nix` does not compile on Windows).
- Nothing under `docker/` is ever deleted by devkit; stray files are only reported.

---

## File map

| Path | Responsibility |
| --- | --- |
| `crates/aeth-devkit-core/src/prompt.rs` | `Prompt`, `StdinPrompt`, `ScriptedPrompt` (moved from release) |
| `crates/aeth-devkit-core/src/compose/tree.rs` | nested key lookup, list items, line edits, re-indent |
| `crates/aeth-devkit-setup/src/context.rs` | `services` switch, `name`/`version`/`origin`, legacy keys |
| `crates/aeth-devkit-setup/src/templates.rs` | new placeholders, generic `gate` |
| `crates/aeth-devkit-setup/src/docker/mod.rs` | `Deps`, `Consent`, orchestration, compose flow, advisories |
| `crates/aeth-devkit-setup/src/docker/static_files.rs` | whole-file replace, diff, stray note |
| `crates/aeth-devkit-setup/src/docker/scaffold.rs` | compose scaffold split/render, lazy `{git_tag}` |
| `crates/aeth-devkit-setup/src/docker/compose_rules.rs` | rule table + edit computation |
| `crates/aeth-devkit-setup/src/git.rs` | dynamic committable list |
| `crates/aeth-devkit-setup/src/cli.rs`, `src/lib.rs` | flag, deps, step wiring |
| `crates/aeth-devkit-container/**` | the container binary |
| `python/aeth_devkit/templates/docker/template.Dockerfile`, `compose.template.yaml` | templates |
| `python/aeth_devkit/templates/pyproject.template.toml` | `[tool.docker]` schema |
| `python/aeth_devkit/templates/github/workflows/release.rust.template.yml` | container build matrix |
| `README.md`, `TODO.md`, `python/aeth_devkit/_tasks_source.py` | docs |

---

### Task 1: Move the `Prompt` trait to core

**Files:**
- Create: `crates/aeth-devkit-core/src/prompt.rs`
- Modify: `crates/aeth-devkit-core/src/lib.rs`, `crates/aeth-devkit-release/src/prompt.rs`

**Interfaces:**
- Produces: `aeth_devkit_core::prompt::{Prompt, StdinPrompt, ScriptedPrompt}` with the exact signatures the release crate has today (`Prompt::ask(&self, question: &str) -> Result<String>`, `ScriptedPrompt::new(&[&str])`, public `answers`/`asked` `RefCell`s).

- [ ] **Step 1: Create the core module**

Move everything from `crates/aeth-devkit-release/src/prompt.rs` **except** `confirm_force` and its test into `crates/aeth-devkit-core/src/prompt.rs`, keeping the module doc but rewording its second paragraph:

```rust
//! Asking the user a question on the terminal, with a scripted stand-in for tests.
//!
//! `devkit release` needs a human for "dirty tree, continue?" and "artefacts exist,
//! remove them?"; `devkit setup-project` needs one before replacing a Docker file. The
//! prompt is a trait so tests feed canned answers and assert which questions were asked.
```

Keep `StdinPrompt`, `ScriptedPrompt`, their impls, and the `scripted_prompt_answers_in_order_then_errors` test verbatim.

- [ ] **Step 2: Register it and re-export from release**

In `crates/aeth-devkit-core/src/lib.rs` add `pub mod prompt;` (alphabetical, after `process`). Replace `crates/aeth-devkit-release/src/prompt.rs` with:

```rust
//! The release-specific prompt helper; the trait and its implementations live in core so
//! `setup-project` can share them.

use anyhow::Result;

// Re-exported so every `crate::prompt::…` path in this crate and its tests keeps working.
pub use aeth_devkit_core::prompt::{Prompt, ScriptedPrompt, StdinPrompt};

/// `true` when `force` is already set (the `--force` flag) or the user types exactly
/// `force`. Anything else — including `y`, `yes`, an empty line — is a refusal.
pub fn confirm_force(prompt: &dyn Prompt, force: bool, question: &str) -> Result<bool> {
  if force {
    return Ok(true);
  }
  Ok(prompt.ask(question)? == "force")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn confirm_force_requires_the_word_force() {
    assert!(confirm_force(&ScriptedPrompt::new(&["force"]), false, "q").unwrap());
    assert!(!confirm_force(&ScriptedPrompt::new(&["y"]), false, "q").unwrap());
    // With `force == true` the prompt must not even be consulted.
    let p = ScriptedPrompt::new(&[]);
    assert!(confirm_force(&p, true, "q").unwrap());
    assert!(p.asked.borrow().is_empty());
  }
}
```

- [ ] **Step 3: Verify**

Run: `cargo test -p aeth-devkit-core prompt` and `cargo test -p aeth-devkit-release`
Expected: all pass (release tests unchanged).

- [ ] **Step 4: Commit**

```bash
git add crates/aeth-devkit-core/src/prompt.rs crates/aeth-devkit-core/src/lib.rs crates/aeth-devkit-release/src/prompt.rs
git commit -m "refactor(core): move the Prompt trait to core for setup-project"
```

---

### Task 2: Compose tree — nested lookup and line edits

**Files:**
- Create: `crates/aeth-devkit-core/src/compose/tree.rs`
- Modify: `crates/aeth-devkit-core/src/compose.rs` (add `pub mod tree;` after the imports)

**Interfaces:**
- Consumes: `compose::key_value` (private fn in the parent module — a child module may use it as `super::key_value`) and `compose::replace_value`.
- Produces (all `pub`, used by Task 6):
  - `struct Node { key: String, value: String, line: usize, indent: usize, end: usize }` — `end` is the exclusive end of the subtree, trailing blank/comment lines excluded.
  - `struct ListItem { line: usize, indent: usize, text: String, end: usize }` — `text` is what follows `- `, unquoted, trailing ` #` comment stripped.
  - `enum Edit { SetValue { line, value }, Insert { at, lines }, Replace { from, to, lines } }`.
  - `fn split_lines(&str) -> Vec<String>`, `fn top_level(&[String], &str) -> Option<Node>`, `fn children(&[String], &Node) -> Vec<Node>`, `fn child(&[String], &Node, &str) -> Option<Node>`, `fn descend(&[String], &Node, &[&str]) -> Option<Node>`, `fn child_indent(&[String], &Node) -> usize`, `fn list_items(&[String], &Node) -> Vec<ListItem>`, `fn item_child(&[String], &ListItem, &str) -> Option<Node>`, `fn re_indent(&[String], from: usize, to: usize) -> Vec<String>`, `fn apply_edits(&str, &[Edit]) -> String`.

- [ ] **Step 1: Write the failing tests** (bottom of the new file)

```rust
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
      Edit::Insert { at: 1, lines: vec!["x".into()] },
      Edit::SetValue { line: 1, value: "B".into() }, // applies to the original line 1 (`b`)
      Edit::Insert { at: 1, lines: vec!["y".into()] },
      Edit::Replace { from: 2, to: 3, lines: vec!["C1".into(), "C2".into()] },
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
      Edit::Insert { at: 3, lines: vec!["  dockerfile: d".into()] },
      Edit::Insert { at: 3, lines: vec!["    B: 2".into()] },
    ];
    assert_eq!(apply_edits(text, &edits), "build:\n  args:\n    A: 1\n    B: 2\n  dockerfile: d\nnext: x\n");
  }

  #[test]
  fn apply_edits_preserves_crlf_and_missing_final_newline() {
    let out = apply_edits("a\r\nb: 1\r\n", &[Edit::SetValue { line: 1, value: "2".into() }]);
    assert_eq!(out, "a\r\nb: 2\r\n");
    let out = apply_edits("a\nb", &[Edit::Insert { at: 2, lines: vec!["c".into()] }]);
    assert_eq!(out, "a\nb\nc");
  }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p aeth-devkit-core compose::tree`
Expected: compile error — module does not exist.

- [ ] **Step 3: Implement**

`crates/aeth-devkit-core/src/compose/tree.rs` (add `pub mod tree;` to `compose.rs`):

```rust
//! Nested key lookup and line-level edits for compose files.
//!
//! Same philosophy as the parent module: every function works on the file's own lines and
//! never re-serialises YAML, so comments, ordering, quoting and blank lines survive. Only
//! the small YAML subset compose files use is understood: block mappings, block sequences
//! (`- item`, including zero-indented ones), and inline scalars.

use std::cmp::Reverse;

use super::{key_value, replace_value};

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

fn indent_of(line: &str) -> usize {
  line.len() - line.trim_start().len()
}

fn is_blank_or_comment(line: &str) -> bool {
  let t = line.trim();
  t.is_empty() || t.starts_with('#')
}

fn is_list_item(line: &str) -> bool {
  let t = line.trim_start();
  t == "-" || t.starts_with("- ")
}

/// Surrounding matching quotes removed, trailing ` # comment` dropped.
fn unquote(s: &str) -> String {
  let v = s.split(" #").next().unwrap_or(s).trim();
  let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
  let v = v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')).unwrap_or(v);
  v.to_string()
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
      out.push(ListItem { line: i, indent: ind, text, end });
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
  children_in(lines, item.line + 1, item.end, item.indent + 2).into_iter().find(|n| n.key == key)
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
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p aeth-devkit-core compose`
Expected: all new tests pass and the existing `compose` tests still pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/aeth-devkit-core/src/compose.rs crates/aeth-devkit-core/src/compose/tree.rs
git commit -m "feat(core): nested lookup and line edits for compose files"
```

---

### Task 3: `[tool.docker].services` becomes the Docker switch

**Files:**
- Modify: `crates/aeth-devkit-setup/src/context.rs`, `crates/aeth-devkit-setup/src/lib.rs` (step 14 advisory), `crates/aeth-devkit-setup/src/toml_merge.rs` (test ctx), `crates/aeth-devkit-setup/src/templates.rs` (two test ctxs), `crates/aeth-devkit-setup/src/md_block.rs` (test ctx), `crates/aeth-devkit-setup/tests/fixtures/pyproject.fixture.toml`, `crates/aeth-devkit-setup/tests/apply.rs`
- Modify: `python/aeth_devkit/templates/pyproject.template.toml`

**Interfaces:**
- Produces on `ProjectContext`: `pub name: String` (the `[project].name` as written), `pub version: Option<String>`, `pub origin: Option<String>` (origin URL when git-tracked), `pub docker_services: Vec<String>`, `pub docker_legacy_keys: Vec<String>` (subset of `["chown_paths", "mkdirs"]` present), `has_docker == !docker_services.is_empty()`, `pub fn uses_aeth_ext(&self) -> bool`, `pub fn docker_files_present(&self) -> bool`.

- [ ] **Step 1: Rewrite the detection tests in `context.rs`**

Replace the whole `docker_detection` module with:

```rust
#[cfg(test)]
mod docker_detection {
  use super::*;

  fn project(pyproject: &str, files: &[&str]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
    for f in files {
      let p = dir.path().join(f);
      std::fs::create_dir_all(p.parent().unwrap()).unwrap();
      std::fs::write(p, "").unwrap();
    }
    dir
  }

  #[test]
  fn services_is_the_only_switch() {
    let on = project("[project]\nname = \"p\"\n[tool.docker]\nservices = [\"p\"]\n", &[]);
    let ctx = ProjectContext::discover(on.path()).unwrap();
    assert!(ctx.has_docker);
    assert_eq!(ctx.docker_services, vec!["p"]);
    // Real Docker files without a services list are not a Docker setup any more…
    let files = project("[project]\nname = \"p\"\n", &["docker/Dockerfile", "docker/compose.yaml"]);
    let ctx = ProjectContext::discover(files.path()).unwrap();
    assert!(!ctx.has_docker);
    // …but the advisory needs to know they exist.
    assert!(ctx.docker_files_present());
    let empty = project("[project]\nname = \"p\"\n[tool.docker]\nservices = []\n", &[]);
    let ctx = ProjectContext::discover(empty.path()).unwrap();
    assert!(!ctx.has_docker && !ctx.docker_files_present());
  }

  #[test]
  fn legacy_keys_name_and_version_are_read() {
    let dir = project(
      "[project]\nname = \"Aeth-Ext\"\nversion = \"8.1.0\"\n[tool.docker]\nchown_paths = [\"x\"]\nmkdirs = [\"\"]\n",
      &[],
    );
    let ctx = ProjectContext::discover(dir.path()).unwrap();
    assert_eq!(ctx.docker_legacy_keys, vec!["chown_paths", "mkdirs"]);
    assert_eq!(ctx.name, "Aeth-Ext");
    assert_eq!(ctx.version.as_deref(), Some("8.1.0"));
    assert!(ctx.uses_aeth_ext(), "aeth_ext itself counts");
    assert_eq!(ctx.origin, None, "not a git repo");
  }

  #[test]
  fn a_dependency_on_aeth_ext_counts() {
    let dir = project("[project]\nname = \"p\"\ndependencies = [\"aeth-ext[sftp]>=8\"]\n", &[]);
    assert!(ProjectContext::discover(dir.path()).unwrap().uses_aeth_ext());
    let dir = project("[project]\nname = \"p\"\ndependencies = [\"requests\"]\n", &[]);
    assert!(!ProjectContext::discover(dir.path()).unwrap().uses_aeth_ext());
  }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-setup docker_detection`
Expected: compile errors (fields do not exist).

- [ ] **Step 3: Implement in `context.rs`**

Add the fields to the struct (with doc comments) and change the `has_docker` doc to:

```rust
  /// Whether `[tool.docker].services` lists at least one service — the only Docker
  /// switch. Docker files alone do not count (see `docker_files_present`).
  pub has_docker: bool,
  /// `[project].name` as written (dist name; `package` is the import name).
  pub name: String,
  /// `[project].version`, when present.
  pub version: Option<String>,
  /// The `origin` remote URL when the project is git-tracked and has one.
  pub origin: Option<String>,
  /// `[tool.docker].services`: the compose services setup-project manages.
  pub docker_services: Vec<String>,
  /// Legacy `[tool.docker]` keys still present (`chown_paths`, `mkdirs`); only reported.
  pub docker_legacy_keys: Vec<String>,
```

In `discover`, keep `project_name` as computed and add before the `Ok(Self { … })`:

```rust
    let docker = doc.get("tool").and_then(|t| t.get("docker"));
    // Only string entries count; a malformed list is treated as empty rather than fatal.
    let docker_services: Vec<String> = docker
      .and_then(|d| d.get("services"))
      .and_then(|s| s.as_array())
      .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
      .unwrap_or_default();
    let docker_legacy_keys: Vec<String> = ["chown_paths", "mkdirs"]
      .into_iter()
      .filter(|k| docker.is_some_and(|d| d.get(k).is_some()))
      .map(str::to_string)
      .collect();
    let has_docker = !docker_services.is_empty();
    let version = doc
      .get("project")
      .and_then(|p| p.get("version"))
      .and_then(|v| v.as_str())
      .map(str::to_string);
    // `git remote get-url` outside a repository fails; `ok().flatten()` folds both "not a
    // repo" and "no origin" into `None`.
    let origin = if aeth_devkit_core::git::is_git_tracked(&root) {
      aeth_devkit_core::git::origin_url(&root).ok().flatten()
    } else {
      None
    };
```

Delete `dir_has_docker_content` and `is_compose_file`. Add the methods:

```rust
  /// The project depends on aeth_ext, or *is* aeth_ext: both get its compose conventions
  /// (the ALERTS_* environment).
  pub fn uses_aeth_ext(&self) -> bool {
    self.has_dependency("aeth-ext") || normalize_dist_name(&self.name) == "aeth-ext"
  }

  /// A Dockerfile at the root or under `docker/`, or a compose file anywhere docker-pin
  /// would find one. Used only for the "services is empty but files exist" advisory.
  pub fn docker_files_present(&self) -> bool {
    let dockerfile_in = |dir: &Path| {
      std::fs::read_dir(dir)
        .map(|rd| rd.flatten().any(|e| e.path().is_file() && e.file_name().to_string_lossy().starts_with("Dockerfile")))
        .unwrap_or(false)
    };
    dockerfile_in(&self.root)
      || dockerfile_in(&self.root.join("docker"))
      || aeth_devkit_core::compose::find_compose_file(&self.root).ok().flatten().is_some()
  }
```

Populate `name: project_name.clone()` (compute `package` from it first as today) and the new fields in `Ok(Self { … })`.

- [ ] **Step 4: Update the five test constructors**

In `md_block.rs`, `templates.rs` (two), and `toml_merge.rs` (two) add to each `ProjectContext { … }` literal:

```rust
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec![],
      docker_legacy_keys: vec![],
```

In `toml_merge.rs`'s `fn ctx(has_docker: bool)`, set `docker_services: if has_docker { vec!["proj".into()] } else { vec![] }` alongside `has_docker`.

- [ ] **Step 5: Advisory in `lib.rs` step 14**

Replace the `[tool.docker]` note with:

```rust
  if !ctx.has_docker && ctx.docker_files_present() {
    changes
      .notes
      .push("Docker files found but `[tool.docker].services` is empty; list the app service(s) to manage them.".into());
  }
  if !ctx.docker_legacy_keys.is_empty() {
    let tail = if !ctx.has_docker && !ctx.docker_files_present() {
      " — or delete the whole table if the project has no Docker setup."
    } else {
      ""
    };
    changes.notes.push(format!(
      "pyproject.toml [tool.docker] still has {}: fold `chown_paths` into `required_persisted_dirs`, move any `mkdirs` \
       scratch directories to temp dirs, and delete both keys; the entrypoint no longer reads them{tail}",
      ctx.docker_legacy_keys.join(" and ")
    ));
  }
```

- [ ] **Step 6: Template schema and fixture**

Replace the `[tool.docker]` block in `python/aeth_devkit/templates/pyproject.template.toml` with:

```toml
# setup-project: if-docker
[tool.docker]
  # Compose services setup-project validates against the standard. Empty = no Docker
  # handling at all. Side services (wireguard, ...) are simply not listed.
  services                = []
  # Paths relative to /app the entrypoint guarantees exist, are backed by a bind mount (the
  # path itself or an ancestor) and are owned by nonroot. Sub-paths of a mount are created
  # on the mounted filesystem. Ephemeral scratch dirs do not belong here (or under /app at
  # all): use tempfile paths.
  required_persisted_dirs = ["persisted_data"]
```

In `crates/aeth-devkit-setup/tests/fixtures/pyproject.fixture.toml` replace the `[tool.docker]` table with:

```toml
[tool.docker]
  services    = ["imap-report-collector"]
  chown_paths = [
    "persisted_data",
  ]
  mkdirs      = [""]
```

In `tests/apply.rs::make_project`, delete the line `write(root, "docker/Dockerfile", "FROM scratch\n");` (the fixture's `services` now enables Docker). In `applies_and_is_idempotent` add after the `[tool.docker]` assertion:

```rust
  assert!(py.contains("required_persisted_dirs = [\"persisted_data\"]"), "{py}");
  assert!(changes.notes.iter().any(|n| n.contains("chown_paths and mkdirs")), "{:?}", changes.notes);
```

- [ ] **Step 7: Run**

Run: `cargo test -p aeth-devkit-setup`
Expected: `docker_detection`, `toml_merge`, `templates`, `md_block` pass. `tests/apply.rs` will fail on `.dockerignore`/idempotency only if Task 7 is not yet in (the Docker step does not exist yet — `applies_and_is_idempotent` must still pass because nothing Docker-specific runs; if `.dockerignore` is absent that assertion fails: it must be present since `has_docker` is now true from the fixture). Fix anything else that surfaces before committing.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add -A crates/aeth-devkit-setup python/aeth_devkit/templates/pyproject.template.toml
git commit -m "feat(setup-project): [tool.docker].services is the Docker switch"
```

---

### Task 4: Templates and placeholders

**Files:**
- Create: `python/aeth_devkit/templates/docker/template.Dockerfile`, `python/aeth_devkit/templates/docker/compose.template.yaml`
- Modify: `crates/aeth-devkit-setup/src/templates.rs`

**Interfaces:**
- Produces: `templates::substitute` additionally replaces `{devkit_version}` and `{git_repo}` (never `{git_tag}` or `{service}`); `pub fn git_repo(ctx: &ProjectContext) -> String`; `pub fn gate(text: &str, enabled: &dyn Fn(&str) -> bool) -> String` handling `# setup-project: if-<name>` / `if-no-<name>` / `end`; `gate_publish_index` becomes a thin wrapper.

- [ ] **Step 1: Failing tests** (append to `templates.rs`'s `publish_index_tests` module or a new `docker_placeholder_tests` module)

```rust
#[cfg(test)]
mod docker_placeholder_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(origin: Option<&str>) -> ProjectContext {
    ProjectContext {
      root: std::path::PathBuf::from("/p"),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: true,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      name: "proj".into(),
      version: Some("1.2.3".into()),
      origin: origin.map(str::to_string),
      docker_services: vec!["proj".into()],
      docker_legacy_keys: vec![],
    }
  }

  #[test]
  fn git_repo_is_the_canonical_https_form_for_github_origins() {
    assert_eq!(
      git_repo(&ctx(Some("git@github.com:AetherBreaker/aeth_ext.git"))),
      "https://github.com/AetherBreaker/aeth_ext.git"
    );
    assert_eq!(git_repo(&ctx(Some("https://gitlab.com/o/r"))), "https://gitlab.com/o/r", "non-GitHub kept as-is");
    assert_eq!(git_repo(&ctx(None)), "");
  }

  #[test]
  fn docker_placeholders_substitute_except_the_lazy_ones() {
    let out = substitute(
      "{devkit_version} {git_repo} {git_tag} {service} {python_dir}",
      &ctx(Some("https://github.com/o/r.git")),
      Escape::None,
    );
    assert_eq!(out, format!("{} https://github.com/o/r.git {{git_tag}} {{service}} src", env!("CARGO_PKG_VERSION")));
  }

  #[test]
  fn gate_handles_any_marker_name() {
    let t = "a\n# setup-project: if-aeth-ext\nx\n# setup-project: end\n# setup-project: if-no-aeth-ext\ny\n# setup-project: end\n";
    assert_eq!(gate(t, &|n| n == "aeth-ext"), "a\nx\n");
    assert_eq!(gate(t, &|_| false), "a\ny\n");
  }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-setup docker_placeholder_tests`
Expected: compile errors (`git_repo`, `gate` missing).

- [ ] **Step 3: Implement in `templates.rs`**

Extend `substitute`'s chain (after `{publish_index_key}`):

```rust
    .replace("{devkit_version}", env!("CARGO_PKG_VERSION"))
    .replace("{git_repo}", &esc(&git_repo(ctx)))
```

Update the `load` doc comment to list the two new placeholders and say `{git_tag}` / `{service}` are left for the Docker scaffold. Add:

```rust
/// The value compose files carry in `GIT_REPO`: a GitHub origin normalised to
/// `https://github.com/<owner>/<repo>.git` (owner/repo case kept), any other origin as
/// written, and empty when there is no origin (the Repo rule then skips itself).
pub fn git_repo(ctx: &ProjectContext) -> String {
  let Some(origin) = ctx.origin.as_deref() else { return String::new() };
  match aeth_devkit_core::github::github_repo_path(origin) {
    Some(path) => format!("https://github.com/{path}.git"),
    None => origin.to_string(),
  }
}

/// Block markers for line-based templates: `# setup-project: if-<name>` … `end` survives
/// only when `enabled(name)`, `if-no-<name>` … `end` only when it does not. Marker lines
/// are dropped either way; they may be indented and the lines inside keep their own
/// indentation. Unknown marker lines pass through untouched.
pub fn gate(text: &str, enabled: &dyn Fn(&str) -> bool) -> String {
  let mut out = String::with_capacity(text.len());
  let mut block: Option<bool> = None;
  for line in text.lines() {
    match line.trim().strip_prefix("# setup-project: ") {
      Some("end") => block = None,
      // `if-no-` must be tried first: `if-no-x` also starts with `if-`.
      Some(m) if m.starts_with("if-no-") => block = Some(!enabled(&m["if-no-".len()..])),
      Some(m) if m.starts_with("if-") => block = Some(enabled(&m["if-".len()..])),
      _ => {
        if block.unwrap_or(true) {
          out.push_str(line);
          out.push('\n');
        }
      }
    }
  }
  out
}

pub fn gate_publish_index(text: &str, has_publish_index: bool) -> String {
  gate(text, &|name| name == "publish-index" && has_publish_index)
}
```

Careful: with `gate_publish_index`, `if-no-publish-index` must be kept when `!has_publish_index` — the generic form does that because `enabled("publish-index") == has_publish_index`. The existing `gate_keeps_exactly_one_variant_and_no_markers` test guards this.

- [ ] **Step 4: Write the Dockerfile template**

`python/aeth_devkit/templates/docker/template.Dockerfile` (LF line endings):

```dockerfile
# syntax=docker/dockerfile:1

# ---- Builder stage ----
FROM ghcr.io/astral-sh/uv:python3.14-bookworm-slim AS builder

WORKDIR /app

ARG GIT_TAG
ARG GIT_REPO

# The devkit container binary answers the build-time pyproject questions here and is the
# entrypoint in the final stage. Pinned to the devkit version that rendered this file.
ADD https://github.com/AetherBreaker/aeth-devkit/releases/download/v{devkit_version}/devkit-container-x86_64-unknown-linux-musl /app/devkit-container
RUN chmod +x /app/devkit-container

# Enable bytecode compilation
ENV UV_COMPILE_BYTECODE=1

# Copy from the cache instead of linking since it's a mounted volume
ENV UV_LINK_MODE=copy

# Install git (required for uv to fetch git-based dependencies)
RUN apt-get update && apt-get install -y --no-install-recommends git \
  && rm -rf /var/lib/apt/lists/*

# Clone only the dependency manifest files first so the dep install layer
# can be cached independently of source code changes.
RUN git clone --depth 1 --branch "${GIT_TAG}" "${GIT_REPO}" /tmp/repo \
  && mv /tmp/repo/pyproject.toml /tmp/repo/uv.lock /app/ \
  && { readme_file=$(/app/devkit-container readme) \
  && [ -n "${readme_file}" ] && [ -f "/tmp/repo/${readme_file}" ] \
  && mv "/tmp/repo/${readme_file}" /app/ || true; }

# Install all dependencies (without the project itself) using the frozen lockfile.
# This layer is cached as long as pyproject.toml/uv.lock don't change, even
# when only source code changes between deployments.
RUN --mount=type=cache,target=/root/.cache/uv \
  extras=$(/app/devkit-container app-extra) \
  && uv sync --frozen --no-dev --no-install-project $extras

# Now bring in the source tree and install the project itself as a
# non-editable wheel so the source tree is not required at runtime.
RUN mv /tmp/repo/{python_dir} /app/{python_dir} && rm -rf /tmp/repo

RUN --mount=type=cache,target=/root/.cache/uv \
  extras=$(/app/devkit-container app-extra) \
  && uv sync --frozen --no-dev --no-editable $extras

# ---- Final stage ----
FROM ghcr.io/astral-sh/uv:python3.14-bookworm-slim

# Setup a non-root user. /app stays root-owned: the code is read-only to the app, which
# writes only to its mounted persisted dirs (or temp dirs).
RUN groupadd --system --gid 999 nonroot \
  && useradd --system --gid 999 --uid 999 --create-home nonroot

WORKDIR /app

# Prevents Python from writing pyc files.
ENV PYTHONDONTWRITEBYTECODE=1
# Keeps Python from buffering stdout and stderr to avoid situations where
# the application crashes without emitting any logs due to buffering.
ENV PYTHONUNBUFFERED=1
# Enable Python optimizations (removes assert statements and sets __debug__ to False)
ENV PYTHONOPTIMIZE=1

# Copy the virtual environment from the builder stage
COPY --from=builder /app/.venv /app/.venv

# Copy artifacts needed by the entrypoint
COPY --from=builder /app/pyproject.toml /app/pyproject.toml
COPY --from=builder /app/devkit-container /app/devkit-container

# Place executables in the environment at the front of the path
ENV PATH="/app/.venv/bin:$PATH"

# The entrypoint checks every required_persisted_dir is bind-mounted, chowns them to
# nonroot, drops privileges, and execs the project's run-app-* script.
ENTRYPOINT ["/app/devkit-container", "run"]
```

- [ ] **Step 5: Write the compose template**

`python/aeth_devkit/templates/docker/compose.template.yaml`:

```yaml
services:
# setup-project: service-block
  {service}:
    container_name: {service}
    build:
      context: .
      dockerfile: docker/Dockerfile
      args:
        GIT_REPO: {git_repo}
        GIT_TAG: {git_tag}
    restart: no
    volumes:
      - type: bind
        source: /data/{package}_files
        target: /app/persisted_data
    # setup-project: if-aeth-ext
    environment:
      - ALERTS_EMAIL=info@sweetfiretobacco.com
      - ALERTS_EMAIL_PWD=${ALERTS_EMAIL_PWD:?}
      - ALERTS_RECIPIENTS=["jacob.ogden@sweetfiretobacco.com"]
    # setup-project: end
    networks:
      - coolify
    healthcheck:
      test:
        - CMD-SHELL
        - bash -ec '[ -f /app/persisted_data/logs/heartbeat.txt ] && ts=$$(cat /app/persisted_data/logs/heartbeat.txt 2>/dev/null) && [ -n "$$ts" ] && hb=$$(date -d "$$ts" +%s 2>/dev/null) && now=$$(date +%s) && [ $$((now - hb)) -lt 180 ]'
      interval: 30s
      timeout: 5s
      retries: 3
      start_period: 15s
# setup-project: end-service-block

networks:
  coolify:
    external: true
```

(`{ALERTS_EMAIL_PWD:?}` is not a placeholder `substitute` knows, so it passes through.)

- [ ] **Step 6: Run**

Run: `cargo test -p aeth-devkit-setup templates`
Expected: pass, including the existing gate tests.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/aeth-devkit-setup/src/templates.rs python/aeth_devkit/templates/docker
git commit -m "feat(setup-project): Docker templates and placeholders"
```

---

### Task 5: Consent and static-file replacement

**Files:**
- Create: `crates/aeth-devkit-setup/src/docker/mod.rs`, `crates/aeth-devkit-setup/src/docker/static_files.rs`
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (`pub mod docker;`, `read_optional` → `pub(crate)`), `crates/aeth-devkit-setup/Cargo.toml` (`similar = { workspace = true }`), root `Cargo.toml` (`similar = "3.2.0"` in `[workspace.dependencies]`)

**Interfaces:**
- Produces:
  - `docker::Mode { Ask, ReplaceAll, KeepAll, DryRun }`, `docker::Deps<'a> { runner: &'a dyn Runner, prompt: &'a dyn Prompt, mode: Mode, interactive: bool }`.
  - `docker::Consent::new(prompt, mode, interactive)`, `.replace(&self, question) -> Result<bool>`, `.add(&self, question) -> Result<bool>`, `.kept_silently(&self) -> bool` (a KeepAll decline happened).
  - `static_files::apply(ctx, templates_dir, consent, changes) -> Result<()>`, `static_files::target_name(&str) -> Option<String>`, `static_files::unified_diff(rel, old, new) -> String`, `static_files::normalize_newlines(&str) -> String`.

- [ ] **Step 1: Failing tests**

In `docker/mod.rs`:

```rust
#[cfg(test)]
mod consent_tests {
  use super::*;
  use aeth_devkit_core::prompt::ScriptedPrompt;

  #[test]
  fn replace_all_sticks_for_the_rest_of_the_run() {
    let p = ScriptedPrompt::new(&["replace all"]);
    let c = Consent::new(&p, Mode::Ask, true);
    assert!(c.replace("a?").unwrap());
    assert!(c.replace("b?").unwrap(), "no second question");
    assert_eq!(p.asked.borrow().len(), 1);
  }

  #[test]
  fn anything_but_the_keywords_keeps() {
    let p = ScriptedPrompt::new(&["replace", "y", "", "add", "no"]);
    let c = Consent::new(&p, Mode::Ask, true);
    assert!(c.replace("a?").unwrap());
    assert!(!c.replace("b?").unwrap());
    assert!(!c.replace("c?").unwrap());
    assert!(c.add("d?").unwrap());
    assert!(!c.add("e?").unwrap());
  }

  #[test]
  fn dry_run_and_keep_all_never_ask() {
    let p = ScriptedPrompt::new(&[]);
    let dry = Consent::new(&p, Mode::DryRun, false);
    assert!(dry.replace("a?").unwrap() && dry.add("b?").unwrap());
    let keep = Consent::new(&p, Mode::KeepAll, false);
    assert!(!keep.replace("a?").unwrap() && !keep.add("b?").unwrap());
    assert!(keep.kept_silently());
    // --replace-docker without a terminal: files replaced, services never added.
    let all = Consent::new(&p, Mode::ReplaceAll, false);
    assert!(all.replace("a?").unwrap());
    assert!(!all.add("b?").unwrap());
    assert!(p.asked.borrow().is_empty());
  }
}
```

In `docker/static_files.rs`:

```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn template_names_map_back_to_targets() {
    assert_eq!(target_name("template.Dockerfile").as_deref(), Some("Dockerfile"));
    assert_eq!(target_name("entrypoint.template.sh").as_deref(), Some("entrypoint.sh"));
    assert_eq!(target_name("compose.template.yaml"), None, "the compose file has its own flow");
    assert_eq!(target_name("README.md"), None);
  }

  #[test]
  fn diff_names_both_sides_and_ignores_crlf_only_drift() {
    let d = unified_diff("docker/Dockerfile", "a\nb\n", "a\nc\n");
    assert!(d.contains("--- docker/Dockerfile (project)"), "{d}");
    assert!(d.contains("+++ docker/Dockerfile (devkit template)"), "{d}");
    assert!(d.contains("-b\n+c\n"), "{d}");
    assert_eq!(normalize_newlines("a\r\nb\r\n"), normalize_newlines("a\nb\n"));
  }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-setup docker`
Expected: compile error (module missing).

- [ ] **Step 3: Implement `docker/mod.rs`** (the compose flow is added in Task 7; keep a stub `apply` for now)

```rust
//! Docker standardisation: templated `docker/` files replaced whole and the compose file
//! edited in place — each only with the user's consent, given per file or once for all.

pub mod compose_rules;
pub mod scaffold;
pub mod static_files;

use std::cell::Cell;
use std::path::Path;

use anyhow::Result;

use aeth_devkit_core::process::Runner;
use aeth_devkit_core::prompt::Prompt;

use crate::changes::Changes;
use crate::context::ProjectContext;

/// How consent questions are answered for this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
  /// A terminal is attached: ask per file.
  Ask,
  /// `--replace-docker`, or the user answered `replace all`.
  ReplaceAll,
  /// No terminal and no flag: show diffs, change nothing.
  KeepAll,
  /// `--dry-run` / `--check`: every intended edit is recorded, nothing is asked or written.
  DryRun,
}

/// The injectable collaborators, in the style of the release and pin crates.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub prompt: &'a dyn Prompt,
  pub mode: Mode,
  /// Whether a human can answer the add-service question (stdin is a terminal and this
  /// is not a dry run). `replace all` never adds a service: a typo in pyproject must not
  /// grow the compose file without someone reading the service name.
  pub interactive: bool,
}

/// Consent state for one run. `Cell` because `replace all` upgrades the mode through a
/// shared reference: the same `&Consent` is handed to every step.
pub struct Consent<'a> {
  prompt: &'a dyn Prompt,
  mode: Cell<Mode>,
  interactive: bool,
  declined_silently: Cell<bool>,
}

impl<'a> Consent<'a> {
  pub fn new(prompt: &'a dyn Prompt, mode: Mode, interactive: bool) -> Self {
    Self {
      prompt,
      mode: Cell::new(mode),
      interactive,
      declined_silently: Cell::new(false),
    }
  }

  /// Whether a change whose diff was just printed may be applied.
  pub fn replace(&self, question: &str) -> Result<bool> {
    match self.mode.get() {
      Mode::ReplaceAll | Mode::DryRun => Ok(true),
      Mode::KeepAll => {
        self.declined_silently.set(true);
        Ok(false)
      }
      Mode::Ask => match self.prompt.ask(question)?.as_str() {
        "replace" => Ok(true),
        "replace all" => {
          self.mode.set(Mode::ReplaceAll);
          Ok(true)
        }
        _ => Ok(false),
      },
    }
  }

  /// Whether a listed-but-absent service may be scaffolded into the compose file.
  pub fn add(&self, question: &str) -> Result<bool> {
    if self.mode.get() == Mode::DryRun {
      return Ok(true); // an intended edit, shown like every other
    }
    if !self.interactive {
      return Ok(false);
    }
    Ok(self.prompt.ask(question)? == "add")
  }

  /// A change was kept only because nobody could be asked.
  pub fn kept_silently(&self) -> bool {
    self.declined_silently.get()
  }
}

/// Everything Docker: static files, then the compose file. Filled in by Task 7.
pub fn apply(ctx: &ProjectContext, templates_dir: &Path, deps: &Deps, changes: &mut Changes) -> Result<()> {
  let consent = Consent::new(deps.prompt, deps.mode, deps.interactive);
  static_files::apply(ctx, templates_dir, &consent, changes)?;
  Ok(())
}
```

Create empty placeholder files `docker/compose_rules.rs` and `docker/scaffold.rs` containing only a `//!` doc line each so the crate compiles (Tasks 6 and 7 fill them).

- [ ] **Step 4: Implement `docker/static_files.rs`**

```rust
//! Whole-file replacement of the templated `docker/` files (everything except the compose
//! file), shown as a diff and applied only on consent. Files the template stopped
//! shipping are reported, never deleted.

use std::path::Path;

use anyhow::{Context as _, Result};
use similar::TextDiff;

use crate::changes::Changes;
use crate::context::ProjectContext;
use crate::docker::Consent;
use crate::templates;

/// Target file name for a template file name, or `None` for files this step does not own:
/// `template.Dockerfile` → `Dockerfile`, `entrypoint.template.sh` → `entrypoint.sh`;
/// the compose file (`compose.template.yaml`) has its own rule-based flow.
pub fn target_name(template_file: &str) -> Option<String> {
  if template_file == "compose.template.yaml" {
    return None;
  }
  if let Some(rest) = template_file.strip_prefix("template.") {
    return Some(rest.to_string());
  }
  // `x.template.ext` → `x.ext`: split at the *last* `.template.` so a stem containing
  // the word keeps it.
  let (stem, ext) = template_file.rsplit_once(".template.")?;
  Some(format!("{stem}.{ext}"))
}

pub fn normalize_newlines(s: &str) -> String {
  s.replace("\r\n", "\n")
}

/// Three lines of context, both sides labelled so the user can tell which is theirs.
pub fn unified_diff(rel: &str, old: &str, new: &str) -> String {
  TextDiff::from_lines(old, new)
    .unified_diff()
    .context_radius(3)
    .header(&format!("{rel} (project)"), &format!("{rel} (devkit template)"))
    .to_string()
}

pub fn apply(ctx: &ProjectContext, templates_dir: &Path, consent: &Consent, changes: &mut Changes) -> Result<()> {
  let dir = templates_dir.join("docker");
  let mut targets: Vec<String> = std::fs::read_dir(&dir)
    .with_context(|| format!("reading {}", dir.display()))?
    .filter_map(|e| e.ok())
    .filter(|e| e.path().is_file())
    .filter_map(|e| target_name(&e.file_name().to_string_lossy()))
    .collect();
  targets.sort(); // deterministic prompt order
  for target in targets {
    let rel = format!("docker/{target}");
    let rendered = templates::load(templates_dir, &rel, ctx, templates::Escape::None)?;
    let path = ctx.root.join("docker").join(&target);
    let Some(original) = crate::read_optional(&path)? else {
      changes.record_optional(&path, None, &rendered, vec!["created from template".into()])?;
      continue;
    };
    if normalize_newlines(&original) == normalize_newlines(&rendered) {
      // Managed, unchanged. CRLF-only drift is not drift: .gitattributes owns line endings.
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      continue;
    }
    println!("{}", unified_diff(&rel, &original, &rendered));
    if consent.replace(&format!("Replace {rel}? [replace / replace all / anything else keeps it]:"))? {
      changes.record_optional(&path, Some(&original), &rendered, vec!["replaced with the devkit template".into()])?;
    } else {
      changes.record_optional(&path, Some(&original), &original, vec![])?;
      println!("Kept {rel}.");
    }
  }
  // Stray leftovers of the shell-and-Python entrypoint. Reported once, never removed.
  let stray: Vec<&str> = [("docker/entrypoint.sh", false), ("docker/scripts", true)]
    .iter()
    .filter(|(rel, is_dir)| {
      let p = ctx.root.join(rel);
      if *is_dir { p.is_dir() } else { p.is_file() }
    })
    .map(|(rel, _)| *rel)
    .collect();
  if !stray.is_empty() {
    changes.notes.push(format!(
      "{} no longer used: the devkit-container binary replaced the shell entrypoint and its helper scripts; safe to delete.",
      stray.join(" and ")
    ));
  }
  Ok(())
}
```

Wire the crate: `pub mod docker;` in `lib.rs`; change `fn read_optional` to `pub(crate) fn read_optional`; add `similar` to the workspace and setup `Cargo.toml`.

- [ ] **Step 5: Run**

Run: `cargo test -p aeth-devkit-setup docker`
Expected: consent and static-file unit tests pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add Cargo.toml Cargo.lock crates/aeth-devkit-setup
git commit -m "feat(setup-project): consent prompts and whole-file Docker replacement"
```

---

### Task 6: Compose scaffold, lazy `{git_tag}`, and rules

**Files:**
- Modify: `crates/aeth-devkit-setup/src/docker/scaffold.rs`, `crates/aeth-devkit-setup/src/docker/compose_rules.rs`

**Interfaces:**
- Consumes: `aeth_devkit_core::compose::tree::*` (Task 2), `templates::{load, gate}` (Task 4), `ctx.uses_aeth_ext()`, `ctx.version`, `ctx.origin`, `github::{list_tags, github_repo_path, normalize_repo}`, `version::{latest_stable_common, parse_lenient}`.
- Produces:
  - `scaffold::Scaffold { head: String, block: String, tail: String }`, `scaffold::parse(gated_template: &str) -> Result<Scaffold>`, `scaffold::service_block(&Scaffold, service: &str) -> String`, `scaffold::render_file(&Scaffold, services: &[String]) -> String`, `scaffold::load(templates_dir, ctx) -> Result<Scaffold>` (load + gate + parse).
  - `scaffold::GitTag::new(runner, ctx)`, `.fill(&self, text: &str) -> String` (resolves only if `{git_tag}` occurs), `.note(&self) -> Option<String>`.
  - `compose_rules::Outcome { edits: Vec<Edit>, details: Vec<String> }`, `compose_rules::service_edits(lines, svc: &Node, sc_lines, sc_svc: &Node, name: &str) -> Outcome`, `compose_rules::top_level_edits(lines, sc_tail_lines) -> Outcome`.

- [ ] **Step 1: Failing scaffold tests** (in `scaffold.rs`)

```rust
#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;
  use std::collections::HashSet;

  const TPL: &str = "services:\n# setup-project: service-block\n  {service}:\n    container_name: {service}\n    build:\n      args:\n        GIT_TAG: {git_tag}\n# setup-project: end-service-block\n\nnetworks:\n  coolify:\n    external: true\n";

  #[test]
  fn splits_and_renders_one_block_per_service() {
    let sc = parse(TPL).unwrap();
    assert_eq!(sc.head, "services:\n");
    assert!(sc.block.starts_with("  {service}:\n"));
    assert_eq!(sc.tail, "\nnetworks:\n  coolify:\n    external: true\n");
    let out = render_file(&sc, &["a".into(), "b".into()]);
    assert_eq!(
      out,
      "services:\n  a:\n    container_name: a\n    build:\n      args:\n        GIT_TAG: {git_tag}\n  b:\n    container_name: b\n    build:\n      args:\n        GIT_TAG: {git_tag}\n\nnetworks:\n  coolify:\n    external: true\n"
    );
    assert!(parse("services:\n").is_err(), "missing markers is a template bug");
  }

  fn ctx(root: &std::path::Path, origin: Option<&str>) -> crate::context::ProjectContext {
    crate::context::ProjectContext {
      root: root.to_path_buf(),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: true,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      name: "proj".into(),
      version: Some("1.2.3".into()),
      origin: origin.map(str::to_string),
      docker_services: vec!["proj".into()],
      docker_legacy_keys: vec![],
    }
  }

  #[test]
  fn git_tag_is_the_latest_stable_remote_tag_with_its_spelling() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v2.0.0-alpha1\nv1.5.0\nv1.4.0\n");
    let c = ctx(std::path::Path::new("."), Some("https://github.com/o/r.git"));
    let tag = GitTag::new(&r, &c);
    assert_eq!(tag.fill("x: {git_tag}"), "x: v1.5.0");
    assert!(tag.note().is_none());
    assert_eq!(r.calls_for("gh").len(), 1, "resolved once");
    assert_eq!(tag.fill("no placeholder"), "no placeholder");
  }

  #[test]
  fn git_tag_is_lazy_and_falls_back_to_the_pyproject_version() {
    let r = RecordingRunner::new(1); // any gh call fails
    let c = ctx(std::path::Path::new("."), Some("https://github.com/o/r.git"));
    let tag = GitTag::new(&r, &c);
    assert!(r.calls_for("gh").is_empty(), "nothing resolved until needed");
    assert_eq!(tag.fill("{git_tag}"), "v1.2.3");
    assert!(tag.note().unwrap().contains("v1.2.3"));
    // Not a GitHub origin: no gh call at all.
    let r2 = RecordingRunner::new(0);
    let c2 = ctx(std::path::Path::new("."), None);
    assert_eq!(GitTag::new(&r2, &c2).fill("{git_tag}"), "v1.2.3");
    assert!(r2.calls_for("gh").is_empty());
  }
}
```

- [ ] **Step 2: Implement `scaffold.rs`**

```rust
//! The compose scaffold: the fresh-file template split into head / one service block /
//! tail, plus the lazily resolved `{git_tag}` placeholder.

use std::cell::{OnceCell, RefCell};
use std::path::Path;

use anyhow::{Result, bail};

use aeth_devkit_core::github;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::version::{latest_stable_common, parse_lenient};

use crate::context::ProjectContext;
use crate::templates;

pub const BLOCK_START: &str = "# setup-project: service-block";
pub const BLOCK_END: &str = "# setup-project: end-service-block";

/// `head` + one `block` per service + `tail` is a complete compose file. The block still
/// carries `{service}` and `{git_tag}`; every other placeholder was substituted on load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scaffold {
  pub head: String,
  pub block: String,
  pub tail: String,
}

/// Split an already substituted and gated template at its block markers.
pub fn parse(template: &str) -> Result<Scaffold> {
  let mut head = String::new();
  let mut block = String::new();
  let mut tail = String::new();
  // A tiny state machine over the marker lines; `Option<bool>` would read as "inside?"
  // but three destinations want three states, so an enum is clearer.
  enum Part {
    Head,
    Block,
    Tail,
  }
  let mut part = Part::Head;
  for line in template.lines() {
    match (line.trim(), &part) {
      (BLOCK_START, Part::Head) => part = Part::Block,
      (BLOCK_END, Part::Block) => part = Part::Tail,
      _ => {
        let dest = match part {
          Part::Head => &mut head,
          Part::Block => &mut block,
          Part::Tail => &mut tail,
        };
        dest.push_str(line);
        dest.push('\n');
      }
    }
  }
  if !matches!(part, Part::Tail) {
    bail!("compose template is missing the `{BLOCK_START}` / `{BLOCK_END}` markers");
  }
  Ok(Scaffold { head, block, tail })
}

/// Load, substitute, gate on aeth_ext, and split the compose template.
pub fn load(templates_dir: &Path, ctx: &ProjectContext) -> Result<Scaffold> {
  let raw = templates::load(templates_dir, "docker/compose.yaml", ctx, templates::Escape::None)?;
  let uses = ctx.uses_aeth_ext();
  parse(&templates::gate(&raw, &|name| name == "aeth-ext" && uses))
}

pub fn service_block(sc: &Scaffold, service: &str) -> String {
  sc.block.replace("{service}", service)
}

pub fn render_file(sc: &Scaffold, services: &[String]) -> String {
  let mut out = sc.head.clone();
  for s in services {
    out.push_str(&service_block(sc, s));
  }
  out.push_str(&sc.tail);
  out
}

/// `{git_tag}`, resolved at most once and only when something containing it is written.
/// A routine run never talks to `gh`.
pub struct GitTag<'a> {
  runner: &'a dyn Runner,
  ctx: &'a ProjectContext,
  // `OnceCell`: write-once through `&self`; `get_or_init` runs the closure the first time.
  resolved: OnceCell<String>,
  note: RefCell<Option<String>>,
}

impl<'a> GitTag<'a> {
  pub fn new(runner: &'a dyn Runner, ctx: &'a ProjectContext) -> Self {
    Self {
      runner,
      ctx,
      resolved: OnceCell::new(),
      note: RefCell::new(None),
    }
  }

  pub fn fill(&self, text: &str) -> String {
    if !text.contains("{git_tag}") {
      return text.to_string();
    }
    text.replace("{git_tag}", self.resolved.get_or_init(|| self.resolve()))
  }

  /// The advisory to print when the fallback was used, if it was.
  pub fn note(&self) -> Option<String> {
    self.note.borrow().clone()
  }

  fn resolve(&self) -> String {
    let fallback = format!("v{}", self.ctx.version.as_deref().unwrap_or("0.0.0"));
    // A closure so the three failure paths share one note; it only reads through the
    // `RefCell`, so plain `Fn` (no `mut`) is enough.
    let fall_back = |why: String| {
      *self.note.borrow_mut() = Some(format!(
        "GIT_TAG set to {fallback} from pyproject.toml ({why}); run `devkit docker-pin` after the next release."
      ));
      fallback.clone()
    };
    let Some(repo) = self.ctx.origin.as_deref().and_then(github::github_repo_path) else {
      return fall_back("origin is not a GitHub repository".into());
    };
    let tags = match github::list_tags(self.runner, &self.ctx.root, &repo) {
      Ok(t) => t,
      Err(e) => return fall_back(format!("{e:#}")),
    };
    // docker-pin's rule with a single source: highest stable version, written with the
    // remote's own spelling (`v1.5.0`, not the normalised `1.5.0`).
    let Some(latest) = latest_stable_common(&[tags.clone()]) else {
      return fall_back("no stable tag on the remote".into());
    };
    tags
      .iter()
      .find(|t| parse_lenient(t).as_ref() == Some(&latest))
      .cloned()
      .unwrap_or_else(|| fall_back("no stable tag on the remote".into()))
  }
}
```

- [ ] **Step 3: Run scaffold tests**

Run: `cargo test -p aeth-devkit-setup scaffold`
Expected: pass.

- [ ] **Step 4: Failing rule tests** (in `compose_rules.rs`)

```rust
#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::compose::tree::{apply_edits, child, split_lines, top_level};

  /// The rendered scaffold block for service `app` (what Task 4's template produces for an
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
    assert!(out.contains("    build:\n      context: .\n      dockerfile: docker/Dockerfile\n      args:\n"), "{out}");
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
```

- [ ] **Step 5: Implement `compose_rules.rs`**

```rust
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
  tree::re_indent(&sc_lines[sc_node.line..sc_node.end], sc_node.indent, tree::child_indent(lines, parent))
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
    let Some(std) = tree::descend(sc_lines, sc_svc, path) else { continue };
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
        let present = |key: &str| {
          items.iter().any(|it| it.text.starts_with(&format!("{key}=")))
            || tree::child(lines, &node, key).is_some()
        };
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
          out.edits.push(Edit::Insert { at: node.end, lines: new_lines });
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
  let Some(sc_networks) = tree::top_level(sc_tail, "networks") else { return out };
  let Some(networks) = tree::top_level(lines, "networks") else {
    // Append at the end, separated from whatever came before by one blank line.
    let mut new: Vec<String> = Vec::new();
    if lines.last().is_some_and(|l| !l.trim().is_empty()) {
      new.push(String::new());
    }
    new.extend(sc_tail[sc_networks.line..sc_networks.end].iter().cloned());
    out.edits.push(Edit::Insert { at: lines.len(), lines: new });
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
```

Why the second test's expected output looks the way it does: every missing key is inserted at its parent's `end`, so `dockerfile` lands after `args` inside `build`, and `GIT_TAG` (inserted at the same line index but deeper) sits above it by `apply_edits`'s indent rule; `restart` and `networks` are appended after `healthcheck`, which already existed; the top-level `networks` map goes last with one blank line before it.

- [ ] **Step 6: Run**

Run: `cargo test -p aeth-devkit-setup compose_rules`
Expected: pass. A failure that differs only in the placement of an inserted block means the tree module's ordering rule was not followed — fix the implementation, not the test.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add crates/aeth-devkit-setup/src/docker
git commit -m "feat(setup-project): compose scaffold, lazy GIT_TAG, and rule engine"
```

---

### Task 7: Wire the Docker step into setup-project

**Files:**
- Modify: `crates/aeth-devkit-setup/src/docker/mod.rs` (compose flow), `crates/aeth-devkit-setup/src/lib.rs`, `crates/aeth-devkit-setup/src/cli.rs`, `crates/aeth-devkit-setup/src/git.rs`
- Create: `crates/aeth-devkit-setup/tests/docker.rs`, `crates/aeth-devkit-setup/tests/fixtures/docker/compose-aeth-ext.yaml`, `crates/aeth-devkit-setup/tests/fixtures/docker/compose-imap.yaml`
- Modify: `crates/aeth-devkit-setup/tests/apply.rs` (`commits_only_changed_trackable_files_in_a_git_repo`)

**Interfaces:**
- Produces: `aeth_devkit_setup::run_with(root, templates_dir, dry_run, deps: &docker::Deps) -> Result<Changes>`; `run(root, templates_dir, dry_run)` = `run_with` with `SystemRunner`, `StdinPrompt`, `Mode::DryRun` when `dry_run` else `Mode::KeepAll`, `interactive: false`. `git::committable(root) -> Vec<String>` replaces `COMMITTABLE`. `cli::Args.replace_docker: bool`.

- [ ] **Step 1: Fixtures**

`tests/fixtures/docker/compose-aeth-ext.yaml` — copy `d:/SFT Software Projects/aeth_ext/docker/compose.yaml` verbatim (the `central-log-server` file shown in the survey; keep its `labels`, `expose`, `restart: on-failure:3`, map-form networks with aliases).

`tests/fixtures/docker/compose-imap.yaml` — copy `d:/SFT Software Projects/IMAPReportCollector/docker/compose.yaml` verbatim (comments, `WATCH_*` env, commented `expose`).

- [ ] **Step 2: Failing e2e tests** — `tests/docker.rs`

```rust
//! End-to-end: the Docker step against fixture projects, with scripted consent.

use std::fs;
use std::path::{Path, PathBuf};

use aeth_devkit_core::process::RecordingRunner;
use aeth_devkit_core::prompt::ScriptedPrompt;
use aeth_devkit_setup::docker::{Deps, Mode};

fn fixtures() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures").join("docker")
}

fn templates() -> PathBuf {
  Path::new(env!("CARGO_MANIFEST_DIR")).join("../../python/aeth_devkit/templates")
}

fn write(root: &Path, rel: &str, content: &str) {
  let p = root.join(rel);
  fs::create_dir_all(p.parent().unwrap()).unwrap();
  fs::write(p, content).unwrap();
}

fn read(root: &Path, rel: &str) -> String {
  fs::read_to_string(root.join(rel)).unwrap()
}

/// A git-tracked project with an origin, `services`, and an aeth-ext dependency.
fn project(services: &[&str], origin: &str) -> tempfile::TempDir {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  let list = services.iter().map(|s| format!("\"{s}\"")).collect::<Vec<_>>().join(", ");
  write(
    root,
    "pyproject.toml",
    &format!(
      "[project]\n  name = \"demo-app\"\n  version = \"1.2.3\"\n  dependencies = [\"aeth-ext>=8\"]\n\n[tool.docker]\n  services = [{list}]\n"
    ),
  );
  write(root, "src/demo_app/__init__.py", "");
  aeth_devkit_core::git::init_test_repo(root);
  let git = |args: &[&str]| {
    let out = std::process::Command::new("git").current_dir(root).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
  };
  git(&["remote", "add", "origin", origin]);
  dir
}

fn run(root: &Path, mode: Mode, interactive: bool, answers: &[&str], dry_run: bool) -> (aeth_devkit_setup::changes::Changes, ScriptedPrompt, RecordingRunner) {
  let prompt = ScriptedPrompt::new(answers);
  let runner = RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\nv1.0.0\n");
  let changes = {
    let deps = Deps { runner: &runner, prompt: &prompt, mode, interactive };
    aeth_devkit_setup::run_with(root, &templates(), dry_run, &deps).unwrap()
  };
  (changes, prompt, runner)
}

#[test]
fn fresh_project_gets_dockerfile_and_compose_then_is_idempotent() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  let (changes, prompt, runner) = run(root, Mode::Ask, true, &[], false);
  assert!(prompt.asked.borrow().is_empty(), "creation never prompts");
  let df = read(root, "docker/Dockerfile");
  assert!(df.contains(&format!("/v{}/devkit-container-x86_64-unknown-linux-musl", env!("CARGO_PKG_VERSION"))), "{df}");
  assert!(df.contains("mv /tmp/repo/src /app/src"), "{df}");
  assert!(!df.contains("gosu"), "{df}");
  let compose = read(root, "docker/compose.yaml");
  assert!(compose.contains("  demo-app:\n    container_name: demo-app\n"), "{compose}");
  assert!(compose.contains("GIT_REPO: https://github.com/O/Demo.git"), "{compose}");
  assert!(compose.contains("GIT_TAG: v1.1.0"), "latest stable tag, remote spelling: {compose}");
  assert!(compose.contains("source: /data/demo_app_files"), "{compose}");
  assert!(compose.contains("ALERTS_EMAIL=info@sweetfiretobacco.com"), "aeth-ext dependency: {compose}");
  assert!(compose.ends_with("networks:\n  coolify:\n    external: true\n"), "{compose}");
  assert_eq!(runner.calls_for("gh").len(), 1);
  assert!(changes.files.iter().any(|f| f.path.ends_with("compose.yaml") && f.created));

  let (again, _, runner) = run(root, Mode::Ask, true, &[], false);
  assert!(again.is_empty(), "{}", again.report(root));
  assert!(runner.calls_for("gh").is_empty(), "a routine run never resolves the tag");
}

#[test]
fn without_aeth_ext_the_alerts_block_is_absent() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(root, "pyproject.toml", &read(root, "pyproject.toml").replace("\"aeth-ext>=8\"", ""));
  run(root, Mode::Ask, true, &[], false);
  assert!(!read(root, "docker/compose.yaml").contains("ALERTS_EMAIL"));
}

#[test]
fn dockerfile_drift_is_replaced_only_with_consent() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, true, &[], false);
  let good = read(root, "docker/Dockerfile");
  write(root, "docker/Dockerfile", &good.replace("PYTHONOPTIMIZE=1", "PYTHONOPTIMIZE=2"));

  let (changes, prompt, _) = run(root, Mode::Ask, true, &[""], false);
  assert_eq!(prompt.asked.borrow()[0], "Replace docker/Dockerfile? [replace / replace all / anything else keeps it]:");
  assert!(changes.is_empty(), "kept: {}", changes.report(root));
  assert!(read(root, "docker/Dockerfile").contains("PYTHONOPTIMIZE=2"));

  let (changes, _, _) = run(root, Mode::Ask, true, &["replace"], false);
  assert!(changes.files.iter().any(|f| f.path.ends_with("Dockerfile")));
  assert_eq!(read(root, "docker/Dockerfile"), good);

  // CRLF-only drift is not drift.
  write(root, "docker/Dockerfile", &good.replace('\n', "\r\n"));
  let (changes, prompt, _) = run(root, Mode::Ask, true, &[], false);
  assert!(changes.is_empty() && prompt.asked.borrow().is_empty());
}

#[test]
fn replace_all_covers_the_compose_edits_too() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, true, &[], false);
  write(root, "docker/Dockerfile", "FROM scratch\n");
  write(root, "docker/compose.yaml", &read(root, "docker/compose.yaml").replace("interval: 30s", "interval: 99s"));
  let (changes, prompt, _) = run(root, Mode::Ask, true, &["replace all"], false);
  assert_eq!(prompt.asked.borrow().len(), 1);
  assert_eq!(changes.files.len(), 2, "{}", changes.report(root));
  assert!(read(root, "docker/compose.yaml").contains("interval: 30s"));
}

#[test]
fn non_interactive_keeps_everything_and_says_so() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, true, &[], false);
  write(root, "docker/Dockerfile", "FROM scratch\n");
  let (changes, _, _) = run(root, Mode::KeepAll, false, &[], false);
  assert!(changes.is_empty());
  assert!(changes.notes.iter().any(|n| n.contains("--replace-docker")), "{:?}", changes.notes);
  assert_eq!(read(root, "docker/Dockerfile"), "FROM scratch\n");
  // --replace-docker without a terminal applies files.
  let (changes, _, _) = run(root, Mode::ReplaceAll, false, &[], false);
  assert!(!changes.is_empty());
}

#[test]
fn dry_run_records_docker_drift_without_writing_or_asking() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, true, &[], false);
  write(root, "docker/Dockerfile", "FROM scratch\n");
  let (changes, prompt, _) = run(root, Mode::DryRun, false, &[], true);
  assert!(prompt.asked.borrow().is_empty());
  assert!(changes.files.iter().any(|f| f.path.ends_with("Dockerfile")), "--check must see Docker drift");
  assert_eq!(read(root, "docker/Dockerfile"), "FROM scratch\n");
}

#[test]
fn imap_fixture_with_injected_drift_gets_exactly_the_standard_edits() {
  let dir = project(&["imap-report-collector"], "https://github.com/AetherBreaker/IMAPReportCollector.git");
  let root = dir.path();
  let original = fs::read_to_string(fixtures().join("compose-imap.yaml")).unwrap();
  let drifted = original
    .replace("      dockerfile: docker/Dockerfile\n", "      dockerfile: Dockerfile\n")
    .replace("      interval: 30s\n", "")
    .replace("      - ALERTS_EMAIL=info@sweetfiretobacco.com\n", "");
  write(root, "docker/compose.yaml", &drifted);
  let (changes, _, _) = run(root, Mode::ReplaceAll, true, &[], false);
  let out = read(root, "docker/compose.yaml");
  assert!(out.contains("dockerfile: docker/Dockerfile"), "{out}");
  assert!(out.contains("interval: 30s"), "{out}");
  assert!(out.contains("- ALERTS_EMAIL=info@sweetfiretobacco.com"), "{out}");
  // Everything the standard does not name survives byte for byte.
  for kept in ["# Email to Watch", "WATCH_POLLING_TIMEOUT_SEC=600", "    # expose:\n", "restart: no\n", "GIT_TAG: v3.0.4"] {
    assert!(out.contains(kept), "{kept} lost: {out}");
  }
  let details: Vec<&str> = changes.files.iter().flat_map(|f| f.details.iter().map(String::as_str)).collect();
  assert_eq!(details.len(), 3, "{details:?}");
  let (again, _, _) = run(root, Mode::ReplaceAll, true, &[], false);
  assert!(again.is_empty(), "{}", again.report(root));
}

#[test]
fn aeth_ext_fixture_is_already_compliant() {
  let dir = project(&["central-log-server"], "git@github.com:AetherBreaker/aeth_ext.git");
  let root = dir.path();
  write(root, "pyproject.toml", &read(root, "pyproject.toml").replace("demo-app", "aeth-ext"));
  write(root, "docker/compose.yaml", &fs::read_to_string(fixtures().join("compose-aeth-ext.yaml")).unwrap());
  let (changes, _, _) = run(root, Mode::ReplaceAll, true, &[], false);
  let compose_changed = changes.files.iter().any(|f| f.path.ends_with("compose.yaml"));
  assert!(!compose_changed, "{}", changes.report(root));
}

#[test]
fn a_missing_service_is_added_only_on_add_and_sidecars_are_untouched() {
  let dir = project(&["demo-app", "worker"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(
    root,
    "docker/compose.yaml",
    "services:\n  wireguard:\n    image: wg\n  demo-app:\n    container_name: demo-app\n",
  );
  let (_, prompt, _) = run(root, Mode::Ask, true, &["", "replace"], false);
  assert_eq!(
    prompt.asked.borrow()[0],
    "Service \"worker\" is not in docker/compose.yaml (found: wireguard, demo-app). Add it? [add / anything else skips]:"
  );
  let out = read(root, "docker/compose.yaml");
  assert!(!out.contains("  worker:"), "skipped: {out}");
  assert!(out.contains("  wireguard:\n    image: wg\n"), "sidecar untouched: {out}");
  assert!(out.contains("  demo-app:\n    container_name: demo-app\n    build:\n"), "{out}");

  let (_, prompt, _) = run(root, Mode::Ask, true, &["add", "replace"], false);
  assert_eq!(prompt.asked.borrow()[1], "Apply these edits to docker/compose.yaml? [replace / replace all / anything else keeps it]:");
  let out = read(root, "docker/compose.yaml");
  assert!(out.contains("\n  worker:\n    container_name: worker\n"), "{out}");
  assert!(out.contains("GIT_TAG: v1.1.0"), "{out}");
}

#[test]
fn stray_entrypoint_files_are_reported_not_deleted() {
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(root, "docker/entrypoint.sh", "#!/bin/sh\n");
  write(root, "docker/scripts/get_readme.py", "");
  let (changes, _, _) = run(root, Mode::Ask, true, &[], false);
  assert!(changes.notes.iter().any(|n| n.contains("docker/entrypoint.sh and docker/scripts")), "{:?}", changes.notes);
  assert!(root.join("docker/entrypoint.sh").is_file());
}
```

- [ ] **Step 3: Extend the git commit test in `tests/apply.rs`**

In `commits_only_changed_trackable_files_in_a_git_repo`, after the `release.yml` assertion add:

```rust
  assert!(committed.contains("docker/Dockerfile"), "{committed}");
  assert!(committed.contains("docker/compose.yaml"), "{committed}");
```

(The fixture has `services`; the test project is not on GitHub so `{git_tag}` falls back with a note — fine.)

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p aeth-devkit-setup --test docker`
Expected: compile errors (`run_with`, `Deps` import).

- [ ] **Step 5: Implement the compose flow in `docker/mod.rs`**

Replace the stub `apply` with:

```rust
/// Everything Docker: static files first, then the compose file, then advisories.
pub fn apply(ctx: &ProjectContext, templates_dir: &Path, deps: &Deps, changes: &mut Changes) -> Result<()> {
  let consent = Consent::new(deps.prompt, deps.mode, deps.interactive);
  static_files::apply(ctx, templates_dir, &consent, changes)?;
  compose(ctx, templates_dir, deps.runner, &consent, changes)?;
  if consent.kept_silently() {
    changes
      .notes
      .push("Docker files were left alone because no terminal was available to confirm; pass --replace-docker to apply them.".into());
  }
  Ok(())
}

fn compose(ctx: &ProjectContext, templates_dir: &Path, runner: &dyn Runner, consent: &Consent, changes: &mut Changes) -> Result<()> {
  use aeth_devkit_core::compose::{find_compose_file, tree};
  use aeth_devkit_core::compose::tree::Edit;

  let sc = scaffold::load(templates_dir, ctx)?;
  let tag = scaffold::GitTag::new(runner, ctx);
  let Some(path) = find_compose_file(&ctx.root)? else {
    let text = tag.fill(&scaffold::render_file(&sc, &ctx.docker_services));
    changes.record_optional(&ctx.root.join("docker").join("compose.yaml"), None, &text, vec!["created from template".into()])?;
    changes.notes.extend(tag.note());
    return Ok(());
  };
  let rel = path
    .strip_prefix(&ctx.root)
    .map(|p| p.to_string_lossy().replace('\\', "/"))
    .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
  let lines = tree::split_lines(&text);
  let Some(services) = tree::top_level(&lines, "services") else {
    bail!("{rel} has no top-level `services:` key");
  };
  let present = tree::children(&lines, &services);
  let mut edits: Vec<Edit> = Vec::new();
  let mut details: Vec<String> = Vec::new();
  for name in &ctx.docker_services {
    // The scaffold block for *this* service, parsed as its own little document so the
    // rule engine can look keys up in it exactly like in the project file.
    let sc_doc = tree::split_lines(&format!("services:\n{}", scaffold::service_block(&sc, name)));
    let sc_services = tree::top_level(&sc_doc, "services").expect("scaffold starts with services:");
    let sc_svc = tree::child(&sc_doc, &sc_services, name).expect("scaffold block names the service");
    match present.iter().find(|n| &n.key == name) {
      Some(svc) => {
        let o = compose_rules::service_edits(&lines, svc, &sc_doc, &sc_svc, name);
        edits.extend(o.edits);
        details.extend(o.details);
      }
      None => {
        let found = present.iter().map(|n| n.key.as_str()).collect::<Vec<_>>().join(", ");
        let q = format!("Service \"{name}\" is not in {rel} (found: {found}). Add it? [add / anything else skips]:");
        if consent.add(&q)? {
          let indent = tree::child_indent(&lines, &services);
          let mut block = tree::re_indent(&sc_doc[sc_svc.line..sc_svc.end], sc_svc.indent, indent);
          // One blank line between service blocks, matching the sister files.
          if services.end > 0 && !lines[services.end - 1].trim().is_empty() {
            block.insert(0, String::new());
          }
          edits.push(Edit::Insert { at: services.end, lines: block });
          details.push(format!("added service {name}"));
        } else {
          println!("Skipped service \"{name}\".");
        }
      }
    }
  }
  let o = compose_rules::top_level_edits(&lines, &tree::split_lines(&sc.tail));
  edits.extend(o.edits);
  details.extend(o.details);
  if edits.is_empty() {
    changes.record_optional(&path, Some(&text), &text, vec![])?;
    return Ok(());
  }
  let new_text = tag.fill(&tree::apply_edits(&text, &edits));
  println!("{}", static_files::unified_diff(&rel, &text, &new_text));
  if consent.replace(&format!("Apply these edits to {rel}? [replace / replace all / anything else keeps it]:"))? {
    changes.record_optional(&path, Some(&text), &new_text, details)?;
    changes.notes.extend(tag.note());
  } else {
    changes.record_optional(&path, Some(&text), &text, vec![])?;
    println!("Kept {rel}.");
  }
  Ok(())
}
```

Add `use anyhow::{Context as _, bail};` to the module's imports.

- [ ] **Step 6: `lib.rs` wiring**

Change the signature and add the wrapper:

```rust
/// Apply every template to the project at `root` with real collaborators and no
/// terminal: Docker drift is shown but kept (or merely recorded on a dry run).
pub fn run(root: &Path, templates_dir: &Path, dry_run: bool) -> Result<Changes> {
  let deps = docker::Deps {
    runner: &aeth_devkit_core::process::SystemRunner,
    prompt: &aeth_devkit_core::prompt::StdinPrompt,
    mode: if dry_run { docker::Mode::DryRun } else { docker::Mode::KeepAll },
    interactive: false,
  };
  run_with(root, templates_dir, dry_run, &deps)
}

/// [`run`] with injectable Docker collaborators (prompt, `gh` runner, consent mode).
pub fn run_with(root: &Path, templates_dir: &Path, dry_run: bool, deps: &docker::Deps) -> Result<Changes> {
```

Insert step 8b right after the `.dockerignore` block:

```rust
  // 8b. Docker: templated docker/ files replaced whole and the compose file edited in
  //     place, each behind consent (see `docker`).
  if ctx.has_docker {
    docker::apply(&ctx, templates_dir, deps, &mut changes)?;
  }
```

- [ ] **Step 7: `cli.rs`**

Add the flag and build the deps:

```rust
  /// Answer `replace all` to every Docker prompt up front (also applies Docker files when
  /// stdin is not a terminal).
  #[arg(long)]
  pub replace_docker: bool,
```

In `run`, replace `crate::run(&root, &templates, dry_run)?` inside `apply` with:

```rust
    // `IsTerminal` is how std asks "is a human here?": prompts only make sense on a tty.
    let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
    let deps = crate::docker::Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      prompt: &aeth_devkit_core::prompt::StdinPrompt,
      mode: match (dry_run, args.replace_docker, tty) {
        (true, _, _) => crate::docker::Mode::DryRun,
        (false, true, _) => crate::docker::Mode::ReplaceAll,
        (false, false, true) => crate::docker::Mode::Ask,
        (false, false, false) => crate::docker::Mode::KeepAll,
      },
      interactive: tty && !dry_run,
    };
    let mut c = crate::run_with(&root, &templates, dry_run, &deps)?;
```

Update the doc comment on `Args` / the README later (Task 12).

- [ ] **Step 8: `git.rs` dynamic committable list**

Replace `COMMITTABLE` and its two uses:

```rust
/// Every committable file `setup-project` can write. Static except the Docker pair: the
/// compose file is wherever docker-pin's discovery finds it (or `docker/compose.yaml` when
/// it will be created). Env files and `settings.local.json` are intentionally local and
/// never committed, so they are not listed. Staging a path the project lacks is a no-op.
pub fn committable(root: &Path) -> Vec<String> {
  let mut out: Vec<String> = [
    "pyproject.toml",
    ".vscode/settings.json",
    ".vscode/extensions.json",
    ".vscode/launch.json",
    ".vscode/tasks.json",
    ".gitignore",
    ".gitattributes",
    ".dockerignore",
    "AGENTS.md",
    ".claude/CLAUDE.md",
    ".claude/settings.json",
    ".github/workflows/claude.yml",
    ".github/workflows/release.yml",
    ".mcp.json",
    "docker/Dockerfile",
  ]
  .iter()
  .map(|s| s.to_string())
  .collect();
  let compose = aeth_devkit_core::compose::find_compose_file(root)
    .ok()
    .flatten()
    .and_then(|p| p.strip_prefix(root).ok().map(|r| r.to_string_lossy().replace('\\', "/")))
    .unwrap_or_else(|| "docker/compose.yaml".into());
  out.push(compose);
  out
}
```

`stage_bases`: `let all = committable(root); let paths: Vec<&str> = all.iter().map(String::as_str).filter(|rel| !is_ignored(root, rel)).collect();`. Same in `commit_changes` for `created`.

- [ ] **Step 9: Run**

Run: `cargo test -p aeth-devkit-setup`
Expected: `tests/docker.rs` and `tests/apply.rs` pass. Expect to adjust the `details.len() == 3` count or diff wording if the engine reports differently; keep the semantic assertions.

- [ ] **Step 10: Commit**

```bash
cargo fmt --all
git add crates/aeth-devkit-setup
git commit -m "feat(setup-project): Docker files managed with per-file consent"
```

---

### Task 8: Container crate — pyproject readers and the two query subcommands

**Files:**
- Create: `crates/aeth-devkit-container/Cargo.toml`, `src/main.rs`, `src/pyproject.rs`, `src/mounts.rs` (empty doc line for now), `src/prepare.rs` (same), `src/run.rs` (same)
- Modify: root `Cargo.toml` (`nix` in workspace deps; the `crates/*` glob already picks the member up)

**Interfaces:**
- Produces: `pyproject::load(&Path) -> Result<DocumentMut>`, `pyproject::app_extra(&DocumentMut) -> bool`, `pyproject::readme(&DocumentMut) -> Option<String>`, `pyproject::launch_script(&DocumentMut) -> Result<String>`, `pyproject::required_persisted_dirs(&DocumentMut) -> Result<Vec<String>>`; binary `devkit-container` with `app-extra`, `readme`, `run`.

- [ ] **Step 1: Cargo manifests**

Root `Cargo.toml` `[workspace.dependencies]` add `similar = "3.2.0"` (if not already from Task 5) and `nix = { version = "0.31.3", features = ["user"] }`, plus `aeth-devkit-container = { path = "crates/aeth-devkit-container" }`.

`crates/aeth-devkit-container/Cargo.toml`:

```toml
[package]
  name    = "aeth-devkit-container"
  version.workspace = true
  edition.workspace = true
  publish.workspace = true

[[bin]]
  name = "devkit-container"
  path = "src/main.rs"

[dependencies]
  anyhow    = { workspace = true }
  clap      = { workspace = true }
  toml_edit = { workspace = true }

# `nix` has no Windows build at all, so it must not even be resolved there.
[target.'cfg(unix)'.dependencies]
  nix = { workspace = true }

[dev-dependencies]
  tempfile = { workspace = true }
```

- [ ] **Step 2: Failing tests** (in `pyproject.rs`)

```rust
#[cfg(test)]
mod tests {
  use super::*;

  fn doc(s: &str) -> DocumentMut {
    s.parse().unwrap()
  }

  #[test]
  fn app_extra_and_readme() {
    let d = doc("[project]\nreadme = \"README.md\"\n[project.optional-dependencies]\napp = [\"x\"]\n");
    assert!(app_extra(&d));
    assert_eq!(readme(&d).as_deref(), Some("README.md"));
    let d = doc("[project]\nreadme = { file = \"docs/R.md\" }\n");
    assert!(!app_extra(&d));
    assert_eq!(readme(&d).as_deref(), Some("docs/R.md"));
    assert_eq!(readme(&doc("[project]\n")), None);
  }

  #[test]
  fn exactly_one_run_app_script() {
    let d = doc("[project.scripts]\nrun-app-x = \"m:main\"\nother = \"m:o\"\n");
    assert_eq!(launch_script(&d).unwrap(), "run-app-x");
    let none = launch_script(&doc("[project.scripts]\nother = \"m:o\"\n")).unwrap_err().to_string();
    assert!(none.contains("run-app-") && none.contains("other"), "{none}");
    let many = launch_script(&doc("[project.scripts]\nrun-app-a = \"m\"\nrun-app-b = \"m\"\n"))
      .unwrap_err()
      .to_string();
    assert!(many.contains("run-app-a, run-app-b"), "{many}");
    assert!(launch_script(&doc("[project]\n")).unwrap_err().to_string().contains("(none)"));
  }

  #[test]
  fn persisted_dirs_are_validated() {
    let d = doc("[tool.docker]\nrequired_persisted_dirs = [\"persisted_data\", \"data/sub/\"]\n");
    assert_eq!(required_persisted_dirs(&d).unwrap(), ["persisted_data", "data/sub"]);
    assert_eq!(required_persisted_dirs(&doc("[project]\n")).unwrap(), Vec::<String>::new());
    for bad in ["\"\"", "\".\"", "\"..\"", "\"/abs\"", "\"a/../b\"", "\"./x\""] {
      let d = doc(&format!("[tool.docker]\nrequired_persisted_dirs = [{bad}]\n"));
      assert!(required_persisted_dirs(&d).is_err(), "{bad} must be rejected");
    }
    let legacy = required_persisted_dirs(&doc("[tool.docker]\nchown_paths = [\"x\"]\n")).unwrap_err().to_string();
    assert!(legacy.contains("chown_paths"), "{legacy}");
  }
}
```

- [ ] **Step 3: Implement `pyproject.rs`**

```rust
//! The questions the image asks of `/app/pyproject.toml`, read with `toml_edit` (a parse
//! only; nothing is written back).

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use toml_edit::{DocumentMut, Item};

pub fn load(path: &Path) -> Result<DocumentMut> {
  std::fs::read_to_string(path)
    .with_context(|| format!("reading {}", path.display()))?
    .parse()
    .with_context(|| format!("parsing {}", path.display()))
}

/// `[project.optional-dependencies].app` exists → the image syncs `--extra app`.
pub fn app_extra(doc: &DocumentMut) -> bool {
  doc
    .get("project")
    .and_then(|p| p.get("optional-dependencies"))
    .and_then(|o| o.get("app"))
    .is_some()
}

/// `project.readme` as a path: either the string form or the `{ file = "…" }` table form.
pub fn readme(doc: &DocumentMut) -> Option<String> {
  let item = doc.get("project")?.get("readme")?;
  match item {
    Item::Value(v) if v.is_str() => v.as_str().map(str::to_string),
    // `as_table_like` covers both `[project.readme]` and the inline `{ file = … }`.
    _ => item.as_table_like()?.get("file")?.as_str().map(str::to_string),
  }
}

/// The single `[project.scripts]` key with the `run-app-` prefix; zero or several is an
/// error that names what was found (the old `get_launch_script.py` rule).
pub fn launch_script(doc: &DocumentMut) -> Result<String> {
  let scripts: Vec<String> = doc
    .get("project")
    .and_then(|p| p.get("scripts"))
    .and_then(|s| s.as_table_like())
    .map(|t| t.iter().map(|(k, _)| k.to_string()).collect())
    .unwrap_or_default();
  let matches: Vec<&String> = scripts.iter().filter(|s| s.starts_with("run-app-")).collect();
  match matches.as_slice() {
    [one] => Ok((*one).clone()),
    [] => {
      let available = if scripts.is_empty() { "(none)".to_string() } else { scripts.join(", ") };
      bail!("no [project.scripts] entry with a 'run-app-' prefix found (available scripts: {available})")
    }
    many => bail!(
      "multiple 'run-app-' scripts found ({}); define exactly one",
      many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
    ),
  }
}

/// `[tool.docker].required_persisted_dirs`, each entry validated so nothing can resolve to
/// `/app` itself or outside it. A table still carrying the legacy keys without the new
/// one is refused with the migration hint rather than silently chowning nothing.
pub fn required_persisted_dirs(doc: &DocumentMut) -> Result<Vec<String>> {
  let docker = doc.get("tool").and_then(|t| t.get("docker"));
  let Some(arr) = docker.and_then(|d| d.get("required_persisted_dirs")).and_then(|v| v.as_array()) else {
    if docker.is_some_and(|d| d.get("chown_paths").is_some() || d.get("mkdirs").is_some()) {
      bail!(
        "[tool.docker] still uses chown_paths/mkdirs; run `devkit setup-project` and fold them into required_persisted_dirs"
      );
    }
    return Ok(Vec::new());
  };
  let mut out = Vec::new();
  for v in arr.iter() {
    let Some(s) = v.as_str() else { bail!("required_persisted_dirs entries must be strings, got {v}") };
    let entry = s.trim().trim_end_matches('/');
    let bad = entry.is_empty()
      || entry.starts_with('/')
      || entry.split('/').any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
      bail!("required_persisted_dirs entry {s:?} is not a relative path inside /app");
    }
    out.push(entry.to_string());
  }
  Ok(out)
}
```

- [ ] **Step 4: `main.rs`**

```rust
//! `devkit-container` — the image-side helper: build-time pyproject queries and the
//! container entrypoint. No Python is needed for either.

// Off Unix only the query subcommands exist, so the entrypoint's helpers would be flagged
// as dead code there; the attribute keeps the Windows build warning-free.
#[cfg_attr(not(unix), allow(dead_code))]
mod mounts;
#[cfg_attr(not(unix), allow(dead_code))]
mod prepare;
mod pyproject;
#[cfg(unix)]
mod run;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "devkit-container", version, about)]
struct Cli {
  #[command(subcommand)]
  command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
  /// Print `--extra app` when pyproject declares an `app` optional-dependency group.
  AppExtra {
    #[arg(long, default_value = "/app/pyproject.toml")]
    pyproject: PathBuf,
  },
  /// Print `project.readme` (nothing when unset), without a trailing newline.
  Readme {
    #[arg(long, default_value = "/app/pyproject.toml")]
    pyproject: PathBuf,
  },
  /// The entrypoint: check mounts, prepare the persisted dirs, drop to nonroot, exec.
  Run {
    #[arg(long, default_value = "/app/pyproject.toml")]
    pyproject: PathBuf,
    #[arg(long, default_value = "/app")]
    app_root: PathBuf,
    #[arg(long, default_value = "/proc/self/mountinfo")]
    mountinfo: PathBuf,
  },
}

fn main() -> ExitCode {
  let cli = Cli::parse();
  let result = match cli.command {
    Command::AppExtra { pyproject } => pyproject::load(&pyproject).map(|d| {
      if pyproject::app_extra(&d) {
        // `print!` (no newline): the Dockerfile splices this into a `uv sync` line.
        print!("--extra app");
      }
    }),
    Command::Readme { pyproject } => pyproject::load(&pyproject).map(|d| print!("{}", pyproject::readme(&d).unwrap_or_default())),
    #[cfg(unix)]
    Command::Run { pyproject, app_root, mountinfo } => run::run(&run::RunArgs { pyproject, app_root, mountinfo }),
    #[cfg(not(unix))]
    Command::Run { .. } => Err(anyhow::anyhow!("unsupported platform: `run` is the Linux container entrypoint")),
  };
  match result {
    Ok(()) => ExitCode::SUCCESS,
    Err(e) => {
      eprintln!("error: {e:#}");
      ExitCode::from(1)
    }
  }
}
```

Create `mounts.rs`, `prepare.rs`, `run.rs` with a one-line `//!` doc each (filled in Tasks 9–10; `run.rs` may stay empty of items until Task 10 — add `#![allow(dead_code)]` nowhere; an empty module compiles).

- [ ] **Step 5: Run**

Run: `cargo test -p aeth-devkit-container` and `cargo run -q -p aeth-devkit-container -- app-extra --pyproject pyproject.toml` (devkit's own pyproject has no `app` extra: prints nothing, exit 0).
Expected: tests pass; the binary builds on Windows.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add Cargo.toml Cargo.lock crates/aeth-devkit-container
git commit -m "feat(container): devkit-container crate with app-extra and readme"
```

---

### Task 9: Container crate — mount check and directory preparation

**Files:**
- Modify: `crates/aeth-devkit-container/src/mounts.rs`, `crates/aeth-devkit-container/src/prepare.rs`

**Interfaces:**
- Produces: `mounts::parse_mountinfo(&str) -> Vec<PathBuf>`, `mounts::is_backed(&[PathBuf], app_root: &Path, entry: &str) -> bool`, `mounts::unbacked<'a>(&[PathBuf], &Path, &'a [String]) -> Vec<&'a str>`; `prepare::prepare(app_root: &Path, entries: &[String], chown: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()>`, `#[cfg(unix)] prepare::chown_nonroot(&Path) -> Result<()>`.

- [ ] **Step 1: Failing tests**

`mounts.rs`:

```rust
#[cfg(test)]
mod tests {
  use super::*;

  const MOUNTINFO: &str = "\
22 28 0:21 / /proc rw,nosuid - proc proc rw
28 0 8:1 / / rw,relatime - ext4 /dev/sda1 rw
99 28 8:2 /data/x_files /app/persisted_data rw,relatime - ext4 /dev/sda2 rw
100 28 8:2 /d /app/with\\040space rw - ext4 /dev/sda2 rw
";

  #[test]
  fn mount_points_come_from_field_five_with_octal_escapes_decoded() {
    let m = parse_mountinfo(MOUNTINFO);
    assert!(m.contains(&PathBuf::from("/app/persisted_data")));
    assert!(m.contains(&PathBuf::from("/app/with space")));
    assert!(m.contains(&PathBuf::from("/")));
  }

  #[test]
  fn an_entry_is_backed_by_itself_or_an_ancestor_below_app() {
    let m = parse_mountinfo(MOUNTINFO);
    let app = Path::new("/app");
    assert!(is_backed(&m, app, "persisted_data"));
    assert!(is_backed(&m, app, "persisted_data/logs/deeper"));
    // `/` is a mount point, but the walk stops before `/app`, so root does not count.
    assert!(!is_backed(&m, app, "scratch"));
    let entries = vec!["persisted_data".to_string(), "scratch".to_string(), "other/x".to_string()];
    assert_eq!(unbacked(&m, app, &entries), ["scratch", "other/x"]);
  }
}
```

`prepare.rs`:

```rust
#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn creates_missing_dirs_and_chowns_every_path_in_each_tree() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("persisted_data/logs")).unwrap();
    std::fs::write(root.join("persisted_data/logs/a.txt"), "x").unwrap();
    let mut seen: Vec<PathBuf> = Vec::new();
    prepare(root, &["persisted_data".into(), "new/deep".into()], &mut |p| {
      seen.push(p.to_path_buf());
      Ok(())
    })
    .unwrap();
    assert!(root.join("new/deep").is_dir());
    for rel in ["persisted_data", "persisted_data/logs", "persisted_data/logs/a.txt", "new/deep"] {
      assert!(seen.contains(&root.join(rel)), "{rel} not chowned: {seen:?}");
    }
    // `new` itself was created on the way to `new/deep` and must be chowned too, or nonroot
    // cannot traverse into its own directory.
    assert!(seen.contains(&root.join("new")), "{seen:?}");
  }

  #[test]
  fn a_chown_failure_stops_with_the_path() {
    let dir = tempfile::tempdir().unwrap();
    let err = prepare(dir.path(), &["x".into()], &mut |p| anyhow::bail!("nope {}", p.display())).unwrap_err();
    assert!(err.to_string().contains("nope"), "{err}");
  }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-container`
Expected: compile errors.

- [ ] **Step 3: Implement `mounts.rs`**

```rust
//! Reading `/proc/self/mountinfo` and deciding whether a required directory is backed by
//! a mount — the "container started without its volume" check.

use std::path::{Path, PathBuf};

/// Mount points (field 5 of each mountinfo line). Paths with spaces are written with
/// octal escapes (`\040`), decoded here.
pub fn parse_mountinfo(text: &str) -> Vec<PathBuf> {
  text
    .lines()
    // `nth(4)` is the fifth whitespace-separated field: the mount point.
    .filter_map(|l| l.split_whitespace().nth(4))
    .map(|p| PathBuf::from(unescape(p)))
    .collect()
}

fn unescape(s: &str) -> String {
  let mut out = String::with_capacity(s.len());
  let bytes = s.as_bytes();
  let mut i = 0;
  while i < bytes.len() {
    // `\` followed by three octal digits is one byte; anything else is literal. The bound
    // guarantees indices `i+1..=i+3` exist before the slice is taken.
    if bytes[i] == b'\\' && i + 3 < bytes.len() && bytes[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b)) {
      let code = u8::from_str_radix(&s[i + 1..i + 4], 8).unwrap_or(b'?');
      out.push(code as char);
      i += 4;
    } else {
      out.push(bytes[i] as char);
      i += 1;
    }
  }
  out
}

/// Walk from `app_root/entry` upwards, stopping *before* `app_root`; `true` if any path on
/// the way is a mount point. The root filesystem is always a mount, which is why the walk
/// must exclude `/app` and above.
pub fn is_backed(mounts: &[PathBuf], app_root: &Path, entry: &str) -> bool {
  let mut p = app_root.join(entry);
  while p != app_root {
    if mounts.iter().any(|m| m == &p) {
      return true;
    }
    // `parent()` is `None` only at the filesystem root, which we never reach from under
    // `app_root`; treat it as "not backed" rather than looping forever.
    let Some(parent) = p.parent() else { return false };
    p = parent.to_path_buf();
  }
  false
}

/// The entries with no mount behind them, in input order.
pub fn unbacked<'a>(mounts: &[PathBuf], app_root: &Path, entries: &'a [String]) -> Vec<&'a str> {
  entries
    .iter()
    .map(String::as_str)
    .filter(|e| !is_backed(mounts, app_root, e))
    .collect()
}
```

- [ ] **Step 4: Implement `prepare.rs`**

```rust
//! `mkdir -p` + recursive chown of every required persisted dir. The chown is injected
//! so the loop is testable on any platform and by any user.

use std::path::Path;

use anyhow::{Context as _, Result};

/// The uid and gid the Dockerfile creates for `nonroot`.
pub const NONROOT: u32 = 999;

/// Create each `entry` under `app_root` (no-op where the mount already provides it), then
/// hand every path in its tree — and every directory created on the way to it — to `chown`.
pub fn prepare(app_root: &Path, entries: &[String], chown: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
  for entry in entries {
    let dir = app_root.join(entry);
    // Remember which ancestors did not exist yet: they are created by `create_dir_all`
    // and must be chowned too, or nonroot could not descend into its own directory.
    let mut created: Vec<std::path::PathBuf> = Vec::new();
    let mut probe = dir.as_path();
    while probe != app_root && !probe.exists() {
      created.push(probe.to_path_buf());
      let Some(parent) = probe.parent() else { break };
      probe = parent;
    }
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    // Deepest first in `created`; chown the shallow ones, then the whole target tree.
    for p in created.iter().rev().filter(|p| *p != &dir) {
      chown(p)?;
    }
    walk(&dir, chown)?;
  }
  Ok(())
}

fn walk(path: &Path, chown: &mut dyn FnMut(&Path) -> Result<()>) -> Result<()> {
  chown(path)?;
  if path.is_dir() {
    for entry in std::fs::read_dir(path).with_context(|| format!("listing {}", path.display()))? {
      walk(&entry?.path(), chown)?;
    }
  }
  Ok(())
}

/// Give `path` to uid/gid 999 without following symlinks (a symlink inside a mounted
/// volume must not redirect the chown elsewhere).
#[cfg(unix)]
pub fn chown_nonroot(path: &Path) -> Result<()> {
  std::os::unix::fs::lchown(path, Some(NONROOT), Some(NONROOT)).with_context(|| format!("chown {}", path.display()))
}
```

- [ ] **Step 5: Run**

Run: `cargo test -p aeth-devkit-container`
Expected: pass on Windows and Linux.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/aeth-devkit-container
git commit -m "feat(container): mount check and persisted-dir preparation"
```

---

### Task 10: Container crate — the `run` entrypoint

**Files:**
- Modify: `crates/aeth-devkit-container/src/run.rs` (Unix only), `src/main.rs` (already dispatches)
- Create: `crates/aeth-devkit-container/tests/entrypoint.rs`

**Interfaces:**
- Produces: `run::RunArgs { pyproject, app_root, mountinfo }`, `run::run(&RunArgs) -> Result<()>` (returns only on failure; success `exec`s).

- [ ] **Step 1: Implement `run.rs`**

```rust
//! The container entrypoint (Linux only): the shell script's job, in order.

use std::path::PathBuf;

use anyhow::{Context as _, Result, anyhow, bail};
use nix::unistd::{Gid, Uid, getuid, setgid, setgroups, setuid};

use crate::{mounts, prepare, pyproject};

pub struct RunArgs {
  pub pyproject: PathBuf,
  pub app_root: PathBuf,
  pub mountinfo: PathBuf,
}

/// Steps 1–5 of the spec. Every check happens before the filesystem is touched.
pub fn run(args: &RunArgs) -> Result<()> {
  // 1. Root only: chown and the privilege drop need it (same rule as the old script).
  if !getuid().is_root() {
    bail!("entrypoint must run as root (uid 0); got uid {}", getuid());
  }
  let doc = pyproject::load(&args.pyproject)?;
  // 2. The launch command — resolved first so a misconfigured pyproject fails before any
  //    directory is created.
  let script = pyproject::launch_script(&doc)?;
  let entries = pyproject::required_persisted_dirs(&doc)?;
  // 3. Mount check.
  let mountinfo = std::fs::read_to_string(&args.mountinfo).with_context(|| format!("reading {}", args.mountinfo.display()))?;
  let mounts = mounts::parse_mountinfo(&mountinfo);
  let missing = mounts::unbacked(&mounts, &args.app_root, &entries);
  if !missing.is_empty() {
    bail!(
      "required_persisted_dirs not backed by a bind mount (start the container with its volume): {}",
      missing.join(", ")
    );
  }
  // 4. mkdir -p + recursive chown.
  prepare::prepare(&args.app_root, &entries, &mut prepare::chown_nonroot)?;
  // 5. Drop privileges, then replace this process with the app. Order matters: once the
  //    uid is 999 the process may no longer change its groups, so groups and gid go first.
  let exe = args.app_root.join(".venv").join("bin").join(&script);
  setgroups(&[]).context("setgroups")?;
  setgid(Gid::from_raw(prepare::NONROOT)).context("setgid")?;
  setuid(Uid::from_raw(prepare::NONROOT)).context("setuid")?;
  // `exec` never returns on success: the current process image is replaced. The only
  // thing it can hand back is the error that stopped it.
  use std::os::unix::process::CommandExt as _;
  let err = std::process::Command::new(&exe).exec();
  Err(anyhow!(err)).with_context(|| format!("exec {}", exe.display()))
}
```

- [ ] **Step 2: Integration test** — `tests/entrypoint.rs`

```rust
//! The binary end to end. `run` is Linux-only and needs root for the chown and the
//! privilege drop, so that part is `#[ignore]`; the query subcommands run everywhere.

use std::path::Path;
use std::process::Command;

fn bin() -> Command {
  Command::new(env!("CARGO_BIN_EXE_devkit-container"))
}

fn write(root: &Path, rel: &str, content: &str) {
  let p = root.join(rel);
  std::fs::create_dir_all(p.parent().unwrap()).unwrap();
  std::fs::write(p, content).unwrap();
}

#[test]
fn query_subcommands_print_without_trailing_newline() {
  let dir = tempfile::tempdir().unwrap();
  write(
    dir.path(),
    "pyproject.toml",
    "[project]\nreadme = \"README.md\"\n[project.optional-dependencies]\napp = []\n",
  );
  let py = dir.path().join("pyproject.toml");
  let out = bin().args(["app-extra", "--pyproject"]).arg(&py).output().unwrap();
  assert_eq!(String::from_utf8_lossy(&out.stdout), "--extra app");
  let out = bin().args(["readme", "--pyproject"]).arg(&py).output().unwrap();
  assert_eq!(String::from_utf8_lossy(&out.stdout), "README.md");
}

#[cfg(not(unix))]
#[test]
fn run_is_unsupported_off_unix() {
  let out = bin().arg("run").output().unwrap();
  assert_eq!(out.status.code(), Some(1));
  assert!(String::from_utf8_lossy(&out.stderr).contains("unsupported platform"));
}

#[cfg(unix)]
#[test]
fn run_refuses_a_non_root_caller() {
  // Only meaningful when *not* root; CI and dev shells are not.
  if nix_is_root() {
    return;
  }
  let out = bin().arg("run").output().unwrap();
  assert_eq!(out.status.code(), Some(1));
  assert!(String::from_utf8_lossy(&out.stderr).contains("must run as root"));
}

#[cfg(unix)]
fn nix_is_root() -> bool {
  std::fs::metadata("/proc/self").map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0).unwrap_or(false)
}

/// Needs root: `sudo -E cargo test -p aeth-devkit-container -- --ignored`.
#[cfg(unix)]
#[test]
#[ignore]
fn as_root_checks_mounts_prepares_dirs_drops_privileges_and_execs() {
  let dir = tempfile::tempdir().unwrap();
  let root = dir.path();
  write(
    root,
    "pyproject.toml",
    "[project.scripts]\nrun-app-demo = \"m:main\"\n[tool.docker]\nrequired_persisted_dirs = [\"persisted_data\"]\n",
  );
  write(root, ".venv/bin/run-app-demo", "#!/bin/sh\nid -u > \"$(dirname \"$0\")/../../uid.txt\"\n");
  std::fs::set_permissions(root.join(".venv/bin/run-app-demo"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
  let no_mount = root.join("mountinfo-none");
  std::fs::write(&no_mount, "28 0 8:1 / / rw - ext4 /dev/sda1 rw\n").unwrap();
  let out = bin()
    .args(["run", "--pyproject"]).arg(root.join("pyproject.toml"))
    .arg("--app-root").arg(root)
    .arg("--mountinfo").arg(&no_mount)
    .output().unwrap();
  assert_eq!(out.status.code(), Some(1));
  assert!(String::from_utf8_lossy(&out.stderr).contains("persisted_data"));

  let mounted = root.join("mountinfo-ok");
  std::fs::write(&mounted, format!("99 28 8:2 /x {} rw - ext4 /dev/sda2 rw\n", root.join("persisted_data").display())).unwrap();
  let out = bin()
    .args(["run", "--pyproject"]).arg(root.join("pyproject.toml"))
    .arg("--app-root").arg(root)
    .arg("--mountinfo").arg(&mounted)
    .output().unwrap();
  assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
  assert_eq!(std::fs::read_to_string(root.join("uid.txt")).unwrap().trim(), "999");
  let meta = std::fs::metadata(root.join("persisted_data")).unwrap();
  assert_eq!(std::os::unix::fs::MetadataExt::uid(&meta), 999);
}
```

- [ ] **Step 3: Run**

Run: `cargo test -p aeth-devkit-container` (Windows: `run_is_unsupported_off_unix` passes; Linux non-root: `run_refuses_a_non_root_caller` passes). Also `cargo build -p aeth-devkit-container` must succeed on Windows. If a Linux machine or WSL with Rust is available, additionally run `sudo -E cargo test -p aeth-devkit-container -- --ignored` and report the result; if not, state that the root test was not run.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/aeth-devkit-container
git commit -m "feat(container): run entrypoint with mount check and privilege drop"
```

---

### Task 11: Release workflow builds and publishes the container binary

**Files:**
- Modify: `python/aeth_devkit/templates/github/workflows/release.rust.template.yml`, `crates/aeth-devkit-setup/tests/apply.rs` (`mixed_rust_python_project_uses_python_dir_and_rust_overlays`), `.github/workflows/release.yml` (regenerated)

- [ ] **Step 1: Failing test** — add to the mixed Rust/Python e2e test in `tests/apply.rs`:

```rust
  let wf = read(root, ".github/workflows/release.yml");
  assert!(wf.contains("cargo build --release -p aeth-devkit-container --target"), "{wf}");
  assert!(wf.contains("container_target: x86_64-unknown-linux-musl"), "{wf}");
  assert!(wf.contains("container_target: x86_64-pc-windows-msvc"), "{wf}");
  assert!(wf.contains("dist/*.whl dist/*.tar.gz"), "publish must not feed binaries to uv: {wf}");
```

- [ ] **Step 2: Edit the template**

Header comment: change "Wheels are built per platform with maturin;" to "Wheels are built per platform with maturin and the `devkit-container` entrypoint binary alongside them (static musl on Linux);".

Matrix:

```yaml
        include:
          - os: windows-latest
            target: x86_64-pc-windows-msvc
            container_target: x86_64-pc-windows-msvc
            exe: .exe
          - os: ubuntu-latest
            target: x86_64-unknown-linux-gnu
            manylinux: "2_17"
            # The container binary is fully static so the image needs no runtime libs.
            container_target: x86_64-unknown-linux-musl
            exe: ""
```

Toolchain step: `targets: ${{ matrix.target }},${{ matrix.container_target }}` (the action takes a comma-separated list). Job name: `Wheel + container (${{ matrix.target }})`.

After the maturin step, before the wheel upload:

```yaml
      # The container entrypoint, named by target so the Dockerfile's ADD URL is stable.
      - name: Build the container entrypoint binary
        shell: bash
        run: |
          cargo build --release -p aeth-devkit-container --target "${{ matrix.container_target }}"
          mkdir -p dist
          cp "target/${{ matrix.container_target }}/release/devkit-container${{ matrix.exe }}" \
             "dist/devkit-container-${{ matrix.container_target }}${{ matrix.exe }}"

      - uses: actions/upload-artifact@v4
        with:
          name: container-${{ matrix.target }}
          path: dist/devkit-container-*
          if-no-files-found: error
```

Publish job: both `uv publish` lines get explicit globs — `uv publish --index {publish_index} dist/*.whl dist/*.tar.gz` and `uv publish --trusted-publishing always dist/*.whl dist/*.tar.gz`. `gh release upload "$TAG" dist/* --clobber` stays (it attaches the binaries).

- [ ] **Step 3: Regenerate devkit's own workflow**

Run: `cargo run -q -p aeth-devkit -- setup-project --no-commit`
Expected: only `.github/workflows/release.yml` (and possibly `python/aeth_devkit/_tasks_generated.py`, from the build) change. Check with `git status --short`; anything else changing means a template regression — investigate before committing.

- [ ] **Step 4: Run**

Run: `cargo test -p aeth-devkit-setup --test apply mixed_rust`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add python/aeth_devkit/templates/github/workflows/release.rust.template.yml crates/aeth-devkit-setup/tests/apply.rs .github/workflows/release.yml
git commit -m "feat(release): build and attach the devkit-container binary per platform"
```

---

### Task 12: Documentation and the poe task help

**Files:**
- Modify: `README.md`, `TODO.md`, `python/aeth_devkit/_tasks_source.py`, `python/aeth_devkit/_tasks_generated.py` (regenerated by the build)

- [ ] **Step 1: README `setup-project` section**

- In the flags line add `--replace-docker` and change "No prompts;" to "Prompts only for Docker file replacement (see below); otherwise no prompts;".
- "Project discovery" bullet: replace "Docker (a real Dockerfile/compose file enables `.dockerignore` and `[tool.docker]`)" with "Docker (`[tool.docker].services` non-empty enables `.dockerignore`, `[tool.docker]`, and the Docker step)".
- Replace the "Not yet implemented" bullet with:

```markdown
- **Docker** - Runs whenever `[tool.docker].services` lists at least one compose service.
  `docker/Dockerfile` (and any other templated file under `docker/`) is created when
  missing; when present and different — ignoring CRLF/LF — a unified diff is printed and
  the file is replaced only on `replace` (`replace all` answers every remaining Docker
  question; anything else keeps it). The compose file (docker-pin's discovery; created as
  `docker/compose.yaml` when absent) is edited in place, format-preserving, per listed
  service: exact keys (`build.context`, `build.dockerfile`, `container_name`,
  `healthcheck.*`), pattern (`GIT_REPO` vs origin), presence (`GIT_TAG`, `restart`,
  `networks`) and at-least (`volumes` mounting `/app/persisted_data`; the ALERTS_*
  environment when the project uses aeth_ext), plus top-level `networks.coolify.external`.
  All compose edits are one diff and one prompt; a listed service missing from the file is
  offered as a scaffold block (`add`). Keys the standard does not name are never touched.
  `--replace-docker` answers `replace all` up front; without a terminal every answer is
  "keep" and a `note:` says so; `--dry-run`/`--check` print everything and count Docker
  drift. `docker/entrypoint.sh` and `docker/scripts/` are reported as safe to delete, never
  removed. New placeholders: `{devkit_version}`, `{git_repo}`, `{git_tag}` (latest stable
  remote tag, resolved lazily; falls back to `v<pyproject version>` with a note), `{service}`.
- **Not yet implemented** - (see TODO.md) `--python-dir` override, vendored-gitignore
  refresh task.
```

- Add a section after `### devkit docker-pin`:

```markdown
### `devkit-container`

A separate static binary (crate `aeth-devkit-container`, release assets
`devkit-container-x86_64-unknown-linux-musl` and `devkit-container-x86_64-pc-windows-msvc.exe`)
that the templated Dockerfile downloads at build time, pinned to the devkit version that
rendered it. No Python runs in the image outside the app itself.

- `app-extra` - prints `--extra app` when `[project.optional-dependencies].app` exists.
- `readme` - prints `project.readme` (string or `{ file = … }` form).
- `run` - the entrypoint (Linux only). Must be root. Resolves the single `run-app-*`
  script in `[project.scripts]`; checks every `[tool.docker].required_persisted_dirs`
  entry is backed by a bind mount (the path or an ancestor below `/app`, per
  `/proc/self/mountinfo`) and refuses to start otherwise; `mkdir -p` + recursive chown to
  `999:999`; `setgroups([])`, `setgid`, `setuid`; `exec /app/.venv/bin/<script>`. `/app`
  itself stays root-owned: the app writes only to its mounted dirs or temp dirs. Entries
  that are empty, `.`, `..`, absolute or escape `/app` are errors; a table still carrying
  `chown_paths`/`mkdirs` without `required_persisted_dirs` is refused with the migration
  hint. Flags `--pyproject`, `--app-root`, `--mountinfo` exist for tests.
```

- Under `### devkit release` (or the release workflow bullet in setup-project) add one sentence: the Rust matrix template also builds `aeth-devkit-container` per platform (musl on Linux) and attaches both binaries to the release.

- [ ] **Step 2: TODO.md**

- Delete the whole "Docker scaffolding flags" item and the "Docker standardization … extends the Rust release workflow's matrix" item.
- Under `## setup-project` add:

```markdown
- [ ] Sister-project Docker migration (after the first devkit release that ships
      `devkit-container`): in each of aeth_ext, IMAPReportCollector, ScheduledInvoiceProcessor,
      ScheduledReportAggregator — add `[tool.docker].services = ["<service>"]`, run
      `poe setup-project` (answer `replace` for the Dockerfile, review the compose diff),
      fold `chown_paths` into `required_persisted_dirs`, delete `chown_paths`/`mkdirs`,
      delete `docker/entrypoint.sh` and `docker/scripts/`, then `poe docker-pin`.
      ScheduledInvoiceProcessor and ScheduledReportAggregator first move `file_holding` /
      `timeclock_playground` to temp dirs (on their own TODO lists, high priority).
- [ ] IMAPReportCollector: `[tool.docker].mkdirs = [""]` is a data bug (would have chowned
      `/app`); goes away with the migration above.
- [ ] `release.rust.template.yml` builds `aeth-devkit-container` unconditionally; a future
      Rust sister project without a container crate needs a `{container_crate}`-style gate.
```

- [ ] **Step 3: poe task help**

In `python/aeth_devkit/_tasks_source.py`, the `setup-project` help's last sentence becomes:

```python
      "Extra args are passed to devkit setup-project: --dry-run, --check, --no-commit, "
      "--replace-docker, --templates-dir PATH."
```

Regenerate: `cargo build -p aeth-devkit`, then confirm `python/aeth_devkit/_tasks_generated.py` changed (`git status --short`).

- [ ] **Step 4: Run**

Run: `uv run pytest tests/test_generated_tasks.py tests/test_build_regenerates.py -q`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add README.md TODO.md python/aeth_devkit/_tasks_source.py python/aeth_devkit/_tasks_generated.py
git commit -m "docs: document Docker standardization and devkit-container"
```

---

### Task 13: Full verification and design-record removal

**Files:**
- Delete: `docs/superpowers/specs/2026-09-03-docker-standardization-design.md`, `docs/superpowers/plans/2026-09-03-docker-standardization.md`

- [ ] **Step 1: Whole suite, once**

Run:
```bash
cargo fmt --all --check
cargo test --workspace
uv run pytest -q
```
Expected: everything green on this (Windows) machine. Report any failure verbatim; do not delete the records until it is green.

- [ ] **Step 2: Manual smoke test against a sister project (dry run only)**

Run from `d:/SFT Software Projects/IMAPReportCollector` after temporarily adding `services = ["imap-report-collector"]` to its `[tool.docker]` (revert afterwards; do not commit there):

```bash
cargo run -q -p aeth-devkit --manifest-path "d:/SFT Software Projects/aeth-devkit/Cargo.toml" -- setup-project --dry-run
```

Expected: a Dockerfile diff (gosu layer removed, `devkit-container` lines), a compose diff that is empty or touches only standard keys, the `chown_paths and mkdirs` advisory, and the stray-files advisory. Paste the output in the task report. Then `git checkout -- pyproject.toml` in that project.

- [ ] **Step 3: Delete the design records and commit**

```bash
git rm docs/superpowers/specs/2026-09-03-docker-standardization-design.md docs/superpowers/plans/2026-09-03-docker-standardization.md
git commit -m "chore(docs): remove the Docker standardization design record before merge"
```

- [ ] **Step 4: Hand off**

Use `superpowers:finishing-a-development-branch`: the user reviews and runs the suite by hand before the PR merges.

---

## Self-review notes

- **Spec coverage:** schema (T3), container binary subcommands and run steps 1–5 (T8–T10), entry validation (T8), templates and placeholders incl. lazy `{git_tag}` (T4, T6), static-file states and prompts (T5, T7), compose discovery/scaffold/rules/add-service/one-diff-one-prompt (T6, T7), modes incl. non-tty note and `--check` (T7), advisories (T3, T5, T6/T7), release matrix + publish glob (T11), docs/TODO/poe (T12), record deletion (T13), Prompt move (T1), tree module (T2), committable list (T7).
- **Type consistency:** `Node`/`ListItem`/`Edit` fields match between T2 and T6; `Deps`/`Mode`/`Consent` between T5 and T7; `ProjectContext` fields between T3, T4, T6, T7; `prepare::NONROOT` referenced from T10.
- **Known soft spots for the executor:** the exact document ordering asserted in `each_rule_kind_edits_exactly_its_key` and the two `details.len()` counts are derived by hand; if the engine's output differs only in placement order or count, correct the test to the implementation's actual behaviour after confirming every intended edit is present and nothing else changed.
