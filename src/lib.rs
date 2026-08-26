//! `sft-setup` — standardize an SFT project's configuration from the templates shipped
//! with `poe_tasks`. See docs/specs/2026-08-26-setup-project-design.md.

pub mod changes;
pub mod context;
pub mod json_merge;
pub mod lines;
pub mod templates;
pub mod toml_merge;

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::changes::Changes;
use crate::context::ProjectContext;

/// Apply every template to the project at `root`. Returns the collected change log;
/// nothing is written when `dry_run` is set.
pub fn run(root: &Path, templates_dir: &Path, dry_run: bool) -> Result<Changes> {
  let ctx = ProjectContext::discover(root)?;
  let mut changes = Changes::new(dry_run);

  // 1. pyproject.toml
  {
    let path = ctx.root.join("pyproject.toml");
    let original = std::fs::read_to_string(&path).context("reading pyproject.toml")?;
    let template = templates::load(templates_dir, "pyproject.toml", &ctx, templates::Escape::Toml)?;
    let mut log = Vec::new();
    let merged = toml_merge::merge_pyproject(&original, &template, &ctx, &mut log)?;
    changes.record(&path, &original, &merged, log)?;
  }

  // 2. .vscode/settings.json and extensions.json — deep merge, plus a Rust overlay
  //    (`vscode/<name>.rust.json`) for projects that also contain a crate.
  for name in ["settings.json", "extensions.json"] {
    let path = ctx.root.join(".vscode").join(name);
    let template = templates::load(templates_dir, &format!("vscode/{name}"), &ctx, templates::Escape::Json)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let mut merged = json_merge::merge_json_file(original.as_deref(), &template, &mut log)?;
    if ctx.has_rust {
      let (stem, _) = name.rsplit_once('.').unwrap_or((name, ""));
      let overlay = templates::load_optional(templates_dir, &format!("vscode/{stem}.rust.json"), &ctx, templates::Escape::Json)?;
      if let Some(overlay) = overlay {
        merged = json_merge::merge_json_file(Some(&merged), &overlay, &mut log)?;
      }
    }
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 3. .vscode/launch.json — create or patch
  let launch_path = ctx.root.join(".vscode").join("launch.json");
  let launch_template = templates::load(templates_dir, "vscode/launch.json", &ctx, templates::Escape::Json)?;
  let launch_original = read_optional(&launch_path)?;
  let mut env_files = Vec::new();
  {
    let mut log = Vec::new();
    let merged = json_merge::patch_launch(launch_original.as_deref(), &launch_template, &mut env_files, &mut log)?;
    changes.record_optional(&launch_path, launch_original.as_deref(), &merged, log)?;
  }

  // 4. .vscode/tasks.json — patch only
  let tasks_path = ctx.root.join(".vscode").join("tasks.json");
  if let Some(original) = read_optional(&tasks_path)? {
    let mut log = Vec::new();
    let merged = json_merge::patch_tasks(&original, &launch_template, &mut log)?;
    changes.record(&tasks_path, &original, &merged, log)?;
  }

  // 5. .env and any other env file referenced by launch.json
  let env_template = templates::load(templates_dir, "env", &ctx, templates::Escape::None)?;
  let mut env_targets = vec![ctx.root.join(".env")];
  for f in env_files {
    let resolved = ctx.resolve_workspace_var(&f);
    if !env_targets.iter().any(|p| same_path(p, &resolved)) {
      env_targets.push(resolved);
    }
  }
  for path in env_targets {
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::upsert_env(original.as_deref(), &env_template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 6. .gitignore — replace-or-prepend (Rust projects get the `rust.gitignore` overlay
  //    appended to the template first, so its rules count as template rules).
  {
    let path = ctx.root.join(".gitignore");
    let template = load_with_rust_overlay(templates_dir, "gitignore", &ctx)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::merge_gitignore(original.as_deref(), &template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 7. .gitattributes — line union
  {
    let path = ctx.root.join(".gitattributes");
    let template = templates::load(templates_dir, "gitattributes", &ctx, templates::Escape::None)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::line_union(original.as_deref(), &template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 8. .dockerignore — line union, only for projects that have a Docker setup
  if ctx.has_docker {
    let path = ctx.root.join(".dockerignore");
    let template = load_with_rust_overlay(templates_dir, "dockerignore", &ctx)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::line_union(original.as_deref(), &template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  Ok(changes)
}

/// Assemble a line-based template (`gitignore`, `dockerignore`) from its layers, in order:
/// the vendored base, the `rust.<name>` overlay (only when the project contains a crate),
/// then the `sft.<name>` additions — so project-specific rules always come last.
fn load_with_rust_overlay(templates_dir: &Path, name: &str, ctx: &ProjectContext) -> Result<String> {
  let mut template = templates::load(templates_dir, name, ctx, templates::Escape::None)?;
  let mut layers = Vec::new();
  if ctx.has_rust {
    layers.push(format!("rust.{name}"));
  }
  layers.push(format!("sft.{name}"));
  for layer in layers {
    if let Some(overlay) = templates::load_optional(templates_dir, &layer, ctx, templates::Escape::None)? {
      if !template.ends_with('\n') {
        template.push('\n');
      }
      template.push('\n');
      template.push_str(&overlay);
    }
  }
  Ok(template)
}
