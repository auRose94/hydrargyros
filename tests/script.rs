//! Interpreter-seam tests (behind the `script` feature): the `Script`
//! facade against the real v12 engine — scalar round-trips, host functions,
//! persistent state, the JSON bridge, the scripted-component engine test,
//! and the thread-local pool's Send story. All real behavior: real JS
//! compilation, real host-fn calls, real engine frames — no mocks, like
//! every other suite here.

#![cfg(feature = "script")]

use std::sync::{Arc, Mutex};
use std::thread;

use hydrargyros::game::component::Component;
use hydrargyros::game::{Engine, Entity, ScriptedComponent};
use hydrargyros::o;
use hydrargyros::script::{Script, ScriptError};
use hydrargyros::{List, Var};

// ---- (a) scalar Var <-> script round-trips ----

#[test]
fn scalar_args_and_results_round_trip() {
    let mut script = Script::new("function echo(a) { return a; }").unwrap();

    // int: 5 crosses as JSON "5", reads back I64 (JS has one number type)
    assert_eq!(
        script.call("echo", &[Var::from(5i64)]).unwrap(),
        Var::I64(5)
    );
    // float with a fraction keeps its float-ness
    assert_eq!(
        script.call("echo", &[Var::from(2.5)]).unwrap(),
        Var::F64(2.5)
    );
    // string crosses quoted and comes back unquoted
    assert_eq!(
        script.call("echo", &[Var::from("hi")]).unwrap(),
        Var::Str("hi".to_string())
    );
    // bool
    assert_eq!(
        script.call("echo", &[Var::from(true)]).unwrap(),
        Var::Bool(true)
    );
    // null → JS null → Null
    assert_eq!(script.call("echo", &[Var::Null]).unwrap(), Var::Null);
    // no return → undefined → Null (gold's call returns null the same way)
    let mut script = Script::new("function nothing() {}").unwrap();
    assert_eq!(script.call("nothing", &[]).unwrap(), Var::Null);
}

#[test]
fn scalar_set_get_global_round_trips() {
    let mut script = Script::new("var sink = 0;").unwrap();
    script.set_global("answer", &Var::from(42i64)).unwrap();
    script
        .set_global("name", &Var::from("hydrargyros"))
        .unwrap();
    script.set_global("ratio", &Var::from(0.5)).unwrap();
    assert_eq!(script.get_global("answer").unwrap(), Var::I64(42));
    assert_eq!(
        script.get_global("name").unwrap(),
        Var::Str("hydrargyros".to_string())
    );
    assert_eq!(script.get_global("ratio").unwrap(), Var::F64(0.5));

    // the script reads what the host set (gold's shared-globals surface)
    let mut script = Script::new("function read() { return answer + 1; }").unwrap();
    script.set_global("answer", &Var::from(41i64)).unwrap();
    assert_eq!(script.call("read", &[]).unwrap(), Var::I64(42));
}

#[test]
fn missing_globals_and_functions_are_explicit_errors() {
    let mut script = Script::new("var x = 1;").unwrap();
    let missing = script.get_global("nowhere").unwrap_err();
    assert!(missing
        .to_string()
        .contains("global 'nowhere' is not defined"));
    let missing = script.call("nowhere", &[]).unwrap_err();
    assert!(missing
        .to_string()
        .contains("script function 'nowhere' is not defined"));

    // a script FUNCTION value crossing OUT is an error, not a fake call
    let mut script = Script::new("function hook() {}").unwrap();
    let fn_out = script.get_global("hook").unwrap_err();
    assert!(fn_out.to_string().contains("function"));
}

#[test]
fn compile_errors_surface_at_construction_like_golds_load() {
    let err = Script::new("let x = ;").unwrap_err();
    assert!(matches!(err, ScriptError::Engine(_)), "got {err:?}");
    // v12's Compile errors carry the front-end message
    assert!(err.to_string().contains("compile error"));
}

#[test]
fn script_throws_surface_as_engine_errors() {
    let mut script = Script::new("function bad() { throw 'out of bounds'; }").unwrap();
    let err = script.call("bad", &[]).unwrap_err();
    assert!(matches!(err, ScriptError::Engine(_)));
    assert!(err.to_string().contains("out of bounds"));
}

#[test]
fn non_representable_values_are_rejected_not_degraded() {
    let mut script = Script::new("var sink = 0;").unwrap();
    // JS numbers are f64: NaN/inf can't cross — an explicit error, not a
    // silent null
    let nan = script.set_global("x", &Var::from(f64::NAN)).unwrap_err();
    assert!(nan.to_string().contains("not finite"), "got {nan}");
    // integers beyond 2^53 would silently round through a JS double
    let big = script.set_global("x", &Var::from(i64::MAX)).unwrap_err();
    assert!(
        big.to_string().contains("not exactly representable"),
        "got {big}"
    );
    // ... and the same rule fires nested inside an object
    let nested = hydrargyros::o! { "deep": i64::MAX };
    let nested = script.set_global("x", &Var::from(nested)).unwrap_err();
    assert!(nested.to_string().contains("not exactly representable"));
}

// ---- (b) host functions: register, call with Var args, get the Var result ----

#[test]
fn host_fn_called_from_script_with_var_args() {
    let mut script = Script::new(
        "function plain(n) { return double(n) + 1; } \
         function greedy(a, b) { return inspect(a, b); } \
         function probe(obj) { return key(obj); }",
    )
    .unwrap();

    // scalar in, scalar out
    script.register("double", |args| {
        Var::from(args[0].as_f64().unwrap_or(0.0) * 2.0)
    });
    assert_eq!(
        script.call("plain", &[Var::from(10i64)]).unwrap(),
        Var::I64(21)
    );

    // multiple Var args arrive in order
    script.register("inspect", |args| {
        Var::from(format!(
            "{}-{}",
            args[0].as_str().unwrap_or("?"),
            args[1].as_i64().unwrap_or(0)
        ))
    });
    assert_eq!(
        script
            .call("greedy", &[Var::from("left"), Var::from(7i64)])
            .unwrap(),
        Var::Str("left-7".to_string())
    );

    // an OBJECT arg crosses the JSON bridge into the host closure
    script.register("key", |args| args[0].get("k"));
    let obj = o! { "k": 42i64 };
    assert_eq!(
        script.call("probe", &[Var::from(obj)]).unwrap(),
        Var::I64(42)
    );
}

#[test]
fn host_fn_state_persists_across_calls() {
    let mut script = Script::new("function bump() { return tick(); }").unwrap();
    let count = Arc::new(Mutex::new(0u32));
    let host_count = Arc::clone(&count);
    script.register("tick", move |_args| {
        let mut c = host_count.lock().unwrap();
        *c += 1;
        Var::from(*c as i64)
    });
    assert_eq!(script.call("bump", &[]).unwrap(), Var::I64(1));
    assert_eq!(script.call("bump", &[]).unwrap(), Var::I64(2));
    assert_eq!(*count.lock().unwrap(), 2);
}

#[test]
fn func_var_crossing_in_becomes_a_callable() {
    let mut script = Script::new("function invoke(a, b) { return combine(a, b); }").unwrap();
    // a host-side Func: set_global hands it to the script as a callable
    script
        .set_global(
            "combine",
            &Var::from(hydrargyros::func!(|a, b| {
                Var::from(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0))
            })),
        )
        .unwrap();
    assert_eq!(
        script
            .call("invoke", &[Var::from(20i64), Var::from(22i64)])
            .unwrap(),
        Var::I64(42)
    );
}

// ---- (c) script state persists across call()s ----

#[test]
fn script_state_accumulates_across_calls() {
    // the update-loop shape: top-level state, per-frame hook — the same-eval
    // closures keep the lexical state alive across calls
    let mut script = Script::new(
        "let n = 0; function update(dt){ n += dt; return n; } function peek(){ return n; }",
    )
    .unwrap();
    for expected in [0.5f64, 1.0, 1.5] {
        let n = script.call("update", &[Var::from(0.5)]).unwrap();
        // JS has one number type: integral sums read back I64, fractions F64
        assert_eq!(n.as_f64(), Some(expected), "got {n:?}");
    }
    // the script's own accessor sees the lexical state; get_global reads
    // global-OBJECT state (var/function/set_global) — v12 lexical bindings
    // are per-eval, a documented divergence
    assert_eq!(script.call("peek", &[]).unwrap().as_f64(), Some(1.5));

    // `var` state IS global-object state: get_global sees it
    let mut script =
        Script::new("var total = 0; function bump(){ total += 7; return total; }").unwrap();
    script.call("bump", &[]).unwrap();
    assert_eq!(script.get_global("total").unwrap(), Var::I64(7));
}

// ---- (d) object/list through the JSON bridge, both directions ----

#[test]
fn objects_and_lists_cross_both_ways() {
    let mut script = Script::new(
        "function read(cfg) { return cfg.list[1] + cfg.nested.score; } \
         function build() { return {name: 'hg', tags: ['a', 'b'], flag: true, empty: null}; }",
    )
    .unwrap();

    // IN: object (with a nested object and a list) → script reads it
    let cfg = o! {
        "list": List::from_iter([Var::from(1i64), Var::from(2i64), Var::from(3i64)]),
        "nested": o! { "score": 40i64 },
    };
    assert_eq!(
        script.call("read", &[Var::from(cfg)]).unwrap(),
        Var::I64(42)
    );

    // OUT: script builds an object → comes back as a Var object (insertion
    // order kept by JSON), lists as Var lists, null as Null
    let out = script.call("build", &[]).unwrap();
    assert_eq!(out.get("name"), Var::Str("hg".to_string()));
    let tags = out.get("tags");
    assert_eq!(tags.get_idx(0), Var::Str("a".to_string()));
    assert_eq!(tags.get_idx(1), Var::Str("b".to_string()));
    assert_eq!(out.get("flag"), Var::Bool(true));
    assert_eq!(out.get("empty"), Var::Null);

    // round-trip: the object that went out sets as a global and reads back equal
    script.set_global("built", &out).unwrap();
    assert_eq!(script.get_global("built").unwrap(), out);
}

#[test]
fn vec_shapes_cross_as_plain_json_objects() {
    // Vec3/Quat serialize as {x,y,z(,w)} — plain JSON objects, and they come
    // back as plain objects too: the JSON bridge is untyped (documented
    // divergence from gold, whose scripts share typed host objects).
    let mut script = Script::new("function read(v) { return v.x + v.y + v.z; }").unwrap();
    let v = hydrargyros::Vec3::new(1.0, 2.0, 3.0);
    assert_eq!(script.call("read", &[Var::from(v)]).unwrap(), Var::I64(6));

    let mut script = Script::new("function build() { return {x: 1, y: 2, z: 3, w: 4}; }").unwrap();
    let out = script.call("build", &[]).unwrap();
    assert_eq!(out.ty(), hydrargyros::Ty::Object);
    assert_eq!(out.get("w"), Var::I64(4));
}

// ---- (e) THE ENGINE TEST: a scripted component moves its entity ----

#[test]
fn scripted_component_moves_its_transform_across_engine_frames() {
    let script = Script::new(
        "function update(dt) { \
             transform.position.x += 2.0 * dt; \
             transform.position.y += 1.0; \
         }",
    )
    .unwrap();
    let component = ScriptedComponent::new(script).unwrap();
    let handle = component.transform_handle();

    let mut entity = Entity::new("mover");
    entity.add_component(Box::new(component));

    let mut engine = Engine::new("acme", "scripted");
    engine.frame_time(0.0);
    engine.add_entity(entity);

    // the headless update-only path: five deterministic frames at dt = 0.5
    for _ in 0..5 {
        engine.update(0.5);
    }

    // read through the shared handle the component carried into the engine —
    // x: 5 frames × 2.0 × 0.5 = 5.0 ; y: 5 frames × 1.0 = 5.0
    let transform = *handle.lock().unwrap();
    assert!(
        (transform.position.x - 5.0).abs() < 1e-5,
        "x was {}",
        transform.position.x
    );
    assert!(
        (transform.position.y - 5.0).abs() < 1e-5,
        "y was {}",
        transform.position.y
    );
    assert_eq!(transform.rotation.w, 1.0); // untouched

    // the component inside the engine sees the same shared transform
    let component = engine.entities()[0]
        .get_component::<ScriptedComponent>()
        .unwrap();
    assert!((component.transform().position.x - 5.0).abs() < 1e-5);
}

#[test]
fn engine_syncs_the_scripted_transform_into_its_entity() {
    // the engine-side link (Engine::script_sync): the script drives the
    // shared handle and the engine writes it back into the ENTITY's
    // transform after the component pass — the draw pass (and anything
    // reading entity transforms) sees the scripted pose. The entity's
    // spawn pose seeds the script (gold's physicsBody spawns at its entity
    // transform), so incremental scripts start from where the entity was
    // placed — not from the handle's default.
    let script = Script::new(
        "function update(dt) { \
             transform.position.x += 2.0 * dt; \
             transform.position.z -= dt; \
         }",
    )
    .unwrap();
    let component = ScriptedComponent::new(script).unwrap();

    let mut entity = Entity::new("mover");
    entity.transform_mut().position = hydrargyros::Vec3::new(10.0, 2.0, 5.0);
    entity.transform_mut().scale = hydrargyros::Vec3::splat(2.0);
    entity.add_component(Box::new(component));

    let mut engine = Engine::new("acme", "scripted-sync");
    engine.frame_time(0.0);
    engine.add_entity(entity);

    // four deterministic frames at dt = 0.5
    for _ in 0..4 {
        engine.update(0.5);
    }

    // read through the NORMAL entity/transform API — not the handle:
    // x: 10 + 4 frames × 2.0 × 0.5 = 14.0 (seeded from the spawn pose)
    let moved = engine.entities()[0].transform();
    assert!(
        (moved.position.x - 14.0).abs() < 1e-5,
        "x was {}",
        moved.position.x
    );
    assert!(
        (moved.position.z - 3.0).abs() < 1e-5,
        "z was {}",
        moved.position.z
    );
    // y untouched by the script, preserved by the whole-transform round-trip
    assert!((moved.position.y - 2.0).abs() < 1e-5);
    // scale rides the seam untouched too (the script never sees it change)
    assert_eq!(moved.scale, hydrargyros::Vec3::splat(2.0));
}

#[test]
fn missing_script_hooks_are_silently_skipped() {
    // gold's optional lifecycle: a script that defines nothing still rides
    // every hook without a squeak
    let script = Script::new("var idle = true;").unwrap();
    let mut component = ScriptedComponent::new(script).unwrap();
    component.initialize();
    component.update(0.25);
    component.draw();
    assert_eq!(
        component.transform(),
        hydrargyros::game::Transform::default()
    );
}

#[test]
fn initialize_hook_reaches_the_transform_too() {
    let script = Script::new("function initialize() { transform.position.z = 9.0; }").unwrap();
    let mut component = ScriptedComponent::new(script).unwrap();
    component.initialize();
    assert!((component.transform().position.z - 9.0).abs() < 1e-6);
}

#[test]
fn script_breaking_the_transform_shape_fails_loud() {
    let script = Script::new("function update(dt) { transform = 5; }").unwrap();
    let mut component = ScriptedComponent::new(script).unwrap();
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        component.update(0.1);
    }));
    let msg = err.expect_err("breaking the transform shape must fail loud");
    let msg = msg
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| msg.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_default();
    assert!(
        msg.contains("scripted component 'update' failed"),
        "got {msg}"
    );
    assert!(msg.contains("transform"), "got {msg}");
}

#[test]
fn hook_names_are_configurable() {
    let script = Script::new("function tick(dt) { transform.position.x += dt; }").unwrap();
    let mut component = ScriptedComponent::with_hooks(
        script,
        hydrargyros::game::HookNames {
            update: "tick".to_string(),
            ..Default::default()
        },
    )
    .unwrap();
    component.update(2.0);
    assert!((component.transform().position.x - 2.0).abs() < 1e-6);
}

#[test]
fn priority_is_configurable() {
    let component = ScriptedComponent::new(Script::new("").unwrap())
        .unwrap()
        .with_priority(hydrargyros::game::Priority::High);
    assert_eq!(component.priority(), hydrargyros::game::Priority::High);
}

// ---- (f) the pool design holds: the Send+Sync handle crosses threads ----

#[test]
fn script_handle_moves_into_another_thread() {
    let mut script = Script::new("function answer() { return 41 + 1; }").unwrap();

    // the handle crosses; first use on the new thread materializes a fresh
    // context THERE (the !Send context never crosses)
    let result = thread::spawn(move || script.call("answer", &[]).unwrap())
        .join()
        .unwrap();
    assert_eq!(result, Var::I64(42));
}

#[test]
fn host_closures_follow_the_script_across_threads() {
    let mut script = Script::new("function go(n) { return double(n) + offset; }").unwrap();
    script.register("double", |args| {
        Var::from(args[0].as_f64().unwrap_or(0.0) * 2.0)
    });

    // per-(script, thread) state: globals set on the main thread stay here —
    // the new thread sets its own (gold's load semantics, per thread)
    let result = thread::spawn(move || {
        script.set_global("offset", &Var::from(3i64)).unwrap();
        script.call("go", &[Var::from(20i64)]).unwrap()
    })
    .join()
    .unwrap();
    assert_eq!(result, Var::I64(43));
}

#[test]
fn host_fn_may_call_into_another_script() {
    // the frame stack exists so host closures can re-enter the bridge —
    // into OTHER scripts on this thread (the caller's entry is checked out
    // of the pool while it runs; the service's is untouched)
    let service = Arc::new(Mutex::new(
        Script::new("function twice(n) { return n * 2; }").unwrap(),
    ));
    let mut caller = Script::new("function run(n) { return useService(n) + 1; }").unwrap();

    caller.register("useService", move |args| {
        let mut svc = service.lock().unwrap();
        svc.call("twice", &[args[0].clone()]).unwrap()
    });

    assert_eq!(
        caller.call("run", &[Var::from(20i64)]).unwrap(),
        Var::I64(41)
    );
}
