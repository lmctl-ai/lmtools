# Changelog

## 0.1.0 (unreleased)

First version: the basic file and web tool belt for LLM agents, a native lmapi tool.

- `list_files`: sorted, optionally recursive (depth-first) listings with glob filtering, hidden
  entries opt-in, offset/limit paging; symlinked directories listed, not followed.
- `read_file`: byte or line pages with exact paging fields, cat -n numbering, binary detection
  (`BINARY_FILE` unless `asBase64`), lossy UTF-8 flagged, total size/lines.
- `write_file`: atomic create/overwrite/append from text, base64 or stdin (16 MiB), permissions
  kept, symlink / non-regular / hard-linked targets refused.
- `edit_file`: lmprobe's text-edit engine moved here (pattern/regex/word/first replacements, line
  insert/delete/replace, prepend/append, JSON scripts), atomic, with a unified diff and
  `expectedCount` (`EDIT_COUNT_MISMATCH`).
- `webget`: HTTP(S) GET with self-followed redirects, non-2xx as results, text or base64 bodies,
  big bodies spilled or written to `--out` in full, `format: text` for HTML, proxies from the
  environment, no secret echo.
- Settings file (`~/.config/lmtools/settings.json`): cwd/other path levels on canonical paths,
  webget allow/deny hosts, output.dir containment, fail-closed loading, denials with
  `manual_command`, `--dangerously-bypass-permission-settings` gated by `allow_bypass`.
- `--dry-run` prints what write_file / edit_file would change (diff) and never writes.
