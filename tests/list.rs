//! `List` — the ordered array. Interior-mutable through every handle,
//! JS-style index-set that grows with nulls, the `a!` build path
//! (prepend head + extend tail), serde, and the self-extend snapshot
//! contract that keeps shared payloads deadlock-free.

use hyg::{a, List, Null, Var};

#[test]
fn list_keeps_order_and_reads() {
    let xs = a!["a", "b", "c"];
    assert_eq!(xs.len(), 3);
    assert!(!xs.is_empty());
    let got: Vec<String> = xs
        .iter()
        .into_iter()
        .map(|v| v.as_string().unwrap().to_string())
        .collect();
    assert_eq!(got, ["a", "b", "c"]);
    assert_eq!(xs.get(0).as_str(), Some("a"));
    assert_eq!(xs.get_idx(2).as_str(), Some("c"));
}

#[test]
fn index_set_beyond_the_end_grows_with_nulls() {
    // the JS contract: arr[i] = x extends the array; the gap fills with null
    let xs = List::new();
    xs.set(2, Var::from("third"));
    assert_eq!(xs.len(), 3);
    assert_eq!(xs.get(0), Null);
    assert_eq!(xs.get(1), Null);
    assert_eq!(xs.get(2).as_str(), Some("third"));

    // setting inside the range overwrites in place, order intact
    xs.set(1, Var::from("second"));
    assert_eq!(xs.len(), 3);
    assert_eq!(xs.get(1).as_str(), Some("second"));
}

#[test]
#[should_panic]
fn get_out_of_range_panics() {
    let xs = a![1i64];
    let _ = xs.get(5);
}

#[test]
fn every_handle_sees_the_same_payload() {
    let xs = a![1i64];
    let via_var = Var::from(xs.clone()); // List clone shares the payload

    xs.push(Var::from(2i64));
    let back = via_var.as_list().expect("var holds the list");
    assert_eq!(back.len(), 2);

    back.set(0, Var::from(9i64));
    assert_eq!(xs.get(0).as_i64(), Some(9));
}

#[test]
fn extend_from_itself_doubles_without_deadlock() {
    // extend snapshots `other` before locking: a list extended from its own
    // handle must double, not deadlock (the shared-payload contract)
    let xs = a![1i64, 2];
    xs.extend(&xs);
    assert_eq!(xs.len(), 4);
    assert_eq!(xs.get(2).as_i64(), Some(1));
    assert_eq!(xs.get(3).as_i64(), Some(2));
}

#[test]
fn prepend_then_extend_builds_source_order() {
    // how `a![]` assembles: the head prepends, the tail extends forward
    let xs = List::new();
    xs.prepend(Var::from("head"));
    let tail = a!["mid", "end"];
    xs.extend(&tail);
    let got: Vec<String> = xs
        .iter()
        .into_iter()
        .map(|v| v.as_string().unwrap().to_string())
        .collect();
    assert_eq!(got, ["head", "mid", "end"]);
}

#[test]
fn serde_round_trips_with_nulls() {
    let xs = a![1i64, "two", Null];
    let json = serde_json::to_string(&Var::from(xs.clone())).unwrap();
    assert_eq!(json, r#"[1,"two",null]"#);

    let back: Var = serde_json::from_str(&json).unwrap();
    let l = back.as_list().expect("list survives");
    assert_eq!(l.len(), 3);
    assert_eq!(l.get(0).as_i64(), Some(1));
    assert_eq!(l.get(2), Null);
}

#[test]
fn display_renders_bracketed_items() {
    let xs = a![1i64, "two"];
    assert_eq!(format!("{xs}"), "[1, two]");
}
