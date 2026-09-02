# poe Completion Thin Shim Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move poe completion logic out of two near-duplicate shell scripts into one Rust engine, leaving each shell a thin permanent shim, so `devkit complete` no longer requires a global devkit install.

**Architecture:** A pure engine (`(shell, words, cursor) -> Directive`) answers every completion; shells keep a ~15-line shim that forwards the command line and acts on a `dirs`/`files` sentinel. bash sends its raw line (split with `shell-words`); PowerShell sends its already-parsed AST elements. A shim-version integer on each request lets a drifted shim be repaired at Tab time.

**Tech Stack:** Rust 2024, clap, `shell-words`, existing `aeth-devkit-complete` cache/resolve layers.

**Spec:** `docs/superpowers/specs/2026-09-01-poe-completion-shim-design.md`

## Global Constraints

- **Dense teaching comments are mandatory.** Every Rust file in this repo is study material: comment around nearly every line, explaining both the Rust syntax in play (ownership, `?`, traits, enums, closures, lifetimes) and the logic. `///` doc comments for items, `//` for teaching inside bodies. The code blocks in this plan are abbreviated — shipped code carries more commentary, not less.
- **Two-space indentation** in Rust, matching the existing crates. `line-length = 135`.
- **A completer must never break the shell.** Every failure path returns empty output and exit code 0.
- **`--shim-version` is `1`** for all of this work, and is bumped only when shim text changes.
- **No new public API** beyond what tasks below specify. `tasks`, `args`, `script` subcommands are retained untouched.
- **Filter by prefix in Rust.** Items returned are already filtered; shims never re-filter.
- Run `cargo clippy --workspace --all-targets` before every commit; the workspace is currently warning-free and must stay that way.

---

### Task 1: Engine types and task-name completion

**Files:**
- Create: `crates/aeth-devkit-complete/src/engine.rs`
- Modify: `crates/aeth-devkit-complete/src/lib.rs` (add `pub mod engine;`)
- Test: `crates/aeth-devkit-complete/tests/engine.rs`

**Interfaces:**
- Consumes: `resolve::Task`, `resolve::TaskArg`, `cache::resolve_cached`, `aeth_devkit_core::process::Runner`
- Produces:
  - `enum Shell { Bash, PowerShell }`
  - `enum ItemKind { Command, Param, Value }`
  - `struct Item { value: String, display: String, tooltip: String, kind: ItemKind }`
  - `enum Directive { Items(Vec<Item>), Dirs, Files }`
  - `struct Request { shell: Shell, words: Vec<String>, cword: usize, prefix: String, root: PathBuf }`
  - `fn complete(req: &Request, runner: &dyn Runner, no_cache: bool) -> Directive`

- [ ] **Step 1: Write the failing test**

```rust
// tests/engine.rs
use aeth_devkit_complete::engine::{Directive, ItemKind, Request, Shell, complete};

/// A project whose pyproject declares two tasks; `fixture_project` writes it to a tempdir.
#[test]
fn completes_task_names_after_the_command() {
  let project = fixture_project(&["build", "test"]);
  let req = Request {
    shell: Shell::Bash,
    words: vec!["poe".into()],
    cword: 1,          // cursor sits one past the last word: a fresh word is being typed
    prefix: String::new(),
    root: project.path().to_path_buf(),
  };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else {
    panic!("expected items, got a file-completion sentinel");
  };
  let names: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
  assert_eq!(names, ["build", "test"]);
  assert!(matches!(items[0].kind, ItemKind::Command));
}

#[test]
fn filters_task_names_by_prefix() {
  let project = fixture_project(&["build", "bench", "test"]);
  let req = Request { prefix: "b".into(), cword: 1, ..base_request(&project) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  let names: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
  assert_eq!(names, ["build", "bench"]);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p aeth-devkit-complete --test engine`
Expected: FAIL — `unresolved import aeth_devkit_complete::engine`

- [ ] **Step 3: Write minimal implementation**

```rust
// src/engine.rs
/// Which shell asked. The engine is shell-agnostic except where result typing differs,
/// so this is deliberately the only shell knowledge inside the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell { Bash, PowerShell }

/// How a completion should be presented. bash ignores this; PowerShell maps it onto
/// `CompletionResultType`, which is what gives you the icon and grouping in the popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind { Command, Param, Value }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item { pub value: String, pub display: String, pub tooltip: String, pub kind: ItemKind }

/// What the shim should do with the answer. `Dirs`/`Files` are the sentinel: the engine
/// declines to enumerate paths and hands that back to the shell, which quotes them correctly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive { Items(Vec<Item>), Dirs, Files }

pub struct Request {
  pub shell: Shell,
  /// argv-style, `words[0]` is always the command itself (`poe`).
  pub words: Vec<String>,
  /// Index of the word being completed. Equals `words.len()` when a fresh word is starting.
  pub cword: usize,
  /// Text of that word to the left of the cursor — what candidates must start with.
  pub prefix: String,
  pub root: PathBuf,
}

impl Item {
  /// Most items repeat their value as the display text; this keeps call sites short.
  fn plain(value: &str, tooltip: &str, kind: ItemKind) -> Self {
    Self { value: value.to_string(), display: value.to_string(), tooltip: tooltip.to_string(), kind }
  }
}

pub fn complete(req: &Request, runner: &dyn Runner, no_cache: bool) -> Directive {
  // A completer must never fail loudly, so an unresolvable project yields no items
  // rather than an error the shell would print over the user's prompt.
  let Ok(resolved) = cache::resolve_cached(&req.root, runner, no_cache) else {
    return Directive::Items(Vec::new());
  };
  let items = resolved
    .tasks
    .iter()
    .filter(|t| t.name.starts_with(&req.prefix))
    .map(|t| Item::plain(&t.name, &format!("Task: {}", t.name), ItemKind::Command))
    .collect();
  Directive::Items(items)
}
```

- [ ] **Step 3b: Add the test fixture helpers**

`fixture_project(tasks: &[&str]) -> tempfile::TempDir` writes a minimal `pyproject.toml`
with `[tool.poe.tasks]` entries, and `base_request(&TempDir) -> Request` returns a
`Request` with `shell: Shell::Bash`, `words: vec!["poe".into()]`, `cword: 1`, empty prefix.
Place both at the bottom of `tests/engine.rs`; mirror the fixture style already used in
`tests/complete.rs`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p aeth-devkit-complete --test engine`
Expected: PASS, 2 tests

- [ ] **Step 5: Commit**

```bash
git add crates/aeth-devkit-complete/src/engine.rs crates/aeth-devkit-complete/src/lib.rs crates/aeth-devkit-complete/tests/engine.rs
git commit -m "feat(complete): engine core with task-name completion"
```

---

### Task 2: bash line splitting and cursor location

**Files:**
- Create: `crates/aeth-devkit-complete/src/words.rs`
- Modify: `crates/aeth-devkit-complete/Cargo.toml` (add `shell-words`), root `Cargo.toml` (workspace dep), `src/lib.rs`
- Test: `crates/aeth-devkit-complete/tests/words.rs`

**Interfaces:**
- Produces: `struct Split { words: Vec<String>, cword: usize, prefix: String }`, `fn split_line(line: &str, point: usize) -> Split`

**Why this exists:** bash gives `COMP_LINE` (raw text) and `COMP_POINT` (byte offset). Everything downstream wants words plus a cursor index, and this is the only place that conversion happens.

- [ ] **Step 1: Write the failing tests**

```rust
use aeth_devkit_complete::words::split_line;

#[test]
fn splits_a_plain_line_and_locates_the_cursor_at_the_end() {
  let s = split_line("poe build", 9);
  assert_eq!(s.words, ["poe", "build"]);
  assert_eq!(s.cword, 1);
  assert_eq!(s.prefix, "build");
}

#[test]
fn a_cursor_on_trailing_space_starts_a_fresh_word() {
  let s = split_line("poe ", 4);
  assert_eq!(s.words, ["poe"]);
  assert_eq!(s.cword, 1);          // one past the last word
  assert_eq!(s.prefix, "");
}

#[test]
fn a_cursor_mid_word_filters_on_the_left_portion_only() {
  // "poe bu|ild" — the design says the text right of the cursor is ignored.
  let s = split_line("poe build", 6);
  assert_eq!(s.cword, 1);
  assert_eq!(s.prefix, "bu");
}

#[test]
fn keeps_a_quoted_path_as_one_word() {
  let s = split_line("poe run \"my file.txt\"", 21);
  assert_eq!(s.words, ["poe", "run", "my file.txt"]);
  assert_eq!(s.cword, 2);
}

#[test]
fn an_unclosed_quote_does_not_panic() {
  // shell-words returns Err here; we must degrade to something usable, not unwrap.
  let s = split_line("poe run \"my fi", 14);
  assert_eq!(s.words[0], "poe");
  assert_eq!(s.cword, 2);
}

#[test]
fn inline_equals_stays_in_one_word() {
  // The whole point of taking the raw line: bash's own COMP_WORDS would split this.
  let s = split_line("poe -C=../other build", 15);
  assert_eq!(s.words, ["poe", "-C=../other", "build"]);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p aeth-devkit-complete --test words`
Expected: FAIL — unresolved import

- [ ] **Step 3: Add the dependency**

Root `Cargo.toml`, `[workspace.dependencies]`: `shell-words = "1"`.
Crate `Cargo.toml`, `[dependencies]`: `shell-words = { workspace = true }`.

- [ ] **Step 4: Write the implementation**

```rust
/// Cut the line at the cursor and split only the left half. Everything the user typed to
/// the right of the cursor is irrelevant to what they are completing, and ignoring it also
/// sidesteps the common case of a half-typed quote later in the line.
pub fn split_line(line: &str, point: usize) -> Split {
  // `point` arrives from the shell and may exceed the line (or land mid-UTF-8), so clamp
  // to a char boundary rather than slicing blindly and panicking.
  let point = clamp_to_boundary(line, point);
  let (left, _) = line.split_at(point);

  // A cursor directly after whitespace begins a new, empty word.
  let starting_new_word = left.ends_with(char::is_whitespace) || left.is_empty();

  // shell-words fails on an unterminated quote. Rather than give up, retry with a quote
  // appended: the user is mid-way through typing one, and the closed form is what they mean.
  let mut parsed = shell_words::split(left)
    .or_else(|_| shell_words::split(&format!("{left}\"")))
    .or_else(|_| shell_words::split(&format!("{left}'")))
    .unwrap_or_else(|_| left.split_whitespace().map(str::to_string).collect());

  if starting_new_word {
    // The prefix is empty and the new word's index is one past what was parsed.
    let cword = parsed.len();
    Split { words: parsed, cword, prefix: String::new() }
  } else {
    // The final parsed token is the partially typed word: it is the prefix, and it stays
    // in `words` so downstream positional counting sees the same shape either way.
    let prefix = parsed.last().cloned().unwrap_or_default();
    let cword = parsed.len().saturating_sub(1);
    if parsed.is_empty() { parsed.push(String::new()); }
    Split { words: parsed, cword, prefix }
  }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p aeth-devkit-complete --test words`
Expected: PASS, 6 tests

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(complete): split a bash command line and locate the cursor"
```

---

### Task 3: Command-line parsing — target dir, task, separator

**Files:**
- Create: `crates/aeth-devkit-complete/src/parse.rs`
- Test: `crates/aeth-devkit-complete/tests/parse.rs`

**Interfaces:**
- Produces:
  - `struct Parsed { target_dir: Option<String>, task: Option<String>, task_position: Option<usize>, after_separator: bool }`
  - `fn parse(words: &[String], cword: usize) -> Parsed`
  - `const GLOBAL_OPTIONS: [&str; 15]`, `const OPTIONS_WITH_VALUES: [&str; 8]`
  - `fn exclusions(opt: &str) -> &'static [&'static str]`

**Why this exists:** both current scripts contain this same scan, written twice. Values are lifted verbatim from `scripts.rs` so behaviour does not drift.

- [ ] **Step 1: Write the failing tests**

```rust
use aeth_devkit_complete::parse::parse;

fn w(s: &str) -> Vec<String> { s.split(' ').map(str::to_string).collect() }

#[test]
fn finds_the_task_as_the_first_non_option_word() {
  let p = parse(&w("poe build --release"), 2);
  assert_eq!(p.task.as_deref(), Some("build"));
  assert_eq!(p.task_position, Some(1));
}

#[test]
fn extracts_target_dir_from_separate_argument() {
  let p = parse(&w("poe -C ../other build"), 3);
  assert_eq!(p.target_dir.as_deref(), Some("../other"));
  assert_eq!(p.task.as_deref(), Some("build"));
  assert_eq!(p.task_position, Some(3));
}

#[test]
fn extracts_target_dir_from_inline_equals() {
  let p = parse(&w("poe --directory=../other build"), 2);
  assert_eq!(p.target_dir.as_deref(), Some("../other"));
  assert_eq!(p.task_position, Some(2));
}

#[test]
fn a_global_options_value_is_not_mistaken_for_the_task() {
  // `-e uv` — "uv" is the executor value, not a task name.
  let p = parse(&w("poe -e uv build"), 3);
  assert_eq!(p.task.as_deref(), Some("build"));
}

#[test]
fn an_inline_equals_option_does_not_swallow_the_next_word() {
  let p = parse(&w("poe -e=uv build"), 2);
  assert_eq!(p.task.as_deref(), Some("build"));
}

#[test]
fn detects_the_pass_through_separator_before_the_cursor() {
  let p = parse(&w("poe run -- somefile"), 3);
  assert!(p.after_separator);
}

#[test]
fn a_separator_after_the_cursor_does_not_count() {
  let p = parse(&w("poe run x -- y"), 2);
  assert!(!p.after_separator);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete --test parse`
Expected: FAIL — unresolved import

- [ ] **Step 3: Implement**

Port `Get-PoeTargetPath`, `Get-PoeCurrentTask` and the separator scan from `scripts.rs`
into one pass. Constants copied verbatim:

```rust
/// poe's global options, verbatim from the generated scripts. Order is the display order.
pub const GLOBAL_OPTIONS: [&str; 15] = [
  "-h", "--help", "--version", "-v", "--verbose", "-q", "--quiet", "-d", "--dry-run",
  "-C", "--directory", "-e", "--executor", "-X", "--executor-opt",
];

/// Options that consume the following word, so a scan must skip that word.
pub const OPTIONS_WITH_VALUES: [&str; 8] =
  ["-h", "--help", "-C", "--directory", "-e", "--executor", "-X", "--executor-opt"];

/// Spellings that suppress each other once one is present (`-v` hides `--quiet`, etc).
pub fn exclusions(opt: &str) -> &'static [&'static str] {
  match opt {
    "-h" | "--help" => &["--help", "-h"],
    "--version" => &["--version"],
    "-v" | "--verbose" => &["--quiet", "-q"],
    "-q" | "--quiet" => &["--verbose", "-v"],
    "-d" | "--dry-run" => &["--dry-run", "-d"],
    "-C" | "--directory" => &["--directory", "-C"],
    "-e" | "--executor" => &["--executor", "-e"],
    "--ansi" | "--no-ansi" => &["--ansi", "--no-ansi"],
    _ => &[],
  }
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p aeth-devkit-complete --test parse`
Expected: PASS, 7 tests

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(complete): parse poe global options, target dir and task"
```

---

### Task 4: Global-option and target-dir completion

**Files:**
- Modify: `crates/aeth-devkit-complete/src/engine.rs`
- Test: `crates/aeth-devkit-complete/tests/engine.rs`

**Interfaces:**
- Consumes: `parse::parse`, `parse::GLOBAL_OPTIONS`, `parse::exclusions`
- Produces: no new public names; `complete` gains these branches.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn completes_global_options_before_the_task() {
  let req = Request { words: w("poe -"), cword: 1, prefix: "-".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert!(items.iter().any(|i| i.value == "--verbose"));
  assert!(matches!(items[0].kind, ItemKind::Param));
}

#[test]
fn omits_globals_excluded_by_one_already_typed() {
  let req = Request { words: w("poe --verbose -"), cword: 2, prefix: "-".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert!(!items.iter().any(|i| i.value == "--quiet"));
  assert!(!items.iter().any(|i| i.value == "-q"));
}

#[test]
fn requests_directory_completion_after_dash_c() {
  let req = Request { words: w("poe -C"), cword: 2, prefix: "".into(), ..base(&p) };
  assert_eq!(complete(&req, &SystemRunner, true), Directive::Dirs);
}

#[test]
fn requests_directory_completion_for_inline_dash_c() {
  let req = Request { words: w("poe -C=sr"), cword: 1, prefix: "-C=sr".into(), ..base(&p) };
  assert_eq!(complete(&req, &SystemRunner, true), Directive::Dirs);
}

#[test]
fn completes_executor_choices() {
  let req = Request { words: w("poe -e"), cword: 2, prefix: "".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  let vals: Vec<&str> = items.iter().map(|i| i.value.as_str()).collect();
  assert_eq!(vals, ["auto", "poetry", "simple", "uv", "virtualenv"]);
}

#[test]
fn offers_file_completion_after_the_separator() {
  let req = Request { words: w("poe run -- "), cword: 3, prefix: "".into(), ..base(&p) };
  assert_eq!(complete(&req, &SystemRunner, true), Directive::Files);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete --test engine`
Expected: FAIL — the new tests return task names instead of options/sentinels

- [ ] **Step 3: Implement the branch order in `complete`**

Order matters and mirrors the scripts: separator check, then global-option value
completion, then global-option name completion, then task names.

```rust
const EXECUTORS: [&str; 5] = ["auto", "poetry", "simple", "uv", "virtualenv"];

// After `--`, everything is passed through to the task, so only paths make sense.
if parsed.after_separator { return Directive::Files; }

// `-C <cursor>` and `-C=<cursor>` both want directories, and the shell does that itself.
if wants_directory(&req.words, req.cword, &req.prefix) { return Directive::Dirs; }
```

`wants_directory` returns true when the previous word is `-C`/`--directory`/`--root`, or
when the prefix itself starts with one of those followed by `=`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p aeth-devkit-complete --test engine`
Expected: PASS, 8 tests

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(complete): global option, executor and directory completion"
```

---

### Task 5: Task-argument completion

**Files:**
- Modify: `crates/aeth-devkit-complete/src/engine.rs`
- Test: `crates/aeth-devkit-complete/tests/engine.rs`

**Interfaces:**
- Consumes: `resolve::TaskArg` (`name`, `options`, `kind`, `help`, `choices`)
- Produces: no new public names.

**Behaviour notes:** `kind` is poe's type string. `"positional"` marks a positional; `"boolean"` marks a flag that takes no value; anything else (`"string"`, `"integer"`, …) takes a value.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn completes_a_tasks_own_options_with_help_as_tooltip() {
  let p = fixture_task_with_args();          // task "build", option --mode (choices fast/slow)
  let req = Request { words: w("poe build -"), cword: 2, prefix: "-".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  let mode = items.iter().find(|i| i.value == "--mode").expect("--mode offered");
  assert_eq!(mode.tooltip, "Build profile");
  assert!(matches!(mode.kind, ItemKind::Param));
}

#[test]
fn hides_every_spelling_of_an_already_used_option() {
  // -m and --mode are the same argument; using one must hide both.
  let p = fixture_task_with_args();
  let req = Request { words: w("poe build -m fast -"), cword: 4, prefix: "-".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert!(!items.iter().any(|i| i.value == "--mode" || i.value == "-m"));
}

#[test]
fn completes_an_options_choices() {
  let p = fixture_task_with_args();
  let req = Request { words: w("poe build --mode"), cword: 3, prefix: "".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert_eq!(items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["fast", "slow"]);
  assert!(matches!(items[0].kind, ItemKind::Value));
}

#[test]
fn completes_inline_equals_choices() {
  let p = fixture_task_with_args();
  let req = Request { words: w("poe build --mode=f"), cword: 2, prefix: "--mode=f".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  // The inserted value carries the option prefix so the shell replaces the whole word.
  assert_eq!(items[0].value, "--mode=fast");
  assert_eq!(items[0].display, "fast");
}

#[test]
fn a_boolean_flag_does_not_consume_the_next_word() {
  // After a boolean, the next Tab offers options again, not that flag's "value".
  let p = fixture_task_with_args();          // also has boolean --force
  let req = Request { words: w("poe build --force -"), cword: 3, prefix: "-".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert!(items.iter().any(|i| i.value == "--mode"));
}

#[test]
fn completes_positional_choices_at_the_right_index() {
  let p = fixture_task_with_positionals();   // positionals: TARGET(a,b) then ENV(dev,prod)
  let req = Request { words: w("poe deploy a "), cword: 3, prefix: "".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert_eq!(items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["dev", "prod"]);
}

#[test]
fn positional_index_skips_options_and_their_values() {
  let p = fixture_task_with_positionals();
  let req = Request { words: w("poe deploy --mode fast a "), cword: 5, prefix: "".into(), ..base(&p) };
  let Directive::Items(items) = complete(&req, &SystemRunner, true) else { panic!() };
  assert_eq!(items.iter().map(|i| i.value.as_str()).collect::<Vec<_>>(), ["dev", "prod"]);
}

#[test]
fn falls_back_to_files_for_a_free_form_value() {
  let p = fixture_task_with_args();          // --out has no choices
  let req = Request { words: w("poe build --out"), cword: 3, prefix: "".into(), ..base(&p) };
  assert_eq!(complete(&req, &SystemRunner, true), Directive::Files);
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete --test engine`
Expected: FAIL on all 8 new tests

- [ ] **Step 3: Implement**

Port from the scripts, in this order: inline `--opt=value` choices, previous-word value
completion (boolean falls through to option list; choices win; otherwise `Files`), option
names filtered by used equivalence groups, then positional choices by computed index.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p aeth-devkit-complete --test engine`
Expected: PASS, 16 tests

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(complete): task option, choice and positional completion"
```

---

### Task 6: The `query` subcommand and response rendering

**Files:**
- Modify: `crates/aeth-devkit-complete/src/lib.rs` (new `Command::Query` variant, `output` arm)
- Create: `crates/aeth-devkit-complete/src/wire.rs`
- Test: `crates/aeth-devkit-complete/tests/wire.rs`, `tests/cli.rs`

**Interfaces:**
- Produces: `fn render(d: &Directive) -> String`, and the clap variant:

```rust
Query {
  #[arg(long, value_enum)] shell: Shell,
  #[arg(long)] shim_version: u32,
  #[arg(long)] line: Option<String>,
  #[arg(long)] point: Option<usize>,
  #[arg(long)] cword: Option<usize>,
  #[arg(long)] word_to_complete: Option<String>,
  #[arg(trailing_var_arg = true)] words: Vec<String>,
}
```

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn renders_items_with_a_header_and_four_columns() {
  let d = Directive::Items(vec![Item {
    value: "build".into(), display: "build".into(),
    tooltip: "Compile".into(), kind: ItemKind::Command,
  }]);
  assert_eq!(render(&d), "items\nbuild\tbuild\tCompile\tcommand\n");
}

#[test]
fn renders_the_sentinels_as_a_bare_header() {
  assert_eq!(render(&Directive::Dirs), "dirs\n");
  assert_eq!(render(&Directive::Files), "files\n");
}

#[test]
fn renders_an_empty_item_list_as_the_header_alone() {
  assert_eq!(render(&Directive::Items(vec![])), "items\n");
}

#[test]
fn a_tab_or_newline_in_help_cannot_break_the_columns() {
  let d = Directive::Items(vec![Item {
    value: "x".into(), display: "x".into(),
    tooltip: "two\tcols\nand a line".into(), kind: ItemKind::Value,
  }]);
  assert_eq!(render(&d), "items\nx\tx\ttwo cols and a line\tvalue\n");
}

// tests/cli.rs
#[test]
fn query_never_exits_non_zero_on_a_bad_project() {
  // A directory with no pyproject at all: empty output, exit 0, no panic.
  let out = run_devkit_complete(&["query", "--shell", "bash", "--shim-version", "1",
                                 "--line", "poe ", "--point", "4"], empty_dir());
  assert_eq!(out.status_code, 0);
  assert_eq!(out.stdout, "items\n");
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete`
Expected: FAIL — `render` not found, `Query` variant unknown

- [ ] **Step 3: Implement**

`render` writes the header then one line per item, replacing `\t` and `\n` in every field
with a space so a task's help text can never forge a column or a row. The `Query` arm
builds a `Request`: for `Shell::Bash` from `words::split_line(line, point)`; for
`Shell::PowerShell` from `words` plus `cword` plus `word_to_complete` as the prefix. The
project root is the parsed `target_dir` if present, else the process cwd.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p aeth-devkit-complete`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(complete): devkit complete query subcommand and wire format"
```

---

### Task 7: The shim scripts and their version stamp

**Files:**
- Modify: `crates/aeth-devkit-complete/src/scripts.rs` (replace both constants), `gen_scripts.py` (retire — the shims are hand-written now, so delete the generator and its "do not edit by hand" header)
- Test: `crates/aeth-devkit-complete/tests/shim.rs`

**Interfaces:**
- Produces: `pub const SHIM_VERSION: u32 = 1;`, `pub const BASH: &str`, `pub const POWERSHELL: &str`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn both_shims_send_the_version_the_binary_speaks() {
  let stamp = format!("--shim-version {SHIM_VERSION}");
  assert!(scripts::BASH.contains(&stamp), "bash shim must send the current version");
  assert!(scripts::POWERSHELL.contains(&stamp), "powershell shim must send it too");
}

#[test]
fn shim_text_is_pinned_to_its_version() {
  // The guard that makes decision 6 mechanical: edit either shim and this fails until
  // SHIM_VERSION is bumped, forcing a deliberate choice rather than silent drift.
  assert_eq!(fnv1a(scripts::BASH), 0x0000_0000, "bump SHIM_VERSION and update this hash");
  assert_eq!(fnv1a(scripts::POWERSHELL), 0x0000_0000, "bump SHIM_VERSION and update this hash");
}

#[test]
fn neither_shim_calls_devkit_at_load_time() {
  // The whole point: sourcing the shim must not shell out. Only the completer body may.
  for line in scripts::POWERSHELL.lines().filter(|l| !l.trim_start().starts_with('#')) {
    assert!(!line.contains("Invoke-Expression"), "shim must not evaluate fetched script text");
  }
}
```

Compute the two real hashes after writing the shims and substitute them for the zeros.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete --test shim`
Expected: FAIL — `SHIM_VERSION` not found

- [ ] **Step 3: Write the bash shim**

```bash
# Bash completion for poe - devkit thin shim (shim version 1)
# Installed by `devkit complete install --bash`; do not edit, it is rewritten on upgrade.
_poe_complete() {
    # No devkit on PATH (venv not activated, say): offer nothing rather than erroring.
    command -v devkit >/dev/null 2>&1 || return 0
    local out
    out=$(devkit complete query --shell bash --shim-version 1 \
            --line "$COMP_LINE" --point "$COMP_POINT" 2>/dev/null) || return 0
    local header="${out%%$'\n'*}"
    case "$header" in
        dirs)  _filedir -d 2>/dev/null || COMPREPLY=($(compgen -d -- "$2")); return 0 ;;
        files) _filedir    2>/dev/null || COMPREPLY=($(compgen -f -- "$2")); return 0 ;;
        items) ;;
        *) return 0 ;;
    esac
    COMPREPLY=()
    # Body is everything after the header line; devkit has already filtered by prefix,
    # so each first column is inserted verbatim.
    local rest="${out#*$'\n'}"
    [[ "$rest" == "$out" ]] && return 0
    local value
    while IFS=$'\t' read -r value _; do
        [[ -n "$value" ]] && COMPREPLY+=("$value")
    done <<< "$rest"
    return 0
}
complete -F _poe_complete poe
```

- [ ] **Step 4: Write the PowerShell shim**

```powershell
# PowerShell completion for poe - devkit thin shim (shim version 1)
# Installed by `devkit complete install --powershell`; rewritten on upgrade.
Register-ArgumentCompleter -CommandName poe -Native -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $dk = Get-Command devkit -ErrorAction SilentlyContinue
    if (-not $dk) { return }
    $els = @($commandAst.CommandElements)
    # Map the character offset onto an element index; past the last element means a fresh word.
    $cword = $els.Count
    for ($i = 0; $i -lt $els.Count; $i++) {
        $e = $els[$i].Extent
        if ($cursorPosition -ge $e.StartOffset -and $cursorPosition -le $e.EndOffset) { $cword = $i; break }
    }
    $texts = @($els | ForEach-Object { $_.Extent.Text })
    $out = & $dk.Source complete query --shell powershell --shim-version 1 --cword $cword --word-to-complete $wordToComplete -- @texts 2>$null
    if (-not $out) { return }
    $lines = @($out -split "`r?`n" | Where-Object { $_ -ne '' })
    if ($lines.Count -eq 0) { return }
    if ($lines[0] -eq 'dirs') {
        Get-ChildItem -Path "$wordToComplete*" -Directory -ErrorAction SilentlyContinue | ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_.FullName, $_.Name, 'ProviderContainer', $_.FullName) }
        return
    }
    if ($lines[0] -eq 'files') {
        Get-ChildItem -Path "$wordToComplete*" -ErrorAction SilentlyContinue | ForEach-Object {
            $t = if ($_.PSIsContainer) { 'ProviderContainer' } else { 'ProviderItem' }
            [System.Management.Automation.CompletionResult]::new($_.FullName, $_.Name, $t, $_.FullName) }
        return
    }
    if ($lines[0] -ne 'items') { return }
    foreach ($line in $lines[1..($lines.Count - 1)]) {
        $p = $line -split "`t"
        if ($p.Count -lt 4) { continue }
        $type = switch ($p[3]) { 'command' { 'Command' } 'param' { 'ParameterName' } default { 'ParameterValue' } }
        [System.Management.Automation.CompletionResult]::new($p[0], $p[1], $type, $p[2])
    }
}
```

- [ ] **Step 5: Run tests, fill in the real hashes, run again**

Run: `cargo test -p aeth-devkit-complete --test shim`
Expected: PASS, 3 tests

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "feat(complete): replace generated scripts with thin shims"
```

---

### Task 8: Installer rework and migration

**Files:**
- Modify: `crates/aeth-devkit-complete/src/install.rs`, `src/lib.rs` (`run_install`, `bash_targets`)
- Test: `crates/aeth-devkit-complete/tests/complete.rs`

**Interfaces:**
- Produces: `pub const POWERSHELL_LINE: &str` (new form), `pub fn powershell_shim_path(home: &Path) -> PathBuf`
- Removes: `pub fn preflight`, `pub fn devkit_on_path`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn the_profile_line_no_longer_invokes_devkit() {
  assert!(!install::POWERSHELL_LINE.contains("Invoke-Expression"));
  assert!(install::POWERSHELL_LINE.contains("poe-completion.ps1"));
}

#[test]
fn migrating_a_profile_replaces_the_old_devkit_line() {
  let before = "Set-Alias ll Get-ChildItem\n\
                devkit complete script --powershell | Out-String | Invoke-Expression\n";
  let (text, log) = install::patch_profile(Some(before));
  assert!(!text.contains("Invoke-Expression"));
  assert!(text.contains("poe-completion.ps1"));
  assert!(text.contains("Set-Alias ll Get-ChildItem"), "user's own lines survive");
  assert!(log.iter().any(|l| l.contains("removed")));
}

#[test]
fn poes_own_registration_is_still_removed() {
  let (text, _) = install::patch_profile(Some("poe _powershell_completion | iex\n"));
  assert!(!text.contains("_powershell_completion"));
}

#[test]
fn a_profile_already_migrated_is_left_byte_identical() {
  let already = format!("{}\n", install::POWERSHELL_LINE);
  let (text, log) = install::patch_profile(Some(&already));
  assert!(log.is_empty());
  assert_eq!(text, already);
}

#[test]
fn install_no_longer_refuses_when_devkit_is_absent_from_path() {
  // The original defect: preflight bailed here. Installing must now succeed.
  let log = install::install_powershell(&profile_path(), false).expect("install must not bail");
  assert!(!log.is_empty());
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete --test complete`
Expected: FAIL — old `POWERSHELL_LINE` still contains `Invoke-Expression`

- [ ] **Step 3: Implement**

```rust
/// The line `install --powershell` puts in `$PROFILE`. Deliberately content-free: it names
/// a path and nothing else, so a shim change never requires touching the user's profile.
pub const POWERSHELL_LINE: &str =
  "$c = \"$HOME/.local/share/devkit/poe-completion.ps1\"; if (Test-Path $c) { . $c }";

/// Fragment identifying the *previous* devkit registration, which this one replaces.
const OLD_DEVKIT_POWERSHELL_LINE: &str = "devkit complete script --powershell";
```

`install_powershell` gains a step writing the shim file (creating parent dirs) before
patching the profile. `preflight` and `devkit_on_path` are deleted along with their call
in `run_install` and any tests naming them.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p aeth-devkit-complete`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(complete): install a shim file, drop the global-install preflight"
```

---

### Task 9: Drift repair

**Files:**
- Create: `crates/aeth-devkit-complete/src/repair.rs`
- Modify: `crates/aeth-devkit-complete/src/lib.rs` (`Query` arm calls it)
- Test: `crates/aeth-devkit-complete/tests/repair.rs`

**Interfaces:**
- Produces: `fn repair_if_stale(home: &Path, shell: Shell, sent_version: u32) -> bool` — returns whether a write happened.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_matching_version_writes_nothing() {
  let home = tempdir();
  write_shim(&home, scripts::POWERSHELL);
  assert!(!repair_if_stale(home.path(), Shell::PowerShell, SHIM_VERSION));
}

#[test]
fn a_stale_version_with_outdated_text_rewrites_the_file() {
  let home = tempdir();
  write_shim(&home, "# ancient shim\n");
  assert!(repair_if_stale(home.path(), Shell::PowerShell, SHIM_VERSION - 1));
  assert_eq!(read_shim(&home), scripts::POWERSHELL);
}

#[test]
fn a_stale_version_whose_file_is_already_current_writes_nothing() {
  // An open shell holds the old shim in memory and reports the old version on EVERY
  // press; the repair must be idempotent or it would rewrite on every Tab.
  let home = tempdir();
  write_shim(&home, scripts::POWERSHELL);
  assert!(!repair_if_stale(home.path(), Shell::PowerShell, SHIM_VERSION - 1));
}

#[test]
fn repair_failure_is_swallowed() {
  // A read-only target must not turn a Tab press into an error.
  let home = read_only_tempdir();
  assert!(!repair_if_stale(home.path(), Shell::PowerShell, SHIM_VERSION - 1));
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p aeth-devkit-complete --test repair`
Expected: FAIL — module not found

- [ ] **Step 3: Implement**

Compare on-disk text with the current constant; if equal, return `false` without writing.
Otherwise write to a sibling temp file and `std::fs::rename` over the target, so a
concurrent reader sees either the old or the new file, never a partial one. Any error
returns `false`.

Wire it into the `Query` arm: compute the answer first, call `repair_if_stale`, then print.
The answer is returned regardless — decision 7.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p aeth-devkit-complete --test repair`
Expected: PASS, 4 tests

- [ ] **Step 5: Commit**

```bash
git add -A && git commit -m "feat(complete): repair a drifted shim atomically at query time"
```

---

### Task 10: Documentation

**Files:**
- Modify: `README.md`, `crates/aeth-devkit-complete/src/lib.rs` (module doc), `TODO.md`

- [ ] **Step 1: Update the crate module doc**

Restore a spec pointer, now that one exists again:
`//! See docs/superpowers/specs/2026-09-01-poe-completion-shim-design.md.`
Describe the shim architecture in place of the current "answers two questions" wording.

- [ ] **Step 2: Update README**

Replace any instruction to `uv tool install aeth-devkit` for completion. State that
completion uses whichever devkit is on PATH, which the activated venv provides, and that a
global install is only needed for unactivated shells.

- [ ] **Step 3: Verify the whole suite and lints**

Run: `cargo test --workspace` then `cargo clippy --workspace --all-targets`
Expected: all pass, zero warnings

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "docs: describe the completion shim architecture"
```

---

## Self-Review

**Spec coverage:** decisions 1–7 map to tasks 1/7 (engine + shim), 8 (PATH only, preflight
deleted), 4–5 (sentinel), 2 and 6 (per-shell input), 9 (repair), 7 (version stamp), 9
(best-effort answer). Wire protocol → task 6. Cursor semantics → tasks 2 and 6. Logic
inventory → tasks 3–5. Install and migration → task 8. Failure modes → tasks 6, 7, 9.
Testing → every task. Known loose end → task 10.

**Naming consistency:** `Shell`, `Item`, `ItemKind`, `Directive`, `Request`, `complete`,
`split_line`, `Split`, `parse`, `Parsed`, `render`, `SHIM_VERSION`, `repair_if_stale`,
`POWERSHELL_LINE` are used identically wherever they appear.

**Known gap accepted deliberately:** the shims are exercised only by string assertions,
not by driving a real bash or PowerShell session. End-to-end shell testing is out of scope
per the spec's non-goals; task 7's assertions cover the contract points that matter (the
version stamp travels, and nothing is evaluated at load time).
