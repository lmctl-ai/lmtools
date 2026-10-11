//! The published lmapi operations and their handlers.

use crate::error::{Result, ToolError};
use crate::ops::{self, Params};
use lmrust::server::{CliError, CliSpec, OpDef, Policy};
use lmrust::{Access, InputForm, OperationSchema, ParamSpec, ParamType, Record, ReturnType};
use serde_json::{json, Value};

pub const TOOL: &str = "lmtools";

fn path_param(what: &str) -> ParamSpec {
  ParamSpec::new("path", ParamType::String)
    .positional()
    .describe(what)
}

fn opt(name: &str, kind: ParamType, what: &str) -> ParamSpec {
  ParamSpec::new(name, kind).optional().describe(what)
}

fn schema(
  name: &str,
  access: Access,
  description: &str,
  params: Vec<ParamSpec>,
) -> OperationSchema {
  let mut s = OperationSchema::new(name)
    .access(access)
    .describe(description);
  s.returns = Some(ReturnType::Object);
  for p in params {
    s = s.param(p);
  }
  s
}

/// The operation schemas, in published order.
pub fn schemas() -> Vec<OperationSchema> {
  use ops::{list, read};
  vec![
    schema(
      "list_files",
      Access::Read,
      "List a directory (sorted; recursive is depth-first). Entries: name, path, type (file|dir|symlink|other), size, mtime. Page with offset/limit until next_offset is null. Symlinked dirs are listed, not followed. Example: lmtools list_files src --recursive --glob '**/*.rs'",
      vec![
        ParamSpec::new("path", ParamType::String)
          .positional()
          .optional()
          .with_default(json!("."))
          .describe("Directory to list"),
        opt("recursive", ParamType::Boolean, "Descend into subdirectories"),
        opt(
          "maxDepth",
          ParamType::Number,
          "With recursive: levels to descend (1 = only the directory itself)",
        ),
        opt(
          "glob",
          ParamType::String,
          "Keep entries matching this glob (* ? [..] **): the name, or the relative path if the glob has a /",
        ),
        opt(
          "includeHidden",
          ParamType::Boolean,
          "Include dot files and dot directories",
        ),
        opt(
          "offset",
          ParamType::Number,
          "Skip this many entries (from next_offset)",
        ),
        opt(
          "limit",
          ParamType::Number,
          &format!(
            "Entries per page (default {}, at most {})",
            list::DEFAULT_LIMIT,
            list::MAX_LIMIT
          ),
        ),
      ],
    ),
    schema(
      "read_file",
      Access::Read,
      "Read a page of a text file, by bytes (offset/maxBytes, default 64 KiB) or by lines (startLine/endLine). Returns content, eof, next_offset / next_start_line to continue, total_size, total_lines (files up to 8 MiB). Binary files (NUL in the first 8 KiB) need asBase64. Example: lmtools read_file src/main.rs --start-line 40 --end-line 80 --line-numbers",
      vec![
        path_param("File to read"),
        opt(
          "offset",
          ParamType::Number,
          "Byte offset to start at (from next_offset)",
        ),
        opt(
          "maxBytes",
          ParamType::Number,
          &format!(
            "Page size in bytes (default {}, at most {}); also caps a line page",
            read::DEFAULT_MAX_BYTES,
            read::MAX_BYTES
          ),
        ),
        opt(
          "startLine",
          ParamType::Number,
          "Line mode: first line (1-based)",
        ),
        opt(
          "endLine",
          ParamType::Number,
          "Line mode: last line (inclusive)",
        ),
        opt(
          "lineNumbers",
          ParamType::Boolean,
          "Prefix each line with its number (cat -n style)",
        ),
        opt(
          "asBase64",
          ParamType::Boolean,
          "Return the bytes as content_base64 (for binary files)",
        ),
      ],
    ),
  ]
}

/// Run an op body and map its error into the lmapi envelope.
fn handle(f: impl FnOnce() -> Result<Value>) -> std::result::Result<Value, CliError> {
  f().map_err(ToolError::into_cli_error)
}

type Handler = fn(&Record, &Policy) -> std::result::Result<Value, CliError>;

pub fn spec() -> CliSpec<Policy> {
  let ops = schemas();
  let def = |name: &str, access: Access, f: Handler| {
    let schema = ops
      .iter()
      .find(|s| s.name == name)
      .cloned()
      .expect("op schema");
    OpDef::new(
      name,
      access,
      schema.description.clone().unwrap_or_default(),
      f,
    )
    .schema(schema)
  };
  CliSpec::with_policy(TOOL)
    .version(env!("CARGO_PKG_VERSION"))
    .description("File and web tools for LLM agents: list, read, write, edit, webget")
    .inputs([InputForm::Flags, InputForm::Json])
    .op(def("list_files", Access::Read, |params, _| {
      handle(|| ops::list::run(&Params(params)))
    }))
    .op(def("read_file", Access::Read, |params, _| {
      handle(|| ops::read::run(&Params(params)))
    }))
}

/// The published document (`lmtools --schema`), byte-identical to the CLI output.
pub fn schema_text() -> String {
  let mut out = Vec::new();
  let code = lmrust::server::run_cli_io(
    &spec(),
    &["--schema".to_string()],
    &mut out,
    &Default::default(),
  );
  assert_eq!(code, 0);
  String::from_utf8(out).expect("utf-8 schema")
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn the_document_passes_lmrust_publish_check() {
    let doc: Value = serde_json::from_str(&schema_text()).unwrap();
    lmrust::parse_published_schema(&doc).unwrap_or_else(|issues| panic!("{issues:?}"));
    assert_eq!(doc["tool"], TOOL);
    assert_eq!(doc["inputs"], json!(["flags", "json"]));
    for op in doc["operations"].as_array().unwrap() {
      assert!(
        op["description"]
          .as_str()
          .is_some_and(|d| d.contains("Example: lmtools")),
        "{}",
        op["name"]
      );
      for p in op["params"].as_array().unwrap() {
        assert!(
          p["description"].as_str().is_some_and(|d| !d.is_empty()),
          "{}",
          p["name"]
        );
      }
    }
  }
}
