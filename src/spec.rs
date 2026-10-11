//! The published lmapi operations and their handlers.

use crate::error::{Result, ToolError};
use crate::ops::{self, Params};
use crate::permissions::Gate;
use crate::settings;
use lmrust::server::ErrorCode;
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
  use ops::{list, read, webget};
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
        bypass_param(),
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
        bypass_param(),
      ],
    ),
    schema(
      "write_file",
      Access::Write,
      "Write one file atomically (temp file + rename; an existing file keeps its permissions). Content from content, contentBase64 or stdin (at most 16 MiB). mode: overwrite (default), create (fails if it exists) or append. Refuses symlinks, non-regular and hard-linked files. --dry-run reports what would change (with a diff) and writes nothing. Example: lmtools write_file notes/todo.md --content '- ship it' --mode create --create-parents",
      vec![
        path_param("File to write"),
        opt("content", ParamType::String, "The text to write"),
        opt("contentBase64", ParamType::String, "The bytes to write, base64-encoded (binary content)"),
        opt("stdin", ParamType::Boolean, "Read the content from standard input"),
        ParamSpec::new("mode", ParamType::String)
          .optional()
          .with_enum([json!("overwrite"), json!("create"), json!("append")])
          .describe("overwrite (default), create (fail if the file exists) or append"),
        opt("createParents", ParamType::Boolean, "Create missing parent directories"),
        bypass_param(),
      ],
    ),
    schema(
      "edit_file",
      Access::Write,
      "Edit one existing UTF-8 file in place, atomically; returns a unified diff and counts. Choose ONE edit: pattern + replacement (literal; regex for Rust regex with $1 / ${name}; word for whole words; first for the first match only; ignoreCase), insertBefore / insertAfter N + text, deleteLines START:END, replaceLines START:END + text, prepend / append + text, or script (a JSON array of ops run in order). Lines are 1-based and inclusive; CRLF files stay CRLF. expectedCount fails with EDIT_COUNT_MISMATCH (writing nothing) unless exactly that many replacements happen. --dry-run shows the diff without writing. Example: lmtools edit_file src/lib.rs --pattern old_name --replacement new_name --word --expected-count 3",
      vec![
        path_param("File to edit"),
        opt("pattern", ParamType::String, "Text to find (literal unless regex)"),
        opt("replacement", ParamType::String, "Replacement for pattern (with regex: $1, ${name}, $$ for $)"),
        opt("regex", ParamType::Boolean, "pattern is a Rust regex (no lookaround/backreferences; (?m) (?s) flags work)"),
        opt("word", ParamType::Boolean, "Match whole (Unicode) words only"),
        opt("first", ParamType::Boolean, "Replace only the first match"),
        opt("ignoreCase", ParamType::Boolean, "Case-insensitive pattern"),
        opt("expectedCount", ParamType::Number, "Fail unless exactly this many replacements are made"),
        opt("insertBefore", ParamType::Number, "Insert text before this line"),
        opt("insertAfter", ParamType::Number, "Insert text after this line"),
        opt("deleteLines", ParamType::String, "Delete lines START:END (or one line N)"),
        opt("replaceLines", ParamType::String, "Replace lines START:END (or N) with text"),
        opt("prepend", ParamType::Boolean, "Add text at the very beginning, exactly as given"),
        opt("append", ParamType::Boolean, "Add text at the very end, exactly as given"),
        opt("text", ParamType::String, "Text for insertBefore/insertAfter/replaceLines/prepend/append"),
        opt("script", ParamType::String, "A JSON array of operations run in order, or - to read it from stdin: [{\"op\":\"replace\",\"pattern\":\"a\",\"replacement\":\"b\",\"word\":true},{\"op\":\"insert_after\",\"line\":2,\"text\":\"x\"},{\"op\":\"delete_lines\",\"start\":4,\"end\":5},{\"op\":\"replace_lines\",\"start\":1,\"end\":1,\"text\":\"y\"},{\"op\":\"prepend\",\"text\":\"p\"},{\"op\":\"append\",\"text\":\"q\"}]"),
        bypass_param(),
      ],
    ),
    schema(
      "webget",
      Access::Read,
      "HTTP(S) GET one URL. Returns status (non-2xx is a normal result, ok: false), final_url (after redirects), content_type, a few response headers, and body: text, or {\"$blob_base64\": ...} for binary. A body over maxBytes (default 64 KiB) is written in full to a spill file (spilled: true, file.path, preview); --out writes the full body to that file instead. format text turns HTML into readable plain text (tags, scripts and styles dropped). Request header values are never echoed. Example: lmtools webget https://example.com/docs --format text",
      vec![
        ParamSpec::new("url", ParamType::String)
          .positional()
          .describe("http:// or https:// URL"),
        opt("format", ParamType::String, "raw (default) or text (HTML to plain text)"),
        opt("out", ParamType::String, "Write the full body to this file (atomic; parent dirs created) and return a preview"),
        opt("maxBytes", ParamType::Number, &format!("Largest body returned inline (default {}, at most {}); bigger bodies spill to a file", webget::DEFAULT_MAX_BYTES, webget::MAX_INLINE_BYTES)),
        opt("offset", ParamType::Number, "Skip this many body bytes first"),
        opt("timeoutMs", ParamType::Number, &format!("Whole-call timeout including redirects and the body (default {}, at most {})", webget::DEFAULT_TIMEOUT_MS, webget::MAX_TIMEOUT_MS)),
        ParamSpec::new("followRedirects", ParamType::Boolean)
          .optional()
          .with_default(json!(true))
          .describe(format!("Follow redirects (at most {})", webget::MAX_REDIRECTS)),
        opt("headers", ParamType::Object, "Extra request headers, e.g. {\"Accept\": \"application/json\"}"),
        opt("userAgent", ParamType::String, "User-Agent (default lmtools/<version>)"),
        bypass_param(),
      ],
    ),
  ]
}

/// `--dangerously-bypass-permission-settings`: on every operation.
pub const BYPASS_PARAM: &str = "dangerouslyBypassPermissionSettings";

fn bypass_param() -> ParamSpec {
  opt(
    BYPASS_PARAM,
    ParamType::Boolean,
    "Skip the settings file's permission rules for this call (paths, hosts, output.dir); works only if the settings file sets allow_bypass: true. Limits still apply",
  )
}

/// The call context: lmrust's policy globals and the settings file's gate (with the bypass flag).
fn ctx(params: &Record, policy: &Policy) -> Result<ops::Ctx> {
  let env = |k: &str| std::env::var(k).ok();
  let settings = settings::load(&env, &mut |w| eprintln!("{w}"))?;
  let bypass = params.get(BYPASS_PARAM) == Some(&Value::Bool(true));
  let gate = match settings {
    None => Gate::open(),
    Some(s) if bypass => {
      if !s.allow_bypass {
        return Err(ToolError::new(
          ErrorCode::Forbidden,
          "BYPASS_NOT_ALLOWED",
          format!(
            "--dangerously-bypass-permission-settings is not allowed: the operator must set \"allow_bypass\": true in {}",
            s.source.display()
          ),
        ));
      }
      eprintln!(
        "lmtools: permission settings bypassed for this call (allow_bypass is true in {})",
        s.source.display()
      );
      Gate::open()
    }
    Some(s) => Gate::new(s)?,
  };
  Ok(ops::Ctx {
    dry_run: policy.dry_run,
    read_only: policy.read_only,
    gate,
  })
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
    .op(def("list_files", Access::Read, |params, policy| {
      handle(|| ops::list::run(&Params(params), &ctx(params, policy)?))
    }))
    .op(def("read_file", Access::Read, |params, policy| {
      handle(|| ops::read::run(&Params(params), &ctx(params, policy)?))
    }))
    .op(def("write_file", Access::Write, |params, policy| {
      handle(|| ops::write::run(&Params(params), &ctx(params, policy)?))
    }))
    .op(def("edit_file", Access::Write, |params, policy| {
      handle(|| ops::edit::run(&Params(params), &ctx(params, policy)?))
    }))
    .op(def("webget", Access::Read, |params, policy| {
      handle(|| ops::webget::run(&Params(params), &ctx(params, policy)?))
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
