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
  fn open_review_writes_one_request_listing_every_file() {
    let _g = SERIAL.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let vs = vscode(tmp.path());
    let runner = RecordingRunner::new(0);
    let root = tmp.path().join("proj");
    let previews = vec![
      crate::changes::Preview {
        path: root.join("docker").join("Dockerfile"),
        current: Some("a\n".into()),
        proposed: "b\n".into(),
      },
      crate::changes::Preview {
        path: root.join("new.txt"),
        current: None,
        proposed: "n\n".into(),
      },
    ];
    open_review(&vs, &runner, &root, &previews).unwrap();
    let id = format!("review-{}", std::process::id());
    let req: serde_json::Value =
      serde_json::from_str(&std::fs::read_to_string(vs.consent_dir.join(format!("{id}.request.json"))).unwrap()).unwrap();
    assert_eq!(req["protocol"], PROTOCOL);
    assert_eq!(req["files"][0]["label"], "docker/Dockerfile");
    assert_eq!(req["files"][1]["current_path"], serde_json::Value::Null);
    assert_eq!(
      std::fs::read_to_string(req["files"][1]["proposed_path"].as_str().unwrap()).unwrap(),
      "n\n"
    );
    assert_eq!(runner.calls_for("code")[0][1], format!("vscode://aeth.aeth-devkit/review?id={id}"));
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
