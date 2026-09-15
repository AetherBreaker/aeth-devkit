//! The Dockerfile windows (hub design 9.3): the regions between `# !window <name>:` and
//! `# !end <name>` belong to the project. Every render copies their lines over unchanged
//! and replaces everything outside them.

use anyhow::{Result, bail};

use crate::gate::{self, Body, Format};

/// One window of a Dockerfile: the 0-based indices of its two marker lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
  pub name: String,
  pub open: usize,
  pub close: usize,
}

/// Every window of `lines`, in file order; `what` names the file in errors. Only a
/// `!window` line and the `!end` naming it count: any other marker-looking line (a project's
/// own `# !note`) is content, never a render error, since this also reads the project's file.
pub fn scan(lines: &[&str], what: &str) -> Result<Vec<Window>> {
  let mut out: Vec<Window> = Vec::new();
  let mut open: Option<(String, usize)> = None;
  for (i, line) in lines.iter().enumerate() {
    let Ok(Some(m)) = gate::find_marker(line, Format::Dockerfile) else {
      continue;
    };
    match gate::parse_body(m.body) {
      Ok(Body::Window(name)) => {
        if let Some((o, at)) = &open {
          bail!("{what}: window `{o}` opened at line {} is still open at line {}", at + 1, i + 1);
        }
        if out.iter().any(|w| w.name == name) {
          bail!("{what}: window `{name}` appears twice (line {})", i + 1);
        }
        open = Some((name, i));
      }
      Ok(Body::End(Some(name))) if open.as_ref().is_some_and(|(o, _)| *o == name) => {
        let (name, at) = open.take().expect("matched above");
        out.push(Window { name, open: at, close: i });
      }
      _ => {}
    }
  }
  if let Some((o, at)) = open {
    bail!("{what}: window `{o}` opened at line {} is not closed", at + 1);
  }
  Ok(out)
}

/// What a splice produced: the text, the change-log details (one per window that carried
/// lines) and the advisories (one per window of the project's file the template lacks).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Spliced {
  pub text: String,
  pub details: Vec<String>,
  pub notes: Vec<String>,
}

/// `rendered` with the project's windows copied in: the lines between the project's markers
/// replace the lines between the template's. Both texts are LF. A window the template lacks
/// goes with its lines (the template's omission is a choice, 9.3): the diff shows the removal
/// and a note names it. `Err` only when the project's file cannot be read as windows.
pub fn splice(rendered: &str, project: &str) -> Result<Spliced> {
  let theirs: Vec<&str> = project.lines().collect();
  let kept = scan(&theirs, "docker/Dockerfile")?;
  let mut out = Spliced {
    text: rendered.to_string(),
    ..Spliced::default()
  };
  if kept.is_empty() {
    return Ok(out);
  }
  let mut ours: Vec<String> = rendered.lines().map(str::to_string).collect();
  let mine = scan(&ours.iter().map(String::as_str).collect::<Vec<_>>(), "the Dockerfile template")?;
  let mut edits: Vec<(&Window, Vec<String>)> = Vec::new();
  for w in &kept {
    let lines: Vec<String> = theirs[w.open + 1..w.close].iter().map(|l| l.to_string()).collect();
    let Some(target) = mine.iter().find(|t| t.name == w.name) else {
      out.notes.push(format!(
        "docker/Dockerfile: window `{}` is not in the template, so its {} line(s) are left out of the render.",
        w.name,
        lines.len()
      ));
      continue;
    };
    if !lines.is_empty() {
      out.details.push(format!("kept {} line(s) in window {}", lines.len(), w.name));
    }
    edits.push((target, lines));
  }
  // Back to front, so an earlier window's indices stay valid while a later one is replaced.
  edits.sort_by_key(|(t, _)| std::cmp::Reverse(t.open));
  for (t, lines) in edits {
    ours.splice(t.open + 1..t.close, lines);
  }
  out.text = ours.join("\n");
  out.text.push('\n');
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  const TPL: &str = "FROM a\n# !window builder:\n# !end builder\nFROM b\n# !window final:\n# !end final\nWORKDIR /app\n";

  #[test]
  fn scan_finds_windows_and_ignores_other_marker_like_lines() {
    let lines: Vec<&str> = TPL.lines().collect();
    assert_eq!(
      scan(&lines, "t").unwrap(),
      vec![
        Window {
          name: "builder".into(),
          open: 1,
          close: 2
        },
        Window {
          name: "final".into(),
          open: 4,
          close: 5
        }
      ]
    );
    // A project's own marker-looking comments, and an `!end` naming nothing open, are content.
    let odd = ["# !note to self", "# !window w:", "RUN a  # !end other", "# !!!", "# !end w"];
    assert_eq!(
      scan(&odd, "t").unwrap(),
      vec![Window {
        name: "w".into(),
        open: 1,
        close: 4
      }]
    );
    assert!(scan(&[], "t").unwrap().is_empty());
    for (bad, needle) in [
      (vec!["# !window w:", "RUN a"], "not closed"),
      (vec!["# !window w:", "# !window v:", "# !end v", "# !end w"], "still open"),
      (vec!["# !window w:", "# !end w", "# !window w:", "# !end w"], "twice"),
    ] {
      let e = scan(&bad, "t").unwrap_err().to_string();
      assert!(e.contains(needle) && e.contains("t:"), "{bad:?}: {e}");
    }
  }

  #[test]
  fn the_projects_lines_replace_the_templates_and_a_missing_window_is_noted() {
    let project = "FROM old\n# !window builder:\nRUN one\n# !end builder\nFROM older\n# !window final:\nRUN two\nRUN three\n# !end final\nWORKDIR /app\n";
    let s = splice(TPL, project).unwrap();
    assert_eq!(
      s.text,
      "FROM a\n# !window builder:\nRUN one\n# !end builder\nFROM b\n# !window final:\nRUN two\nRUN three\n# !end final\nWORKDIR /app\n"
    );
    assert_eq!(
      s.details,
      vec!["kept 1 line(s) in window builder", "kept 2 line(s) in window final"]
    );
    assert!(s.notes.is_empty());
    // No markers in the project's file: the template as rendered, its windows empty.
    let s = splice(TPL, "FROM old\n").unwrap();
    assert!(s.text == TPL && s.details.is_empty() && s.notes.is_empty());
    // Empty windows carry nothing and report nothing.
    let s = splice(TPL, TPL).unwrap();
    assert!(s.text == TPL && s.details.is_empty() && s.notes.is_empty());
    // Only the windows the project filled are touched; order in the file does not matter.
    let only_final = "# !window final:\nRUN two\n# !end final\n";
    assert_eq!(
      splice(TPL, only_final).unwrap().text,
      TPL.replace("# !window final:\n", "# !window final:\nRUN two\n")
    );
    // A window the template lacks goes with its lines, and a note says so (9.3).
    let s = splice(
      TPL,
      "# !window extra:\nRUN mine\nRUN more\n# !end extra\n# !window final:\nRUN two\n# !end final\n",
    )
    .unwrap();
    assert_eq!(s.text, TPL.replace("# !window final:\n", "# !window final:\nRUN two\n"));
    assert_eq!(s.details, vec!["kept 1 line(s) in window final"]);
    assert_eq!(s.notes.len(), 1);
    assert!(
      s.notes[0].contains("window `extra`") && s.notes[0].contains("2 line(s)"),
      "{}",
      s.notes[0]
    );
    // A malformed window in the project's file is still an error: the file cannot be read.
    assert!(splice(TPL, "# !window w:\nRUN a\n").is_err());
  }
}
