//! `devkit docker-pin` — pin the compose file's version for the services that build this
//! project, then commit and push the change.

pub mod resolve;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result, bail};
use clap::Parser;
use toml_edit::DocumentMut;

use aeth_devkit_core::compose::tree::{self, Edit};
use aeth_devkit_core::compose::{self, PinKind};
use aeth_devkit_core::index::IndexClient;
use aeth_devkit_core::paths::strip_verbatim;
use aeth_devkit_core::process::Runner;
use aeth_devkit_core::version::parse_lenient;
use aeth_devkit_core::{git, github, pyproject};

/// Pin the docker compose file to a released version of this project.
#[derive(Parser, Debug, Clone)]
#[command(name = "devkit-docker-pin", about)]
pub struct Args {
  /// Project root (defaults to the current directory).
  #[arg(long, default_value = ".")]
  pub root: PathBuf,

  /// Pin to this exact version (with or without a leading `v`; pre-releases allowed).
  /// Default: the latest stable version released everywhere.
  #[arg(long, short = 'V')]
  pub version: Option<String>,

  /// Resolve and report without changing anything.
  #[arg(long)]
  pub dry_run: bool,

  /// Edit the compose file but do not commit (implies --no-push).
  #[arg(long)]
  pub no_commit: bool,

  /// Commit locally but do not push.
  #[arg(long)]
  pub no_push: bool,

  /// Compose file to edit (default: auto-discovered from the repo root).
  #[arg(long, short = 'c')]
  pub compose_file: Option<PathBuf>,
}

/// The injectable collaborators, mirroring the release crate's pattern: production passes
/// real ones (see `run_real`), tests pass recorders and stubs.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub index: &'a dyn IndexClient,
}

pub fn run(args: &Args, deps: &Deps) -> Result<ExitCode> {
  let start = strip_verbatim(
    args
      .root
      .canonicalize()
      .with_context(|| format!("resolving {}", args.root.display()))?,
  );
  let root = git::toplevel(&start).context("docker-pin must run inside a git repository")?;

  // --- Project identity: package name, publish indexes, origin. ---
  let pyproject_path = root.join("pyproject.toml");
  let doc: DocumentMut = std::fs::read_to_string(&pyproject_path)
    .with_context(|| format!("{} not found", pyproject_path.display()))?
    .parse()
    .context("parsing pyproject.toml")?;
  let package = pyproject::project_name(&doc)?;
  let indexes = pyproject::publish_indexes(&doc)?;
  let origin = git::origin_url(&root)?;
  let origin_norm = origin.as_deref().and_then(github::normalize_repo);
  let gh_repo = origin.as_deref().and_then(github::github_repo_path);

  // --- Compose file: explicit flag or discovery. ---
  let compose_path = match &args.compose_file {
    Some(p) => {
      let p = if p.is_absolute() { p.clone() } else { start.join(p) };
      if !p.is_file() {
        bail!("{} does not exist", p.display());
      }
      p
    }
    None => compose::find_compose_file(&root)?.with_context(|| {
      format!(
        "no docker compose file found under {} (expected one of: {})",
        root.display(),
        compose::COMPOSE_NAMES.join(", ")
      )
    })?,
  };
  let rel = compose_path
    .strip_prefix(&root)
    .map(|p| p.to_string_lossy().replace('\\', "/"))
    .unwrap_or_else(|_| compose_path.to_string_lossy().replace('\\', "/"));
  println!("Compose  : {rel}");

  // --- Decide which content to edit: HEAD's copy when we will commit over a dirty file. ---
  let will_commit = !args.no_commit && !args.dry_run;
  let will_push = will_commit && !args.no_push;
  let worktree_text = std::fs::read_to_string(&compose_path).with_context(|| format!("reading {}", compose_path.display()))?;
  let dirty = git::is_dirty(&root, &[&rel])?;
  let head = git::head_blob(&root, &rel)?;
  if will_commit && dirty && head.is_none() {
    bail!("{rel} is not committed yet; commit it first (or pass --no-commit)");
  }
  let base_text: String = if will_commit && dirty {
    String::from_utf8(head.clone().unwrap()).context("compose file at HEAD is not UTF-8")?
  } else {
    worktree_text.clone()
  };

  // --- Match services and resolve the version (which is also the completeness preflight). ---
  let blocks = compose::parse_services(&base_text);
  let targets = compose::match_services(&blocks, &package, origin_norm.as_deref())?;
  let need_tag = targets.iter().any(|t| t.kind == PinKind::GitTag);
  let resolved = resolve::resolve_version(
    deps,
    &root,
    &package,
    gh_repo.as_deref(),
    &indexes,
    args.version.as_deref(),
    need_tag,
  )?;
  let value_for = |kind: PinKind| -> String {
    match kind {
      PinKind::GitTag => resolved.tag_spelling.clone().expect("need_tag guaranteed a spelling"),
      PinKind::PackageVersion => resolved.version.to_string(),
    }
  };
  let display = if need_tag {
    value_for(PinKind::GitTag)
  } else {
    resolved.version.to_string()
  };
  println!(
    "Version  : {display}{}",
    if args.version.is_some() {
      " (explicitly provided)"
    } else {
      " (latest complete release)"
    }
  );

  // --- Report and short-circuit. ---
  let mut edits: Vec<Edit> = Vec::new();
  for t in &targets {
    let new = value_for(t.kind);
    let already = parse_lenient(&t.current).is_some_and(|v| v == resolved.version);
    println!(
      "  {}: {} {} -> {new}{}",
      t.service,
      if t.kind == PinKind::GitTag { "GIT_TAG" } else { "PACKAGE_VERSION" },
      if t.current.is_empty() { "<not set>" } else { &t.current },
      if already { " (already pinned)" } else { "" },
    );
    if !already {
      edits.push(Edit::SetValue { line: t.line, value: new });
    }
  }
  if edits.is_empty() {
    println!("Already pinned to {display}. No changes made.");
    return Ok(ExitCode::SUCCESS);
  }
  if args.dry_run {
    println!("Dry run: no changes made.");
    return Ok(ExitCode::SUCCESS);
  }

  // --- Behind-origin preflight, before touching anything. ---
  if will_push {
    git::fetch(deps.runner, &root)?;
    if git::upstream(deps.runner, &root)?.is_none() {
      bail!("the current branch has no upstream; push it once first (or pass --no-push)");
    }
    let behind = git::behind_count(deps.runner, &root)?;
    if behind > 0 {
      bail!("the branch is {behind} commit(s) behind origin; pull first (or pass --no-push)");
    }
  }

  let message = format!("chore: pin {package} to {display}");
  let pinned_text = tree::apply_edits(&base_text, &edits);

  if will_commit && dirty {
    // Commit the pin against HEAD's copy; the user's uncommitted edits ride on top.
    let base = head.unwrap();
    // The user's copy in repository form (clean filters applied), not the raw file: on a
    // `core.autocrlf=true` checkout the raw bytes are CRLF against an LF `base`, and the
    // merge would then flag the pinned line as an overlapping edit.
    let current = git::worktree_blob(&root, &rel)?.with_context(|| format!("{rel} vanished during the run"))?;
    let merged = git::merge_file(&root, &current.bytes, &base, pinned_text.as_bytes())?
      .context("your uncommitted compose changes overlap the pinned lines; commit or revert them first")?;
    let mode = git::head_mode(&root, &rel)?.unwrap_or_else(|| "100644".into());
    let sha = git::hash_object(&root, pinned_text.as_bytes())?;
    git::commit_files_on_head(
      &root,
      &[git::IndexEntry {
        path: rel.clone(),
        staged: Some((mode, sha)),
      }],
      &message,
    )?;
    // Smudged iff the user's copy was, so the file keeps the line endings it had.
    git::write_worktree(&root, &rel, &merged, current.filtered).with_context(|| format!("writing {}", compose_path.display()))?;
    println!("Committed pin on HEAD; your uncommitted changes to {rel} were kept in the working tree.");
  } else {
    std::fs::write(&compose_path, &pinned_text).with_context(|| format!("writing {}", compose_path.display()))?;
    println!("Updated {rel}");
    if will_commit {
      // `commit_paths` takes `paths: &[String]` — it only *borrows* the list for the
      // duration of the call, so we never needed to own one. `&[rel.clone()]` built a
      // temporary one-element array, deep-copying the String's heap buffer into it, just
      // to immediately hand back a borrow of that array and drop it. `slice::from_ref`
      // reinterprets the single `&String` we already hold as a slice of length 1: same
      // pointer, no allocation, no copy.
      let hash = git::commit_paths(&root, std::slice::from_ref(&rel), &message)?;
      println!("Committed {hash}: {message}");
    }
  }

  if will_push {
    let branch = git::current_branch(&root)?;
    git::push_refs(deps.runner, &root, &[&branch])?;
    println!("Pushed {branch}.");
  }
  Ok(ExitCode::SUCCESS)
}

/// [`run`] with the real collaborators.
pub fn run_real(args: &Args) -> Result<ExitCode> {
  let index = aeth_devkit_core::index::HttpIndexClient::with_timeout(std::time::Duration::from_secs(30));
  run(
    args,
    &Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      index: &index,
    },
  )
}
