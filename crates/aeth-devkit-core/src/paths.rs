//! Path helpers shared by the commands.

use std::path::PathBuf;

/// `\\?\D:\foo` → `D:\foo` (Windows `canonicalize` adds the verbatim prefix, which confuses
/// tools that receive the path as a string).
pub fn strip_verbatim(p: PathBuf) -> PathBuf {
  let s = p.to_string_lossy();
  match s.strip_prefix(r"\\?\") {
    Some(rest) => PathBuf::from(rest),
    None => p,
  }
}

#[cfg(test)]
mod tests {
  use super::strip_verbatim;
  use std::path::PathBuf;

  #[test]
  fn strips_only_the_verbatim_prefix() {
    assert_eq!(strip_verbatim(PathBuf::from(r"\\?\D:\proj")), PathBuf::from(r"D:\proj"));
    assert_eq!(strip_verbatim(PathBuf::from(r"D:\proj")), PathBuf::from(r"D:\proj"));
    assert_eq!(strip_verbatim(PathBuf::from("/home/x")), PathBuf::from("/home/x"));
  }
}
