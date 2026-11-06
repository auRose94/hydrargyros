//! Function values: the `Func` container and the `func!` literal.

use hydrargyros::{func, o, Func, Ty, Var};

#[test]
fn func_named_args() {
    let add = func!(|a, b| a + b);
    assert_eq!(add.call(&[Var::from(2i64), Var::from(3i64)]).as_i64(), Some(5));
    // Var arithmetic widens int + float, JS-style
    let r = add.call(&[Var::from(1.5f64), Var::from(2i64)]);
    assert_eq!(r.as_f64(), Some(3.5));
}

#[test]
fn func_missing_args_are_null() {
    // JS hands you undefined for missing args; Mercury hands you Null
    let pick_second = func!(|a, b| if b.ty() == Ty::Null { a } else { b });
    assert_eq!(pick_second.call(&[Var::from("only")]).as_str(), Some("only"));
    assert_eq!(pick_second.call(&[Var::from("a"), Var::from("b")]).as_str(), Some("b"));
    // zero args: a is Null too
    assert_eq!(pick_second.call(&[]).ty(), Ty::Null);
}

#[test]
fn func_zero_args() {
    let f = func!(|| Var::from(7i64));
    assert_eq!(f.call(&[]).as_i64(), Some(7));
}

#[test]
fn func_raw_closure_form() {
    // typed parameter = full control over the raw argument list
    let sum = func!(|args: &[Var]| {
        let mut total = 0i64;
        for a in args { total += a.as_i64().unwrap_or(0); }
        Var::from(total)
    });
    assert_eq!(sum.call(&[Var::from(1i64), Var::from(2i64), Var::from(3i64)]).as_i64(), Some(6));
}

#[test]
fn func_captures_environment() {
    let owner = "bob".to_string();
    // named form auto-moves captures (a Func is 'static, so this must move)
    let greet = func!(|who| Var::from(format!("{owner} greets {who}")));
    assert_eq!(greet.call(&[Var::from("alice")]).as_str(), Some("bob greets alice"));
}

#[test]
fn func_identity_by_id() {
    let f = func!(|| Var::Null);
    let g = f.clone();
    // clones share the id; a fresh Func never collides
    assert_eq!(f, g);
    assert_eq!(f.id(), g.id());
    let h = Func::new(|_| Var::Null);
    assert_ne!(f, h);

    // the Var level: Func participates in Var equality by id
    let vf = Var::Func(f);
    let vg = Var::Func(g);
    let vh = Var::Func(h);
    assert_eq!(vf, vg);
    assert_ne!(vf, vh);
}

#[test]
fn func_id_shows_in_display_and_debug() {
    let v = Var::Func(func!(|| Var::Null));
    let Var::Func(f) = &v else { unreachable!() };
    let id = f.id();
    assert_eq!(v.to_string(), format!("<function {id}>"));
    assert_eq!(format!("{v:?}"), format!("Func {{ id: {id} }}"));
}

#[test]
fn func_into_var_and_object() {
    let obj = o!{ "cb": func!(|| Var::from(1i64)), "n": 5i64 };
    assert_eq!(obj.get("cb").ty(), Ty::Func);
    assert_eq!(obj.get("cb").call(&[]).as_i64(), Some(1));

    let v: Var = func!(|| Var::from(2i64)).into();
    assert_eq!(v.call(&[]).as_i64(), Some(2));
}

#[test]
fn func_serializes_as_id_handle() {
    // the id→Func registry: a Func serializes as the one-key handle
    // {"__func__": id} — formerly null, back when functions died at the
    // serialization boundary
    let f = func!(|| Var::Null);
    let id = f.id();
    let v = Var::Func(f);
    assert_eq!(
        serde_json::to_string(&v).unwrap(),
        format!(r#"{{"__func__":{id}}}"#)
    );
}

#[test]
#[should_panic(expected = "cannot call non-function")]
fn call_non_function_panics() {
    let _ = Var::from(1i64).call(&[]);
}
