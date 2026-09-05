//! The compose scaffold: the fresh-file template split into head / one service block /
//! tail, plus the lazily resolved `{git_tag}` placeholder.

use std::cell::{OnceCell, RefCell};
use std::path::Path;

use anyhow::{Result, bail};

use aeth_devkit_core::github;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::version::{latest_stable_common, parse_lenient};

use crate::context::ProjectContext;
use crate::templates;

pub const BLOCK_START: &str = "# setup-project: service-block";
pub const BLOCK_END: &str = "# setup-project: end-service-block";

/// `head` + one `block` per service + `tail` is a complete compose file. The block still
/// carries `{service}` and `{git_tag}`; every other placeholder was substituted on load.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scaffold {
  pub head: String,
  pub block: String,
  pub tail: String,
}

/// Split an already substituted and gated template at its block markers.
pub fn parse(template: &str) -> Result<Scaffold> {
  let mut head = String::new();
  let mut block = String::new();
  let mut tail = String::new();
  // A tiny state machine over the marker lines; `Option<bool>` would read as "inside?"
  // but three destinations want three states, so an enum is clearer.
  enum Part {
    Head,
    Block,
    Tail,
  }
  let mut part = Part::Head;
  for line in template.lines() {
    match (line.trim(), &part) {
      (BLOCK_START, Part::Head) => part = Part::Block,
      (BLOCK_END, Part::Block) => part = Part::Tail,
      _ => {
        let dest = match part {
          Part::Head => &mut head,
          Part::Block => &mut block,
          Part::Tail => &mut tail,
        };
        dest.push_str(line);
        dest.push('\n');
      }
    }
  }
  if !matches!(part, Part::Tail) {
    bail!("compose template is missing the `{BLOCK_START}` / `{BLOCK_END}` markers");
  }
  Ok(Scaffold { head, block, tail })
}

/// Load, substitute, gate on aeth_ext, and split the compose template.
pub fn load(templates_dir: &Path, ctx: &ProjectContext) -> Result<Scaffold> {
  let raw = templates::load(templates_dir, "docker/compose.yaml", ctx, templates::Escape::None)?;
  let uses = ctx.uses_aeth_ext();
  parse(&templates::gate(&raw, &|name| name == "aeth-ext" && uses))
}

pub fn service_block(sc: &Scaffold, service: &str) -> String {
  sc.block.replace("{service}", service)
}

pub fn render_file(sc: &Scaffold, services: &[String]) -> String {
  let mut out = sc.head.clone();
  for s in services {
    out.push_str(&service_block(sc, s));
  }
  out.push_str(&sc.tail);
  out
}

/// `{git_tag}`, resolved at most once and only when something containing it is written.
/// A routine run never talks to `gh`.
pub struct GitTag<'a> {
  runner: &'a dyn Runner,
  ctx: &'a ProjectContext,
  // `OnceCell`: write-once through `&self`; `get_or_init` runs the closure the first time.
  resolved: OnceCell<String>,
  note: RefCell<Option<String>>,
}

impl<'a> GitTag<'a> {
  pub fn new(runner: &'a dyn Runner, ctx: &'a ProjectContext) -> Self {
    Self {
      runner,
      ctx,
      resolved: OnceCell::new(),
      note: RefCell::new(None),
    }
  }

  pub fn fill(&self, text: &str) -> String {
    if !text.contains("{git_tag}") {
      return text.to_string();
    }
    text.replace("{git_tag}", self.resolved.get_or_init(|| self.resolve()))
  }

  /// The advisory to print when the fallback was used, if it was.
  pub fn note(&self) -> Option<String> {
    self.note.borrow().clone()
  }

  fn resolve(&self) -> String {
    let fallback = format!("v{}", self.ctx.version.as_deref().unwrap_or("0.0.0"));
    // A closure so the three failure paths share one note; it only reads through the
    // `RefCell`, so plain `Fn` (no `mut`) is enough.
    let fall_back = |why: String| {
      *self.note.borrow_mut() = Some(format!(
        "GIT_TAG resolved to {fallback} from pyproject.toml ({why}); run `devkit docker-pin` after the next release."
      ));
      fallback.clone()
    };
    let Some(repo) = self.ctx.origin.as_deref().and_then(github::github_repo_path) else {
      return fall_back("origin is not a GitHub repository".into());
    };
    let tags = match github::list_tags(self.runner, &self.ctx.root, &repo) {
      Ok(t) => t,
      Err(e) => return fall_back(format!("{e:#}")),
    };
    // docker-pin's rule with a single source: highest stable version, written with the
    // remote's own spelling (`v1.5.0`, not the normalised `1.5.0`).
    let Some(latest) = latest_stable_common(std::slice::from_ref(&tags)) else {
      return fall_back("no stable tag on the remote".into());
    };
    tags
      .iter()
      .find(|t| parse_lenient(t).as_ref() == Some(&latest))
      .cloned()
      .unwrap_or_else(|| fall_back("no stable tag on the remote".into()))
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;
  use std::collections::HashSet;

  const TPL: &str = "services:\n# setup-project: service-block\n  {service}:\n    container_name: {service}\n    build:\n      args:\n        GIT_TAG: {git_tag}\n# setup-project: end-service-block\n\nnetworks:\n  coolify:\n    external: true\n";

  #[test]
  fn splits_and_renders_one_block_per_service() {
    let sc = parse(TPL).unwrap();
    assert_eq!(sc.head, "services:\n");
    assert!(sc.block.starts_with("  {service}:\n"));
    assert_eq!(sc.tail, "\nnetworks:\n  coolify:\n    external: true\n");
    let out = render_file(&sc, &["a".into(), "b".into()]);
    assert_eq!(
      out,
      "services:\n  a:\n    container_name: a\n    build:\n      args:\n        GIT_TAG: {git_tag}\n  b:\n    container_name: b\n    build:\n      args:\n        GIT_TAG: {git_tag}\n\nnetworks:\n  coolify:\n    external: true\n"
    );
    assert!(parse("services:\n").is_err(), "missing markers is a template bug");
  }

  fn ctx(root: &std::path::Path, origin: Option<&str>) -> ProjectContext {
    ProjectContext {
      root: root.to_path_buf(),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: true,
      python_dir: "src".into(),
      has_rust: false,
      has_container_crate: false,
      publish_index: None,
      name: "proj".into(),
      version: Some("1.2.3".into()),
      origin: origin.map(str::to_string),
      docker_services: vec!["proj".into()],
      docker_legacy_keys: vec![],
      docker_files: false,
      silence_unlisted_services_warning: false,
    }
  }

  #[test]
  fn git_tag_is_the_latest_stable_remote_tag_with_its_spelling() {
    let r = RecordingRunner::new(0);
    r.script("gh", &["api"], 0, "v2.0.0-alpha1\nv1.5.0\nv1.4.0\n");
    let c = ctx(std::path::Path::new("."), Some("https://github.com/o/r.git"));
    let tag = GitTag::new(&r, &c);
    assert_eq!(tag.fill("x: {git_tag}"), "x: v1.5.0");
    assert!(tag.note().is_none());
    assert_eq!(r.calls_for("gh").len(), 1, "resolved once");
    assert_eq!(tag.fill("no placeholder"), "no placeholder");
  }

  #[test]
  fn git_tag_is_lazy_and_falls_back_to_the_pyproject_version() {
    let r = RecordingRunner::new(1); // any gh call fails
    let c = ctx(std::path::Path::new("."), Some("https://github.com/o/r.git"));
    let tag = GitTag::new(&r, &c);
    assert!(r.calls_for("gh").is_empty(), "nothing resolved until needed");
    assert_eq!(tag.fill("{git_tag}"), "v1.2.3");
    assert!(tag.note().unwrap().contains("v1.2.3"));
    // Not a GitHub origin: no gh call at all.
    let r2 = RecordingRunner::new(0);
    let c2 = ctx(std::path::Path::new("."), None);
    assert_eq!(GitTag::new(&r2, &c2).fill("{git_tag}"), "v1.2.3");
    assert!(r2.calls_for("gh").is_empty());
  }
}
