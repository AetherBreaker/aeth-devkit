//! The VS Code side of a run: request files in the devkit cache, a `vscode://` URL that
//! opens the diff, and polling for the answer. Ctrl-C while waiting hands the question
//! back to the terminal (see `crate::interrupt` for every other moment).

use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::Ordering::SeqCst;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};

use aeth_devkit_core::process::Runner;

use super::VsCode;
use super::protocol::{EXTENSION_ID, PROTOCOL, Proposal, Request, Response, Reviewer};
use crate::docker::static_files::normalize_newlines;
use crate::interrupt::{INTERRUPTED, WAITING};

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
/// and reports `Dismissed`, which the caller answers with the terminal prompt. The ack
/// (written once the extension holds the texts) is due within `ack_timeout`; without it
/// nobody has the request, so the wait is an error rather than a hang. After it, the
/// wait is unbounded: the user may take as long as they like.
pub fn wait_for(response: &Path, cancel: &Path, ack: &Path, ack_timeout: Duration, poll: Duration) -> Result<Response> {
  INTERRUPTED.store(false, SeqCst);
  WAITING.store(true, SeqCst);
  let ack_due = std::time::Instant::now() + ack_timeout;
  let result = loop {
    if INTERRUPTED.swap(false, SeqCst) {
      let _ = std::fs::write(cancel, "");
      break Ok(Response::Dismissed);
    }
    if response.is_file() {
      let text = std::fs::read_to_string(response).with_context(|| format!("reading {}", response.display()))?;
      break serde_json::from_str(&text).context("parsing the VS Code response");
    }
    if !ack.is_file() && std::time::Instant::now() >= ack_due {
      break Err(anyhow::anyhow!(
        "VS Code did not pick up the request within {}s: the extension may be disabled or not loaded yet, or the editor sees a different cache directory than this shell",
        ack_timeout.as_secs_f32()
      ));
    }
    std::thread::sleep(poll);
  };
  WAITING.store(false, SeqCst);
  result
}

/// `--dry-run`: one request listing every file, opened as a multi-diff. Waits (at most
/// `ack_timeout`) for the extension's ack, written once it has read every text, because
/// the run folder is removed when the run ends and VS Code reads it after `code` returns.
pub fn open_review(
  vs: &VsCode,
  runner: &dyn Runner,
  root: &Path,
  previews: &[crate::changes::Preview],
  ack_timeout: Duration,
) -> Result<()> {
  let id = format!("review-{}", std::process::id());
  let dir = &vs.run_dir;
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
  open_url(runner, &vs.launcher, &format!("vscode://{EXTENSION_ID}/review?id={id}"))?;
  let ack = dir.join(format!("{id}.ack"));
  let deadline = std::time::Instant::now() + ack_timeout;
  while !ack.is_file() {
    if std::time::Instant::now() >= deadline {
      bail!("VS Code did not pick up the review within {}s", ack_timeout.as_secs_f32());
    }
    std::thread::sleep(Duration::from_millis(50));
  }
  Ok(())
}

pub struct VsCodeReviewer<'a> {
  vs: &'a VsCode,
  runner: &'a dyn Runner,
  poll: Duration,
  ack_timeout: Duration,
  next: Cell<u32>,
}

impl<'a> VsCodeReviewer<'a> {
  pub fn new(vs: &'a VsCode, runner: &'a dyn Runner) -> Self {
    Self {
      vs,
      runner,
      poll: Duration::from_millis(250),
      ack_timeout: Duration::from_secs(5),
      next: Cell::new(0),
    }
  }
}

impl Reviewer for VsCodeReviewer<'_> {
  fn review(&self, p: &Proposal, offer_replace_all: bool) -> Result<Response> {
    let n = self.next.get();
    self.next.set(n + 1);
    // `<pid>-<n>`: unique across concurrent runs, and the only thing the URL carries.
    let id = format!("{}-{n}", std::process::id());
    let file = |ext: &str| self.vs.run_dir.join(format!("{id}.{ext}"));
    std::fs::create_dir_all(&self.vs.run_dir)?;
    std::fs::write(file("current"), normalize_newlines(&p.current))?;
    std::fs::write(file("proposed"), normalize_newlines(&p.proposed))?;
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
    wait_for(&file("response.json"), &file("cancel"), &file("ack"), self.ack_timeout, self.poll)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::interrupt::tests::SERIAL;
  use aeth_devkit_core::process::RecordingRunner;

  fn vscode(dir: &Path) -> VsCode {
    VsCode {
      launcher: "code".into(),
      run_dir: dir.join("consent").join("1"),
      lock: None,
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
    let mut reviewer = VsCodeReviewer::new(&vs, &runner);
    reviewer.poll = Duration::from_millis(5);
    let dir = vs.run_dir.clone();
    let responder = std::thread::spawn(move || {
      let request = loop {
        if let Some(p) = std::fs::read_dir(&dir).ok().and_then(|d| {
          d.flatten()
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(".request.json"))
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
      std::fs::write(request.with_extension("").with_extension("ack"), "").unwrap();
      write_atomic(&req.response_path, r#"{"decision":"partial","accepted":[0]}"#).unwrap();
      req
    });
    let p = Proposal::new("docker/Dockerfile", "q", "a\nb\n", "a\nc\n");
    assert_eq!(reviewer.review(&p, true).unwrap(), Response::Partial { accepted: vec![0] });
    let req = responder.join().unwrap();
    let calls = runner.calls_for("code");
    assert_eq!(
      calls[0],
      vec!["--open-url", &format!("vscode://aeth.aeth-devkit/consent?id={}", req.id)]
    );
    assert!(req.id.starts_with(&format!("{}-0", std::process::id())));
    drop(vs);
    assert!(
      !tmp.path().join("consent").join("1").exists(),
      "dropping VsCode removes the run folder"
    );
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
    let ack = tmp.path().join("r.ack");
    assert_eq!(
      wait_for(&response, &cancel, &ack, Duration::from_secs(5), Duration::from_millis(5)).unwrap(),
      Response::Dismissed
    );
    assert!(cancel.is_file());
    assert!(!WAITING.load(SeqCst));
  }

  #[test]
  fn no_ack_in_time_is_an_error_but_an_acked_request_waits_indefinitely() {
    let _g = SERIAL.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let vs = vscode(tmp.path());
    let runner = RecordingRunner::new(0);
    let mut reviewer = VsCodeReviewer::new(&vs, &runner);
    reviewer.poll = Duration::from_millis(5);
    reviewer.ack_timeout = Duration::from_millis(30);
    let err = reviewer.review(&Proposal::new("t", "q", "a\n", "b\n"), true).unwrap_err();
    assert!(err.to_string().contains("did not pick up"), "{err:#}");
    // Acked: the answer may come long after the ack deadline.
    let response = tmp.path().join("r.json");
    let ack = tmp.path().join("r.ack");
    std::fs::write(&ack, "").unwrap();
    let late = response.clone();
    std::thread::spawn(move || {
      std::thread::sleep(Duration::from_millis(80));
      write_atomic(&late, r#"{"decision":"keep"}"#).unwrap();
    });
    let got = wait_for(
      &response,
      &tmp.path().join("r.cancel"),
      &ack,
      Duration::from_millis(30),
      Duration::from_millis(5),
    );
    assert_eq!(got.unwrap(), Response::Keep);
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
    let id = format!("review-{}", std::process::id());
    // No ack: the review is reported as not picked up, and the files stay for the drop.
    let err = open_review(&vs, &runner, &root, &previews, Duration::from_millis(20)).unwrap_err();
    assert!(err.to_string().contains("did not pick up"), "{err:#}");
    let request = vs.run_dir.join(format!("{id}.request.json"));
    assert!(request.is_file());
    let ack = vs.run_dir.join(format!("{id}.ack"));
    let acker = std::thread::spawn(move || {
      std::thread::sleep(Duration::from_millis(30));
      std::fs::write(ack, "").unwrap();
    });
    open_review(&vs, &runner, &root, &previews, Duration::from_secs(5)).unwrap();
    acker.join().unwrap();
    let req: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&request).unwrap()).unwrap();
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
    let bad = wait_for(
      &response,
      &tmp.path().join("bad.cancel"),
      &tmp.path().join("bad.ack"),
      Duration::from_secs(5),
      Duration::from_millis(1),
    );
    assert!(bad.unwrap_err().to_string().contains("parsing"));
  }
}
