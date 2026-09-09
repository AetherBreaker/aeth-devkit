//! Step 15 of setup-project: `devkit-complete install` for the shells this machine has, so
//! poe completion comes from the venv's binary without anyone typing the command. The binary
//! is the one the package step installed (step 1b); the shells are whatever `PATH` can start.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use aeth_devkit_core::process::Runner;

use crate::changes::Changes;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Shells {
  pub powershell: bool,
  pub bash: bool,
}

/// Which shells `PATH` can start: Windows PowerShell or `pwsh.exe` counts as PowerShell (the
/// installer resolves `$PROFILE` through `powershell`, so a Linux `pwsh` would only make the
/// whole install fail), any `bash` counts (Git Bash on Windows, the login shell elsewhere).
pub fn shells_on(path: &OsStr) -> Shells {
  let has = |names: &[&str]| std::env::split_paths(path).any(|d| names.iter().any(|n| d.join(n).is_file()));
  Shells {
    powershell: has(&["powershell.exe", "pwsh.exe"]),
    bash: has(&["bash.exe", "bash"]),
  }
}

/// The `devkit-complete` in the environment the package step syncs.
pub fn binary(root: &Path) -> Option<PathBuf> {
  let env = crate::packages::environment(root);
  ["Scripts/devkit-complete.exe", "bin/devkit-complete"]
    .iter()
    .map(|rel| env.join(rel))
    .find(|p| p.is_file())
}

/// Run the install for `shells`; `--dry-run` on a dry run, since it writes to the home
/// directory rather than the project. The installer's "  - …" lines become notes (worded as
/// what a plain run would do on a dry run, since the installer's lines read as done either
/// way), and its "Nothing to do" is silence, so a routine run says nothing about completion.
pub fn install(root: &Path, binary: &Path, shells: Shells, runner: &dyn Runner, dry_run: bool, changes: &mut Changes) {
  if shells == Shells::default() {
    return;
  }
  let mut args: Vec<String> = vec!["install".into()];
  if shells.powershell {
    args.push("--powershell".into());
  }
  if shells.bash {
    args.push("--bash".into());
  }
  if dry_run {
    args.push("--dry-run".into());
  }
  match runner.run_capture(&binary.to_string_lossy(), &args, root) {
    Ok(out) if out.success() => {
      let prefix = if dry_run {
        "shell completion would change"
      } else {
        "shell completion"
      };
      let mut changed = false;
      for line in out.stdout.lines().filter_map(|l| l.strip_prefix("  - ")) {
        // A file the installer declined to overwrite ("left … alone") is listed beside
        // what it wrote: standing advice, not a change to open a new shell for.
        changed |= !line.starts_with("left ");
        changes.notes.push(format!("{prefix}: {line}"));
      }
      if !dry_run && changed {
        changes
          .notes
          .push("shell completion changed: open a new shell for it to take effect".into());
      }
    }
    Ok(out) => changes
      .warnings
      .push(format!("devkit-complete install failed: {}", out.stderr.trim())),
    Err(e) => changes.warnings.push(format!("devkit-complete install could not run: {e:#}")),
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;

  #[test]
  fn shells_are_found_on_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = std::env::join_paths([dir.path()]).unwrap();
    assert_eq!(shells_on(&path), Shells::default());
    std::fs::write(dir.path().join("bash"), "").unwrap();
    assert_eq!(
      shells_on(&path),
      Shells {
        powershell: false,
        bash: true
      }
    );
    std::fs::write(dir.path().join("pwsh"), "").unwrap();
    assert_eq!(
      shells_on(&path),
      Shells {
        powershell: false,
        bash: true
      },
      "a Linux pwsh is not one the installer can configure"
    );
    std::fs::write(dir.path().join("pwsh.exe"), "").unwrap();
    assert_eq!(
      shells_on(&path),
      Shells {
        powershell: true,
        bash: true
      }
    );
  }

  #[test]
  fn the_binary_is_the_venvs() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(binary(dir.path()), None);
    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("devkit-complete"), "").unwrap();
    assert_eq!(binary(dir.path()), Some(bin.join("devkit-complete")));
  }

  #[test]
  fn install_reports_changes_and_stays_quiet_when_there_are_none() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("devkit-complete");
    let shells = Shells {
      powershell: true,
      bash: true,
    };
    let r = RecordingRunner::new(0);
    r.script(
      &bin.to_string_lossy(),
      &["install"],
      0,
      "Changed:\n  - created C:/home/.local/share/devkit/poe-completion.ps1\n  - added: $c = …\nOpen a new shell for it to take effect.\n",
    );
    let mut changes = Changes::new(false);
    install(root.path(), &bin, shells, &r, false, &mut changes);
    assert_eq!(r.calls_for(&bin.to_string_lossy())[0], vec!["install", "--powershell", "--bash"]);
    assert_eq!(
      changes.notes,
      vec![
        "shell completion: created C:/home/.local/share/devkit/poe-completion.ps1",
        "shell completion: added: $c = …",
        "shell completion changed: open a new shell for it to take effect"
      ]
    );

    let r = RecordingRunner::new(0);
    r.script(
      &bin.to_string_lossy(),
      &["install"],
      0,
      "Nothing to do — completion is already installed.\n",
    );
    let mut changes = Changes::new(true);
    install(
      root.path(),
      &bin,
      Shells {
        powershell: false,
        bash: true,
      },
      &r,
      true,
      &mut changes,
    );
    assert_eq!(r.calls_for(&bin.to_string_lossy())[0], vec!["install", "--bash", "--dry-run"]);
    assert!(changes.notes.is_empty(), "{:?}", changes.notes);

    let r = RecordingRunner::new(0);
    r.script(
      &bin.to_string_lossy(),
      &["install"],
      0,
      "Would change:\n  - created C:/home/bash_completion.d/poe.bash\n",
    );
    let mut changes = Changes::new(true);
    install(root.path(), &bin, shells, &r, true, &mut changes);
    assert_eq!(
      changes.notes,
      vec!["shell completion would change: created C:/home/bash_completion.d/poe.bash"]
    );

    let r = RecordingRunner::new(0);
    r.script(
      &bin.to_string_lossy(),
      &["install"],
      0,
      "Changed:\n  - left C:/home/bash_completion.d/poe.bash alone: not a generated file (remove it by hand to replace it)\n",
    );
    let mut changes = Changes::new(false);
    install(root.path(), &bin, shells, &r, false, &mut changes);
    assert_eq!(
      changes.notes.len(),
      1,
      "a declined file is advice, not a change: {:?}",
      changes.notes
    );

    let r = RecordingRunner::new(0);
    let mut changes = Changes::new(false);
    install(root.path(), &bin, Shells::default(), &r, false, &mut changes);
    assert!(r.calls.borrow().is_empty(), "no shell, no call");

    let r = RecordingRunner::new(1);
    let mut changes = Changes::new(false);
    install(root.path(), &bin, shells, &r, false, &mut changes);
    assert_eq!(changes.warnings.len(), 1, "{:?}", changes.warnings);
    assert!(
      changes.warnings[0].starts_with("devkit-complete install failed"),
      "{:?}",
      changes.warnings
    );
  }
}
