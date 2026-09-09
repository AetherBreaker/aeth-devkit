//! `devkit docker-pin` — pin the compose file's version for the services that build this
//! project, then commit and push the change.

pub mod resolve;

use std::path::{Path, PathBuf};
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
use aeth_devkit_setup::context::ProjectContext;
use aeth_devkit_setup::docker::static_files::{normalize_newlines, render};
use aeth_devkit_setup::packages::{self, Venv};

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
  pub venv: &'a dyn Venv,
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
  // The Dockerfile is judged now but written only after the preflight below, so a refresh
  // is never left as a local commit when the pin itself cannot proceed.
  let refresh = dockerfile_drift(&root, &doc, deps, args.dry_run)?;
  if edits.is_empty() && refresh.is_none() {
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
  // The compose merge is decided before the Dockerfile refresh commits anything, so an
  // overlap aborts with nothing written: the two commits land together or not at all.
  let compose_merge = if !edits.is_empty() && will_commit && dirty {
    // Commit the pin against HEAD's copy; the user's uncommitted edits ride on top. Their
    // copy is taken in repository form (clean filters applied), not the raw file: on a
    // `core.autocrlf=true` checkout the raw bytes are CRLF against an LF base, and the
    // merge would then flag the pinned line as an overlapping edit.
    let base = head.as_deref().unwrap();
    let current = git::worktree_blob(&root, &rel)?.with_context(|| format!("{rel} vanished during the run"))?;
    let merged = git::merge_file(&root, &current.bytes, base, pinned_text.as_bytes())?
      .context("your uncommitted compose changes overlap the pinned lines; commit or revert them first")?;
    Some((current, merged))
  } else {
    None
  };

  // --- The Dockerfile first: its own commit, ahead of the pin's. ---
  if let Some(refresh) = refresh {
    apply_refresh(&root, refresh, will_commit)?;
  }
  if let Some((current, merged)) = compose_merge {
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
  } else if edits.is_empty() {
    println!("Already pinned to {display}; {rel} unchanged.");
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

/// A `docker/Dockerfile` that differs from the locked devkit-container's template, decided
/// by [`dockerfile_drift`] and written by [`apply_refresh`].
struct DockerfileRefresh {
  rendered: String,
  locked: String,
  /// HEAD's copy when the working copy carries edits of its own, which a write must keep
  /// (a 3-way merge); `None` when the file is clean or absent and is simply replaced.
  base: Option<Vec<u8>>,
}

/// Before a pin: does the committed Dockerfile match the locked devkit-container's
/// template? The lock can advance the entrypoint (`poe lock`) while the Dockerfile stays on
/// the old shape, and a deploy would build the mismatch; this is the last moment before every
/// deploy. Read-only apart from syncing a venv that lags the lock, which a dry run only
/// announces: the template must come from the locked version, or the comparison is against
/// the wrong file. `None` when the file matches, the project has no Docker services, or the
/// package is not locked yet.
fn dockerfile_drift(root: &Path, doc: &DocumentMut, deps: &Deps, dry_run: bool) -> Result<Option<DockerfileRefresh>> {
  // The services switch is read on its own first: `discover` validates the whole
  // setup-project configuration (a single publish index, among others), which a project
  // without Docker services never had to satisfy to be pinned.
  if aeth_devkit_setup::context::services_key(doc)?.is_none_or(|s| s.is_empty()) {
    return Ok(None);
  }
  let ctx = ProjectContext::discover(root)?;
  let lock = std::fs::read_to_string(root.join("uv.lock")).ok();
  let Some(locked) = lock.as_deref().and_then(|l| packages::locked_version(l, packages::CONTAINER.name)) else {
    println!("Dockerfile: devkit-container is not in uv.lock; run setup-project to adopt it. Skipping the refresh.");
    return Ok(None);
  };
  let installed = || deps.venv.installed(root, &packages::CONTAINER).map(|i| i.version);
  let have = installed();
  if have.as_deref() != Some(locked.as_str()) {
    let shown = have.unwrap_or_else(|| "nothing".into());
    if dry_run {
      println!(
        "Dockerfile: devkit-container {locked} is locked but {shown} is installed; a real run syncs the venv first and compares then."
      );
      return Ok(None);
    }
    println!("Syncing the venv (devkit-container {locked} is locked, {shown} installed)");
    match deps.runner.run_inherit("uv", &["sync".into(), "--frozen".into()], root)? {
      Some(0) => {}
      Some(code) => bail!("uv sync --frozen exited with {code}"),
      None => bail!("uv sync --frozen was terminated by a signal"),
    }
    // A sync that left another version in the venv would render the wrong template under a
    // commit message naming the locked one.
    if installed().as_deref() != Some(locked.as_str()) {
      bail!(
        "devkit-container {locked} is locked but `uv sync --frozen` did not put it in the project's environment; is it elsewhere (UV_PROJECT_ENVIRONMENT)?"
      );
    }
  }
  // `installed()` answered just above, and `render` asks the same venv.
  let rendered = render(&ctx, deps.venv)?.expect("the installed package renders");
  let rel = "docker/Dockerfile";
  // Absent is a state; unreadable (locked by an editor, not UTF-8) is an error, or a present
  // file would be judged deleted, or refreshed against nothing.
  let worktree = match std::fs::read_to_string(root.join(rel)) {
    Ok(text) => Some(text),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
    Err(e) => return Err(e).with_context(|| format!("reading {rel}")),
  };
  let dirty = git::is_dirty(root, &[rel])?;
  // HEAD's copy is only ever consulted for a file with uncommitted changes; a clean checkout,
  // the case the pin is meant for, skips those two git spawns.
  let head = if dirty { git::head_blob(root, rel)? } else { None };
  if dirty && head.is_some() && worktree.is_none() {
    bail!("{rel} is deleted in the working tree; restore it or commit the deletion first");
  }
  // Drift is judged against what a commit would be made on: HEAD's copy when the working
  // copy carries edits of its own, else the file as it is.
  let base_text = match &head {
    Some(h) => String::from_utf8(h.clone()).context("docker/Dockerfile at HEAD is not UTF-8")?,
    None => worktree.unwrap_or_default(),
  };
  if normalize_newlines(&base_text) == normalize_newlines(&rendered) {
    println!("Dockerfile: matches devkit-container {locked}.");
    return Ok(None);
  }
  // A file that was never committed has no base to merge its edits against, and replacing
  // it would lose them; the compose file in that state is refused the same way.
  if dirty && head.is_none() {
    bail!("{rel} is not committed yet; commit it first so its edits survive the refresh");
  }
  // Written in the file's own line endings, as setup-project writes it (the template is
  // LF): a CRLF file would otherwise be rewritten whole, and every uncommitted edit in it
  // would read as overlapping the refresh.
  let rendered = if base_text.contains("\r\n") && !rendered.contains("\r\n") {
    rendered.replace('\n', "\r\n")
  } else {
    rendered
  };
  println!(
    "Dockerfile: drifted from devkit-container {locked}; {}.",
    if dry_run { "would be refreshed" } else { "refreshing" }
  );
  Ok(Some(DockerfileRefresh {
    rendered,
    locked,
    base: head,
  }))
}

/// Write the refreshed Dockerfile, in its own commit ahead of the pin's when committing.
/// The file is devkit-owned, so there is no prompt; uncommitted edits to it are merged back
/// on top (3-way against HEAD, like the compose pin), and overlapping edits abort before
/// anything is written.
fn apply_refresh(root: &Path, refresh: DockerfileRefresh, will_commit: bool) -> Result<()> {
  let rel = "docker/Dockerfile".to_string();
  let path = root.join("docker").join("Dockerfile");
  let DockerfileRefresh { rendered, locked, base } = refresh;
  let message = format!("chore(docker): refresh Dockerfile from devkit-container {locked}");
  if let Some(base) = base {
    let current = git::worktree_blob(root, &rel)?.with_context(|| format!("{rel} vanished during the run"))?;
    let merged = git::merge_file(root, &current.bytes, &base, rendered.as_bytes())?
      .context("your uncommitted Dockerfile changes overlap the refreshed lines; commit or revert them first")?;
    if will_commit {
      let mode = git::head_mode(root, &rel)?.unwrap_or_else(|| "100644".into());
      let sha = git::hash_object(root, rendered.as_bytes())?;
      git::commit_files_on_head(
        root,
        &[git::IndexEntry {
          path: rel.clone(),
          staged: Some((mode, sha)),
        }],
        &message,
      )?;
    }
    git::write_worktree(root, &rel, &merged, current.filtered).with_context(|| format!("writing {}", path.display()))?;
    println!(
      "{} your uncommitted changes to {rel} were kept in the working tree.",
      if will_commit {
        "Committed the Dockerfile refresh on HEAD;"
      } else {
        "Refreshed the Dockerfile;"
      }
    );
  } else {
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, &rendered).with_context(|| format!("writing {}", path.display()))?;
    if will_commit {
      let hash = git::commit_paths(root, std::slice::from_ref(&rel), &message)?;
      println!("Committed {hash}: {message}");
    } else {
      println!("Refreshed {rel}");
    }
  }
  Ok(())
}

/// [`run`] with the real collaborators.
pub fn run_real(args: &Args) -> Result<ExitCode> {
  let index = aeth_devkit_core::index::HttpIndexClient::with_timeout(std::time::Duration::from_secs(30));
  run(
    args,
    &Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      index: &index,
      venv: &packages::SystemVenv,
    },
  )
}
