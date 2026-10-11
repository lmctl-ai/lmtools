//! Byte-exact envelopes for representative results and errors (temp paths normalised to
//! `<ROOT>`). Regenerate after an intended change: `UPDATE_GOLDENS=1 cargo test --test goldens`.

mod support;

use std::path::PathBuf;
use support::*;

fn golden(name: &str, out: &Out, root: &std::path::Path) {
  let text = out.text().replace(&s(root), "<ROOT>");
  let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    .join("tests/goldens")
    .join(format!("{name}.json"));
  if std::env::var_os("UPDATE_GOLDENS").is_some() {
    std::fs::write(&file, &text).unwrap();
  }
  let expected = std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("missing golden {name}"));
  assert_eq!(
    text, expected,
    "golden {name} differs (UPDATE_GOLDENS=1 to accept)"
  );
}

#[test]
fn envelopes() {
  let d = tempfile::tempdir().unwrap();
  let root = d.path().canonicalize().unwrap();
  std::fs::write(root.join("notes.txt"), "alpha\nbeta old\ngamma\n").unwrap();
  std::fs::write(root.join("blob.bin"), b"\x00\x01").unwrap();
  let cases: Vec<(&str, Vec<&str>)> = vec![
    (
      "read_file_lines",
      vec![
        "read_file",
        "notes.txt",
        "--start-line",
        "2",
        "--line-numbers",
      ],
    ),
    ("read_file_binary", vec!["read_file", "blob.bin"]),
    (
      "edit_file_dry_run",
      vec![
        "edit_file",
        "notes.txt",
        "--pattern",
        "old",
        "--replacement",
        "new",
        "--dry-run",
      ],
    ),
    (
      "edit_file_count_mismatch",
      vec![
        "edit_file",
        "notes.txt",
        "--pattern",
        "a",
        "--replacement",
        "A",
        "--expected-count",
        "1",
      ],
    ),
    (
      "write_file_create_exists",
      vec![
        "write_file",
        "notes.txt",
        "--content",
        "x",
        "--mode",
        "create",
      ],
    ),
    ("list_files_missing", vec!["list_files", "nope"]),
  ];
  for (name, args) in cases {
    golden(name, &run_in(&root, &args), &root);
  }
  // A permission denial, with the settings file path normalised.
  std::fs::write(
    root.join("settings.json"),
    r#"{"version": 1, "cwd": "readonly", "other": "none"}"#,
  )
  .unwrap();
  let out = run_call(
    &["write_file", "notes.txt", "--content", "x"],
    &Call {
      cwd: Some(&root),
      env: vec![("LMTOOLS_CONFIG", s(&root.join("settings.json")))],
      ..Call::default()
    },
  );
  golden("write_file_permission_denied", &out, &root);
}
