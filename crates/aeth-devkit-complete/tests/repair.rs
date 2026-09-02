//! Drift repair: an old shim asking a new binary gets rewritten for the next shell.

use std::path::Path;

use aeth_devkit_complete::engine::Shell;
use aeth_devkit_complete::install::{bash_targets, powershell_shim_path};
use aeth_devkit_complete::repair::repair_if_stale;
use aeth_devkit_complete::scripts::{BASH, POWERSHELL, SHIM_VERSION};

/// The version an already-open shell would report after devkit was upgraded under it.
const OLD: u32 = SHIM_VERSION - 1;

fn write(path: &Path, text: &str) {
  std::fs::create_dir_all(path.parent().unwrap()).unwrap();
  std::fs::write(path, text).unwrap();
}

#[test]
fn a_matching_version_writes_nothing() {
  let home = tempfile::tempdir().unwrap();
  write(&powershell_shim_path(home.path()), "# stale content\n");
  assert!(!repair_if_stale(home.path(), Shell::PowerShell, SHIM_VERSION));
  // Even though the file is wrong, a current version means we never look at it.
  assert_eq!(
    std::fs::read_to_string(powershell_shim_path(home.path())).unwrap(),
    "# stale content\n"
  );
}

#[test]
fn a_stale_version_with_outdated_text_rewrites_the_file() {
  let home = tempfile::tempdir().unwrap();
  write(&powershell_shim_path(home.path()), "# ancient shim\n");
  assert!(repair_if_stale(home.path(), Shell::PowerShell, OLD));
  assert_eq!(std::fs::read_to_string(powershell_shim_path(home.path())).unwrap(), POWERSHELL);
}

#[test]
fn a_stale_version_whose_file_is_already_current_writes_nothing() {
  // The case that makes idempotence essential: an open shell holds the old shim in memory
  // and reports the old version on EVERY press, long after the file was fixed.
  let home = tempfile::tempdir().unwrap();
  write(&powershell_shim_path(home.path()), POWERSHELL);
  assert!(!repair_if_stale(home.path(), Shell::PowerShell, OLD));
}

#[test]
fn a_missing_shim_file_is_created() {
  let home = tempfile::tempdir().unwrap();
  assert!(repair_if_stale(home.path(), Shell::PowerShell, OLD));
  assert_eq!(std::fs::read_to_string(powershell_shim_path(home.path())).unwrap(), POWERSHELL);
}

#[test]
fn bash_repairs_every_target_not_just_the_first() {
  // Short-circuiting here would leave the second loader's copy stale forever.
  let home = tempfile::tempdir().unwrap();
  for target in bash_targets(home.path()) {
    write(&target, "# ancient shim\n");
  }
  assert!(repair_if_stale(home.path(), Shell::Bash, OLD));
  for target in bash_targets(home.path()) {
    assert_eq!(std::fs::read_to_string(&target).unwrap(), BASH, "{}", target.display());
  }
}

#[test]
fn no_temp_file_is_left_behind() {
  let home = tempfile::tempdir().unwrap();
  repair_if_stale(home.path(), Shell::PowerShell, OLD);
  let shim = powershell_shim_path(home.path());
  let leftovers: Vec<_> = std::fs::read_dir(shim.parent().unwrap())
    .unwrap()
    .filter_map(Result::ok)
    .map(|e| e.file_name().to_string_lossy().into_owned())
    .filter(|n| n.contains("tmp"))
    .collect();
  assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn an_unwritable_target_is_swallowed_rather_than_erroring() {
  // The shim's parent path is an ordinary file, so creating the directory must fail. A Tab
  // press has nowhere to report that, so the only correct behaviour is to give up quietly.
  let home = tempfile::tempdir().unwrap();
  let blocker = home.path().join(".local");
  std::fs::write(&blocker, "not a directory").unwrap();
  assert!(!repair_if_stale(home.path(), Shell::PowerShell, OLD));
}
