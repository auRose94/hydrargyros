//! HTML5 rendering — the element builder and markup parser, as data.
//!
//! **Builder.** Elements are data: a `tag`, an attribute object, and a list
//! of child nodes. Constructors take one list of children, text, and
//! attribute bundles (merged key by key); an `items` key inside a bundle
//! routes to the children instead of becoming an attribute. On serialize,
//! text and attribute values are escaped — except inside `<script>` /
//! `<style>`, which round-trip their raw source — and void tags
//! (`<br>`, `<img>`, …) never emit end tags.
//!
//! ```
//! use hyg::web::html::{self as h, Node};
//!
//! let card = h::div([
//!     h::h2([Node::text("Title")]),
//!     h::p([Node::text("Fish & <Chips>")]),
//!     Node::attrs(hyg::o!{ "class": "card", "id": "main" }),
//! ]);
//! assert_eq!(
//!     card.render(),
//!     r#"<div class="card" id="main"><h2>Title</h2><p>Fish &amp; &lt;Chips&gt;</p></div>"#
//! );
//! ```
//!
//! **Parser.** [`parse`] turns markup back into nodes: nested trees,
//! attribute forms (quoted, unquoted, boolean), void/self-closing tags,
//! implicit closes (`<li>` before `<li>`, block elements before `<p>`,
//! table rows and cells, nested `<a>`), character references — and malformed
//! input is recovered from, never fatal: unterminated tags, comments, raw
//! text and entities all degrade to ordinary content.

use std::fmt;

use crate::object::Object;
use crate::var::Var;

/// One node of an HTML tree.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    /// A child element.
    Element(Element),
    /// A text node (source text in the builder; decoded in the parser).
    Text(String),
    /// An attribute bundle, merged into the element being built.
    Attrs(Object),
}

impl Node {
    /// A text node.
    pub fn text(s: impl Into<String>) -> Node { Node::Text(s.into()) }

    /// An attribute bundle.
    pub fn attrs(o: Object) -> Node { Node::Attrs(o) }

    /// The inner element, if this is one.
    pub fn as_element(&self) -> Option<&Element> {
        match self { Node::Element(e) => Some(e), _ => None }
    }

    /// The inner element (panics on text/attribute nodes — the crate's
    /// loud-misuse convention, like `Var::call` on a non-function).
    pub fn element(&self) -> &Element {
        self.as_element().unwrap_or_else(|| panic!("{self:?} is not an element"))
    }

    /// Serialize this node: elements render their subtree, text escapes.
    pub fn render(&self) -> String {
        match self {
            Node::Element(e) => e.render(),
            Node::Text(t) => escape_html(t, false),
            Node::Attrs(_) => String::new(),
        }
    }

    /// The tag name (panics on a non-element node).
    pub fn tag(&self) -> &str { self.element().tag() }

    /// The live attribute object (panics on a non-element node).
    pub fn attr(&self) -> &Object { self.element().attr() }

    /// The live attribute object, mutable (panics on a non-element node).
    pub fn attr_mut(&mut self) -> &mut Object { self.element_mut().attr_mut() }

    /// The children (panics on a non-element node).
    pub fn items(&self) -> &[Node] { self.element().items() }

    /// Append/merge onto the inner element (panics on a non-element node).
    pub fn push(&mut self, node: Node) { self.element_mut().push(node); }

    /// Merge an attribute bundle into the inner element.
    pub fn merge_attrs(&mut self, attrs: &Object) { self.element_mut().merge_attrs(attrs); }

    /// Remove the first structurally-equal child element.
    pub fn remove_element(&mut self, target: &Node) { self.element_mut().remove_element(target); }

    /// The node as a `Var` (elements: `{ "tag", "attr", "items" }`).
    pub fn to_var(&self) -> Var {
        match self {
            Node::Element(e) => e.to_var(),
            Node::Text(t) => Var::from(t.clone()),
            Node::Attrs(o) => Var::from(o.clone()),
        }
    }

    fn element_mut(&mut self) -> &mut Element {
        match self {
            Node::Element(e) => e,
            other => panic!("{other:?} is not an element"),
        }
    }
}

/// An HTML element: tag, attribute object, children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Element {
    tag: String,
    attr: Object,
    items: Vec<Node>,
}

impl Element {
    /// Build an element: child elements and strings append to the children;
    /// attribute bundles merge key by key, with an `items` key (a list)
    /// routing to the children instead.
    pub fn new(tag: &str, nodes: impl IntoIterator<Item = Node>) -> Element {
        let mut el = Element { tag: tag.to_ascii_lowercase(), attr: Object::new(), items: Vec::new() };
        for node in nodes {
            el.push(node);
        }
        el
    }

    pub fn tag(&self) -> &str { &self.tag }

    /// The live attribute object.
    pub fn attr(&self) -> &Object { &self.attr }

    pub fn attr_mut(&mut self) -> &mut Object { &mut self.attr }

    /// The children.
    pub fn items(&self) -> &[Node] { &self.items }

    /// Append a child (element/text) or merge an attribute bundle — child
    /// append and attribute merge in one method.
    pub fn push(&mut self, node: Node) {
        match node {
            Node::Attrs(attrs) => self.merge_attrs(&attrs),
            other => self.items.push(other),
        }
    }

    /// Merge an attribute bundle; an `items` list value routes to children.
    pub fn merge_attrs(&mut self, attrs: &Object) {
        for (k, v) in attrs.entries() {
            if k == "items" {
                if let Some(list) = v.as_list() {
                    for item in list.iter() {
                        if let Some(s) = item.as_string() {
                            self.items.push(Node::Text(s));
                        }
                    }
                }
            } else {
                self.attr.set(k, v);
            }
        }
    }

    /// Remove the first structurally-equal child element.
    pub fn remove_element(&mut self, target: &Node) {
        self.items.retain(|n| n != target);
    }

    /// The element as a `Var`: `{ "tag", "attr", "items" }`.
    pub fn to_var(&self) -> Var {
        let items = crate::list::List::from_iter(self.items.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e.to_var()),
            Node::Text(t) => Some(Var::from(t.clone())),
            Node::Attrs(_) => None,
        }));
        let o = Object::new();
        o.set("tag", Var::from(self.tag.clone()));
        o.set("attr", Var::from(self.attr.clone()));
        o.set("items", Var::from(items));
        Var::from(o)
    }

    /// Serialize to markup: attributes typed (bools render bare when true
    /// and vanish when false), values escaped, void tags end-tag-free, and
    /// script/style bodies verbatim.
    pub fn render(&self) -> String {
        let mut out = format!("<{}", self.tag);
        for (k, v) in self.attr.entries() {
            match &v {
                Var::Bool(true) => out.push_str(&format!(" {k}")),
                Var::Bool(false) => {}
                Var::Str(s) => out.push_str(&format!(" {k}=\"{}\"", escape_html(s, true))),
                Var::I64(n) => out.push_str(&format!(" {k}=\"{n}\"")),
                Var::F64(n) => out.push_str(&format!(" {k}=\"{n}\"")),
                other => out.push_str(&format!(" {k}=\"{other}\"")),
            }
        }
        out.push('>');
        if is_void_tag(&self.tag) {
            return out;
        }
        let raw = self.tag == "script" || self.tag == "style";
        for node in &self.items {
            match node {
                Node::Element(e) => out.push_str(&e.render()),
                Node::Text(t) => {
                    out.push_str(&if raw { t.clone() } else { escape_html(t, false) });
                }
                Node::Attrs(_) => {}
            }
        }
        out.push_str(&format!("</{}>", self.tag));
        out
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.render()) }
}

// ---- tag constructors (the common set; use Element::new for the rest) ----
//
// Constructors return Node — the universal item — so children, text, and
// attribute bundles mix freely in one list.

macro_rules! tags {
    ($($name:ident),* $(,)?) => {
        $(
            #[doc = concat!("An `<", stringify!($name), ">` element, as a `Node`.")]
            pub fn $name(nodes: impl IntoIterator<Item = Node>) -> Node {
                Node::Element(Element::new(stringify!($name), nodes))
            }
        )*
    };
}

tags!(
    html, head, title, body, main, nav, header, footer, section, article, aside,
    div, span, p, a, h1, h2, h3, h4, h5, h6, ul, ol, li, dl, dt, dd, table, tr, td, th,
    thead, tbody, tfoot, form, label, button, select, option, optgroup, fieldset,
    pre, code, em, strong, small, blockquote, script, style, textarea,
);

/// `<br>` — void.
pub fn br() -> Node { Node::Element(Element::new("br", [])) }
/// `<hr>` — void.
pub fn hr() -> Node { Node::Element(Element::new("hr", [])) }
/// `<img>` with attributes (src etc.) — void.
pub fn img(attrs: impl IntoIterator<Item = Node>) -> Node { Node::Element(Element::new("img", attrs)) }
/// `<input>` with attributes — void.
pub fn input(attrs: impl IntoIterator<Item = Node>) -> Node { Node::Element(Element::new("input", attrs)) }
/// `<meta>` with attributes — void.
pub fn meta(attrs: impl IntoIterator<Item = Node>) -> Node { Node::Element(Element::new("meta", attrs)) }
/// `<link>` with attributes — void.
pub fn link(attrs: impl IntoIterator<Item = Node>) -> Node { Node::Element(Element::new("link", attrs)) }

// ---- escaping and entities (escape_html / decode_entities) ----

/// Escape markup-significant characters; in attribute mode `"` becomes
/// `&quot;` too.
pub fn escape_html(s: &str, attribute: bool) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// The named set: the five the spec requires plus practicals that show up in
/// real text. Unknown or truncated entities survive as written.
const ENTITIES: &[(&str, &str)] = &[
    ("amp", "&"),
    ("lt", "<"),
    ("gt", ">"),
    ("quot", "\""),
    ("apos", "'"),
    ("nbsp", "\u{a0}"),
    ("shy", "\u{ad}"),
    ("copy", "\u{a9}"),
    ("reg", "\u{ae}"),
    ("trade", "\u{2122}"),
    ("hellip", "\u{2026}"),
    ("mdash", "\u{2014}"),
    ("ndash", "\u{2013}"),
    ("lsquo", "\u{2018}"),
    ("rsquo", "\u{2019}"),
    ("ldquo", "\u{201c}"),
    ("rdquo", "\u{201d}"),
    ("bull", "\u{2022}"),
    ("middot", "\u{b7}"),
    ("times", "\u{d7}"),
    ("divide", "\u{f7}"),
    ("deg", "\u{b0}"),
    ("plusmn", "\u{b1}"),
    ("frac12", "\u{bd}"),
    ("frac14", "\u{bc}"),
];

/// Decode `&...;` character references: the named set above, decimal
/// (`&#65;`) and hex (`&#x42;`). Unknown or truncated references survive
/// as written.
pub fn decode_entities(src: &str) -> String {
    let b = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'&' {
            if let Some((repl, next)) = decode_one(src, i) {
                out.extend_from_slice(repl.as_bytes());
                i = next;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode one `&...;` at `at`; `None` keeps the source characters as-is.
fn decode_one(src: &str, at: usize) -> Option<(String, usize)> {
    let b = src.as_bytes();
    let semi = b[at..].iter().position(|&c| c == b';')? + at;
    if semi == at + 1 || semi - at > 12 {
        return None;
    }
    let body = &src[at + 1..semi];
    if let Some(num) = body.strip_prefix('#') {
        let cp = if let Some(hex) = num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            num.parse::<u32>().ok()?
        };
        if cp == 0 || cp > 0x10FFFF || (0xD800..=0xDFFF).contains(&cp) {
            return None;
        }
        let c = char::from_u32(cp)?;
        return Some((c.to_string(), semi + 1));
    }
    ENTITIES
        .iter()
        .find(|(name, _)| *name == body)
        .map(|(_, repl)| (repl.to_string(), semi + 1))
}

// ---- void tags (VOID_TAGS / is_void_tag) ----

/// Tags that never have children or end tags.
pub const VOID_TAGS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta",
    "param", "source", "track", "wbr",
];

/// True when `tag` (any case) is a void element.
pub fn is_void_tag(tag: &str) -> bool {
    let t = tag.trim().to_ascii_lowercase();
    VOID_TAGS.contains(&t.as_str())
}

// ---- parser ----

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | b'\x0c' | b'\x0b')
}

/// Raw-text elements: content is source text up to the matching close tag.
/// `textarea`/`title` still decode entities; `script`/`style` stay verbatim.
fn is_raw_text_tag(tag: &str) -> bool {
    matches!(tag, "script" | "style" | "textarea" | "title")
}

/// Block-level elements implicitly close an open `<p>`.
fn is_block_tag(tag: &str) -> bool {
    matches!(
        tag,
        "address" | "article" | "aside" | "blockquote" | "details" | "div" | "dl"
            | "fieldset" | "figcaption" | "figure" | "footer" | "form" | "hgroup"
            | "main" | "menu" | "nav" | "ol" | "pre" | "section" | "table" | "ul"
    ) || matches!(tag.as_bytes(), [b'h', b'1'..=b'6'])
}

/// An opening tag that implicitly closes `existing` (walking the stack) —
/// a table covering `<p>`, list items, definition lists, options, links,
/// and the table elements.
fn closes_tag(open: &str, existing: &str) -> bool {
    match existing {
        "p" => is_block_tag(open) || open == "p",
        "li" => open == "li",
        "dt" | "dd" => open == "dt" || open == "dd",
        "option" => open == "option" || open == "optgroup",
        "optgroup" => open == "optgroup",
        "a" => open == "a",
        "tr" => matches!(open, "tr" | "td" | "th" | "thead" | "tbody" | "tfoot"),
        "td" | "th" => matches!(open, "td" | "th" | "tr" | "thead" | "tbody" | "tfoot"),
        "thead" | "tbody" | "tfoot" => matches!(open, "thead" | "tbody" | "tfoot"),
        _ => false,
    }
}

fn attach(stack: &mut [Element], roots: &mut Vec<Node>, node: Node) {
    match stack.last_mut() {
        Some(top) => top.items.push(node),
        None => roots.push(node),
    }
}

fn attach_el(stack: &mut [Element], roots: &mut Vec<Node>, el: Element) {
    attach(stack, roots, Node::Element(el));
}

/// Parse HTML into top-level nodes (elements and text). Malformed input
/// never hangs or throws: everything degrades to ordinary content.
pub fn parse(src: &str) -> Vec<Node> {
    let b = src.as_bytes();
    let mut roots: Vec<Node> = Vec::new();
    let mut stack: Vec<Element> = Vec::new();
    let mut i = 0;

    while i < b.len() {
        if b[i] == b'<' {
            if b[i..].starts_with(b"<!--") {
                // unterminated comment swallows the rest
                match find(b, b"-->", i + 4) {
                    Some(end) => i = end + 3,
                    None => i = b.len(),
                }
                continue;
            }
            if b[i..].starts_with(b"<!") {
                i = skip_to_gt(b, i);
                continue;
            }
            if b[i..].starts_with(b"</") {
                i = close_tag(src, i, &mut stack, &mut roots);
                continue;
            }
            if i + 1 < b.len() && b[i + 1].is_ascii_alphabetic() {
                i = open_tag(src, i, &mut stack, &mut roots);
                continue;
            }
            // a stray '<' is ordinary text
        }
        let start = i;
        while i < b.len() && b[i] != b'<' {
            i += 1;
        }
        let text = decode_entities(&src[start..i]);
        if !text.is_empty() {
            attach(&mut stack, &mut roots, Node::Text(text));
        }
    }

    while let Some(el) = stack.pop() {
        attach_el(&mut stack, &mut roots, el);
    }
    roots
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    hay.windows(needle.len().max(1))
        .skip(from)
        .position(|w| w == needle)
        .map(|p| p + from)
}

fn skip_to_gt(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && b[i] != b'>' {
        i += 1;
    }
    (i + 1).min(b.len())
}

fn close_tag(src: &str, mut i: usize, stack: &mut Vec<Element>, roots: &mut Vec<Node>) -> usize {
    let b = src.as_bytes();
    i += 2;
    let name_start = i;
    while i < b.len() && b[i].is_ascii_alphanumeric() {
        i += 1;
    }
    let name = src[name_start..i].to_ascii_lowercase();
    i = skip_to_gt(b, i);

    if let Some(depth) = stack.iter().rposition(|e| e.tag == name) {
        // a close tag pops everything still open inside it; a stray close
        // (no matching open) is ignored
        while stack.len() > depth + 1 {
            let el = stack.pop().unwrap();
            attach_el(stack, roots, el);
        }
        let el = stack.pop().unwrap();
        attach_el(stack, roots, el);
    }
    i
}

fn open_tag(src: &str, mut i: usize, stack: &mut Vec<Element>, roots: &mut Vec<Node>) -> usize {
    let b = src.as_bytes();
    i += 1;
    let name_start = i;
    while i < b.len() && b[i].is_ascii_alphanumeric() {
        i += 1;
    }
    let tag = src[name_start..i].to_ascii_lowercase();

    // scan to the tag's '>', honoring quoted attribute values
    let mut j = i;
    let mut in_quote: Option<u8> = None;
    while j < b.len() {
        match (in_quote, b[j]) {
            (Some(q), c) if c == q => in_quote = None,
            (None, q @ (b'"' | b'\'')) => in_quote = Some(q),
            (None, b'>') => break,
            _ => {}
        }
        j += 1;
    }
    let inner_end = j.min(b.len());
    let inner_raw = &src[i..inner_end];
    let inner = inner_raw.strip_suffix('/').unwrap_or(inner_raw);
    let self_closing = inner_raw.len() != inner.len();
    let attr = parse_attrs(inner);

    // implicit closes walk the stack (a `<p>` before a block, `<li>` before
    // `<li>`, nested `<a>`, table rows/cells/sections, …)
    loop {
        let closes = stack.last().is_some_and(|top| closes_tag(&tag, &top.tag));
        if !closes {
            break;
        }
        let el = stack.pop().unwrap();
        attach_el(stack, roots, el);
    }

    let end = (inner_end + 1).min(b.len());
    if is_raw_text_tag(&tag) {
        let mut el = Element { tag: tag.clone(), attr, items: Vec::new() };
        let close_start = find_close_tag(b, end, &tag);
        let content_end = close_start.unwrap_or(b.len());
        let content = &src[end..content_end];
        let text = if tag == "script" || tag == "style" {
            content.to_string()
        } else {
            decode_entities(content)
        };
        el.items.push(Node::Text(text));
        if let Some(cs) = close_start {
            i = skip_to_gt(b, cs);
        } else {
            i = b.len();
        }
        attach_el(stack, roots, el);
        return i;
    }

    let el = Element { tag: tag.clone(), attr, items: Vec::new() };
    if self_closing || is_void_tag(&tag) {
        attach_el(stack, roots, el);
        return end;
    }
    stack.push(el);
    end
}

fn find_close_tag(b: &[u8], from: usize, tag: &str) -> Option<usize> {
    let needle = format!("</{tag}");
    let nb = needle.as_bytes();
    let mut i = from;
    while i + nb.len() <= b.len() {
        if b[i..i + nb.len()].eq_ignore_ascii_case(nb) {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Parse the attribute section of a start tag: quoted, unquoted, and boolean
/// forms; names lowercase; values always strings, entity-decoded.
fn parse_attrs(inner: &str) -> Object {
    let o = Object::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let name_start = i;
        while i < b.len() && !is_space(b[i]) && b[i] != b'=' && b[i] != b'/' {
            i += 1;
        }
        if i == name_start {
            i += 1;
            continue;
        }
        let name = inner[name_start..i].to_ascii_lowercase();

        let save = i;
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            // boolean attribute (e.g. `disabled`)
            o.set(name, Var::Bool(true));
            i = save;
            continue;
        }
        i += 1;
        while i < b.len() && is_space(b[i]) {
            i += 1;
        }
        let value = if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
            let quote = b[i];
            i += 1;
            let start = i;
            while i < b.len() && b[i] != quote {
                i += 1;
            }
            let v = inner[start..i.min(b.len())].to_string();
            if i < b.len() {
                i += 1;
            }
            v
        } else {
            let start = i;
            while i < b.len() && !is_space(b[i]) && b[i] != b'>' {
                i += 1;
            }
            inner[start..i].to_string()
        };
        o.set(name, Var::from(decode_entities(&value)));
    }
    o
}
