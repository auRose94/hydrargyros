//! The Send + Sync story, exercised for real: one shared `Var` handle cloned
//! across OS threads, writes through every handle visible everywhere.

use std::thread;

use hyg::{func, o, Var};

#[test]
fn shared_object_across_threads() {
    let shared = Var::from(o!{ "n": 0i64 });
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let v = shared.clone();
            thread::spawn(move || {
                v.set(&format!("t{t}"), t * 10);
                v.at("n").put(t);
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    // each thread's own key landed, with the value it wrote
    for t in 0..8 {
        assert_eq!(shared.get(&format!("t{t}")).as_i64(), Some((t * 10) as i64));
    }
    // "n" was written by all 8 threads — whichever landed, it's an int
    assert!(shared.get("n").as_i64().is_some());
}

#[test]
fn functions_are_send_and_callable_across_threads() {
    let f: Var = func!(|a| a).into();
    let handles: Vec<_> = (0..4)
        .map(|t| {
            let f = f.clone();
            thread::spawn(move || f.call(&[Var::from(t * 2)]).as_i64())
        })
        .collect();
    for (t, h) in handles.into_iter().enumerate() {
        assert_eq!(h.join().unwrap(), Some((t * 2) as i64));
    }
}
