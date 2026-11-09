//! Feel-the-ergonomics tests: do the gold sugar port faithfully, and where
//! does Rust change the shape?

use hyg::sugar::{each, filter, find, join, map};
use hyg::{a, func, o, Null, tpl, Var};

#[test]
fn jo_literal_nested() {
    let user = o! {
        "name": "bob",
        "age": 30i64,
        "tags": a!["admin", "dev"],
        "stats": o!{ "score": 9i64, "level": 3i64 },
    };
    assert_eq!(user.get("name").as_str(), Some("bob"));
    assert_eq!(user.get("age").as_i64(), Some(30));
    assert_eq!(user.get("tags").as_list().unwrap().len(), 2);
    assert_eq!(user.get_path("stats.score").as_i64(), Some(9));
    assert_eq!(user.get_path("stats.level").as_i64(), Some(3));
}

#[test]
fn write_through_varref() {
    // The faithful port: write through a shared handle, visible on the original.
    let user = o!{ "score": 0i64 };
    user.at("score").put(10i64);
    assert_eq!(user.get("score").as_i64(), Some(10));

    // Nested write via chained proxy.
    let game = o!{ "player": o!{ "hp": 100i64 } };
    game.at("player").at("hp").put(42i64);
    assert_eq!(game.get_path("player.hp").as_i64(), Some(42));
}

#[test]
fn write_through_dotted_path() {
    let cfg = o!{ "a": o!{ "b": o!{ "c": 1i64 } } };
    cfg.set_path("a.b.c", 99i64);
    assert_eq!(cfg.get_path("a.b.c").as_i64(), Some(99));

    // Creates missing intermediates.
    let fresh = o!{};
    fresh.set_path("x.y.z", 5i64);
    assert_eq!(fresh.get_path("x.y.z").as_i64(), Some(5));
}

#[test]
fn typed_reads() {
    let v = o!{ "name": "bob", "age": 31i64, "active": true };
    let name: String = v.get("name").into();
    assert_eq!(name, "bob");
    let age: i64 = v.get("age").into();
    assert_eq!(age, 31);
    let active: bool = v.get("active").into();
    assert!(active);
}

#[test]
fn null_safe_reads() {
    let v = o!{ "a": 1i64 };
    // missing key -> Null, no panic
    assert_eq!(v.get("missing"), Null);
    assert_eq!(v.get("a.missing"), Null); // descend into non-object
}

#[test]
fn arithmetic_widens() {
    let a = Var::from(1i64);
    let b = Var::from(2.5f64);
    let sum = a.clone() + b;
    assert_eq!(sum.as_f64(), Some(3.5));
    assert_eq!((a.clone() + a).as_i64(), Some(2));
}

#[test]
fn array_helpers() {
    let xs = a![1i64, 2, 3, 4, 5];
    let doubled = map(&xs, |v| v.clone() * Var::from(2i64));
    assert_eq!(doubled.get_idx(0).as_i64(), Some(2));
    assert_eq!(doubled.len(), 5);

    let evens = filter(&xs, |v| v.as_i64().unwrap() % 2 == 0);
    assert_eq!(evens.len(), 2);

    let big = find(&xs, |v| v.as_i64().unwrap() > 3);
    assert_eq!(big.as_i64(), Some(4));

    let joined = join(&xs, "-");
    assert_eq!(joined, "1-2-3-4-5");

    let mut seen = 0;
    each(&xs, |_| seen += 1);
    assert_eq!(seen, 5);
}

#[test]
fn tpl_template() {
    let s = tpl!("Hello $0, you scored $1", "bob", 9i64);
    assert_eq!(s, "Hello bob, you scored 9");
    // out-of-range index preserved literally
    let s2 = tpl!("$0 and $5", "a");
    assert_eq!(s2, "a and $5");
}

#[test]
fn function_value() {
    let f: Var = func!(|args: &[Var]| {
        // sum
        let mut total = 0i64;
        for a in args { total += a.as_i64().unwrap_or(0); }
        Var::from(total)
    }).into();
    let out = f.call(&[Var::from(1i64), Var::from(2i64), Var::from(3i64)]);
    assert_eq!(out.as_i64(), Some(6));
}

#[test]
fn prototype_inheritance() {
    let base = o!{ "color": "blue" };
    let mut child = o!{ "name": "car" };
    child.set_parent(base.clone());
    // own key shadows / own data
    assert_eq!(child.get("name").as_str(), Some("car"));
    // inherited key
    assert_eq!(child.get("color").as_str(), Some("blue"));
    assert!(child.inherits(&base));
}

#[test]
fn cow_sharing() {
    // Two handles share the same object; a write via one is visible via the
    // other (the whole point of the shared-data model).
    let a = o!{ "x": 1i64 };
    let b = a.clone(); // shares payload
    a.at("x").put(99i64);
    assert_eq!(b.get("x").as_i64(), Some(99));
}
