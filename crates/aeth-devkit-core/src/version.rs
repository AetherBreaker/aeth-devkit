//! Choosing a release version from an index listing.

use pep440_rs::Version;

/// The highest final release among `versions` (no pre/dev/post/local segments), rendered
/// in normalized PEP 440 form. Unparseable strings are ignored.
pub fn latest_stable<'a>(versions: impl IntoIterator<Item = &'a str>) -> Option<String> {
  versions
    .into_iter()
    .filter_map(|s| s.parse::<Version>().ok())
    .filter(|v| !v.any_prerelease() && !v.is_post() && !v.is_local())
    .max()
    .map(|v| v.to_string())
}

#[cfg(test)]
mod tests {
  use super::latest_stable;

  #[test]
  fn picks_highest_final_release() {
    let v = ["6.0.2", "7.0.0a1", "6.10.0", "6.9.9.post1", "6.10.0.dev3", "6.3.0+local", "junk"];
    assert_eq!(latest_stable(v).as_deref(), Some("6.10.0"));
  }

  #[test]
  fn none_when_only_prereleases() {
    assert_eq!(latest_stable(["1.0a1", "1.0rc1"]), None);
    assert_eq!(latest_stable([]), None);
  }
}
