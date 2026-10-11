//! `edit_file`: the text-edit engine ported from lmprobe (`crate::text_edit`) applied to one file,
//! written atomically with its permission bits kept. Returns the unified diff and counts; a dry
//! run returns the same without writing.

use super::{read_stdin, too_large, Ctx, Params};
use crate::atomic::{self, Existing, MAX_FILE_BYTES};
use crate::diff::unified;
use crate::error::{Result, ToolError};
use crate::permissions::{manual_path, Need};
use crate::text_edit::{parse_range, parse_script, transform, EditOperation};
use crate::util::absolute;
use lmrust::server::ErrorCode;
use serde_json::{json, Value};

fn invalid(message: impl Into<String>) -> ToolError {
  ToolError::invalid("INVALID_EDIT", message)
}

/// The edit to apply: exactly one operation selector (as lmprobe's `edit`).
fn operations(p: &Params) -> Result<Vec<EditOperation>> {
  const SELECTORS: [&str; 8] = [
    "pattern",
    "script",
    "insertBefore",
    "insertAfter",
    "deleteLines",
    "replaceLines",
    "prepend",
    "append",
  ];
  let chosen: Vec<&str> = SELECTORS
    .iter()
    .copied()
    .filter(|s| {
      if *s == "prepend" || *s == "append" {
        p.flag(s)
      } else {
        p.has(s)
      }
    })
    .collect();
  if chosen.len() != 1 {
    return Err(invalid(format!(
      "choose exactly one edit: {} (got {})",
      SELECTORS.join(", "),
      if chosen.is_empty() {
        "none".to_string()
      } else {
        chosen.join(", ")
      }
    )));
  }
  let selector = chosen[0];
  let uses_text = matches!(
    selector,
    "insertBefore" | "insertAfter" | "replaceLines" | "prepend" | "append"
  );
  if p.has("text") != uses_text {
    return Err(invalid(if uses_text {
      format!("{selector} needs text")
    } else {
      format!("text is not used by {selector}")
    }));
  }
  for flag in ["replacement", "regex", "word", "first", "ignoreCase"] {
    if selector != "pattern" && p.has(flag) {
      return Err(invalid(format!("{flag} only applies to pattern")));
    }
  }
  let text = p.str("text").unwrap_or_default().to_string();
  let line = |name: &str| -> Result<usize> { Ok(p.uint(name, 0, 1, u32::MAX as u64)? as usize) };
  let range = |name: &str| -> Result<(usize, usize)> {
    parse_range(p.str(name).unwrap_or_default()).map_err(|e| invalid(format!("{name}: {e}")))
  };
  Ok(vec![match selector {
    "pattern" => EditOperation::Replace {
      pattern: p.str("pattern").unwrap_or_default().to_string(),
      replacement: p
        .str("replacement")
        .ok_or_else(|| invalid("pattern needs replacement"))?
        .to_string(),
      regex: p.flag("regex"),
      word: p.flag("word"),
      ignore_case: p.flag("ignoreCase"),
      first: p.flag("first"),
    },
    "script" => {
      let script = p.str("script").unwrap_or_default();
      let json = if script == "-" {
        String::from_utf8(read_stdin(MAX_FILE_BYTES)?)
          .map_err(|_| invalid("script on stdin is not UTF-8"))?
      } else {
        script.to_string()
      };
      return parse_script(&json).map_err(invalid);
    }
    "insertBefore" => EditOperation::InsertBefore {
      line: line("insertBefore")?,
      text,
    },
    "insertAfter" => EditOperation::InsertAfter {
      line: line("insertAfter")?,
      text,
    },
    "deleteLines" => {
      let (start, end) = range("deleteLines")?;
      EditOperation::DeleteLines { start, end }
    }
    "replaceLines" => {
      let (start, end) = range("replaceLines")?;
      EditOperation::ReplaceLines { start, end, text }
    }
    "prepend" => EditOperation::Prepend { text },
    _ => EditOperation::Append { text },
  }])
}

pub fn run(p: &Params, ctx: &Ctx) -> Result<Value> {
  ctx.writable("edit_file")?;
  let given = p
    .str("path")
    .ok_or_else(|| ToolError::invalid("INVALID_PATH", "path is required"))?;
  let target = absolute(given)?;
  ctx.gate.check_path(&target, given, Need::Write, || {
    manual_path("\"${EDITOR:-vi}\"", given)
  })?;
  let ops = operations(p)?;
  let expected = p.opt_uint("expectedCount", 0, u32::MAX as u64)?;
  if expected.is_some()
    && !ops
      .iter()
      .any(|o| matches!(o, EditOperation::Replace { .. }))
  {
    return Err(invalid(
      "expectedCount applies to replacements (pattern or a script with replace)",
    ));
  }
  let existing = atomic::inspect(&target, given)?;
  let Existing::File { size, .. } = existing else {
    return Err(ToolError::new(
      ErrorCode::NotFound,
      "NOT_FOUND",
      format!("{given} does not exist (use write_file to create it)"),
    ));
  };
  if size > MAX_FILE_BYTES {
    return Err(too_large(given, MAX_FILE_BYTES));
  }
  let bytes = std::fs::read(&target).map_err(|e| ToolError::io("read", &target, &e))?;
  let before = String::from_utf8(bytes).map_err(|_| {
    ToolError::invalid(
      "NOT_UTF8",
      format!("{given} is not valid UTF-8; edit_file edits text"),
    )
  })?;
  let result = transform(&before, &ops).map_err(invalid)?;
  if let Some(expected) = expected {
    if result.replacements as u64 != expected {
      return Err(
        ToolError::new(
          ErrorCode::Conflict,
          "EDIT_COUNT_MISMATCH",
          format!(
            "expected {expected} replacement(s) in {given}, found {}; nothing was written",
            result.replacements
          ),
        )
        .with("replacements", result.replacements)
        .with("expected_count", expected),
      );
    }
  }
  if result.text.len() as u64 > MAX_FILE_BYTES {
    return Err(too_large("the edited file", MAX_FILE_BYTES));
  }
  let changed = result.text != before;
  let mut out = unified(given, &before, &result.text);
  out["path"] = json!(given);
  out["replacements"] = json!(result.replacements);
  out["bytes_before"] = json!(before.len());
  out["bytes_after"] = json!(result.text.len());
  if ctx.dry_run {
    out["dry_run"] = json!(true);
    out["executed"] = json!(false);
    out["would_change"] = json!(changed);
    return Ok(out);
  }
  if changed {
    atomic::replace(&target, result.text.as_bytes(), existing, false)?;
  }
  out["changed"] = json!(changed);
  Ok(out)
}
