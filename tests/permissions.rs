//! The settings file: path levels (cwd / other), host rules, output.dir, bypass, fail-closed
//! loading, and dry runs that still respect the rules.

mod support;

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use support::*;

/// root/ settings.json, work/ (the working directory: a.txt, link -> ../outside/b.txt),
/// outside/ (b.txt).
struct Fx {
  root: tempfile::TempDir,
}

impl Fx {
  fn new() -> Fx {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("work/sub")).unwrap();
    std::fs::create_dir_all(root.path().join("outside")).unwrap();
    std::fs::write(root.path().join("work/a.txt"), "a old\n").unwrap();
    std::fs::write(root.path().join("outside/b.txt"), "b old\n").unwrap();
    std::os::unix::fs::symlink("../outside/b.txt", root.path().join("work/link.txt")).unwrap();
    Fx { root }
  }
  fn path(&self, rel: &str) -> PathBuf {
    self.root.path().join(rel)
  }
  fn settings(&self, doc: Value) {
    let file = self.path("settings.json");
    std::fs::write(&file, doc.to_string()).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
  }
  fn levels(&self, cwd: &str, other: &str) {
    self.settings(json!({"version": 1, "cwd": cwd, "other": other}));
  }
  fn run(&self, args: &[&str]) -> Out {
    run_call(
      args,
      &Call {
        cwd: Some(&self.path("work")),
        env: vec![
          ("LMTOOLS_CONFIG", s(&self.path("settings.json"))),
          ("LMTOOLS_OUT_DIR", s(&self.path("spill"))),
        ],
        ..Call::default()
      },
    )
  }
}

fn denied(out: &Out) -> Value {
  assert_eq!(
    (out.code, out.tool_code().as_str()),
    (6, "PERMISSION_DENIED"),
    "{}",
    out.text()
  );
  let doc = out.json();
  assert_eq!(doc["error"]["code"], "FORBIDDEN");
  doc
}

#[test]
fn path_levels_matrix() {
  let fx = Fx::new();
  // (cwd, other, args, allowed)
  let cases: Vec<(&str, &str, Vec<&str>, bool)> = vec![
    ("readwrite", "none", vec!["read_file", "a.txt"], true),
    (
      "readwrite",
      "none",
      vec!["read_file", "../outside/b.txt"],
      false,
    ),
    ("readwrite", "none", vec!["read_file", "link.txt"], false), // resolves outside
    (
      "readwrite",
      "none",
      vec!["read_file", "sub/../../outside/b.txt"],
      false,
    ),
    ("readwrite", "none", vec!["list_files"], true),
    ("readwrite", "none", vec!["list_files", ".."], false),
    (
      "readwrite",
      "none",
      vec!["write_file", "new.txt", "--content", "x"],
      true,
    ),
    (
      "readwrite",
      "none",
      vec!["write_file", "../outside/new.txt", "--content", "x"],
      false,
    ),
    (
      "readwrite",
      "none",
      vec![
        "edit_file",
        "a.txt",
        "--pattern",
        "old",
        "--replacement",
        "new",
      ],
      true,
    ),
    (
      "readonly",
      "readonly",
      vec!["read_file", "../outside/b.txt"],
      true,
    ),
    ("readonly", "readonly", vec!["list_files", ".."], true),
    (
      "readonly",
      "readonly",
      vec!["write_file", "new2.txt", "--content", "x"],
      false,
    ),
    (
      "readonly",
      "readonly",
      vec![
        "edit_file",
        "a.txt",
        "--pattern",
        "new",
        "--replacement",
        "x",
      ],
      false,
    ),
    (
      "readonly",
      "readonly",
      vec!["write_file", "../outside/new.txt", "--content", "x"],
      false,
    ),
    (
      "readwrite",
      "readonly",
      vec![
        "edit_file",
        "../outside/b.txt",
        "--pattern",
        "old",
        "--replacement",
        "x",
      ],
      false,
    ),
    (
      "readwrite",
      "readwrite",
      vec![
        "edit_file",
        "../outside/b.txt",
        "--pattern",
        "old",
        "--replacement",
        "new",
      ],
      true,
    ),
  ];
  for (cwd, other, args, allowed) in cases {
    fx.levels(cwd, other);
    let out = fx.run(&args);
    if allowed {
      assert_eq!(out.code, 0, "{cwd}/{other} {args:?}: {}", out.text());
    } else {
      let doc = denied(&out);
      let message = doc["error"]["message"].as_str().unwrap();
      assert!(message.contains(args[1]) || args.len() < 2, "{message}");
      assert!(message.contains(&s(&fx.path("settings.json"))), "{message}");
    }
  }
  assert!(!fx.path("outside/new.txt").exists() && !fx.path("work/new2.txt").exists());
  assert_eq!(
    std::fs::read_to_string(fx.path("outside/b.txt")).unwrap(),
    "b new\n"
  );
}

#[test]
fn denials_carry_the_operator_command() {
  let fx = Fx::new();
  fx.levels("readonly", "none");
  for (args, manual) in [
    (
      vec!["read_file", "../outside/b.txt"],
      "cat '../outside/b.txt'",
    ),
    (
      vec![
        "read_file",
        "../outside/b.txt",
        "--start-line",
        "2",
        "--end-line",
        "4",
      ],
      "sed -n '2,4p' '../outside/b.txt'",
    ),
    (vec!["list_files", "../outside"], "ls -la '../outside'"),
    (
      vec!["write_file", "a.txt", "--content", "it's"],
      r"printf '%s' 'it'\''s' > 'a.txt'",
    ),
    (
      vec!["write_file", "a.txt", "--content", "x", "--mode", "append"],
      "printf '%s' 'x' >> 'a.txt'",
    ),
    (
      vec!["edit_file", "a.txt", "--pattern", "a", "--replacement", "b"],
      "\"${EDITOR:-vi}\" 'a.txt'",
    ),
  ] {
    let doc = denied(&fx.run(&args));
    assert_eq!(doc["error"]["data"]["manual_command"], manual, "{args:?}");
  }
}

#[test]
fn dry_runs_respect_the_rules_and_never_write() {
  let fx = Fx::new();
  fx.levels("readonly", "none");
  denied(&fx.run(&["write_file", "x.txt", "--content", "x", "--dry-run"]));
  denied(&fx.run(&[
    "edit_file",
    "a.txt",
    "--pattern",
    "old",
    "--replacement",
    "new",
    "--dry-run",
  ]));
  fx.levels("readwrite", "none");
  let doc = fx
    .run(&[
      "edit_file",
      "a.txt",
      "--pattern",
      "old",
      "--replacement",
      "new",
      "--dry-run",
    ])
    .json();
  assert_eq!(
    (doc["executed"].clone(), doc["would_change"].clone()),
    (json!(false), json!(true))
  );
  let doc = fx
    .run(&[
      "write_file",
      "deep/x.txt",
      "--content",
      "x",
      "--create-parents",
      "--dry-run",
    ])
    .json();
  assert_eq!(
    (
      doc["would_create"].clone(),
      doc["would_create_parents"].clone()
    ),
    (json!(true), json!(true))
  );
  assert_eq!(
    std::fs::read_to_string(fx.path("work/a.txt")).unwrap(),
    "a old\n"
  );
  assert!(!fx.path("work/deep").exists());
}

#[test]
fn invalid_settings_fail_closed_and_loose_files_are_warned_about() {
  let fx = Fx::new();
  for (text, expect) in [
    ("{not json", "is not valid JSON"),
    (
      r#"{"version": 1, "cwd": "readwrite", "other": "none", "typo": true}"#,
      "unknown key (top level).typo",
    ),
    (r#"{"version": 1, "cwd": "readwrite"}"#, "other is required"),
    (
      r#"{"version": 1, "cwd": "readwrite", "other": "everything"}"#,
      "other must be one of",
    ),
  ] {
    std::fs::write(fx.path("settings.json"), text).unwrap();
    for args in [
      vec!["read_file", "a.txt"],
      vec!["list_files"],
      vec!["webget", "http://127.0.0.1:1/"],
    ] {
      let out = fx.run(&args);
      assert_eq!(
        (out.code, out.tool_code().as_str()),
        (2, "SETTINGS_INVALID"),
        "{text} {args:?}"
      );
      let message = out.json()["error"]["message"].as_str().unwrap().to_string();
      assert!(
        message.contains(expect) && message.contains("settings.json"),
        "{message}"
      );
    }
  }
  // A directory where the file should be: unreadable, also closed.
  std::fs::remove_file(fx.path("settings.json")).unwrap();
  std::fs::create_dir(fx.path("settings.json")).unwrap();
  assert_eq!(
    fx.run(&["read_file", "a.txt"]).tool_code(),
    "SETTINGS_INVALID"
  );
  std::fs::remove_dir(fx.path("settings.json")).unwrap();
  // Missing file: open.
  let out = fx.run(&["read_file", "../outside/b.txt"]);
  assert_eq!(out.code, 0, "{}", out.text());
  // Group/other-writable: a warning, still applied.
  fx.levels("readwrite", "none");
  for (mode, warned) in [(0o600, false), (0o644, false), (0o664, true), (0o666, true)] {
    std::fs::set_permissions(
      fx.path("settings.json"),
      std::fs::Permissions::from_mode(mode),
    )
    .unwrap();
    let out = fx.run(&["read_file", "a.txt"]);
    assert_eq!(out.code, 0);
    assert_eq!(
      out.stderr.contains("writable by group or others"),
      warned,
      "{mode:o}: {}",
      out.stderr
    );
  }
}

#[test]
fn bypass_needs_allow_bypass() {
  let fx = Fx::new();
  const FLAG: &str = "--dangerously-bypass-permission-settings";
  // (allow_bypass, args, exit, tool code, bypass line)
  let cases: Vec<(bool, Vec<&str>, i32, &str, bool)> = vec![
    (
      false,
      vec!["read_file", "../outside/b.txt", FLAG],
      6,
      "BYPASS_NOT_ALLOWED",
      false,
    ),
    (
      false,
      vec!["read_file", "../outside/b.txt"],
      6,
      "PERMISSION_DENIED",
      false,
    ),
    (
      true,
      vec!["read_file", "../outside/b.txt", FLAG],
      0,
      "ok",
      true,
    ),
    (
      true,
      vec!["write_file", "../outside/c.txt", "--content", "c", FLAG],
      0,
      "ok",
      true,
    ),
    (
      true,
      vec!["read_file", "../outside/b.txt"],
      6,
      "PERMISSION_DENIED",
      false,
    ),
  ];
  for (allow, args, exit, code, line) in cases {
    fx.settings(json!({"version": 1, "cwd": "readwrite", "other": "none", "allow_bypass": allow}));
    let out = fx.run(&args);
    assert_eq!(out.code, exit, "{allow} {args:?}: {}", out.text());
    if code != "ok" {
      assert_eq!(out.tool_code(), code);
    }
    if code == "BYPASS_NOT_ALLOWED" {
      assert!(out.text().contains("allow_bypass"));
    }
    assert_eq!(
      out.stderr.contains("permission settings bypassed"),
      line,
      "{}",
      out.stderr
    );
  }
  assert_eq!(
    std::fs::read_to_string(fx.path("outside/c.txt")).unwrap(),
    "c"
  );
  // No settings file: a harmless no-op.
  std::fs::remove_file(fx.path("settings.json")).unwrap();
  let out = fx.run(&["read_file", "a.txt", FLAG]);
  assert_eq!(out.code, 0);
  assert!(out.stderr.is_empty(), "{}", out.stderr);
}

/// A server that answers /ok and redirects /away to `target`.
fn server(target: String) -> String {
  let listener = TcpListener::bind("127.0.0.1:0").unwrap();
  let base = format!("http://{}", listener.local_addr().unwrap());
  std::thread::spawn(move || {
    for stream in listener.incoming() {
      let Ok(mut stream) = stream else { continue };
      let mut reader = BufReader::new(stream.try_clone().unwrap());
      let mut first = String::new();
      let _ = reader.read_line(&mut first);
      loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
          break;
        }
      }
      let reply = if first.contains("/away") {
        format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
      } else {
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 3\r\nConnection: close\r\n\r\nok\n".to_string()
      };
      let _ = stream.write_all(reply.as_bytes());
    }
  });
  base
}

#[test]
fn webget_hosts_and_output_dir() {
  let fx = Fx::new();
  let port = {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
  };
  let base = server(format!("http://localhost:{port}/ok"));
  let host_rules = |webget: Value| {
    fx.settings(json!({"version": 1, "cwd": "readwrite", "other": "none", "webget": webget}))
  };
  // (webget rules, path, allowed)
  let cases: Vec<(Value, &str, bool)> = vec![
    (json!({}), "/ok", true),
    (json!({"allow_hosts": ["127.0.0.*"]}), "/ok", true),
    (json!({"allow_hosts": ["docs.rs"]}), "/ok", false),
    (json!({"deny_hosts": ["127.0.0.1"]}), "/ok", false),
    (
      json!({"allow_hosts": ["127.0.0.1"], "deny_hosts": ["127.*"]}),
      "/ok",
      false,
    ),
    // A redirect to a host that is not allowed is refused at that hop.
    (json!({"allow_hosts": ["127.0.0.1"]}), "/away", false),
  ];
  for (rules, path, allowed) in cases {
    host_rules(rules.clone());
    let out = fx.run(&["webget", &format!("{base}{path}")]);
    if allowed {
      assert_eq!(out.code, 0, "{rules} {path}: {}", out.text());
    } else {
      let doc = denied(&out);
      assert!(
        doc["error"]["data"]["manual_command"]
          .as_str()
          .unwrap()
          .starts_with("curl -L '"),
        "{doc}"
      );
    }
  }
  // output.dir: out must stay inside; spills go there.
  fx.settings(
    json!({"version": 1, "cwd": "readwrite", "other": "readwrite", "output": {"dir": "results"}}),
  );
  let results = fx.path("results");
  let out = fx.run(&[
    "webget",
    &format!("{base}/ok"),
    "--out",
    &s(&results.join("page.txt")),
  ]);
  assert_eq!(out.code, 0, "{}", out.text());
  assert_eq!(
    std::fs::read_to_string(results.join("page.txt")).unwrap(),
    "ok\n"
  );
  for outside in [fx.path("work/page.txt"), results.join("../escape.txt")] {
    let doc = denied(&fx.run(&["webget", &format!("{base}/ok"), "--out", &s(&outside)]));
    assert!(
      doc["error"]["message"]
        .as_str()
        .unwrap()
        .contains("output.dir"),
      "{doc}"
    );
  }
  assert!(!fx.path("escape.txt").exists() && !fx.path("work/page.txt").exists());
  let doc = fx
    .run(&["webget", &format!("{base}/ok"), "--max-bytes", "1"])
    .json();
  assert_eq!(doc["spilled"], true, "{doc}");
  let spilled = PathBuf::from(doc["file"]["path"].as_str().unwrap());
  assert_eq!(spilled.parent().unwrap(), results.as_path());
  assert!(
    !fx.path("spill").exists(),
    "LMTOOLS_OUT_DIR is not used when output.dir is set"
  );
  // Without output.dir an out path follows the path rules.
  fx.levels("readwrite", "none");
  denied(&fx.run(&[
    "webget",
    &format!("{base}/ok"),
    "--out",
    &s(&fx.path("outside/page.txt")),
  ]));
}
