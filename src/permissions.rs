//! The permission gate between an op and the file system / network, driven by the settings file.
//! Without settings (or with an allowed bypass) every check passes.

use crate::error::{Result, ToolError};
use crate::settings::{Level, Settings};
use crate::util::{glob_regex, shell_quote};
use lmrust::server::ErrorCode;
use std::path::{Path, PathBuf};

/// What an op needs on a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Need {
  Read,
  Write,
}

pub struct Gate {
  /// `None`: open (no settings file, or an allowed bypass).
  settings: Option<Settings>,
  /// The canonical working directory.
  cwd: PathBuf,
}

/// Where `path` really is: canonical (symlinks, also dangling ones, and `..` followed). For a
/// path that does not exist yet, the canonical nearest existing ancestor plus the remaining names
/// (`None` if a remaining component is `..`).
pub fn resolve_real(path: &Path) -> Option<PathBuf> {
  let mut rest = Vec::new();
  let mut current = path.to_path_buf();
  for _ in 0..64 {
    if let Ok(mut real) = current.canonicalize() {
      for name in rest.iter().rev() {
        real.push(name);
      }
      return Some(real);
    }
    if let Ok(target) = std::fs::read_link(&current) {
      current = if target.is_absolute() {
        target
      } else {
        current.parent()?.join(target)
      };
      continue;
    }
    rest.push(current.file_name()?.to_os_string());
    current = current.parent()?.to_path_buf();
  }
  None
}

pub fn denied(message: String, manual_command: Option<String>) -> ToolError {
  let mut err = ToolError::new(ErrorCode::Forbidden, "PERMISSION_DENIED", message);
  if let Some(command) = manual_command {
    err = err.with("manual_command", command);
  }
  err
}

impl Gate {
  pub fn open() -> Gate {
    Gate {
      settings: None,
      cwd: PathBuf::new(),
    }
  }

  pub fn new(settings: Settings) -> Result<Gate> {
    let cwd = std::env::current_dir()
      .and_then(|d| d.canonicalize())
      .map_err(|e| ToolError::io("resolve", Path::new("."), &e))?;
    Ok(Gate {
      settings: Some(settings),
      cwd,
    })
  }

  fn file(&self) -> String {
    self
      .settings
      .as_ref()
      .map(|s| s.source.display().to_string())
      .unwrap_or_default()
  }

  /// `path` (absolute) needs `need`; `given` is the path as the caller wrote it, `manual` the
  /// operator's equivalent shell command.
  pub fn check_path(
    &self,
    path: &Path,
    given: &str,
    need: Need,
    manual: impl FnOnce() -> String,
  ) -> Result<()> {
    let Some(settings) = &self.settings else {
      return Ok(());
    };
    let real = resolve_real(path);
    let inside = real.as_ref().is_some_and(|r| r.starts_with(&self.cwd));
    let (level, rule) = if inside {
      (settings.cwd, "cwd")
    } else {
      (settings.other, "other")
    };
    let enough = match need {
      Need::Read => level >= Level::ReadOnly,
      Need::Write => level == Level::ReadWrite,
    };
    if enough {
      return Ok(());
    }
    let place = if inside {
      "inside the working directory"
    } else {
      "outside the working directory"
    };
    let what = if need == Need::Write { "write" } else { "read" };
    Err(denied(
      format!(
        "permission denied: {given} is {place}, which is {} (\"{rule}\" in {}); lmtools may not {what} it",
        level.as_str(),
        self.file()
      ),
      Some(manual()),
    ))
  }

  /// A webget host: `deny_hosts` wins; with `allow_hosts`, the host must match one of them.
  pub fn check_host(&self, url: &url::Url, manual: impl FnOnce() -> String) -> Result<()> {
    let Some(settings) = &self.settings else {
      return Ok(());
    };
    let host = url
      .host_str()
      .unwrap_or("")
      .trim_matches(['[', ']'])
      .to_ascii_lowercase();
    let matches = |globs: &[String]| {
      globs
        .iter()
        .any(|g| glob_regex(g).is_ok_and(|re| re.is_match(&host)))
    };
    let refusal = if matches(&settings.deny_hosts) {
      Some("is in webget.deny_hosts")
    } else if settings
      .allow_hosts
      .as_ref()
      .is_some_and(|allow| !matches(allow))
    {
      Some("is not in webget.allow_hosts")
    } else {
      None
    };
    match refusal {
      None => Ok(()),
      Some(why) => Err(denied(
        format!("permission denied: host {host} {why} ({})", self.file()),
        Some(manual()),
      )),
    }
  }

  /// The settings file's output.dir (where spills go; `out` must stay inside).
  pub fn output_dir(&self) -> Option<&Path> {
    self.settings.as_ref().and_then(|s| s.output_dir.as_deref())
  }

  /// An `out` file: inside output.dir when set, and writable by the path rules.
  pub fn check_out(&self, out: &Path, manual: impl Fn() -> String) -> Result<()> {
    if let Some(root) = self.output_dir() {
      let inside = match (resolve_real(root), resolve_real(out)) {
        (Some(root), Some(real)) => real.starts_with(&root) && real != root,
        _ => false,
      };
      if !inside {
        return Err(denied(
          format!(
            "permission denied: out {} is outside output.dir {} ({}); write under that directory",
            out.display(),
            root.display(),
            self.file()
          ),
          Some(manual()),
        ));
      }
      return Ok(());
    }
    self.check_path(out, &out.display().to_string(), Need::Write, manual)
  }
}

/// `cat 'path'` and friends, for denials.
pub fn manual_path(command: &str, given: &str) -> String {
  format!("{command} {}", shell_quote(given))
}
