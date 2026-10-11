//! The lmapi surface: the published document and its channel files, mode detection, the legacy
//! surface. Regenerate the dev channel file after changing an operation:
//! `UPDATE_LMAPI_SCHEMA=1 cargo test --test lmapi schema_dev_file`.

mod support;

use serde_json::Value;
use std::path::PathBuf;
use support::*;

fn root() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn channel(name: &str) -> Value {
  serde_json::from_str(&std::fs::read_to_string(root().join(name)).unwrap()).unwrap()
}

#[test]
fn schema_dev_file_matches_lmtools_schema_byte_for_byte() {
  let out = run(&["--schema"]);
  assert_eq!(out.code, 0);
  let file = root().join("cli.schema.dev.json");
  if std::env::var_os("UPDATE_LMAPI_SCHEMA").is_some() {
    std::fs::write(&file, &out.stdout).unwrap();
  }
  assert!(
    std::fs::read(&file).unwrap() == out.stdout,
    "cli.schema.dev.json is stale: run UPDATE_LMAPI_SCHEMA=1 cargo test --test lmapi schema_dev_file"
  );
}

#[test]
fn latest_is_an_additive_subset_of_dev() {
  let dev = channel("cli.schema.dev.json");
  let latest = channel("cli.schema.latest.json");
  assert_eq!(latest["tool"], dev["tool"]);
  for op in latest["operations"].as_array().unwrap() {
    let name = &op["name"];
    let dev_op = dev["operations"]
      .as_array()
      .unwrap()
      .iter()
      .find(|d| &d["name"] == name)
      .unwrap_or_else(|| panic!("{name} was removed from dev"));
    assert_eq!(dev_op["access"], op["access"], "access of {name} changed");
    for param in op["params"].as_array().unwrap() {
      if matches!(param["occurs"].as_str(), None | Some("1") | Some("+")) {
        let dev_param = dev_op["params"]
          .as_array()
          .unwrap()
          .iter()
          .find(|p| p["name"] == param["name"])
          .unwrap_or_else(|| panic!("required param {} of {name} removed", param["name"]));
        assert_eq!(dev_param["type"], param["type"]);
      }
    }
  }
}

#[test]
fn channel_files_pass_the_lmrust_publish_check() {
  for name in ["cli.schema.dev.json", "cli.schema.latest.json"] {
    lmrust::parse_published_schema(&channel(name)).unwrap_or_else(|i| panic!("{name}: {i:?}"));
  }
}

#[test]
fn list_and_op_help_are_usable_on_their_own() {
  let ops: Vec<String> = run(&["--list"])
    .json()
    .as_array()
    .unwrap()
    .iter()
    .map(|o| o["name"].as_str().unwrap().to_string())
    .collect();
  assert_eq!(ops, crate_ops());
  for op in crate_ops() {
    let out = run(&[&op, "--help"]);
    assert_eq!(out.code, 0, "{op}");
    let text = out.text();
    assert!(text.starts_with(&format!("lmtools {op}")), "{text}");
    assert!(text.contains("Example: lmtools"), "{op}: {text}");
  }
}

fn crate_ops() -> Vec<String> {
  let doc = channel("cli.schema.dev.json");
  doc["operations"]
    .as_array()
    .unwrap()
    .iter()
    .map(|o| o["name"].as_str().unwrap().to_string())
    .collect()
}

#[test]
fn legacy_surface() {
  let out = run(&[]);
  assert_eq!(out.code, 0);
  assert!(out.text().starts_with("lmtools: file and web tools"));
  for op in crate_ops() {
    assert!(out.text().contains(&op), "--help mentions {op}");
  }
  let out = run(&["--version"]);
  assert_eq!(
    out.text(),
    format!("lmtools {}\n", env!("CARGO_PKG_VERSION"))
  );
  let out = run(&["cat", "x"]);
  assert_eq!((out.code, out.stdout.len()), (2, 0));
  assert!(out.stderr.contains("unknown command"));
}
