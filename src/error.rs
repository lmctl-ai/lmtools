//! Errors: every failure carries a standard lmapi code (exit code) and a tool code kept in
//! `error.data.tool_code`, plus optional extra data (e.g. `manual_command`).

use lmrust::server::{CliError, ErrorCode};
use serde_json::{Map, Value};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct ToolError {
  pub code: ErrorCode,
  pub tool_code: String,
  pub message: String,
  pub data: Map<String, Value>,
}

pub type Result<T> = std::result::Result<T, ToolError>;

impl ToolError {
  pub fn new(code: ErrorCode, tool_code: impl Into<String>, message: impl Into<String>) -> Self {
    ToolError {
      code,
      tool_code: tool_code.into(),
      message: message.into(),
      data: Map::new(),
    }
  }

  /// A request that cannot be served as given (exit 2).
  pub fn invalid(tool_code: &str, message: impl Into<String>) -> Self {
    ToolError::new(ErrorCode::ValidationError, tool_code, message)
  }

  pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
    self.data.insert(key.to_string(), value.into());
    self
  }

  /// An I/O failure on `path` while doing `what`.
  pub fn io(what: &str, path: &Path, e: &std::io::Error) -> Self {
    use std::io::ErrorKind;
    let message = format!("cannot {what} {}: {e}", path.display());
    match e.kind() {
      ErrorKind::NotFound => ToolError::new(ErrorCode::NotFound, "NOT_FOUND", message),
      ErrorKind::PermissionDenied => {
        ToolError::new(ErrorCode::Forbidden, "OS_PERMISSION_DENIED", message)
      }
      _ => ToolError::new(ErrorCode::InternalError, "IO_ERROR", message),
    }
  }

  pub fn into_cli_error(self) -> CliError {
    let mut err = CliError::new(self.code, self.message).tool_code(self.tool_code);
    for (key, value) in self.data {
      err = err.with(&key, value);
    }
    err
  }
}
