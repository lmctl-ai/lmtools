//! Result files (lmsql's convention): `out` is a file the caller names; a body too big to return
//! inline spills to `$LMTOOLS_OUT_DIR` (default `<temp>/lmtools/`, created 0700 and required to be
//! private). Files are written to a temporary name (mode 0600) and renamed into place only when
//! complete; on any error the temporary file is removed. lmtools never deletes old spill files.

use crate::atomic::{random_hex, TempFile};
use crate::error::{Result, ToolError};
use crate::util::utc_parts;
use lmrust::server::ErrorCode;
use std::path::{Path, PathBuf};

pub const OUT_DIR_ENV: &str = "LMTOOLS_OUT_DIR";

pub fn out_error(message: impl Into<String>) -> ToolError {
  ToolError::new(ErrorCode::Forbidden, "OUT_NOT_WRITABLE", message)
}

/// Resolve an `out` path: absolute, not a directory. With `create_dirs`, missing parents are
/// created (not in a dry run).
pub fn out_path(raw: &str, create_dirs: bool) -> Result<PathBuf> {
  if raw.trim().is_empty() || raw.ends_with('/') {
    return Err(out_error("out must name a file"));
  }
  let path =
    std::path::absolute(raw).map_err(|e| out_error(format!("cannot resolve {raw}: {e}")))?;
  if std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
    return Err(out_error(format!(
      "out names a directory: {}",
      path.display()
    )));
  }
  let dir = path.parent().unwrap_or(Path::new("/"));
  if create_dirs {
    std::fs::create_dir_all(dir)
      .map_err(|e| out_error(format!("cannot create directory {}: {e}", dir.display())))?;
  }
  Ok(path)
}

/// The spill directory: `root` (the settings file's output.dir) if given, else `$LMTOOLS_OUT_DIR`,
/// else `<temp>/lmtools` (which must be a real directory owned by this user and closed to others,
/// since the system temp directory is shared). Created 0700.
pub fn spill_dir(root: Option<&Path>) -> Result<PathBuf> {
  let configured = match root {
    Some(root) => Some(root.to_path_buf()),
    None => std::env::var(OUT_DIR_ENV)
      .ok()
      .filter(|d| !d.is_empty())
      .map(PathBuf::from),
  };
  let default = configured.is_none();
  let dir = match configured {
    Some(d) => std::path::absolute(&d)
      .map_err(|e| out_error(format!("cannot resolve {}: {e}", d.display())))?,
    None => std::env::temp_dir().join("lmtools"),
  };
  let mut builder = std::fs::DirBuilder::new();
  builder.recursive(true);
  #[cfg(unix)]
  {
    use std::os::unix::fs::DirBuilderExt;
    builder.mode(0o700);
  }
  builder
    .create(&dir)
    .map_err(|e| out_error(format!("cannot create directory {}: {e}", dir.display())))?;
  let meta = std::fs::symlink_metadata(&dir)
    .map_err(|e| out_error(format!("cannot inspect {}: {e}", dir.display())))?;
  if !meta.is_dir() {
    return Err(out_error(format!(
      "spill directory {} is not a directory (a symlink or file)",
      dir.display()
    )));
  }
  #[cfg(unix)]
  if default {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    if meta.uid() != me || meta.mode() & 0o077 != 0 {
      return Err(out_error(format!(
        "spill directory {} is not private to this user; set {OUT_DIR_ENV}",
        dir.display()
      )));
    }
  }
  let _ = default;
  Ok(dir)
}

/// `lmtools-<UTC yyyymmddThhmmssZ>-<random>.<ext>` in the spill directory.
pub fn spill_path(root: Option<&Path>, ext: &str) -> Result<PathBuf> {
  let secs = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0);
  let (y, m, d, hh, mm, ss) = utc_parts(secs);
  let stamp = format!("{y:04}{m:02}{d:02}T{hh:02}{mm:02}{ss:02}Z");
  Ok(spill_dir(root)?.join(format!("lmtools-{stamp}-{}.{ext}", &random_hex()[..8])))
}

/// A result file being written: temporary until [`OutFile::commit`].
pub struct OutFile {
  temp: TempFile,
  pub bytes: u64,
}

impl OutFile {
  pub fn create(path: &Path) -> Result<OutFile> {
    let temp = TempFile::create(path, Some(0o600)).map_err(|e| out_error(e.message))?;
    Ok(OutFile { temp, bytes: 0 })
  }

  pub fn write(&mut self, bytes: &[u8]) -> Result<()> {
    self
      .temp
      .write_all(bytes)
      .map_err(|e| out_error(e.message))?;
    self.bytes += bytes.len() as u64;
    Ok(())
  }

  /// Move into place; returns whether a file was replaced.
  pub fn commit(self) -> Result<bool> {
    let overwrote = std::fs::symlink_metadata(&self.temp.target).is_ok();
    self.temp.commit(false).map_err(|e| out_error(e.message))?;
    Ok(overwrote)
  }
}
