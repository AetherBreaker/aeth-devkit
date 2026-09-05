//! Change collection and (optional) writing.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};

#[derive(Debug)]
pub struct FileChange {
  pub path: PathBuf,
  pub created: bool,
  pub details: Vec<String>,
}

#[derive(Debug)]
pub struct Changes {
  dry_run: bool,
  pub files: Vec<FileChange>,
  /// Every path devkit manages, whether or not this run changed it.
  ///
  /// `files` only holds what changed, which is the right list to report and to commit. It is
  /// the wrong list to decide what to un-ignore: a project that tightens its `.gitignore`
  /// *after* a successful run changes nothing on the next one, so the managed files would
  /// stay invisible to git forever.
  pub managed: Vec<PathBuf>,
  /// Advisory `note:` lines — things the user may want to clean up by hand. Never written.
  pub notes: Vec<String>,
  /// `warning:` lines (stderr): a managed file devkit deliberately left whole because the
  /// project's layout puts it out of reach, not because anything is wrong with it. Never
  /// written; `--check` passes, so a supported layout cannot fail a pipeline forever.
  pub warnings: Vec<String>,
  /// `problem:` lines: drift the run saw in a file it manages but could not edit (a compose
  /// shape the engine does not model). Never written, never committed; `--check` fails on
  /// them, since a listed service is a declared intent to have the file managed.
  pub problems: Vec<String>,
}

impl Changes {
  pub fn new(dry_run: bool) -> Self {
    Self {
      dry_run,
      files: Vec::new(),
      managed: Vec::new(),
      notes: Vec::new(),
      warnings: Vec::new(),
      problems: Vec::new(),
    }
  }

  pub fn is_empty(&self) -> bool {
    self.files.is_empty()
  }

  /// Record a change to an existing file. Writes unless dry-run; no-op if unchanged.
  pub fn record(&mut self, path: &Path, original: &str, merged: &str, details: Vec<String>) -> Result<()> {
    self.record_optional(path, Some(original), merged, details)
  }

  /// Record a change to a possibly-absent file.
  pub fn record_optional(&mut self, path: &Path, original: Option<&str>, merged: &str, details: Vec<String>) -> Result<()> {
    // Tracked even when nothing changed: this is the list of files devkit owns.
    if !self.managed.iter().any(|p| p == path) {
      self.managed.push(path.to_path_buf());
    }
    if original == Some(merged) {
      return Ok(());
    }
    let created = original.is_none();
    let details = if created && details.is_empty() {
      vec!["created".to_string()]
    } else {
      details
    };
    if !self.dry_run {
      if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
      }
      std::fs::write(path, merged).with_context(|| format!("writing {}", path.display()))?;
    }
    // One entry per path. Two steps can touch the same file — `.gitignore` is merged from the
    // template and then given un-ignore lines — and two entries would list it twice in the
    // report, twice in the commit body, and twice in the `git add` argument list.
    match self.files.iter_mut().find(|f| f.path == path) {
      Some(existing) => existing.details.extend(details),
      None => self.files.push(FileChange {
        path: path.to_path_buf(),
        created,
        details,
      }),
    }
    Ok(())
  }

  /// Record a change an external tool already wrote to disk (nothing is written here).
  /// Appends `detail` to an existing entry for `path`, or adds a new "updated" entry.
  pub fn note(&mut self, path: &Path, detail: &str) {
    if let Some(f) = self.files.iter_mut().find(|f| f.path == path) {
      f.details.push(detail.to_string());
    } else {
      self.files.push(FileChange {
        path: path.to_path_buf(),
        created: false,
        details: vec![detail.to_string()],
      });
    }
  }

  /// Human-readable report, one file per block.
  pub fn report(&self, root: &Path) -> String {
    let mut out = String::new();
    for f in &self.files {
      let rel = f.path.strip_prefix(root).unwrap_or(&f.path).to_string_lossy().replace('\\', "/");
      let verb = if f.created { "created" } else { "updated" };
      out.push_str(&format!("{rel}: {verb}\n"));
      for d in &f.details {
        if d != "created" {
          out.push_str(&format!("  - {d}\n"));
        }
      }
    }
    out
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn two_steps_touching_one_file_produce_one_entry() {
    // More than one step can legitimately write the same file. Two `FileChange` entries for
    // one path would list it twice in the report, twice in the commit body, and twice in the
    // `git add` argument list.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    let mut c = Changes::new(false);
    c.record_optional(&path, None, "one\n", vec!["created".into()]).unwrap();
    c.record_optional(&path, Some("one\n"), "one\ntwo\n", vec!["appended".into()])
      .unwrap();

    assert_eq!(c.files.len(), 1, "{:?}", c.files);
    // The first write created it, and that stays true however many steps touch it after.
    assert!(c.files[0].created);
    assert!(c.files[0].details.iter().any(|d| d == "appended"), "{:?}", c.files[0].details);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "one\ntwo\n");
  }

  #[test]
  fn an_unchanged_file_is_still_tracked_as_managed() {
    // `files` is what changed; `managed` is what devkit owns. The gitignore warning needs the
    // second list, or a project that tightens its rules after setup never hears about it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    let mut c = Changes::new(false);
    c.record_optional(&path, Some("same\n"), "same\n", vec![]).unwrap();
    assert!(c.files.is_empty(), "nothing changed: {:?}", c.files);
    assert_eq!(c.managed, vec![path]);
  }

  #[test]
  fn a_dry_run_records_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    let mut c = Changes::new(true);
    c.record_optional(&path, None, "content\n", vec![]).unwrap();
    assert_eq!(c.files.len(), 1);
    assert!(!path.exists(), "--dry-run must not write");
  }
}
