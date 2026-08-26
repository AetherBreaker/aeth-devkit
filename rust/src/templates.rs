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

/// Read a template and substitute `{project_root}` / `{package}`.
pub fn load(templates_dir: &Path, name: &str, ctx: &ProjectContext, escape: Escape) -> Result<String> {
  let path = templates_dir.join(name);
  let text = std::fs::read_to_string(&path).with_context(|| format!("reading template {}", path.display()))?;
  Ok(substitute(&text, ctx, escape))
}

pub fn substitute(text: &str, ctx: &ProjectContext, escape: Escape) -> String {
  let root = ctx.root.to_string_lossy();
  let esc = |s: &str| -> String {
    match escape {
      Escape::None => s.to_string(),
      Escape::Toml | Escape::Json => s.replace('\\', "\\\\").replace('"', "\\\""),
    }
  };
  text.replace("{project_root}", &esc(&root)).replace("{package}", &esc(&ctx.package))
}

/// Resolve the templates directory: explicit flag, env var, the Python package next to
/// this executable, or (dev builds) the source tree.
pub fn locate(explicit: Option<&Path>) -> Result<PathBuf> {
  if let Some(p) = explicit {
    return existing_dir(p.to_path_buf(), "--templates-dir");
  }
  if let Ok(p) = std::env::var("SFT_SETUP_TEMPLATES") {
    return existing_dir(PathBuf::from(p), "SFT_SETUP_TEMPLATES");
  }
  if let Some(p) = from_python() {
    return Ok(p);
  }
  let dev = Path::new(env!("CARGO_MANIFEST_DIR"))
    .join("src")
    .join("poe_tasks")
    .join("templates");
  if dev.is_dir() {
    return Ok(dev);
  }
  bail!("could not locate poe_tasks templates; pass --templates-dir or set SFT_SETUP_TEMPLATES")
}

fn existing_dir(p: PathBuf, what: &str) -> Result<PathBuf> {
  if p.is_dir() {
    Ok(p)
  } else {
    bail!("{what}: {} is not a directory", p.display())
  }
}

/// Ask the Python interpreter that lives alongside this binary (the venv's `Scripts/`)
/// where `poe_tasks` is installed.
fn from_python() -> Option<PathBuf> {
  let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
  let candidates = [exe_dir.join("python.exe"), exe_dir.join("python"), PathBuf::from("python")];
  for py in candidates {
    let out = Command::new(&py)
      .args([
        "-c",
        "import poe_tasks, os; print(os.path.join(os.path.dirname(poe_tasks.__file__), 'templates'))",
      ])
      .output()
      .ok()?;
    if out.status.success() {
      let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
      if p.is_dir() {
        return Some(p);
      }
    }
  }
  None
}
