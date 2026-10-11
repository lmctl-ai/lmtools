//! list_files, read_file, write_file and edit_file through the binary, on temp-dir fixtures.

mod support;

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use support::*;

fn ok(out: &Out) -> Value {
  assert_eq!(out.code, 0, "{}{}", out.text(), out.stderr);
  out.json()
}

fn write(dir: &Path, name: &str, bytes: &[u8]) {
  let path = dir.join(name);
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent).unwrap();
  }
  std::fs::write(path, bytes).unwrap();
}

fn read(dir: &Path, name: &str) -> Vec<u8> {
  std::fs::read(dir.join(name)).unwrap()
}

/// Names in `dir` (to catch leftover temporary files).
fn names(dir: &Path) -> Vec<String> {
  let mut v: Vec<String> = std::fs::read_dir(dir)
    .unwrap()
    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
    .collect();
  v.sort();
  v
}

// ---- list_files --------------------------------------------------------------------------------

#[test]
fn list_files_is_sorted_filtered_and_paged() {
  let d = tempfile::tempdir().unwrap();
  for f in [
    "b.rs",
    "a.txt",
    "src/main.rs",
    "src/lib.rs",
    "src/deep/x.rs",
    ".hidden/y.rs",
    ".env",
  ] {
    write(d.path(), f, b"x");
  }
  std::os::unix::fs::symlink(d.path().join("src"), d.path().join("link")).unwrap();
  let paths = |out: &Out| -> Vec<String> {
    out.json()["entries"]
      .as_array()
      .unwrap()
      .iter()
      .map(|e| e["path"].as_str().unwrap().to_string())
      .collect()
  };
  // (args, expected paths)
  let cases: Vec<(Vec<&str>, Vec<&str>)> = vec![
    (vec!["list_files"], vec!["a.txt", "b.rs", "link", "src"]),
    (
      vec!["list_files", "--recursive"],
      vec![
        "a.txt",
        "b.rs",
        "link",
        "src",
        "src/deep",
        "src/deep/x.rs",
        "src/lib.rs",
        "src/main.rs",
      ],
    ),
    (
      vec!["list_files", "--recursive", "--max-depth", "2"],
      vec![
        "a.txt",
        "b.rs",
        "link",
        "src",
        "src/deep",
        "src/lib.rs",
        "src/main.rs",
      ],
    ),
    (
      vec!["list_files", "--recursive", "--glob", "*.rs"],
      vec!["b.rs", "src/deep/x.rs", "src/lib.rs", "src/main.rs"],
    ),
    (
      vec!["list_files", "--recursive", "--glob", "src/*.rs"],
      vec!["src/lib.rs", "src/main.rs"],
    ),
    (
      vec!["list_files", "--include-hidden"],
      vec![".env", ".hidden", "a.txt", "b.rs", "link", "src"],
    ),
    (vec!["list_files", "src"], vec!["deep", "lib.rs", "main.rs"]),
  ];
  for (args, expected) in cases {
    let out = run_in(d.path(), &args);
    assert_eq!(out.code, 0, "{args:?}: {}", out.text());
    assert_eq!(paths(&out), expected, "{args:?}");
  }
  // Types, sizes, times.
  let doc = run_in(d.path(), &["list_files"]).json();
  let link = &doc["entries"][2];
  assert_eq!(
    (link["type"].clone(), link["size"].clone()),
    (json!("symlink"), Value::Null)
  );
  assert_eq!(doc["entries"][0]["size"], 1);
  assert!(doc["entries"][0]["mtime"].as_str().unwrap().ends_with('Z'));
  // Paging: exact truncated / next_offset.
  let mut seen = Vec::new();
  let mut offset = 0;
  loop {
    let doc = run_in(
      d.path(),
      &[
        "list_files",
        "--recursive",
        "--limit",
        "3",
        "--offset",
        &offset.to_string(),
      ],
    )
    .json();
    seen.extend(
      doc["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["path"].as_str().unwrap().to_string()),
    );
    if doc["next_offset"].is_null() {
      assert_eq!(doc["truncated"], false);
      break;
    }
    assert_eq!(doc["truncated"], true);
    offset = doc["next_offset"].as_u64().unwrap();
  }
  assert_eq!(seen.len(), 8);
  // Errors.
  assert_eq!(
    run_in(d.path(), &["list_files", "a.txt"]).tool_code(),
    "NOT_A_DIRECTORY"
  );
  assert_eq!(
    run_in(d.path(), &["list_files", "nope"]).tool_code(),
    "NOT_FOUND"
  );
  assert_eq!(
    run_in(d.path(), &["list_files", "--limit", "5001"]).tool_code(),
    "LIMIT_OUT_OF_RANGE"
  );
}

// ---- read_file ---------------------------------------------------------------------------------

#[test]
fn read_file_text_shapes() {
  let d = tempfile::tempdir().unwrap();
  // (file bytes, args after the path, expected content, total_lines, eof)
  type Case<'a> = (&'a [u8], Vec<&'a str>, &'a str, u64, bool);
  let cases: Vec<Case> = vec![
    (b"", vec![], "", 0, true),
    (b"no newline", vec![], "no newline", 1, true),
    (b"a\r\nb\r\n", vec![], "a\r\nb\r\n", 2, true),
    ("é ☃ 😀\n".as_bytes(), vec![], "é ☃ 😀\n", 1, true),
    (
      b"1\n2\n3\n4\n",
      vec!["--start-line", "2", "--end-line", "3"],
      "2\n3\n",
      4,
      false,
    ),
    (
      b"1\n2\n3\n4\n",
      vec!["--start-line", "3", "--end-line", "4"],
      "3\n4\n",
      4,
      true,
    ),
    (b"1\n2\n3", vec!["--start-line", "3"], "3", 3, true),
    (
      b"1\n2\n",
      vec!["--line-numbers"],
      "     1\t1\n     2\t2\n",
      2,
      true,
    ),
    (
      b"1\n2\n3\n",
      vec!["--start-line", "2", "--line-numbers"],
      "     2\t2\n     3\t3\n",
      3,
      true,
    ),
    (
      b"1\n2\n3\n",
      vec!["--offset", "2", "--line-numbers"],
      "     2\t2\n     3\t3\n",
      3,
      true,
    ),
  ];
  for (i, (bytes, args, content, lines, eof)) in cases.into_iter().enumerate() {
    let name = format!("f{i}");
    write(d.path(), &name, bytes);
    let mut all = vec!["read_file", name.as_str()];
    all.extend(args.iter());
    let doc = ok(&run_in(d.path(), &all));
    assert_eq!(doc["content"], content, "{args:?}");
    assert_eq!(doc["total_lines"], lines, "{args:?}");
    assert_eq!(doc["eof"], eof, "{args:?}");
    assert_eq!(doc["lossy"], false);
  }
  // Invalid UTF-8: replaced, flagged.
  write(d.path(), "bad", b"ok \xff\xfe end");
  let doc = ok(&run_in(d.path(), &["read_file", "bad"]));
  assert_eq!(
    (doc["content"].clone(), doc["lossy"].clone()),
    (json!("ok \u{fffd}\u{fffd} end"), json!(true))
  );
}

#[test]
fn read_file_pages_a_big_file_without_splitting_characters() {
  let d = tempfile::tempdir().unwrap();
  let line = "αβγ 0123456789 ☃\n"; // multi-byte characters everywhere
  let text = line.repeat(20_000); // ~430 KB
  write(d.path(), "big.txt", text.as_bytes());
  let mut rebuilt = String::new();
  let mut offset = 0u64;
  let mut pages = 0;
  loop {
    let doc = ok(&run_in(
      d.path(),
      &[
        "read_file",
        "big.txt",
        "--offset",
        &offset.to_string(),
        "--max-bytes",
        "65537",
      ],
    ));
    assert_eq!(doc["lossy"], false, "page {pages} split a character");
    rebuilt.push_str(doc["content"].as_str().unwrap());
    pages += 1;
    if doc["eof"] == true {
      assert!(doc["next_offset"].is_null());
      break;
    }
    assert_eq!(doc["truncated"], true);
    offset = doc["next_offset"].as_u64().unwrap();
  }
  assert_eq!(rebuilt, text);
  assert!(pages >= 7);
  // Line pages, capped by maxBytes.
  let doc = ok(&run_in(
    d.path(),
    &[
      "read_file",
      "big.txt",
      "--start-line",
      "100",
      "--max-bytes",
      "1000",
    ],
  ));
  assert_eq!(doc["end_line"], 100 + 1000 / line.len() as u64 - 1);
  assert_eq!(doc["truncated"], true);
  assert_eq!(
    doc["next_start_line"],
    doc["end_line"].as_u64().unwrap() + 1
  );
  assert_eq!(doc["total_lines"], 20_000);
}

#[test]
fn read_file_refuses_binary_and_non_regular_files() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "img.bin", b"\x89PNG\x00\x01\x02");
  let out = run_in(d.path(), &["read_file", "img.bin"]);
  assert_eq!((out.code, out.tool_code().as_str()), (2, "BINARY_FILE"));
  let doc = ok(&run_in(d.path(), &["read_file", "img.bin", "--as-base64"]));
  assert_eq!(
    (doc["content_base64"].clone(), doc["encoding"].clone()),
    (json!("iVBORwABAg=="), json!("base64"))
  );
  std::fs::create_dir(d.path().join("dir")).unwrap();
  assert_eq!(
    run_in(d.path(), &["read_file", "dir"]).tool_code(),
    "NOT_A_REGULAR_FILE"
  );
  assert_eq!(
    run_in(d.path(), &["read_file", "missing"]).tool_code(),
    "NOT_FOUND"
  );
  // A symlink to a file is read through.
  write(d.path(), "t.txt", b"target\n");
  std::os::unix::fs::symlink("t.txt", d.path().join("l.txt")).unwrap();
  assert_eq!(
    ok(&run_in(d.path(), &["read_file", "l.txt"]))["content"],
    "target\n"
  );
}

// ---- write_file --------------------------------------------------------------------------------

#[test]
fn write_file_modes_and_targets() {
  let d = tempfile::tempdir().unwrap();
  let p = d.path();
  // (args, expected exit, tool code or "ok", file content afterwards)
  type Case<'a> = (Vec<&'a str>, i32, &'a str, Option<&'a [u8]>);
  let cases: Vec<Case> = vec![
    (
      vec!["write_file", "a.txt", "--content", "one"],
      0,
      "ok",
      Some(b"one"),
    ),
    (
      vec!["write_file", "a.txt", "--content", "two"],
      0,
      "ok",
      Some(b"two"),
    ),
    (
      vec!["write_file", "a.txt", "--content", "+", "--mode", "append"],
      0,
      "ok",
      Some(b"two+"),
    ),
    (
      vec!["write_file", "a.txt", "--content", "x", "--mode", "create"],
      5,
      "ALREADY_EXISTS",
      Some(b"two+"),
    ),
    (
      vec!["write_file", "b.bin", "--content-base64", "AP8Q"],
      0,
      "ok",
      Some(b"\x00\xff\x10"),
    ),
    (
      vec!["write_file", "b.bin", "--content-base64", "!!"],
      2,
      "INVALID_BASE64",
      Some(b"\x00\xff\x10"),
    ),
    (
      vec!["write_file", "new/deep/c.txt", "--content", "c"],
      4,
      "NOT_FOUND",
      None,
    ),
    (
      vec![
        "write_file",
        "new/deep/c.txt",
        "--content",
        "c",
        "--create-parents",
      ],
      0,
      "ok",
      Some(b"c"),
    ),
    (
      vec!["write_file", "e.txt", "--content", ""],
      0,
      "ok",
      Some(b""),
    ),
    (vec!["write_file", "f.txt"], 2, "INVALID_PARAMS", None),
    (
      vec![
        "write_file",
        "f.txt",
        "--content",
        "a",
        "--content-base64",
        "YQ==",
      ],
      2,
      "INVALID_PARAMS",
      None,
    ),
  ];
  for (args, exit, expect, after) in cases {
    let out = run_in(p, &args);
    assert_eq!(out.code, exit, "{args:?}: {}", out.text());
    if expect != "ok" {
      assert_eq!(out.tool_code(), expect, "{args:?}");
    }
    match after {
      Some(bytes) => assert_eq!(read(p, args[1]), bytes, "{args:?}"),
      None => assert!(!p.join(args[1]).exists(), "{args:?}"),
    }
  }
  let doc = ok(&run_in(p, &["write_file", "a.txt", "--content", "three"]));
  assert_eq!(
    (
      doc["bytes_written"].clone(),
      doc["created"].clone(),
      doc["overwrote"].clone()
    ),
    (json!(5), json!(false), json!(true))
  );
  // Content from stdin.
  let out = run_call(
    &["write_file", "s.txt", "--stdin"],
    &Call {
      cwd: Some(p),
      stdin: Some("from stdin ☃\n".as_bytes()),
      ..Call::default()
    },
  );
  assert_eq!(ok(&out)["bytes_written"], 15);
  assert_eq!(read(p, "s.txt"), "from stdin ☃\n".as_bytes());
  // Refused targets.
  std::os::unix::fs::symlink("a.txt", p.join("link.txt")).unwrap();
  std::fs::create_dir(p.join("dir")).unwrap();
  std::fs::hard_link(p.join("e.txt"), p.join("e2.txt")).unwrap();
  for (target, code) in [
    ("link.txt", "SYMLINK_TARGET"),
    ("dir", "NOT_A_REGULAR_FILE"),
    ("e.txt", "HARDLINKED_FILE"),
  ] {
    let out = run_in(p, &["write_file", target, "--content", "x"]);
    assert_eq!(out.tool_code(), code, "{target}: {}", out.text());
  }
  assert_eq!(read(p, "a.txt"), b"three");
  assert!(
    names(p).iter().all(|n| !n.contains(".lmtools-")),
    "{:?}",
    names(p)
  );
}

#[test]
fn write_file_keeps_the_mode_of_an_existing_file() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "run.sh", b"#!/bin/sh\n");
  std::fs::set_permissions(
    d.path().join("run.sh"),
    std::fs::Permissions::from_mode(0o751),
  )
  .unwrap();
  ok(&run_in(
    d.path(),
    &["write_file", "run.sh", "--content", "#!/bin/sh\necho hi\n"],
  ));
  let mode = std::fs::metadata(d.path().join("run.sh"))
    .unwrap()
    .permissions()
    .mode()
    & 0o7777;
  assert_eq!(mode, 0o751);
}

/// A write that fails half-way (the file size limit is hit) leaves the original file as it was
/// and no temporary file behind.
#[test]
fn a_write_that_fails_midway_leaves_the_original() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "keep.txt", b"original\n");
  let input = tempfile::tempdir().unwrap();
  std::fs::write(input.path().join("big"), "x".repeat(200_000)).unwrap();
  // 64 blocks (32-64 KiB) of file size allowed; SIGXFSZ ignored so the write fails with EFBIG.
  let script = format!(
    "ulimit -f 64; trap '' XFSZ; exec {} write_file keep.txt --stdin < {}",
    s(&bin()),
    s(&input.path().join("big"))
  );
  let status = std::process::Command::new("sh")
    .arg("-c")
    .arg(&script)
    .current_dir(d.path())
    .env("LMTOOLS_CONFIG", "/nonexistent/lmtools-test/settings.json")
    .output()
    .unwrap();
  assert_ne!(
    status.status.code(),
    Some(0),
    "{}",
    String::from_utf8_lossy(&status.stdout)
  );
  let doc: Value = serde_json::from_slice(&status.stdout).unwrap();
  assert_eq!(doc["error"]["data"]["tool_code"], "IO_ERROR", "{doc}");
  assert_eq!(read(d.path(), "keep.txt"), b"original\n");
  assert_eq!(names(d.path()), ["keep.txt"]);
}

// ---- edit_file (lmprobe's edit tests, as single-file calls) ------------------------------------

#[test]
fn literal_whole_word_preview_and_apply_preserve_dollars_and_permissions() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "notes.txt", b"cost $old oldish old\n");
  std::fs::set_permissions(
    d.path().join("notes.txt"),
    std::fs::Permissions::from_mode(0o640),
  )
  .unwrap();
  let preview = ok(&run_in(
    d.path(),
    &[
      "edit_file",
      "notes.txt",
      "--pattern",
      "old",
      "--replacement",
      "$new",
      "--word",
      "--dry-run",
    ],
  ));
  assert_eq!(
    (
      preview["dry_run"].clone(),
      preview["executed"].clone(),
      preview["would_change"].clone(),
      preview["replacements"].clone()
    ),
    (json!(true), json!(false), json!(true), json!(2))
  );
  assert!(
    preview["diff"]
      .as_str()
      .unwrap()
      .contains("+cost $$new oldish $new"),
    "{}",
    preview["diff"]
  );
  assert_eq!(
    read(d.path(), "notes.txt"),
    b"cost $old oldish old\n",
    "a dry run wrote"
  );
  let applied = ok(&run_in(
    d.path(),
    &[
      "edit_file",
      "notes.txt",
      "--pattern",
      "old",
      "--replacement",
      "$new",
      "--word",
    ],
  ));
  assert_eq!(
    (applied["changed"].clone(), applied["replacements"].clone()),
    (json!(true), json!(2))
  );
  assert_eq!(read(d.path(), "notes.txt"), b"cost $$new oldish $new\n");
  let mode = std::fs::metadata(d.path().join("notes.txt"))
    .unwrap()
    .permissions()
    .mode()
    & 0o777;
  assert_eq!(mode, 0o640);
}

#[test]
fn regex_capture_multiline_and_first() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "r.txt", b"old1\nold2\nkeep\n");
  ok(&run_in(
    d.path(),
    &[
      "edit_file",
      "r.txt",
      "--pattern",
      "(?m)^old([0-9])$",
      "--replacement",
      "new-$1",
      "--regex",
      "--first",
    ],
  ));
  assert_eq!(read(d.path(), "r.txt"), b"new-1\nold2\nkeep\n");
  ok(&run_in(
    d.path(),
    &[
      "edit_file",
      "r.txt",
      "--pattern",
      "OLD",
      "--replacement",
      "x",
      "--ignore-case",
    ],
  ));
  assert_eq!(read(d.path(), "r.txt"), b"new-1\nx2\nkeep\n");
}

#[test]
fn scripts_sequence_line_operations_and_preserve_crlf() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "c.txt", b"one\r\ntwo\r\nthree\r\n");
  let script = r#"[{"op":"insert_after","line":1,"text":"middle"},{"op":"replace_lines","start":3,"end":3,"text":"TWO"},{"op":"replace","pattern":"three","replacement":"3"}]"#;
  let doc = ok(&run_in(
    d.path(),
    &["edit_file", "c.txt", "--script", script],
  ));
  assert_eq!(doc["replacements"], 1);
  assert_eq!(read(d.path(), "c.txt"), b"one\r\nmiddle\r\nTWO\r\n3\r\n");
  // The script can come from stdin.
  let out = run_call(
    &["edit_file", "c.txt", "--script", "-"],
    &Call {
      cwd: Some(d.path()),
      stdin: Some(br#"[{"op":"append","text":"end"}]"#),
      ..Call::default()
    },
  );
  ok(&out);
  assert_eq!(read(d.path(), "c.txt"), b"one\r\nmiddle\r\nTWO\r\n3\r\nend");
}

#[test]
fn line_edits() {
  // (start text, args, result)
  let cases: Vec<(&str, Vec<&str>, &str)> = vec![
    (
      "a\nb\n",
      vec!["--insert-before", "1", "--text", "h"],
      "h\na\nb\n",
    ),
    (
      "a\nb",
      vec!["--insert-after", "2", "--text", "c"],
      "a\nb\nc\n",
    ),
    ("a\nb\nc\n", vec!["--delete-lines", "2"], "a\nc\n"),
    ("a\nb\nc\n", vec!["--delete-lines", "1:2"], "c\n"),
    (
      "a\nb\nc\n",
      vec!["--replace-lines", "2:3", "--text", "x\ny"],
      "a\nx\ny\n",
    ),
    ("body", vec!["--prepend", "--text", "<"], "<body"),
    ("body", vec!["--append", "--text", ">\n"], "body>\n"),
    ("", vec!["--append", "--text", "first"], "first"),
  ];
  for (start, args, result) in cases {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), "f", start.as_bytes());
    let mut all = vec!["edit_file", "f"];
    all.extend(args.iter());
    ok(&run_in(d.path(), &all));
    assert_eq!(
      String::from_utf8(read(d.path(), "f")).unwrap(),
      result,
      "{args:?}"
    );
  }
}

#[test]
fn invalid_edits_and_count_mismatches_write_nothing() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "f.txt", b"a old b old\nline2\n");
  // (args, exit, tool code)
  let cases: Vec<(Vec<&str>, i32, &str)> = vec![
    (
      vec![
        "--pattern",
        "old",
        "--replacement",
        "new",
        "--expected-count",
        "3",
      ],
      5,
      "EDIT_COUNT_MISMATCH",
    ),
    (
      vec![
        "--pattern",
        "old",
        "--replacement",
        "new",
        "--first",
        "--expected-count",
        "2",
      ],
      5,
      "EDIT_COUNT_MISMATCH",
    ),
    (vec!["--delete-lines", "2:9"], 2, "INVALID_EDIT"),
    (vec!["--delete-lines", "3:2"], 2, "INVALID_EDIT"),
    (vec!["--insert-after", "1"], 2, "INVALID_EDIT"),
    (vec!["--pattern", "old"], 2, "INVALID_EDIT"),
    (
      vec!["--pattern", "[", "--replacement", "x", "--regex"],
      2,
      "INVALID_EDIT",
    ),
    (
      vec![
        "--pattern",
        "a",
        "--replacement",
        "b",
        "--prepend",
        "--text",
        "x",
      ],
      2,
      "INVALID_EDIT",
    ),
    (
      vec!["--script", r#"[{"op":"prepend","text":"x","extra":1}]"#],
      2,
      "INVALID_EDIT",
    ),
    (vec!["--script", "[]"], 2, "INVALID_EDIT"),
    (
      vec!["--delete-lines", "1", "--expected-count", "1"],
      2,
      "INVALID_EDIT",
    ),
    (vec![], 2, "INVALID_EDIT"),
  ];
  for (args, exit, code) in cases {
    let mut all = vec!["edit_file", "f.txt"];
    all.extend(args.iter());
    let out = run_in(d.path(), &all);
    assert_eq!(
      (out.code, out.tool_code()),
      (exit, code.to_string()),
      "{args:?}: {}",
      out.text()
    );
  }
  let mismatch = run_in(
    d.path(),
    &[
      "edit_file",
      "f.txt",
      "--pattern",
      "old",
      "--replacement",
      "new",
      "--expected-count",
      "3",
    ],
  )
  .json();
  assert_eq!(mismatch["error"]["data"]["replacements"], 2);
  assert_eq!(read(d.path(), "f.txt"), b"a old b old\nline2\n");
  // The right count applies.
  ok(&run_in(
    d.path(),
    &[
      "edit_file",
      "f.txt",
      "--pattern",
      "old",
      "--replacement",
      "new",
      "--expected-count",
      "2",
    ],
  ));
  assert_eq!(read(d.path(), "f.txt"), b"a new b new\nline2\n");
}

#[test]
fn no_match_changes_nothing_and_bad_targets_are_refused() {
  let d = tempfile::tempdir().unwrap();
  write(d.path(), "f.txt", b"text\n");
  let doc = ok(&run_in(
    d.path(),
    &[
      "edit_file",
      "f.txt",
      "--pattern",
      "absent",
      "--replacement",
      "x",
    ],
  ));
  assert_eq!(
    (
      doc["changed"].clone(),
      doc["replacements"].clone(),
      doc["diff"].clone()
    ),
    (json!(false), json!(0), json!(""))
  );
  write(d.path(), "bad.txt", b"\xff\xfe old");
  std::os::unix::fs::symlink("f.txt", d.path().join("link.txt")).unwrap();
  std::fs::hard_link(d.path().join("f.txt"), d.path().join("hard.txt")).unwrap();
  for (target, code) in [
    ("bad.txt", "NOT_UTF8"),
    ("link.txt", "SYMLINK_TARGET"),
    ("hard.txt", "HARDLINKED_FILE"),
    ("missing.txt", "NOT_FOUND"),
  ] {
    let out = run_in(
      d.path(),
      &[
        "edit_file",
        target,
        "--pattern",
        "old",
        "--replacement",
        "new",
      ],
    );
    assert_eq!(out.tool_code(), code, "{target}: {}", out.text());
  }
  assert_eq!(read(d.path(), "bad.txt"), b"\xff\xfe old");
}
