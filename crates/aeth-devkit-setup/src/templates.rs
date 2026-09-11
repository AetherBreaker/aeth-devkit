//! Loading templates, with placeholder substitution, and the override that renders a
//! working tree instead of the environment's package.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use crate::context::ProjectContext;
use crate::gate::{Format, Gates};

/// How placeholder values must be escaped for the file type they are inserted into.
#[derive(Debug, Clone, Copy)]
pub enum Escape {
  None,
  Toml,
  Json,
}

/// Map a target file name to its template file name. Templates keep a real extension so
/// editors provide highlighting, but never use the exact name a tool would key on:
/// `pyproject.toml` → `pyproject.template.toml`, `vscode/settings.json` →
/// `vscode/settings.template.jsonc` (comments allowed), `gitignore` → `template.gitignore`.
pub fn template_file_name(target: &str) -> String {
  let (dir, file) = match target.rsplit_once('/') {
    Some((d, f)) => (format!("{d}/"), f),
    None => (String::new(), target),
  };
  match file.rsplit_once('.') {
    Some((stem, "json")) => format!("{dir}{stem}.template.jsonc"),
    Some((stem, ext)) => format!("{dir}{stem}.template.{ext}"),
    None => format!("{dir}template.{file}"),
  }
}

/// Read a template (by its target name, e.g. `pyproject.toml`), render its gates, then
/// substitute the placeholders (see [`substitute`]).
pub fn load(templates_dir: &Path, name: &str, ctx: &ProjectContext, escape: Escape, gates: &Gates) -> Result<String> {
  let path = templates_dir.join(template_file_name(name));
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading template {}", path.display()))?;
  let gated = gates.apply(&text, Format::for_target(name), name)?;
  Ok(substitute(&gated, ctx, escape))
}

/// Like [`load`], but returns `None` when the template file does not exist (used for
/// optional overlays such as `vscode/extensions.rust.json`).
pub fn load_optional(templates_dir: &Path, name: &str, ctx: &ProjectContext, escape: Escape, gates: &Gates) -> Result<Option<String>> {
  let path = templates_dir.join(template_file_name(name));
  if !path.is_file() {
    return Ok(None);
  }
  load(templates_dir, name, ctx, escape, gates).map(Some)
}

/// Substitute `{project_root}` / `{package}` / `{python_dir}` / `{hook_bin}` /
/// `{publish_index}` / `{publish_index_key}` / `{devkit_index}` / `{git_repo}`. `{git_tag}`
/// and `{service}` are deliberately left in place for the Docker scaffold, which fills them
/// per block, and `{latest}` for the pyproject merger.
pub fn substitute(text: &str, ctx: &ProjectContext, escape: Escape) -> String {
  let root = ctx.root.to_string_lossy();
  let esc = |s: &str| -> String {
    match escape {
      Escape::None => s.to_string(),
      Escape::Toml | Escape::Json => s.replace('\\', "\\\\").replace('"', "\\\""),
    }
  };
  text
    .replace("{project_root}", &esc(&root))
    .replace("{package}", &esc(&ctx.package))
    .replace("{python_dir}", &esc(&ctx.python_dir))
    .replace("{hook_bin}", &esc(&hook_bin(&ctx.root)))
    .replace("{publish_index}", &esc(ctx.publish_index.as_deref().unwrap_or("")))
    .replace(
      "{publish_index_key}",
      &esc(
        &ctx
          .publish_index
          .as_deref()
          .map(aeth_devkit_core::pyproject::index_env_key)
          .unwrap_or_default(),
      ),
    )
    .replace("{devkit_index}", &esc(&ctx.devkit_index))
    .replace("{git_repo}", &esc(&git_repo(ctx)))
}

/// The value compose files carry in `GIT_REPO`: a GitHub origin normalised to
/// `https://github.com/<owner>/<repo>.git` (owner/repo case kept), any other origin as
/// written, and empty when there is no origin (the Repo rule then skips itself).
pub fn git_repo(ctx: &ProjectContext) -> String {
  let Some(origin) = ctx.origin.as_deref() else {
    return String::new();
  };
  match aeth_devkit_core::github::github_repo_path(origin) {
    Some(path) => format!("https://github.com/{path}.git"),
    None => origin.to_string(),
  }
}

/// How a hook line invokes `devkit-hook`: the project environment's own console script when
/// one exists (quoted; via `$CLAUDE_PROJECT_DIR` when the environment is inside the project,
/// so the file stays valid if the repo moves), else `uv run devkit-hook`. The direct path
/// skips `uv run`'s ~140 ms environment check on every hook invocation.
fn hook_bin(root: &Path) -> String {
  let env = crate::packages::environment(root);
  for rel in ["Scripts/devkit-hook.exe", "bin/devkit-hook"] {
    let bin = env.join(rel);
    if bin.is_file() {
      let shown = match bin.strip_prefix(root) {
        Ok(inside) => format!("$CLAUDE_PROJECT_DIR/{}", inside.display()),
        Err(_) => bin.display().to_string(),
      };
      return format!("\"{}\"", shown.replace('\\', "/"));
    }
  }
  "uv run devkit-hook".to_string()
}

/// The directory that renders instead of the environment's `devkit_templates`, if any:
/// `--templates-dir`, else `DEVKIT_TEMPLATES`, else `[tool.devkit].templates-dir` (joined at
/// discovery). Each must exist, and is a working tree: a checkout beside the project, the
/// templates repository itself, or its CI. `None` means the package in the venv, which
/// `packages::ensure_templates` installs first when the project lacks it.
pub fn override_dir(explicit: Option<&Path>, ctx: &ProjectContext) -> Result<Option<PathBuf>> {
  if let Some(p) = explicit {
    return existing_dir(p.to_path_buf(), "--templates-dir").map(Some);
  }
  if let Ok(p) = std::env::var("DEVKIT_TEMPLATES") {
    return existing_dir(PathBuf::from(p), "DEVKIT_TEMPLATES").map(Some);
  }
  ctx
    .templates_dir
    .clone()
    .map(|p| existing_dir(p, "[tool.devkit].templates-dir"))
    .transpose()
}

fn existing_dir(p: PathBuf, what: &str) -> Result<PathBuf> {
  if p.is_dir() {
    Ok(p)
  } else {
    bail!("{what}: {} is not a directory", p.display())
  }
}

#[cfg(test)]
mod tests {
  use super::template_file_name;

  #[test]
  fn template_names() {
    assert_eq!(template_file_name("pyproject.toml"), "pyproject.template.toml");
    assert_eq!(template_file_name("vscode/settings.json"), "vscode/settings.template.jsonc");
    assert_eq!(template_file_name("gitignore"), "template.gitignore");
    assert_eq!(template_file_name("env"), "template.env");
  }
}

#[cfg(test)]
mod hook_bin_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(root: &Path) -> ProjectContext {
    ProjectContext {
      root: root.to_path_buf(),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: false,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec![],
      docker_legacy_keys: vec![],
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
      templates_dir: None,
    }
  }

  #[test]
  fn hook_bin_falls_back_to_uv_run_without_a_venv() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
      substitute("{hook_bin} pre-edit-protect", &ctx(dir.path()), Escape::None),
      "uv run devkit-hook pre-edit-protect"
    );
  }

  #[test]
  fn hook_bin_uses_the_venv_script_quoted_and_json_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("devkit-hook"), "").unwrap();
    let out = substitute(r#""cmd": "{hook_bin} stop-ruff""#, &ctx(dir.path()), Escape::Json);
    assert_eq!(out, r#""cmd": "\"$CLAUDE_PROJECT_DIR/.venv/bin/devkit-hook\" stop-ruff""#);
  }
}

#[cfg(test)]
mod override_dir_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(root: &Path, templates_dir: Option<PathBuf>) -> ProjectContext {
    ProjectContext {
      root: root.to_path_buf(),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: false,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec![],
      docker_legacy_keys: vec![],
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
      templates_dir,
    }
  }

  #[test]
  fn the_flag_wins_over_the_pyproject_setting_and_must_be_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    assert_eq!(override_dir(Some(&b), &ctx(dir.path(), Some(a.clone()))).unwrap(), Some(b));
    let missing = dir.path().join("missing");
    let err = override_dir(Some(&missing), &ctx(dir.path(), None)).unwrap_err().to_string();
    assert!(err.contains("--templates-dir") && err.contains("missing"), "{err}");
    // The env branch sits between the two and is the process environment: a unit test
    // setting it would race the rest of this binary, so it is covered at binary level
    // (tests/apply.rs). Here, whatever the environment holds is what must win or yield.
    match std::env::var_os("DEVKIT_TEMPLATES") {
      Some(env) => {
        let env = PathBuf::from(env);
        assert_eq!(override_dir(None, &ctx(dir.path(), Some(a.clone()))).unwrap(), Some(env));
      }
      None => {
        assert_eq!(override_dir(None, &ctx(dir.path(), Some(a.clone()))).unwrap(), Some(a));
        assert_eq!(override_dir(None, &ctx(dir.path(), None)).unwrap(), None);
        let err = override_dir(None, &ctx(dir.path(), Some(missing.clone()))).unwrap_err().to_string();
        assert!(err.contains("[tool.devkit].templates-dir") && err.contains("missing"), "{err}");
      }
    }
  }
}

#[cfg(test)]
mod publish_index_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(publish_index: Option<&str>) -> ProjectContext {
    ProjectContext {
      root: std::path::PathBuf::from("/p"),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: false,
      name: "proj".into(),
      version: None,
      origin: None,
      docker_services: vec![],
      docker_legacy_keys: vec![],
      docker_files: false,
      silence_unlisted_services_warning: false,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: publish_index.map(str::to_string),
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
      templates_dir: None,
    }
  }

  #[test]
  fn the_devkit_index_placeholder() {
    let mut c = ctx(None);
    c.devkit_index = "Internal".into();
    assert_eq!(substitute("{devkit_index}", &c, Escape::Toml), "Internal");
  }

  #[test]
  fn publish_index_placeholders() {
    let out = substitute("{publish_index} {publish_index_key}", &ctx(Some("my-index")), Escape::None);
    assert_eq!(out, "my-index MY_INDEX");
    assert_eq!(substitute("[{publish_index}]", &ctx(None), Escape::None), "[]");
  }
}

#[cfg(test)]
mod docker_placeholder_tests {
  use super::*;
  use std::collections::HashSet;

  fn ctx(origin: Option<&str>) -> ProjectContext {
    ProjectContext {
      root: std::path::PathBuf::from("/p"),
      package: "proj".into(),
      dependencies: HashSet::new(),
      has_docker: true,
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
      devkit_index: "SFTPyPI".into(),
      release_workflow: true,
      templates_dir: None,
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
  fn git_repo_is_the_canonical_https_form_for_github_origins() {
    assert_eq!(
      git_repo(&ctx(Some("git@github.com:AetherBreaker/aeth_ext.git"))),
      "https://github.com/AetherBreaker/aeth_ext.git"
    );
    assert_eq!(
      git_repo(&ctx(Some("https://gitlab.com/o/r"))),
      "https://gitlab.com/o/r",
      "non-GitHub kept as-is"
    );
    assert_eq!(git_repo(&ctx(None)), "");
  }

  #[test]
  fn docker_placeholders_substitute_except_the_lazy_ones() {
    let out = substitute(
      "{git_repo} {git_tag} {service} {python_dir}",
      &ctx(Some("https://github.com/o/r.git")),
      Escape::None,
    );
    assert_eq!(out, "https://github.com/o/r.git {git_tag} {service} src");
  }
}
