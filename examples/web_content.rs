//! The web-content tour, feature-free — the four always-on siblings:
//! the HTML builder + parser (elements as data, lossless round-trip),
//! the CSS parser (typed declarations, `!important` flags), the file-backed
//! document store (`$set`/`$inc`, `$in` queries, Mongo-shaped ids), and
//! the `database` facade over it. No server, no feature gates.
//!
//! Run:
//!   cargo run --example web_content

use hyg::web::css::parse_css;
use hyg::web::database::Database;
use hyg::web::html::{self as h, Node};
use hyg::{a, o};

fn main() {
    // ---- HTML: elements are data you build, inspect, and re-parse ----
    let heading = h::h1([Node::text("hyg content")]);
    let page = h::html([
        h::head([h::title([Node::text("tour")])]),
        h::body([
            Node::attrs(o!{ "class": "page" }),
            heading.clone(),
            h::p([Node::text("escaped: <b> & \"quotes\"")]),
            h::img([Node::attrs(o!{ "src": "x.png" })]), // void tag, no close
        ]),
    ]);
    let serialized = page.render();
    println!("built:\n{serialized}");

    // elements are inspectable data, not a string
    println!(
        "heading is a <{}>, as Var data: {}",
        heading.tag(),
        heading.to_var(),
    );

    // the parser recovers the tree from text — void tags, entities, the lot
    let reparsed = h::parse(&serialized);
    let again: String = reparsed.iter().map(|n| n.render()).collect();
    assert_eq!(again, serialized);
    println!("parse round-trip: identical");

    // ---- CSS: rules with typed declarations; !important rides as name! ----
    let rules = parse_css(
        "h1 { color: #3366ff !important; margin: 4px 8px } @media print { p { margin: 0 } }",
    );
    println!("{} rule(s); at-rules are skipped", rules.len());
    for rule in &rules {
        println!("selector: {}", rule.selector);
        for (name, value) in rule.declarations.entries() {
            if name.ends_with('!') {
                continue; // the flag key, printed with its declaration below
            }
            let bang = if rule.declarations.get(&format!("{name}!")).as_bool() == Some(true) {
                "  (!important)"
            } else {
                ""
            };
            println!("  {name}: {value}{bang}");
        }
    }

    // ---- store: documents on disk, Mongo-shaped ----
    let dir = std::env::temp_dir().join(format!("hyg-web-content-{}", std::process::id()));
    let db = Database::connect(o!{
        "backend": "file",
        "name": "content",
        "path": dir.display().to_string(),
    })
    .unwrap();
    let posts = db.get_collection("posts");

    let a = posts.insert(o!{ "title": "first", "views": 1i64, "tags": a!["news"] }).unwrap();
    posts.insert(o!{ "title": "second", "views": 5i64, "tags": a!["dev"] }).unwrap();
    println!(
        "inserted: {} (_id {})",
        a.get("title").as_str().unwrap_or("?"),
        a.get("_id").as_str().unwrap_or("?"),
    );

    // $set / $inc updates
    posts
        .update_one(
            o!{ "title": "first" },
            o!{ "$inc": o!{ "views": 4i64 }, "$set": o!{ "edited": true } },
        )
        .unwrap();
    let first = posts.find_one(o!{ "title": "first" }).unwrap().unwrap();
    println!(
        "after $inc/$set: views={} edited={}",
        first.get("views").as_i64().unwrap_or(0),
        first.get("edited").as_bool().unwrap_or(false),
    );

    // $in queries
    let tagged = posts.find(o!{ "tags": o!{ "$in": a!["dev", "news"] } }).unwrap();
    println!("$in query matched {} docs", tagged.len());

    // ids are Mongo-shaped 24-char hex
    assert_eq!(a.get("_id").as_str().map(|s| s.len()), Some(24));
    let _ = std::fs::remove_dir_all(&dir);
}
