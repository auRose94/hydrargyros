//! Physics tests (behind the `physics` feature): the rapier world sim —
//! gravity, mass, falling bodies, raycasts — plus the none-backend contract.

#![cfg(feature = "physics")]

use hydrargyros::game::physics::{BodyType, PhysicsWorld, Shape};
use hydrargyros::var::{Quat, Vec3};
use hydrargyros::Var;

#[test]
fn gravity_pulls_dynamic_bodies_down() {
    let mut world = PhysicsWorld::create("rapier", &Var::Null).unwrap();
    assert_eq!(world.backend_name(), "rapier");
    world.add_body("ball", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 10.0, 0.0), Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, 1.0, 0.0);

    let dt = 1.0 / 60.0;
    let start_y = world.body_pose("ball").unwrap().0.y;
    for _ in 0..30 {
        world.step(dt);
    }
    let end_y = world.body_pose("ball").unwrap().0.y;
    assert!(end_y < start_y - 1.0, "body should have fallen: {start_y} → {end_y}");
}

#[test]
fn static_bodies_hold_the_line() {
    let mut world = PhysicsWorld::create("rapier", &Var::Null).unwrap();
    world.add_body("floor", &Shape::Box(Vec3::new(10.0, 0.5, 10.0)), BodyType::Static, Vec3::new(0.0, 0.0, 0.0), Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, 0.0, 0.0);
    let id = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };
    world.add_body("ball", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 5.0, 0.0), id, 1.0, 0.0);

    let dt = 1.0 / 60.0;
    for _ in 0..240 {
        world.step(dt);
    }
    let (pos, _) = world.body_pose("ball").unwrap();
    // the ball came to rest on the floor (sphere radius 0.5, floor top 0.5)
    assert!(pos.y < 3.0, "should have fallen far: {pos:?}");
    assert!(pos.y > 0.5, "should rest on the floor, not through it: {pos:?}");
}

#[test]
fn raycast_names_the_hit_body() {
    let mut world = PhysicsWorld::create("rapier", &Var::Null).unwrap();
    world.add_body("target", &Shape::Sphere(1.0), BodyType::Static, Vec3::new(0.0, 0.0, -10.0), Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, 0.0, 0.0);

    // straight at the sphere's center
    let hit = world
        .raycast(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, -1.0), 100.0)
        .expect("ray should hit the sphere");
    assert_eq!(hit.body, "target");
    assert!((hit.distance - 9.0).abs() < 1e-3, "hit at the sphere's near surface: {}", hit.distance);
    assert!((hit.point.z - (-9.0)).abs() < 1e-3);

    // and away from it: no hit
    assert!(world.raycast(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0), 100.0).is_none());
}

#[test]
fn re_registering_a_name_replaces_the_body() {
    let mut world = PhysicsWorld::create("rapier", &Var::Null).unwrap();
    let id = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };
    world.add_body("b", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 10.0, 0.0), id, 1.0, 0.0);
    world.step(1.0 / 60.0);
    // replace: back at the fresh position
    world.add_body("b", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 10.0, 0.0), id, 1.0, 0.0);
    let (pos, rot) = world.body_pose("b").unwrap();
    assert_eq!(pos, Vec3::new(0.0, 10.0, 0.0));
    assert_eq!(rot, Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 });
}

// ---- mass: gold's `mass` proto knob ----

#[test]
fn mass_scales_momentum_transfer() {
    let mut world = PhysicsWorld::create("rapier", &Var::Null).unwrap();
    let id = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };
    // gravity along -x accelerates the heavy body into the light one; the
    // light body is gravity-immune (gravity_scale 0) so it only moves when
    // hit — and a lone twin shows an untouched light body doesn't drift
    world.set_gravity(Vec3::new(-10.0, 0.0, 0.0));
    world.add_body("heavy", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(8.0, 0.0, 0.0), id, 1.0, 100.0);
    world.add_body("light", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 0.0, 0.0), id, 0.0, 0.01);
    world.add_body("lone", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 5.0, 0.0), id, 0.0, 0.01);

    let dt = 1.0 / 60.0;
    for _ in 0..180 {
        world.step(dt);
    }

    let heavy = world.body_pose("heavy").unwrap().0;
    let light = world.body_pose("light").unwrap().0;
    let lone = world.body_pose("lone").unwrap().0;
    // control: nothing touched the lone one, so it never moved — the impact
    // is the only thing that could have displaced "light"
    assert!(lone.x.abs() < 1e-3 && (lone.y - 5.0).abs() < 1e-3, "control body drifted: {lone:?}");
    // 100 kg into 0.01 kg: the light one is driven far down-range while the
    // heavy plow barely slows (momentum transfer scales with mass)
    assert!(light.x < -2.0, "light body should have been knocked down-range: {light:?}");
    // and the heavy one trails it — no tunneling, no dead stop
    assert!(heavy.x > light.x, "heavy should trail the light body: heavy={heavy:?} light={light:?}");
}

#[test]
fn free_fall_is_mass_independent() {
    let mut world = PhysicsWorld::create("rapier", &Var::Null).unwrap();
    let id = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };
    // the equivalence principle: gravity accelerates every mass the same,
    // so two spheres of wildly different mass dropped together stay level
    world.set_gravity(Vec3::new(0.0, -9.81, 0.0));
    world.add_body("feather", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(0.0, 10.0, 0.0), id, 1.0, 0.01);
    world.add_body("anvil", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(2.0, 10.0, 0.0), id, 1.0, 100.0);

    let dt = 1.0 / 60.0;
    for _ in 0..60 {
        world.step(dt);
    }

    let feather = world.body_pose("feather").unwrap().0;
    let anvil = world.body_pose("anvil").unwrap().0;
    // both fell (about 5 m in 1 s)...
    assert!(feather.y < 6.0, "feather should have fallen: {feather:?}");
    assert!(anvil.y < 6.0, "anvil should have fallen: {anvil:?}");
    // ...and stayed level — mass must not affect free fall
    assert!(
        (feather.y - anvil.y).abs() < 1e-3,
        "mass must not affect free fall: feather={feather:?} anvil={anvil:?}"
    );
}

// ---- the none backend: gold's contract ----

#[test]
fn none_backend_initializes_and_no_ops() {
    let mut world = PhysicsWorld::none();
    assert_eq!(world.backend_name(), "none");
    world.set_gravity(Vec3::new(0.0, -9.81, 0.0));
    world.add_body("ghost", &Shape::Sphere(0.5), BodyType::Dynamic, Vec3::new(1.0, 2.0, 3.0), Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, 1.0, 0.0);
    world.step(1.0 / 60.0);
    // no rigid state: body_pose is None, so transforms stay under script control
    assert!(world.body_pose("ghost").is_none());
    assert!(world.raycast(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), 10.0).is_none());
}

#[test]
fn unknown_backend_is_refused() {
    assert!(PhysicsWorld::create("dreamcast", &Var::Null).is_err());
}
