//! `webget`: one HTTP(S) GET. Redirects are followed here (not by the client) so every hop can be
//! checked; non-2xx statuses are normal results. A body bigger than `maxBytes` (or any body with
//! `out`) goes to a file, streamed, never truncated.

use super::{Ctx, Params};
use crate::error::{Result, ToolError};
use crate::ops::read::{looks_binary, text};
use crate::output::{out_path, spill_path, OutFile};
use crate::util::base64;
use lmrust::server::ErrorCode;
use serde_json::{json, Map, Value};
use std::error::Error as _;
use std::io::Read;
use std::time::{Duration, Instant};
use url::Url;

pub const DEFAULT_TIMEOUT_MS: u64 = 20_000;
pub const MAX_TIMEOUT_MS: u64 = 300_000;
pub const DEFAULT_MAX_BYTES: u64 = 64 << 10;
pub const MAX_INLINE_BYTES: u64 = 1 << 20;
pub const MAX_REDIRECTS: u32 = 10;
/// `format: text` converts the whole body in memory: bodies above this are refused.
pub const MAX_TEXT_SOURCE: u64 = 16 << 20;
/// Bytes of the body shown as `preview` when the body went to a file.
pub const PREVIEW_BYTES: usize = 2048;
/// Response headers reported (others, and every request header, are never echoed).
const REPORTED_HEADERS: [&str; 7] = [
  "content-type",
  "content-length",
  "content-encoding",
  "last-modified",
  "etag",
  "cache-control",
  "location",
];

/// The URL without credentials (for results and messages).
pub fn public_url(url: &Url) -> String {
  let mut clean = url.clone();
  let _ = clean.set_username("");
  let _ = clean.set_password(None);
  clean.to_string()
}

fn network_error(url: &Url, t: &ureq::Transport) -> ToolError {
  let detail = t.message().unwrap_or("");
  let timed_out = detail.contains("timed out")
    || t
      .source()
      .and_then(|s| s.downcast_ref::<std::io::Error>())
      .is_some_and(|e| {
        matches!(
          e.kind(),
          std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
      });
  if timed_out {
    return ToolError::new(
      ErrorCode::Timeout,
      "TIMEOUT",
      format!("GET {} timed out", public_url(url)),
    );
  }
  ToolError::new(
    ErrorCode::ServiceUnavailable,
    "NETWORK_ERROR",
    format!(
      "GET {} failed: {}{}{}",
      public_url(url),
      t.kind(),
      if detail.is_empty() { "" } else { ": " },
      detail
    ),
  )
}

fn body_error(url: &Url, e: &std::io::Error) -> ToolError {
  if matches!(
    e.kind(),
    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
  ) {
    ToolError::new(
      ErrorCode::Timeout,
      "TIMEOUT",
      format!("reading {} timed out", public_url(url)),
    )
  } else {
    ToolError::new(
      ErrorCode::ServiceUnavailable,
      "NETWORK_ERROR",
      format!("reading {} failed: {e}", public_url(url)),
    )
  }
}

/// Content types read as text (everything else is binary unless it has no NUL bytes and no type).
fn texty(content_type: &str) -> bool {
  let ct = content_type.to_ascii_lowercase();
  ct.is_empty()
    || ct.starts_with("text/")
    || [
      "json",
      "xml",
      "javascript",
      "ecmascript",
      "x-www-form-urlencoded",
      "yaml",
      "csv",
      "toml",
    ]
    .iter()
    .any(|k| ct.contains(k))
}

/// The body as a JSON value: text, or the base64 marker for binary bytes.
fn body_value(bytes: &[u8], binary: bool) -> (Value, bool) {
  if binary {
    (json!({ "$blob_base64": base64(bytes) }), false)
  } else {
    let (t, lossy) = text(bytes);
    (json!(t), lossy)
  }
}

/// The host check every hop goes through (the permission gate; see `crate::permissions`).
pub type HostCheck<'a> = &'a dyn Fn(&Url) -> Result<()>;

pub fn run(
  p: &Params,
  ctx: &Ctx,
  check_host: HostCheck,
  spill_root: Option<&std::path::Path>,
) -> Result<Value> {
  let raw = p
    .str("url")
    .ok_or_else(|| ToolError::invalid("INVALID_URL", "url is required"))?;
  let mut url = Url::parse(raw)
    .map_err(|e| ToolError::invalid("INVALID_URL", format!("url is not valid: {e}")))?;
  if !matches!(url.scheme(), "http" | "https") {
    return Err(ToolError::invalid(
      "INVALID_URL",
      "webget fetches http:// and https:// URLs only",
    ));
  }
  let timeout =
    Duration::from_millis(p.uint("timeoutMs", DEFAULT_TIMEOUT_MS, 1, MAX_TIMEOUT_MS)?);
  let max_bytes = p.uint("maxBytes", DEFAULT_MAX_BYTES, 1, MAX_INLINE_BYTES)? as usize;
  let offset = p.uint("offset", 0, 0, u64::MAX)?;
  let follow = !p.has("followRedirects") || p.flag("followRedirects");
  let as_text = match p.str("format").unwrap_or("raw") {
    "raw" => false,
    "text" => true,
    other => {
      return Err(ToolError::invalid(
        "INVALID_FORMAT",
        format!("format {other:?} is not raw or text"),
      ))
    }
  };
  let mut headers: Vec<(String, String)> = Vec::new();
  if let Some(map) = p.0.get("headers").filter(|v| !v.is_null()) {
    let map = map.as_object().ok_or_else(|| {
      ToolError::invalid("INVALID_HEADERS", "headers must be an object of strings")
    })?;
    for (k, v) in map {
      let v = v.as_str().ok_or_else(|| {
        ToolError::invalid("INVALID_HEADERS", format!("header {k} must be a string"))
      })?;
      headers.push((k.clone(), v.to_string()));
    }
  }
  let user_agent = p
    .str("userAgent")
    .map(str::to_string)
    .unwrap_or_else(|| format!("lmtools/{}", env!("CARGO_PKG_VERSION")));
  let out = p
    .str("out")
    .map(|o| out_path(o, !ctx.dry_run))
    .transpose()?;

  let agent = ureq::AgentBuilder::new()
    .redirects(0)
    .user_agent(&user_agent)
    .try_proxy_from_env(true)
    .build();
  let deadline = Instant::now() + timeout;
  let mut redirects = 0;
  let response = loop {
    check_host(&url)?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
      return Err(ToolError::new(
        ErrorCode::Timeout,
        "TIMEOUT",
        format!("GET {} timed out", public_url(&url)),
      ));
    }
    let mut request = agent.get(url.as_str()).timeout(remaining);
    for (k, v) in &headers {
      request = request.set(k, v);
    }
    let response = match request.call() {
      Ok(r) | Err(ureq::Error::Status(_, r)) => r,
      Err(ureq::Error::Transport(t)) => return Err(network_error(&url, &t)),
    };
    let location = response.header("location").map(str::to_string);
    match location {
      Some(location) if follow && (300..400).contains(&response.status()) => {
        redirects += 1;
        if redirects > MAX_REDIRECTS {
          return Err(ToolError::new(
            ErrorCode::ServiceUnavailable,
            "TOO_MANY_REDIRECTS",
            format!(
              "more than {MAX_REDIRECTS} redirects from {}",
              public_url(&url)
            ),
          ));
        }
        url = url
          .join(&location)
          .map_err(|e| ToolError::invalid("INVALID_URL", format!("bad redirect location: {e}")))?;
        if !matches!(url.scheme(), "http" | "https") {
          return Err(ToolError::invalid(
            "INVALID_URL",
            "a redirect left http/https",
          ));
        }
      }
      _ => break response,
    }
  };

  let status = response.status();
  let content_type = response.header("content-type").unwrap_or("").to_string();
  let mut reported = Map::new();
  for name in REPORTED_HEADERS {
    if let Some(v) = response.header(name) {
      reported.insert(name.to_string(), json!(v));
    }
  }
  let mut reader: Box<dyn Read + Send> = response.into_reader();
  if offset > 0 {
    std::io::copy(&mut reader.by_ref().take(offset), &mut std::io::sink())
      .map_err(|e| body_error(&url, &e))?;
  }
  if as_text {
    // The whole body is converted in memory (bounded), then delivered like any other body.
    let mut all = Vec::new();
    reader
      .by_ref()
      .take(MAX_TEXT_SOURCE + 1)
      .read_to_end(&mut all)
      .map_err(|e| body_error(&url, &e))?;
    if all.len() as u64 > MAX_TEXT_SOURCE {
      return Err(ToolError::invalid(
        "TOO_LARGE",
        "format text converts at most 16 MiB; fetch it raw with --out instead",
      ));
    }
    let (source, _) = text(&all);
    let converted = if content_type.to_ascii_lowercase().contains("html")
      || source.trim_start().starts_with('<')
    {
      crate::html::to_text(&source)
    } else {
      source
    };
    reader = Box::new(std::io::Cursor::new(converted.into_bytes()));
  }

  let mut head = Vec::new();
  reader
    .by_ref()
    .take(max_bytes as u64 + 1)
    .read_to_end(&mut head)
    .map_err(|e| body_error(&url, &e))?;
  let binary = !as_text && (!texty(&content_type) || looks_binary(&head));
  let mut out_doc = json!({
    "status": status,
    "ok": (200..300).contains(&status),
    "final_url": public_url(&url),
    "redirects": redirects,
    "content_type": content_type,
    "headers": reported,
    "offset": offset,
    "format": if as_text { "text" } else { "raw" },
    "truncated": false,
  });
  if out.is_none() && head.len() <= max_bytes {
    let (body, lossy) = body_value(&head, binary);
    out_doc["body"] = body;
    out_doc["bytes"] = json!(head.len());
    out_doc["lossy"] = json!(lossy);
    out_doc["spilled"] = json!(false);
    return Ok(out_doc);
  }
  // The body goes to a file: `out`, or a spill.
  let spilled = out.is_none();
  let preview_len = head.len().min(PREVIEW_BYTES).min(max_bytes);
  let preview_end = if binary {
    preview_len
  } else {
    crate::ops::read::utf8_prefix(&head[..preview_len])
  };
  out_doc["preview"] = body_value(&head[..preview_end], binary).0;
  out_doc["spilled"] = json!(spilled);
  let ext = if binary { "bin" } else { "txt" };
  if ctx.dry_run {
    // The GET is a read and ran; nothing is written.
    let total = head.len() as u64
      + std::io::copy(&mut reader, &mut std::io::sink()).map_err(|e| body_error(&url, &e))?;
    out_doc["bytes"] = json!(total);
    out_doc["file"] = Value::Null;
    out_doc["dry_run"] = json!(true);
    match &out {
      Some(path) => {
        out_doc["would_write"] = json!({"path": path.display().to_string(), "format": ext})
      }
      None => {
        out_doc["note"] =
          json!("dry run: the body would have spilled to a file; nothing was written")
      }
    }
    return Ok(out_doc);
  }
  let path = match out {
    Some(path) => path,
    None => spill_path(spill_root, ext)?,
  };
  let mut file = OutFile::create(&path)?;
  file.write(&head)?;
  let mut buf = vec![0u8; 1 << 16];
  loop {
    let n = reader.read(&mut buf).map_err(|e| body_error(&url, &e))?;
    if n == 0 {
      break;
    }
    file.write(&buf[..n])?;
  }
  let bytes = file.bytes;
  let overwrote = file.commit()?;
  out_doc["bytes"] = json!(bytes);
  out_doc["file"] = json!({"path": path.display().to_string(), "bytes": bytes, "format": ext});
  out_doc["overwrote"] = json!(overwrote);
  Ok(out_doc)
}
