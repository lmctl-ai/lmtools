//! Small shared helpers: paths, RFC 3339 times, base64, simple globs, shell quoting.

use crate::error::{Result, ToolError};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// `path` made absolute against the process working directory (no symlinks resolved).
pub fn absolute(path: &str) -> Result<PathBuf> {
  if path.is_empty() {
    return Err(ToolError::invalid("INVALID_PATH", "path must not be empty"));
  }
  std::path::absolute(path).map_err(|e| ToolError::io("resolve", Path::new(path), &e))
}

/// POSIX shell single-quoting.
pub fn shell_quote(text: &str) -> String {
  format!("'{}'", text.replace('\'', "'\\''"))
}

/// Days since 1970-01-01 -> (year, month, day) (H. Hinnant's civil_from_days).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
  let z = z + 719_468;
  let era = z.div_euclid(146_097);
  let doe = z.rem_euclid(146_097);
  let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
  let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
  let mp = (5 * doy + 2) / 153;
  let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
  let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
  (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Seconds since the epoch -> (y, m, d, hh, mm, ss), UTC.
pub fn utc_parts(secs: i64) -> (i64, u32, u32, i64, i64, i64) {
  let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
  let (y, m, d) = civil_from_days(days);
  (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// RFC 3339 UTC with second precision (`2026-10-11T09:30:00Z`).
pub fn rfc3339(time: SystemTime) -> String {
  let secs = match time.duration_since(SystemTime::UNIX_EPOCH) {
    Ok(d) => d.as_secs() as i64,
    Err(e) => -(e.duration().as_secs() as i64),
  };
  let (y, m, d, hh, mm, ss) = utc_parts(secs);
  format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard padded base64 (RFC 4648 section 4).
pub fn base64(bytes: &[u8]) -> String {
  let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
  for chunk in bytes.chunks(3) {
    let b = [
      chunk[0],
      *chunk.get(1).unwrap_or(&0),
      *chunk.get(2).unwrap_or(&0),
    ];
    let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
    for i in 0..4 {
      if i <= chunk.len() {
        out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
      } else {
        out.push('=');
      }
    }
  }
  out
}

/// Decode standard base64 (padding optional; whitespace ignored); `None` if malformed.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
  let mut out = Vec::with_capacity(text.len() / 4 * 3);
  let (mut acc, mut bits) = (0u32, 0u32);
  let mut padding = false;
  for c in text.bytes().filter(|c| !c.is_ascii_whitespace()) {
    if c == b'=' {
      padding = true;
      continue;
    }
    if padding {
      return None;
    }
    let v = ALPHABET.iter().position(|&a| a == c)? as u32;
    acc = (acc << 6) | v;
    bits += 6;
    if bits >= 8 {
      bits -= 8;
      out.push((acc >> bits) as u8);
      acc &= (1 << bits) - 1;
    }
  }
  Some(out)
}

/// A simple glob as an anchored regex: `*` (no `/`), `?`, `[...]` / `[!...]`, and `**` (any
/// number of path segments).
pub fn glob_regex(glob: &str) -> Result<regex::Regex> {
  let mut re = String::from("^");
  let chars: Vec<char> = glob.chars().collect();
  let mut i = 0;
  while i < chars.len() {
    match chars[i] {
      '*' if chars.get(i + 1) == Some(&'*') => {
        if chars.get(i + 2) == Some(&'/') {
          re.push_str("(?:.*/)?");
          i += 3;
        } else {
          re.push_str(".*");
          i += 2;
        }
        continue;
      }
      '*' => re.push_str("[^/]*"),
      '?' => re.push_str("[^/]"),
      '[' => {
        let end = chars[i + 1..]
          .iter()
          .position(|&c| c == ']')
          .map(|p| p + i + 1);
        match end {
          Some(end) if end > i + 1 => {
            re.push('[');
            let mut j = i + 1;
            if chars[j] == '!' {
              re.push('^');
              j += 1;
            }
            for &c in &chars[j..end] {
              if c == '\\' || c == '[' {
                re.push('\\');
              }
              re.push(c);
            }
            re.push(']');
            i = end + 1;
            continue;
          }
          _ => re.push_str("\\["),
        }
      }
      c => re.push_str(&regex::escape(&c.to_string())),
    }
    i += 1;
  }
  re.push('$');
  regex::Regex::new(&re)
    .map_err(|e| ToolError::invalid("INVALID_GLOB", format!("invalid glob {glob:?}: {e}")))
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn base64_round_trip() {
    for bytes in [&b""[..], b"f", b"fo", b"foo", b"foob", b"\x00\xff\x10"] {
      assert_eq!(base64_decode(&base64(bytes)).unwrap(), bytes);
    }
    assert_eq!(base64(b"\x00\xff\x10"), "AP8Q");
    assert_eq!(base64_decode("AP8"), Some(vec![0, 255]));
    assert_eq!(base64_decode("A*"), None);
    assert_eq!(base64_decode("AA==AA"), None);
  }

  #[test]
  fn times_are_rfc3339_utc() {
    assert_eq!(rfc3339(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
    let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(951_782_400); // 2000-02-29
    assert_eq!(rfc3339(t), "2000-02-29T00:00:00Z");
  }

  #[test]
  fn globs() {
    for (glob, text, hit) in [
      ("*.rs", "main.rs", true),
      ("*.rs", "src/main.rs", false),
      ("**/*.rs", "src/main.rs", true),
      ("**/*.rs", "main.rs", true),
      ("src/**", "src/a/b.txt", true),
      ("?.txt", "a.txt", true),
      ("[ab].txt", "c.txt", false),
      ("[!ab].txt", "c.txt", true),
      ("a.b", "aXb", false),
    ] {
      assert_eq!(
        glob_regex(glob).unwrap().is_match(text),
        hit,
        "{glob} {text}"
      );
    }
  }

  #[test]
  fn shell_quoting() {
    assert_eq!(shell_quote("it's"), r"'it'\''s'");
  }
}
