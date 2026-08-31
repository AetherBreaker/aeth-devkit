//! Choosing a release version from an index listing.

use std::collections::BTreeSet;

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

/// Parse a version string the way humans write them: surrounding whitespace and a leading
/// `v`/`V` (tag convention) are ignored. `None` for anything PEP 440 cannot read.
pub fn parse_lenient(s: &str) -> Option<Version> {
  let t = s.trim();
  t.strip_prefix(['v', 'V']).unwrap_or(t).parse().ok()
}

/// A final release: no pre/post/dev/local segments.
pub fn is_stable(v: &Version) -> bool {
  !v.any_prerelease() && !v.is_post() && !v.is_local()
}

/// Whether `want` appears in `versions`, comparing parsed values so `v1.2.0a1`,
/// `1.2.0-alpha1`, and `1.2.0a1` all count as the same version.
pub fn contains<'a>(versions: impl IntoIterator<Item = &'a str>, want: &Version) -> bool {
  versions.into_iter().filter_map(parse_lenient).any(|v| v == *want)
}

/// The highest *stable* version present in every one of `lists` (an intersection, so a
/// version only half-published across indexes is never chosen). `None` when `lists` is
/// empty or nothing stable is common to all.
pub fn latest_stable_common(lists: &[Vec<String>]) -> Option<Version> {
  let mut sets = lists
    .iter()
    .map(|l| l.iter().filter_map(|s| parse_lenient(s)).collect::<BTreeSet<Version>>());
  let first = sets.next()?;
  let common = sets.fold(first, |acc, s| acc.intersection(&s).cloned().collect());
  common.into_iter().filter(is_stable).max()
}

#[cfg(test)]
mod tests {
  use super::*;

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

  #[test]
  fn parse_lenient_strips_v_and_normalizes() {
    assert_eq!(parse_lenient(" v1.2.0-alpha1 ").unwrap().to_string(), "1.2.0a1");
    assert_eq!(parse_lenient("1.2.3").unwrap().to_string(), "1.2.3");
    assert!(parse_lenient("junk").is_none());
  }

  #[test]
  fn contains_compares_parsed_values() {
    let want = parse_lenient("1.2.0a1").unwrap();
    assert!(contains(["v1.2.0-alpha1", "0.9"], &want));
    assert!(!contains(["1.2.0"], &want));
  }

  #[test]
  fn latest_stable_common_intersects() {
    let a: Vec<String> = vec!["v1.0.0".into(), "v2.0.0".into(), "v3.0.0a1".into()];
    let b: Vec<String> = vec!["1.0.0".into(), "2.0.0".into(), "3.0.0a1".into()];
    let c: Vec<String> = vec!["1.0.0".into(), "3.0.0a1".into()];
    assert_eq!(latest_stable_common(&[a.clone(), b.clone()]).unwrap().to_string(), "2.0.0");
    assert_eq!(latest_stable_common(&[a, b, c]).unwrap().to_string(), "1.0.0");
    assert_eq!(latest_stable_common(&[]), None);
    assert_eq!(latest_stable_common(&[vec!["1.0a1".into()]]), None);
  }
}
