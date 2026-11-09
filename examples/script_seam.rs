//! The scripting seam (the `script` feature): v12 underneath, `Var`
//! across. Scripts are values — construct, call, register host functions,
//! round-trip globals — and a `ScriptedComponent` drives an entity through
//! the engine's transform seam in a plain headless pump.
//!
//! Run:
//!   cargo run --example script_seam --features script

use hyg::game::{Engine, Entity, HeadlessWindow, ScriptedComponent};
use hyg::script::Script;
use hyg::var::Vec3;
use hyg::{o, Var};

fn main() {
    // ---- a script is a value: construct + call by name ----
    let mut script = Script::new(
        "function greet(who) { return 'hello ' + who + ' — from v12'; } \
         function viaHost(n) { return double(n) + 1; }",
    )
    .expect("script compiles");
    let out = script.call("greet", &[Var::from("hyg")]).unwrap();
    println!("call: {}", out.as_str().unwrap_or("?"));

    // ---- host functions cross IN as Var::Func closures ----
    script.register("double", |args: &[Var]| {
        let n = args.first().and_then(|v| v.as_i64()).unwrap_or(0);
        Var::from(n * 2)
    });
    let out = script.call("viaHost", &[Var::from(20i64)]).unwrap();
    println!("host fn: viaHost(20) = {}", out.as_i64().unwrap_or(0));

    // ---- globals round-trip as Var (the JSON bridge) ----
    script
        .set_global("config", &Var::from(o!{ "name": "pacer", "speed": 2 }))
        .unwrap();
    let back = script.get_global("config").unwrap();
    println!(
        "global round-trip: name={} speed={}",
        back.get("name").as_str().unwrap_or("?"),
        back.get("speed").as_i64().unwrap_or(0),
    );

    // ---- the component seam: the script drives the entity transform ----
    // top-level `let` state survives across calls (same-eval closures), so
    // `t` accumulates; the engine seeds the component's transform handle
    // from the entity's spawn pose and syncs writes back after each pass.
    const PACE: &str = "let t = 0; \
                        function update(dt) { \
                            t += dt; \
                            transform.position.x = 1.0 + 2.0 * t; \
                        }";
    let mut engine = Engine::new("acme", "script-seam");
    engine.frame_time(0.0); // headless: no vsync cap
    engine.fixed_dt(1.0 / 60.0); // deterministic: 60 frames = exactly 1s of script time

    let mut pacer = Entity::new("pacer");
    pacer.transform_mut().position = Vec3::new(1.0, 0.0, 0.0);
    pacer.add_component(Box::new(
        ScriptedComponent::new(Script::new(PACE).expect("paces")).expect("arms"),
    ));
    engine.add_entity(pacer);

    println!("pumping 60 headless frames at dt=1/60…");
    engine.start_with(Box::new(HeadlessWindow::for_frames(60)));

    let pose = engine
        .entity_mut("pacer")
        .expect("pacer still here")
        .transform()
        .position;
    println!(
        "scripted pose: x={:.3} (spawned at 1.0; 61 in-flight frames at dt=1/60 → ~3.03)",
        pose.x
    );
}
