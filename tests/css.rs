//! CSS parser tests: selectors, declarations, function/quoted values with
//! embedded semicolons/braces, `!important`, at-rules, typed numbers.

use hyg::web::css;

#[test]
fn css_parser_rules_and_declarations() {
    let rules = css::parse_css("/* c */ p, .x { color: red; margin : 0 } #id { width: 50%; }");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].selector, "p, .x");
    let decl = &rules[0].declarations;
    assert_eq!(decl.get("color").as_str(), Some("red"));
    assert_eq!(decl.get("margin").as_i64(), Some(0));
    let decl = &rules[1].declarations;
    assert_eq!(decl.get("width").as_str(), Some("50%"));
}

#[test]
fn css_parser_function_values_with_semicolons_inside() {
    let decl = css::parse_declarations("background: rgba(1,2,3,0.5); x: 1");
    assert_eq!(decl.len(), 2);
    assert_eq!(decl.get("background").as_str(), Some("rgba(1,2,3,0.5)"));
}

#[test]
fn css_parser_rule_streaming() {
    let css = "a { x: 1 } b { y: 2 } trailing text";
    let mut pos = 0;
    let first = css::parse_rule(css, &mut pos).unwrap();
    assert_eq!(first.selector, "a");
    assert_eq!(first.declarations.get("x").as_i64(), Some(1));
    let second = css::parse_rule(css, &mut pos).unwrap();
    assert_eq!(second.selector, "b");
    assert_eq!(second.declarations.get("y").as_i64(), Some(2));
    // pos moved past both rules, into the trailing text
    assert_eq!(pos, 21);
}

#[test]
fn css_parser_important_flag_on_any_value_type() {
    let decl = css::parse_declarations(
        "margin: 0 !important; width: 100px !important; color: red !IMPORTANT; z-index: 3",
    );
    assert_eq!(decl.get("margin").as_i64(), Some(0));
    assert_eq!(decl.get("margin!").as_bool(), Some(true));
    assert_eq!(decl.get("width").as_str(), Some("100px"));
    assert_eq!(decl.get("width!").as_bool(), Some(true));
    assert_eq!(decl.get("color").as_str(), Some("red"));
    assert_eq!(decl.get("color!").as_bool(), Some(true));
    // no flag on the value without !important
    assert_eq!(decl.get("z-index!").ty(), hyg::Ty::Null);
}

#[test]
fn css_parser_quoted_values_with_semicolons_and_braces() {
    let decl = css::parse_declarations(
        "font-family: \"Foo; Bar\"; content: 'x}y'; color: rgb(1,2,3)",
    );
    assert_eq!(decl.get("font-family").as_str(), Some("\"Foo; Bar\""));
    assert_eq!(decl.get("content").as_str(), Some("'x}y'"));
    assert_eq!(decl.get("color").as_str(), Some("rgb(1,2,3)"));
}

#[test]
fn css_at_rules_do_not_leak_into_rules() {
    // the plain parser skips at-rules whole: block forms balance their
    // braces, statement forms end at ';'
    let rules = css::parse_css(
        "@charset \"utf-8\";\n\
         @media (max-width: 600px) { p { color: red } }\n\
         @import url(\"x.css\");\n\
         @keyframes spin { to { left: 1px } }\n\
         .kept { margin: 0 }\n\
         div.kept { color: blue }\n",
    );
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].selector, ".kept");
    assert_eq!(rules[0].declarations.get("margin").as_i64(), Some(0));
    assert_eq!(rules[1].selector, "div.kept");
    assert_eq!(rules[1].declarations.get("color").as_str(), Some("blue"));

    // parse_rule called directly with an at-rule consumes it and returns
    // nothing, leaving pos past the block
    let mut pos = 0;
    assert!(css::parse_rule("@media screen { a { b: c } }", &mut pos).is_none());
    assert_eq!(pos, "@media screen { a { b: c } }".len());
}

#[test]
fn css_parser_negative_and_float_values_stay_typed() {
    let decl = css::parse_declarations("left: -3px; top: 12.5; right: -4; bottom: 50%");
    assert_eq!(decl.get("left").as_str(), Some("-3px"));
    assert_eq!(decl.get("top").as_f64(), Some(12.5));
    assert_eq!(decl.get("right").as_i64(), Some(-4));
    assert_eq!(decl.get("bottom").as_str(), Some("50%"));
}
