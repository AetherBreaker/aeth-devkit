//! Locating and loading templates, with placeholder substitution.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

use crate::context::ProjectContext;

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

/// Read a template (by its target name, e.g. `pyproject.toml`) and substitute
/// `{project_root}` / `{package}` / `{python_dir}` / `{devkit_bin}` / `{publish_index}` /
/// `{publish_index_key}`.
pub fn load(templates_dir: &Path, name: &str, ctx: &ProjectContext, escape: Escape) -> Result<String> {
  let path = templates_dir.join(template_file_name(name));
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading template {}", path.display()))?;
  Ok(substitute(&text, ctx, escape))
}

/// Like [`load`], but returns `None` when the template file does not exist (used for
/// optional overlays such as `vscode/extensions.rust.json`).
pub fn load_optional(templates_dir: &Path, name: &str, ctx: &ProjectContext, escape: Escape) -> Result<Option<String>> {
  let path = templates_dir.join(template_file_name(name));
  if !path.is_file() {
    return Ok(None);
  }
  load(templates_dir, name, ctx, escape).map(Some)
}

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
    .replace("{devkit_bin}", &esc(&devkit_bin(&ctx.root)))
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
}

/// Block markers for line-based templates (YAML): `# setup-project: if-publish-index` …
/// `# setup-project: end` survives only when the project has a publish index,
/// `# setup-project: if-no-publish-index` … `# setup-project: end` only when it has none.
/// Marker lines are dropped either way. Markers may be indented; the lines inside keep
/// their own indentation, so a block can sit anywhere in the document.
pub fn gate_publish_index(text: &str, has_publish_index: bool) -> String {
  let mut out = String::with_capacity(text.len());
  // `Some(keep)` while inside a block, saying whether its lines are emitted.
  let mut block: Option<bool> = None;
  for line in text.lines() {
    match line.trim().strip_prefix("# setup-project: ") {
      Some("if-publish-index") => block = Some(has_publish_index),
      Some("if-no-publish-index") => block = Some(!has_publish_index),
      Some("end") => block = None,
      _ => {
        if block.unwrap_or(true) {
          out.push_str(line);
          out.push('\n');
        }
      }
    }
  }
  out
}

/// How a hook should invoke `devkit`: the venv's own console script when one exists
/// (quoted, and via `$CLAUDE_PROJECT_DIR` so the file stays valid if the repo moves),
/// else `uv run devkit`. The direct path skips `uv run`'s ~140 ms environment check on
/// every hook invocation.
fn devkit_bin(root: &Path) -> String {
  for rel in [".venv/Scripts/devkit.exe", ".venv/bin/devkit"] {
    if root.join(rel).is_file() {
      return format!("\"$CLAUDE_PROJECT_DIR/{rel}\"");
    }
  }
  "uv run devkit".to_string()
}

/// Resolve the templates directory: explicit flag, env var, the Python package next to
/// this executable, or (dev builds) the source tree.
pub fn locate(explicit: Option<&Path>) -> Result<PathBuf> {
  if let Some(p) = explicit {
    return existing_dir(p.to_path_buf(), "--templates-dir");
  }
  if let Ok(p) = std::env::var("DEVKIT_TEMPLATES") {
    return existing_dir(PathBuf::from(p), "DEVKIT_TEMPLATES");
  }
  if let Some(p) = from_python() {
    return Ok(p);
  }
  let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("..")
    .join("..")
    .join("python")
    .join("aeth_devkit")
    .join("templates");
  if dev.is_dir() {
    return Ok(dev);
  }
  bail!("could not locate aeth_devkit templates; pass --templates-dir or set DEVKIT_TEMPLATES")
}

fn existing_dir(p: PathBuf, what: &str) -> Result<PathBuf> {
  if p.is_dir() {
    Ok(p)
  } else {
    bail!("{what}: {} is not a directory", p.display())
  }
}

/// Ask the Python interpreter that lives alongside this binary (the venv's `Scripts/`)
/// where `aeth_devkit` is installed.
fn from_python() -> Option<PathBuf> {
  let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
  let candidates = [exe_dir.join("python.exe"), exe_dir.join("python"), PathBuf::from("python")];
  for py in candidates {
    // A candidate that cannot be spawned (e.g. `python.exe` on Unix) must not end the search.
    let Ok(out) = Command::new(&py)
      .args([
        "-c",
        "import aeth_devkit, os; print(os.path.join(os.path.dirname(aeth_devkit.__file__), 'templates'))",
      ])
      .output()
    else {
      continue;
    };
    if out.status.success() {
      let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
      if p.is_dir() {
        return Some(p);
      }
    }
  }
  None
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
mod devkit_bin_tests {
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
      python_dir: "src".into(),
      has_rust: false,
      publish_index: None,
    }
  }

  #[test]
  fn devkit_bin_falls_back_to_uv_run_without_a_venv() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
      substitute("{devkit_bin} hook x", &ctx(dir.path()), Escape::None),
      "uv run devkit hook x"
    );
  }

  #[test]
  fn devkit_bin_uses_the_venv_script_quoted_and_json_escaped() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join(".venv").join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("devkit"), "").unwrap();
    let out = substitute(r#""cmd": "{devkit_bin} hook x""#, &ctx(dir.path()), Escape::Json);
    assert_eq!(out, r#""cmd": "\"$CLAUDE_PROJECT_DIR/.venv/bin/devkit\" hook x""#);
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
      python_dir: "src".into(),
      has_rust: false,
      publish_index: publish_index.map(str::to_string),
    }
  }

  #[test]
  fn publish_index_placeholders() {
    let out = substitute("{publish_index} {publish_index_key}", &ctx(Some("my-index")), Escape::None);
    assert_eq!(out, "my-index MY_INDEX");
    assert_eq!(substitute("[{publish_index}]", &ctx(None), Escape::None), "[]");
  }

  const GATED: &str = "a\n# setup-project: if-publish-index\nidx1\n  # setup-project: end\nb\n  # setup-project: if-no-publish-index\n  pypi\n  # setup-project: end\nc\n";

  #[test]
  fn gate_keeps_exactly_one_variant_and_no_markers() {
    assert_eq!(gate_publish_index(GATED, true), "a\nidx1\nb\nc\n");
    assert_eq!(gate_publish_index(GATED, false), "a\nb\n  pypi\nc\n");
  }

  #[test]
  fn gate_leaves_unmarked_text_alone() {
    assert_eq!(gate_publish_index("x\n  y\n", true), "x\n  y\n");
  }
}
