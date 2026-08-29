use aeth_devkit_complete::{Command, output, scripts};

#[test]
fn scripts_register_for_poe_and_call_devkit_for_data() {
  for (script, shell) in [(scripts::POWERSHELL, "powershell"), (scripts::BASH, "bash")] {
    assert!(script.contains("devkit complete tasks"), "{shell}");
    assert!(script.contains("devkit complete args"), "{shell}");
    assert!(
      !script.contains("poe _list_tasks") && !script.contains("poe _describe_task_args"),
      "{shell}"
    );
  }
  assert!(scripts::POWERSHELL.contains("Register-ArgumentCompleter -CommandName poe"));
  assert!(scripts::BASH.contains("complete -F _poe_complete poe"));
}

#[test]
fn script_command_selects_the_shell() {
  assert_eq!(
    output(
      &Command::Script {
        powershell: true,
        bash: false
      },
      false
    ),
    scripts::POWERSHELL
  );
  assert_eq!(
    output(
      &Command::Script {
        powershell: false,
        bash: true
      },
      false
    ),
    scripts::BASH
  );
  // PowerShell is the default on this fleet.
  assert_eq!(
    output(
      &Command::Script {
        powershell: false,
        bash: false
      },
      false
    ),
    scripts::POWERSHELL
  );
}

#[test]
fn tasks_and_args_print_nothing_for_a_directory_without_a_pyproject() {
  let dir = tempfile::tempdir().unwrap();
  assert_eq!(
    output(
      &Command::Tasks {
        dir: Some(dir.path().to_path_buf())
      },
      false
    ),
    ""
  );
  assert_eq!(
    output(
      &Command::Args {
        task: "x".into(),
        dir: Some(dir.path().to_path_buf())
      },
      false
    ),
    ""
  );
}

#[test]
fn tasks_and_args_for_a_real_project_directory() {
  let dir = tempfile::tempdir().unwrap();
  std::fs::write(
    dir.path().join("pyproject.toml"),
    "[tool.poe.tasks]\nlint = \"ruff\"\n[tool.poe.tasks.t]\ncmd = \"x\"\nargs = [{ name = \"n\", type = \"integer\", help = \"count\" }]\n",
  )
  .unwrap();
  let d = Some(dir.path().to_path_buf());
  assert_eq!(output(&Command::Tasks { dir: d.clone() }, true), "lint t\n");
  assert_eq!(
    output(
      &Command::Args {
        task: "t".into(),
        dir: d.clone()
      },
      true
    ),
    "--n\tinteger\tcount\t_\n"
  );
  assert_eq!(
    output(
      &Command::Args {
        task: "nope".into(),
        dir: d
      },
      true
    ),
    ""
  );
}
