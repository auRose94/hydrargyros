//! The core tour: everything the value layer does, feature-free — `Var`
//! literals, prototype chains, shared-mutable writes through `VarRef`,
//! typed reads, the list helpers, template strings, serde, and function
//! values that ride data as `{"__func__": id}` handles.
//!
//! Run:
//!   cargo run --example core_tour

use hyg::sugar::{each, filter, find, join, map};
use hyg::{a, func, o, tpl, FromVar, Null, Var};

// the typed hot-path seam: prototype in Var, ship this struct
#[derive(FromVar, Debug)]
struct Stats {
    score: i64,
    #[from_var(default)]
    rank: String,
}

fn main() {
    // ---- literals read like JS ----
    let user = Var::from(o!{
        "name": "bob",
        "age": 30i64,
        "tags": a!["admin", "dev"],
        "stats": o!{ "score": 9i64 },
    });
    println!("user: {user}");

    // ---- shared-mutable: writes through a handle mutate the original ----
    let mut handle = user.at("stats").at("score");
    handle.put(10i64);
    assert_eq!(user.get_path("stats.score").as_i64(), Some(10));
    println!("score after put: {}", user.get_path("stats.score"));

    // ---- prototype chains: lookups walk the parent when a key is missing ----
    let base = o!{ "kind": "npc", "greet": "hello" };
    let mut goblin = o!{ "name": "gruber" };
    goblin.set_parent(base.clone());
    let goblin = Var::from(goblin);
    println!(
        "goblin: kind={} (inherited), name={} (own)",
        goblin.get("kind").as_str().unwrap_or("?"),
        goblin.get("name").as_str().unwrap_or("?"),
    );
    let base_var = Var::from(base);
    assert!(goblin.get("kind") != Null); // found via the parent
    assert_eq!(base_var.get("name"), Null); // the parent doesn't see down

    // ---- typed reads: Into for the happy path, as_* for the check ----
    let name: String = user.get("name").into();
    let age: i64 = user.get("age").into();
    println!("typed read: {name} is {age}");
    assert_eq!(user.get("nope"), Null); // missing keys are Null, a literal

    // ---- list helpers ----
    let xs = a![1i64, 2, 3, 4, 5];
    let doubled = map(&xs, |v| Var::from(v.as_i64().unwrap_or(0) * 2));
    let evens = filter(&doubled, |v| v.as_i64().unwrap_or(1) % 2 == 0);
    let first_big = find(&evens, |v| v.as_i64().unwrap_or(0) > 6);
    let mut count = 0;
    each(&xs, |_| count += 1);
    println!(
        "map: {doubled} | filter: {evens} | find: {first_big} | join: {} | each counted {count}",
        join(&xs, "-"),
    );

    // ---- template strings ----
    let who = "hyg";
    let line = tpl!("hello from $0 — $1 objects, one lock-free read", who, 42i64);
    println!("{line}");

    // ---- serde: one impl, any format ----
    let json = serde_json::to_string(&user).unwrap();
    let back: Var = serde_json::from_str(&json).unwrap();
    assert_eq!(back, user);
    println!("serde round-trip: {json}");

    // ---- functions are values that ride data ----
    let shout: Var = func!(|args: &[Var]| {
        let text = args.first().cloned().unwrap_or(Null);
        Var::from(format!("{}!", text))
    })
    .into();
    let out = shout.call(&[Var::from("prototyped")]);
    println!("func call: {}", out.as_str().unwrap_or("?"));
    // serialize a function handle; the process-global registry restores it
    let handle_json = serde_json::to_string(&shout).unwrap();
    let restored: Var = serde_json::from_str(&handle_json).unwrap();
    println!(
        "func handle: {handle_json} -> restored call says: {}",
        restored.call(&[Var::from("round-tripped")]).as_str().unwrap_or("?"),
    );

    // ---- FromVar: the strict, path-tracked typed read ----
    let stats = Stats::from_var(&user.get("stats")).unwrap();
    println!(
        "derived struct: {stats:?} (score {}, rank defaulted to {:?})",
        stats.score, stats.rank,
    );
    let bad = o!{ "score": "nine" };
    match Stats::from_var(&Var::from(bad)) {
        Ok(_) => unreachable!(),
        Err(e) => println!("strict rejection: {e}"),
    }
}
