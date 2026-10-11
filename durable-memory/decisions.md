# Decisions (0.1.0)

- **Rust, one binary, lmapi native** (lmrust v0.1.2): milliseconds to start per call, the same
  wiring as lmsql (spec.rs, schema drift test, legacy help, CI). Params are camelCase like lmsql
  (`maxBytes` -> `--max-bytes`); op names snake_case.
- **Five ops.** An earlier plan had a `sed_file` with an in-process GNU-sed subset; the owner
  dropped it: lmprobe already had a text editor, so its engine moved here as `edit_file`
  (lmtools = basic tools, lmprobe = advanced). The engine is copied (MIT, same owner) with
  serde/anyhow replaced by plain parsing, replacements counted, and `expectedCount` added; no
  git/force/recovery machinery. lmprobe is unchanged until a later dedup round.
- **Atomic writes everywhere** (temp + rename, permissions kept). Hard-linked targets are refused
  like lmprobe does: a rename would split them silently.
- **Paging vs files.** read_file / list_files page (cheap to re-ask); webget cannot re-read a
  response cheaply, so a big body goes to a file in full (lmsql's spill/out convention).
- **regex** (full Unicode, linear time) for the engine and globs; **ureq 2** (blocking, rustls,
  bundled roots, no async runtime) for webget; **url** for redirect resolution and credential
  removal. All build on Rust 1.79 (lockfile resolved with the MSRV-aware fallback).
- **webget follows redirects itself** (client redirects off) so every hop passes the host rules
  and the reported URL is exact.
- **format text** is a small deterministic HTML-to-text pass, not a renderer.
- Dev/test profile optimised (opt-level 1, deps 2) as in lmsql.
