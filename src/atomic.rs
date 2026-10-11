//! Atomic file replacement: the new bytes go to a temporary file in the target's directory and
//! are renamed over the target only when complete, so a reader never sees a partial file and an
//! error leaves the original untouched. The target's permission bits are kept.
//!
//! Refused targets: symlinks (the link would be replaced, not its target), anything that is not a
//! regular file, and hard-linked files (a rename would silently split the link, as lmprobe also
//! refuses).

use crate::error::{Result, ToolError};
use lmrust::server::ErrorCode;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Largest file write_file / edit_file produce.
pub const MAX_FILE_BYTES: u64 = 16 << 20;

/// What is at the target path now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Existing {
  Missing,
  /// A regular file with these permission bits (unix mode) and size.
  File {
    mode: u32,
    size: u64,
  },
}

/// Inspect a write target: refuses symlinks, non-regular files and hard-linked files.
pub fn inspect(path: &Path, given: &str) -> Result<Existing> {
  let meta = match std::fs::symlink_metadata(path) {
    Ok(meta) => meta,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Existing::Missing),
    Err(e) => return Err(ToolError::io("inspect", path, &e)),
  };
  if meta.file_type().is_symlink() {
    return Err(ToolError::invalid(
      "SYMLINK_TARGET",
      format!("{given} is a symlink; write to the file it points to instead"),
    ));
  }
  if !meta.is_file() {
    return Err(ToolError::invalid(
      "NOT_A_REGULAR_FILE",
      format!("{given} is not a regular file (a directory, device or fifo)"),
    ));
  }
  #[cfg(unix)]
  {
    use std::os::unix::fs::MetadataExt;
    if meta.nlink() > 1 {
      return Err(ToolError::invalid(
        "HARDLINKED_FILE",
        format!(
          "{given} has {} hard links; replacing it would split them (edit it with another tool)",
          meta.nlink()
        ),
      ));
    }
    Ok(Existing::File {
      mode: meta.mode() & 0o7777,
      size: meta.len(),
    })
  }
  #[cfg(not(unix))]
  Ok(Existing::File {
    mode: 0,
    size: meta.len(),
  })
}

/// 16 random hex digits (std's per-process random hash keys; no extra dependency).
pub fn random_hex() -> String {
  use std::hash::{BuildHasher, Hasher};
  let mut h = std::collections::hash_map::RandomState::new().build_hasher();
  h.write_u128(
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .map(|d| d.as_nanos())
      .unwrap_or(0),
  );
  h.write_u32(std::process::id());
  format!("{:016x}", h.finish())
}

/// A temporary file next to `target`, removed on drop unless committed.
pub struct TempFile {
  pub target: PathBuf,
  temp: Option<PathBuf>,
  pub file: File,
}

impl Drop for TempFile {
  fn drop(&mut self) {
    if let Some(temp) = self.temp.take() {
      let _ = std::fs::remove_file(temp);
    }
  }
}

impl TempFile {
  /// Create the temporary file (never an existing one). `mode`: the permission bits it gets
  /// (`None`: 0666 minus the umask, like a new file).
  pub fn create(target: &Path, mode: Option<u32>) -> Result<TempFile> {
    let dir = target.parent().unwrap_or(Path::new("/"));
    let name = target
      .file_name()
      .and_then(|n| n.to_str())
      .unwrap_or("file");
    let temp = dir.join(format!(".{name}.lmtools-{}.tmp", random_hex()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
      use std::os::unix::fs::OpenOptionsExt;
      options.mode(0o666);
    }
    let file = options
      .open(&temp)
      .map_err(|e| ToolError::io("create a file in", dir, &e))?;
    #[cfg(unix)]
    if let Some(mode) = mode {
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode))
        .map_err(|e| ToolError::io("set permissions on", &temp, &e))?;
    }
    #[cfg(not(unix))]
    let _ = mode;
    Ok(TempFile {
      target: target.to_path_buf(),
      temp: Some(temp),
      file,
    })
  }

  pub fn write_all(&mut self, bytes: &[u8]) -> Result<()> {
    let target = self.target.clone();
    self
      .file
      .write_all(bytes)
      .map_err(|e| ToolError::io("write", &target, &e))
  }

  /// Sync and move into place. `no_clobber`: fail with ALREADY_EXISTS instead of replacing an
  /// existing file (a hard link is atomic and refuses to overwrite).
  pub fn commit(mut self, no_clobber: bool) -> Result<()> {
    let target = self.target.clone();
    self
      .file
      .sync_all()
      .map_err(|e| ToolError::io("sync", &target, &e))?;
    let temp = self.temp.clone().expect("not committed yet");
    if no_clobber {
      match std::fs::hard_link(&temp, &target) {
        Ok(()) => {
          let _ = std::fs::remove_file(&temp);
          self.temp = None;
          return Ok(());
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
          return Err(already_exists(&target));
        }
        // No hard links on this filesystem: check, then rename.
        Err(_) if std::fs::symlink_metadata(&target).is_ok() => {
          return Err(already_exists(&target));
        }
        Err(_) => {}
      }
    }
    std::fs::rename(&temp, &target).map_err(|e| ToolError::io("replace", &target, &e))?;
    self.temp = None;
    Ok(())
  }
}

pub fn already_exists(path: &Path) -> ToolError {
  ToolError::new(
    ErrorCode::Conflict,
    "ALREADY_EXISTS",
    format!("{} already exists (mode create)", path.display()),
  )
}

/// Replace (or create) `target` with `bytes` atomically, keeping the existing permission bits.
pub fn replace(target: &Path, bytes: &[u8], existing: Existing, no_clobber: bool) -> Result<()> {
  let mode = match existing {
    Existing::File { mode, .. } => Some(mode),
    Existing::Missing => None,
  };
  let mut temp = TempFile::create(target, mode)?;
  temp.write_all(bytes)?;
  temp.commit(no_clobber)
}
