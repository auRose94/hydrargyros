//! FromVar derive tests: strict typed reads, field options, nested structs,
//! and error paths.

use hyg::{a, o, FromVar, Var};

/// `o!{...}` as a `Var` — the test-side shape of the config seam.
macro_rules! var {
    ($($t:tt)*) => { ::std::convert::Into::<hyg::Var>::into(o!{ $($t)* }) };
}

#[derive(FromVar, Debug, PartialEq)]
struct Simple {
    name: String,
    score: i64,
    active: bool,
}

#[test]
fn reads_typed_fields() {
    let s = Simple::from_var(&var!{ "name": "bob", "score": 9, "active": true }).unwrap();
    assert_eq!(s, Simple { name: "bob".to_string(), score: 9, active: true });
}

#[test]
fn type_mismatch_is_an_error_with_a_path() {
    let err = Simple::from_var(&var!{ "name": "bob", "score": "nine", "active": true }).unwrap_err();
    assert_eq!(err.path, "score");
    assert!(err.message.contains("expected int"), "{}", err.message);
}

#[test]
fn non_object_root_is_an_error() {
    let err = Simple::from_var(&Var::from(42)).unwrap_err();
    assert!(err.message.contains("expected object"), "{}", err.message);
}

#[test]
fn missing_field_is_strict() {
    // no silent zero/empty coercion — that's what default is for
    let err = Simple::from_var(&var!{ "name": "bob" }).unwrap_err();
    assert_eq!(err.path, "score");
    assert!(err.message.contains("found null"), "{}", err.message);
}

#[derive(FromVar, Debug, PartialEq)]
struct WithOptions {
    #[from_var(rename = "userId")]
    id: String,
    #[from_var(default)]
    tags: Vec<String>,
    nickname: Option<String>,
}

#[test]
fn rename_default_and_option() {
    let full = WithOptions::from_var(&var!{
        "userId": "u-1",
        "tags": a!["admin"],
        "nickname": "rosie",
    })
    .unwrap();
    assert_eq!(full.id, "u-1");
    assert_eq!(full.tags, vec!["admin".to_string()]);
    assert_eq!(full.nickname.as_deref(), Some("rosie"));

    // defaults and Options fill in for missing keys
    let sparse = WithOptions::from_var(&var!{ "userId": "u-2" }).unwrap();
    assert_eq!(sparse.tags, Vec::<String>::new());
    assert_eq!(sparse.nickname, None);
}

#[derive(FromVar, Debug, PartialEq)]
struct Inner {
    x: i64,
}

#[derive(FromVar, Debug, PartialEq)]
struct Outer {
    inner: Inner,
    points: Vec<Inner>,
}

#[test]
fn nested_structs_and_lists() {
    let v = Outer::from_var(&var!{
        "inner": o!{ "x": 1 },
        "points": a![o!{ "x": 2 }, o!{ "x": 3 }],
    })
    .unwrap();
    assert_eq!(v.inner.x, 1);
    assert_eq!(v.points.iter().map(|p| p.x).collect::<Vec<_>>(), vec![2, 3]);
}

#[test]
fn nested_errors_compose_paths() {
    let err = Outer::from_var(&var!{
        "inner": o!{ "x": 1 },
        "points": a![o!{ "x": 2 }, o!{ "x": "three" }],
    })
    .unwrap_err();
    assert_eq!(err.path, "points.1.x");
    assert!(err.message.contains("expected int"), "{}", err.message);
}

#[test]
fn var_passthrough_clones() {
    let v = Var::from(o!{ "any": "thing" });
    assert_eq!(Var::from_var(&v).unwrap(), v);
}

#[test]
fn math_types_read_strictly() {
    use hyg::{Quat, Vec3};
    let ok = Vec3::from_var(&Var::from(hyg::Vec3::new(1.0, 2.0, 3.0))).unwrap();
    assert_eq!(ok, Vec3::new(1.0, 2.0, 3.0));
    assert!(Vec3::from_var(&Var::from(1)).is_err());
    assert!(Quat::from_var(&Var::Null).is_err());
}
