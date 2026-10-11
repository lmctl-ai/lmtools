//! webget against a local HTTP server (std TcpListener, one thread per connection).

mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use support::*;

const BIG: usize = 300_000;

fn respond(
  stream: &mut std::net::TcpStream,
  status: &str,
  headers: &[(&str, String)],
  body: &[u8],
) {
  let mut head = format!(
    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
    body.len()
  );
  for (k, v) in headers {
    head.push_str(&format!("{k}: {v}\r\n"));
  }
  head.push_str("\r\n");
  let _ = stream.write_all(head.as_bytes());
  let _ = stream.write_all(body);
}

/// Start the server; returns its base URL.
fn server() -> String {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(mut stream) = stream else { continue };
      std::thread::spawn(move || {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut request = Vec::new();
        loop {
          let mut line = String::new();
          if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
            break;
          }
          request.push(line.trim_end().to_string());
        }
        let path = request
          .first()
          .and_then(|l| l.split_whitespace().nth(1))
          .unwrap_or("/")
          .to_string();
        let text = |s: &str| ("Content-Type", s.to_string());
        match path.as_str() {
          "/ok" => respond(&mut stream, "200 OK", &[text("text/plain; charset=utf-8"), ("ETag", "\"v1\"".into()), ("Set-Cookie", "s=1".into())], b"hello \xe2\x98\x83\n"),
          "/html" => respond(&mut stream, "200 OK", &[text("text/html")], b"<html><head><style>x{}</style></head><body><h1>Hi &amp; bye</h1><p>one<br>two</p><script>alert(1)</script></body></html>"),
          "/redirect" => respond(&mut stream, "302 Found", &[("Location", "/ok".into())], b""),
          "/relative/start" => respond(&mut stream, "301 Moved", &[("Location", "../ok".into())], b""),
          "/loop" => respond(&mut stream, "302 Found", &[("Location", "/loop".into())], b""),
          "/missing" => respond(&mut stream, "404 Not Found", &[text("text/plain")], b"not here\n"),
          "/big" => respond(&mut stream, "200 OK", &[text("text/plain")], "0123456789\n".repeat(BIG / 11).as_bytes()),
          "/bin" => respond(&mut stream, "200 OK", &[text("application/octet-stream")], b"\x00\x01\x02\xff"),
          "/headers" => {
            let echoed = request[1..].join("\n");
            respond(&mut stream, "200 OK", &[text("text/plain")], echoed.as_bytes())
          }
          "/slow" => {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nab");
            std::thread::sleep(Duration::from_secs(5));
            let _ = stream.write_all(b"cdefghij");
          }
          _ => respond(&mut stream, "500 Oops", &[], b""),
        }
      });
    }
  });
  base
}

fn ok(out: &Out) -> Value {
  assert_eq!(out.code, 0, "{}{}", out.text(), out.stderr);
  out.json()
}

/// Run webget with a private spill directory.
fn get(spill: &std::path::Path, args: &[&str]) -> Out {
  let mut all = vec!["webget"];
  all.extend(args);
  run_call(
    &all,
    &Call {
      env: vec![("LMTOOLS_OUT_DIR", s(spill))],
      ..Call::default()
    },
  )
}

#[test]
fn text_html_binary_and_statuses() {
  let base = server();
  let spill = tempfile::tempdir().unwrap();
  let doc = ok(&get(spill.path(), &[&format!("{base}/ok")]));
  assert_eq!(
    (
      doc["status"].clone(),
      doc["ok"].clone(),
      doc["body"].clone(),
      doc["bytes"].clone(),
      doc["spilled"].clone()
    ),
    (
      json!(200),
      json!(true),
      json!("hello ☃\n"),
      json!(10),
      json!(false)
    )
  );
  assert_eq!(doc["content_type"], "text/plain; charset=utf-8");
  assert_eq!(
    doc["headers"],
    json!({"content-type": "text/plain; charset=utf-8", "content-length": "10", "etag": "\"v1\""})
  );
  // Non-2xx is a normal result.
  let doc = ok(&get(spill.path(), &[&format!("{base}/missing")]));
  assert_eq!(
    (
      doc["status"].clone(),
      doc["ok"].clone(),
      doc["body"].clone()
    ),
    (json!(404), json!(false), json!("not here\n"))
  );
  // Binary bodies use the base64 marker.
  let doc = ok(&get(spill.path(), &[&format!("{base}/bin")]));
  assert_eq!(doc["body"], json!({"$blob_base64": "AAEC/w=="}));
  // HTML to text.
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/html"), "--format", "text"],
  ));
  assert_eq!(doc["body"], "Hi & bye\n\none\ntwo\n");
  // offset skips body bytes.
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/ok"), "--offset", "6"],
  ));
  assert_eq!(doc["body"], "☃\n");
}

#[test]
fn redirects() {
  let base = server();
  let spill = tempfile::tempdir().unwrap();
  for start in ["/redirect", "/relative/start"] {
    let doc = ok(&get(spill.path(), &[&format!("{base}{start}")]));
    assert_eq!(
      (doc["redirects"].clone(), doc["final_url"].clone()),
      (json!(1), json!(format!("{base}/ok"))),
      "{start}"
    );
  }
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/redirect"), "--follow-redirects=false"],
  ));
  assert_eq!(
    (doc["status"].clone(), doc["headers"]["location"].clone()),
    (json!(302), json!("/ok"))
  );
  let out = get(spill.path(), &[&format!("{base}/loop")]);
  assert_eq!(out.tool_code(), "TOO_MANY_REDIRECTS", "{}", out.text());
}

#[test]
fn big_bodies_spill_or_go_to_out_in_full() {
  let base = server();
  let spill = tempfile::tempdir().unwrap();
  let expected = "0123456789\n".repeat(BIG / 11);
  let doc = ok(&get(spill.path(), &[&format!("{base}/big")]));
  assert_eq!(
    (doc["spilled"].clone(), doc["bytes"].clone()),
    (json!(true), json!(expected.len()))
  );
  let path = doc["file"]["path"].as_str().unwrap().to_string();
  assert!(path.starts_with(&s(spill.path())), "{path}");
  assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
  assert_eq!(
    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
    0o600
  );
  assert_eq!(doc["preview"].as_str().unwrap().len(), 2048);
  // Under the inline cap nothing spills.
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/big"), "--max-bytes", "1048576"],
  ));
  assert_eq!(
    (doc["spilled"].clone(), doc["body"].as_str().unwrap().len()),
    (json!(false), expected.len())
  );
  // --out: the full body to that file.
  let dir = tempfile::tempdir().unwrap();
  let target = dir.path().join("sub/page.txt");
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/big"), "--out", &s(&target)],
  ));
  assert_eq!(
    (doc["spilled"].clone(), doc["overwrote"].clone()),
    (json!(false), json!(false))
  );
  assert_eq!(std::fs::read_to_string(&target).unwrap(), expected);
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/ok"), "--out", &s(&target)],
  ));
  assert_eq!(doc["overwrote"], true);
  assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello ☃\n");
  // Dry run: the GET runs, nothing is written.
  let fresh = dir.path().join("dry.txt");
  let doc = ok(&get(
    spill.path(),
    &[&format!("{base}/big"), "--out", &s(&fresh), "--dry-run"],
  ));
  assert_eq!(
    (doc["file"].clone(), doc["would_write"]["path"].clone()),
    (Value::Null, json!(s(&fresh)))
  );
  assert!(!fresh.exists());
  let before = std::fs::read_dir(spill.path()).unwrap().count();
  let doc = ok(&get(spill.path(), &[&format!("{base}/big"), "--dry-run"]));
  assert!(doc["note"].as_str().unwrap().contains("spilled"));
  assert_eq!(std::fs::read_dir(spill.path()).unwrap().count(), before);
}

#[test]
fn errors_timeouts_and_secrets() {
  let base = server();
  let spill = tempfile::tempdir().unwrap();
  let out = get(
    spill.path(),
    &[&format!("{base}/slow"), "--timeout-ms", "700"],
  );
  assert_eq!(out.tool_code(), "TIMEOUT", "{}", out.text());
  assert!(out.elapsed < Duration::from_secs(4), "{:?}", out.elapsed);
  // A closed port.
  let closed = TcpListener::bind("127.0.0.1:0")
    .unwrap()
    .local_addr()
    .unwrap();
  let out = get(spill.path(), &[&format!("http://{closed}/")]);
  assert_eq!(out.tool_code(), "NETWORK_ERROR", "{}", out.text());
  for bad in ["ftp://example.com/x", "not a url"] {
    assert_eq!(
      get(spill.path(), &[bad]).tool_code(),
      "INVALID_URL",
      "{bad}"
    );
  }
  // Request headers are sent but never echoed by lmtools; credentials leave the reported URL.
  let with_auth = base.replace("http://", "http://user:pa55word@");
  let out = get(
    spill.path(),
    &[
      &format!("{with_auth}/ok"),
      "--headers",
      r#"{"Authorization": "Bearer s3cret-token"}"#,
    ],
  );
  let text = out.text();
  assert!(
    !text.contains("s3cret-token") && !text.contains("pa55word"),
    "{text}"
  );
  assert_eq!(out.json()["final_url"], format!("{base}/ok"));
  let doc = ok(&get(
    spill.path(),
    &[
      &format!("{base}/headers"),
      "--headers",
      r#"{"X-Test": "yes"}"#,
      "--user-agent",
      "probe/1",
    ],
  ));
  let echoed = doc["body"].as_str().unwrap().to_ascii_lowercase();
  assert!(
    echoed.contains("x-test: yes") && echoed.contains("user-agent: probe/1"),
    "{echoed}"
  );
  let doc = ok(&get(spill.path(), &[&format!("{base}/headers")]));
  assert!(doc["body"]
    .as_str()
    .unwrap()
    .contains(&format!("lmtools/{}", env!("CARGO_PKG_VERSION"))));
}
