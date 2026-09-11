//! `devkit setup-project` — standardize a project's configuration from the templates in the
//! `devkit-templates` package of the project's environment.

pub mod changes;
pub mod cli;
pub mod completion;
pub mod context;
pub mod docker;
pub mod format;
pub mod gate;
pub mod gate_eval;
pub mod git;
pub mod interrupt;
pub mod json_merge;
pub mod lines;
pub mod md_block;
pub mod packages;
pub mod templates;
pub mod toml_merge;
pub mod vscode;

use std::path::Path;

use anyhow::{Context as _, Result};

use crate::changes::Changes;
use crate::context::ProjectContext;

/// First line of every rendered release workflow; a file without it is the project's own.
const DEVKIT_WORKFLOW_HEADER: &str = "# Installed and kept current by `devkit setup-project`";

/// Everything `run_with` needs from outside: the Docker collaborators, the index client the
/// package step asks for newer releases, and the project's venv as it holds devkit packages.
pub struct Deps<'a> {
  pub docker: docker::Deps<'a>,
  pub index: &'a dyn aeth_devkit_core::index::IndexClient,
  pub venv: &'a dyn packages::Venv,
}

/// Apply every template to the project with the given collaborators (see [`Deps`]) and a
/// context the caller discovered — before `git::stage_bases` when committing, which resets
/// `pyproject.toml` to HEAD; `cli` refuses a `[tool.docker].services` that differs between
/// the two rather than merge on one and switch on the other.
/// Returns the collected change log; nothing is written when `dry_run` is set.
pub fn run_with(ctx: &ProjectContext, templates_override: Option<&Path>, dry_run: bool, deps: &Deps) -> Result<Changes> {
  let mut changes = Changes::new(dry_run);
  // Previews feed the VS Code review at the end of a dry run, and a reviewer present is
  // the sign one will open; without it (a headless dry run) nobody pays for the copies.
  changes.keep_previews = deps.docker.reviewer.is_some();

  // 0. The templates: an override renders a working tree; otherwise the environment's
  //    devkit-templates, installed first when the project lacks it, since nothing renders
  //    without it (spec 4.0). Before the merge, whose template this reads.
  let templates_dir = match templates_override {
    Some(dir) => dir.to_path_buf(),
    None => packages::ensure_templates(ctx, deps, dry_run, &mut changes)?,
  };
  let templates_dir = templates_dir.as_path();

  // 0b. The gates (spec 2.5): every expression in the templates package, evaluated once
  //     against the working copy's pyproject as it is now. The container package's two
  //     templates join the table after the package step below has installed them, against
  //     this same document, so an adopting or upgrading run renders what it just installed.
  let pyproject_text = std::fs::read_to_string(ctx.root.join("pyproject.toml")).context("reading pyproject.toml")?;
  let pyproject_doc: toml_edit::DocumentMut = pyproject_text.parse().context("parsing pyproject.toml")?;
  let facts = gate::Facts {
    rust: ctx.has_rust,
    docker_files: ctx.docker_files,
  };
  let mut gates = gate::Gates::build(&gate::collect_dir(templates_dir)?, &pyproject_doc, None, &facts)?;

  // 1. pyproject.toml
  let pyproject_template = templates::load(templates_dir, "pyproject.toml", ctx, templates::Escape::Toml, &gates)?;
  {
    let path = ctx.root.join("pyproject.toml");
    let original = std::fs::read_to_string(&path).context("reading pyproject.toml")?;
    let mut log = Vec::new();
    let merged = toml_merge::merge_pyproject(&original, &pyproject_template, ctx, &mut log)?;
    changes.record(&path, &original, &merged, log)?;
  }

  // 1b. The devkit packages (spec 4.0): list, lock under the running devkit, sync. Before
  //     the Docker step, which renders the Dockerfile from the installed container package.
  packages::advance(
    ctx,
    deps,
    dry_run,
    &packages::active(ctx),
    &packages::latest_requested(&pyproject_template),
    &mut changes,
  )?;
  gates.extend(&container_templates(&ctx.root, deps.venv)?, &pyproject_doc, &facts)?;

  // 2. .vscode/settings.json and extensions.json — deep merge, plus a Rust overlay
  //    (`vscode/<name>.rust.json`) for projects that also contain a crate.
  for name in ["settings.json", "extensions.json"] {
    let path = ctx.root.join(".vscode").join(name);
    let template = templates::load(templates_dir, &format!("vscode/{name}"), ctx, templates::Escape::Json, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let mut merged = json_merge::merge_json_file(original.as_deref(), &template, &mut log)?;
    if ctx.has_rust {
      let (stem, _) = name.rsplit_once('.').unwrap_or((name, ""));
      let overlay = templates::load_optional(
        templates_dir,
        &format!("vscode/{stem}.rust.json"),
        ctx,
        templates::Escape::Json,
        &gates,
      )?;
      if let Some(overlay) = overlay {
        merged = json_merge::merge_json_file(Some(&merged), &overlay, &mut log)?;
      }
    }
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 3. .vscode/launch.json — create or patch
  let launch_path = ctx.root.join(".vscode").join("launch.json");
  let launch_template = templates::load(templates_dir, "vscode/launch.json", ctx, templates::Escape::Json, &gates)?;
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
  let env_template = templates::load(templates_dir, "env", ctx, templates::Escape::None, &gates)?;
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
    let template = load_with_rust_overlay(templates_dir, "gitignore", ctx, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::merge_gitignore(original.as_deref(), &template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 7. .gitattributes — line union
  {
    let path = ctx.root.join(".gitattributes");
    let template = templates::load(templates_dir, "gitattributes", ctx, templates::Escape::None, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::line_union(original.as_deref(), &template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 8. .dockerignore — line union, only for projects that have a Docker setup
  if ctx.has_docker {
    let path = ctx.root.join(".dockerignore");
    let template = load_with_rust_overlay(templates_dir, "dockerignore", ctx, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = lines::line_union(original.as_deref(), &template, &mut log);
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 8b. Docker: the Dockerfile from the installed container package replaced whole and the
  //     compose file edited in place, each behind consent (see `docker`).
  if ctx.has_docker {
    docker::apply(ctx, templates_dir, deps, &gates, &mut changes)?;
  }

  // 9. AGENTS.md — devkit-managed block; text outside the markers belongs to the project.
  {
    let path = ctx.root.join("AGENTS.md");
    let template = templates::load(templates_dir, "AGENTS.md", ctx, templates::Escape::None, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = md_block::merge_managed_block(original.as_deref(), &template, &mut log)?;
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 10. Create-if-missing files: `.claude/CLAUDE.md` (the project's hook for Claude-only
  //     text; the shared content is one `@../AGENTS.md` import away) and the Claude GitHub
  //     workflow (a project may have customized it; a routine run must not revert that —
  //     the release workflow below is the opposite case).
  for (rel, template_name) in [
    (".claude/CLAUDE.md", "claude/CLAUDE.md"),
    (".github/workflows/claude.yml", "github/workflows/claude.yml"),
  ] {
    let path = ctx.root.join(rel);
    if path.is_file() {
      continue;
    }
    let template = templates::load(templates_dir, template_name, ctx, templates::Escape::None, &gates)?;
    changes.record_optional(&path, None, &template, vec!["created from template".into()])?;
  }

  // 10b. The release workflow is devkit-owned, unlike `claude.yml`: nothing in it is
  //      project-specific beyond the placeholders, so drift is replaced and reported. The
  //      one manual step — credentials — is announced whenever the devkit workflow displaces
  //      something else (nothing, or a workflow the project wrote): that is when its
  //      secret / trusted-publisher requirement arrives. `[tool.devkit].release-workflow =
  //      false` skips all of it: that project's `release.yml` is its own (a vsix, say), and
  //      the file is neither written nor listed as managed. Nothing is deleted on the way
  //      out; a project that opts out later keeps whatever copy it has.
  if ctx.release_workflow {
    let path = ctx.root.join(".github").join("workflows").join("release.yml");
    let template_name = if ctx.has_rust {
      "github/workflows/release.rust.yml"
    } else {
      "github/workflows/release.yml"
    };
    let rendered = templates::load(templates_dir, template_name, ctx, templates::Escape::None, &gates)?;
    let original = read_optional(&path)?;
    let devkit_owned = original.as_deref().is_some_and(|o| o.starts_with(DEVKIT_WORKFLOW_HEADER));
    let first_install = !devkit_owned;
    let details = if original.is_none() {
      vec![]
    } else {
      vec!["replaced with the devkit release workflow".into()]
    };
    changes.record_optional(&path, original.as_deref(), &rendered, details)?;
    if first_install {
      changes.notes.push(match &ctx.publish_index {
        Some(name) => {
          let key = aeth_devkit_core::pyproject::index_env_key(name);
          format!(
            "the release workflow publishes to {name}; add the repository secrets UV_INDEX_{key}_USERNAME and \
             UV_INDEX_{key}_PASSWORD (gh secret set <NAME>) before the first `devkit release`."
          )
        }
        None => {
          let repo = git::is_git_tracked(&ctx.root)
            .then(|| aeth_devkit_core::git::origin_url(&ctx.root).ok().flatten())
            .flatten()
            .and_then(|u| aeth_devkit_core::github::github_repo_path(&u))
            .unwrap_or_else(|| "<owner>/<repo>".into());
          format!(
            "the release workflow publishes to PyPI with trusted publishing; register {repo} with workflow file name \
             release.yml as a trusted publisher at https://pypi.org/manage/account/publishing/ before the first `devkit release`."
          )
        }
      });
    }
  }

  // 11. Claude settings, in two halves. `settings.json` is shared and committed; everything
  //     resolved from *this* checkout — the absolute pycache prefix, and hook commands
  //     naming this machine's venv layout — goes in the gitignored `settings.local.json`,
  //     because a committed copy would break every teammate on another path or OS. Claude
  //     Code merges the two with the local file winning. Both merge `hooks` by hook name, so
  //     a hand-edited entry is updated in place rather than duplicated.
  for (rel, template_name) in [
    (".claude/settings.json", "claude/settings.json"),
    (".claude/settings.local.json", "claude/settings.local.json"),
  ] {
    let path = ctx.root.join(rel);
    let template = templates::load(templates_dir, template_name, ctx, templates::Escape::Json, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = json_merge::merge_claude_settings(original.as_deref(), &template, &mut log)?;
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 12. .mcp.json — add missing servers, never edit one the project already defines.
  {
    let path = ctx.root.join(".mcp.json");
    let template = templates::load(templates_dir, ".mcp.json", ctx, templates::Escape::Json, &gates)?;
    let original = read_optional(&path)?;
    let mut log = Vec::new();
    let merged = json_merge::merge_mcp_file(original.as_deref(), &template, &mut log)?;
    changes.record_optional(&path, original.as_deref(), &merged, log)?;
  }

  // 13. Managed files git will ignore: say so, and leave the project's `.gitignore` alone.
  //
  //     devkit used to append `!` negations here so a rule like `*.json` could not silently
  //     keep `.claude/settings.json` out of the repo. That was the wrong trade. The rules
  //     are the project's, and doing it *correctly* is worse than doing nothing: git never
  //     descends into an ignored directory, so a file-level negation under a `.claude/` rule
  //     is dead, and the only fix is to un-ignore the directory — which also un-ignores
  //     `settings.local.json`, `shell-snapshots/` and everything else Claude keeps there.
  //     Reversing a deliberate ignore rule is the user's call, so we describe the situation
  //     and let them make it.
  if git::is_git_tracked(&ctx.root) {
    // Every managed file, not just the ones this run changed: a project that tightens its
    // `.gitignore` after a successful setup changes nothing on the next run, and the
    // warning has to keep appearing until it is dealt with.
    for managed in &changes.managed {
      let Ok(rel) = managed.strip_prefix(&ctx.root) else {
        // Written outside the project root (a `launch.json` envFile pointing at `../`).
        // git has no opinion on it, so there is nothing to warn about.
        continue;
      };
      let rel = rel.to_string_lossy().replace('\\', "/");
      // Files that are *supposed* to be ignored: secrets, and the per-machine settings half.
      if git::is_intentionally_local(&rel) || rel == ".gitignore" {
        continue;
      }
      if !git::is_ignored(&ctx.root, &rel) {
        continue;
      }
      // Name the ancestor when that is the real cause, because it changes the fix.
      // `match_indices('/')` yields each prefix up to a separator, i.e. each ancestor dir.
      let blocked_by = rel
        .match_indices('/')
        .map(|(i, _)| format!("{}/", &rel[..i]))
        .find(|dir| git::is_ignored(&ctx.root, dir));
      changes.notes.push(match blocked_by {
        Some(dir) => format!(
          "{rel} is managed by devkit but git ignores {dir}, so it will not be committed. \
           A `!{rel}` line alone will not help — git does not look inside an ignored \
           directory. Un-ignoring {dir} would also expose everything else in it."
        ),
        None => format!("{rel} is managed by devkit but is gitignored, so it will not be committed."),
      });
    }
  }

  // 14. Obsolete artifacts: reported, never removed.
  if !ctx.has_docker && ctx.docker_files && !ctx.silence_unlisted_services_warning {
    changes.notes.push(
      "Docker files found but no service is listed in `[tool.docker].services`; list the app service(s) to manage \
       them, or set `[tool.docker].silence_unlisted_services_warning = true` to keep your own Docker setup."
        .into(),
    );
  }
  if !ctx.docker_legacy_keys.is_empty() {
    // Any Docker setup at all (services or files) means the table is not junk.
    let tail = if ctx.has_docker || ctx.docker_files {
      ""
    } else {
      " — or delete the whole table if the project has no Docker setup."
    };
    changes.notes.push(format!(
      "pyproject.toml [tool.docker] still has {}: fold `chown_paths` into `required_persisted_dirs`, move any `mkdirs` \
       scratch directories to temp dirs, and delete both keys; the entrypoint refuses to start while they remain{tail}",
      ctx.docker_legacy_keys.join(" and ")
    ));
  }
  if ctx.root.join(".github").join("copilot-instructions.md").is_file() {
    changes
      .notes
      .push(".github/copilot-instructions.md is superseded by AGENTS.md (chat.useAgentsMdFile is on); safe to delete.".into());
  }

  // 15. Shell completion for poe, from the venv's `devkit-complete` the package step
  //     installed (1b), for the shells on PATH. Last, and outside the project: it writes to
  //     the home directory, so a dry run asks the installer for its own dry run. A dry run
  //     before adoption has no binary yet; 1b's note already says a plain run adds it.
  match completion::binary(&ctx.root) {
    Some(bin) => {
      let shells = completion::shells_on(&std::env::var_os("PATH").unwrap_or_default());
      completion::install(&ctx.root, &bin, shells, deps.docker.runner, dry_run, &mut changes);
    }
    None if dry_run => {}
    None => changes.notes.push(
      "devkit-complete is not in the project's environment, so shell completion was not installed; is the environment \
       elsewhere (UV_PROJECT_ENVIRONMENT)?"
        .into(),
    ),
  }

  Ok(changes)
}

/// Assemble a line-based template (`gitignore`, `dockerignore`) from its layers, in order:
/// the vendored base, the `rust.<name>` overlay (only when the project contains a crate),
/// then the `devkit.<name>` additions — so project-specific rules always come last.
fn load_with_rust_overlay(templates_dir: &Path, name: &str, ctx: &ProjectContext, gates: &gate::Gates) -> Result<String> {
  let mut template = templates::load(templates_dir, name, ctx, templates::Escape::None, gates)?;
  let mut layers = Vec::new();
  if ctx.has_rust {
    layers.push(format!("rust.{name}"));
  }
  layers.push(format!("devkit.{name}"));
  for layer in layers {
    if let Some(overlay) = templates::load_optional(templates_dir, &layer, ctx, templates::Escape::None, gates)? {
      if !template.ends_with('\n') {
        template.push('\n');
      }
      template.push('\n');
      template.push_str(&overlay);
    }
  }
  Ok(template)
}

/// The two templates of the container package as the venv holds it now, `(name, text)`;
/// empty when the package (or a template) is absent.
pub fn container_templates(root: &Path, venv: &dyn packages::Venv) -> Result<Vec<(String, String)>> {
  let mut templates = Vec::new();
  if let Some(installed) = venv.installed(root, &packages::CONTAINER) {
    for file in [docker::static_files::TEMPLATE_FILE, docker::scaffold::TEMPLATE_FILE] {
      let path = installed.dir.join(file);
      if path.is_file() {
        templates.push((
          file.to_string(),
          std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
        ));
      }
    }
  }
  Ok(templates)
}

/// The gates for a render outside `run_with`: every template under `templates_dir` (when
/// given) plus the container package's as installed now, swept and evaluated against the
/// working copy's `pyproject.toml`, and against `head_pyproject` when a committing run must
/// refuse a gate that differs (spec 2.5). `docker-pin` passes no templates dir: it renders
/// only the container's Dockerfile.
pub fn gates_for(
  ctx: &ProjectContext,
  templates_dir: Option<&Path>,
  venv: &dyn packages::Venv,
  head_pyproject: Option<&str>,
) -> Result<gate::Gates> {
  let mut templates = match templates_dir {
    Some(dir) => gate::collect_dir(dir)?,
    None => Vec::new(),
  };
  templates.extend(container_templates(&ctx.root, venv)?);
  let text = std::fs::read_to_string(ctx.root.join("pyproject.toml")).context("reading pyproject.toml")?;
  let doc: toml_edit::DocumentMut = text.parse().context("parsing pyproject.toml")?;
  let head = head_pyproject
    .map(|t| t.parse::<toml_edit::DocumentMut>().context("parsing HEAD's pyproject.toml"))
    .transpose()?;
  gate::Gates::build(
    &templates,
    &doc,
    head.as_ref(),
    &gate::Facts {
      rust: ctx.has_rust,
      docker_files: ctx.docker_files,
    },
  )
}

pub(crate) fn read_optional(path: &Path) -> Result<Option<String>> {
  match std::fs::read_to_string(path) {
    Ok(s) => Ok(Some(s)),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
    Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
  }
}

fn same_path(a: &Path, b: &Path) -> bool {
  let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
  norm(a) == norm(b)
}
