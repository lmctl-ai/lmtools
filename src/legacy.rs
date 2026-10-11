//! The non-lmapi surface: help, version and a clear error for anything else.

use std::io::Write;

pub const HELP: &str = "lmtools: file and web tools for LLM agents; one JSON document out per call

Ops (lmtools <op> --help for one, lmtools --list for all as JSON):
  list_files  lmtools list_files src --recursive --glob '**/*.rs'
  read_file   lmtools read_file src/main.rs --start-line 1 --end-line 80 --line-numbers
  write_file  lmtools write_file notes.txt --content 'hello' --mode create
  edit_file   lmtools edit_file src/lib.rs --pattern old_name --replacement new_name --expected-count 2
              lmtools edit_file notes.txt --insert-after 3 --text 'new line'
  webget      lmtools webget https://example.com --format text

Every result and every error is one JSON document on stdout; an error has error.data.tool_code.
Paths are relative to the working directory. Big results page (read_file / list_files:
next_offset) or go to a file (webget: --out; otherwise spilled: true and file.path).
--dry-run shows what write_file / edit_file would change without writing.
A settings file (~/.config/lmtools/settings.json) may restrict paths and hosts; a denial has
error.data.tool_code PERMISSION_DENIED and error.data.manual_command for the operator.
";

/// Handle a non-lmapi invocation.
pub fn run(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> i32 {
  match args.first().map(String::as_str) {
    None | Some("--help") | Some("-h") | Some("help") => {
      let _ = out.write_all(HELP.as_bytes());
      0
    }
    Some("--version") | Some("-V") => {
      let _ = writeln!(out, "lmtools {}", env!("CARGO_PKG_VERSION"));
      0
    }
    Some(other) => {
      let _ = writeln!(
        err,
        "lmtools: unknown command {other:?}; ops are list_files, read_file, write_file, edit_file, webget (lmtools --help)"
      );
      2
    }
  }
}
