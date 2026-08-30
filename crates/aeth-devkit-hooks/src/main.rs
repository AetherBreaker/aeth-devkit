use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
  // `Args::parse()` exits 2 on an unrecognized hook name, and Claude renders *any* non-zero
  // exit as a hook error in every session. That is reachable without anyone making a typo: a
  // `.claude/settings.json` written by a newer devkit, run against an older binary still on
  // PATH, names a hook the old clap enum has never heard of. So argv is parsed leniently and
  // a failure degrades to silence, like every other failure path in this crate.
  //
  // `--help` and `--version` also arrive here as `Err`; `print` emits them on stdout (and a
  // real error on stderr), which keeps the binary usable by hand without ever exiting non-zero.
  match aeth_devkit_hooks::Args::try_parse() {
    Ok(args) => aeth_devkit_hooks::run_real(&args),
    Err(e) => {
      let _ = e.print();
      ExitCode::SUCCESS
    }
  }
}
