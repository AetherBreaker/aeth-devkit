//! What already exists for a version, and how to show it.
//!
//! Before a release mutates anything, the command probes every place a version can leak
//! to — local tag, remote tag, GitHub release, devpi — and shows the user a table. That
//! tells them at a glance whether they are looking at a single stray tag or a complete
//! earlier release, before they decide whether to type `force`.

/// The probe results. `Default` gives the all-`none` value.
///
/// The fields use different shapes on purpose: a local tag has a target commit worth
/// showing (`Option<String>`), a GitHub release has a URL (`Option<String>`), while
/// remote tag and devpi are simple yes/no (`bool`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Existing {
  pub local_tag: Option<String>,
  pub remote_tag: bool,
  pub github: Option<String>,
  pub devpi: bool,
}

impl Existing {
  /// Does anything exist at all? If not, no prompt is needed.
  pub fn any(&self) -> bool {
    self.local_tag.is_some() || self.remote_tag || self.github.is_some() || self.devpi
  }
}

/// Render the table shown to the user.
pub fn render(version: &str, ex: &Existing, package: &str, index_name: &str) -> String {
  let tag = format!("v{version}");
  // A closure that formats one row. `{label:<15}` left-aligns `label` in a 15-column
  // field so the values line up.
  let row = |label: &str, value: String| format!("  {label:<15} {value}\n");
  let mut s = format!("Existing artefacts for {tag}:\n");
  // `+=` on a `String` appends; `&row(…)` borrows the temporary `String` as `&str`.
  // `map_or(default, f)` on an `Option`: use `f(value)` if `Some`, else `default`.
  s += &row(
    "local tag",
    ex.local_tag.as_ref().map_or("none".into(), |sha| format!("{tag} -> {sha}")),
  );
  s += &row(
    "remote tag",
    if ex.remote_tag {
      format!("refs/tags/{tag} on origin")
    } else {
      "none".into()
    },
  );
  s += &row("GitHub release", ex.github.clone().unwrap_or_else(|| "none".into()));
  s += &row(
    "devpi",
    if ex.devpi {
      format!("{package}=={version} on {index_name}")
    } else {
      "none".into()
    },
  );
  s
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn renders_none_and_present() {
    let none = Existing::default();
    assert!(!none.any());
    let s = render("1.2.3", &none, "demo", "SFTPyPI");
    assert!(s.contains("local tag       none") && s.contains("devpi           none"), "{s}");
    let all = Existing {
      local_tag: Some("abc1234".into()),
      remote_tag: true,
      github: Some("https://gh/r/v1.2.3".into()),
      devpi: true,
    };
    assert!(all.any());
    let s = render("1.2.3", &all, "demo", "SFTPyPI");
    assert!(s.contains("v1.2.3 -> abc1234"));
    assert!(s.contains("remote tag      refs/tags/v1.2.3 on origin"));
    assert!(s.contains("https://gh/r/v1.2.3"));
    assert!(s.contains("demo==1.2.3 on SFTPyPI"));
  }
}
