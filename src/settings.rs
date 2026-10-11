//! The settings file: guardrails that only RESTRICT what lmtools may touch (against an LLM's
//! honest mistakes; the OS file permissions stay the real boundary). Without the file lmtools is
//! open.
//!
//! Location: `$LMTOOLS_CONFIG`, else `$XDG_CONFIG_HOME/lmtools/settings.json`, else
//! `~/.config/lmtools/settings.json`. A file that exists but cannot be read or does not match the
//! schema exactly fails closed (`SETTINGS_INVALID`).
//!
//! ```json
//! {"version": 1, "cwd": "readwrite", "other": "readonly",
//!  "webget": {"allow_hosts": ["*.example.com", "docs.rs"], "deny_hosts": ["internal.example.com"]},
//!  "allow_bypass": false, "output": {"dir": "~/lmtools-out"}}
//! ```

use crate::error::{Result, ToolError};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

pub const CONFIG_ENV: &str = "LMTOOLS_CONFIG";

/// What lmtools may do with a set of paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
  None,
  ReadOnly,
  ReadWrite,
}

impl Level {
  pub fn as_str(self) -> &'static str {
    match self {
      Level::None => "none",
      Level::ReadOnly => "readonly",
      Level::ReadWrite => "readwrite",
    }
  }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
  pub source: PathBuf,
  /// The process working directory and everything under it.
  pub cwd: Level,
  /// Everything else.
  pub other: Level,
  /// `webget.allow_hosts`: when present, only these hosts (globs).
  pub allow_hosts: Option<Vec<String>>,
  /// `webget.deny_hosts`: never these hosts (globs; wins over allow).
  pub deny_hosts: Vec<String>,
  pub allow_bypass: bool,
  /// `output.dir` (resolved): `out` files and spills must stay inside it; spills go there.
  pub output_dir: Option<PathBuf>,
}

fn invalid(file: &Path, problem: impl std::fmt::Display) -> ToolError {
  ToolError::invalid(
    "SETTINGS_INVALID",
    format!("settings file {}: {problem}", file.display()),
  )
}

/// Where the settings file is looked for. `lookup` reads an environment variable.
pub fn settings_path(lookup: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
  let nonempty = |k: &str| lookup(k).filter(|v| !v.is_empty());
  if let Some(p) = nonempty(CONFIG_ENV) {
    return Some(PathBuf::from(p));
  }
  if let Some(xdg) = nonempty("XDG_CONFIG_HOME") {
    return Some(Path::new(&xdg).join("lmtools/settings.json"));
  }
  nonempty("HOME").map(|h| Path::new(&h).join(".config/lmtools/settings.json"))
}

/// Load the settings: `Ok(None)` when there is no file (open). `warn` gets non-fatal warnings.
pub fn load(
  lookup: &dyn Fn(&str) -> Option<String>,
  warn: &mut dyn FnMut(String),
) -> Result<Option<Settings>> {
  let Some(path) = settings_path(lookup) else {
    return Ok(None);
  };
  let text = match std::fs::read(&path) {
    Ok(bytes) => bytes,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
    Err(e) => return Err(invalid(&path, format!("cannot be read ({e})"))),
  };
  #[cfg(unix)]
  {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(&path) {
      if meta.permissions().mode() & 0o022 != 0 {
        warn(format!(
          "lmtools: warning: settings file {} is writable by group or others; chmod 600 it",
          path.display()
        ));
      }
    }
  }
  let doc: Value = serde_json::from_slice(&text)
    .map_err(|e| invalid(&path, format!("is not valid JSON ({e})")))?;
  parse(&doc, &path, lookup("HOME").as_deref()).map(Some)
}

fn object<'a>(
  v: &'a Value,
  at: &str,
  allowed: &[&str],
  file: &Path,
) -> Result<&'a Map<String, Value>> {
  let map = v
    .as_object()
    .ok_or_else(|| invalid(file, format!("{at} must be an object")))?;
  if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
    return Err(invalid(
      file,
      format!("unknown key {at}.{key} (allowed: {})", allowed.join(", ")),
    ));
  }
  Ok(map)
}

fn level(map: &Map<String, Value>, key: &str, choices: &[Level], file: &Path) -> Result<Level> {
  let names: Vec<&str> = choices.iter().map(|l| l.as_str()).collect();
  let value = map
    .get(key)
    .ok_or_else(|| invalid(file, format!("{key} is required ({})", names.join(" | "))))?;
  choices
    .iter()
    .copied()
    .find(|l| value.as_str() == Some(l.as_str()))
    .ok_or_else(|| invalid(file, format!("{key} must be one of {}", names.join(", "))))
}

fn hosts(map: &Map<String, Value>, key: &str, file: &Path) -> Result<Option<Vec<String>>> {
  match map.get(key) {
    None => Ok(None),
    Some(Value::Array(items)) => items
      .iter()
      .map(|v| {
        v.as_str()
          .filter(|s| !s.is_empty())
          .map(str::to_ascii_lowercase)
          .ok_or_else(|| {
            invalid(
              file,
              format!("webget.{key} must be a list of host names or globs"),
            )
          })
      })
      .collect::<Result<Vec<_>>>()
      .map(Some),
    Some(_) => Err(invalid(
      file,
      format!("webget.{key} must be a list of host names or globs"),
    )),
  }
}

/// `~/x` -> `$HOME/x`; a relative path is relative to the settings file's directory.
fn resolve_dir(raw: &str, file: &Path, home: Option<&str>) -> Result<PathBuf> {
  let path = match raw.strip_prefix("~/") {
    Some(rest) => {
      Path::new(home.ok_or_else(|| invalid(file, "output.dir uses ~ but HOME is not set"))?)
        .join(rest)
    }
    None => PathBuf::from(raw),
  };
  Ok(if path.is_relative() {
    file.parent().unwrap_or(Path::new(".")).join(path)
  } else {
    path
  })
}

/// Validate a settings document (schema version 1).
pub fn parse(doc: &Value, file: &Path, home: Option<&str>) -> Result<Settings> {
  let map = object(
    doc,
    "(top level)",
    &[
      "version",
      "cwd",
      "other",
      "webget",
      "allow_bypass",
      "output",
    ],
    file,
  )?;
  match map.get("version") {
    Some(v) if v.as_u64() == Some(1) => {}
    Some(_) => return Err(invalid(file, "version must be 1")),
    None => return Err(invalid(file, "version is required (1)")),
  }
  let cwd = level(map, "cwd", &[Level::ReadOnly, Level::ReadWrite], file)?;
  let other = level(
    map,
    "other",
    &[Level::None, Level::ReadOnly, Level::ReadWrite],
    file,
  )?;
  let (allow_hosts, deny_hosts) = match map.get("webget") {
    None => (None, Vec::new()),
    Some(v) => {
      let w = object(v, "webget", &["allow_hosts", "deny_hosts"], file)?;
      (
        hosts(w, "allow_hosts", file)?,
        hosts(w, "deny_hosts", file)?.unwrap_or_default(),
      )
    }
  };
  let allow_bypass = match map.get("allow_bypass") {
    None => false,
    Some(Value::Bool(b)) => *b,
    Some(_) => return Err(invalid(file, "allow_bypass must be true or false")),
  };
  let output_dir = match map.get("output") {
    None => None,
    Some(v) => {
      let o = object(v, "output", &["dir"], file)?;
      match o.get("dir") {
        None => None,
        Some(Value::String(d)) if !d.is_empty() => Some(resolve_dir(d, file, home)?),
        Some(_) => return Err(invalid(file, "output.dir must be a non-empty string")),
      }
    }
  };
  Ok(Settings {
    source: file.to_path_buf(),
    cwd,
    other,
    allow_hosts,
    deny_hosts,
    allow_bypass,
    output_dir,
  })
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn p(doc: Value) -> std::result::Result<Settings, String> {
    parse(&doc, Path::new("/cfg/settings.json"), Some("/home/u")).map_err(|e| {
      assert_eq!(e.tool_code, "SETTINGS_INVALID");
      assert!(e.message.contains("/cfg/settings.json"), "{}", e.message);
      e.message
    })
  }

  #[test]
  fn a_valid_document() {
    let s = p(json!({"version": 1, "cwd": "readwrite", "other": "none",
      "webget": {"allow_hosts": ["*.Example.com"]}, "allow_bypass": true, "output": {"dir": "~/out"}}))
    .unwrap();
    assert_eq!(
      (s.cwd, s.other, s.allow_bypass),
      (Level::ReadWrite, Level::None, true)
    );
    assert_eq!(s.allow_hosts, Some(vec!["*.example.com".to_string()]));
    assert_eq!(s.output_dir, Some(PathBuf::from("/home/u/out")));
    let s =
      p(json!({"version": 1, "cwd": "readonly", "other": "readonly", "output": {"dir": "rel"}}))
        .unwrap();
    assert_eq!(s.output_dir, Some(PathBuf::from("/cfg/rel")));
  }

  #[test]
  fn invalid_documents_name_the_problem() {
    for (doc, expect) in [
      (json!([]), "must be an object"),
      (
        json!({"cwd": "readwrite", "other": "none"}),
        "version is required",
      ),
      (
        json!({"version": 2, "cwd": "readwrite", "other": "none"}),
        "version must be 1",
      ),
      (json!({"version": 1, "other": "none"}), "cwd is required"),
      (
        json!({"version": 1, "cwd": "none", "other": "none"}),
        "cwd must be one of readonly, readwrite",
      ),
      (
        json!({"version": 1, "cwd": "readwrite", "other": "admin"}),
        "other must be one of",
      ),
      (
        json!({"version": 1, "cwd": "readwrite", "other": "none", "extra": 1}),
        "unknown key (top level).extra",
      ),
      (
        json!({"version": 1, "cwd": "readwrite", "other": "none", "webget": {"allow": []}}),
        "unknown key webget.allow",
      ),
      (
        json!({"version": 1, "cwd": "readwrite", "other": "none", "webget": {"deny_hosts": "x"}}),
        "must be a list",
      ),
      (
        json!({"version": 1, "cwd": "readwrite", "other": "none", "allow_bypass": "yes"}),
        "allow_bypass must be",
      ),
      (
        json!({"version": 1, "cwd": "readwrite", "other": "none", "output": {"dir": 3}}),
        "output.dir must be",
      ),
    ] {
      let message = p(doc.clone()).unwrap_err();
      assert!(message.contains(expect), "{doc}: {message}");
    }
  }

  #[test]
  fn settings_path_order() {
    let env = |pairs: &'static [(&'static str, &'static str)]| {
      move |k: &str| {
        pairs
          .iter()
          .find(|(n, _)| *n == k)
          .map(|(_, v)| v.to_string())
      }
    };
    let f = env(&[
      ("LMTOOLS_CONFIG", "/a.json"),
      ("XDG_CONFIG_HOME", "/x"),
      ("HOME", "/h"),
    ]);
    assert_eq!(settings_path(&f), Some("/a.json".into()));
    let f = env(&[("XDG_CONFIG_HOME", "/x"), ("HOME", "/h")]);
    assert_eq!(settings_path(&f), Some("/x/lmtools/settings.json".into()));
    let f = env(&[("HOME", "/h")]);
    assert_eq!(
      settings_path(&f),
      Some("/h/.config/lmtools/settings.json".into())
    );
  }
}
