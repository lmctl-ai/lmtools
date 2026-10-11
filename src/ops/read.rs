//! `read_file`: one page of a file, by bytes (`offset` / `maxBytes`) or by lines (`startLine` /
//! `endLine`). Stateless paging: the response says where the next page starts.

use super::Params;
use crate::error::{Result, ToolError};
use crate::util::{absolute, base64};
use serde_json::{json, Value};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub const DEFAULT_MAX_BYTES: u64 = 64 << 10;
pub const MAX_BYTES: u64 = 1 << 20;
/// Bytes inspected for a NUL to call a file binary.
pub const SNIFF_BYTES: usize = 8 << 10;
/// `total_lines` is counted only for files up to this size (cheap).
pub const COUNT_LINES_UP_TO: u64 = 8 << 20;

fn open(path: &Path, given: &str) -> Result<(File, u64)> {
  let meta = std::fs::metadata(path).map_err(|e| ToolError::io("read", path, &e))?;
  if !meta.is_file() {
    return Err(ToolError::invalid(
      "NOT_A_REGULAR_FILE",
      format!("{given} is not a regular file (a directory, device or fifo)"),
    ));
  }
  let file = File::open(path).map_err(|e| ToolError::io("read", path, &e))?;
  Ok((file, meta.len()))
}

/// A NUL byte in the first [`SNIFF_BYTES`] bytes.
pub fn looks_binary(bytes: &[u8]) -> bool {
  bytes[..bytes.len().min(SNIFF_BYTES)].contains(&0)
}

fn is_binary(file: &mut File, path: &Path) -> Result<bool> {
  let mut head = Vec::with_capacity(SNIFF_BYTES);
  file
    .by_ref()
    .take(SNIFF_BYTES as u64)
    .read_to_end(&mut head)
    .map_err(|e| ToolError::io("read", path, &e))?;
  file
    .seek(SeekFrom::Start(0))
    .map_err(|e| ToolError::io("read", path, &e))?;
  Ok(looks_binary(&head))
}

fn count_newlines(reader: impl Read, path: &Path) -> Result<u64> {
  let mut reader = BufReader::with_capacity(1 << 16, reader);
  let mut count = 0u64;
  loop {
    let buf = reader
      .fill_buf()
      .map_err(|e| ToolError::io("read", path, &e))?;
    if buf.is_empty() {
      return Ok(count);
    }
    count += buf.iter().filter(|&&b| b == b'\n').count() as u64;
    let n = buf.len();
    reader.consume(n);
  }
}

/// Lines in the file (a final line without newline counts), when the file is small.
fn total_lines(path: &Path, size: u64) -> Result<Option<u64>> {
  if size > COUNT_LINES_UP_TO {
    return Ok(None);
  }
  let bytes = std::fs::read(path).map_err(|e| ToolError::io("read", path, &e))?;
  let newlines = bytes.iter().filter(|&&b| b == b'\n').count() as u64;
  Ok(Some(
    newlines + u64::from(!bytes.is_empty() && !bytes.ends_with(b"\n")),
  ))
}

/// Length of the longest prefix of `bytes` that does not end inside a UTF-8 sequence.
pub fn utf8_prefix(bytes: &[u8]) -> usize {
  let n = bytes.len();
  for back in 1..=3.min(n) {
    let b = bytes[n - back];
    if b & 0xC0 == 0x80 {
      continue; // a continuation byte: keep looking for the lead byte
    }
    let need = if b >= 0xF0 {
      4
    } else if b >= 0xE0 {
      3
    } else if b >= 0xC0 {
      2
    } else {
      1
    };
    return if need > back { n - back } else { n };
  }
  n
}

/// Bytes as text; invalid UTF-8 is replaced (U+FFFD) and reported as lossy.
pub fn text(bytes: &[u8]) -> (String, bool) {
  match std::str::from_utf8(bytes) {
    Ok(s) => (s.to_string(), false),
    Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
  }
}

fn numbered(content: &str, first_line: u64) -> String {
  let mut out = String::with_capacity(content.len() + content.len() / 8);
  for (i, line) in content.split_inclusive('\n').enumerate() {
    out.push_str(&format!("{:>6}\t{line}", first_line + i as u64));
  }
  out
}

pub fn run(p: &Params) -> Result<Value> {
  let given = p
    .str("path")
    .ok_or_else(|| ToolError::invalid("INVALID_PATH", "path is required"))?;
  let path = absolute(given)?;
  let (mut file, size) = open(&path, given)?;
  let max_bytes = p.uint("maxBytes", DEFAULT_MAX_BYTES, 1, MAX_BYTES)? as usize;
  let as_base64 = p.flag("asBase64");
  if !as_base64 && is_binary(&mut file, &path)? {
    return Err(
      ToolError::invalid(
        "BINARY_FILE",
        format!("{given} looks binary (a NUL byte in its first 8 KiB); pass asBase64 to read it"),
      )
      .with("total_size", size),
    );
  }
  let line_mode = p.has("startLine") || p.has("endLine");
  if line_mode && (p.has("offset") || as_base64) {
    return Err(ToolError::invalid(
      "CONFLICTING_PARAMS",
      "line mode (startLine/endLine) cannot be combined with offset or asBase64",
    ));
  }
  let mut out = if line_mode {
    read_lines(p, &mut file, &path, max_bytes)?
  } else {
    read_bytes(p, &mut file, &path, size, max_bytes, as_base64)?
  };
  out["path"] = json!(given);
  out["total_size"] = json!(size);
  out["total_lines"] = json!(total_lines(&path, size)?);
  Ok(out)
}

fn read_bytes(
  p: &Params,
  file: &mut File,
  path: &Path,
  size: u64,
  max_bytes: usize,
  as_base64: bool,
) -> Result<Value> {
  let offset = p.uint("offset", 0, 0, u64::MAX)?;
  file
    .seek(SeekFrom::Start(offset.min(size)))
    .map_err(|e| ToolError::io("read", path, &e))?;
  let mut buf = Vec::with_capacity(max_bytes.min(size as usize));
  file
    .by_ref()
    .take(max_bytes as u64)
    .read_to_end(&mut buf)
    .map_err(|e| ToolError::io("read", path, &e))?;
  if !as_base64 && offset + (buf.len() as u64) < size {
    // Never end a page inside a UTF-8 sequence: the next page starts at the cut.
    let keep = utf8_prefix(&buf);
    if keep > 0 {
      buf.truncate(keep);
    }
  }
  let next = offset.min(size) + buf.len() as u64;
  let eof = next >= size;
  let mut out = json!({
    "offset": offset,
    "bytes_read": buf.len(),
    "next_offset": if eof { Value::Null } else { json!(next) },
    "eof": eof,
    "truncated": !eof,
  });
  if as_base64 {
    out["content_base64"] = json!(base64(&buf));
    out["encoding"] = json!("base64");
    return Ok(out);
  }
  let (mut content, lossy) = text(&buf);
  if p.flag("lineNumbers") {
    let first = if offset == 0 {
      1
    } else {
      let head = File::open(path).map_err(|e| ToolError::io("read", path, &e))?;
      count_newlines(head.take(offset), path)? + 1
    };
    content = numbered(&content, first);
  }
  out["content"] = json!(content);
  out["encoding"] = json!("utf-8");
  out["lossy"] = json!(lossy);
  Ok(out)
}

/// Why a line page ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
  Eof,
  EndLine,
  Budget,
}

fn read_lines(p: &Params, file: &mut File, path: &Path, max_bytes: usize) -> Result<Value> {
  let start = p.uint("startLine", 1, 1, u64::MAX)?;
  let end = p.opt_uint("endLine", 1, u64::MAX)?;
  if end.is_some_and(|e| e < start) {
    return Err(ToolError::invalid(
      "INVALID_RANGE",
      format!("endLine must be >= startLine ({start})"),
    ));
  }
  let mut reader = BufReader::with_capacity(1 << 16, file);
  let mut line = Vec::new();
  let (mut number, mut offset) = (0u64, 0u64);
  let mut content: Vec<u8> = Vec::new();
  let (mut first_offset, mut last) = (None, None);
  let mut line_truncated = false;
  let next_line = |reader: &mut BufReader<&mut File>, line: &mut Vec<u8>| {
    line.clear();
    reader
      .read_until(b'\n', line)
      .map_err(|e| ToolError::io("read", path, &e))
  };
  let stop = loop {
    let n = next_line(&mut reader, &mut line)?;
    if n == 0 {
      break Stop::Eof;
    }
    number += 1;
    if number < start {
      offset += n as u64;
      continue;
    }
    if end.is_some_and(|e| number > e) {
      break Stop::EndLine;
    }
    if content.len() + n > max_bytes {
      if !content.is_empty() {
        break Stop::Budget;
      }
      // One line longer than the page: return its first maxBytes.
      let keep = utf8_prefix(&line[..max_bytes]);
      content.extend_from_slice(&line[..keep]);
      first_offset = Some(offset);
      last = Some(number);
      offset += n as u64;
      line_truncated = true;
      break if next_line(&mut reader, &mut line)? > 0 {
        Stop::Budget
      } else {
        Stop::Eof
      };
    }
    first_offset.get_or_insert(offset);
    content.extend_from_slice(&line);
    last = Some(number);
    offset += n as u64;
  };
  let (mut text_content, lossy) = text(&content);
  if p.flag("lineNumbers") && last.is_some() {
    text_content = numbered(&text_content, start);
  }
  let eof = stop == Stop::Eof;
  let next_start = last.map(|l| l + 1).unwrap_or(start);
  Ok(json!({
    "content": text_content,
    "encoding": "utf-8",
    "lossy": lossy,
    "start_line": start,
    "end_line": last,
    "line_truncated": line_truncated,
    "bytes_read": content.len(),
    "offset": first_offset.unwrap_or(offset),
    "next_start_line": if eof { Value::Null } else { json!(next_start) },
    "next_offset": if eof { Value::Null } else { json!(offset) },
    "eof": eof,
    "truncated": stop == Stop::Budget,
  }))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn utf8_prefix_never_splits_a_character() {
    let s = "aé☃😀".as_bytes(); // 1 + 2 + 3 + 4 bytes
    for cut in 0..=s.len() {
      let keep = utf8_prefix(&s[..cut]);
      assert!(std::str::from_utf8(&s[..keep]).is_ok(), "{cut}");
      assert!(cut - keep < 4);
    }
  }

  #[test]
  fn numbering_is_cat_n_style() {
    assert_eq!(numbered("a\nb", 9), "     9\ta\n    10\tb");
  }
}
