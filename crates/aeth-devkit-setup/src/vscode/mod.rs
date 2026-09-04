//! VS Code integration for `setup-project`: an in-editor diff with per-hunk consent in
//! place of the typed terminal prompt, when the run happens inside a VS Code terminal.
//! `prepare` runs the detection/install/grant pipeline; the pure pieces live here.

pub mod install;
pub mod protocol;
pub mod session;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};

use protocol::EXTENSION_ID;

/// The `argv.json` key that grants proposed API contributions to a listed extension.
pub const ARGV_KEY: &str = "enable-proposed-api";

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

#[cfg(test)]
mod tests {
  use super::*;
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

  /// The launcher `options` planted, spelled as `ensure` passes it to the runner.
  fn code(dir: &Path) -> String {
    dir
      .join("bin")
      .join(if cfg!(windows) { "code.cmd" } else { "code" })
      .to_string_lossy()
      .into_owned()
  }

  fn installed_runner(dir: &Path) -> RecordingRunner {
    let r = RecordingRunner::new(0);
    r.script(&code(dir), &["--list-extensions"], 0, "aeth.aeth-devkit@1.0.0\n");
    r
  }

  #[test]
  fn prepare_is_inert_outside_vscode_unless_forced() {
    let tmp = tempfile::tempdir().unwrap();
    let mut o = options(tmp.path(), true);
    o.term_program = None;
    assert!(matches!(prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()), Prepared::Inert));
    o.force = true;
    assert!(matches!(prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()), Prepared::Ready(_)));
    o.path = Some(std::env::join_paths([tmp.path()]).unwrap());
    assert!(matches!(prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()), Prepared::Unavailable(m) if m.contains("PATH")));
  }

  #[test]
  fn prepare_grants_the_proposal_once_and_reports_content_menu_after() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path(), true);
    let Prepared::Ready(vs) = prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()) else {
      panic!()
    };
    assert!(!vs.content_menu);
    assert!(vs.notes.iter().any(|n| n.contains("restart VS Code once")), "{:?}", vs.notes);
    assert!(vs.consent_dir.is_dir() && vs.consent_dir.starts_with(tmp.path().join("cache")));
    let argv = std::fs::read_to_string(tmp.path().join("home/.vscode/argv.json")).unwrap();
    assert!(argv.contains("\"enable-proposed-api\": [\"aeth.aeth-devkit\"]"), "{argv}");
    let dir = vs.consent_dir.clone();
    drop(vs);
    assert!(!dir.exists());

    let Prepared::Ready(vs) = prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()) else {
      panic!()
    };
    assert!(vs.content_menu);
    assert!(!vs.notes.iter().any(|n| n.contains("restart")), "{:?}", vs.notes);
  }

  #[test]
  fn dry_run_reads_argv_but_never_writes_it_and_never_installs() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path(), false);
    let Prepared::Ready(vs) = prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()) else {
      panic!()
    };
    assert!(!vs.content_menu);
    assert!(!tmp.path().join("home/.vscode/argv.json").exists());
    let absent = RecordingRunner::new(0);
    absent.script(&code(tmp.path()), &["--list-extensions"], 0, "");
    assert!(matches!(prepare(&o, &absent, &StubFetch::default()), Prepared::Unavailable(_)));
    assert_eq!(absent.calls_for(&code(tmp.path())).len(), 1);
  }

  #[test]
  fn prepare_stops_on_reload_needed_and_carries_stray_notes() {
    let tmp = tempfile::tempdir().unwrap();
    let o = options(tmp.path(), true);
    std::fs::create_dir_all(tmp.path().join("proj/.vscode/extension")).unwrap();
    let Prepared::Ready(vs) = prepare(&o, &installed_runner(tmp.path()), &StubFetch::default()) else {
      panic!()
    };
    assert!(vs.notes.iter().any(|n| n.starts_with(".vscode/extension/")), "{:?}", vs.notes);
    let old = RecordingRunner::new(0);
    old.script(&code(tmp.path()), &["--list-extensions"], 0, "aeth.aeth-devkit@0.0.0\n");
    let mut f = StubFetch::default();
    f.bodies.insert(install::refs_url(), r#"[{"ref":"refs/tags/vscode-extension-v1"}]"#.into());
    assert!(matches!(prepare(&o, &old, &f), Prepared::ReloadNeeded));
  }

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
