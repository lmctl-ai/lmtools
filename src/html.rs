//! `webget --format text`: HTML to readable plain text. Simple and deterministic, not a renderer:
//! drops `<script>`, `<style>`, `<noscript>`, `<template>`, comments and all tags; block-level
//! tags become line breaks, `<li>` a "- " bullet; the common character references are decoded;
//! spaces are collapsed within lines and blank lines to at most one.

const DROP: [&str; 4] = ["script", "style", "noscript", "template"];
const BLOCK: [&str; 26] = [
  "p",
  "div",
  "br",
  "hr",
  "h1",
  "h2",
  "h3",
  "h4",
  "h5",
  "h6",
  "ul",
  "ol",
  "li",
  "tr",
  "table",
  "section",
  "article",
  "header",
  "footer",
  "nav",
  "aside",
  "main",
  "pre",
  "blockquote",
  "dt",
  "dd",
];

fn entity(name: &str) -> Option<String> {
  if let Some(num) = name.strip_prefix('#') {
    let code = match num.strip_prefix(['x', 'X']) {
      Some(hex) => u32::from_str_radix(hex, 16).ok()?,
      None => num.parse().ok()?,
    };
    return char::from_u32(code).map(String::from);
  }
  Some(
    match name {
      "amp" => "&",
      "lt" => "<",
      "gt" => ">",
      "quot" => "\"",
      "apos" => "'",
      "nbsp" => " ",
      "copy" => "©",
      "reg" => "®",
      "mdash" => "—",
      "ndash" => "–",
      "hellip" => "…",
      "laquo" => "«",
      "raquo" => "»",
      _ => return None,
    }
    .to_string(),
  )
}

/// Decode character references in text.
fn decode(text: &str, out: &mut String) {
  let mut rest = text;
  while let Some(at) = rest.find('&') {
    out.push_str(&rest[..at]);
    let tail = &rest[at + 1..];
    match tail.find(';').filter(|&end| end <= 10) {
      Some(end) => match entity(&tail[..end]) {
        Some(decoded) => {
          out.push_str(&decoded);
          rest = &tail[end + 1..];
        }
        None => {
          out.push('&');
          rest = tail;
        }
      },
      None => {
        out.push('&');
        rest = tail;
      }
    }
  }
  out.push_str(rest);
}

/// The tag name of `<...>` content (`/p`, `br/`, `div class=x` -> `p`, `br`, `div`), lower-cased.
fn tag_name(inner: &str) -> (String, bool) {
  let closing = inner.starts_with('/');
  let name: String = inner
    .trim_start_matches('/')
    .chars()
    .take_while(|c| c.is_ascii_alphanumeric())
    .collect();
  (name.to_ascii_lowercase(), closing)
}

pub fn to_text(html: &str) -> String {
  let mut raw = String::with_capacity(html.len() / 2);
  let lower = html.to_ascii_lowercase();
  let mut i = 0;
  while i < html.len() {
    let Some(rel) = html[i..].find('<') else {
      decode(&html[i..], &mut raw);
      break;
    };
    decode(&html[i..i + rel], &mut raw);
    i += rel;
    if html[i..].starts_with("<!--") {
      i = html[i..].find("-->").map_or(html.len(), |e| i + e + 3);
      continue;
    }
    let Some(end) = html[i..].find('>') else {
      decode(&html[i..], &mut raw);
      break;
    };
    let (name, closing) = tag_name(&html[i + 1..i + end]);
    i += end + 1;
    if !closing && DROP.contains(&name.as_str()) {
      // Skip to the matching close tag.
      let close = format!("</{name}");
      i = lower[i..].find(&close).map_or(html.len(), |e| {
        let after = i + e;
        html[after..]
          .find('>')
          .map_or(html.len(), |g| after + g + 1)
      });
      continue;
    }
    if name == "li" {
      if !closing {
        raw.push_str("\n- ");
      }
    } else if BLOCK.contains(&name.as_str()) {
      raw.push('\n');
    } else if name == "td" || name == "th" {
      raw.push(' ');
    }
  }
  // Collapse whitespace: within lines to single spaces, blank lines to one.
  let mut out = String::with_capacity(raw.len());
  let mut blank = 0;
  for line in raw.lines() {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.is_empty() {
      blank += 1;
      continue;
    }
    if !out.is_empty() {
      out.push_str(if blank > 0 { "\n\n" } else { "\n" });
    }
    blank = 0;
    out.push_str(&words.join(" "));
  }
  if !out.is_empty() {
    out.push('\n');
  }
  out
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn strips_tags_scripts_and_styles() {
    let html = "<html><head><title>T</title><style>p{color:red}</style>\
      <script>var a = '<p>';</script></head><body><h1>Title &amp; more</h1>\
      <p>One   two<br>three</p><!-- hidden --><ul><li>a</li><li>b &lt;c&gt;</li></ul>\
      <p>x&nbsp;y &#233; &#x263A; &bogus; AT&T</p></body></html>";
    assert_eq!(
      to_text(html),
      "T\nTitle & more\n\nOne two\nthree\n\n- a\n- b <c>\n\nx y é ☺ &bogus; AT&T\n"
    );
  }

  #[test]
  fn plain_text_and_empty_input() {
    assert_eq!(to_text("just text"), "just text\n");
    assert_eq!(to_text(""), "");
    assert_eq!(to_text("<SCRIPT>x</SCRIPT>ok"), "ok\n");
  }
}
