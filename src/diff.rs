//! A small unified diff (3 lines of context) between two texts, for edit_file and write_file
//! results. Lines are compared exactly (CRLF kept). Large inputs fall back to one hunk.

use serde_json::{json, Value};

const CONTEXT: usize = 3;
/// Above this many cells the LCS table is skipped and the changed middle is one block.
const MAX_LCS_CELLS: usize = 4_000_000;
/// The diff text is capped (`diff_truncated: true`).
pub const MAX_DIFF_BYTES: usize = 32 << 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
  Keep,
  Del,
  Add,
}

fn lines(text: &str) -> Vec<&str> {
  text.split_inclusive('\n').collect()
}

/// Edit script between `a` and `b` (indices into each).
fn script(a: &[&str], b: &[&str]) -> Vec<(Op, usize, usize)> {
  let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
  let suffix = a[prefix..]
    .iter()
    .rev()
    .zip(b[prefix..].iter().rev())
    .take_while(|(x, y)| x == y)
    .count();
  let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
  let mut out: Vec<(Op, usize, usize)> = (0..prefix).map(|i| (Op::Keep, i, i)).collect();
  if am.len().saturating_mul(bm.len()) <= MAX_LCS_CELLS {
    // LCS lengths from the end, then walk forward.
    let (n, m) = (am.len(), bm.len());
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
      for j in (0..m).rev() {
        t[i * (m + 1) + j] = if am[i] == bm[j] {
          t[(i + 1) * (m + 1) + j + 1] + 1
        } else {
          t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1])
        };
      }
    }
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
      if i < n && j < m && am[i] == bm[j] {
        out.push((Op::Keep, prefix + i, prefix + j));
        i += 1;
        j += 1;
      } else if i < n && (j == m || t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1]) {
        // Removals before additions on a tie, as diff(1) prints them.
        out.push((Op::Del, prefix + i, prefix + j));
        i += 1;
      } else {
        out.push((Op::Add, prefix + i, prefix + j));
        j += 1;
      }
    }
  } else {
    out.extend((0..am.len()).map(|i| (Op::Del, prefix + i, prefix)));
    out.extend((0..bm.len()).map(|j| (Op::Add, prefix + am.len(), prefix + j)));
  }
  let (sa, sb) = (a.len() - suffix, b.len() - suffix);
  out.extend((0..suffix).map(|k| (Op::Keep, sa + k, sb + k)));
  out
}

fn push_line(out: &mut String, mark: char, line: &str) {
  out.push(mark);
  match line.strip_suffix('\n') {
    Some(body) => {
      out.push_str(body);
      out.push('\n');
    }
    None => {
      out.push_str(line);
      out.push_str("\n\\ No newline at end of file\n");
    }
  }
}

/// The unified diff of `before` -> `after` for `path`, with counts:
/// `{diff, diff_truncated, lines_added, lines_removed}`.
pub fn unified(path: &str, before: &str, after: &str) -> Value {
  let (a, b) = (lines(before), lines(after));
  let ops = script(&a, &b);
  let added = ops.iter().filter(|o| o.0 == Op::Add).count();
  let removed = ops.iter().filter(|o| o.0 == Op::Del).count();
  let mut text = String::new();
  if added + removed > 0 {
    text.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    let changed: Vec<usize> = (0..ops.len()).filter(|&k| ops[k].0 != Op::Keep).collect();
    let mut k = 0;
    while k < changed.len() {
      // Group changes whose context windows touch.
      let start = changed[k].saturating_sub(CONTEXT);
      let mut end = changed[k];
      while k + 1 < changed.len() && changed[k + 1] <= end + 2 * CONTEXT + 1 {
        k += 1;
        end = changed[k];
      }
      let end = (end + CONTEXT).min(ops.len() - 1);
      let hunk = &ops[start..=end];
      let a_count = hunk.iter().filter(|o| o.0 != Op::Add).count();
      let b_count = hunk.iter().filter(|o| o.0 != Op::Del).count();
      let a_start = if a_count == 0 {
        hunk[0].1
      } else {
        hunk[0].1 + 1
      };
      let b_start = if b_count == 0 {
        hunk[0].2
      } else {
        hunk[0].2 + 1
      };
      text.push_str(&format!(
        "@@ -{a_start},{a_count} +{b_start},{b_count} @@\n"
      ));
      for &(op, i, j) in hunk {
        match op {
          Op::Keep => push_line(&mut text, ' ', a[i]),
          Op::Del => push_line(&mut text, '-', a[i]),
          Op::Add => push_line(&mut text, '+', b[j]),
        }
      }
      k += 1;
    }
  }
  let truncated = text.len() > MAX_DIFF_BYTES;
  if truncated {
    let mut cut = MAX_DIFF_BYTES;
    while !text.is_char_boundary(cut) {
      cut -= 1;
    }
    text.truncate(cut);
  }
  json!({
    "diff": text,
    "diff_truncated": truncated,
    "lines_added": added,
    "lines_removed": removed,
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn hunks_and_counts() {
    let d = unified("f", "a\nb\nc\n", "a\nB\nc\n");
    assert_eq!(
      d["diff"],
      "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
    );
    assert_eq!(
      (d["lines_added"].clone(), d["lines_removed"].clone()),
      (json!(1), json!(1))
    );
  }

  #[test]
  fn identical_texts_have_an_empty_diff() {
    assert_eq!(unified("f", "x\n", "x\n")["diff"], "");
  }

  #[test]
  fn missing_final_newline_is_marked() {
    let d = unified("f", "a", "a\n");
    assert_eq!(
      d["diff"],
      "--- a/f\n+++ b/f\n@@ -1,1 +1,1 @@\n-a\n\\ No newline at end of file\n+a\n"
    );
  }

  #[test]
  fn distant_changes_make_separate_hunks() {
    let before: String = (1..=20).map(|i| format!("l{i}\n")).collect();
    let after = before
      .replace("l2\n", "two\n")
      .replace("l19\n", "nineteen\n");
    let d = unified("f", &before, &after);
    assert_eq!(
      d["diff"].as_str().unwrap().matches("@@ -").count(),
      2,
      "{}",
      d["diff"]
    );
  }

  #[test]
  fn creating_a_file_starts_at_line_zero() {
    let d = unified("f", "", "new\n");
    assert_eq!(d["diff"], "--- a/f\n+++ b/f\n@@ -0,0 +1,1 @@\n+new\n");
  }
}
