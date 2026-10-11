//! lmtools: the file and web tool belt for LLM agents. A native lmapi tool (`lmtools --schema`)
//! built on lmrust: list_files, read_file, write_file, edit_file, webget.

pub mod atomic;
pub mod diff;
pub mod error;
pub mod html;
pub mod legacy;
pub mod ops;
pub mod output;
pub mod permissions;
pub mod settings;
pub mod spec;
pub mod text_edit;
pub mod util;

/// Run with the arguments after the binary name; returns the exit code.
///
/// lmapi-shaped argv (an op such as `read_file`, or a root lmapi global such as `--schema`) goes
/// to the lmapi runner, which is also the human CLI. Everything else (no arguments, `--help`,
/// `--version`, unknown words) is the small legacy surface in [`legacy`].
pub fn run(args: Vec<String>) -> i32 {
  let spec = spec::spec();
  if let Some(code) = lmrust::server::maybe_run_cli(&spec, args.clone()) {
    return code;
  }
  legacy::run(&args, &mut std::io::stdout(), &mut std::io::stderr())
}
