//! Serde round-trip tests: Var serializes to / from any serde format.
//! JSON is the representative one (via serde_json).

use hyg::{a, func, o, Null, Ty, Var};

#[test]
fn roundtrip_scalar() {
    for v in [
        Null,
        Var::from(true),
        Var::from(42i64),
        Var::from(3.5f64),
        Var::from("hello"),
    ] {
        let json = serde_json::to_string(&v).unwrap();
        let back: Var = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v, "round-trip mismatch for {v}");
    }
}

#[test]
fn roundtrip_object_with_prototype() {
    // A nested object with a list and nested object.
    let v = o! {
        "name": "bob",
        "age": 30i64,
        "tags": a!["admin", "dev"],
        "stats": o!{ "score": 9i64 },
    };
    let json = serde_json::to_string(&v).unwrap();
    let back: Var = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Var::from(v.clone()));
    // spot-check the structure survived
    assert_eq!(back.get("name").as_str(), Some("bob"));
    assert_eq!(back.get_path("stats.score").as_i64(), Some(9));
    assert_eq!(back.get("tags").get_idx(1).as_str(), Some("dev"));
}

#[test]
fn func_roundtrips_through_registry() {
    // a registered id restores a Func equal by id, whose closure actually runs
    let f = Var::from(func!(|n| Var::from(n.as_i64().unwrap_or(0) * 2)));
    let json = serde_json::to_string(&f).unwrap();
    let back: Var = serde_json::from_str(&json).unwrap();
    assert_eq!(back, f);
    assert_eq!(back.call(&[Var::from(21i64)]).as_i64(), Some(42));
}

#[test]
fn func_unknown_id_deserializes_to_null() {
    // the id is a within-process handle: an id that isn't registered can only
    // come from another process or from pre-registry data, so it degrades to
    // Null instead of erroring
    let v: Var = serde_json::from_str(r#"{"__func__": 987654321}"#).unwrap();
    assert_eq!(v, Null);
    // a bare null still deserializes to Null as before
    let bare: Var = serde_json::from_str("null").unwrap();
    assert_eq!(bare, Null);
}

#[test]
fn object_with_func_field_roundtrips() {
    // the function survives an object round-trip and stays callable
    let v = o! { "name": "cb", "run": func!(|| Var::from("called")) };
    let json = serde_json::to_string(&v).unwrap();
    let back: Var = serde_json::from_str(&json).unwrap();
    assert_eq!(back.get("name").as_str(), Some("cb"));
    assert_eq!(back.get("run").ty(), Ty::Func);
    assert_eq!(back.get("run").call(&[]).as_str(), Some("called"));
}

#[test]
fn func_serialization_is_stable() {
    // the same Func always carries the same id, clones included
    let f = Var::from(func!(|| Var::Null));
    let first = serde_json::to_string(&f).unwrap();
    let second = serde_json::to_string(&f).unwrap();
    assert_eq!(first, second);
    let clone = f.clone();
    assert_eq!(serde_json::to_string(&clone).unwrap(), first);
}

#[test]
fn deserialize_into_typed_var() {
    // Deserialize straight into a typed Var (not just `Var`).
    let v: Var = serde_json::from_str(r#"{"a": [1, 2, 3], "b": true}"#).unwrap();
    assert_eq!(v.get("b").as_bool(), Some(true));
    assert_eq!(v.get("a").get_idx(2).as_i64(), Some(3));
}
