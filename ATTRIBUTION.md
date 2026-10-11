# Attribution

- `src/text_edit.rs` (the `edit_file` engine) and the edit tests are ported from
  [lmprobe](https://github.com/lmctl-ai/lmprobe) (`crates/lmprobe/src/text_edit.rs`), MIT,
  Copyright Lattice Systems LLC — the same owner and license as lmtools. The semantics are
  unchanged; serde/anyhow were replaced by plain parsing and errors, and replacements are counted.
