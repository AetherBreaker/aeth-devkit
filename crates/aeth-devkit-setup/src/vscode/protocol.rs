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
