//! `StdinPrompt` and `stdin_present` against a real standard input. Each case re-runs
//! this test binary as a child with the stdin shape under test; `DEVKIT_PROMPT_CHILD`
//! picks the child's job, and its exit code is the result.

use std::io::Write as _;
use std::process::{Command, Stdio};

use aeth_devkit_core::prompt::{Prompt as _, StdinPrompt, stdin_present};

const CHILD: &str = "DEVKIT_PROMPT_CHILD";

/// The child half: runs before the harness' own logic when the variable is set, so the
/// parent's assertions never execute in the child.
fn child_if_asked() {
  match std::env::var(CHILD).as_deref() {
    Ok("present") => std::process::exit(if stdin_present() { 0 } else { 3 }),
    Ok("ask") => match StdinPrompt.ask("q?") {
      Ok(answer) => {
        println!("answer=[{answer}]");
        std::process::exit(0)
      }
      Err(e) => {
        eprintln!("{e:#}");
        std::process::exit(4)
      }
    },
    _ => {}
  }
}

fn child(job: &str, test: &str, stdin: Stdio) -> std::process::Child {
  Command::new(std::env::current_exe().unwrap())
    .args(["--exact", test, "--nocapture"])
    .env(CHILD, job)
    .stdin(stdin)
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap()
}

#[test]
fn stdin_is_present_for_a_pipe_and_a_file_but_not_the_null_device() {
  child_if_asked();
  let me = "stdin_is_present_for_a_pipe_and_a_file_but_not_the_null_device";
  assert_eq!(child("present", me, Stdio::null()).wait().unwrap().code(), Some(3), "null device");
  let mut piped = child("present", me, Stdio::piped());
  drop(piped.stdin.take());
  assert_eq!(piped.wait().unwrap().code(), Some(0), "a pipe, even one already at EOF");
  let file = tempfile::NamedTempFile::new().unwrap();
  let handle = std::fs::File::open(file.path()).unwrap();
  assert_eq!(child("present", me, Stdio::from(handle)).wait().unwrap().code(), Some(0), "a file");
}

#[test]
fn a_piped_answer_is_read_and_an_ended_input_is_an_error() {
  child_if_asked();
  let me = "a_piped_answer_is_read_and_an_ended_input_is_an_error";
  let mut answered = child("ask", me, Stdio::piped());
  answered.stdin.take().unwrap().write_all(b"  force \n").unwrap();
  let out = answered.wait_with_output().unwrap();
  let stdout = String::from_utf8_lossy(&out.stdout);
  assert_eq!(out.status.code(), Some(0), "{stdout}");
  assert!(stdout.contains("answer=[force]"), "trimmed: {stdout}");
  let mut ended = child("ask", me, Stdio::piped());
  drop(ended.stdin.take());
  let out = ended.wait_with_output().unwrap();
  let stderr = String::from_utf8_lossy(&out.stderr);
  assert_eq!(out.status.code(), Some(4), "{stderr}");
  assert!(stderr.contains("standard input ended before \"q?\""), "{stderr}");
}
