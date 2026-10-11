//! The operations. Each takes the validated lmapi params and returns the result document.

pub mod edit;
pub mod list;
pub mod read;
pub mod webget;
pub mod write;

use crate::error::{Result, ToolError};
use lmrust::Record;
use serde_json::Value;

/// How a call runs: lmrust's policy globals and the permission gate.
pub struct Ctx {
  /// `--dry-run` / `LMTOOLS_DRYRUN`: print what would happen, never write.
  pub dry_run: bool,
  /// `--readonly` / `LMTOOLS_READONLY`: write ops are refused.
  pub read_only: bool,
  /// The settings file's rules (open without one, or with an allowed bypass).
  pub gate: crate::permissions::Gate,
}

impl Ctx {
  /// Refuse a write op under `--readonly`.
  pub fn writable(&self, op: &str) -> Result<()> {
    if self.read_only {
      return Err(ToolError::new(
        lmrust::server::ErrorCode::ReadOnlyViolation,
        "READONLY_VIOLATION",
        format!("{op} writes, and this call runs with --readonly"),
      ));
    }
    Ok(())
  }
}

/// Read all of stdin, at most `max` bytes (more is an error).
pub fn read_stdin(max: u64) -> Result<Vec<u8>> {
  use std::io::Read;
  let mut buf = Vec::new();
  std::io::stdin()
    .lock()
    .take(max + 1)
    .read_to_end(&mut buf)
    .map_err(|e| ToolError::io("read", std::path::Path::new("<stdin>"), &e))?;
  if buf.len() as u64 > max {
    return Err(too_large("stdin", max));
  }
  Ok(buf)
}

pub fn too_large(what: &str, max: u64) -> ToolError {
  ToolError::invalid(
    "TOO_LARGE",
    format!("{what} is larger than {} MiB", max >> 20),
  )
}

/// Typed access to validated params.
pub struct Params<'a>(pub &'a Record);

impl<'a> Params<'a> {
  pub fn str(&self, name: &str) -> Option<&'a str> {
    self.0.get(name).and_then(Value::as_str)
  }
  pub fn flag(&self, name: &str) -> bool {
    self.0.get(name) == Some(&Value::Bool(true))
  }
  pub fn has(&self, name: &str) -> bool {
    self.0.get(name).is_some_and(|v| !v.is_null())
  }
  /// A whole number in `min..=max`, or `default` when absent.
  pub fn uint(&self, name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let Some(v) = self.0.get(name).and_then(Value::as_f64) else {
      return Ok(default);
    };
    if v.fract() != 0.0 || v < min as f64 || v > max as f64 {
      return Err(ToolError::invalid(
        "LIMIT_OUT_OF_RANGE",
        format!("{name} must be a whole number from {min} to {max}"),
      ));
    }
    Ok(v as u64)
  }
  pub fn opt_uint(&self, name: &str, min: u64, max: u64) -> Result<Option<u64>> {
    if self.has(name) {
      self.uint(name, 0, min, max).map(Some)
    } else {
      Ok(None)
    }
  }
}
