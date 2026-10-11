//! `list_files`: a directory listing, optionally recursive, in a deterministic order
//! (depth-first, each directory's entries sorted by name), paged with offset/limit.

use super::{Ctx, Params};
use crate::error::{Result, ToolError};
use crate::permissions::{manual_path, Need};
use crate::util::{absolute, glob_regex, rfc3339};
use serde_json::{json, Value};
use std::path::Path;

pub const DEFAULT_LIMIT: u64 = 200;
pub const MAX_LIMIT: u64 = 5000;

fn kind(meta: &std::fs::Metadata) -> &'static str {
  let t = meta.file_type();
  if t.is_symlink() {
    "symlink"
  } else if t.is_dir() {
    "dir"
  } else if t.is_file() {
    "file"
  } else {
    "other"
  }
}

struct Walk {
  glob: Option<regex::Regex>,
  glob_on_path: bool,
  hidden: bool,
  max_depth: u64,
  skip: u64,
  limit: u64,
  entries: Vec<Value>,
  seen: u64,
  unreadable: u64,
}

impl Walk {
  /// Visit `dir` (relative path `rel`; its children are at `depth`). Returns false once the
  /// page is full and another entry exists (so `truncated` is exact).
  fn visit(&mut self, dir: &Path, rel: &str, depth: u64) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else {
      self.unreadable += 1;
      return true;
    };
    let mut children: Vec<(String, std::path::PathBuf)> = read
      .filter_map(|e| e.ok())
      .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
      .collect();
    children.sort();
    for (name, path) in children {
      if !self.hidden && name.starts_with('.') {
        continue;
      }
      let child_rel = if rel.is_empty() {
        name.clone()
      } else {
        format!("{rel}/{name}")
      };
      let Ok(meta) = std::fs::symlink_metadata(&path) else {
        self.unreadable += 1;
        continue;
      };
      let matches = match &self.glob {
        None => true,
        Some(re) => re.is_match(if self.glob_on_path { &child_rel } else { &name }),
      };
      if matches {
        self.seen += 1;
        if self.seen > self.skip {
          if self.entries.len() as u64 == self.limit {
            return false;
          }
          self.entries.push(json!({
            "name": name,
            "path": child_rel,
            "type": kind(&meta),
            "size": if meta.is_file() { json!(meta.len()) } else { Value::Null },
            "mtime": meta.modified().ok().map(rfc3339),
          }));
        }
      }
      // Symlinked directories are listed, never followed.
      if meta.is_dir() && depth < self.max_depth && !self.visit(&path, &child_rel, depth + 1) {
        return false;
      }
    }
    true
  }
}

pub fn run(p: &Params, ctx: &Ctx) -> Result<Value> {
  let given = p.str("path").unwrap_or(".");
  let root = absolute(given)?;
  ctx
    .gate
    .check_path(&root, given, Need::Read, || manual_path("ls -la", given))?;
  let meta = std::fs::metadata(&root).map_err(|e| ToolError::io("list", &root, &e))?;
  if !meta.is_dir() {
    return Err(ToolError::invalid(
      "NOT_A_DIRECTORY",
      format!("{given} is not a directory (use read_file for files)"),
    ));
  }
  let max_depth = if p.flag("recursive") {
    p.uint("maxDepth", u64::MAX, 1, u64::MAX)?
  } else {
    1
  };
  let offset = p.uint("offset", 0, 0, u64::MAX)?;
  let limit = p.uint("limit", DEFAULT_LIMIT, 1, MAX_LIMIT)?;
  let glob = p.str("glob");
  let mut walk = Walk {
    glob: glob.map(glob_regex).transpose()?,
    glob_on_path: glob.is_some_and(|g| g.contains('/')),
    hidden: p.flag("includeHidden"),
    max_depth,
    skip: offset,
    limit,
    entries: Vec::new(),
    seen: 0,
    unreadable: 0,
  };
  let complete = walk.visit(&root, "", 1);
  let count = walk.entries.len() as u64;
  Ok(json!({
    "path": given,
    "entries": walk.entries,
    "count": count,
    "offset": offset,
    "truncated": !complete,
    "next_offset": if complete { Value::Null } else { json!(offset + count) },
    "unreadable": walk.unreadable,
  }))
}
