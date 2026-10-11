# lmtools development

The basic file and web tool belt for LLM agents (list_files, read_file, write_file, edit_file,
webget): one Rust binary, a native lmapi tool (lmrust `v0.1.2`, default features off), laid out
like lmsql. Read [README.md](README.md) for the contract and
[durable-memory/index.md](durable-memory/index.md) for decisions.

## Invariants

- **Stateless one-shot CLI.** One process per call, no prompts, no cursors or sessions; nothing
  persists between calls except files the caller asked for (`write_file`, `edit_file`, `--out`,
  spill files). Big results page (read_file, list_files) or go to a file (webget).
- **One JSON document out**, results and errors; errors carry `error.data.tool_code`
  (`src/error.rs`). `--list` and `<op> --help` must be enough for an LLM: every op description
  ends with `Example: lmtools ...` (a unit test checks it).
- **Writes are atomic** (`src/atomic.rs`): temp file in the target directory, synced, renamed;
  permission bits kept; temp removed on any error; symlink, non-regular and hard-linked targets
  refused.
- **Dry run prints, never writes** (lmrust's dryrun global); `--readonly` refuses write ops.
- **Settings only restrict, and fail closed** (`src/settings.rs`, `src/permissions.rs`): paths
  are canonicalised before the cwd/other check; hosts checked per redirect hop; output.dir
  contains out/spill files; bypass only with `allow_bypass`; denials carry `manual_command`
  (never secrets). Tests pin `LMTOOLS_CONFIG` to a missing file and clear proxy variables.
- **No secret echo**: request header values and URL credentials never appear in output.
- **edit_file's engine is lmprobe's** (`src/text_edit.rs`, origin comment + ATTRIBUTION.md): keep
  its semantics in step with lmprobe until the later dedup round.
- **Contract files.** After changing an operation: `cargo run -- --schema > cli.schema.dev.json`
  (the drift test compares byte for byte; `cli.schema.latest.json` changes in a release).
  Goldens: `UPDATE_GOLDENS=1 cargo test --test goldens` only for intended changes.

## Checks

`cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`,
`cargo test --locked`, `cargo +1.79 test --locked` (MSRV), and after touching the gate, dry runs,
atomic writes or the edit count: `python3 scripts/mutation-check.py` (every mutant must fail a
test).

Commits: stage explicitly (never `git add -A`); intent line, context, trailers.
