//! HTML builder + parser tests, ported from gold's webTest.cpp suite.

use hyg::web::html::{self as h, Node};
use hyg::{a, o};

#[test]
fn parser_nested_tree() {
    let roots = h::parse("<div class='foo'><p>Hello</p></div>");
    assert_eq!(roots.len(), 1);
    let Node::Element(div) = &roots[0] else { panic!("expected element") };
    assert_eq!(div.tag(), "div");
    assert_eq!(div.attr().get("class").as_str(), Some("foo"));
    let items = div.items();
    assert_eq!(items.len(), 1);
    let Node::Element(p) = &items[0] else { panic!("expected element") };
    assert_eq!(p.tag(), "p");
    let Node::Text(t) = &p.items()[0] else { panic!("expected text") };
    assert_eq!(t, "Hello");
}

#[test]
fn parser_round_trips_to_string() {
    let roots = h::parse("<div class='foo'><p>Hello</p></div>");
    let Node::Element(div) = &roots[0] else { panic!() };
    assert_eq!(div.render(), r#"<div class="foo"><p>Hello</p></div>"#);
}

#[test]
fn parser_attribute_forms() {
    let roots = h::parse("<input type=\"text\" name=q disabled data-x='1 2'>");
    let Node::Element(el) = &roots[0] else { panic!() };
    let attr = el.attr();
    assert_eq!(attr.get("type").as_str(), Some("text"));
    assert_eq!(attr.get("name").as_str(), Some("q"));
    assert_eq!(attr.get("disabled").as_bool(), Some(true));
    assert_eq!(attr.get("data-x").as_str(), Some("1 2"));
}

#[test]
fn parser_void_and_self_closing() {
    let roots = h::parse("<div><br><img src='a.png'><hr/></div>");
    let Node::Element(div) = &roots[0] else { panic!() };
    // void tags never nest, so all three stay direct children of <div>
    assert_eq!(div.items().len(), 3);
    let Node::Element(img) = &div.items()[1] else { panic!() };
    assert_eq!(img.tag(), "img");
}

#[test]
fn parser_skips_comments_and_doctype() {
    let roots = h::parse("<!DOCTYPE html><!-- hidden --><p>shown</p>");
    assert_eq!(roots.len(), 1);
    let Node::Element(p) = &roots[0] else { panic!() };
    let Node::Text(t) = &p.items()[0] else { panic!() };
    assert_eq!(t, "shown");
}

#[test]
fn parser_implicit_close() {
    let roots = h::parse("<ul><li>a<li>b</ul>");
    let Node::Element(ul) = &roots[0] else { panic!() };
    assert_eq!(ul.items().len(), 2);
    let Node::Element(li) = &ul.items()[1] else { panic!() };
    assert_eq!(li.tag(), "li");
}

#[test]
fn parser_raw_text_elements() {
    let roots = h::parse("<style>p{color:red}</style><p>after</p>");
    assert_eq!(roots.len(), 2);
    let Node::Element(style) = &roots[0] else { panic!() };
    let Node::Text(t) = &style.items()[0] else { panic!() };
    assert_eq!(t, "p{color:red}");
    let Node::Element(p) = &roots[1] else { panic!() };
    assert_eq!(p.tag(), "p");
}

#[test]
fn parser_unbalanced_close_tag_is_ignored() {
    let roots = h::parse("</span><p>ok</p>");
    assert_eq!(roots.len(), 1);
    let Node::Element(p) = &roots[0] else { panic!() };
    assert_eq!(p.tag(), "p");
}

#[test]
fn void_tag_table() {
    assert!(h::is_void_tag("BR"));
    assert!(!h::is_void_tag("div"));
    assert!(h::VOID_TAGS.len() > 10);
}

#[test]
fn parser_decodes_entities_in_text() {
    let roots = h::parse("<p>Fish &amp; Chips &lt;3 &#65;&#x42; &nbsp;&mdash;&copy;</p>");
    let Node::Element(p) = &roots[0] else { panic!() };
    let Node::Text(t) = &p.items()[0] else { panic!() };
    assert_eq!(t, "Fish & Chips <3 AB \u{a0}\u{2014}\u{a9}");
}

#[test]
fn parser_decodes_entities_in_attributes() {
    let roots = h::parse("<a title='Fish &amp; Chips' href='/x?a=1&amp;b=2'>go</a>");
    let Node::Element(a) = &roots[0] else { panic!() };
    assert_eq!(a.attr().get("title").as_str(), Some("Fish & Chips"));
    assert_eq!(a.attr().get("href").as_str(), Some("/x?a=1&b=2"));
}

#[test]
fn parser_unknown_entities_survive() {
    // a named entity outside the set, and a truncated ampersand, are left
    // as written rather than dropped
    let roots = h::parse("<p>100 &nope; &amp 10 &lt;= 20</p>");
    let Node::Element(p) = &roots[0] else { panic!() };
    let Node::Text(t) = &p.items()[0] else { panic!() };
    assert_eq!(t, "100 &nope; &amp 10 <= 20");
}

#[test]
fn parser_title_and_textarea_decode_entities() {
    let roots = h::parse("<title>a &amp; b</title><textarea>1 &lt; 2</textarea>");
    let Node::Element(title) = &roots[0] else { panic!() };
    let Node::Text(t) = &title.items()[0] else { panic!() };
    assert_eq!(t, "a & b");
    let Node::Element(area) = &roots[1] else { panic!() };
    let Node::Text(t) = &area.items()[0] else { panic!() };
    assert_eq!(t, "1 < 2");
}

#[test]
fn parser_keeps_script_and_style_content_raw() {
    let markup = "<script>if (a < b && c > d) { x(); }</script>\
                  <style>p > li:not(.x) { color: red }</style><p>ok</p>";
    let roots = h::parse(markup);
    assert_eq!(roots.len(), 3);
    let Node::Element(script) = &roots[0] else { panic!() };
    let Node::Text(t) = &script.items()[0] else { panic!() };
    assert_eq!(t, "if (a < b && c > d) { x(); }");
    let Node::Element(style) = &roots[1] else { panic!() };
    let Node::Text(t) = &style.items()[0] else { panic!() };
    assert_eq!(t, "p > li:not(.x) { color: red }");
    // and they serialize verbatim, so parse(serialize) is stable
    let Node::Element(script_el) = &roots[0] else { panic!() };
    assert_eq!(script_el.render(), "<script>if (a < b && c > d) { x(); }</script>");
}

#[test]
fn builder_escapes_special_text_on_serialize() {
    // the builder puts markup-significant characters into a text node
    let element = h::p([Node::text("Fish & <Chips> \"quoted\"")]);
    let markup = element.render();
    assert_eq!(markup, r#"<p>Fish &amp; &lt;Chips&gt; "quoted"</p>"#);
    // parse → serialize → parse is stable at this form
    let back = h::parse(&markup);
    let Node::Element(p) = &back[0] else { panic!() };
    assert_eq!(p.render(), markup);
    let Node::Text(t) = &p.items()[0] else { panic!() };
    assert_eq!(t, "Fish & <Chips> \"quoted\"");
}

#[test]
fn builder_attributes_and_items() {
    // constructors take one list of: child elements, attribute bundles
    // (merged), and strings; an `items` key inside a bundle routes to
    // children instead of becoming an attribute
    let el = h::div([
        h::h2([Node::text("Title")]),
        h::p([Node::text("Body")]),
        Node::attrs(o!{ "class": "card", "id": "main" }),
        Node::attrs(o!{ "items": a!["tail"] }),
    ]);
    assert_eq!(el.tag(), "div");
    assert_eq!(el.attr().get("class").as_str(), Some("card"));
    assert_eq!(el.attr().get("id").as_str(), Some("main"));
    let items = el.items();
    assert_eq!(items.len(), 3);
    let Node::Element(h2) = &items[0] else { panic!() };
    assert_eq!(h2.tag(), "h2");
    let Node::Element(p) = &items[1] else { panic!() };
    assert_eq!(p.tag(), "p");
    let Node::Text(t) = &items[2] else { panic!() };
    assert_eq!(t, "tail");
}

#[test]
fn builder_operators_and_methods() {
    let mut box_el = h::div([Node::text("inner")]);
    box_el.push(h::span([Node::text("more")]));
    box_el.merge_attrs(&o!{ "data-k": "v" });
    assert_eq!(box_el.attr().get("data-k").as_str(), Some("v"));
    assert_eq!(box_el.items().len(), 2);

    // setAttributes merges one attribute bundle into the element
    box_el.merge_attrs(&o!{ "lang": "en" });
    assert_eq!(box_el.attr().get("lang").as_str(), Some("en"));
    assert_eq!(box_el.attr().get("data-k").as_str(), Some("v"));
    // the live attribute object is the element's own
    box_el.attr_mut().set("lang", "sv".into());
    assert_eq!(box_el.attr().get("lang").as_str(), Some("sv"));

    // addElements / removeElement manage the children list
    box_el.push(Node::text("tail text"));
    assert_eq!(box_el.items().len(), 3);
    let span = h::span([Node::text("gone")]);
    box_el.push(span.clone());
    assert_eq!(box_el.items().len(), 4);
    box_el.remove_element(&span);
    assert_eq!(box_el.items().len(), 3);
    let Node::Text(t) = &box_el.items()[2] else { panic!() };
    assert_eq!(t, "tail text");
}

#[test]
fn builder_numeric_attributes_stay_typed() {
    let el = h::div([Node::attrs(o!{ "width": 300, "checked": true, "note": "plain" })]);
    assert_eq!(el.attr().get("width").as_i64(), Some(300));
    assert_eq!(el.attr().get("checked").as_bool(), Some(true));
    assert_eq!(el.attr().get("note").as_str(), Some("plain"));
}

#[test]
fn parser_survives_unterminated_input() {
    // no closing '>' on the tag, unterminated comment, unterminated raw
    // text, truncated entity: nothing may hang or throw
    let roots = h::parse("<div class='x");
    assert_eq!(roots.len(), 1);

    let roots = h::parse("<!-- no end ever");
    assert_eq!(roots.len(), 0);

    let roots = h::parse("<script>never closed");
    assert_eq!(roots.len(), 1);
    let Node::Element(script) = &roots[0] else { panic!() };
    let Node::Text(t) = &script.items()[0] else { panic!() };
    assert_eq!(t, "never closed");

    let roots = h::parse("<p>oops &am");
    let Node::Element(p) = &roots[0] else { panic!() };
    let Node::Text(t) = &p.items()[0] else { panic!() };
    assert_eq!(t, "oops &am");
}

#[test]
fn parser_close_tag_crosses_unclosed_children() {
    // `</div>` pops everything still open inside it; stray close tags for
    // already-closed elements are ignored
    let roots = h::parse("<div><span><p>x</span></p></div><p>after</p>");
    assert_eq!(roots.len(), 2);
    let Node::Element(div) = &roots[0] else { panic!() };
    assert_eq!(div.tag(), "div");
    let Node::Element(p) = &roots[1] else { panic!() };
    assert_eq!(p.tag(), "p");
}

#[test]
fn parser_links_do_not_nest() {
    // `<a>` implicitly closes an open `<a>`, like real browsers
    let roots = h::parse("<a href='1'>first<a href='2'>second</a>");
    assert_eq!(roots.len(), 2);
    let Node::Element(a1) = &roots[0] else { panic!() };
    assert_eq!(a1.attr().get("href").as_str(), Some("1"));
    let Node::Element(a2) = &roots[1] else { panic!() };
    assert_eq!(a2.attr().get("href").as_str(), Some("2"));
}

#[test]
fn parser_top_level_text_is_kept() {
    let roots = h::parse("hello <b>x</b> tail");
    assert_eq!(roots.len(), 3);
    let Node::Text(t0) = &roots[0] else { panic!() };
    assert_eq!(t0, "hello ");
    let Node::Text(t2) = &roots[2] else { panic!() };
    assert_eq!(t2, " tail");
}

#[test]
fn parser_attribute_values_are_strings() {
    // attribute values are always strings, per HTML: nothing truncates
    // "12.5" through a numeric conversion
    let roots = h::parse("<div width=300 height='12.5'>x</div>");
    let Node::Element(div) = &roots[0] else { panic!() };
    assert_eq!(div.attr().get("width").as_str(), Some("300"));
    assert_eq!(div.attr().get("height").as_str(), Some("12.5"));
}

#[test]
fn void_tags_serialize_without_end_tags() {
    // parses → serializes: void elements must not emit "</br>"-style end
    // tags (double renders when a browser re-parses them)
    let roots = h::parse("<div><br><img src='a.png'><hr/></div>");
    let Node::Element(div) = &roots[0] else { panic!() };
    let Node::Element(br) = &div.items()[0] else { panic!() };
    assert_eq!(br.render(), "<br>");
    let Node::Element(img) = &div.items()[1] else { panic!() };
    assert_eq!(img.render(), r#"<img src="a.png">"#);

    let rendered = div.render();
    assert!(rendered.contains("</div>"));
    assert!(!rendered.contains("</br>"));
    assert!(!rendered.contains("</img>"));
    assert!(!rendered.contains("</hr>"));

    // a page mixing void and normal elements round-trips the same way
    let page = h::body([h::img([Node::attrs(o!{ "src": "b.png" })]), h::p([Node::text("text")])]);
    let out = page.render();
    assert!(out.contains(r#"<img src="b.png">"#));
    assert!(!out.contains("</img>"));
    assert!(out.contains("</p>"));
}

#[test]
fn builder_to_var_is_gold_shaped_data() {
    let el = h::div([Node::text("x"), Node::attrs(o!{ "class": "c" })]);
    let v = el.to_var();
    assert_eq!(v.get("tag").as_str(), Some("div"));
    assert_eq!(v.get("attr").get("class").as_str(), Some("c"));
    assert_eq!(v.get("items").get_idx(0).as_str(), Some("x"));
}
