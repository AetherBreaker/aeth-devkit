//! Repair a shim that is older than the binary answering it.
//!
//! An installed shim lives in the user's profile indefinitely, so it will eventually be
//! older than the devkit it calls. Rather than making that the user's problem, a request
//! carrying a stale [`crate::scripts::SHIM_VERSION`] rewrites the shim file in place.
//!
//! Two properties matter, and both come from the same constraint: a shell that is *already
//! open* holds the old shim in memory and will report the old version on **every** Tab press
//! for the life of that shell.
//!
//! - The repair must be **idempotent** — once the file on disk is current, later presses
//!   must not rewrite it, or every Tab would hit the disk.
//! - The request is still answered. Refusing would kill completion in every open shell until
//!   each was restarted, even for a cosmetic shim change.

use std::path::Path;

use crate::engine::Shell;
use crate::install;
use crate::scripts;

/// Replace `path`'s contents with `script`, atomically, if they differ.
///
/// Returns whether a write happened. The temp file is created beside the target rather than
/// in the system temp directory, because `rename` is only atomic within one filesystem.
fn rewrite_if_different(path: &Path, script: &str) -> bool {
  // An unreadable or missing file counts as "differs": missing certainly does, and if it
  // cannot be read there is no way to conclude it is already correct. `io::Error` is not
  // comparable, so the Result is narrowed to an Option before the comparison.
  if std::fs::read_to_string(path).ok().as_deref() == Some(script) {
    return false;
  }

  let Some(dir) = path.parent() else {
    return false;
  };
  if std::fs::create_dir_all(dir).is_err() {
    return false;
  }

  // A fixed suffix rather than a random name: two concurrent repairs write identical bytes,
  // so the worst case is one clobbering the other with the same content.
  let temp = path.with_extension("tmp-devkit");
  if std::fs::write(&temp, script).is_err() {
    return false;
  }

  // `rename` over an existing file is atomic on both Windows and Unix, so a concurrent Tab
  // press in another shell sees either the whole old file or the whole new one, never a
  // half-written script.
  if std::fs::rename(&temp, path).is_err() {
    // Leaving a stray temp file behind would be untidy; failure to clean up is not worth
    // reporting, hence the discarded result.
    let _ = std::fs::remove_file(&temp);
    return false;
  }
  true
}

/// Bring the installed shim for `shell` up to date if the request came from an old one.
///
/// Returns whether anything was written. Every failure is swallowed: this runs inside a Tab
/// press, where an error would print over the user's prompt.
pub fn repair_if_stale(home: &Path, shell: Shell, sent_version: u32) -> bool {
  // The common case by far: the shim is current, so there is nothing to check on disk.
  if sent_version == scripts::SHIM_VERSION {
    return false;
  }

  match shell {
    Shell::PowerShell => rewrite_if_different(&install::powershell_shim_path(home), scripts::POWERSHELL),
    Shell::Bash => {
      // bash installs the same script to two locations, because which one the shell loads
      // depends on which loader it has. Both must be repaired.
      //
      // Written as a loop on purpose. The obvious `.any(|t| rewrite_if_different(t, BASH))`
      // is wrong: `any` short-circuits, so the second file would stay stale forever
      // whenever the first one needed writing. Every target is visited unconditionally and
      // the flag is accumulated afterwards.
      let mut written = false;
      for target in install::bash_targets(home) {
        if rewrite_if_different(&target, scripts::BASH) {
          written = true;
        }
      }
      written
    }
  }
}
