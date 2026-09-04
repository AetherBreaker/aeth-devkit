//! Markdown files with a devkit-managed block (`AGENTS.md`).
//!
//! TOML tables and JSON keys have identity, so merging them can tell "template-owned" from
//! "project-owned" for free. Markdown has none, so we manufacture the boundary: everything
//! between `<!-- devkit:begin -->` and `<!-- devkit:end -->` is devkit's and is replaced
//! wholesale; everything outside is the project's and is never touched.

use anyhow::{Result, bail};

use crate::context::ProjectContext;

pub const BEGIN: &str = "<!-- devkit:begin -->";
pub const END: &str = "<!-- devkit:end -->";
/// `<!-- setup-project: if-dep NAME -->` above a heading gates that section on a dependency.
const IF_DEP_MARKER: &str = "<!-- setup-project: if-dep ";

/// Replace the managed block in `original` with `block`, or append it when absent.
/// `original: None` creates the file. Errors on a `begin` marker with no `end`.
pub fn merge_managed_block(original: Option<&str>, block: &str, log: &mut Vec<String>) -> Result<String> {
  // Work in LF internally and restore CRLF at the end if the file used it, so a
  // Windows-authored AGENTS.md doesn't come back with mixed endings. `is_some_and` is the
  // `Option` idiom for "present *and* satisfies this predicate".
  let crlf = original.is_some_and(|o| o.contains("\r\n"));
  let original = original.map(|o| o.replace("\r\n", "\n"));

  // The block always ends in exactly one newline before the end marker, whatever the
  // template file's trailing whitespace happened to be.
  let wrapped = format!("{BEGIN}\n{}\n{END}\n", block.trim_end_matches('\n'));

  // `as_deref` turns `Option<String>` into `Option<&str>` so we borrow rather than move.
  let merged = match original.as_deref() {
    None => wrapped,
    Some(text) => match text.find(BEGIN) {
      // No block yet: append after the project's own text, separated by a blank line.
      // Appending (not prepending) keeps the project's title at the top where readers
      // expect it.
      None => {
        log.push("added devkit-managed block".into());
        if text.trim().is_empty() {
          wrapped
        } else {
          format!("{}\n\n{wrapped}", text.trim_end_matches('\n'))
        }
      }
      Some(start) => {
        // Search for the end marker only *after* the begin marker. `text[start..].find`
        // returns an offset relative to the slice, so add `start` back to get a document
        // offset. `let … else` binds on the `Some` path and must diverge on the `None` one.
        let Some(rel_end) = text[start..].find(END) else {
          // A begin with no end means someone edited inside the block and lost the marker.
          // Guessing where the block ends could destroy their text, so refuse.
          bail!("{END} marker missing after {BEGIN}; restore it (or remove both) and re-run")
        };
        let mut after = start + rel_end + END.len();
        // Swallow the newline that followed the old end marker; `wrapped` brings its own.
        if text[after..].starts_with('\n') {
          after += 1;
        }
        let merged = format!("{}{wrapped}{}", &text[..start], &text[after..]);
        if merged != text {
          log.push("updated devkit-managed block".into());
        }
        merged
      }
    },
  };
  Ok(if crlf { merged.replace('\n', "\r\n") } else { merged })
}

/// Drop `if-dep` sections whose dependency is absent; strip the markers from the rest.
///
/// A marker governs from the heading below it to the next heading of the same or a higher
/// level (fewer `#`s), or to the end of the text.
///
/// Three things make this fiddlier than it looks, and each one used to make the gate a
/// silent no-op — the marker was stripped either way, so the output *looked* right while the
/// gated section shipped to a project that should never have seen it:
///
/// * the heading may not be on the very next line (a blank line between them is natural
///   Markdown), so blank lines are skipped when looking for it;
/// * a second marker can appear while the first section is still being dropped, so markers
///   are recognised inside the skip loop rather than only outside it;
/// * a `# ` at column 0 inside a fenced code block is a shell comment, not a heading, and
///   letting it close a section leaks the rest plus an unbalanced fence.
pub fn apply_if_dep(template: &str, ctx: &ProjectContext, log: &mut Vec<String>) -> String {
  let mut out = String::with_capacity(template.len());
  // `Some(level)` while we are inside a section being dropped; `None` otherwise.
  let mut skipping: Option<usize> = None;
  // Set while inside a ``` or ~~~ fence, where no line is a heading.
  let mut fence: Option<String> = None;
  // `peekable` lets us look past a marker for its heading without consuming the lines.
  let mut lines = template.lines().peekable();

  while let Some(line) = lines.next() {
    // Track fences first: it decides whether the lines below count as headings at all. A
    // fence inside a section being dropped still has to be tracked, or its closing ``` would
    // be read as opening a new one.
    let fence_token = fence_delimiter(line);
    match (&fence, fence_token) {
      (None, Some(t)) => fence = Some(t),
      // A fence closes only on a delimiter at least as long, and of the same character.
      (Some(open), Some(t)) if t.starts_with(open.as_str()) => fence = None,
      _ => {}
    }
    let level_here = if fence.is_some() { None } else { heading_level(line) };

    if let Some(level) = skipping {
      match level_here {
        // A heading at this level or higher closes the gated section; fall through so the
        // heading itself is examined and emitted.
        Some(l) if l <= level => skipping = None,
        // Another marker while still dropping: gate it independently rather than swallowing
        // it as body text, which would leave its own section ungated.
        _ if marker_dep(line).is_some() => {}
        _ => continue,
      }
    }

    if let Some(dep) = marker_dep(line) {
      // The marker line itself never survives into the output, kept or dropped.
      if ctx.has_dependency(&dep) {
        continue;
      }
      // Gate at the level of the heading this marker introduces, looking past blank lines.
      // A marker with no heading under it gates until *any* heading: the skip loop closes on
      // `l <= level`, so the loosest gate is 6 — every heading (1..=6) satisfies it.
      let mut level = 6;
      while let Some(next) = lines.peek() {
        if next.trim().is_empty() {
          // Part of the gated section's own spacing; drop it with the section.
          lines.next();
          continue;
        }
        if let Some(l) = heading_level(next) {
          level = l;
          // Consume the gated heading here; otherwise the skip loop above would see a
          // heading at its own level and close the section it was just asked to drop.
          lines.next();
        }
        break;
      }
      log.push(format!("skipped if-dep {dep} section"));
      skipping = Some(level);
      continue;
    }
    out.push_str(line);
    out.push('\n');
  }
  // Dropping the last section can leave the separator blank line dangling at the end.
  while out.ends_with("\n\n") {
    out.pop();
  }
  out
}

/// The dependency named by an `<!-- setup-project: if-dep NAME -->` line, if this is one.
fn marker_dep(line: &str) -> Option<String> {
  let rest = line.trim().strip_prefix(IF_DEP_MARKER)?.strip_suffix("-->")?;
  Some(rest.trim().to_string())
}

/// The run of ``` or ~~~ opening or closing a fenced code block, if this line is one.
/// Returned rather than a bool because a fence closes only on a run at least as long as the
/// one that opened it — ```` ```` inside a ``` block is content, not a close.
fn fence_delimiter(line: &str) -> Option<String> {
  let t = line.trim_start();
  for c in ['`', '~'] {
    let n = t.chars().take_while(|&x| x == c).count();
    if n >= 3 {
      return Some(c.to_string().repeat(n));
    }
  }
  None
}

/// `Some(n)` for an ATX heading line with `n` leading `#`s, `None` otherwise.
fn heading_level(line: &str) -> Option<usize> {
  let hashes = line.chars().take_while(|&c| c == '#').count();
  // A real heading has 1–6 hashes followed by a space (`#hashtag` is not a heading).
  // `then_some` maps a bool to `Option`; `filter` then drops the no-space case.
  (1..=6).contains(&hashes).then_some(hashes).filter(|&n| line[n..].starts_with(' '))
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

  const BLOCK: &str = "## Rule\n\nDo the thing.\n";

  fn wrapped(body: &str) -> String {
    format!("{BEGIN}\n{body}{END}\n")
  }

  #[test]
  fn creates_file_from_block() {
    let mut log = vec![];
    let out = merge_managed_block(None, BLOCK, &mut log).unwrap();
    assert_eq!(out, wrapped(BLOCK));
  }

  #[test]
  fn appends_when_no_markers() {
    let mut log = vec![];
    let out = merge_managed_block(Some("# My Project\n\nIntro.\n"), BLOCK, &mut log).unwrap();
    assert_eq!(out, format!("# My Project\n\nIntro.\n\n{}", wrapped(BLOCK)));
    assert!(log.iter().any(|l| l.contains("added")), "{log:?}");
  }

  #[test]
  fn replaces_between_markers_and_preserves_outside() {
    let orig = format!("# Title\n\n{}\n## Project notes\n\nkeep me\n", wrapped("old stuff\n"));
    let mut log = vec![];
    let out = merge_managed_block(Some(&orig), BLOCK, &mut log).unwrap();
    assert_eq!(out, format!("# Title\n\n{}\n## Project notes\n\nkeep me\n", wrapped(BLOCK)));
    assert!(log.iter().any(|l| l.contains("updated")), "{log:?}");
  }

  #[test]
  fn second_merge_is_a_no_op() {
    let mut log = vec![];
    let once = merge_managed_block(Some("# T\n"), BLOCK, &mut log).unwrap();
    let mut log2 = vec![];
    let twice = merge_managed_block(Some(&once), BLOCK, &mut log2).unwrap();
    assert_eq!(once, twice);
    assert!(log2.is_empty(), "{log2:?}");
  }

  #[test]
  fn unterminated_block_is_an_error() {
    let mut log = vec![];
    let err = merge_managed_block(Some(&format!("{BEGIN}\nhalf\n")), BLOCK, &mut log).unwrap_err();
    assert!(err.to_string().contains(END), "{err}");
  }

  #[test]
  fn preserves_crlf() {
    let orig = format!("# T\r\n\r\n{BEGIN}\r\nold\r\n{END}\r\n");
    let mut log = vec![];
    let out = merge_managed_block(Some(&orig), BLOCK, &mut log).unwrap();
    assert_eq!(out, format!("# T\r\n\r\n{BEGIN}\r\n## Rule\r\n\r\nDo the thing.\r\n{END}\r\n"));
  }

  const GATED: &str = "## Always\n\na\n\n<!-- setup-project: if-dep aeth-ext -->\n## Pydantic\n\nb\n\n## Also always\n\nc\n";

  #[test]
  fn if_dep_section_kept_when_dependency_present() {
    let mut log = vec![];
    let out = apply_if_dep(GATED, &ctx(&["aeth-ext"]), &mut log);
    assert_eq!(out, "## Always\n\na\n\n## Pydantic\n\nb\n\n## Also always\n\nc\n");
  }

  #[test]
  fn if_dep_section_dropped_when_dependency_absent() {
    let mut log = vec![];
    let out = apply_if_dep(GATED, &ctx(&[]), &mut log);
    assert_eq!(out, "## Always\n\na\n\n## Also always\n\nc\n");
    assert!(log.iter().any(|l| l.contains("Pydantic") || l.contains("aeth-ext")), "{log:?}");
  }

  #[test]
  fn if_dep_marker_without_heading_closes_at_any_heading() {
    // No heading under the marker: drop only the bare paragraph, not everything up to the next H1.
    let tpl = "## A

a

<!-- setup-project: if-dep nope -->
loose text

### B

b
";
    let mut log = vec![];
    assert_eq!(
      apply_if_dep(tpl, &ctx(&[]), &mut log),
      "## A

a

### B

b
"
    );
  }

  #[test]
  fn if_dep_gated_section_at_end_of_block() {
    let tpl = "## A\n\na\n\n<!-- setup-project: if-dep nope -->\n## Z\n\nz\n";
    let mut log = vec![];
    assert_eq!(apply_if_dep(tpl, &ctx(&[]), &mut log), "## A\n\na\n");
  }

  #[test]
  fn a_blank_line_between_marker_and_heading_does_not_disable_the_gate() {
    // The marker used to have to sit on the very line above its heading. With a blank line
    // between them the lookahead saw "no heading", gated at level 6, and the heading below
    // immediately closed the section — so the gated content shipped while the log still
    // claimed it had been dropped. Invisible, because the marker is stripped either way.
    let tpl = "## A\n\na\n\n<!-- setup-project: if-dep nope -->\n\n## Gated\n\nsecret\n\n## Tail\n\nt\n";
    let mut log = vec![];
    let out = apply_if_dep(tpl, &ctx(&[]), &mut log);
    assert!(!out.contains("Gated"), "gated section leaked:\n{out}");
    assert!(!out.contains("secret"), "gated body leaked:\n{out}");
    assert!(out.contains("## Tail"), "{out}");
  }

  #[test]
  fn adjacent_if_dep_sections_are_gated_independently() {
    // While skipping, a second marker line is not a heading, so it used to be swallowed as
    // body text — and its own heading then closed the *first* skip and was emitted ungated.
    let tpl = "## Always\n\na\n\n<!-- setup-project: if-dep alpha -->\n## Alpha\n\nx\n\n<!-- setup-project: if-dep beta -->\n## Beta\n\ny\n\n## Tail\n\nt\n";
    let mut log = vec![];
    let out = apply_if_dep(tpl, &ctx(&[]), &mut log);
    assert!(!out.contains("## Alpha"), "{out}");
    assert!(!out.contains("## Beta"), "second gate ignored:\n{out}");
    assert!(out.contains("## Always") && out.contains("## Tail"), "{out}");
    assert_eq!(log.len(), 2, "both gates must be recorded: {log:?}");

    // With only `beta` present, exactly that one survives.
    let mut log2 = vec![];
    let out2 = apply_if_dep(tpl, &ctx(&["beta"]), &mut log2);
    assert!(!out2.contains("## Alpha"), "{out2}");
    assert!(out2.contains("## Beta"), "{out2}");
  }

  #[test]
  fn a_hash_inside_a_code_fence_does_not_close_a_gated_section() {
    // `# install` at column 0 in a shell snippet is a comment, not a heading. Treating it as
    // one ended the section early and leaked the rest — including a closing fence whose
    // opener had already been dropped.
    let tpl = "## A\n\na\n\n<!-- setup-project: if-dep nope -->\n## Gated\n\n```bash\n# install\nuv sync\n```\n\nmore secret text\n\n## Tail\n\nt\n";
    let mut log = vec![];
    let out = apply_if_dep(tpl, &ctx(&[]), &mut log);
    assert!(!out.contains("# install"), "fence content leaked:\n{out}");
    assert!(!out.contains("more secret text"), "section tail leaked:\n{out}");
    assert!(!out.contains("```"), "unbalanced fence left behind:\n{out}");
    assert!(out.contains("## Tail"), "{out}");
  }

  #[test]
  fn a_code_fence_is_preserved_when_the_dependency_is_present() {
    let tpl = "<!-- setup-project: if-dep keep -->\n## Gated\n\n```bash\n# install\nuv sync\n```\n";
    let mut log = vec![];
    let out = apply_if_dep(tpl, &ctx(&["keep"]), &mut log);
    assert_eq!(out, "## Gated\n\n```bash\n# install\nuv sync\n```\n");
  }
}
