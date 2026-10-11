//! Small, deterministic text-edit engine for `edit_file`.
//!
//! Origin: ported from lmprobe (crates/lmprobe/src/text_edit.rs, MIT, Lattice Systems LLC); same
//! semantics, with serde/anyhow replaced by plain parsing and errors, and replacements counted.
//!
//! Operations run in the order supplied. Line numbers are one-based and inclusive. Line edits
//! retain the input's LF/CRLF convention. Inserted text is given a trailing newline for
//! line-oriented operations; prepend and append preserve their text exactly.

use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};

/// A single sequential edit operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditOperation {
  Replace {
    pattern: String,
    replacement: String,
    regex: bool,
    word: bool,
    ignore_case: bool,
    first: bool,
  },
  InsertBefore {
    line: usize,
    text: String,
  },
  InsertAfter {
    line: usize,
    text: String,
  },
  DeleteLines {
    start: usize,
    end: usize,
  },
  ReplaceLines {
    start: usize,
    end: usize,
    text: String,
  },
  Prepend {
    text: String,
  },
  Append {
    text: String,
  },
}

/// The result of [`transform`]: the new text and how many replacements were made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transformed {
  pub text: String,
  pub replacements: usize,
}

/// Apply `ops` in order and return the resulting text.
pub fn transform(input: &str, ops: &[EditOperation]) -> Result<Transformed, String> {
  let mut output = input.to_owned();
  let mut replacements = 0;
  for operation in ops {
    output = match operation {
      EditOperation::Replace {
        pattern,
        replacement,
        regex,
        word,
        ignore_case,
        first,
      } => {
        let (text, count) = replace(
          &output,
          pattern,
          replacement,
          *regex,
          *word,
          *ignore_case,
          *first,
        )?;
        replacements += count;
        text
      }
      EditOperation::InsertBefore { line, text } => {
        let lines = split_lines(&output);
        let index = checked_line(*line, lines.len())?;
        let newline = newline_for(&output);
        let inserted = terminated(text, newline);
        let mut result = String::new();
        result.push_str(&lines[..index].concat());
        result.push_str(&inserted);
        result.push_str(&lines[index..].concat());
        result
      }
      EditOperation::InsertAfter { line, text } => {
        let lines = split_lines(&output);
        let index = checked_line(*line, lines.len())?;
        let newline = newline_for(&output);
        let inserted = terminated(text, newline);
        let mut result = String::new();
        result.push_str(&lines[..=index].concat());
        if !lines[index].ends_with('\n') {
          result.push_str(newline);
        }
        result.push_str(&inserted);
        result.push_str(&lines[index + 1..].concat());
        result
      }
      EditOperation::DeleteLines { start, end } => {
        let lines = split_lines(&output);
        let (start, end) = checked_range(*start, *end, lines.len())?;
        lines[..start].concat() + &lines[end + 1..].concat()
      }
      EditOperation::ReplaceLines { start, end, text } => {
        let lines = split_lines(&output);
        let (start, end) = checked_range(*start, *end, lines.len())?;
        let selected = lines[start..=end].concat();
        let newline = newline_for(&output);
        let replacement = if selected.ends_with('\n') {
          terminated(text, newline)
        } else {
          normalize_newlines(text, newline)
        };
        lines[..start].concat() + &replacement + &lines[end + 1..].concat()
      }
      EditOperation::Prepend { text } => text.clone() + &output,
      EditOperation::Append { text } => output + text,
    };
  }
  Ok(Transformed {
    text: output,
    replacements,
  })
}

fn replace(
  input: &str,
  pattern: &str,
  replacement: &str,
  is_regex: bool,
  whole_word: bool,
  ignore_case: bool,
  first: bool,
) -> Result<(String, usize), String> {
  if pattern.is_empty() {
    return Err("replace pattern must not be empty".into());
  }
  let expression = if is_regex {
    pattern.to_owned()
  } else {
    regex::escape(pattern)
  };
  let regex = RegexBuilder::new(&expression)
    .case_insensitive(ignore_case)
    .build()
    .map_err(|error| format!("invalid replacement pattern: {error}"))?;
  let mut selected = Vec::new();
  for captures in regex.captures_iter(input) {
    let matched = captures
      .get(0)
      .expect("captures_iter always contains the full match");
    if !whole_word || is_word_boundary(input, matched.start(), matched.end()) {
      let expanded = if is_regex {
        let mut expanded = String::new();
        captures.expand(replacement, &mut expanded);
        expanded
      } else {
        replacement.to_owned()
      };
      selected.push((matched.start(), matched.end(), expanded));
      if first {
        break;
      }
    }
  }
  let count = selected.len();
  if selected.is_empty() {
    return Ok((input.to_owned(), 0));
  }
  let mut result = String::with_capacity(input.len());
  let mut cursor = 0;
  for (start, end, expanded) in selected {
    result.push_str(&input[cursor..start]);
    result.push_str(&expanded);
    cursor = end;
  }
  result.push_str(&input[cursor..]);
  Ok((result, count))
}

fn is_word_boundary(input: &str, start: usize, end: usize) -> bool {
  fn word(character: Option<char>) -> bool {
    static WORD: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    character.is_some_and(|c| {
      WORD
        .get_or_init(|| Regex::new(r"^\w$").unwrap())
        .is_match(c.encode_utf8(&mut [0; 4]))
    })
  }
  !word(input[..start].chars().next_back()) && !word(input[end..].chars().next())
}

fn split_lines(input: &str) -> Vec<String> {
  if input.is_empty() {
    return Vec::new();
  }
  input.split_inclusive('\n').map(str::to_owned).collect()
}

fn newline_for(input: &str) -> &'static str {
  if input.contains("\r\n") {
    "\r\n"
  } else {
    "\n"
  }
}

fn terminated(text: &str, newline: &str) -> String {
  let text = normalize_newlines(text, newline);
  if text.is_empty() || text.ends_with(newline) {
    text
  } else {
    format!("{text}{newline}")
  }
}

fn normalize_newlines(text: &str, newline: &str) -> String {
  let text = text.replace("\r\n", "\n").replace('\r', "\n");
  if newline == "\r\n" {
    text.replace('\n', "\r\n")
  } else {
    text
  }
}

fn checked_line(line: usize, count: usize) -> Result<usize, String> {
  if line == 0 || line > count {
    return Err(format!("line {line} is outside the input (1..={count})"));
  }
  Ok(line - 1)
}

fn checked_range(start: usize, end: usize, count: usize) -> Result<(usize, usize), String> {
  if start == 0 || end == 0 || start > end {
    return Err(format!("invalid line range {start}..={end}"));
  }
  Ok((checked_line(start, count)?, checked_line(end, count)?))
}

/// One script operation from JSON (`{"op": "replace", ...}`); unknown keys are refused, optional
/// booleans default to false (as lmprobe's serde form).
pub fn parse_operation(value: &Value) -> Result<EditOperation, String> {
  let map = value
    .as_object()
    .ok_or("each script operation must be an object")?;
  let op = map
    .get("op")
    .and_then(Value::as_str)
    .ok_or("each script operation needs \"op\"")?;
  let allowed: &[&str] = match op {
    "replace" => &[
      "op",
      "pattern",
      "replacement",
      "regex",
      "word",
      "ignore_case",
      "first",
    ],
    "insert_before" | "insert_after" => &["op", "line", "text"],
    "delete_lines" => &["op", "start", "end"],
    "replace_lines" => &["op", "start", "end", "text"],
    "prepend" | "append" => &["op", "text"],
    other => return Err(format!("unknown script op {other:?}")),
  };
  if let Some(key) = map.keys().find(|k| !allowed.contains(&k.as_str())) {
    return Err(format!("unknown field {key:?} in a {op} operation"));
  }
  let text = |key: &str| -> Result<String, String> {
    map
      .get(key)
      .and_then(Value::as_str)
      .map(str::to_owned)
      .ok_or(format!("{op} needs a string \"{key}\""))
  };
  let number = |key: &str| -> Result<usize, String> {
    map
      .get(key)
      .and_then(Value::as_u64)
      .map(|n| n as usize)
      .ok_or(format!("{op} needs a whole number \"{key}\""))
  };
  let flag = |map: &Map<String, Value>, key: &str| -> Result<bool, String> {
    match map.get(key) {
      None => Ok(false),
      Some(Value::Bool(b)) => Ok(*b),
      Some(_) => Err(format!("{op}.{key} must be true or false")),
    }
  };
  Ok(match op {
    "replace" => EditOperation::Replace {
      pattern: text("pattern")?,
      replacement: text("replacement")?,
      regex: flag(map, "regex")?,
      word: flag(map, "word")?,
      ignore_case: flag(map, "ignore_case")?,
      first: flag(map, "first")?,
    },
    "insert_before" => EditOperation::InsertBefore {
      line: number("line")?,
      text: text("text")?,
    },
    "insert_after" => EditOperation::InsertAfter {
      line: number("line")?,
      text: text("text")?,
    },
    "delete_lines" => EditOperation::DeleteLines {
      start: number("start")?,
      end: number("end")?,
    },
    "replace_lines" => EditOperation::ReplaceLines {
      start: number("start")?,
      end: number("end")?,
      text: text("text")?,
    },
    "prepend" => EditOperation::Prepend {
      text: text("text")?,
    },
    _ => EditOperation::Append {
      text: text("text")?,
    },
  })
}

/// A JSON script: a non-empty array of operations.
pub fn parse_script(json: &str) -> Result<Vec<EditOperation>, String> {
  let value: Value =
    serde_json::from_str(json).map_err(|e| format!("script is not valid JSON: {e}"))?;
  let items = value.as_array().ok_or("script must be a JSON array")?;
  if items.is_empty() {
    return Err("edit script must contain at least one operation".into());
  }
  items.iter().map(parse_operation).collect()
}

/// `START:END` or `N` (an inclusive 1-based range).
pub fn parse_range(input: &str) -> Result<(usize, usize), String> {
  let (start, end) = input.split_once(':').unwrap_or((input, input));
  Ok((
    start.trim().parse().map_err(|_| "invalid start line")?,
    end.trim().parse().map_err(|_| "invalid end line")?,
  ))
}

#[cfg(test)]
mod tests {
  //! Ported from lmprobe's text_edit tests (same cases and expectations).
  use super::*;

  fn t(input: &str, ops: &[EditOperation]) -> Result<String, String> {
    transform(input, ops).map(|r| r.text)
  }

  fn rep(
    pattern: &str,
    replacement: &str,
    regex: bool,
    word: bool,
    ic: bool,
    first: bool,
  ) -> EditOperation {
    EditOperation::Replace {
      pattern: pattern.into(),
      replacement: replacement.into(),
      regex,
      word,
      ignore_case: ic,
      first,
    }
  }

  #[test]
  fn final_line_replacement_normalizes_internal_newlines_without_adding_one() {
    for (text, expected) in [
      ("alpha\nbeta", "one\r\nalpha\r\nbeta"),
      ("alpha\r\nbeta\n", "one\r\nalpha\r\nbeta\r\n"),
    ] {
      let op = EditOperation::ReplaceLines {
        start: 2,
        end: 2,
        text: text.into(),
      };
      assert_eq!(t("one\r\ntwo", &[op]).unwrap(), expected);
    }
  }

  #[test]
  fn literal_dollar_is_not_a_capture() {
    assert_eq!(
      t("old old", &[rep("old", "$1", false, false, false, false)]).unwrap(),
      "$1 $1"
    );
  }

  #[test]
  fn regex_captures_and_first() {
    let r = transform(
      "old1 old2",
      &[rep(r"(old)([0-9])", "$2-$1", true, false, false, true)],
    )
    .unwrap();
    assert_eq!((r.text.as_str(), r.replacements), ("1-old old2", 1));
  }

  #[test]
  fn whole_word_is_unicode_aware() {
    let r = transform(
      "Café caféine café",
      &[rep("café", "tea", false, true, true, false)],
    )
    .unwrap();
    assert_eq!((r.text.as_str(), r.replacements), ("tea caféine tea", 2));
  }

  #[test]
  fn sequential_line_operations_preserve_crlf() {
    let ops = [
      EditOperation::InsertAfter {
        line: 1,
        text: "middle".into(),
      },
      EditOperation::ReplaceLines {
        start: 3,
        end: 3,
        text: "tail".into(),
      },
    ];
    assert_eq!(
      t("one\r\ntwo\r\nthree", &ops).unwrap(),
      "one\r\nmiddle\r\ntail\r\nthree"
    );
  }

  #[test]
  fn line_ranges_validate_and_insert_does_not_join_lines() {
    assert!(t(
      "one\ntwo",
      &[EditOperation::DeleteLines { start: 2, end: 1 }]
    )
    .is_err());
    let op = EditOperation::InsertBefore {
      line: 2,
      text: "new".into(),
    };
    assert_eq!(t("one\ntwo", &[op]).unwrap(), "one\nnew\ntwo");
  }

  #[test]
  fn prepend_and_append_are_exact_byte_concatenations() {
    let ops = [
      EditOperation::Prepend { text: "<".into() },
      EditOperation::Append { text: ">".into() },
    ];
    assert_eq!(t("body\r\n", &ops).unwrap(), "<body\r\n>");
  }

  #[test]
  fn regex_captures_are_expanded_from_the_original_context() {
    let r = transform(
      "old\noldish\nold",
      &[rep(r"(?m)^old$", "new", true, false, false, false)],
    )
    .unwrap();
    assert_eq!((r.text.as_str(), r.replacements), ("new\noldish\nnew", 2));
  }

  #[test]
  fn script_parsing_rejects_unknown_fields_and_defaults_flags() {
    assert_eq!(
      parse_script(r#"[{"op":"replace","pattern":"a","replacement":"b"}]"#).unwrap(),
      vec![rep("a", "b", false, false, false, false)]
    );
    assert!(parse_script(r#"[{"op":"prepend","text":"x","extra":true}]"#).is_err());
    assert!(parse_script("[]").is_err());
    assert!(parse_script(r#"[{"op":"rotate"}]"#).is_err());
    assert!(parse_script(r#"{"op":"append","text":"x"}"#).is_err());
  }

  #[test]
  fn empty_input_supports_exact_prefix_and_suffix() {
    let ops = [
      EditOperation::Prepend { text: "a".into() },
      EditOperation::Append { text: "b".into() },
    ];
    assert_eq!(t("", &ops).unwrap(), "ab");
    assert_eq!(t("", &[]).unwrap(), "");
  }

  #[test]
  fn line_zero_and_out_of_bounds_ranges_are_rejected() {
    for op in [
      EditOperation::InsertBefore {
        line: 0,
        text: "x".into(),
      },
      EditOperation::InsertAfter {
        line: 3,
        text: "x".into(),
      },
      EditOperation::DeleteLines { start: 1, end: 3 },
      EditOperation::ReplaceLines {
        start: 0,
        end: 1,
        text: "x".into(),
      },
    ] {
      assert!(t("one\ntwo", &[op]).is_err());
    }
  }

  #[test]
  fn insert_after_unterminated_last_line_adds_one_separator() {
    let op = EditOperation::InsertAfter {
      line: 1,
      text: "added".into(),
    };
    assert_eq!(t("last", &[op]).unwrap(), "last\nadded\n");
  }

  #[test]
  fn deleting_all_lines_from_a_terminated_file_leaves_empty_text() {
    assert_eq!(
      t(
        "one\ntwo\n",
        &[EditOperation::DeleteLines { start: 1, end: 2 }]
      )
      .unwrap(),
      ""
    );
  }

  #[test]
  fn sequential_line_edits_use_the_shifted_line_numbers() {
    let ops = [
      EditOperation::InsertBefore {
        line: 1,
        text: "x".into(),
      },
      EditOperation::DeleteLines { start: 3, end: 3 },
    ];
    assert_eq!(t("a\nb\nc", &ops).unwrap(), "x\na\nc");
  }

  #[test]
  fn literal_replacement_escapes_regex_punctuation() {
    assert_eq!(
      t("a.* a.*", &[rep("a.*", "done", false, false, false, false)]).unwrap(),
      "done done"
    );
  }

  #[test]
  fn whole_word_boundaries_include_adjacent_combining_marks() {
    for (input, pattern, expected) in [
      (
        " café\u{301} café\u{301}x",
        "café\u{301}",
        " tea café\u{301}x",
      ),
      (" café\u{301} café ", "café", " café\u{301} tea "),
      (" \u{301}café café ", "café", " \u{301}café tea "),
    ] {
      assert_eq!(
        t(input, &[rep(pattern, "tea", false, true, false, false)]).unwrap(),
        expected
      );
    }
  }

  #[test]
  fn named_regex_captures_expand_escaped_dollar_text() {
    let op = rep(
      r"(?P<name>\w+):(?P<id>\d+)",
      "${name}=$$${id}",
      true,
      false,
      false,
      false,
    );
    assert_eq!(t("user:42", &[op]).unwrap(), "user=$42");
  }

  #[test]
  fn regex_zero_width_anchors_replace_each_matching_line() {
    assert_eq!(
      t("a\nb", &[rep(r"(?m)^", ">", true, false, false, false)]).unwrap(),
      ">a\n>b"
    );
  }

  #[test]
  fn malformed_regex_and_empty_pattern_report_errors() {
    assert!(t("text", &[rep("[", "x", true, false, false, false)]).is_err());
    assert!(t("text", &[rep("", "x", false, false, false, false)]).is_err());
  }

  #[test]
  fn ranges() {
    assert_eq!(parse_range("2:4"), Ok((2, 4)));
    assert_eq!(parse_range("7"), Ok((7, 7)));
    assert!(parse_range("a:2").is_err());
  }
}
