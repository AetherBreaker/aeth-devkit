# VS Code Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `devkit setup-project` gets an in-editor consent flow (a VS Code diff with per-hunk accept/reject) for every Docker change, a `--dry-run` multi-file review, and one devkit-owned extension that also absorbs the Drekker `addToRuntimeBaseClasses` command.

**Architecture:** The Rust setup crate stays the single owner of every decision and every byte written: a `Reviewer` trait sits in front of the terminal `Prompt`, the VS Code implementation exchanges JSON files in the devkit cache dir and opens a `vscode://aeth.aeth-devkit/consent?id=…` URL, and a partial answer comes back as accepted hunk indices that the CLI reassembles itself. The extension (TypeScript, esbuild, vitest) is a view: it serves the CLI's two text snapshots through a content provider, shows a diff with CodeLens controls, and writes one response file. It is released on its own `vscode-extension-vN` tag stream by a workflow that fires on devkit releases.

**Tech Stack:** Rust (`similar`, `serde_json`, `ureq`, `ctrlc`), TypeScript 5 / esbuild / vitest / `@vscode/vsce`, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-09-03-vscode-extension-design.md`

## Global Constraints

- Branch: `feat/vscode-extension`, based on `feat/docker-standardization` (not `main`). Already created; the spec is committed on it.
- Extension id `aeth.aeth-devkit`; scheme `aeth-devkit-proposed`; extension version is a single integer `N`, manifest `N.0.0`, tag `vscode-extension-vN`, asset `aeth-devkit-vscode-N.vsix`. Repo `AetherBreaker/aeth-devkit`.
- Protocol version `1` in every request and in the manifest's `aethDevkit.protocol`; `MIN_EXTENSION_VERSION = 1`.
- The VS Code module engages only when stdin is a TTY and mode is `Ask`. `--check` never touches it. `--dry-run` never installs or edits `argv.json`.
- Only the `code` launcher is supported (`code.cmd` on Windows). Insiders/Cursor go to TODO.md.
- Both diff texts are LF-normalised; hunk ranges are 0-based `[start, end)` line ranges excluding context.
- No timeout while waiting. First Ctrl-C while waiting → terminal prompt for that file; a second Ctrl-C ends the process.
- One refinement to the spec's URL: the CLI passes the request **id** (`?id=<pid>-<n>`, review `?id=review-<pid>`), not a path; the extension derives the path from the cache dir it computes identically. This removes percent-encoding through `code.cmd` and makes the "inside the cache" check trivial. Task 14 updates the spec wording.
- Rust style: 2-space indent (rustfmt config in repo), teaching comments on non-obvious lines, no small single-use helpers. Tests carry no docstrings. Run only targeted tests per task; the full suite once in Task 14.
- Commit messages: Conventional Commits with the `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>` trailer. Never commit `.env`.
- `python/aeth_devkit/_tasks_generated.py` is modified in the worktree from earlier work; never stage it.

## File structure

Rust, `crates/aeth-devkit-setup/src/`:

| File | Responsibility |
|---|---|
| `docker/hunks.rs` (new) | `Hunk`, `hunks()`, `assemble()` — the hunk table and reassembly from accepted indices. Pure. |
| `vscode/protocol.rs` (new) | `Proposal`, `Request`, `Response`, `Reviewer` trait, `ScriptedReviewer`, the version constants. |
| `docker/mod.rs` (modify) | `Consent::decide()` (reviewer first, then prompt), `Decision`, `Deps.reviewer`; per-service compose diffs; `add()`/`interactive` removed. |
| `docker/static_files.rs` (modify) | Uses `decide()`; writes the decision's text. |
| `vscode/mod.rs` (new) | `find_launcher`, `grant_proposal` (argv.json text edit), `stray_notes`, `Options`, `VsCode`, `prepare()`. |
| `vscode/install.rs` (new) | `Fetch` trait + `HttpFetch`/`StubFetch`, tag resolution, `installed_version`, `ensure_extension`. |
| `vscode/session.rs` (new) | `VsCodeReviewer` (request files, `--open-url`, polling, Ctrl-C), `open_review` for dry-run. |
| `changes.rs` (modify) | `previews` collected in dry-run for the review request. |
| `cli.rs` (modify) | `--vscode` / `--no-vscode`, `prepare()` wiring, reload exit, dry-run review. |
| `crates/aeth-devkit-core/src/update.rs` (modify) | `cache_dir()` split out of `cache_path()`. |

TypeScript, `vscode-extension/`:

| File | Responsibility |
|---|---|
| `package.json`, `tsconfig.json`, `esbuild.mjs`, `vitest.config.ts`, `test/vscode-stub.ts` | Project scaffold; single-file bundle to `dist/extension.js`. |
| `src/consent.ts` | Protocol types, `cacheDir`, `requestPath`, `parseRequest`, `isInside`, `HunkState`, `writeResponse`, `Session`. No `vscode` import (vitest-testable). |
| `src/proposedDocs.ts` | `ProposedDocs` content provider + `parseUri`. |
| `src/lenses.ts` | `ConsentLenses` CodeLens provider. |
| `src/extension.ts` | `activate`: URI handler, sessions, commands, tab-close → dismissed, cancel poll, `diffEditor.codeLens`. |
| `src/review.ts` | `openReview` for `--dry-run` (`vscode.changes`). |
| `src/runtimeBaseClasses.ts` | Port of the Drekker command; helpers exported. |
| `scripts/package.sh` | Builds and packages `aeth-devkit-vscode-N.vsix`. |

Other: `.github/workflows/vscode-extension.yml`, `.gitignore`, `.vscode/launch.json`, `README.md`, `TODO.md`, the spec.

---

### Task 1: Hunk table and partial assembly

**Files:**
- Create: `crates/aeth-devkit-setup/src/docker/hunks.rs`
- Modify: `crates/aeth-devkit-setup/src/docker/mod.rs` (add `pub mod hunks;` next to the other `pub mod` lines)

**Interfaces:**
- Produces: `pub struct Hunk { pub current: [usize; 2], pub proposed: [usize; 2] }` (serde Serialize/Deserialize), `pub fn hunks(current: &str, proposed: &str) -> Vec<Hunk>`, `pub fn assemble(current: &str, proposed: &str, hunks: &[Hunk], accepted: &[usize]) -> anyhow::Result<String>`. Inputs must be LF-normalised.

- [ ] **Step 1: Write the module with failing tests**

`crates/aeth-devkit-setup/src/docker/hunks.rs`:

```rust
//! Hunk table for a proposed change and reassembly from the hunks the user accepted.
//! Both texts must be LF-normalised (`static_files::normalize_newlines`): only then does
//! `similar` split lines exactly like `split_inclusive('\n')`, and these ranges are the
//! contract with the VS Code extension, which indexes the same two texts.

use serde::{Deserialize, Serialize};
use similar::{DiffOp, TextDiff};

/// One changed region as 0-based `[start, end)` line ranges in each text. Context lines
/// are excluded, so `proposed[0]` is the line the extension puts the hunk's lens on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hunk {
  pub current: [usize; 2],
  pub proposed: [usize; 2],
}

/// The hunks of the unified diff the terminal prints: three lines of context decide which
/// nearby changes merge into one hunk, exactly as in `static_files::unified_diff`.
pub fn hunks(current: &str, proposed: &str) -> Vec<Hunk> {
  let diff = TextDiff::from_lines(current, proposed);
  diff
    .grouped_ops(3)
    .iter()
    .filter_map(|group| {
      // A group is context + changes + context; only the change ops bound the hunk.
      let mut changed = group.iter().filter(|op| !matches!(op, DiffOp::Equal { .. }));
      let first = changed.next()?;
      let last = changed.last().unwrap_or(first);
      Some(Hunk {
        current: [first.old_range().start, last.old_range().end],
        proposed: [first.new_range().start, last.new_range().end],
      })
    })
    .collect()
}

/// `proposed` with every hunk not in `accepted` reverted to the current text. `Err` when
/// an index is out of range (a malformed response from the extension).
pub fn assemble(current: &str, proposed: &str, hunks: &[Hunk], accepted: &[usize]) -> anyhow::Result<String> {
  if let Some(bad) = accepted.iter().find(|i| **i >= hunks.len()) {
    anyhow::bail!("accepted hunk {bad} does not exist ({} hunks)", hunks.len());
  }
  // `split_inclusive` keeps each line's `\n`, so joining the slices back is lossless,
  // including a missing final newline.
  let cur: Vec<&str> = current.split_inclusive('\n').collect();
  let new: Vec<&str> = proposed.split_inclusive('\n').collect();
  let mut out = String::new();
  let mut cursor = 0;
  for (i, h) in hunks.iter().enumerate() {
    out.extend(cur[cursor..h.current[0]].iter().copied());
    let side = if accepted.contains(&i) {
      &new[h.proposed[0]..h.proposed[1]]
    } else {
      &cur[h.current[0]..h.current[1]]
    };
    out.extend(side.iter().copied());
    cursor = h.current[1];
  }
  out.extend(cur[cursor..].iter().copied());
  Ok(out)
}

#[cfg(test)]
mod tests {
  use super::*;

  const CUR: &str = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
  const NEW: &str = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n";

  #[test]
  fn hunks_exclude_context_and_split_far_apart_changes() {
    assert_eq!(
      hunks(CUR, NEW),
      vec![
        Hunk { current: [1, 2], proposed: [1, 2] },
        Hunk { current: [9, 10], proposed: [9, 11] },
      ]
    );
  }

  #[test]
  fn hunks_for_pure_insert_delete_and_no_change() {
    assert_eq!(hunks("a\nb\n", "a\nx\nb\n"), vec![Hunk { current: [1, 1], proposed: [1, 2] }]);
    assert_eq!(hunks("a\nx\nb\n", "a\nb\n"), vec![Hunk { current: [1, 2], proposed: [1, 1] }]);
    assert!(hunks("a\n", "a\n").is_empty());
  }

  #[test]
  fn assemble_reverts_rejected_hunks() {
    let h = hunks(CUR, NEW);
    assert_eq!(assemble(CUR, NEW, &h, &[0, 1]).unwrap(), NEW);
    assert_eq!(assemble(CUR, NEW, &h, &[]).unwrap(), CUR);
    assert_eq!(assemble(CUR, NEW, &h, &[1]).unwrap(), "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\nK\n");
    assert_eq!(assemble(CUR, NEW, &h, &[0]).unwrap(), "a\nB\nc\nd\ne\nf\ng\nh\ni\nj\n");
    assert!(assemble(CUR, NEW, &h, &[2]).is_err());
  }

  #[test]
  fn assemble_keeps_a_missing_final_newline() {
    let (cur, new) = ("a\nb", "a\nc");
    let h = hunks(cur, new);
    assert_eq!(assemble(cur, new, &h, &[0]).unwrap(), "a\nc");
    assert_eq!(assemble(cur, new, &h, &[]).unwrap(), "a\nb");
  }
}
```

Add `pub mod hunks;` to `docker/mod.rs` after `pub mod compose_rules;`.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p aeth-devkit-setup docker::hunks`
Expected: 4 passed. If `hunks_exclude_context_and_split_far_apart_changes` fails on the second hunk's shape, print `TextDiff::from_lines(CUR, NEW).grouped_ops(3)` and adjust only the test's expectation if `similar` reports the change as separate `Delete`+`Insert` ops with the same bounds (the bounds are what matter).

- [ ] **Step 3: Commit**

```bash
git add crates/aeth-devkit-setup/src/docker/hunks.rs crates/aeth-devkit-setup/src/docker/mod.rs
git commit -m "feat(setup): hunk table and partial reassembly for Docker consent" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 2: Consent protocol types and the `Reviewer` seam

**Files:**
- Create: `crates/aeth-devkit-setup/src/vscode/mod.rs` (for now only `pub mod protocol;`)
- Create: `crates/aeth-devkit-setup/src/vscode/protocol.rs`
- Modify: `crates/aeth-devkit-setup/src/lib.rs` (add `pub mod vscode;` in the alphabetical `pub mod` list)

**Interfaces:**
- Consumes: `docker::hunks::{Hunk, hunks}`, `docker::static_files::normalize_newlines`.
- Produces: `PROTOCOL: u32`, `MIN_EXTENSION_VERSION: u32`, `EXTENSION_ID: &str`, `Proposal { title, question, current, proposed, hunks }` + `Proposal::new(title, question, current, proposed)`, `Request` (serde), `Response` enum (serde, tag `decision`), `trait Reviewer { fn review(&self, proposal: &Proposal, offer_replace_all: bool) -> Result<Response>; }`, `ScriptedReviewer::new(Vec<Response>)` with `reviewed: RefCell<Vec<String>>`.

- [ ] **Step 1: Write the module with tests**

`crates/aeth-devkit-setup/src/vscode/mod.rs`:

```rust
//! VS Code integration for `setup-project`: an in-editor diff with per-hunk consent in
//! place of the typed terminal prompt, when the run happens inside a VS Code terminal.

pub mod protocol;
```

`crates/aeth-devkit-setup/src/vscode/protocol.rs`:

```rust
//! The consent protocol between the CLI and the extension: what the CLI asks, what the
//! extension answers, and the [`Reviewer`] seam the Docker step calls so tests never need
//! an editor. The CLI owns every decision and every byte written; the extension only
//! reports what the user chose.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::docker::hunks::{self, Hunk};
use crate::docker::static_files::normalize_newlines;

/// Bumped only when the request or response shape changes.
pub const PROTOCOL: u32 = 1;
/// The first extension build (`N` of `vscode-extension-vN`) that speaks [`PROTOCOL`].
pub const MIN_EXTENSION_VERSION: u32 = 1;
pub const EXTENSION_ID: &str = "aeth.aeth-devkit";

/// One change the CLI wants consent for. Texts are LF-normalised (see `docker::hunks`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
  /// Diff title: `docker/Dockerfile`, `docker/compose.yaml: service web`.
  pub title: String,
  /// The terminal question, used when VS Code is absent or the diff was dismissed.
  pub question: String,
  pub current: String,
  pub proposed: String,
  pub hunks: Vec<Hunk>,
}

impl Proposal {
  pub fn new(title: impl Into<String>, question: impl Into<String>, current: &str, proposed: &str) -> Self {
    let current = normalize_newlines(current);
    let proposed = normalize_newlines(proposed);
    let hunks = hunks::hunks(&current, &proposed);
    Self {
      title: title.into(),
      question: question.into(),
      current,
      proposed,
      hunks,
    }
  }
}

/// `<id>.request.json`, as the extension reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
  pub protocol: u32,
  pub id: String,
  pub title: String,
  pub current_path: PathBuf,
  pub proposed_path: PathBuf,
  pub hunks: Vec<Hunk>,
  pub offer_replace_all: bool,
  pub content_menu: bool,
  pub response_path: PathBuf,
}

/// `<id>.response.json`. `Dismissed` and `Error` are not decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Response {
  Replace,
  ReplaceAll,
  Keep,
  Partial { accepted: Vec<usize> },
  Dismissed,
  Error { message: String },
}

/// Shows one proposal and waits for the answer. `Err` is a transport failure (the request
/// could not be delivered or the answer not read); the caller then stops using the
/// reviewer for the rest of the run.
pub trait Reviewer {
  fn review(&self, proposal: &Proposal, offer_replace_all: bool) -> Result<Response>;
}

/// Answers from a queue and records every title; for tests.
pub struct ScriptedReviewer {
  pub answers: RefCell<VecDeque<Response>>,
  pub reviewed: RefCell<Vec<String>>,
}

impl ScriptedReviewer {
  pub fn new(answers: Vec<Response>) -> Self {
    Self {
      answers: RefCell::new(answers.into()),
      reviewed: RefCell::new(Vec::new()),
    }
  }
}

impl Reviewer for ScriptedReviewer {
  fn review(&self, proposal: &Proposal, _offer_replace_all: bool) -> Result<Response> {
    self.reviewed.borrow_mut().push(proposal.title.clone());
    self
      .answers
      .borrow_mut()
      .pop_front()
      .ok_or_else(|| anyhow::anyhow!("no scripted answer for {}", proposal.title))
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn response_json_matches_the_spec() {
    let parse = |s: &str| serde_json::from_str::<Response>(s).unwrap();
    assert_eq!(parse(r#"{"decision":"replace"}"#), Response::Replace);
    assert_eq!(parse(r#"{"decision":"replace_all"}"#), Response::ReplaceAll);
    assert_eq!(parse(r#"{"decision":"keep"}"#), Response::Keep);
    assert_eq!(
      parse(r#"{"decision":"partial","accepted":[0,2]}"#),
      Response::Partial { accepted: vec![0, 2] }
    );
    assert_eq!(parse(r#"{"decision":"dismissed"}"#), Response::Dismissed);
    assert_eq!(
      parse(r#"{"decision":"error","message":"old"}"#),
      Response::Error { message: "old".into() }
    );
    assert!(serde_json::from_str::<Response>(r#"{"decision":"maybe"}"#).is_err());
    assert_eq!(serde_json::to_string(&Response::Keep).unwrap(), r#"{"decision":"keep"}"#);
  }

  #[test]
  fn request_json_uses_the_spec_field_names() {
    let r = Request {
      protocol: PROTOCOL,
      id: "1-0".into(),
      title: "t".into(),
      current_path: "c".into(),
      proposed_path: "p".into(),
      hunks: vec![Hunk { current: [0, 1], proposed: [0, 2] }],
      offer_replace_all: true,
      content_menu: false,
      response_path: "r".into(),
    };
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["hunks"][0]["proposed"], serde_json::json!([0, 2]));
    assert_eq!(v["offer_replace_all"], serde_json::json!(true));
    assert_eq!(v["content_menu"], serde_json::json!(false));
    assert_eq!(v["response_path"], serde_json::json!("r"));
    assert_eq!(serde_json::from_value::<Request>(v).unwrap(), r);
  }

  #[test]
  fn proposal_normalises_line_endings_and_computes_hunks() {
    let p = Proposal::new("t", "q?", "a\r\nb\r\n", "a\nc\n");
    assert_eq!(p.current, "a\nb\n");
    assert_eq!(p.hunks, vec![Hunk { current: [1, 2], proposed: [1, 2] }]);
  }

  #[test]
  fn scripted_reviewer_records_titles_and_runs_dry() {
    let r = ScriptedReviewer::new(vec![Response::Keep]);
    let p = Proposal::new("t", "q", "a\n", "b\n");
    assert_eq!(r.review(&p, true).unwrap(), Response::Keep);
    assert!(r.review(&p, true).is_err());
    assert_eq!(*r.reviewed.borrow(), vec!["t", "t"]);
  }
}
```

Add `pub mod vscode;` to `crates/aeth-devkit-setup/src/lib.rs` after `pub mod toml_merge;`.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p aeth-devkit-setup vscode::protocol`
Expected: 4 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/aeth-devkit-setup/src/vscode crates/aeth-devkit-setup/src/lib.rs
git commit -m "feat(setup): consent protocol types and the Reviewer seam" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---
### Task 3: `Consent::decide`, per-service compose diffs, and the add question's removal

Every Docker change becomes a `Proposal` decided by `Consent::decide`, which consults the reviewer (VS Code) first and the terminal prompt second. The compose file is diffed per service and once for the top level, each against the text as accepted so far. `Consent::add` and `Deps.interactive` go away: adding a service is accepting its one-hunk diff, and `replace all` / `--replace-docker` accept adds too.

**Files:**
- Modify: `crates/aeth-devkit-setup/src/docker/mod.rs`
- Modify: `crates/aeth-devkit-setup/src/docker/static_files.rs:66-75`
- Modify: `crates/aeth-devkit-setup/src/cli.rs:57-71`
- Modify: `crates/aeth-devkit-setup/tests/docker.rs`

**Interfaces:**
- Consumes: `vscode::protocol::{Proposal, Response, Reviewer, ScriptedReviewer}`, `docker::hunks::assemble`.
- Produces: `Deps { runner, prompt, reviewer: Option<&dyn Reviewer>, mode }`, `Consent::new(prompt, reviewer, mode)`, `Consent::decide(&self, &Proposal) -> Result<Decision>`, `enum Decision { Keep, Replace, Partial { text, accepted, total } }`, `Decision::text(self, &Proposal) -> Option<String>`, `Decision::detail(&self, full: &str) -> String`.

- [ ] **Step 1: Rewrite `Consent`, `Deps`, and `compose` in `docker/mod.rs`**

Replace everything from `/// The injectable collaborators` through the end of `fn compose` with:

```rust
/// The injectable collaborators, in the style of the release and pin crates.
pub struct Deps<'a> {
  pub runner: &'a dyn Runner,
  pub prompt: &'a dyn Prompt,
  /// The VS Code reviewer when one is available; consulted before the terminal prompt.
  pub reviewer: Option<&'a dyn Reviewer>,
  pub mode: Mode,
}

/// What the user decided about one [`Proposal`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
  Keep,
  Replace,
  /// The proposed text with the rejected hunks reverted, assembled by the CLI.
  Partial { text: String, accepted: usize, total: usize },
}

impl Decision {
  /// The text to write, or `None` to keep the file as it is.
  pub fn text(self, proposal: &Proposal) -> Option<String> {
    match self {
      Decision::Keep => None,
      Decision::Replace => Some(proposal.proposed.clone()),
      Decision::Partial { text, .. } => Some(text),
    }
  }

  /// The change-report line: `full` for a replace, with the hunk count for a partial.
  pub fn detail(&self, full: &str) -> String {
    match self {
      Decision::Partial { accepted, total, .. } => format!("{full} ({accepted} of {total} hunks)"),
      _ => full.to_string(),
    }
  }
}

/// Consent state for one run. `Cell` because `replace all` upgrades the mode through a
/// shared reference: the same `&Consent` is handed to every step. The reviewer is dropped
/// after a transport error so a broken VS Code costs one note, not one per file.
pub struct Consent<'a> {
  prompt: &'a dyn Prompt,
  reviewer: Cell<Option<&'a dyn Reviewer>>,
  mode: Cell<Mode>,
  declined_silently: Cell<bool>,
}

impl<'a> Consent<'a> {
  pub fn new(prompt: &'a dyn Prompt, reviewer: Option<&'a dyn Reviewer>, mode: Mode) -> Self {
    Self {
      prompt,
      reviewer: Cell::new(reviewer),
      mode: Cell::new(mode),
      declined_silently: Cell::new(false),
    }
  }

  /// Decide one proposal whose diff was just printed: VS Code first when a reviewer is
  /// present, then the terminal. `dismissed` falls back to the terminal for this file
  /// only; an error or a malformed answer retires the reviewer for the run.
  pub fn decide(&self, p: &Proposal) -> Result<Decision> {
    match self.mode.get() {
      Mode::ReplaceAll | Mode::DryRun => return Ok(Decision::Replace),
      Mode::KeepAll => {
        self.declined_silently.set(true);
        return Ok(Decision::Keep);
      }
      Mode::Ask => {}
    }
    if let Some(r) = self.reviewer.get() {
      match r.review(p, true) {
        Ok(Response::Replace) => return Ok(Decision::Replace),
        Ok(Response::ReplaceAll) => {
          self.mode.set(Mode::ReplaceAll);
          return Ok(Decision::Replace);
        }
        Ok(Response::Keep) => return Ok(Decision::Keep),
        Ok(Response::Partial { accepted }) => match partial(p, &accepted) {
          Ok(d) => return Ok(d),
          Err(e) => self.retire_reviewer(&format!("{e:#}")),
        },
        Ok(Response::Dismissed) => println!("Diff closed in VS Code; answer here instead."),
        Ok(Response::Error { message }) => self.retire_reviewer(&message),
        Err(e) => self.retire_reviewer(&format!("{e:#}")),
      }
    }
    Ok(match self.prompt.ask(&p.question)?.as_str() {
      "replace" => Decision::Replace,
      "replace all" => {
        self.mode.set(Mode::ReplaceAll);
        Decision::Replace
      }
      _ => Decision::Keep,
    })
  }

  fn retire_reviewer(&self, why: &str) {
    self.reviewer.set(None);
    println!("note: VS Code review unavailable ({why}); using the terminal prompt for the rest of the run.");
  }

  /// A change was kept only because nobody could be asked.
  pub fn kept_silently(&self) -> bool {
    self.declined_silently.get()
  }
}

/// A partial answer with every hunk is a replace and with none a keep, so the report and
/// the terminal flow see the same three outcomes.
fn partial(p: &Proposal, accepted: &[usize]) -> Result<Decision> {
  let mut accepted = accepted.to_vec();
  accepted.sort_unstable();
  accepted.dedup();
  let text = hunks::assemble(&p.current, &p.proposed, &p.hunks, &accepted)?;
  Ok(match accepted.len() {
    0 => Decision::Keep,
    n if n == p.hunks.len() => Decision::Replace,
    n => Decision::Partial {
      text,
      accepted: n,
      total: p.hunks.len(),
    },
  })
}

/// Everything Docker: static files first, then the compose file, then advisories.
pub fn apply(ctx: &ProjectContext, templates_dir: &Path, deps: &Deps, changes: &mut Changes) -> Result<()> {
  let consent = Consent::new(deps.prompt, deps.reviewer, deps.mode);
  static_files::apply(ctx, templates_dir, &consent, changes)?;
  compose(ctx, templates_dir, deps.runner, &consent, changes)?;
  if consent.kept_silently() {
    changes
      .notes
      .push("Docker files were left alone because no terminal was available to confirm; pass --replace-docker to apply them.".into());
  }
  Ok(())
}

/// The compose file: created whole from the scaffold when absent; otherwise one diff per
/// listed service (its rule-engine edits, or its scaffold block when the file lacks it)
/// and one for the top-level keys. Each diff is computed against the text as accepted so
/// far, so a hunk never straddles two services and a partial answer leaves later line
/// numbers valid.
fn compose(ctx: &ProjectContext, templates_dir: &Path, runner: &dyn Runner, consent: &Consent, changes: &mut Changes) -> Result<()> {
  use aeth_devkit_core::compose::find_compose_file;
  use aeth_devkit_core::compose::tree::{self, Edit};

  let sc = scaffold::load(templates_dir, ctx)?;
  let tag = scaffold::GitTag::new(runner, ctx);
  let Some(path) = find_compose_file(&ctx.root)? else {
    let text = tag.fill(&scaffold::render_file(&sc, &ctx.docker_services));
    changes.record_optional(
      &ctx.root.join("docker").join("compose.yaml"),
      None,
      &text,
      vec!["created from template".into()],
    )?;
    changes.notes.extend(tag.note());
    return Ok(());
  };
  let rel = path
    .strip_prefix(&ctx.root)
    .map(|p| p.to_string_lossy().replace('\\', "/"))
    .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));
  let original = std::fs::read_to_string(&path).with_context(|| format!("reading {rel}"))?;
  let mut text = original.clone();
  let mut details: Vec<String> = Vec::new();
  // One diff and one decision; `text` advances only on replace or partial. A closure
  // (not a fn) so it can share `consent`, `tag`, `rel` and `details` without a struct.
  let mut ask = |text: &mut String, what: &str, question: String, edits: &[Edit], edit_details: Vec<String>| -> Result<()> {
    if edits.is_empty() {
      return Ok(());
    }
    let proposed = tag.fill(&tree::apply_edits(text, edits));
    let title = format!("{rel}: {what}");
    println!("{}", static_files::unified_diff(&title, text, &proposed));
    let proposal = Proposal::new(&title, question, text, &proposed);
    match consent.decide(&proposal)? {
      Decision::Keep => println!("Kept {title}."),
      Decision::Replace => {
        *text = proposal.proposed;
        details.extend(edit_details);
      }
      Decision::Partial { text: t, accepted, total } => {
        *text = t;
        details.push(format!("{what}: {accepted} of {total} hunks applied"));
      }
    }
    Ok(())
  };
  let keywords = "[replace / replace all / anything else keeps it]:";
  for name in &ctx.docker_services {
    let lines = tree::split_lines(&text);
    let Some(services) = tree::top_level(&lines, "services") else {
      bail!("{rel} has no top-level `services:` key");
    };
    // The scaffold block for *this* service, parsed as its own little document so the
    // rule engine can look keys up in it exactly like in the project file.
    let sc_doc = tree::split_lines(&format!("services:\n{}", scaffold::service_block(&sc, name)));
    let sc_services = tree::top_level(&sc_doc, "services").expect("scaffold starts with services:");
    let sc_svc = tree::child(&sc_doc, &sc_services, name).expect("scaffold block names the service");
    match tree::child(&lines, &services, name) {
      Some(svc) => {
        let o = compose_rules::service_edits(&lines, &svc, &sc_doc, &sc_svc, name);
        ask(&mut text, &format!("service {name}"), format!("Apply the {name} edits to {rel}? {keywords}"), &o.edits, o.details)?;
      }
      None => {
        let indent = tree::child_indent(&lines, &services);
        let mut block = tree::re_indent(&sc_doc[sc_svc.line..sc_svc.end], sc_svc.indent, indent);
        // One blank line between service blocks, matching the sister files.
        if services.end > 0 && !lines[services.end - 1].trim().is_empty() {
          block.insert(0, String::new());
        }
        let edit = Edit::Insert {
          at: services.end,
          lines: block,
        };
        ask(&mut text, &format!("new service {name}"), format!("Add service {name} to {rel}? {keywords}"), &[edit], vec![format!("added service {name}")])?;
      }
    }
  }
  let lines = tree::split_lines(&text);
  let o = compose_rules::top_level_edits(&lines, &tree::split_lines(&sc.tail));
  ask(&mut text, "top level", format!("Apply the top-level edits to {rel}? {keywords}"), &o.edits, o.details)?;
  drop(ask);
  if text == original {
    changes.record_optional(&path, Some(&original), &original, vec![])?;
    return Ok(());
  }
  changes.record_optional(&path, Some(&original), &text, details)?;
  changes.notes.extend(tag.note());
  Ok(())
}
```

Update the imports at the top of the file: keep `use aeth_devkit_core::prompt::Prompt;`, and add

```rust
use crate::vscode::protocol::{Proposal, Response, Reviewer};
```

Delete the old `Deps.interactive` doc comment and field, the old `Consent::replace`, and `Consent::add`. The `Mode` enum is unchanged.

- [ ] **Step 2: Replace the consent unit tests in `docker/mod.rs`**

Replace the whole `mod consent_tests` with:

```rust
#[cfg(test)]
mod consent_tests {
  use super::*;
  use crate::vscode::protocol::ScriptedReviewer;
  use aeth_devkit_core::prompt::ScriptedPrompt;

  fn proposal(title: &str) -> Proposal {
    Proposal::new(title, format!("{title}?"), "a\nb\n", "a\nc\n")
  }

  #[test]
  fn replace_all_sticks_for_the_rest_of_the_run() {
    let p = ScriptedPrompt::new(&["replace all"]);
    let c = Consent::new(&p, None, Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Replace, "no second question");
    assert_eq!(p.asked.borrow().len(), 1);
  }

  #[test]
  fn anything_but_the_keywords_keeps() {
    let p = ScriptedPrompt::new(&["replace", "y", ""]);
    let c = Consent::new(&p, None, Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("c")).unwrap(), Decision::Keep);
  }

  #[test]
  fn dry_run_and_keep_all_never_ask() {
    let p = ScriptedPrompt::new(&[]);
    let dry = Consent::new(&p, None, Mode::DryRun);
    assert_eq!(dry.decide(&proposal("a")).unwrap(), Decision::Replace);
    let keep = Consent::new(&p, None, Mode::KeepAll);
    assert_eq!(keep.decide(&proposal("a")).unwrap(), Decision::Keep);
    assert!(keep.kept_silently());
    let all = Consent::new(&p, None, Mode::ReplaceAll);
    assert_eq!(all.decide(&proposal("a")).unwrap(), Decision::Replace);
    assert!(p.asked.borrow().is_empty());
  }

  #[test]
  fn reviewer_answers_first_and_dismissed_falls_back_per_file() {
    let p = ScriptedPrompt::new(&["replace"]);
    let r = ScriptedReviewer::new(vec![Response::Keep, Response::Dismissed, Response::ReplaceAll]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Replace, "dismissed, terminal said replace");
    assert_eq!(c.decide(&proposal("c")).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&proposal("d")).unwrap(), Decision::Replace, "replace all from VS Code sticks");
    assert_eq!(p.asked.borrow().len(), 1);
    assert_eq!(*r.reviewed.borrow(), vec!["a", "b", "c"]);
  }

  #[test]
  fn partial_assembles_text_and_collapses_to_replace_or_keep() {
    let p = ScriptedPrompt::new(&[]);
    let cur = "a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n";
    let new = "a\nB\nc\nd\ne\nf\ng\nh\ni\nJ\n";
    let prop = Proposal::new("t", "q", cur, new);
    assert_eq!(prop.hunks.len(), 2);
    let r = ScriptedReviewer::new(vec![
      Response::Partial { accepted: vec![1, 1] },
      Response::Partial { accepted: vec![0, 1] },
      Response::Partial { accepted: vec![] },
    ]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(
      c.decide(&prop).unwrap(),
      Decision::Partial {
        text: "a\nb\nc\nd\ne\nf\ng\nh\ni\nJ\n".into(),
        accepted: 1,
        total: 2
      }
    );
    assert_eq!(c.decide(&prop).unwrap(), Decision::Replace);
    assert_eq!(c.decide(&prop).unwrap(), Decision::Keep);
    assert_eq!(Decision::Partial { text: String::new(), accepted: 1, total: 2 }.detail("replaced"), "replaced (1 of 2 hunks)");
  }

  #[test]
  fn a_broken_reviewer_is_retired_after_one_note() {
    let p = ScriptedPrompt::new(&["", ""]);
    let r = ScriptedReviewer::new(vec![Response::Error { message: "protocol 9".into() }]);
    let c = Consent::new(&p, Some(&r), Mode::Ask);
    assert_eq!(c.decide(&proposal("a")).unwrap(), Decision::Keep);
    assert_eq!(c.decide(&proposal("b")).unwrap(), Decision::Keep);
    assert_eq!(r.reviewed.borrow().len(), 1, "not consulted again");
    assert_eq!(p.asked.borrow().len(), 2);
    let bad = ScriptedReviewer::new(vec![Response::Partial { accepted: vec![7] }]);
    let c = Consent::new(&p, Some(&bad), Mode::Ask);
    assert!(c.decide(&proposal("a")).is_err(), "prompt queue is empty, so the fallback prompt errors: proves the reviewer was retired");
  }
}
```

- [ ] **Step 3: Use `decide` in `static_files.rs`**

Replace the block from `println!("{}", unified_diff(&rel, &original, &rendered));` through the matching `}` of the `if consent.replace(...)` / `else` with:

```rust
    println!("{}", unified_diff(&rel, &original, &rendered));
    let proposal = Proposal::new(&rel, format!("Replace {rel}? [replace / replace all / anything else keeps it]:"), &original, &rendered);
    let decision = consent.decide(&proposal)?;
    let detail = decision.detail("replaced with the devkit template");
    match decision.text(&proposal) {
      Some(text) => changes.record_optional(&path, Some(&original), &text, vec![detail])?,
      None => {
        changes.record_optional(&path, Some(&original), &original, vec![])?;
        println!("Kept {rel}.");
      }
    }
```

and add `use crate::vscode::protocol::Proposal;` to the imports.

- [ ] **Step 4: Fix `cli.rs` and the integration test helper so the crate compiles**

In `cli.rs`, the `Deps` literal becomes (the `reviewer` is wired for real in Task 8):

```rust
    let deps = crate::docker::Deps {
      runner: &aeth_devkit_core::process::SystemRunner,
      prompt: &aeth_devkit_core::prompt::StdinPrompt,
      reviewer: None,
      mode: match (dry_run, args.replace_docker, tty) {
        (true, _, _) => crate::docker::Mode::DryRun,
        (false, true, _) => crate::docker::Mode::ReplaceAll,
        (false, false, true) => crate::docker::Mode::Ask,
        (false, false, false) => crate::docker::Mode::KeepAll,
      },
    };
```

In `tests/docker.rs`, the helper drops `interactive`:

```rust
fn run(root: &Path, mode: Mode, answers: &[&str], dry_run: bool) -> (Changes, ScriptedPrompt, RecordingRunner) {
  let prompt = ScriptedPrompt::new(answers);
  let runner = RecordingRunner::new(0);
  runner.script("gh", &["api"], 0, "v1.1.0\nv1.0.0\n");
  let changes = {
    let deps = Deps {
      runner: &runner,
      prompt: &prompt,
      reviewer: None,
      mode,
    };
    aeth_devkit_setup::run_with(root, &templates(), dry_run, &deps).unwrap()
  };
  (changes, prompt, runner)
}
```

Remove the third argument (`true` / `false`) from every other `run(` call in the file.

- [ ] **Step 5: Rewrite the behavioural integration tests**

In `tests/docker.rs`, replace `a_missing_service_is_added_only_on_add_and_sidecars_are_untouched` with:

```rust
#[test]
fn a_missing_service_is_its_own_diff_and_sidecars_are_untouched() {
  let dir = project(&["demo-app", "worker"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(
    root,
    "docker/compose.yaml",
    "services:\n  wireguard:\n    image: wg\n  demo-app:\n    container_name: demo-app\n",
  );
  // demo-app edits: replace; worker add: keep; top level: replace.
  let (_, prompt, _) = run(root, Mode::Ask, &["replace", "", "replace"], false);
  let asked = prompt.asked.borrow().clone();
  assert_eq!(
    asked[0],
    "Apply the demo-app edits to docker/compose.yaml? [replace / replace all / anything else keeps it]:"
  );
  assert_eq!(
    asked[1],
    "Add service worker to docker/compose.yaml? [replace / replace all / anything else keeps it]:"
  );
  assert!(asked[2].starts_with("Apply the top-level edits"), "{asked:?}");
  let out = read(root, "docker/compose.yaml");
  assert!(!out.contains("  worker:"), "kept: {out}");
  assert!(out.contains("  wireguard:\n    image: wg\n"), "sidecar untouched: {out}");
  assert!(out.contains("  demo-app:\n    container_name: demo-app\n    build:\n"), "{out}");

  let (_, prompt, _) = run(root, Mode::Ask, &["replace"], false);
  assert_eq!(prompt.asked.borrow().len(), 1, "only the add remains: {:?}", prompt.asked.borrow());
  let out = read(root, "docker/compose.yaml");
  assert!(out.contains("\n  worker:\n    container_name: worker\n"), "{out}");
  assert!(out.contains("GIT_TAG: v1.1.0"), "{out}");
}

#[test]
fn replace_all_and_replace_docker_add_missing_services() {
  let dir = project(&["demo-app", "worker"], "https://github.com/O/Demo.git");
  let root = dir.path();
  write(root, "docker/compose.yaml", "services:\n  demo-app:\n    container_name: demo-app\n");
  let (_, prompt, _) = run(root, Mode::Ask, &["replace all"], false);
  assert_eq!(prompt.asked.borrow().len(), 1);
  assert!(read(root, "docker/compose.yaml").contains("\n  worker:\n"));

  write(root, "docker/compose.yaml", "services:\n  demo-app:\n    container_name: demo-app\n");
  let (changes, _, _) = run(root, Mode::ReplaceAll, &[], false);
  assert!(read(root, "docker/compose.yaml").contains("\n  worker:\n"), "{}", changes.report(root));
}

#[test]
fn a_partial_answer_from_the_reviewer_writes_the_assembled_text() {
  use aeth_devkit_setup::vscode::protocol::{Response, ScriptedReviewer};
  let dir = project(&["demo-app"], "https://github.com/O/Demo.git");
  let root = dir.path();
  run(root, Mode::Ask, &[], false);
  let good = read(root, "docker/Dockerfile");
  // Two edits more than six lines apart, so `similar` reports two hunks.
  write(root, "docker/Dockerfile", &(good.replace("PYTHONOPTIMIZE=1", "PYTHONOPTIMIZE=2") + "# trailing\n"));
  let prompt = ScriptedPrompt::new(&[]);
  let runner = RecordingRunner::new(0);
  let reviewer = ScriptedReviewer::new(vec![Response::Partial { accepted: vec![0] }]);
  let deps = Deps {
    runner: &runner,
    prompt: &prompt,
    reviewer: Some(&reviewer),
    mode: Mode::Ask,
  };
  let changes = aeth_devkit_setup::run_with(root, &templates(), false, &deps).unwrap();
  let out = read(root, "docker/Dockerfile");
  assert!(out.contains("PYTHONOPTIMIZE=1") && out.ends_with("# trailing\n"), "{out}");
  assert!(
    changes.files.iter().any(|f| f.details.iter().any(|d| d.contains("1 of 2 hunks"))),
    "{}",
    changes.report(root)
  );
  assert!(prompt.asked.borrow().is_empty());
  assert_eq!(*reviewer.reviewed.borrow(), vec!["docker/Dockerfile"]);
}
```

`replace_all_covers_the_compose_edits_too` keeps its assertions (still one question). Check that `imap_fixture_with_injected_drift_gets_exactly_the_standard_edits` and `aeth_ext_fixture_is_already_compliant` still hold: their answers now go per service, so where they answered `"replace"` once for the compose file, supply one `"replace"` per service that has edits plus one for the top level if it has edits. Run them, read the recorded questions from the failure output, and set the answer list to match; keep the assertions on the resulting file untouched.

- [ ] **Step 6: Run the Docker tests**

Run: `cargo test -p aeth-devkit-setup docker`
Expected: all unit tests in `docker::` pass and every test in `tests/docker.rs` passes. Also `cargo clippy -p aeth-devkit-setup --all-targets` shows no new warnings.

- [ ] **Step 7: Commit**

```bash
git add crates/aeth-devkit-setup/src/docker crates/aeth-devkit-setup/src/cli.rs crates/aeth-devkit-setup/tests/docker.rs
git commit -m "feat(setup-project): per-service Docker diffs decided through a Reviewer seam" -m "Every Docker change is a Proposal decided by Consent::decide, which asks a VS Code reviewer before the terminal and reassembles partial answers itself. The compose file is diffed per service and once for the top level; adding a service is accepting its one-hunk diff, so the separate add question and Deps.interactive are gone." -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---
### Task 4: Cache dir, launcher lookup, `argv.json` grant, stray-extension notes

**Files:**
- Modify: `crates/aeth-devkit-core/src/update.rs:111-127`
- Modify: `crates/aeth-devkit-setup/src/vscode/mod.rs`

**Interfaces:**
- Produces (core): `aeth_devkit_core::update::cache_dir() -> Option<PathBuf>`.
- Produces (setup): `vscode::find_launcher(path: &OsStr) -> Option<PathBuf>`, `vscode::home_dir() -> Option<PathBuf>`, `vscode::ARGV_KEY`, `vscode::grant_proposal(argv: Option<&str>) -> Result<Option<String>>` (`Ok(None)` = already granted), `vscode::stray_notes(home: &Path, project_root: &Path) -> Vec<String>`.

- [ ] **Step 1: Split `cache_dir` out of `cache_path` in core**

In `update.rs`, replace `cache_path` with:

```rust
/// This user's devkit cache directory: `%LOCALAPPDATA%\aeth-devkit` on Windows, else
/// `$XDG_CACHE_HOME/aeth-devkit` (default `~/.cache/aeth-devkit`). The VS Code extension
/// computes the same path, so the two find each other's files without configuration.
pub fn cache_dir() -> Option<PathBuf> {
  let base = if cfg!(windows) {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)?
  } else {
    match std::env::var_os("XDG_CACHE_HOME") {
      Some(x) => PathBuf::from(x),
      None => PathBuf::from(std::env::var_os("HOME")?).join(".cache"),
    }
  };
  Some(base.join("aeth-devkit"))
}

/// Where the update-check cache lives: [`CACHE_ENV`] if set, else
/// `update-check.json` under [`cache_dir`].
pub fn cache_path() -> Option<PathBuf> {
  if let Some(p) = std::env::var_os(CACHE_ENV) {
    return Some(PathBuf::from(p));
  }
  Some(cache_dir()?.join("update-check.json"))
}
```

Run: `cargo test -p aeth-devkit-core update` — Expected: existing tests pass.

- [ ] **Step 2: Write the helpers and tests in `vscode/mod.rs`**

Replace `vscode/mod.rs` with:

```rust
//! VS Code integration for `setup-project`: an in-editor diff with per-hunk consent in
//! place of the typed terminal prompt, when the run happens inside a VS Code terminal.
//! `prepare` (Task 7) runs the detection/install/grant pipeline; the pure pieces live here.

pub mod protocol;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use protocol::EXTENSION_ID;

/// The `argv.json` key that grants proposed API contributions to a listed extension.
pub const ARGV_KEY: &str = "enable-proposed-api";

/// `code` on `PATH`. On Windows the launcher is a `.cmd` shim and `Command` does not
/// apply `PATHEXT`, so the candidates are spelled out (std runs `.cmd` through `cmd.exe`
/// with its own argument escaping, which is why the URL we pass carries no `%`).
pub fn find_launcher(path: &OsStr) -> Option<PathBuf> {
  let names: &[&str] = if cfg!(windows) { &["code.cmd", "code.exe", "code"] } else { &["code"] };
  std::env::split_paths(path)
    .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
    .find(|p| p.is_file())
}

pub fn home_dir() -> Option<PathBuf> {
  let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
  std::env::var_os(var).map(PathBuf::from)
}

/// `argv.json` with the extension added to `enable-proposed-api`, or `None` when it is
/// already listed. Text surgery rather than a parse-and-print round trip: the file is VS
/// Code's, full of comments the JSON merge would drop. `rfind` for the key because a
/// comment mentioning it sits above the real entry, never below.
pub fn grant_proposal(argv: Option<&str>) -> Result<Option<String>> {
  let entry = format!("\"{EXTENSION_ID}\"");
  let Some(text) = argv else {
    return Ok(Some(format!("{{\n\t\"{ARGV_KEY}\": [{entry}]\n}}\n")));
  };
  let doc: serde_json::Value = serde_json::from_str(&crate::json_merge::strip_jsonc(text)).context("parsing argv.json")?;
  match doc.get(ARGV_KEY) {
    Some(serde_json::Value::Array(items)) => {
      if items.iter().any(|v| v.as_str() == Some(EXTENSION_ID)) {
        return Ok(None);
      }
      let key_at = text.rfind(&format!("\"{ARGV_KEY}\"")).context("argv.json: key not found in the text")?;
      let open = key_at + text[key_at..].find('[').context("argv.json: array not found")? + 1;
      let rest = &text[open..];
      let sep = if rest.trim_start().starts_with(']') {
        ""
      } else if rest.starts_with(char::is_whitespace) {
        ","
      } else {
        ", "
      };
      Ok(Some(format!("{}{entry}{sep}{rest}", &text[..open])))
    }
    Some(_) => bail!("argv.json: `{ARGV_KEY}` is not an array"),
    None => {
      let brace = text.find('{').context("argv.json has no object")? + 1;
      let comma = if doc.as_object().is_some_and(|o| !o.is_empty()) { "," } else { "" };
      let indent = text
        .lines()
        .find(|l| l.trim_start().starts_with('"'))
        .map(|l| l[..l.len() - l.trim_start().len()].to_string())
        .unwrap_or_else(|| "\t".into());
      Ok(Some(format!("{}\n{indent}\"{ARGV_KEY}\": [{entry}]{comma}{}", &text[..brace], &text[brace..])))
    }
  }
}

/// Leftovers of the Drekker extension this one replaces. Reported, never removed: the
/// extensions-dir entry is a junction into a sister project's working tree.
pub fn stray_notes(home: &Path, project_root: &Path) -> Vec<String> {
  let mut notes = Vec::new();
  let ext_dir = home.join(".vscode").join("extensions");
  if let Ok(entries) = std::fs::read_dir(&ext_dir) {
    for e in entries.flatten() {
      let name = e.file_name().to_string_lossy().into_owned();
      if name.starts_with("local.drekker-add-to-runtime-base") {
        notes.push(format!(
          "{} is the old Drekker extension junction; the devkit extension replaces it, so delete the junction (not its target).",
          ext_dir.join(&name).display()
        ));
      }
    }
  }
  if project_root.join(".vscode").join("extension").is_dir() {
    notes.push(".vscode/extension/ is the old Drekker extension source; the devkit extension replaces it, so it can be deleted.".into());
  }
  notes
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn finds_the_launcher_on_path_including_the_cmd_shim() {
    let dir = tempfile::tempdir().unwrap();
    let name = if cfg!(windows) { "code.cmd" } else { "code" };
    let empty = tempfile::tempdir().unwrap();
    let path = std::env::join_paths([empty.path(), dir.path()]).unwrap();
    assert_eq!(find_launcher(&path), None);
    std::fs::write(dir.path().join(name), "").unwrap();
    assert_eq!(find_launcher(&path), Some(dir.path().join(name)));
  }

  #[test]
  fn grant_creates_the_file_when_absent() {
    assert_eq!(
      grant_proposal(None).unwrap().unwrap(),
      "{\n\t\"enable-proposed-api\": [\"aeth.aeth-devkit\"]\n}\n"
    );
  }

  #[test]
  fn grant_inserts_the_key_after_the_brace_keeping_comments() {
    let argv = "// header\n{\n\t// Use software rendering.\n\t// \"disable-hardware-acceleration\": true,\n\t\"enable-crash-reporter\": true,\n\t\"crash-reporter-id\": \"x\"\n}\n";
    let out = grant_proposal(Some(argv)).unwrap().unwrap();
    assert_eq!(
      out,
      "// header\n{\n\t\"enable-proposed-api\": [\"aeth.aeth-devkit\"],\n\t// Use software rendering.\n\t// \"disable-hardware-acceleration\": true,\n\t\"enable-crash-reporter\": true,\n\t\"crash-reporter-id\": \"x\"\n}\n"
    );
    assert_eq!(grant_proposal(Some("{}")).unwrap().unwrap(), "{\n\t\"enable-proposed-api\": [\"aeth.aeth-devkit\"]}");
    assert_eq!(grant_proposal(Some(&out)).unwrap(), None, "second run: already granted");
  }

  #[test]
  fn grant_extends_an_existing_array() {
    assert_eq!(
      grant_proposal(Some("{\n  \"enable-proposed-api\": []\n}\n")).unwrap().unwrap(),
      "{\n  \"enable-proposed-api\": [\"aeth.aeth-devkit\"]\n}\n"
    );
    assert_eq!(
      grant_proposal(Some("{\"enable-proposed-api\": [\"other.ext\"]}")).unwrap().unwrap(),
      "{\"enable-proposed-api\": [\"aeth.aeth-devkit\", \"other.ext\"]}"
    );
    assert_eq!(
      grant_proposal(Some("{\n  \"enable-proposed-api\": [\n    \"other.ext\"\n  ]\n}\n")).unwrap().unwrap(),
      "{\n  \"enable-proposed-api\": [\"aeth.aeth-devkit\",\n    \"other.ext\"\n  ]\n}\n"
    );
    assert!(grant_proposal(Some("{\"enable-proposed-api\": true}")).is_err());
    assert!(grant_proposal(Some("not json")).is_err());
  }

  #[test]
  fn stray_notes_report_the_junction_and_the_project_folder() {
    let home = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    assert!(stray_notes(home.path(), root.path()).is_empty());
    std::fs::create_dir_all(home.path().join(".vscode/extensions/local.drekker-add-to-runtime-base-0.0.1")).unwrap();
    std::fs::create_dir_all(root.path().join(".vscode/extension")).unwrap();
    let notes = stray_notes(home.path(), root.path());
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert!(notes[0].contains("local.drekker-add-to-runtime-base-0.0.1") && notes[0].contains("junction"));
    assert!(notes[1].starts_with(".vscode/extension/"));
  }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p aeth-devkit-setup vscode::tests`
Expected: 5 passed. If `grant_inserts_the_key_after_the_brace_keeping_comments` fails on the `{}` case, the object-empty check is the culprit: `doc.as_object()` must be `Some(empty)` there and `comma` empty.

- [ ] **Step 4: Commit**

```bash
git add crates/aeth-devkit-core/src/update.rs crates/aeth-devkit-setup/src/vscode/mod.rs
git commit -m "feat(setup): VS Code launcher lookup, argv.json grant, and stray-extension notes" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 5: Extension install: tag resolution, download, `--install-extension`

**Files:**
- Create: `crates/aeth-devkit-setup/src/vscode/install.rs`
- Modify: `crates/aeth-devkit-setup/src/vscode/mod.rs` (add `pub mod install;`)

**Interfaces:**
- Consumes: `protocol::{EXTENSION_ID, MIN_EXTENSION_VERSION}`, `aeth_devkit_core::process::Runner`.
- Produces: `trait Fetch { fn get_text(&self, url: &str) -> Result<String>; fn download(&self, url: &str, dest: &Path) -> Result<()>; }`, `HttpFetch`, `StubFetch { bodies: HashMap<String, String>, downloads: RefCell<Vec<(String, PathBuf)>> }`, `REPO`, `TAG_PREFIX`, `refs_url()`, `vsix_url(n)`, `latest_tag_number(json) -> Result<Option<u32>>`, `installed_version(list_output) -> Option<u32>`, `enum Ensure { Ready, ReloadNeeded, Unavailable(String) }`, `ensure_extension(runner, fetch, launcher, cache, install) -> Ensure`.

- [ ] **Step 1: Write the module with tests**

```rust
//! Getting a compatible extension into VS Code: the newest `vscode-extension-vN` release
//! is fetched from GitHub (no auth: the repo is public and this runs once per install)
//! and handed to `code --install-extension`. A fresh install is live at once; an upgrade
//! over a loaded extension needs a window reload, which the caller reports and stops on.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;

use super::protocol::{EXTENSION_ID, MIN_EXTENSION_VERSION};

pub const REPO: &str = "AetherBreaker/aeth-devkit";
pub const TAG_PREFIX: &str = "vscode-extension-v";

pub fn refs_url() -> String {
  format!("https://api.github.com/repos/{REPO}/git/matching-refs/tags/{TAG_PREFIX}")
}

pub fn vsix_url(n: u32) -> String {
  format!("https://github.com/{REPO}/releases/download/{TAG_PREFIX}{n}/aeth-devkit-vscode-{n}.vsix")
}

/// Two HTTP verbs behind a trait so the install flow is testable without a network.
pub trait Fetch {
  fn get_text(&self, url: &str) -> Result<String>;
  fn download(&self, url: &str, dest: &Path) -> Result<()>;
}

pub struct HttpFetch;

impl HttpFetch {
  fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
      .timeout_global(Some(std::time::Duration::from_secs(60)))
      .http_status_as_error(false)
      .build()
      .into()
  }
}

impl Fetch for HttpFetch {
  fn get_text(&self, url: &str) -> Result<String> {
    // GitHub's API rejects requests without a User-Agent.
    let mut resp = Self::agent()
      .get(url)
      .header("User-Agent", "aeth-devkit")
      .header("Accept", "application/vnd.github+json")
      .call()
      .with_context(|| format!("fetching {url}"))?;
    if resp.status().as_u16() != 200 {
      bail!("HTTP {} from GET {url}", resp.status());
    }
    resp.body_mut().read_to_string().with_context(|| format!("reading {url}"))
  }

  fn download(&self, url: &str, dest: &Path) -> Result<()> {
    // Release assets redirect to a storage host; ureq follows redirects by default.
    let mut resp = Self::agent()
      .get(url)
      .header("User-Agent", "aeth-devkit")
      .call()
      .with_context(|| format!("downloading {url}"))?;
    if resp.status().as_u16() != 200 {
      bail!("HTTP {} from GET {url}", resp.status());
    }
    let bytes = resp.body_mut().read_to_vec().with_context(|| format!("reading {url}"))?;
    if let Some(parent) = dest.parent() {
      std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(dest, bytes).with_context(|| format!("writing {}", dest.display()))
  }
}

/// Canned bodies per URL; downloads write a stub file and are recorded. For tests.
#[derive(Default)]
pub struct StubFetch {
  pub bodies: HashMap<String, String>,
  pub downloads: RefCell<Vec<(String, PathBuf)>>,
}

impl Fetch for StubFetch {
  fn get_text(&self, url: &str) -> Result<String> {
    self.bodies.get(url).cloned().ok_or_else(|| anyhow::anyhow!("stub: no body for {url}"))
  }

  fn download(&self, url: &str, dest: &Path) -> Result<()> {
    self.downloads.borrow_mut().push((url.to_string(), dest.to_path_buf()));
    std::fs::create_dir_all(dest.parent().unwrap())?;
    std::fs::write(dest, b"vsix")?;
    Ok(())
  }
}

/// The highest `N` among `refs/tags/vscode-extension-vN` in a matching-refs response.
pub fn latest_tag_number(refs_json: &str) -> Result<Option<u32>> {
  let refs: Vec<serde_json::Value> = serde_json::from_str(refs_json).context("parsing the extension tag list")?;
  Ok(
    refs
      .iter()
      .filter_map(|r| r["ref"].as_str())
      .filter_map(|r| r.strip_prefix("refs/tags/")?.strip_prefix(TAG_PREFIX))
      .filter_map(|n| n.parse().ok())
      .max(),
  )
}

/// `N` from the `aeth.aeth-devkit@N.0.0` line of `code --list-extensions --show-versions`.
pub fn installed_version(list_output: &str) -> Option<u32> {
  list_output.lines().find_map(|l| {
    let (id, ver) = l.trim().split_once('@')?;
    if !id.eq_ignore_ascii_case(EXTENSION_ID) {
      return None;
    }
    ver.split('.').next()?.parse().ok()
  })
}

#[derive(Debug, PartialEq, Eq)]
pub enum Ensure {
  /// A compatible extension is installed and loaded.
  Ready,
  /// A newer extension was just installed over a loaded one; VS Code must reload first.
  ReloadNeeded,
  /// No compatible extension and none could be installed; the note says why.
  Unavailable(String),
}

/// Make sure a compatible extension is installed, installing the newest release when
/// `install` is set. Never fails: every problem becomes `Unavailable`.
pub fn ensure_extension(runner: &dyn Runner, fetch: &dyn Fetch, launcher: &Path, cache: &Path, install: bool) -> Ensure {
  match ensure(runner, fetch, launcher, cache, install) {
    Ok(e) => e,
    Err(e) => Ensure::Unavailable(format!("{e:#}")),
  }
}

fn ensure(runner: &dyn Runner, fetch: &dyn Fetch, launcher: &Path, cache: &Path, install: bool) -> Result<Ensure> {
  let code = launcher.to_string_lossy();
  let out = runner.run_capture(&code, &["--list-extensions".into(), "--show-versions".into()], Path::new("."))?;
  if !out.success() {
    bail!("`code --list-extensions` failed: {}", out.stderr.trim());
  }
  let installed = installed_version(&out.stdout);
  if installed.is_some_and(|n| n >= MIN_EXTENSION_VERSION) {
    return Ok(Ensure::Ready);
  }
  if !install {
    return Ok(Ensure::Unavailable(
      "the devkit VS Code extension is not installed (a run without --dry-run installs it)".into(),
    ));
  }
  let latest = latest_tag_number(&fetch.get_text(&refs_url())?)?;
  let Some(n) = latest.filter(|n| *n >= MIN_EXTENSION_VERSION) else {
    bail!("no compatible devkit VS Code extension release exists yet (need build {MIN_EXTENSION_VERSION})");
  };
  let vsix = cache.join("vsix").join(format!("aeth-devkit-vscode-{n}.vsix"));
  fetch.download(&vsix_url(n), &vsix)?;
  let args: Vec<String> = vec!["--install-extension".into(), vsix.to_string_lossy().into_owned(), "--force".into()];
  let out = runner.run_capture(&code, &args, Path::new("."))?;
  if !out.success() {
    bail!("`code --install-extension` failed: {}", out.stderr.trim());
  }
  Ok(if installed.is_some() { Ensure::ReloadNeeded } else { Ensure::Ready })
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;

  const LIST: &[&str] = &["--list-extensions"];
  const REFS: &str = r#"[{"ref":"refs/tags/vscode-extension-v1"},{"ref":"refs/tags/vscode-extension-v3"},{"ref":"refs/tags/vscode-extension-v2"},{"ref":"refs/tags/vscode-extension-vX"}]"#;

  fn fetch_with_refs() -> StubFetch {
    let mut f = StubFetch::default();
    f.bodies.insert(refs_url(), REFS.into());
    f
  }

  #[test]
  fn parses_installed_version_and_tag_numbers() {
    assert_eq!(installed_version("ms-python.python@2024.1.0\nAeth.aeth-devkit@3.0.0\n"), Some(3));
    assert_eq!(installed_version("ms-python.python@2024.1.0\n"), None);
    assert_eq!(latest_tag_number(REFS).unwrap(), Some(3));
    assert_eq!(latest_tag_number("[]").unwrap(), None);
    assert!(latest_tag_number("nope").is_err());
    assert_eq!(vsix_url(3), "https://github.com/AetherBreaker/aeth-devkit/releases/download/vscode-extension-v3/aeth-devkit-vscode-3.vsix");
  }

  #[test]
  fn ready_when_a_compatible_extension_is_installed() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "aeth.aeth-devkit@1.0.0\n");
    let f = StubFetch::default();
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(ensure_extension(&r, &f, Path::new("code"), cache.path(), true), Ensure::Ready);
    assert_eq!(r.calls_for("code").len(), 1, "no install");
    assert!(f.downloads.borrow().is_empty());
  }

  #[test]
  fn installs_the_newest_release_when_absent() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "ms-python.python@2024.1.0\n");
    let f = fetch_with_refs();
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(ensure_extension(&r, &f, Path::new("code"), cache.path(), true), Ensure::Ready);
    let vsix = cache.path().join("vsix").join("aeth-devkit-vscode-3.vsix");
    assert_eq!(f.downloads.borrow()[0], (vsix_url(3), vsix.clone()));
    assert!(vsix.is_file());
    let calls = r.calls_for("code");
    assert_eq!(calls[1], vec!["--install-extension", &vsix.to_string_lossy().into_owned(), "--force"]);
  }

  #[test]
  fn an_upgrade_over_a_loaded_extension_needs_a_reload() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "aeth.aeth-devkit@0.0.0\n");
    let cache = tempfile::tempdir().unwrap();
    assert_eq!(ensure_extension(&r, &fetch_with_refs(), Path::new("code"), cache.path(), true), Ensure::ReloadNeeded);
  }

  #[test]
  fn dry_run_never_installs_and_failures_are_unavailable() {
    let r = RecordingRunner::new(0);
    r.script("code", LIST, 0, "");
    let f = fetch_with_refs();
    let cache = tempfile::tempdir().unwrap();
    assert!(matches!(ensure_extension(&r, &f, Path::new("code"), cache.path(), false), Ensure::Unavailable(m) if m.contains("not installed")));
    assert!(f.downloads.borrow().is_empty());

    let offline = StubFetch::default();
    assert!(matches!(ensure_extension(&r, &offline, Path::new("code"), cache.path(), true), Ensure::Unavailable(m) if m.contains("no body")));

    let mut old = StubFetch::default();
    old.bodies.insert(refs_url(), r#"[{"ref":"refs/tags/vscode-extension-v0"}]"#.into());
    assert!(matches!(ensure_extension(&r, &old, Path::new("code"), cache.path(), true), Ensure::Unavailable(m) if m.contains("no compatible")));

    let failing = RecordingRunner::new(0);
    failing.script("code", LIST, 0, "");
    failing.script_err("code", &["--install-extension"], 1, "boom");
    assert!(matches!(ensure_extension(&failing, &f, Path::new("code"), cache.path(), true), Ensure::Unavailable(m) if m.contains("boom")));
  }
}
```

Add `pub mod install;` to `vscode/mod.rs` under `pub mod protocol;`.

- [ ] **Step 2: Run the tests**

Run: `cargo test -p aeth-devkit-setup vscode::install`
Expected: 5 passed. If ureq's `read_to_vec` does not exist in the pinned ureq 3.4, use `resp.body_mut().with_config().limit(u64::MAX).read_to_vec()` per the ureq 3 docs (check `cargo doc -p ureq --open` or the context7 docs).

- [ ] **Step 3: Commit**

```bash
git add crates/aeth-devkit-setup/src/vscode
git commit -m "feat(setup): install the newest compatible devkit VS Code extension" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 6: The VS Code reviewer session: request files, `--open-url`, polling, Ctrl-C

**Files:**
- Create: `crates/aeth-devkit-setup/src/vscode/session.rs`
- Modify: `crates/aeth-devkit-setup/src/vscode/mod.rs` (add `pub mod session;` and the `VsCode` struct)
- Modify: `crates/aeth-devkit-setup/Cargo.toml` (add `ctrlc = { workspace = true }` under `[dependencies]`)

**Interfaces:**
- Consumes: `protocol::{Proposal, Request, Response, Reviewer, PROTOCOL, EXTENSION_ID}`, `Runner`.
- Produces: `vscode::VsCode { launcher: PathBuf, consent_dir: PathBuf, content_menu: bool, notes: Vec<String> }` (`Drop` empties `consent_dir`), `session::VsCodeReviewer::new(&VsCode, &dyn Runner) -> Self` (`with_poll(Duration)` for tests), `impl Reviewer for VsCodeReviewer`, `session::install_ctrlc_handler() -> Result<()>`, `session::wait_for(response, cancel, poll) -> Result<Response>`, `session::write_atomic(path, text) -> Result<()>`, `session::open_review(vs, runner, previews: &[Preview]) -> Result<()>` (Task 8 adds `Preview`; write `open_review` in Task 8).

- [ ] **Step 1: Add `VsCode` to `vscode/mod.rs`**

Under the `pub mod` lines add `pub mod session;`, and after `ARGV_KEY` add:

```rust
/// A usable VS Code: the launcher to call and the consent folder both sides share.
/// Dropping it empties the folder, so a run leaves nothing behind however it ends.
pub struct VsCode {
  pub launcher: PathBuf,
  pub consent_dir: PathBuf,
  /// Whether the `editor/content` proposal is believed granted (see the spec).
  pub content_menu: bool,
  pub notes: Vec<String>,
}

impl Drop for VsCode {
  fn drop(&mut self) {
    let _ = std::fs::remove_dir_all(&self.consent_dir);
  }
}
```

- [ ] **Step 2: Write `session.rs` with tests**

```rust
//! The VS Code side of a run: request files in the devkit cache, a `vscode://` URL that
//! opens the diff, and polling for the answer. Ctrl-C while waiting hands the question
//! back to the terminal; a second Ctrl-C (anywhere else) ends the process as it always
//! did, because installing a handler removes the default behaviour.

use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering::SeqCst};
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;

use super::VsCode;
use super::protocol::{EXTENSION_ID, PROTOCOL, Proposal, Request, Response, Reviewer};

/// True only inside [`wait_for`]; the handler reads it to choose between "cancel the
/// VS Code request" and "exit".
static WAITING: AtomicBool = AtomicBool::new(false);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn install_ctrlc_handler() -> Result<()> {
  ctrlc::set_handler(|| {
    if WAITING.load(SeqCst) {
      INTERRUPTED.store(true, SeqCst);
    } else {
      std::process::exit(130);
    }
  })
  .context("installing Ctrl-C handler")
}

/// Write via a sibling temp file and rename, so a reader polling the path never sees a
/// half-written file (the extension does the same for responses).
pub fn write_atomic(path: &Path, text: &str) -> Result<()> {
  let tmp = path.with_extension("tmp");
  std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
  std::fs::rename(&tmp, path).with_context(|| format!("renaming to {}", path.display()))
}

pub fn open_url(runner: &dyn Runner, launcher: &Path, url: &str) -> Result<()> {
  let out = runner.run_capture(&launcher.to_string_lossy(), &["--open-url".into(), url.into()], Path::new("."))?;
  if !out.success() {
    bail!("`code --open-url` failed: {}", out.stderr.trim());
  }
  Ok(())
}

/// Poll for the response. Ctrl-C writes the cancel marker (the extension closes the tab)
/// and reports `Dismissed`, which the caller answers with the terminal prompt.
pub fn wait_for(response: &Path, cancel: &Path, poll: Duration) -> Result<Response> {
  INTERRUPTED.store(false, SeqCst);
  WAITING.store(true, SeqCst);
  let result = loop {
    if INTERRUPTED.swap(false, SeqCst) {
      let _ = std::fs::write(cancel, "");
      break Ok(Response::Dismissed);
    }
    if response.is_file() {
      let text = std::fs::read_to_string(response).with_context(|| format!("reading {}", response.display()))?;
      break serde_json::from_str(&text).context("parsing the VS Code response");
    }
    std::thread::sleep(poll);
  };
  WAITING.store(false, SeqCst);
  result
}

pub struct VsCodeReviewer<'a> {
  vs: &'a VsCode,
  runner: &'a dyn Runner,
  poll: Duration,
  next: Cell<u32>,
}

impl<'a> VsCodeReviewer<'a> {
  pub fn new(vs: &'a VsCode, runner: &'a dyn Runner) -> Self {
    Self {
      vs,
      runner,
      poll: Duration::from_millis(250),
      next: Cell::new(0),
    }
  }

  pub fn with_poll(mut self, poll: Duration) -> Self {
    self.poll = poll;
    self
  }
}

impl Reviewer for VsCodeReviewer<'_> {
  fn review(&self, p: &Proposal, offer_replace_all: bool) -> Result<Response> {
    let n = self.next.get();
    self.next.set(n + 1);
    // `<pid>-<n>`: unique across concurrent runs, and the only thing the URL carries.
    let id = format!("{}-{n}", std::process::id());
    let file = |ext: &str| self.vs.consent_dir.join(format!("{id}.{ext}"));
    std::fs::create_dir_all(&self.vs.consent_dir)?;
    std::fs::write(file("current"), &p.current)?;
    std::fs::write(file("proposed"), &p.proposed)?;
    let request = Request {
      protocol: PROTOCOL,
      id: id.clone(),
      title: p.title.clone(),
      current_path: file("current"),
      proposed_path: file("proposed"),
      hunks: p.hunks.clone(),
      offer_replace_all,
      content_menu: self.vs.content_menu,
      response_path: file("response.json"),
    };
    write_atomic(&file("request.json"), &serde_json::to_string_pretty(&request)?)?;
    open_url(self.runner, &self.vs.launcher, &format!("vscode://{EXTENSION_ID}/consent?id={id}"))?;
    println!("waiting for VS Code (Ctrl-C to answer here instead)…");
    wait_for(&file("response.json"), &file("cancel"), self.poll)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use aeth_devkit_core::process::RecordingRunner;
  use std::sync::Mutex;

  // The two statics are process-wide; these tests must not overlap.
  static SERIAL: Mutex<()> = Mutex::new(());

  fn vscode(dir: &Path) -> VsCode {
    VsCode {
      launcher: "code".into(),
      consent_dir: dir.join("consent"),
      content_menu: true,
      notes: vec![],
    }
  }

  #[test]
  fn review_writes_the_request_opens_the_url_and_reads_the_response() {
    let _g = SERIAL.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let vs = vscode(tmp.path());
    let runner = RecordingRunner::new(0);
    let reviewer = VsCodeReviewer::new(&vs, &runner).with_poll(Duration::from_millis(5));
    let dir = vs.consent_dir.clone();
    let responder = std::thread::spawn(move || {
      let request = loop {
        if let Some(p) = std::fs::read_dir(&dir).ok().and_then(|d| {
          d.flatten().map(|e| e.path()).find(|p| p.to_string_lossy().ends_with(".request.json"))
        }) {
          break p;
        }
        std::thread::sleep(Duration::from_millis(5));
      };
      let req: Request = serde_json::from_str(&std::fs::read_to_string(&request).unwrap()).unwrap();
      assert_eq!(req.protocol, PROTOCOL);
      assert_eq!(req.title, "docker/Dockerfile");
      assert!(req.content_menu && req.offer_replace_all);
      assert_eq!(std::fs::read_to_string(&req.proposed_path).unwrap(), "a\nc\n");
      write_atomic(&req.response_path, r#"{"decision":"partial","accepted":[0]}"#).unwrap();
      req
    });
    let p = Proposal::new("docker/Dockerfile", "q", "a\nb\n", "a\nc\n");
    assert_eq!(reviewer.review(&p, true).unwrap(), Response::Partial { accepted: vec![0] });
    let req = responder.join().unwrap();
    let calls = runner.calls_for("code");
    assert_eq!(calls[0], vec!["--open-url", &format!("vscode://aeth.aeth-devkit/consent?id={}", req.id)]);
    assert!(req.id.starts_with(&format!("{}-0", std::process::id())));
    drop(vs);
    assert!(!tmp.path().join("consent").exists(), "dropping VsCode empties the folder");
  }

  #[test]
  fn ctrl_c_while_waiting_writes_the_cancel_marker_and_dismisses() {
    let _g = SERIAL.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let response = tmp.path().join("r.json");
    let cancel = tmp.path().join("r.cancel");
    std::thread::spawn(|| {
      std::thread::sleep(Duration::from_millis(30));
      INTERRUPTED.store(true, SeqCst);
    });
    assert_eq!(wait_for(&response, &cancel, Duration::from_millis(5)).unwrap(), Response::Dismissed);
    assert!(cancel.is_file());
    assert!(!WAITING.load(SeqCst));
  }

  #[test]
  fn a_failed_open_url_or_a_bad_response_is_an_error() {
    let _g = SERIAL.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let vs = vscode(tmp.path());
    let runner = RecordingRunner::new(1);
    let reviewer = VsCodeReviewer::new(&vs, &runner);
    assert!(reviewer.review(&Proposal::new("t", "q", "a\n", "b\n"), true).is_err());
    let response = tmp.path().join("bad.json");
    std::fs::write(&response, "{").unwrap();
    assert!(wait_for(&response, &tmp.path().join("bad.cancel"), Duration::from_millis(1)).is_err());
  }
}
```

- [ ] **Step 3: Run the tests**

Run: `cargo test -p aeth-devkit-setup vscode::session`
Expected: 3 passed.

- [ ] **Step 4: Commit**

```bash
git add crates/aeth-devkit-setup/Cargo.toml crates/aeth-devkit-setup/src/vscode
git commit -m "feat(setup): VS Code reviewer session with polling and Ctrl-C fallback" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---
### Task 7: `prepare()`: detect, find, ensure, grant

**Files:**
- Modify: `crates/aeth-devkit-setup/src/vscode/mod.rs`

**Interfaces:**
- Consumes: `find_launcher`, `grant_proposal`, `stray_notes`, `install::{ensure_extension, Ensure, Fetch}`, `VsCode`.
- Produces: `Options { force, install, term_program: Option<String>, path: Option<OsString>, home: Option<PathBuf>, cache: Option<PathBuf>, project_root: PathBuf }`, `Options::from_env(force, install, project_root) -> Options`, `enum Prepared { Inert, Unavailable(String), ReloadNeeded, Ready(VsCode) }`, `prepare(opts: &Options, runner: &dyn Runner, fetch: &dyn Fetch) -> Prepared`.

- [ ] **Step 1: Add `Options`, `Prepared`, `prepare` and tests to `vscode/mod.rs`**

Append after `stray_notes`:

```rust
/// Everything `prepare` reads from the environment, as plain data so tests can fake it.
pub struct Options {
  /// `--vscode`: skip the `TERM_PROGRAM` check.
  pub force: bool,
  /// False under `--dry-run`: use an installed extension, never install or edit argv.
  pub install: bool,
  pub term_program: Option<String>,
  pub path: Option<std::ffi::OsString>,
  pub home: Option<PathBuf>,
  pub cache: Option<PathBuf>,
  pub project_root: PathBuf,
}

impl Options {
  pub fn from_env(force: bool, install: bool, project_root: &Path) -> Self {
    Self {
      force,
      install,
      term_program: std::env::var("TERM_PROGRAM").ok(),
      path: std::env::var_os("PATH"),
      home: home_dir(),
      cache: aeth_devkit_core::update::cache_dir(),
      project_root: project_root.to_path_buf(),
    }
  }
}

pub enum Prepared {
  /// Not a VS Code terminal: the terminal flow, silently.
  Inert,
  /// VS Code is here but cannot be used; the note says why.
  Unavailable(String),
  /// A newer extension was just installed over a loaded one; stop and say so.
  ReloadNeeded,
  Ready(VsCode),
}

/// Steps 1–5 of the spec's `setup-project` section. Runs once, before any consent
/// prompt, and touches nothing when `install` is false.
pub fn prepare(opts: &Options, runner: &dyn aeth_devkit_core::process::Runner, fetch: &dyn install::Fetch) -> Prepared {
  if !opts.force && opts.term_program.as_deref() != Some("vscode") {
    return Prepared::Inert;
  }
  let Some(launcher) = opts.path.as_deref().and_then(find_launcher) else {
    return Prepared::Unavailable("`code` is not on PATH".into());
  };
  let (Some(cache), Some(home)) = (&opts.cache, &opts.home) else {
    return Prepared::Unavailable("cannot locate the devkit cache or home directory".into());
  };
  match install::ensure_extension(runner, fetch, &launcher, cache, opts.install) {
    install::Ensure::Ready => {}
    install::Ensure::ReloadNeeded => return Prepared::ReloadNeeded,
    install::Ensure::Unavailable(why) => return Prepared::Unavailable(why),
  }
  let argv_path = home.join(".vscode").join("argv.json");
  let argv = match std::fs::read_to_string(&argv_path) {
    Ok(t) => Some(t),
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
    Err(e) => return Prepared::Unavailable(format!("reading {}: {e}", argv_path.display())),
  };
  let mut notes = stray_notes(home, &opts.project_root);
  let content_menu = match grant_proposal(argv.as_deref()) {
    Ok(None) => true,
    Ok(Some(granted)) if opts.install => {
      if let Err(e) = std::fs::create_dir_all(argv_path.parent().unwrap()).and_then(|()| std::fs::write(&argv_path, granted)) {
        notes.push(format!("could not edit {}: {e}; the in-editor buttons stay hidden", argv_path.display()));
      } else {
        notes.push("restart VS Code once to enable the in-editor devkit buttons".into());
      }
      false
    }
    Ok(Some(_)) => false,
    Err(e) => {
      notes.push(format!("{e:#}; the in-editor buttons stay hidden"));
      false
    }
  };
  let consent_dir = cache.join("consent");
  // Start clean: a run killed mid-review leaves files here that the extension must not
  // mistake for live requests.
  let _ = std::fs::remove_dir_all(&consent_dir);
  if let Err(e) = std::fs::create_dir_all(&consent_dir) {
    return Prepared::Unavailable(format!("creating {}: {e}", consent_dir.display()));
  }
  Prepared::Ready(VsCode {
    launcher,
    consent_dir,
    content_menu,
    notes,
  })
}
```

Add to the `tests` module in the same file:

```rust
  use aeth_devkit_core::process::RecordingRunner;
  use install::StubFetch;

  fn options(dir: &Path, install: bool) -> Options {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join(if cfg!(windows) { "code.cmd" } else { "code" }), "").unwrap();
    Options {
      force: false,
      install,
      term_program: Some("vscode".into()),
      path: Some(std::env::join_paths([bin]).unwrap()),
      home: Some(dir.join("home")),
      cache: Some(dir.join("cache")),
      project_root: dir.join("proj"),
    }
  }

  fn installed_runner() -> RecordingRunner {
    let r = RecordingRunner::new(0);
    r.script("code", &["--list-extensions"], 0, "aeth.aeth-devkit@1.0.0\n");
    r
  }

  #[test]
  fn prepare_is_inert_outside_vscode_unless_forced() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = options(tmp.path(), true);
    o.term_program = None;
    assert!(matches!(prepare(&o, &installed_runner(), &StubFetch::default()), Prepared::Inert));
    o.force = true;
    assert!(matches!(prepare(&o, &installed_runner(), &StubFetch::default()), Prepared::Ready(_)));
    o.path = Some(std::env::join_paths([tmp.path()]).unwrap());
    assert!(matches!(prepare(&o, &installed_runner(), &StubFetch::default()), Prepared::Unavailable(m) if m.contains("PATH")));
  }

  #[test]
  fn prepare_grants_the_proposal_once_and_reports_content_menu_after() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path(), true);
    let Prepared::Ready(vs) = prepare(&o, &installed_runner(), &StubFetch::default()) else { panic!() };
    assert!(!vs.content_menu);
    assert!(vs.notes.iter().any(|n| n.contains("restart VS Code once")), "{:?}", vs.notes);
    assert!(vs.consent_dir.is_dir() && vs.consent_dir.starts_with(tmp.path().join("cache")));
    let argv = std::fs::read_to_string(tmp.path().join("home/.vscode/argv.json")).unwrap();
    assert!(argv.contains("\"enable-proposed-api\": [\"aeth.aeth-devkit\"]"), "{argv}");
    let dir = vs.consent_dir.clone();
    drop(vs);
    assert!(!dir.exists());

    let Prepared::Ready(vs) = prepare(&o, &installed_runner(), &StubFetch::default()) else { panic!() };
    assert!(vs.content_menu);
    assert!(!vs.notes.iter().any(|n| n.contains("restart")), "{:?}", vs.notes);
  }

  #[test]
  fn dry_run_reads_argv_but_never_writes_it_and_never_installs() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path(), false);
    let Prepared::Ready(vs) = prepare(&o, &installed_runner(), &StubFetch::default()) else { panic!() };
    assert!(!vs.content_menu);
    assert!(!tmp.path().join("home/.vscode/argv.json").exists());
    let absent = RecordingRunner::new(0);
    absent.script("code", &["--list-extensions"], 0, "");
    assert!(matches!(prepare(&o, &absent, &StubFetch::default()), Prepared::Unavailable(_)));
    assert_eq!(absent.calls_for("code").len(), 1);
  }

  #[test]
  fn prepare_stops_on_reload_needed_and_carries_stray_notes() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path(), true);
    std::fs::create_dir_all(tmp.path().join("proj/.vscode/extension")).unwrap();
    let Prepared::Ready(vs) = prepare(&o, &installed_runner(), &StubFetch::default()) else { panic!() };
    assert!(vs.notes.iter().any(|n| n.starts_with(".vscode/extension/")), "{:?}", vs.notes);
    let old = RecordingRunner::new(0);
    old.script("code", &["--list-extensions"], 0, "aeth.aeth-devkit@0.0.0\n");
    let mut f = StubFetch::default();
    f.bodies.insert(install::refs_url(), r#"[{"ref":"refs/tags/vscode-extension-v1"}]"#.into());
    assert!(matches!(prepare(&o, &old, &f), Prepared::ReloadNeeded));
  }
```

- [ ] **Step 2: Run the tests**

Run: `cargo test -p aeth-devkit-setup vscode::tests`
Expected: 9 passed (5 from Task 4 + 4 new).

- [ ] **Step 3: Commit**

```bash
git add crates/aeth-devkit-setup/src/vscode/mod.rs
git commit -m "feat(setup): prepare the VS Code extension and proposal grant once per run" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 8: CLI wiring, `--vscode` / `--no-vscode`, and the dry-run review request

**Files:**
- Modify: `crates/aeth-devkit-setup/src/cli.rs`
- Modify: `crates/aeth-devkit-setup/src/changes.rs`
- Modify: `crates/aeth-devkit-setup/src/vscode/session.rs` (add `open_review`)
- Test: `crates/aeth-devkit-setup/src/changes.rs` (unit), `crates/aeth-devkit-setup/src/vscode/session.rs` (unit)

**Interfaces:**
- Produces: `changes::Preview { path: PathBuf, current: Option<String>, proposed: String }`, `Changes.previews: Vec<Preview>` (filled only in dry-run), `session::open_review(vs: &VsCode, runner: &dyn Runner, root: &Path, previews: &[Preview]) -> Result<()>`.

- [ ] **Step 1: Collect previews in dry-run**

In `changes.rs`, add after `FileChange`:

```rust
/// What a dry run would write, kept only then, for the VS Code review.
#[derive(Debug)]
pub struct Preview {
  pub path: PathBuf,
  pub current: Option<String>,
  pub proposed: String,
}
```

add `pub previews: Vec<Preview>,` to `Changes` (initialised `Vec::new()` in `new`), and in `record_optional`, right before `if !self.dry_run {`:

```rust
    if self.dry_run {
      // Two steps can touch one file (`.gitignore`); the last proposal is the whole one.
      self.previews.retain(|p| p.path != path);
      self.previews.push(Preview {
        path: path.to_path_buf(),
        current: original.map(str::to_string),
        proposed: merged.to_string(),
      });
    }
```

Add a unit test to `changes.rs` (create a `#[cfg(test)] mod tests` if there is none):

```rust
  #[test]
  fn dry_run_keeps_the_last_preview_per_path_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("f");
    let mut c = Changes::new(true);
    c.record_optional(&p, None, "one\n", vec![]).unwrap();
    c.record_optional(&p, None, "two\n", vec![]).unwrap();
    assert_eq!(c.previews.len(), 1);
    assert_eq!(c.previews[0].proposed, "two\n");
    assert!(!p.exists());
    let mut wet = Changes::new(false);
    wet.record_optional(&p, None, "one\n", vec![]).unwrap();
    assert!(wet.previews.is_empty());
  }
```

- [ ] **Step 2: Add `open_review` to `session.rs`**

```rust
/// `--dry-run`: one request listing every file, opened as a multi-diff. Nothing awaited.
pub fn open_review(vs: &VsCode, runner: &dyn Runner, root: &Path, previews: &[crate::changes::Preview]) -> Result<()> {
  let id = format!("review-{}", std::process::id());
  let dir = &vs.consent_dir;
  std::fs::create_dir_all(dir)?;
  let mut files = Vec::new();
  for (i, p) in previews.iter().enumerate() {
    let proposed = dir.join(format!("{id}-{i}.proposed"));
    std::fs::write(&proposed, &p.proposed)?;
    let current = match &p.current {
      Some(text) => {
        let path = dir.join(format!("{id}-{i}.current"));
        std::fs::write(&path, text)?;
        Some(path)
      }
      None => None,
    };
    let label = p.path.strip_prefix(root).unwrap_or(&p.path).to_string_lossy().replace('\\', "/");
    files.push(serde_json::json!({
      "path": p.path, "label": label, "current_path": current, "proposed_path": proposed,
    }));
  }
  let request = serde_json::json!({ "protocol": PROTOCOL, "id": id, "files": files });
  write_atomic(&dir.join(format!("{id}.request.json")), &serde_json::to_string_pretty(&request)?)?;
  open_url(runner, &vs.launcher, &format!("vscode://{EXTENSION_ID}/review?id={id}"))
}
```

and a test in the `session::tests` module:

```rust
  #[test]
  fn open_review_writes_one_request_listing_every_file() {
    let _g = SERIAL.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let vs = vscode(tmp.path());
    let runner = RecordingRunner::new(0);
    let root = tmp.path().join("proj");
    let previews = vec![
      crate::changes::Preview { path: root.join("docker").join("Dockerfile"), current: Some("a\n".into()), proposed: "b\n".into() },
      crate::changes::Preview { path: root.join("new.txt"), current: None, proposed: "n\n".into() },
    ];
    open_review(&vs, &runner, &root, &previews).unwrap();
    let id = format!("review-{}", std::process::id());
    let req: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vs.consent_dir.join(format!("{id}.request.json"))).unwrap()).unwrap();
    assert_eq!(req["protocol"], PROTOCOL);
    assert_eq!(req["files"][0]["label"], "docker/Dockerfile");
    assert_eq!(req["files"][1]["current_path"], serde_json::Value::Null);
    assert_eq!(std::fs::read_to_string(req["files"][1]["proposed_path"].as_str().unwrap()).unwrap(), "n\n");
    assert_eq!(runner.calls_for("code")[0][1], format!("vscode://aeth.aeth-devkit/review?id={id}"));
  }
```

- [ ] **Step 3: Wire `cli.rs`**

Add the flags to `Args` after `replace_docker`:

```rust
  /// Use the VS Code diff for Docker consent even when TERM_PROGRAM is not "vscode".
  #[arg(long, conflicts_with = "no_vscode")]
  pub vscode: bool,

  /// Never open VS Code; always use the terminal prompt.
  #[arg(long)]
  pub no_vscode: bool,
```

Rewrite `run` as:

```rust
pub fn run(args: &Args) -> Result<ExitCode> {
  let dry_run = args.dry_run || args.check;
  let templates = crate::templates::locate(args.templates_dir.as_deref())?;
  let root = crate::context::strip_verbatim(args.root.canonicalize().unwrap_or(args.root.clone()));
  // `IsTerminal` is how std asks "is a human here?": prompts only make sense on a tty.
  let tty = std::io::IsTerminal::is_terminal(&std::io::stdin());
  let runner = aeth_devkit_core::process::SystemRunner;

  // VS Code is consulted only where a human could answer in the terminal anyway: never
  // for --check (hooks and CI), never without a tty, never when --replace-docker has
  // already answered. It runs before staging so a "reload and rerun" stop touches nothing.
  let vs = if args.no_vscode || args.check || !tty || args.replace_docker {
    None
  } else {
    let opts = crate::vscode::Options::from_env(args.vscode, !dry_run, &root);
    match crate::vscode::prepare(&opts, &runner, &crate::vscode::install::HttpFetch) {
      crate::vscode::Prepared::Inert => None,
      crate::vscode::Prepared::Unavailable(why) => {
        println!("note: {why}; using the terminal prompt.");
        None
      }
      crate::vscode::Prepared::ReloadNeeded => {
        println!("The devkit VS Code extension was updated. Reload the VS Code window, then run setup-project again.");
        return Ok(ExitCode::SUCCESS);
      }
      crate::vscode::Prepared::Ready(vs) => Some(vs),
    }
  };
  if let Some(vs) = &vs {
    for note in &vs.notes {
      println!("note: {note}");
    }
    if !dry_run {
      crate::vscode::session::install_ctrlc_handler()?;
    }
  }
  let reviewer = vs.as_ref().filter(|_| !dry_run).map(|v| crate::vscode::session::VsCodeReviewer::new(v, &runner));

  // When committing, the committable managed files are merged against their `HEAD`
  // content, so the commit carries only this run's changes and the user's uncommitted
  // edits are replayed back on top afterwards (see `aeth_devkit_core::commit`).
  let committing = !dry_run && !args.no_commit && crate::git::is_git_tracked(&root);
  let mut bases = if committing { Some(crate::git::stage_bases(&root)?) } else { None };

  // Apply the templates (plus tombi), putting the user's files back on any failure.
  let apply = |changes: &mut Option<crate::changes::Changes>| -> Result<()> {
    let deps = crate::docker::Deps {
      runner: &runner,
      prompt: &aeth_devkit_core::prompt::StdinPrompt,
      reviewer: reviewer.as_ref().map(|r| r as &dyn crate::vscode::protocol::Reviewer),
      mode: match (dry_run, args.replace_docker, tty) {
        (true, _, _) => crate::docker::Mode::DryRun,
        (false, true, _) => crate::docker::Mode::ReplaceAll,
        (false, false, true) => crate::docker::Mode::Ask,
        (false, false, false) => crate::docker::Mode::KeepAll,
      },
    };
    let mut c = crate::run_with(&root, &templates, dry_run, &deps)?;
    if !dry_run {
      match crate::format::format_pyproject(&root, &crate::format::SystemRunner, &mut c)? {
        crate::format::Outcome::Formatted(_) => {}
        crate::format::Outcome::Unavailable => println!("note: tombi not found; skipping pyproject.toml formatting."),
        crate::format::Outcome::Failed { code } => {
          eprintln!("warning: tombi format exited with {code:?}; pyproject.toml left unformatted.");
        }
      }
    }
    *changes = Some(c);
    Ok(())
  };
  let mut changes = None;
  if let Err(e) = apply(&mut changes) {
    if let Some(bases) = &bases {
      aeth_devkit_core::commit::restore_worktree(&root, bases)?;
    }
    return Err(e);
  }
  // `expect` documents the invariant: `apply` only returns `Ok` after setting it.
  let changes = changes.expect("apply sets changes on success");

  for note in &changes.notes {
    println!("note: {note}");
  }
  if changes.is_empty() {
    // No file differs from its merge base; undo the staging so the user's uncommitted
    // edits to managed files are back in place.
    if let Some(bases) = &bases {
      aeth_devkit_core::commit::unstage_clean_base(&root, bases)?;
    }
    println!("Nothing to do — project already matches the templates.");
    return Ok(ExitCode::SUCCESS);
  }
  let header = if dry_run { "Would change:" } else { "Changed:" };
  println!("{header}\n{}", changes.report(&root));
  if dry_run && let Some(vs) = &vs {
    if let Err(e) = crate::vscode::session::open_review(vs, &runner, &root, &changes.previews) {
      println!("note: could not open the review in VS Code: {e:#}");
    }
  }
  if args.check {
    return Ok(ExitCode::from(1));
  }
  if let Some(bases) = &mut bases {
    match crate::git::commit_changes(&root, &changes, bases) {
      Ok(Some(hash)) => println!("Committed as {hash}."),
      Ok(None) => println!("Nothing to commit (only gitignored or env files changed)."),
      Err(e) => {
        eprintln!("warning: not committed; the template changes were rolled back: {e:#}");
        return Ok(ExitCode::from(3));
      }
    }
  }
  Ok(ExitCode::SUCCESS)
}
```

Note the `reviewer` binding must outlive `apply` (it does: declared before). `vs` is dropped at the end of `run`, which empties the consent folder.

- [ ] **Step 4: Build, run the targeted tests, and check the CLI help**

Run: `cargo test -p aeth-devkit-setup changes && cargo test -p aeth-devkit-setup vscode::session && cargo build -p aeth-devkit-setup`
Expected: tests pass, build clean. Then `cargo run -p aeth-devkit-setup -- --help | grep -A1 vscode` shows both flags. `cargo clippy -p aeth-devkit-setup --all-targets` shows no new warnings (the `if dry_run && let Some(vs)` chain needs the crate's edition to be 2024; if it is not, nest the two `if`s).

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-setup/src/cli.rs crates/aeth-devkit-setup/src/changes.rs crates/aeth-devkit-setup/src/vscode/session.rs
git commit -m "feat(setup-project): --vscode/--no-vscode, VS Code consent wiring, dry-run review request" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---
### Task 9: Extension scaffold and the ported `addToRuntimeBaseClasses` command

**Files:**
- Create: `vscode-extension/package.json`, `tsconfig.json`, `esbuild.mjs`, `vitest.config.ts`, `README.md`, `.vscodeignore`, `test/vscode-stub.ts`, `src/runtimeBaseClasses.ts`, `src/extension.ts` (minimal), `test/runtimeBaseClasses.test.ts`
- Modify: `.gitignore` (append `vscode-extension/node_modules/`, `vscode-extension/dist/`, `*.vsix`)

**Interfaces:**
- Produces: `runtimeBaseClasses.ts` exports `insertIntoRuntimeBaseClasses(content, fqcn): string | null`, `ensureRuntimeBaseClassesArray(content): string`, `computeModulePath(filePath): string`, `addToRuntimeBaseClasses(): Promise<void>`.

- [ ] **Step 1: Scaffold the project**

`vscode-extension/package.json`:

```json
{
  "name": "aeth-devkit",
  "displayName": "aeth-devkit",
  "description": "In-editor consent for devkit setup-project, plus the runtime-evaluated-base-classes command.",
  "version": "0.0.0",
  "publisher": "aeth",
  "private": true,
  "repository": { "type": "git", "url": "https://github.com/AetherBreaker/aeth-devkit" },
  "engines": { "vscode": "^1.90.0" },
  "categories": ["Other"],
  "main": "./dist/extension.js",
  "activationEvents": ["onUri"],
  "enabledApiProposals": ["contribEditorContentMenu"],
  "aethDevkit": { "protocol": 1 },
  "contributes": {
    "commands": [
      { "command": "aeth-devkit.replaceFile", "title": "Replace file with the devkit proposal", "category": "devkit", "icon": "$(replace-all)" },
      { "command": "aeth-devkit.keepFile", "title": "Keep file as it is", "category": "devkit", "icon": "$(discard)" },
      { "command": "aeth-devkit.replaceAll", "title": "Replace all remaining Docker files this run", "category": "devkit" },
      { "command": "aeth-devkit.applyAccepted", "title": "Apply accepted hunks", "category": "devkit" },
      { "command": "aeth-devkit.acceptAllHunks", "title": "Accept all hunks", "category": "devkit" },
      { "command": "aeth-devkit.acceptHunk", "title": "Accept hunk", "category": "devkit" },
      { "command": "aeth-devkit.rejectHunk", "title": "Reject hunk", "category": "devkit" },
      { "command": "aeth-devkit.addToRuntimeBaseClasses", "title": "Add to runtime-evaluated-base-classes", "category": "devkit" }
    ],
    "menus": {
      "commandPalette": [
        { "command": "aeth-devkit.acceptHunk", "when": "false" },
        { "command": "aeth-devkit.rejectHunk", "when": "false" },
        { "command": "aeth-devkit.applyAccepted", "when": "resourceScheme == aeth-devkit-proposed" },
        { "command": "aeth-devkit.acceptAllHunks", "when": "resourceScheme == aeth-devkit-proposed" },
        { "command": "aeth-devkit.replaceFile", "when": "resourceScheme == aeth-devkit-proposed" },
        { "command": "aeth-devkit.replaceAll", "when": "resourceScheme == aeth-devkit-proposed" },
        { "command": "aeth-devkit.keepFile", "when": "resourceScheme == aeth-devkit-proposed" }
      ],
      "editor/context": [
        { "command": "aeth-devkit.addToRuntimeBaseClasses", "when": "editorLangId == python", "group": "1_modification@100" }
      ],
      "editor/content": [
        { "command": "aeth-devkit.replaceFile", "group": "navigation@1", "when": "resourceScheme == aeth-devkit-proposed" },
        { "command": "aeth-devkit.keepFile", "group": "navigation@2", "when": "resourceScheme == aeth-devkit-proposed" }
      ],
      "editor/title": [
        { "command": "aeth-devkit.replaceFile", "group": "navigation@1", "when": "resourceScheme == aeth-devkit-proposed && !aeth-devkit.contentMenu" },
        { "command": "aeth-devkit.keepFile", "group": "navigation@2", "when": "resourceScheme == aeth-devkit-proposed && !aeth-devkit.contentMenu" }
      ]
    }
  },
  "scripts": {
    "build": "node esbuild.mjs",
    "watch": "node esbuild.mjs --watch",
    "typecheck": "tsc --noEmit",
    "test": "vitest run"
  },
  "devDependencies": {
    "@types/node": "^20.14.0",
    "@types/vscode": "^1.90.0",
    "@vscode/vsce": "^3.2.0",
    "esbuild": "^0.24.0",
    "typescript": "^5.6.0",
    "vitest": "^2.1.0"
  }
}
```

`vscode-extension/tsconfig.json`:

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "lib": ["ES2022"],
    "types": ["node"],
    "strict": true,
    "noEmit": true,
    "skipLibCheck": true,
    "esModuleInterop": true
  },
  "include": ["src", "test", "esbuild.mjs", "vitest.config.ts"]
}
```

`vscode-extension/esbuild.mjs`:

```js
// One minified CommonJS file; `vscode` is provided by the host and must stay external.
import * as esbuild from 'esbuild';

const watch = process.argv.includes('--watch');
const ctx = await esbuild.context({
  entryPoints: ['src/extension.ts'],
  bundle: true,
  outfile: 'dist/extension.js',
  platform: 'node',
  format: 'cjs',
  target: 'node18',
  external: ['vscode'],
  minify: !watch,
  sourcemap: watch,
});
if (watch) {
  await ctx.watch();
} else {
  await ctx.rebuild();
  await ctx.dispose();
}
```

`vscode-extension/vitest.config.ts`:

```ts
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// The pure modules never touch `vscode`, but the modules that import it must still load.
export default defineConfig({
  test: { include: ['test/**/*.test.ts'] },
  resolve: { alias: { vscode: fileURLToPath(new URL('./test/vscode-stub.ts', import.meta.url)) } },
});
```

`vscode-extension/test/vscode-stub.ts`:

```ts
// Empty stand-in for the `vscode` host module under vitest; nothing here is called.
export {};
```

`vscode-extension/.vscodeignore`:

```
src/**
test/**
node_modules/**
esbuild.mjs
tsconfig.json
vitest.config.ts
*.vsix
```

`vscode-extension/README.md`:

```markdown
# aeth-devkit VS Code extension

Installed by `devkit setup-project` from devkit's GitHub releases (`vscode-extension-vN`);
never published to the marketplace. It shows each Docker change setup-project proposes as
a native diff with per-hunk Accept/Reject, opens a multi-diff review for `--dry-run`, and
carries the `Add to runtime-evaluated-base-classes` command. See the devkit README.
```

`vscode-extension/src/extension.ts` (minimal for this task):

```ts
import * as vscode from 'vscode';
import { addToRuntimeBaseClasses } from './runtimeBaseClasses';

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.commands.registerCommand('aeth-devkit.addToRuntimeBaseClasses', addToRuntimeBaseClasses),
  );
}

export function deactivate(): void {}
```

Append to the repo `.gitignore`:

```
# VS Code extension build products
vscode-extension/node_modules/
vscode-extension/dist/
*.vsix
```

- [ ] **Step 2: Port the Drekker command**

`vscode-extension/src/runtimeBaseClasses.ts` is `aeth_ext/.vscode/extension/extension.js` (read it at `../aeth_ext/.vscode/extension/extension.js` relative to the devkit repo) translated to TypeScript, behaviour unchanged:

```ts
// Port of the Drekker `addToRuntimeBaseClasses` command. Behaviour is unchanged; only
// the command id moved to `aeth-devkit.addToRuntimeBaseClasses`.
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as vscode from 'vscode';

/**
 * Inserts `fqcn` into the multi-line `runtime-evaluated-base-classes` array. Returns the
 * updated content, or null when the array is missing or inline (`[...]` on one line).
 * A depth counter finds the closing bracket reliably.
 */
export function insertIntoRuntimeBaseClasses(content: string, fqcn: string): string | null {
  const lines = content.split('\n');
  let inArray = false;
  let depth = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (!inArray) {
      if (/^\s*runtime-evaluated-base-classes\s*=\s*\[/.test(line)) {
        inArray = true;
        depth = 0;
        for (const ch of line) {
          if (ch === '[') depth++;
          else if (ch === ']') depth--;
        }
        if (depth <= 0) return null;
      }
    } else {
      for (const ch of line) {
        if (ch === '[') depth++;
        else if (ch === ']') depth--;
      }
      if (depth <= 0) {
        let entryIndent = '      ';
        for (let j = i - 1; j >= 0; j--) {
          const m = /^(\s+)"/.exec(lines[j]);
          if (m) {
            entryIndent = m[1];
            break;
          }
        }
        lines.splice(i, 0, `${entryIndent}"${fqcn}",`);
        return lines.join('\n');
      }
    }
  }
  return null;
}

/**
 * Ensures a multi-line `runtime-evaluated-base-classes` array exists, adding an empty one
 * under `[tool.ruff.lint.flake8-type-checking]` (creating the table at the end if needed).
 */
export function ensureRuntimeBaseClassesArray(content: string): string {
  if (/^\s*runtime-evaluated-base-classes\s*=\s*\[/m.test(content)) return content;
  const emptyArray = ['  runtime-evaluated-base-classes = [', '  ]'];
  const lines = content.split('\n');
  const headerIdx = lines.findIndex((l) => /^\s*\[tool\.ruff\.lint\.flake8-type-checking\]\s*$/.test(l));
  if (headerIdx !== -1) {
    lines.splice(headerIdx + 1, 0, ...emptyArray);
    return lines.join('\n');
  }
  const trimmed = content.replace(/\s*$/, '');
  return `${trimmed}\n\n[tool.ruff.lint.flake8-type-checking]\n${emptyArray.join('\n')}\n`;
}

/**
 * Walks up from a Python file through packages (directories with `__init__.py(i)`) to
 * build its fully-qualified module path. Works for `src/` layouts and site-packages.
 */
export function computeModulePath(filePath: string, exists: (p: string) => boolean = fs.existsSync): string {
  if (!filePath) return '';
  const hasInit = (dir: string) => exists(path.join(dir, '__init__.py')) || exists(path.join(dir, '__init__.pyi'));
  const fileBase = path.basename(filePath).replace(/\.pyi?$/i, '');
  const chain: string[] = [];
  if (fileBase && fileBase !== '__init__') chain.push(fileBase);
  let dir = path.dirname(filePath);
  while (dir && hasInit(dir)) {
    chain.push(path.basename(dir));
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  return chain.reverse().join('.');
}

interface Resolved {
  fsPath: string;
  className?: string;
}

/** Follows the definition provider through re-exports to the original `class`/`def`. */
async function resolveDefinition(startUri: vscode.Uri, startPos: vscode.Position): Promise<Resolved | null> {
  type Loc = { uri: vscode.Uri; range: vscode.Range };
  const normalize = (item: vscode.Location | vscode.LocationLink | undefined): Loc | null => {
    if (!item) return null;
    if ('targetUri' in item) return { uri: item.targetUri, range: item.targetSelectionRange ?? item.targetRange };
    if (item.uri && item.range) return { uri: item.uri, range: item.range };
    return null;
  };
  let curUri = startUri;
  let curPos = startPos;
  let result: Resolved | null = null;
  const visited = new Set<string>();
  for (let i = 0; i < 16; i++) {
    let defs: (vscode.Location | vscode.LocationLink)[] | undefined;
    try {
      defs = await vscode.commands.executeCommand('vscode.executeDefinitionProvider', curUri, curPos);
    } catch {
      break;
    }
    if (!defs || defs.length === 0) break;
    const loc = normalize(defs[0]);
    if (!loc) break;
    let doc: vscode.TextDocument;
    try {
      doc = await vscode.workspace.openTextDocument(loc.uri);
    } catch {
      break;
    }
    const lineText = doc.lineAt(loc.range.start.line).text;
    result = { fsPath: loc.uri.fsPath };
    const defMatch = /^\s*(?:class|def)\s+(\w+)/.exec(lineText);
    if (defMatch) {
      result.className = defMatch[1];
      break;
    }
    const key = `${loc.uri.toString()}:${loc.range.start.line}:${loc.range.start.character}`;
    if (visited.has(key)) break;
    visited.add(key);
    curUri = loc.uri;
    curPos = loc.range.start;
  }
  return result;
}

export async function addToRuntimeBaseClasses(): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) {
    void vscode.window.showErrorMessage('No active editor.');
    return;
  }
  const sel = editor.selection;
  let position = sel.active;
  let fallbackName = '';
  if (!sel.isEmpty) {
    position = sel.start;
    fallbackName = editor.document.getText(sel).trim();
    const wordRange = editor.document.getWordRangeAtPosition(sel.start);
    if (wordRange) {
      position = wordRange.start;
      fallbackName = editor.document.getText(wordRange);
    }
  } else {
    const lineText = editor.document.lineAt(sel.active.line).text;
    const classMatch = /^\s*class\s+(\w+)/.exec(lineText);
    if (classMatch) {
      fallbackName = classMatch[1];
    } else {
      const wordRange = editor.document.getWordRangeAtPosition(sel.active);
      if (wordRange) fallbackName = editor.document.getText(wordRange);
    }
  }
  const wsFolders = vscode.workspace.workspaceFolders;
  if (!wsFolders || wsFolders.length === 0) {
    void vscode.window.showErrorMessage('No workspace folder open.');
    return;
  }
  const wsRoot = wsFolders[0].uri.fsPath;
  const resolved = await resolveDefinition(editor.document.uri, position);
  let className = fallbackName;
  let modulePath = '';
  if (resolved) {
    if (resolved.className) className = resolved.className;
    modulePath = computeModulePath(resolved.fsPath);
  }
  if (!modulePath) modulePath = computeModulePath(editor.document.uri.fsPath);
  const suggested = modulePath && className ? `${modulePath}.${className}` : className;
  const fqcn = await vscode.window.showInputBox({
    title: 'Add to runtime-evaluated-base-classes',
    prompt: 'Fully-qualified class name to add to pyproject.toml',
    value: suggested,
    validateInput: (v) => (v && v.trim() ? null : 'Cannot be empty'),
  });
  if (!fqcn) return;
  const pyprojectPath = path.join(wsRoot, 'pyproject.toml');
  if (!fs.existsSync(pyprojectPath)) {
    void vscode.window.showErrorMessage('pyproject.toml not found in workspace root.');
    return;
  }
  const original = fs.readFileSync(pyprojectPath, 'utf8');
  if (original.includes(`"${fqcn.trim()}"`)) {
    void vscode.window.showInformationMessage(`"${fqcn}" is already listed in runtime-evaluated-base-classes.`);
    return;
  }
  const updated = insertIntoRuntimeBaseClasses(ensureRuntimeBaseClassesArray(original), fqcn.trim());
  if (updated === null) {
    void vscode.window.showErrorMessage('Could not locate runtime-evaluated-base-classes array in pyproject.toml.');
    return;
  }
  fs.writeFileSync(pyprojectPath, updated, 'utf8');
  void vscode.window.showInformationMessage(`Added "${fqcn}" to runtime-evaluated-base-classes in pyproject.toml.`);
}
```

`vscode-extension/test/runtimeBaseClasses.test.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { computeModulePath, ensureRuntimeBaseClassesArray, insertIntoRuntimeBaseClasses } from '../src/runtimeBaseClasses';

describe('insertIntoRuntimeBaseClasses', () => {
  it('appends before the closing bracket with the existing indent', () => {
    const toml = '[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n    "a.B",\n  ]\n';
    expect(insertIntoRuntimeBaseClasses(toml, 'c.D')).toBe(
      '[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n    "a.B",\n    "c.D",\n  ]\n',
    );
  });
  it('uses the default indent for an empty array and rejects inline arrays', () => {
    expect(insertIntoRuntimeBaseClasses('runtime-evaluated-base-classes = [\n]\n', 'x.Y')).toBe(
      'runtime-evaluated-base-classes = [\n      "x.Y",\n]\n',
    );
    expect(insertIntoRuntimeBaseClasses('runtime-evaluated-base-classes = ["a"]\n', 'x.Y')).toBeNull();
    expect(insertIntoRuntimeBaseClasses('[tool]\n', 'x.Y')).toBeNull();
  });
});

describe('ensureRuntimeBaseClassesArray', () => {
  it('leaves an existing array alone', () => {
    const toml = 'runtime-evaluated-base-classes = [\n]\n';
    expect(ensureRuntimeBaseClassesArray(toml)).toBe(toml);
  });
  it('adds the array under the existing table or appends the table', () => {
    expect(ensureRuntimeBaseClassesArray('[tool.ruff.lint.flake8-type-checking]\n  strict = true\n')).toBe(
      '[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n  ]\n  strict = true\n',
    );
    expect(ensureRuntimeBaseClassesArray('[project]\nname = "x"\n\n')).toBe(
      '[project]\nname = "x"\n\n[tool.ruff.lint.flake8-type-checking]\n  runtime-evaluated-base-classes = [\n  ]\n',
    );
  });
});

describe('computeModulePath', () => {
  it('walks up through packages and drops __init__', () => {
    const pkg = new Set(['/w/src/pkg/__init__.py', '/w/src/pkg/sub/__init__.py']);
    const exists = (p: string) => pkg.has(p.replace(/\\/g, '/'));
    expect(computeModulePath('/w/src/pkg/sub/mod.py', exists)).toBe('pkg.sub.mod');
    expect(computeModulePath('/w/src/pkg/sub/__init__.py', exists)).toBe('pkg.sub');
    expect(computeModulePath('/w/src/other.py', exists)).toBe('other');
    expect(computeModulePath('', exists)).toBe('');
  });
});
```

- [ ] **Step 3: Install, test, typecheck, build**

Run (in `vscode-extension/`): `npm install && npm test && npm run typecheck && npm run build`
Expected: `package-lock.json` created (commit it: `npm ci` in CI needs it), 7 tests pass, no type errors, `dist/extension.js` exists. If `@types/vscode` complains that `TabInputTextDiff` etc. are missing, raise `engines.vscode` and the types version together (both `^1.90.0` is fine).

- [ ] **Step 4: Commit**

```bash
git add vscode-extension .gitignore
git commit -m "feat(vscode-extension): scaffold and port addToRuntimeBaseClasses" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 10: Extension consent core (pure): protocol, cache dir, request validation, hunk state

**Files:**
- Create: `vscode-extension/src/consent.ts`
- Test: `vscode-extension/test/consent.test.ts`

**Interfaces:**
- Produces: `PROTOCOL`, `EXTENSION_ID`, `SCHEME`, types `Hunk`, `Request`, `Response`, `ReviewRequest`, `Session`; `cacheDir(env, platform, home)`, `ID_PATTERN`, `requestPath(cache, id)`, `isInside(dir, file)`, `parseRequest(text, cache): Request`, `class HunkState`, `writeResponse(path, response)`, `cancelPath(req)`.

- [ ] **Step 1: Write the module**

```ts
// The consent protocol as the extension sees it. No `vscode` import: everything here is
// unit-tested under vitest. The CLI owns every decision; this side reports choices.
import * as fs from 'node:fs';
import * as path from 'node:path';

export const PROTOCOL = 1;
export const EXTENSION_ID = 'aeth.aeth-devkit';
export const SCHEME = 'aeth-devkit-proposed';

/** `[start, end)` 0-based line ranges in each text; context excluded. */
export interface Hunk {
  current: [number, number];
  proposed: [number, number];
}

export interface Request {
  protocol: number;
  id: string;
  title: string;
  current_path: string;
  proposed_path: string;
  hunks: Hunk[];
  offer_replace_all: boolean;
  content_menu: boolean;
  response_path: string;
}

export interface ReviewRequest {
  protocol: number;
  id: string;
  files: { path: string; label: string; current_path: string | null; proposed_path: string }[];
}

export type Response =
  | { decision: 'replace' }
  | { decision: 'replace_all' }
  | { decision: 'keep' }
  | { decision: 'partial'; accepted: number[] }
  | { decision: 'dismissed' }
  | { decision: 'error'; message: string };

/** devkit's cache dir, computed exactly as `aeth_devkit_core::update::cache_dir` does. */
export function cacheDir(env: NodeJS.ProcessEnv, platform: NodeJS.Platform, home: string): string | undefined {
  if (platform === 'win32') return env.LOCALAPPDATA ? path.join(env.LOCALAPPDATA, 'aeth-devkit') : undefined;
  return path.join(env.XDG_CACHE_HOME || path.join(home, '.cache'), 'aeth-devkit');
}

/** Consent ids are `<pid>-<n>`, review ids `review-<pid>`; nothing else reaches the disk. */
export const ID_PATTERN = /^(\d+-\d+|review-\d+)$/;

export function requestPath(cache: string, id: string): string {
  if (!ID_PATTERN.test(id)) throw new Error(`malformed request id: ${JSON.stringify(id)}`);
  return path.join(cache, 'consent', `${id}.request.json`);
}

/** Whether `file` is strictly inside `dir` (any `vscode://` link can name a request). */
export function isInside(dir: string, file: string): boolean {
  const rel = path.relative(path.resolve(dir), path.resolve(file));
  return rel !== '' && !rel.startsWith('..') && !path.isAbsolute(rel);
}

export function parseRequest(text: string, cache: string): Request {
  const r = JSON.parse(text) as Partial<Request>;
  const ok =
    typeof r.protocol === 'number' &&
    typeof r.id === 'string' &&
    typeof r.title === 'string' &&
    typeof r.current_path === 'string' &&
    typeof r.proposed_path === 'string' &&
    typeof r.response_path === 'string' &&
    Array.isArray(r.hunks);
  if (!ok) throw new Error('malformed consent request');
  for (const p of [r.current_path, r.proposed_path, r.response_path]) {
    if (!isInside(cache, p)) throw new Error(`request path outside the devkit cache: ${p}`);
  }
  return r as Request;
}

export function cancelPath(req: Request): string {
  return req.response_path.replace(/\.response\.json$/, '.cancel');
}

/** Per-hunk accept/reject, accepted by default. */
export class HunkState {
  readonly accepted: boolean[];

  constructor(count: number) {
    this.accepted = Array<boolean>(count).fill(true);
  }

  toggle(i: number): void {
    if (i >= 0 && i < this.accepted.length) this.accepted[i] = !this.accepted[i];
  }

  set(i: number, on: boolean): void {
    if (i >= 0 && i < this.accepted.length) this.accepted[i] = on;
  }

  acceptAll(): void {
    this.accepted.fill(true);
  }

  get acceptedCount(): number {
    return this.accepted.filter(Boolean).length;
  }

  /** `Apply accepted`: every hunk is a plain replace, none a keep, otherwise partial. */
  response(): Response {
    const idx = this.accepted.flatMap((a, i) => (a ? [i] : []));
    if (idx.length === this.accepted.length) return { decision: 'replace' };
    if (idx.length === 0) return { decision: 'keep' };
    return { decision: 'partial', accepted: idx };
  }
}

/** Temp file + rename: the CLI polls this path and must never read a half-written file. */
export function writeResponse(responsePath: string, response: Response): void {
  const tmp = `${responsePath}.tmp`;
  fs.writeFileSync(tmp, JSON.stringify(response), 'utf8');
  fs.renameSync(tmp, responsePath);
}

/** One open consent diff. `answered` stops the tab-close handler writing `dismissed`. */
export interface Session {
  req: Request;
  state: HunkState;
  answered: boolean;
}
```

- [ ] **Step 2: Write the tests**

`vscode-extension/test/consent.test.ts`:

```ts
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';
import { describe, expect, it } from 'vitest';
import { HunkState, cacheDir, cancelPath, isInside, parseRequest, requestPath, writeResponse } from '../src/consent';

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'aeth-consent-'));

const request = (cache: string) =>
  JSON.stringify({
    protocol: 1,
    id: '12-0',
    title: 'docker/Dockerfile',
    current_path: path.join(cache, 'consent', '12-0.current'),
    proposed_path: path.join(cache, 'consent', '12-0.proposed'),
    hunks: [{ current: [1, 2], proposed: [1, 2] }],
    offer_replace_all: true,
    content_menu: false,
    response_path: path.join(cache, 'consent', '12-0.response.json'),
  });

describe('cacheDir', () => {
  it('matches the CLI on every platform', () => {
    expect(cacheDir({ LOCALAPPDATA: 'C:\\Users\\j\\AppData\\Local' }, 'win32', 'C:\\Users\\j')).toBe(
      path.join('C:\\Users\\j\\AppData\\Local', 'aeth-devkit'),
    );
    expect(cacheDir({}, 'win32', 'C:\\Users\\j')).toBeUndefined();
    expect(cacheDir({}, 'linux', '/home/j')).toBe(path.join('/home/j/.cache', 'aeth-devkit'));
    expect(cacheDir({ XDG_CACHE_HOME: '/x' }, 'linux', '/home/j')).toBe(path.join('/x', 'aeth-devkit'));
  });
});

describe('requestPath and isInside', () => {
  it('accepts only well-formed ids', () => {
    expect(requestPath('/c', '12-0')).toBe(path.join('/c', 'consent', '12-0.request.json'));
    expect(requestPath('/c', 'review-12')).toBe(path.join('/c', 'consent', 'review-12.request.json'));
    for (const bad of ['', '../x', '12-0/../../etc', 'review', 'a-b']) expect(() => requestPath('/c', bad)).toThrow();
  });
  it('rejects paths outside the cache', () => {
    const c = tmp();
    expect(isInside(c, path.join(c, 'consent', 'x'))).toBe(true);
    expect(isInside(c, c)).toBe(false);
    expect(isInside(c, path.join(c, '..', 'x'))).toBe(false);
    expect(isInside(c, path.join(os.tmpdir(), 'elsewhere'))).toBe(false);
  });
});

describe('parseRequest', () => {
  it('parses a spec request and rejects escapes and garbage', () => {
    const c = tmp();
    const r = parseRequest(request(c), c);
    expect(r.hunks[0].proposed).toEqual([1, 2]);
    expect(cancelPath(r)).toBe(path.join(c, 'consent', '12-0.cancel'));
    const escaped = request(c).replace(/"response_path":"[^"]*"/, `"response_path":${JSON.stringify(path.join(c, '..', 'evil.json'))}`);
    expect(() => parseRequest(escaped, c)).toThrow(/outside the devkit cache/);
    expect(() => parseRequest('{"id":"12-0"}', c)).toThrow(/malformed/);
  });
});

describe('HunkState', () => {
  it('collapses all-accepted to replace and none to keep', () => {
    const s = new HunkState(3);
    expect(s.response()).toEqual({ decision: 'replace' });
    s.toggle(1);
    expect(s.acceptedCount).toBe(2);
    expect(s.response()).toEqual({ decision: 'partial', accepted: [0, 2] });
    s.set(0, false);
    s.set(2, false);
    expect(s.response()).toEqual({ decision: 'keep' });
    s.acceptAll();
    expect(s.response()).toEqual({ decision: 'replace' });
    s.toggle(99);
    expect(s.acceptedCount).toBe(3);
  });
});

describe('writeResponse', () => {
  it('leaves only the final file behind', () => {
    const dir = tmp();
    const p = path.join(dir, 'r.response.json');
    writeResponse(p, { decision: 'partial', accepted: [0] });
    expect(JSON.parse(fs.readFileSync(p, 'utf8'))).toEqual({ decision: 'partial', accepted: [0] });
    expect(fs.readdirSync(dir)).toEqual(['r.response.json']);
  });
});
```

- [ ] **Step 3: Run the tests**

Run (in `vscode-extension/`): `npm test && npm run typecheck`
Expected: all pass. On Windows the `isInside(c, path.join(c, '..', 'x'))` case relies on `path.relative` producing `..\x`; it does.

- [ ] **Step 4: Commit**

```bash
git add vscode-extension/src/consent.ts vscode-extension/test/consent.test.ts
git commit -m "feat(vscode-extension): consent protocol, request validation, and hunk state" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---
### Task 11: The consent diff in VS Code: content provider, CodeLens, URI handler, commands

**Files:**
- Create: `vscode-extension/src/proposedDocs.ts`, `vscode-extension/src/lenses.ts`
- Modify: `vscode-extension/src/extension.ts`

**Interfaces:**
- Consumes: everything from `consent.ts`.
- Produces: `ProposedDocs` (`register(id, side, title, text): Uri`, `forget(id)`), `parseUri(uri): { id, side } | undefined`, `ConsentLenses` (`refresh()`), the eight commands, `handleUri` for `/consent` (Task 12 adds `/review`).

- [ ] **Step 1: `proposedDocs.ts`**

```ts
import * as vscode from 'vscode';
import { SCHEME } from './consent';

/**
 * Read-only texts the CLI wrote, served as `aeth-devkit-proposed:/<id>/<side>/<title>`.
 * Both sides of the diff come from here (never the real file), so an unsaved editor
 * buffer can never shift the hunk numbering the CLI computed.
 */
export class ProposedDocs implements vscode.TextDocumentContentProvider {
  private readonly texts = new Map<string, string>();

  register(id: string, side: 'current' | 'proposed', title: string, text: string): vscode.Uri {
    const uri = vscode.Uri.from({ scheme: SCHEME, path: `/${id}/${side}/${title}` });
    this.texts.set(uri.path, text);
    return uri;
  }

  forget(id: string): void {
    for (const key of [...this.texts.keys()]) if (key.startsWith(`/${id}/`)) this.texts.delete(key);
  }

  provideTextDocumentContent(uri: vscode.Uri): string {
    return this.texts.get(uri.path) ?? '';
  }
}

/** `{ id, side }` for a document of this scheme, else undefined. Titles may contain `/`. */
export function parseUri(uri: vscode.Uri): { id: string; side: string } | undefined {
  if (uri.scheme !== SCHEME) return undefined;
  const [, id, side] = uri.path.split('/');
  return id && side ? { id, side } : undefined;
}
```

- [ ] **Step 2: `lenses.ts`**

```ts
import * as vscode from 'vscode';
import type { Session } from './consent';
import { parseUri } from './proposedDocs';

/**
 * Line 0: the whole-file decisions. Above each hunk: its accept/reject toggle showing the
 * current state. Only the proposed (right-hand) document gets lenses.
 */
export class ConsentLenses implements vscode.CodeLensProvider {
  private readonly emitter = new vscode.EventEmitter<void>();
  readonly onDidChangeCodeLenses = this.emitter.event;

  constructor(private readonly session: (id: string) => Session | undefined) {}

  refresh(): void {
    this.emitter.fire();
  }

  provideCodeLenses(doc: vscode.TextDocument): vscode.CodeLens[] {
    const at = parseUri(doc.uri);
    if (!at || at.side !== 'proposed') return [];
    const s = this.session(at.id);
    if (!s) return [];
    const top = new vscode.Range(0, 0, 0, 0);
    const m = s.req.hunks.length;
    const lenses = [
      new vscode.CodeLens(top, {
        title: `$(check-all) Apply accepted (${s.state.acceptedCount} of ${m})`,
        command: 'aeth-devkit.applyAccepted',
        arguments: [at.id],
      }),
      new vscode.CodeLens(top, { title: 'Accept all hunks', command: 'aeth-devkit.acceptAllHunks', arguments: [at.id] }),
    ];
    if (s.req.offer_replace_all) {
      lenses.push(new vscode.CodeLens(top, { title: 'Replace all (rest of this run)', command: 'aeth-devkit.replaceAll', arguments: [at.id] }));
    }
    lenses.push(new vscode.CodeLens(top, { title: 'Keep file', command: 'aeth-devkit.keepFile', arguments: [at.id] }));
    s.req.hunks.forEach((h, i) => {
      // A pure deletion has an empty proposed range at the end; clamp into the document.
      const line = Math.min(h.proposed[0], Math.max(doc.lineCount - 1, 0));
      const on = s.state.accepted[i];
      lenses.push(
        new vscode.CodeLens(new vscode.Range(line, 0, line, 0), {
          title: on ? `$(check) Hunk ${i + 1} accepted — reject` : `$(x) Hunk ${i + 1} rejected — accept`,
          command: on ? 'aeth-devkit.rejectHunk' : 'aeth-devkit.acceptHunk',
          arguments: [at.id, i],
        }),
      );
    });
    return lenses;
  }
}
```

- [ ] **Step 3: `extension.ts`**

Replace the file with:

```ts
import * as fs from 'node:fs';
import * as os from 'node:os';
import * as vscode from 'vscode';
import {
  HunkState,
  PROTOCOL,
  Request,
  Response,
  SCHEME,
  Session,
  cacheDir,
  cancelPath,
  parseRequest,
  requestPath,
  writeResponse,
} from './consent';
import { ConsentLenses } from './lenses';
import { ProposedDocs, parseUri } from './proposedDocs';
import { openReview } from './review';
import { addToRuntimeBaseClasses } from './runtimeBaseClasses';

interface OpenSession extends Session {
  proposed: vscode.Uri;
  /** Polls for the CLI's `<id>.cancel` marker (Ctrl-C in the terminal). */
  cancelPoll: NodeJS.Timeout;
}

const sessions = new Map<string, OpenSession>();
const docs = new ProposedDocs();
const lenses = new ConsentLenses((id) => sessions.get(id));

export function activate(context: vscode.ExtensionContext): void {
  context.subscriptions.push(
    vscode.workspace.registerTextDocumentContentProvider(SCHEME, docs),
    vscode.languages.registerCodeLensProvider({ scheme: SCHEME }, lenses),
    vscode.window.registerUriHandler({ handleUri }),
    vscode.window.tabGroups.onDidChangeTabs(onTabsChanged),
    vscode.commands.registerCommand('aeth-devkit.acceptHunk', (id: string, i: number) => setHunk(id, i, true)),
    vscode.commands.registerCommand('aeth-devkit.rejectHunk', (id: string, i: number) => setHunk(id, i, false)),
    vscode.commands.registerCommand('aeth-devkit.acceptAllHunks', (id: string) => {
      sessions.get(id)?.state.acceptAll();
      lenses.refresh();
    }),
    vscode.commands.registerCommand('aeth-devkit.applyAccepted', (id: string) => {
      const s = sessions.get(id);
      if (s) void decide(s, s.state.response());
    }),
    vscode.commands.registerCommand('aeth-devkit.replaceFile', (arg?: unknown) => withSession(arg, (s) => decide(s, { decision: 'replace' }))),
    vscode.commands.registerCommand('aeth-devkit.replaceAll', (arg?: unknown) => withSession(arg, (s) => decide(s, { decision: 'replace_all' }))),
    vscode.commands.registerCommand('aeth-devkit.keepFile', (arg?: unknown) => withSession(arg, (s) => decide(s, { decision: 'keep' }))),
    vscode.commands.registerCommand('aeth-devkit.addToRuntimeBaseClasses', addToRuntimeBaseClasses),
  );
}

export function deactivate(): void {
  for (const s of sessions.values()) clearInterval(s.cancelPoll);
}

function fail(message: string): void {
  void vscode.window.showErrorMessage(`aeth-devkit: ${message}`);
}

async function handleUri(uri: vscode.Uri): Promise<void> {
  // The URL carries only an id; the file lives where this extension expects devkit's
  // cache, so a link from anywhere else can name nothing outside that folder.
  const id = new URLSearchParams(uri.query).get('id') ?? '';
  const cache = cacheDir(process.env, process.platform, os.homedir());
  if (!cache) return fail('cannot locate the devkit cache directory');
  let file: string;
  try {
    file = requestPath(cache, id);
  } catch (e) {
    return fail((e as Error).message);
  }
  if (uri.path === '/review') return openReview(file, cache, docs);
  if (uri.path !== '/consent') return;
  let req: Request;
  try {
    req = parseRequest(fs.readFileSync(file, 'utf8'), cache);
  } catch (e) {
    return fail((e as Error).message);
  }
  if (req.protocol !== PROTOCOL) {
    writeResponse(req.response_path, { decision: 'error', message: `the extension speaks protocol ${PROTOCOL}, devkit sent ${req.protocol}; update one of them` });
    return;
  }
  await ensureDiffCodeLens();
  await vscode.commands.executeCommand('setContext', 'aeth-devkit.contentMenu', req.content_menu);
  const current = docs.register(req.id, 'current', req.title, fs.readFileSync(req.current_path, 'utf8'));
  const proposed = docs.register(req.id, 'proposed', req.title, fs.readFileSync(req.proposed_path, 'utf8'));
  const s: OpenSession = {
    req,
    state: new HunkState(req.hunks.length),
    answered: false,
    proposed,
    cancelPoll: setInterval(() => {
      if (fs.existsSync(cancelPath(req))) {
        s.answered = true;
        void closeTab(s);
      }
    }, 250),
  };
  sessions.set(req.id, s);
  await vscode.commands.executeCommand('vscode.diff', current, proposed, `devkit: ${req.title}`, { preview: false });
}

/** `diffEditor.codeLens` is off by default; without it the per-hunk lenses never show. */
async function ensureDiffCodeLens(): Promise<void> {
  const cfg = vscode.workspace.getConfiguration('diffEditor');
  const info = cfg.inspect<boolean>('codeLens');
  if (info?.globalValue === undefined && info?.workspaceValue === undefined) {
    await cfg.update('codeLens', true, vscode.ConfigurationTarget.Global);
    return;
  }
  if (cfg.get<boolean>('codeLens') === false) {
    void vscode.window.showWarningMessage('aeth-devkit: diffEditor.codeLens is off, so per-hunk Accept/Reject is hidden; the whole-file buttons still work.');
  }
}

function setHunk(id: string, i: number, on: boolean): void {
  sessions.get(id)?.state.set(i, on);
  lenses.refresh();
}

/** Title/content buttons receive the resource URI; the palette gives nothing. */
function withSession(arg: unknown, f: (s: OpenSession) => Promise<void>): void {
  const uri = arg instanceof vscode.Uri ? arg : vscode.window.activeTextEditor?.document.uri;
  const id = uri ? parseUri(uri)?.id : undefined;
  const s = id ? sessions.get(id) : sessions.size === 1 ? [...sessions.values()][0] : undefined;
  if (s) void f(s);
  else void vscode.window.showWarningMessage('aeth-devkit: no open consent diff.');
}

async function decide(s: OpenSession, r: Response): Promise<void> {
  s.answered = true;
  writeResponse(s.req.response_path, r);
  await closeTab(s);
}

async function closeTab(s: OpenSession): Promise<void> {
  clearInterval(s.cancelPoll);
  sessions.delete(s.req.id);
  docs.forget(s.req.id);
  lenses.refresh();
  for (const group of vscode.window.tabGroups.all) {
    for (const tab of group.tabs) {
      if (tab.input instanceof vscode.TabInputTextDiff && tab.input.modified.toString() === s.proposed.toString()) {
        await vscode.window.tabGroups.close(tab);
      }
    }
  }
}

/** A diff closed by the user (not by `decide`/`closeTab`) is a dismissal. */
function onTabsChanged(e: vscode.TabChangeEvent): void {
  for (const tab of e.closed) {
    if (!(tab.input instanceof vscode.TabInputTextDiff)) continue;
    const at = parseUri(tab.input.modified);
    const s = at ? sessions.get(at.id) : undefined;
    if (s && !s.answered) {
      s.answered = true;
      writeResponse(s.req.response_path, { decision: 'dismissed' });
      void closeTab(s);
    }
  }
}
```

`./review` does not exist yet; create `vscode-extension/src/review.ts` with a placeholder export so the build passes, replaced in Task 12:

```ts
import type { ProposedDocs } from './proposedDocs';

export async function openReview(_file: string, _cache: string, _docs: ProposedDocs): Promise<void> {}
```

- [ ] **Step 4: Typecheck and build**

Run (in `vscode-extension/`): `npm run typecheck && npm run build && npm test`
Expected: clean. If `vscode.TabInputTextDiff` or `tabGroups` are unknown, the `@types/vscode` version is below 1.67; it must be `^1.90.0`.

- [ ] **Step 5: Commit**

```bash
git add vscode-extension/src
git commit -m "feat(vscode-extension): consent diff with per-hunk CodeLens and whole-file controls" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 12: Review mode for `--dry-run`

**Files:**
- Modify: `vscode-extension/src/review.ts`

**Interfaces:**
- Consumes: `ReviewRequest`, `isInside` from `consent.ts`; `ProposedDocs`.
- Produces: `openReview(file, cache, docs): Promise<void>`.

- [ ] **Step 1: Implement**

```ts
import * as fs from 'node:fs';
import * as vscode from 'vscode';
import { ReviewRequest, isInside } from './consent';
import { ProposedDocs } from './proposedDocs';

/** `--dry-run`: every change in one multi-diff editor, read-only, nothing awaited. */
export async function openReview(file: string, cache: string, docs: ProposedDocs): Promise<void> {
  let req: ReviewRequest;
  try {
    req = JSON.parse(fs.readFileSync(file, 'utf8')) as ReviewRequest;
    if (!Array.isArray(req.files) || typeof req.id !== 'string') throw new Error('malformed review request');
    for (const f of req.files) {
      for (const p of [f.current_path, f.proposed_path]) {
        if (p && !isInside(cache, p)) throw new Error(`review path outside the devkit cache: ${p}`);
      }
    }
  } catch (e) {
    void vscode.window.showErrorMessage(`aeth-devkit: ${(e as Error).message}`);
    return;
  }
  // `vscode.changes` takes [label, original, modified] triples; a created file diffs
  // against an empty document.
  const resources = req.files.map((f, i): [vscode.Uri, vscode.Uri, vscode.Uri] => [
    vscode.Uri.file(f.path),
    docs.register(req.id, 'current', `${i}/${f.label}`, f.current_path ? fs.readFileSync(f.current_path, 'utf8') : ''),
    docs.register(req.id, 'proposed', `${i}/${f.label}`, fs.readFileSync(f.proposed_path, 'utf8')),
  ]);
  await vscode.commands.executeCommand('vscode.changes', 'devkit setup-project (dry run)', resources);
}
```

- [ ] **Step 2: Typecheck and build**

Run (in `vscode-extension/`): `npm run typecheck && npm run build`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add vscode-extension/src/review.ts
git commit -m "feat(vscode-extension): multi-diff review for setup-project --dry-run" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---
### Task 13: Packaging script and the tag-triggered release workflow

**Files:**
- Create: `vscode-extension/scripts/package.sh`
- Create: `.github/workflows/vscode-extension.yml`

**Interfaces:**
- Produces: `scripts/package.sh N` → `vscode-extension/aeth-devkit-vscode-N.vsix` with manifest version `N.0.0`.

- [ ] **Step 1: The packaging script**

`vscode-extension/scripts/package.sh` (commit with the executable bit: `git update-index --chmod=+x`):

```bash
#!/usr/bin/env bash
# Build and package the extension as aeth-devkit-vscode-N.vsix, manifest version N.0.0.
# The repo's package.json stays at 0.0.0: the version is stamped here only.
set -euo pipefail
n="${1:?usage: package.sh N}"
cd "$(dirname "$0")/.."
npm run build
npx @vscode/vsce package "$n.0.0" \
  --no-dependencies --no-git-tag-version --no-update-package-json \
  --allow-missing-repository \
  --out "aeth-devkit-vscode-$n.vsix"
```

Run (in `vscode-extension/`): `bash scripts/package.sh 0` then `unzip -l aeth-devkit-vscode-0.vsix | grep -E 'extension/(package.json|dist/extension.js)'`.
Expected: both entries listed, and `unzip -p aeth-devkit-vscode-0.vsix extension.vsixmanifest | grep -o 'Version="0.0.0"'` prints the stamped version. Then `git status` must show `package.json` unmodified.

If `vsce package` refuses the manifest because of `enabledApiProposals`, replace the `npx` line with the zip fallback below (a `.vsix` is a zip with a manifest) and re-run the same checks:

```bash
v="$n.0.0"
rm -rf .package && mkdir -p .package/extension
cp -r dist package.json README.md .package/extension/
node -e "const p=require('./.package/extension/package.json');p.version='$v';require('fs').writeFileSync('./.package/extension/package.json',JSON.stringify(p,null,2))"
cat > .package/extension.vsixmanifest <<EOF
<?xml version="1.0" encoding="utf-8"?>
<PackageManifest Version="2.0.0" xmlns="http://schemas.microsoft.com/developer/vsx-schema/2011" xmlns:d="http://schemas.microsoft.com/developer/vsx-schema-design/2011">
  <Metadata>
    <Identity Language="en-US" Id="aeth-devkit" Version="$v" Publisher="aeth"/>
    <DisplayName>aeth-devkit</DisplayName>
    <Description xml:space="preserve">In-editor consent for devkit setup-project.</Description>
    <Categories>Other</Categories>
    <GalleryFlags>Private</GalleryFlags>
    <Properties>
      <Property Id="Microsoft.VisualStudio.Code.Engine" Value="^1.90.0"/>
      <Property Id="Microsoft.VisualStudio.Code.ExtensionKind" Value="ui,workspace"/>
    </Properties>
  </Metadata>
  <Installation><InstallationTarget Id="Microsoft.VisualStudio.Code"/></Installation>
  <Dependencies/>
  <Assets><Asset Type="Microsoft.VisualStudio.Code.Manifest" Path="extension/package.json" Addressable="true"/></Assets>
</PackageManifest>
EOF
cat > '.package/[Content_Types].xml' <<EOF
<?xml version="1.0" encoding="utf-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension=".json" ContentType="application/json"/>
  <Default Extension=".js" ContentType="application/javascript"/>
  <Default Extension=".md" ContentType="text/markdown"/>
  <Default Extension=".vsixmanifest" ContentType="text/xml"/>
</Types>
EOF
(cd .package && zip -qr "../aeth-devkit-vscode-$n.vsix" .)
rm -rf .package
```

Also add `.package/` to `.vscodeignore` and `vscode-extension/.package/` to the repo `.gitignore` if the fallback is used. Delete the local `aeth-devkit-vscode-0.vsix` afterwards (it is gitignored anyway).

- [ ] **Step 2: The workflow**

`.github/workflows/vscode-extension.yml`:

```yaml
# Releases the VS Code extension on its own tag stream (`vscode-extension-vN`) whenever a
# devkit release ships with changes under `vscode-extension/` since the previous extension
# tag. It fires on the same `release: published` event as release.yml, so `devkit release`
# needs no knowledge of the extension. The release it creates uses the workflow token,
# and events raised with that token never start workflows, so nothing re-fires.
name: VS Code extension

on:
  release:
    types: [published]
  workflow_dispatch:

permissions:
  contents: read

env:
  DEVKIT_TAG: ${{ github.event.release.tag_name || github.sha }}

jobs:
  release:
    # Only devkit's own `v*` tags; a hand-made extension release must not recurse.
    if: github.event_name == 'workflow_dispatch' || startsWith(github.event.release.tag_name, 'v')
    runs-on: ubuntu-latest
    permissions:
      contents: write
    defaults:
      run:
        working-directory: vscode-extension
    steps:
      - uses: actions/checkout@v4
        with:
          ref: ${{ github.sha }}
          # Every tag, to find the previous extension release.
          fetch-depth: 0
          persist-credentials: false

      - name: Next extension version, if anything changed
        id: version
        shell: bash
        run: |
          prev="$(git tag --list 'vscode-extension-v*' | sed 's/^vscode-extension-v//' | sort -n | tail -1)"
          if [ -n "$prev" ] && git diff --quiet "vscode-extension-v$prev" "$GITHUB_SHA" -- .; then
            echo "extension unchanged since vscode-extension-v$prev; nothing to release"
            exit 0
          fi
          echo "n=$(( ${prev:-0} + 1 ))" >> "$GITHUB_OUTPUT"

      - if: steps.version.outputs.n
        uses: actions/setup-node@v4
        with:
          node-version: lts/*
          cache: npm
          cache-dependency-path: vscode-extension/package-lock.json

      - if: steps.version.outputs.n
        run: npm ci

      - if: steps.version.outputs.n
        run: npm test

      - if: steps.version.outputs.n
        run: bash scripts/package.sh "${{ steps.version.outputs.n }}"

      - if: steps.version.outputs.n
        name: Tag and release
        env:
          GH_TOKEN: ${{ github.token }}
          N: ${{ steps.version.outputs.n }}
        run: |
          gh release create "vscode-extension-v$N" --target "$GITHUB_SHA" \
            --title "VS Code extension $N" \
            --notes "aeth-devkit VS Code extension build $N, from devkit $DEVKIT_TAG. Installed by devkit setup-project; not published to the marketplace." \
            "aeth-devkit-vscode-$N.vsix"
```

- [ ] **Step 3: Validate the workflow file**

Run: `npx --yes @action-validator/cli .github/workflows/vscode-extension.yml` if available; otherwise `python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/vscode-extension.yml'))"` under `uv run` to confirm it parses. Also dry-run the version step locally from `vscode-extension/`:

```bash
prev="$(git tag --list 'vscode-extension-v*' | sed 's/^vscode-extension-v//' | sort -n | tail -1)"; echo "prev=${prev:-none} next=$(( ${prev:-0} + 1 ))"
```

Expected: `prev=none next=1`.

- [ ] **Step 4: Commit**

```bash
git add vscode-extension/scripts/package.sh .github/workflows/vscode-extension.yml vscode-extension/.vscodeignore .gitignore
git update-index --chmod=+x vscode-extension/scripts/package.sh
git commit -m "ci(vscode-extension): package script and tag-triggered extension release workflow" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

---

### Task 14: Docs, TODO, launch config, spec touch-up, full test run, manual smoke test

**Files:**
- Modify: `README.md` (the `setup-project` Docker bullet at lines 83–98 and a new `### VS Code extension` subsection after `### devkit-container`)
- Modify: `TODO.md`
- Modify: `.vscode/launch.json`
- Modify: `docs/superpowers/specs/2026-09-03-vscode-extension-design.md`

- [ ] **Step 1: README**

Replace the **Docker** bullet's consent sentences so it reads (keep the rule list in the middle unchanged):

```markdown
- **Docker** - Runs whenever `[tool.docker].services` lists at least one compose service.
  `docker/Dockerfile` (and any other templated file under `docker/`) is created when
  missing; when present and different — ignoring CRLF/LF — a unified diff is printed and
  the file is replaced only on `replace` (`replace all` answers every remaining Docker
  question; anything else keeps it). The compose file (docker-pin's discovery; created as
  `docker/compose.yaml` when absent) is edited in place, format-preserving, one diff per
  listed service: exact keys (`build.context`, `build.dockerfile`, `container_name`,
  `healthcheck.*`), pattern (`GIT_REPO` vs origin), presence (`GIT_TAG`, `restart`,
  `networks`) and at-least (`volumes` mounting `/app/persisted_data`; the ALERTS_*
  environment when the project uses aeth_ext), plus one diff for the top-level
  `networks.coolify.external`. A listed service missing from the file is its own one-hunk
  diff (accepting it adds the scaffold block; `replace all` and `--replace-docker` accept
  adds too). Keys the standard does not name are never touched. Without a terminal every
  answer is "keep" and a `note:` says so; `--dry-run`/`--check` print everything and count
  Docker drift. Inside a VS Code terminal the diff opens in the editor instead (see
  **VS Code extension**). `docker/entrypoint.sh` and `docker/scripts/` are reported as
  safe to delete, never removed.
```

Add after the `### devkit-container` section:

```markdown
### VS Code extension

`aeth.aeth-devkit`, in `vscode-extension/`. Never published to the marketplace: each build
is a GitHub release on its own tag stream (`vscode-extension-vN`, asset
`aeth-devkit-vscode-N.vsix`), cut automatically by `.github/workflows/vscode-extension.yml`
when a devkit release ships with changes under `vscode-extension/`.

When `devkit setup-project` runs in a VS Code terminal (`TERM_PROGRAM=vscode`; force with
`--vscode`, disable with `--no-vscode`) with stdin a terminal and neither `--check` nor
`--replace-docker`, it installs the newest compatible extension if none is present (a
one-off `code --install-extension`; an upgrade over a running one asks you to reload the
window and run again), adds itself to `enable-proposed-api` in `~/.vscode/argv.json`
(restart VS Code once; this enables the floating Replace/Keep button), and then opens
each Docker change as a native diff instead of the typed prompt. Per hunk: Accept/Reject
CodeLens (`diffEditor.codeLens` is switched on once if unset). Whole file: `Apply
accepted (n of m)`, `Accept all hunks`, `Replace all` (rest of the run), `Keep file`.
Closing the diff without deciding falls back to the terminal prompt for that file; Ctrl-C
in the terminal does the same, and a second Ctrl-C aborts. Partial answers are
reassembled by the CLI from the accepted hunk indices; the extension never writes project
files. `--dry-run` opens every proposed change in one multi-diff review instead.

The extension also carries `Add to runtime-evaluated-base-classes` (Python editor context
menu), ported from the Drekker extension; setup-project reports the old junction and
`.vscode/extension/` folder when it finds them.
```

- [ ] **Step 2: TODO.md**

Replace the line `- [ ] The VS Code extension design adds a `vsix` job to the Rust release workflow's matrix.` with:

```markdown
- [ ] VS Code extension: support `code-insiders` and `cursor` launchers (each has its own
      URI scheme, `argv.json` location and extensions dir); only `code` works today.
- [ ] After the first `vscode-extension-v1` release: delete `.vscode/extension/` and
      `install.ps1` from aeth_ext and aeth_ext-2, and the
      `~/.vscode/extensions/local.drekker-add-to-runtime-base-*` junction (setup-project
      prints a note while they exist).
```

- [ ] **Step 3: Extension development host launch config**

Add to the `configurations` array in `.vscode/launch.json` (non-Python entries are untouched by setup-project's launch patching):

```json
        {
            "name": "VS Code extension (dev host)",
            "type": "extensionHost",
            "request": "launch",
            "args": ["--extensionDevelopmentPath=${workspaceFolder}/vscode-extension"],
            "outFiles": ["${workspaceFolder}/vscode-extension/dist/**/*.js"]
        }
```

(The dev host grants proposed APIs to the extension under development, so the floating button shows there without an `argv.json` entry.)

- [ ] **Step 4: Spec touch-up**

In the spec, change the two URL mentions (`?request=<path>` in the *Request* paragraph and the *Extension behaviour* item 1) to `?id=<id>`, and reword item 1's first clause to: "The URI handler accepts only a well-formed id (`<pid>-<n>` or `review-<pid>`) and reads `<cache>/consent/<id>.request.json`, so a link from any web page can name nothing outside the cache". Under *Review mode*, note that the request's file entries carry `path`, `label`, `current_path` (null for a created file) and `proposed_path`.

- [ ] **Step 5: Full verification**

Run, from the repo root:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace
(cd vscode-extension && npm test && npm run typecheck && npm run build)
```

Expected: all green. Fix anything that fails before committing; do not weaken a test to pass unless the plan's own expectation was wrong (then say so in the commit body).

- [ ] **Step 6: Commit**

```bash
git add README.md TODO.md .vscode/launch.json docs/superpowers/specs/2026-09-03-vscode-extension-design.md
git commit -m "docs: VS Code extension section, TODO follow-ups, dev-host launch config" -m "Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>"
```

- [ ] **Step 7: Manual smoke test (needs a human at VS Code; report the checklist, do not skip silently)**

1. Press F5 on "VS Code extension (dev host)". In the dev host open a Docker project with drift, run `cargo run -p aeth-devkit-setup -- --vscode --no-commit` in its terminal. Confirm: a diff tab opens titled `devkit: docker/Dockerfile`; CodeLens line 0 shows `Apply accepted`; the floating Replace/Keep button shows (dev host grants the proposal); toggling a hunk and applying writes the partially replaced file; `changes.report` shows `(1 of N hunks)`.
2. Close a diff tab without deciding: the terminal prompt appears for that file; the next file opens in VS Code again.
3. Press Ctrl-C while a diff is open: the tab closes and the terminal asks; press Ctrl-C again at the prompt: the process exits.
4. `--dry-run`: a multi-diff editor opens listing every managed file that would change; nothing is written and nothing is awaited.
5. In stable VS Code without the `argv.json` entry: only title icons show (no floating button); after the restart the floating button shows and title icons are gone.
6. On Windows, `code.cmd` on PATH: `--open-url` succeeds (the URL has no `%`, so cmd.exe escaping is not exercised).
7. `vsce package` accepted `enabledApiProposals` (Task 13 Step 1); the installed `.vsix` activates on the `vscode://` URL and shows no "cannot use proposed API" error in the extension host log.

Record any deviation in TODO.md before finishing.

---

## Self-review notes

- Spec coverage: goals 1–3 → Tasks 3, 8, 12 and 9; source layout → 9–12; proposed-API decision (`content_menu`, title icons, `diffEditor.codeLens`) → 7, 9, 11; versioning (`N`, protocol, `MIN_EXTENSION_VERSION`) → 2, 5, 13; release workflow → 13; setup-project steps 1–5 → 4, 5, 7, 8; consent protocol (per-service diffs, HEAD snapshot via cache files, request/response, dismissed, error, partial via indices) → 3, 6, 10, 11; interrupts and cleanup → 6, 7, 8; review mode → 8, 12; ported command → 9; testing lists → each task; documentation → 14.
- Known deviation from the spec, deliberate: the URL carries an id instead of a path (Global Constraints, Task 14 Step 4 updates the spec).
- Type consistency: `Deps.reviewer: Option<&dyn Reviewer>` (Tasks 3, 8); `Decision::text(self, &Proposal)` and `detail(&self, &str)` (3); `VsCode { launcher, consent_dir, content_menu, notes }` (6, 7, 8); `Preview { path, current, proposed }` (8); TS `Session { req, state, answered }` (10, 11); `ProposedDocs.register(id, side, title, text)` (11, 12).
