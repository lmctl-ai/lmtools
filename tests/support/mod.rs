//! Shared helpers: run the built binary with a hard timeout, in a directory, with env and stdin.

#![allow(dead_code)]

use serde_json::Value;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub struct Out {
  pub code: i32,
  pub stdout: Vec<u8>,
  pub stderr: String,
  pub elapsed: Duration,
}

impl Out {
  pub fn json(&self) -> Value {
    serde_json::from_slice(&self.stdout).unwrap_or_else(|e| {
      panic!(
        "stdout is not one JSON document ({e}): {}\nstderr: {}",
        String::from_utf8_lossy(&self.stdout),
        self.stderr
      )
    })
  }
  pub fn text(&self) -> String {
    String::from_utf8_lossy(&self.stdout).into_owned()
  }
  /// `error.data.tool_code` of an error envelope.
  pub fn tool_code(&self) -> String {
    self.json()["error"]["data"]["tool_code"]
      .as_str()
      .unwrap_or("<none>")
      .to_string()
  }
}

pub fn bin() -> PathBuf {
  PathBuf::from(env!("CARGO_BIN_EXE_lmtools"))
}

/// What a call runs with.
#[derive(Default)]
pub struct Call<'a> {
  pub cwd: Option<&'a Path>,
  pub env: Vec<(&'a str, String)>,
  pub stdin: Option<&'a [u8]>,
}

/// Run lmtools; kills it and panics after 60 s. Hermetic: a missing settings file unless the
/// call sets LMTOOLS_CONFIG, and a private spill directory unless it sets LMTOOLS_OUT_DIR.
pub fn run_call(args: &[&str], call: &Call) -> Out {
  let mut cmd = Command::new(bin());
  cmd
    .args(args)
    .stdin(if call.stdin.is_some() {
      Stdio::piped()
    } else {
      Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .env_remove("LMTOOLS_READONLY")
    .env_remove("LMTOOLS_DRYRUN")
    .env("LMTOOLS_CONFIG", "/nonexistent/lmtools-test/settings.json");
  // The local test server must never be reached through a proxy.
  for var in [
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
  ] {
    cmd.env_remove(var);
  }
  for (k, v) in &call.env {
    cmd.env(k, v);
  }
  if let Some(dir) = call.cwd {
    cmd.current_dir(dir);
  }
  let start = Instant::now();
  let mut child = cmd.spawn().expect("spawn lmtools");
  if let (Some(input), Some(mut pipe)) = (call.stdin, child.stdin.take()) {
    pipe.write_all(input).unwrap();
  }
  let mut so = child.stdout.take().unwrap();
  let mut se = child.stderr.take().unwrap();
  let t1 = std::thread::spawn(move || {
    let mut v = Vec::new();
    let _ = so.read_to_end(&mut v);
    v
  });
  let t2 = std::thread::spawn(move || {
    let mut v = Vec::new();
    let _ = se.read_to_end(&mut v);
    v
  });
  let status = loop {
    if let Some(s) = child.try_wait().unwrap() {
      break s;
    }
    if start.elapsed() > Duration::from_secs(60) {
      let _ = child.kill();
      let _ = child.wait();
      panic!("lmtools {args:?} did not finish within 60 s");
    }
    std::thread::sleep(Duration::from_millis(5));
  };
  Out {
    code: status.code().expect("exit code"),
    stdout: t1.join().unwrap(),
    stderr: String::from_utf8_lossy(&t2.join().unwrap()).into_owned(),
    elapsed: start.elapsed(),
  }
}

pub fn run(args: &[&str]) -> Out {
  run_call(args, &Call::default())
}

/// Run in `dir` (paths in `args` relative to it).
pub fn run_in(dir: &Path, args: &[&str]) -> Out {
  run_call(
    args,
    &Call {
      cwd: Some(dir),
      ..Call::default()
    },
  )
}

pub fn s(path: &Path) -> String {
  path.display().to_string()
}
