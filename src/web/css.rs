//! CSS parsing — stylesheets as data.
//!
//! [`parse_css`] turns a stylesheet into rules (`selector` + typed
//! `declarations`): numeric values stay typed (`margin: 0` → `0`),
//! everything else is a string (`width: 50%`), `!important` is recorded as a
//! boolean under the key `name + "!"` so values keep their type, and quoted
//! values survive `;`/`}`/`,` inside them. At-rules (`@media`, `@keyframes`,
//! …) are skipped whole — block forms balance their braces, statement forms
//! end at `;`.
//!
//! ```
//! use hyg::web::css;
//!
//! let rules = css::parse_css("/* c */ p, .x { color: red; margin : 0 }");
//! assert_eq!(rules.len(), 1);
//! assert_eq!(rules[0].selector, "p, .x");
//! assert_eq!(rules[0].declarations.get("color").as_str(), Some("red"));
//! assert_eq!(rules[0].declarations.get("margin").as_i64(), Some(0));
//! ```

use crate::object::Object;
use crate::var::Var;

/// One stylesheet rule: the selector text and its typed declarations.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub selector: String,
    pub declarations: Object,
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b')
}

/// Skip whitespace and `/* … */` comments from `pos`.
fn skip_inert(src: &str, pos: &mut usize) {
    let b = src.as_bytes();
    loop {
        while *pos < b.len() && is_space(b[*pos]) {
            *pos += 1;
        }
        if *pos + 1 < b.len() && b[*pos] == b'/' && b[*pos + 1] == b'*' {
            match src[*pos + 2..].find("*/") {
                Some(end) => *pos += 2 + end + 2,
                None => *pos = b.len(),
            }
            continue;
        }
        break;
    }
}

/// True when the trimmed value is a plain number (`-3`, `12.5`), so it can
/// stay typed instead of degrading to a string.
fn numeric_value(s: &str) -> Option<Var> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let digits = match t.as_bytes()[0] {
        b'-' | b'+' => &t[1..],
        _ => t,
    };
    if digits.is_empty() || !digits.bytes().all(|c| c.is_ascii_digit() || c == b'.') {
        return None;
    }
    if digits.matches('.').count() > 1 {
        return None;
    }
    if digits.contains('.') {
        t.parse::<f64>().ok().map(Var::F64)
    } else {
        t.parse::<i64>().ok().map(Var::I64)
    }
}

/// Parse a declaration block body: `name: value` pairs split at top-level
/// `;` (quotes and parens protect their contents). `!important` (any case)
/// is stripped from the value and recorded as `name!` → `true`.
pub fn parse_declarations(src: &str) -> Object {
    let o = Object::new();
    for part in split_top_level(src, b';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once(':') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let value = value.trim();
        let (value, important) = strip_important(value);
        let typed = numeric_value(value).unwrap_or_else(|| Var::from(value));
        o.set(name, typed);
        if important {
            o.set(format!("{name}!"), Var::Bool(true));
        }
    }
    o
}

fn strip_important(value: &str) -> (&str, bool) {
    if let Some(head) = value.strip_suffix("!important") {
        (head.trim_end(), true)
    } else if let Some(head) = value.strip_suffix("!IMPORTANT") {
        (head.trim_end(), true)
    } else {
        (value, false)
    }
}

/// Split at `sep` occurrences that sit outside quotes and parentheses.
fn split_top_level(src: &str, sep: u8) -> Vec<&str> {
    let b = src.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let mut quote: Option<u8> = None;
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        match (quote, b[i]) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, q @ (b'"' | b'\'')) => quote = Some(q),
            (None, b'(') => depth += 1,
            (None, b')') => depth = depth.saturating_sub(1),
            (None, c) if c == sep && depth == 0 => {
                parts.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&src[start..]);
    parts
}

/// Parse a whole stylesheet into rules. At-rules are skipped whole.
pub fn parse_css(src: &str) -> Vec<Rule> {
    let mut rules = Vec::new();
    let mut pos = 0;
    while pos < src.len() {
        let before = pos;
        if let Some(rule) = parse_rule(src, &mut pos) {
            rules.push(rule);
        }
        if pos == before {
            // safety against a non-advancing parse (malformed input degrades)
            pos += 1;
        }
    }
    rules
}

/// Streaming form: parse one rule starting at `pos`, advancing it past the
/// rule. At-rules are consumed and yield `None`; the stream position still
/// moves past them, so callers just keep calling.
pub fn parse_rule(src: &str, pos: &mut usize) -> Option<Rule> {
    let b = src.as_bytes();
    skip_inert(src, pos);
    if *pos >= b.len() {
        return None;
    }
    if b[*pos] == b'@' {
        // block forms balance their braces; statement forms end at ';'
        match find_outside_quotes(src, *pos, &['{', ';']) {
            Some(at) if b[at] == b';' => *pos = at + 1,
            Some(at) => *pos = matching_brace(src, at) + 1,
            None => *pos = b.len(),
        }
        return None;
    }

    let Some(sel_end) = find_outside_quotes(src, *pos, &['{']) else {
        *pos = b.len();
        return None;
    };
    let selector = src[*pos..sel_end].trim().to_string();
    let close = matching_brace(src, sel_end);
    let declarations = parse_declarations(&src[sel_end + 1..close]);
    *pos = (close + 1).min(b.len());
    Some(Rule { selector, declarations })
}

/// First index of any char in `set` outside quotes.
fn find_outside_quotes(src: &str, from: usize, set: &[char]) -> Option<usize> {
    let b = src.as_bytes();
    let mut quote: Option<u8> = None;
    for (i, &c) in b.iter().enumerate().skip(from) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, q @ (b'"' | b'\'')) => quote = Some(q),
            (None, c) if set.contains(&(c as char)) => return Some(i),
            _ => {}
        }
    }
    None
}

/// Index of the `}` balancing the `{` at `open` (quotes respected); end of
/// input when unbalanced — malformed input degrades, never panics.
fn matching_brace(src: &str, open: usize) -> usize {
    let b = src.as_bytes();
    let mut quote: Option<u8> = None;
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match (quote, b[i]) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, q @ (b'"' | b'\'')) => quote = Some(q),
            (None, b'{') => depth += 1,
            (None, b'}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}
