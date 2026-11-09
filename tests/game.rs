//! Game-module tests: the pump, priority ordering, entity hierarchies, the
//! backend registry, and the Var event seam — all headless, like gold's
//! `windowSystemHeadless`.

use std::sync::{Arc, Mutex};

use hyg::game::component::{Component, Priority};
use hyg::game::window::{create_window_backend, register_window_backend, WindowSystem};
use hyg::game::{Engine, Entity, Transform};
use hyg::var::{Quat, Vec3};
use hyg::{o, Var};

/// Logs the frame order of whatever components wear it.
#[derive(Clone)]
struct Trace {
    label: &'static str,
    log: Arc<Mutex<Vec<&'static str>>>,
    priority: Priority,
}

impl Component for Trace {
    fn priority(&self) -> Priority { self.priority }
    fn initialize(&mut self) { self.log.lock().unwrap().push("init"); }
    fn update(&mut self, _dt: f64) { self.log.lock().unwrap().push(self.label); }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

#[test]
fn components_update_in_priority_order() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new("acme", "order-test");

    // added out of order; the frame sorts by tier
    let tiers = [
        (Priority::Draw, "draw"),
        (Priority::High, "high"),
        (Priority::Generic, "generic"),
        (Priority::Physics, "physics"),
    ];
    for (priority, label) in tiers {
        let mut e = Entity::new(label);
        e.add_component(Box::new(Trace { label, log: log.clone(), priority }));
        engine.add_entity(e);
    }

    engine.update(0.016);
    let seen = log.lock().unwrap().clone();
    // init runs per slot, right before that slot's first update
    assert_eq!(seen.iter().filter(|s| **s == "init").count(), 4);
    let updates: Vec<_> = seen.iter().filter(|s| **s != "init").copied().collect();
    assert_eq!(updates, vec!["high", "physics", "generic", "draw"]);
}

#[test]
fn initialize_runs_once_per_component() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new("acme", "init-test");
    let mut e = Entity::new("a");
    e.add_component(Box::new(Trace {
        label: "a",
        log: log.clone(),
        priority: Priority::Generic,
    }));
    engine.add_entity(e);

    engine.update(0.016);
    engine.update(0.016);
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen.iter().filter(|s| **s == "init").count(), 1);
}

#[test]
fn headless_pump_runs_scripted_events_then_quits() {
    // gold's frame: events first, quit stops the loop after the in-flight
    // frame completes
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new("acme", "pump-test");
    engine.frame_time(0.0);
    let mut e = Entity::new("c");
    e.add_component(Box::new(Trace {
        label: "c",
        log: log.clone(),
        priority: Priority::Generic,
    }));
    engine.add_entity(e);

    let events = vec![
        Var::from(o!{ "type": "key", "key": "w" }),
        Var::from(o!{ "type": "mouse", "x": 3 }),
    ];
    engine.start_with(Box::new(
        hyg::game::HeadlessWindow::with_events(events),
    ));

    assert!(!engine.running());
    // frame 1: two events + update; frame 2: quit event + update; loop exits
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen, vec!["init", "c", "c"]);
}

#[test]
fn disabled_entities_are_skipped() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new("acme", "disable-test");
    let mut on = Entity::new("on");
    on.add_component(Box::new(Trace {
        label: "on",
        log: log.clone(),
        priority: Priority::Generic,
    }));
    let mut off = Entity::new("off");
    off.add_component(Box::new(Trace {
        label: "off",
        log: log.clone(),
        priority: Priority::Generic,
    }));
    off.set_enabled(false);
    engine.add_entity(on);
    engine.add_entity(off);

    engine.update(0.016);
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen, vec!["init", "on"]);
}

#[test]
fn typed_component_lookup() {
    struct Health(f64);
    impl Component for Health {
        fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
    }

    let mut parent = Entity::new("parent");
    parent.add_component(Box::new(Health(100.0)));
    let mut child = Entity::new("child");
    child.add_component(Box::new(Health(40.0)));
    parent.add_child(child);

    assert_eq!(parent.get_component::<Health>().unwrap().0, 100.0);
    let all: Vec<f64> = parent
        .get_components_recursive::<Health>()
        .into_iter()
        .map(|h| h.0)
        .collect();
    assert_eq!(all, vec![100.0, 40.0]);
}

#[test]
fn transform_composes_parent_first() {
    // parent rotated 90° around Z; child one unit along parent-local +X
    let parent = Transform {
        position: Vec3::new(10.0, 20.0, 0.0),
        rotation: Quat::axis_angle(Vec3::new(0.0, 0.0, 1.0), std::f64::consts::FRAC_PI_2 as f32),
        scale: Vec3::splat(2.0),
    };
    let mut local = Transform::new();
    local.position = Vec3::new(1.0, 0.0, 0.0);

    let world = local.compose(&parent);
    // scaled 2x, rotated a quarter turn: (1,0,0) → (0,2,0), plus parent pos
    assert!((world.position.x - 10.0).abs() < 1e-5);
    assert!((world.position.y - 22.0).abs() < 1e-5);
}

#[test]
fn window_backend_registry() {
    // headless is pre-registered (gold ships a headless window system too)
    let mut window = create_window_backend("headless", &Var::Null).unwrap();
    assert!(window.poll().is_empty());

    // unknown names are refused
    assert!(create_window_backend("dreamcast", &Var::Null).is_err());

    // custom backends register under a name
    register_window_backend(
        "always-quit",
        Arc::new(|_config| Ok(Box::new(hyg::game::HeadlessWindow::with_events([])) as Box<dyn WindowSystem>)),
    );
    assert!(create_window_backend("always-quit", &Var::Null).is_ok());
}

#[test]
fn settings_roundtrip() {
    let dir = std::env::temp_dir().join("hyg_engine_settings_test");
    let mut engine = Engine::new("acme", "settings-test");
    engine.settings_root(&dir);

    engine.save_settings(&Var::from(o!{ "volume": 0.5, "fullscreen": true })).unwrap();
    let loaded = engine.load_settings().unwrap();
    assert_eq!(loaded.get("volume").as_f64(), Some(0.5));
    assert_eq!(loaded.get("fullscreen").as_bool(), Some(true));

    let _ = std::fs::remove_dir_all(&dir);
}

// update() is the headless frame API: drive frames without a window.
#[test]
fn frames_can_be_driven_without_a_window() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let mut engine = Engine::new("acme", "manual-frames");
    let mut e = Entity::new("c");
    e.add_component(Box::new(Trace {
        label: "c",
        log: log.clone(),
        priority: Priority::Generic,
    }));
    engine.add_entity(e);

    engine.update(0.016);
    engine.update(0.016);
    engine.update(0.016);
    let seen = log.lock().unwrap().clone();
    assert_eq!(seen, vec!["init", "c", "c", "c"]);
}

// keep WindowSystem referenced outside the registry test
#[test]
fn window_trait_is_object_safe_over_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Box<dyn WindowSystem>>();
}

// ---- physics integration: the engine materializes bodies and syncs poses ----

#[test]
fn none_backend_keeps_transforms_under_script_control() {
    use hyg::game::{PhysicsBody, PhysicsWorld};
    use hyg::var::Vec3;
    use hyg::game::physics::Shape;

    let mut engine = Engine::new("acme", "none-physics");
    engine.set_physics(PhysicsWorld::none());
    let mut e = Entity::new("ghost");
    e.transform_mut().position = Vec3::new(1.0, 2.0, 3.0);
    // mass rides the materialization seam; the none backend ignores it
    e.add_component(Box::new(PhysicsBody::new("ghost", Shape::Sphere(0.5)).mass(50.0)));
    engine.add_entity(e);

    // several frames: the none backend never reports a pose, so the
    // transform stays exactly where the script put it
    for _ in 0..10 {
        engine.update(1.0 / 60.0);
    }
    let t = engine.entities()[0].transform();
    assert_eq!(t.position, Vec3::new(1.0, 2.0, 3.0));
}

#[cfg(feature = "physics")]
#[test]
fn rapier_world_syncs_falling_poses_into_transforms() {
    use hyg::game::{PhysicsBody, PhysicsWorld};
    use hyg::var::Vec3;
    use hyg::game::physics::Shape;

    let mut engine = Engine::new("acme", "rapier-sync");
    engine.frame_time(0.0);
    engine.set_physics(PhysicsWorld::create("rapier", &Var::Null).unwrap());
    let mut e = Entity::new("ball");
    e.transform_mut().position = Vec3::new(0.0, 10.0, 0.0);
    // the component's mass rides the materialization seam into the world;
    // free fall is mass-independent (equivalence principle), so the fall is
    // unchanged — the tight mass assertions live in tests/physics.rs
    e.add_component(Box::new(PhysicsBody::new("ball", Shape::Sphere(0.5)).mass(2.0)));
    engine.add_entity(e);

    let dt = 1.0 / 60.0;
    for _ in 0..60 {
        engine.update(dt);
    }
    let y = engine.entities()[0].transform().position.y;
    assert!(y < 10.0 - 1.0, "the engine should have synced the fall: y={y}");
    assert!(y > 0.0, "should still be well above a floor it never had: y={y}");
}
