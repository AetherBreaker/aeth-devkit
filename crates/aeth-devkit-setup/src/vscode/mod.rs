//! VS Code integration for `setup-project`: an in-editor diff with per-hunk consent in
//! place of the typed terminal prompt, when the run happens inside a VS Code terminal.
//! `prepare` runs the detection/install/grant pipeline; the pure pieces live here.

pub mod protocol;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use protocol::EXTENSION_ID;

/// The `argv.json` key that grants proposed API contributions to a listed extension.
pub const ARGV_KEY: &str = "enable-proposed-api";

/// `code` on `PATH`. On Windows the launcher is a `.cmd` shim and `Command` does not
/// apply `PATHEXT`, so the candidates are spelled out (std runs `.cmd` through `cmd.exe`
/// with its own argument escaping, which is why the URL we pass carries no `%`).
pub fn find_launcher(path: &OsStr) -> Option<PathBuf> {
  let names: &[&str] = if cfg!(windows) { &["code.cmd", "code.exe", "code"] } else { &["code"] };
  std::env::split_paths(path)
    .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
    .find(|p| p.is_file())
}

pub fn home_dir() -> Option<PathBuf> {
  let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
  std::env::var_os(var).map(PathBuf::from)
}

/// `argv.json` with the extension added to `enable-proposed-api`, or `None` when it is
/// already listed. Text surgery rather than a parse-and-print round trip: the file is VS
/// Code's, full of comments the JSON merge would drop. `rfind` for the key because a
/// comment mentioning it sits above the real entry, never below.
pub fn grant_proposal(argv: Option<&str>) -> Result<Option<String>> {
  let entry = format!("\"{EXTENSION_ID}\"");
  let Some(text) = argv else {
    return Ok(Some(format!("{{\n\t\"{ARGV_KEY}\": [{entry}]\n}}\n")));
  };
  let doc: serde_json::Value = serde_json::from_str(&crate::json_merge::strip_jsonc(text)).context("parsing argv.json")?;
  match doc.get(ARGV_KEY) {
    Some(serde_json::Value::Array(items)) => {
      if items.iter().any(|v| v.as_str() == Some(EXTENSION_ID)) {
        return Ok(None);
      }
      let key_at = text.rfind(&format!("\"{ARGV_KEY}\"")).context("argv.json: key not found in the text")?;
      let open = key_at + text[key_at..].find('[').context("argv.json: array not found")? + 1;
      let rest = &text[open..];
      let sep = if rest.trim_start().starts_with(']') {
        ""
      } else if rest.starts_with(char::is_whitespace) {
        ","
      } else {
        ", "
      };
      Ok(Some(format!("{}{entry}{sep}{rest}", &text[..open])))
    }
    Some(_) => bail!("argv.json: `{ARGV_KEY}` is not an array"),
    None => {
      let brace = text.find('{').context("argv.json has no object")? + 1;
      let comma = if doc.as_object().is_some_and(|o| !o.is_empty()) { "," } else { "" };
      let indent = text
        .lines()
        .find(|l| l.trim_start().starts_with('"'))
        .map(|l| l[..l.len() - l.trim_start().len()].to_string())
        .unwrap_or_else(|| "\t".into());
      Ok(Some(format!("{}\n{indent}\"{ARGV_KEY}\": [{entry}]{comma}{}", &text[..brace], &text[brace..])))
    }
  }
}

/// Leftovers of the Drekker extension this one replaces. Reported, never removed: the
/// extensions-dir entry is a junction into a sister project's working tree.
pub fn stray_notes(home: &Path, project_root: &Path) -> Vec<String> {
  let mut notes = Vec::new();
  let ext_dir = home.join(".vscode").join("extensions");
  if let Ok(entries) = std::fs::read_dir(&ext_dir) {
    for e in entries.flatten() {
      let name = e.file_name().to_string_lossy().into_owned();
      if name.starts_with("local.drekker-add-to-runtime-base") {
        notes.push(format!(
          "{} is the old Drekker extension junction; the devkit extension replaces it, so delete the junction (not its target).",
          ext_dir.join(&name).display()
        ));
      }
    }
  }
  if project_root.join(".vscode").join("extension").is_dir() {
    notes.push(".vscode/extension/ is the old Drekker extension source; the devkit extension replaces it, so it can be deleted.".into());
  }
  notes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn finds_the_launcher_on_path_including_the_cmd_shim() {
    let dir = tempfile::tempdir().unwrap();
    let name = if cfg!(windows) { "code.cmd" } else { "code" };
    let empty = tempfile::tempdir().unwrap();
    let path = std::env::join_paths([empty.path(), dir.path()]).unwrap();
    assert_eq!(find_launcher(&path), None);
    std::fs::write(dir.path().join(name), "").unwrap();
    assert_eq!(find_launcher(&path), Some(dir.path().join(name)));
  }

  #[test]
  fn grant_creates_the_file_when_absent() {
    assert_eq!(
      grant_proposal(None).unwrap().unwrap(),
      "{\n\t\"enable-proposed-api\": [\"aeth.aeth-devkit\"]\n}\n"
    );
  }

  #[test]
  fn grant_inserts_the_key_after_the_brace_keeping_comments() {
    let argv = "// header\n{\n\t// Use software rendering.\n\t// \"disable-hardware-acceleration\": true,\n\t\"enable-crash-reporter\": true,\n\t\"crash-reporter-id\": \"x\"\n}\n";
    let out = grant_proposal(Some(argv)).unwrap().unwrap();
    assert_eq!(
      out,
      "// header\n{\n\t\"enable-proposed-api\": [\"aeth.aeth-devkit\"],\n\t// Use software rendering.\n\t// \"disable-hardware-acceleration\": true,\n\t\"enable-crash-reporter\": true,\n\t\"crash-reporter-id\": \"x\"\n}\n"
    );
    assert_eq!(grant_proposal(Some("{}")).unwrap().unwrap(), "{\n\t\"enable-proposed-api\": [\"aeth.aeth-devkit\"]}");
    assert_eq!(grant_proposal(Some(&out)).unwrap(), None, "second run: already granted");
  }

  #[test]
  fn grant_extends_an_existing_array() {
    assert_eq!(
      grant_proposal(Some("{\n  \"enable-proposed-api\": []\n}\n")).unwrap().unwrap(),
      "{\n  \"enable-proposed-api\": [\"aeth.aeth-devkit\"]\n}\n"
    );
    assert_eq!(
      grant_proposal(Some("{\"enable-proposed-api\": [\"other.ext\"]}")).unwrap().unwrap(),
      "{\"enable-proposed-api\": [\"aeth.aeth-devkit\", \"other.ext\"]}"
    );
    assert_eq!(
      grant_proposal(Some("{\n  \"enable-proposed-api\": [\n    \"other.ext\"\n  ]\n}\n")).unwrap().unwrap(),
      "{\n  \"enable-proposed-api\": [\"aeth.aeth-devkit\",\n    \"other.ext\"\n  ]\n}\n"
    );
    assert!(grant_proposal(Some("{\"enable-proposed-api\": true}")).is_err());
    assert!(grant_proposal(Some("not json")).is_err());
  }

  #[test]
  fn stray_notes_report_the_junction_and_the_project_folder() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    assert!(stray_notes(home.path(), root.path()).is_empty());
    std::fs::create_dir_all(home.path().join(".vscode/extensions/local.drekker-add-to-runtime-base-0.0.1")).unwrap();
    std::fs::create_dir_all(root.path().join(".vscode/extension")).unwrap();
    let notes = stray_notes(home.path(), root.path());
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("local.drekker-add-to-runtime-base-0.0.1") && notes[0].contains("junction"));
    assert!(notes[1].starts_with(".vscode/extension/"));
  }
}
