# lmtools

The basic file and web tool belt for LLM agents: **list, read, write, edit, webget** in one small
Rust binary. Every call is one process (milliseconds to start), takes flags or JSON, and prints
**one JSON document** on stdout — results and errors alike. Nothing is remembered between calls:
big results are paged by the caller or written to a file.

It is a native [lmapi](https://github.com/lmctl-ai/lmapi) tool (`cli.schema.v1`) built on
[lmrust](https://github.com/lmctl-ai/lmrust): `lmtools --schema` publishes the contract,
`lmtools --list` lists the ops, and `lmtools <op> --help` explains one with an example. An agent
needs nothing else.

```sh
lmtools list_files src --recursive --glob '**/*.rs'
lmtools read_file src/main.rs --start-line 40 --end-line 80 --line-numbers
lmtools write_file notes/todo.md --content '- ship it' --mode create --create-parents
lmtools edit_file src/lib.rs --pattern old_name --replacement new_name --word --expected-count 3
lmtools webget https://example.com/docs --format text
lmtools <op> --input-json '{"path": "src/main.rs", "startLine": 1, "endLine": 20}'
```

Paths are relative to the working directory unless absolute. Errors look like
`{"success": false, "error": {"code": "NOT_FOUND", "message": "...", "data": {"tool_code": "NOT_FOUND"}}}`;
`error.data.tool_code` is the precise reason (table below).

## The operations

### list_files

Lists a directory, sorted by name; `--recursive` walks depth-first (`--max-depth N`). Each entry
has `name`, `path` (relative to the listed directory), `type` (`file`, `dir`, `symlink`, `other`),
`size` (files) and `mtime` (RFC 3339 UTC). `--glob` keeps matching entries (`*`, `?`, `[..]`,
`**`): matched against the name, or the relative path when the glob contains `/`. Dot files are
skipped unless `--include-hidden`. Symlinked directories are listed but never followed.
Paging: `--limit` (default 200, at most 5000) and `--offset`; continue with `next_offset` until it
is `null` (`truncated` says whether more entries exist).

### read_file

Reads one page of a text file — by bytes (`--offset`, `--max-bytes`, default 64 KiB, at most
1 MiB) or by lines (`--start-line`, `--end-line`, 1-based inclusive, the page still capped by
`--max-bytes`). Returns `content`, `eof`, and where to continue (`next_offset`;
`next_start_line` in line mode), plus `total_size` and `total_lines` (files up to 8 MiB).
`--line-numbers` prefixes lines like `cat -n`. A byte page never ends inside a UTF-8 character.
Invalid UTF-8 is replaced and flagged `lossy: true`. A file with a NUL byte in its first 8 KiB is
binary: `BINARY_FILE` unless `--as-base64` (then `content_base64`).

### write_file

Writes one file **atomically**: a temporary file in the same directory, synced, then renamed over
the target, so a reader never sees half a file and a failure leaves the original untouched. An
existing file keeps its permission bits. Content comes from `--content`, `--content-base64` or
`--stdin` (at most 16 MiB). `--mode` is `overwrite` (default), `create` (fails with
`ALREADY_EXISTS`) or `append`. `--create-parents` creates missing directories. Symlinks,
non-regular files and hard-linked files are refused (a rename would replace the link or split the
hard links). Returns `bytes_written`, `created`, `overwrote`.

### edit_file

Edits one existing UTF-8 file in place (atomically, permissions kept) and returns a unified
`diff`, `replacements`, `lines_added` / `lines_removed`. Choose **one** edit:

| Edit | Flags |
| --- | --- |
| Replace text | `--pattern old --replacement new` (+ `--regex`, `--word`, `--first`, `--ignore-case`) |
| Insert a line | `--insert-before N --text '...'` / `--insert-after N --text '...'` |
| Delete lines | `--delete-lines START:END` (or `N`) |
| Replace lines | `--replace-lines START:END --text '...'` |
| Add at the start / end | `--prepend --text '...'` / `--append --text '...'` (exact bytes) |
| Several, in order | `--script '[{"op":"replace","pattern":"a","replacement":"b"},{"op":"insert_after","line":2,"text":"x"}]'` (or `--script -` from stdin) |

Patterns are literal unless `--regex` (Rust regex: `(?m)`, `(?s)`, captures `$1` / `${name}`, `$$`
for a dollar; no lookaround or backreferences). Literal replacements keep `$` as is. `--word`
matches whole Unicode words. Lines are 1-based and inclusive; a CRLF file stays CRLF; inserted
lines get a line ending. Script ops: `replace` (`pattern`, `replacement`, optional `regex`,
`word`, `ignore_case`, `first`), `insert_before` / `insert_after` (`line`, `text`), `delete_lines`
(`start`, `end`), `replace_lines` (`start`, `end`, `text`), `prepend` / `append` (`text`); unknown
fields are refused. **`--expected-count N`** fails with `EDIT_COUNT_MISMATCH` (writing nothing)
unless exactly N replacements happen — use it to catch a pattern that matches more or less than
you think. No match is not an error: `changed: false`.

The engine is lmprobe's text editor, moved here (see [ATTRIBUTION.md](ATTRIBUTION.md)).

### webget

One HTTP(S) GET. Returns `status` (a non-2xx status is a normal result with `ok: false`),
`final_url` (after up to 10 redirects; `--follow-redirects=false` to stop at the first),
`content_type`, a few response `headers`, and `body`: text, or `{"$blob_base64": "..."}` for
binary content. `--headers '{"Accept": "application/json"}'` adds request headers (their values
are never echoed back; credentials in a URL are removed from `final_url`). `--format text` turns
HTML into readable plain text: scripts, styles and comments dropped, block tags become line
breaks, list items become `- ` bullets, entities decoded — simple and deterministic, not a
browser. `--timeout-ms` (default 20000) covers the whole call. `HTTP(S)_PROXY` is honoured.

## Big output goes to a file

`read_file` and `list_files` page (the caller asks for the next page). `webget` never truncates a
body: one bigger than `--max-bytes` (default 64 KiB) is written **in full** to a spill file and
the response carries `spilled: true`, `file: {path, bytes, format}` and a short `preview`;
`--out PATH` writes the full body to that file instead (`overwrote` says whether one was
replaced). Spill files go to `$LMTOOLS_OUT_DIR`, default `<system temp>/lmtools/` (created 0700
and required to be private to you); files are 0600, written to a temporary name and renamed when
complete. Follow up with your own tools (`head`, `grep`, `jq`) or `read_file` on `file.path`.
lmtools never deletes old spill files — that is the caller's job.

## Dry run

`--dry-run` (or `LMTOOLS_DRYRUN=1`) **prints information only and never writes**. Reads run
normally. `write_file` / `edit_file` return
`{"dry_run": true, "executed": false, "would_change": ..., "path": ..., "diff": ...}` plus what
would happen (`bytes`, `would_create`, `replacements`, ...). `webget` still performs the GET (a
read) but writes no file (`file: null`, `would_write: {path, format}`, or a note when it would
have spilled). `--readonly` (or `LMTOOLS_READONLY=1`) refuses `write_file` / `edit_file` outright.

## Permissions (settings file)

The settings file is a **guardrail against honest mistakes** (the wrong file, the wrong
directory, the wrong host); it only ever restricts. **The real boundary is the operating
system**: run the agent as a user that can only touch what it should. Without a settings file
lmtools is open (it can do whatever the user running it can).

The file is `$LMTOOLS_CONFIG`, else `$XDG_CONFIG_HOME/lmtools/settings.json`, else
`~/.config/lmtools/settings.json`. Keep it mode 0600 (lmtools warns on stderr if group or others
can write it).

```json
{
  "version": 1,
  "cwd": "readwrite",
  "other": "readonly",
  "webget": {"allow_hosts": ["*.example.com", "docs.rs"], "deny_hosts": ["internal.example.com"]},
  "allow_bypass": false,
  "output": {"dir": "~/lmtools-out"}
}
```

- **Fail closed.** A file that exists but cannot be read, is not valid JSON, has an unknown key or
  a wrong value fails every call with `SETTINGS_INVALID` (naming the file and key); lmtools never
  falls back to open.
- **Paths.** `cwd` (`readonly` | `readwrite`, required) applies to the working directory and
  everything under it, `other` (`none` | `readonly` | `readwrite`, required) to everything else.
  Reads need `readonly`, writes need `readwrite`. Paths are resolved first — symlinks (also
  dangling ones) and `..` are followed — so `link-to-elsewhere.txt` or `sub/../../x` are judged by
  where they really are.
- **Hosts.** `webget.deny_hosts` (globs) always refuses; with `webget.allow_hosts`, only those
  hosts are allowed. Every redirect hop is checked.
- **Output.** With `output.dir`, a `--out` file must resolve inside it and spills go there;
  without it, `--out` follows the path rules.
- **Denials** are one short error: `FORBIDDEN` (exit 6), `tool_code` `PERMISSION_DENIED`, naming
  the path or host, the rule and the settings file, and `error.data.manual_command` with the plain
  command the operator can run themselves (`cat`, `sed -n`, `ls -la`, `printf ... >`,
  `"$EDITOR"`, `curl -L`) — never header values or URL credentials.
- **Bypass.** `--dangerously-bypass-permission-settings` skips the rules for one call (not the
  limits) and prints one line on stderr — but only if the operator set `"allow_bypass": true`;
  otherwise it fails with `BYPASS_NOT_ALLOWED`. The flag is visible to the model, so the opt-in
  lives in the operator's file. Without a settings file it does nothing.
- Dry runs follow the same rules: a refused write is refused in a dry run too.

## Error codes (`error.data.tool_code`)

| tool_code | exit | meaning |
| --- | --- | --- |
| `NOT_FOUND` | 4 | no such file / directory (or parent without `--create-parents`) |
| `NOT_A_DIRECTORY`, `NOT_A_REGULAR_FILE` | 2 | wrong kind of path |
| `BINARY_FILE` | 2 | read_file of a binary file without `--as-base64` |
| `NOT_UTF8` | 2 | edit_file of a non-UTF-8 file |
| `SYMLINK_TARGET`, `HARDLINKED_FILE` | 2 | write/edit target refused |
| `ALREADY_EXISTS` | 5 | write_file `--mode create` on an existing file |
| `EDIT_COUNT_MISMATCH` | 5 | edit_file replacements differ from `--expected-count` (nothing written) |
| `INVALID_EDIT`, `INVALID_PARAMS`, `INVALID_URL`, `LIMIT_OUT_OF_RANGE`, `TOO_LARGE`, ... | 2 | the request cannot be served as given |
| `TIMEOUT` | 7 | webget timed out |
| `NETWORK_ERROR`, `TOO_MANY_REDIRECTS` | 8 | webget could not get a response |
| `OUT_NOT_WRITABLE` | 6 | an out or spill file could not be written |
| `PERMISSION_DENIED`, `BYPASS_NOT_ALLOWED` | 6 | the settings file refused it |
| `SETTINGS_INVALID` | 2 | the settings file is broken |
| `READONLY_VIOLATION` | 6 | a write op under `--readonly` |
| `OS_PERMISSION_DENIED`, `IO_ERROR` | 6 / 1 | the operating system refused or failed |

## Build

```sh
cargo build --release    # target/release/lmtools
cargo test               # unit + functional tests (temp dirs, a local HTTP server)
python3 scripts/mutation-check.py   # the safety mutants (permission gate, dry run, atomic writes, edit count)
```

MSRV: Rust 1.79. Dependencies: lmrust, serde_json, regex, ureq (rustls), url, libc.

## License

MIT, see [LICENSE](LICENSE). Copyright Lattice Systems LLC. See also [ATTRIBUTION.md](ATTRIBUTION.md).
