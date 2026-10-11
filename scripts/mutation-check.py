#!/usr/bin/env python3
"""Mutation check of lmtools' safety-relevant code: the permission gate (levels, symlink/`..`
resolution, hosts, output.dir, bypass, fail-closed settings), dry runs never writing, atomic
writes (temp cleanup, permission bits), and the edit count check. Each mutation is applied
alone, `cargo test --no-fail-fast` runs, the file is restored. Every mutation must make at least
one test fail (exit 1 lists survivors).

Usage: python3 scripts/mutation-check.py   (from anywhere; takes a few minutes)
"""
import pathlib, subprocess, re, sys, os
os.chdir(pathlib.Path(__file__).resolve().parent.parent)
PERM = 'src/permissions.rs'
SPEC = 'src/spec.rs'
SET = 'src/settings.rs'
W = 'src/ops/write.rs'
E = 'src/ops/edit.rs'
A = 'src/atomic.rs'

mutations = [
  ("M1 write allowed at any level", PERM,
   "      Need::Write => level == Level::ReadWrite,",
   "      Need::Write => true,"),
  ("M2 symlinks and .. not resolved", PERM,
   "    let real = resolve_real(path);",
   "    let real = Some(path.to_path_buf());"),
  ("M3 deny_hosts ignored", PERM,
   "    let refusal = if matches(&settings.deny_hosts) {",
   "    let refusal = if false {"),
  ("M4 output.dir containment removed", PERM,
   "        (Some(root), Some(real)) => real.starts_with(&root) && real != root,",
   "        (Some(_), Some(_)) => true,"),
  ("M5 bypass without allow_bypass", SPEC,
   "      if !s.allow_bypass {",
   "      if false {"),
  ("M6 invalid settings fall back to open", SET,
   """  let doc: Value = serde_json::from_slice(&text)
    .map_err(|e| invalid(&path, format!("is not valid JSON ({e})")))?;""",
   """  let Ok(doc) = serde_json::from_slice::<Value>(&text) else {
    return Ok(None);
  };"""),
  ("M7 write_file dry run writes", W,
   "  if ctx.dry_run {",
   "  if false {"),
  ("M8 edit_file dry run writes", E,
   "  if ctx.dry_run {",
   "  if false {"),
  ("M9 temp file kept on error", A,
   "      let _ = std::fs::remove_file(temp);",
   "      let _ = temp;"),
  ("M10 permission bits not kept", A,
   "      std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(mode))",
   "      std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o644 | (mode & 0)))"),
  ("M11 edit count check removed", E,
   "    if result.replacements as u64 != expected {",
   "    if false {"),
]

files = {f for _, f, _, _ in mutations}
originals = {f: pathlib.Path(f).read_text() for f in files}
for name, f, a, b in mutations:
  assert originals[f].count(a) == 1, (name, originals[f].count(a))
survivors = []
try:
  for name, f, a, b in mutations:
    pathlib.Path(f).write_text(originals[f].replace(a, b))
    r = subprocess.run(["cargo", "test", "--no-fail-fast"], capture_output=True, text=True)
    failed = sorted(set(re.findall(r"^test (\S+) \.\.\. FAILED", r.stdout, re.M)))
    compiled = "error[" not in r.stderr
    print(f"{name}: exit={r.returncode} compiled={compiled} failing={failed}", flush=True)
    if not (compiled and failed):
      survivors.append(name)
    pathlib.Path(f).write_text(originals[f])
finally:
  for f, t in originals.items():
    pathlib.Path(f).write_text(t)
print("SURVIVORS:", survivors)
sys.exit(1 if survivors else 0)
