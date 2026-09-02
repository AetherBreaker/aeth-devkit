//! Contract tests for the shell shims.
//!
//! The shims cannot be driven from a Rust test without spawning real shells, which is a
//! non-goal. What *is* testable is the contract: that they send the version the binary
//! speaks, that they never invoke devkit at load time, and that their text cannot change
//! without the version stamp changing with it.

use aeth_devkit_complete::scripts::{BASH, POWERSHELL, SHIM_VERSION};

/// FNV-1a, 64-bit. A hand-rolled hash rather than a dependency: this is a change detector,
/// not a security primitive, and pinning the algorithm here means the expected constants
/// below cannot shift under us when a crate updates.
fn fnv1a(text: &str) -> u64 {
  // `wrapping_mul` because overflow is the point of the algorithm, and a plain `*` would
  // panic in debug builds the moment the hash exceeded u64.
  let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
  for byte in text.as_bytes() {
    hash ^= u64::from(*byte);
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
  }
  hash
}

#[test]
fn both_shims_send_the_version_the_binary_speaks() {
  let stamp = format!("--shim-version {SHIM_VERSION}");
  assert!(BASH.contains(&stamp), "bash shim must send the current version");
  assert!(POWERSHELL.contains(&stamp), "powershell shim must send it too");
}

#[test]
fn shim_text_is_pinned_to_its_version() {
  // The guard that makes the version discipline mechanical instead of aspirational: edit
  // either shim and this fails, forcing a deliberate decision about SHIM_VERSION rather
  // than letting an installed shim silently differ from the shipped one.
  assert_eq!(
    (fnv1a(BASH), fnv1a(POWERSHELL)),
    (0xbee4_da6e_4061_9b6a, 0x40ca_fe5e_8fc5_6d3e),
    "shim text changed: bump SHIM_VERSION (and the shim's own header comment), then update these hashes"
  );
}

#[test]
fn neither_shim_evaluates_fetched_script_text() {
  // The original defect: `devkit complete script | Invoke-Expression` in $PROFILE ran devkit
  // at every shell start, which is what made a global install mandatory.
  for line in POWERSHELL.lines().filter(|l| !l.trim_start().starts_with('#')) {
    assert!(!line.contains("Invoke-Expression"), "shim must not evaluate fetched text: {line}");
  }
}

#[test]
fn both_shims_tolerate_devkit_being_absent() {
  // An unactivated venv is a normal state, not an error state.
  assert!(BASH.contains("command -v devkit"), "bash shim must guard on devkit's presence");
  assert!(POWERSHELL.contains("Get-Command devkit"), "powershell shim must guard likewise");
}

#[test]
fn both_shims_handle_every_directive_the_wire_format_can_send() {
  for (name, text) in [("bash", BASH), ("powershell", POWERSHELL)] {
    for directive in ["items", "dirs", "files"] {
      assert!(text.contains(directive), "{name} shim ignores the {directive} directive");
    }
  }
}

#[test]
fn the_powershell_shim_passes_an_empty_word_as_one_token() {
  // `--word-to-complete $w` with an empty $w can be dropped entirely by PowerShell's native
  // argument passing, which would make clap swallow the following token as its value.
  assert!(
    POWERSHELL.contains("--word-to-complete=$wordToComplete"),
    "must use the =value form so an empty word survives as a single token"
  );
}
