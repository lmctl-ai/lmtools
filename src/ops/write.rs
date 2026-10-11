//! `write_file`: create, replace or append to one file, atomically (temp file + rename), keeping
//! an existing file's permission bits.

use super::{read_stdin, too_large, Ctx, Params};
use crate::atomic::{self, Existing, MAX_FILE_BYTES};
use crate::diff::unified;
use crate::error::{Result, ToolError};
use crate::permissions::Need;
use crate::util::shell_quote;
use crate::util::{absolute, base64_decode};
use serde_json::{json, Value};

/// An existing file appended to is copied into the new version; larger ones are refused.
pub const MAX_APPEND_TO: u64 = 256 << 20;
/// A dry run includes a diff when both versions are UTF-8 and at most this large.
const DIFF_UP_TO: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
  Create,
  Overwrite,
  Append,
}

fn content(p: &Params) -> Result<Vec<u8>> {
  let given = [p.has("content"), p.has("contentBase64"), p.flag("stdin")];
  if given.iter().filter(|g| **g).count() != 1 {
    return Err(ToolError::invalid(
      "INVALID_PARAMS",
      "give exactly one of content, contentBase64 or stdin",
    ));
  }
  let bytes = if let Some(text) = p.str("content") {
    text.as_bytes().to_vec()
  } else if let Some(b64) = p.str("contentBase64") {
    base64_decode(b64)
      .ok_or_else(|| ToolError::invalid("INVALID_BASE64", "contentBase64 is not valid base64"))?
  } else {
    read_stdin(MAX_FILE_BYTES)?
  };
  if bytes.len() as u64 > MAX_FILE_BYTES {
    return Err(too_large("content", MAX_FILE_BYTES));
  }
  Ok(bytes)
}

pub fn run(p: &Params, ctx: &Ctx) -> Result<Value> {
  ctx.writable("write_file")?;
  let given = p
    .str("path")
    .ok_or_else(|| ToolError::invalid("INVALID_PATH", "path is required"))?;
  let target = absolute(given)?;
  ctx.gate.check_path(&target, given, Need::Write, || {
    let redirect = if p.str("mode") == Some("append") {
      ">>"
    } else {
      ">"
    };
    match p.str("content") {
      Some(text) if text.len() <= 1024 => {
        format!(
          "printf '%s' {} {redirect} {}",
          shell_quote(text),
          shell_quote(given)
        )
      }
      _ => format!("cat CONTENT_FILE {redirect} {}", shell_quote(given)),
    }
  })?;
  let mode = match p.str("mode").unwrap_or("overwrite") {
    "create" => Mode::Create,
    "overwrite" => Mode::Overwrite,
    "append" => Mode::Append,
    other => {
      return Err(ToolError::invalid(
        "INVALID_MODE",
        format!("mode {other:?} is not create, overwrite or append"),
      ))
    }
  };
  let content = content(p)?;
  let existing = atomic::inspect(&target, given)?;
  if mode == Mode::Create && existing != Existing::Missing {
    return Err(atomic::already_exists(&target));
  }
  let parent = target.parent().unwrap_or(std::path::Path::new("/"));
  let missing_parent = !parent.is_dir();
  if missing_parent && !p.flag("createParents") {
    return Err(ToolError::new(
      lmrust::server::ErrorCode::NotFound,
      "NOT_FOUND",
      format!("the directory of {given} does not exist (pass createParents to create it)"),
    ));
  }
  // The current bytes: needed to append, and to tell whether an overwrite changes anything.
  let before: Option<Vec<u8>> = match existing {
    Existing::File { size, .. } if mode == Mode::Append || size <= MAX_FILE_BYTES => {
      if mode == Mode::Append && size > MAX_APPEND_TO {
        return Err(too_large("the file to append to", MAX_APPEND_TO));
      }
      Some(std::fs::read(&target).map_err(|e| ToolError::io("read", &target, &e))?)
    }
    _ => None,
  };
  let after = match (&before, mode) {
    (Some(old), Mode::Append) => [old.as_slice(), &content].concat(),
    _ => content.clone(),
  };
  let created = existing == Existing::Missing;
  let changed = created || before.as_deref() != Some(after.as_slice());
  if ctx.dry_run {
    let mut out = json!({
      "dry_run": true,
      "executed": false,
      "would_change": changed,
      "path": given,
      "mode": p.str("mode").unwrap_or("overwrite"),
      "bytes": content.len(),
      "size_after": after.len(),
      "would_create": created,
      "would_overwrite": !created && mode == Mode::Overwrite,
      "would_create_parents": missing_parent,
    });
    let old = before.as_deref().unwrap_or_default();
    if let (Ok(a), Ok(b)) = (std::str::from_utf8(old), std::str::from_utf8(&after)) {
      if a.len() <= DIFF_UP_TO && b.len() <= DIFF_UP_TO {
        for (k, v) in unified(given, a, b).as_object().unwrap() {
          out[k] = v.clone();
        }
      }
    }
    return Ok(out);
  }
  if missing_parent {
    std::fs::create_dir_all(parent).map_err(|e| ToolError::io("create directory", parent, &e))?;
  }
  atomic::replace(&target, &after, existing, mode == Mode::Create)?;
  Ok(json!({
    "path": given,
    "mode": p.str("mode").unwrap_or("overwrite"),
    "bytes_written": content.len(),
    "size": after.len(),
    "created": created,
    "overwrote": !created && mode == Mode::Overwrite,
    "appended": mode == Mode::Append,
  }))
}
