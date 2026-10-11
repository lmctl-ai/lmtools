//! The operations. Each takes the validated lmapi params and returns the result document.

pub mod list;
pub mod read;

use crate::error::{Result, ToolError};
use lmrust::Record;
use serde_json::Value;

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
