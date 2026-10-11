# Permissions

Owner's model (shared with lmsql): permissions live in each tool's own settings file; a missing
file = wide open; the tool layer only restricts further — the OS user's file permissions are the
real boundary. Guardrails against an LLM's honest mistakes, not an adversary. No prompts.

- Settings: `$LMTOOLS_CONFIG` / `$XDG_CONFIG_HOME/lmtools/settings.json` /
  `~/.config/lmtools/settings.json`. Strict parse; any problem => `SETTINGS_INVALID` (never open).
  `cwd` and `other` are required (no implicit level). Warn if group/other-writable.
- Paths: `permissions::resolve_real` canonicalises (symlinks incl. dangling, `..`; a new file via
  its nearest existing parent). Inside the canonical cwd => `cwd` level, else `other`. Reads need
  readonly, writes readwrite. list_files never follows symlinked dirs, so a listing stays under
  the checked root.
- Hosts: `deny_hosts` first, then `allow_hosts` if present; globs on the lower-cased host; every
  redirect hop checked (webget follows redirects itself for this).
- output.dir: `--out` must resolve inside it and spills go there; without it `--out` follows the
  path rules. Spill files in the default spill dir are lmtools' own output (not path-checked).
- Denial: FORBIDDEN / exit 6 / PERMISSION_DENIED, one sentence (path or host, rule, settings
  file), `manual_command` = the operator's plain command (cat, sed -n, ls -la, printf/cat >,
  "$EDITOR", curl -L [-o]); never header values or URL credentials.
- Bypass flag on every op; only with `allow_bypass: true` (else BYPASS_NOT_ALLOWED); skips rules
  not limits; one stderr line; no-op without settings.
- Dry run keeps the rules and never writes. `--readonly` refuses write ops.
- No operator `config` helper in 0.1.0.
- Mutation check: scripts/mutation-check.py (gate levels, symlink resolution, deny hosts,
  output.dir, bypass, fail-closed, dry runs x2, temp cleanup, permission bits, edit count).
