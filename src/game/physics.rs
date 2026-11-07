//! Physics — gold's `world`/`physicsBackend` split: a
//! [`PhysicsWorldBackend`] trait with register/create registries, the
//! built-in no-op `none` backend (worlds initialize, bodies no-op,
//! transforms stay under script control — gold ships the same fallback),
//! and the real simulation behind the `physics` feature: **rapier3d**, the
//! maintained pure-Rust rigid-body engine (gravity, stepping, raycasts —
//! the wheel gold loads Bullet for).
//!
//! The [`PhysicsWorld`] facade is what entities and tests talk to: bodies
//! are named (the script-facing shape), the [`PhysicsBody`](crate::game::PhysicsBody)
//! component materializes into it, and the engine syncs body poses back to
//! entity transforms every frame — gold's step-and-sync.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use crate::var::{Quat, Vec3};
use crate::var::Var;

/// A collision shape (gold's `boxShape`/`sphereShape` kinds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    /// Half-extents on each axis.
    Box(Vec3),
    /// Radius.
    Sphere(f32),
}

/// How a body responds to the simulation (gold's body kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyType {
    /// Driven by gravity and impulses.
    Dynamic,
    /// Immovable (walls, floors).
    Static,
}

/// One hit of a [`PhysicsWorld::raycast`].
#[derive(Debug, Clone, PartialEq)]
pub struct RayHit {
    /// The name of the body that was hit.
    pub body: String,
    pub point: Vec3,
    pub normal: Vec3,
    /// Distance along the ray (`toi`).
    pub distance: f32,
}

/// A per-world physics simulation behind the facade. One trait object per
/// world — gold's `physicsBackend::createWorld` flattened into the natural
/// Rust shape.
pub trait PhysicsWorldBackend: Send {
    fn name(&self) -> &str;

    fn set_gravity(&mut self, gravity: Vec3);

    /// Advance the simulation by `dt` seconds.
    fn step(&mut self, dt: f32);

    /// Create a body; the backend owns its id. Re-registering a name
    /// replaces the old body (gold's replace semantics).
    ///
    /// `mass` is gold's `mass` proto knob, and only affects DYNAMIC bodies:
    /// 0 (the default) keeps the shape's density-derived mass, an explicit
    /// value becomes the body's whole mass. Ignored on static bodies — the
    /// port keeps `BodyType` explicit rather than gold's `mass != 0`
    /// dynamic collapse (we have both knobs).
    // the whole body spec in one call — gold's createBody(comp, w)
    #[allow(clippy::too_many_arguments)]
    fn add_body(
        &mut self,
        name: &str,
        shape: &Shape,
        body_type: BodyType,
        position: Vec3,
        rotation: Quat,
        gravity_scale: f32,
        mass: f32,
    );

    /// The body's current pose; `None` when the backend keeps no rigid
    /// state (the `none` backend — transforms stay under script control).
    fn body_pose(&self, name: &str) -> Option<(Vec3, Quat)>;

    /// Cast a ray; nearest hit within `max_distance`.
    fn raycast(&mut self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<RayHit>;
}

/// Factory shape for the backend registry: config `Var` in, backend out.
pub type PhysicsFactory =
    Arc<dyn Fn(&Var) -> Result<Box<dyn PhysicsWorldBackend>, String> + Send + Sync>;
type BackendMap = HashMap<String, PhysicsFactory>;

fn backends() -> &'static RwLock<BackendMap> {
    static BACKENDS: OnceLock<RwLock<BackendMap>> = OnceLock::new();
    BACKENDS.get_or_init(|| {
        let mut map: BackendMap = HashMap::new();
        map.insert("none".to_string(), Arc::new(|_config| Ok(Box::new(NoPhysicsBackend))));
        #[cfg(feature = "physics")]
        map.insert("rapier".to_string(), Arc::new(|_config| {
            Ok(Box::new(super::physics_rapier::RapierBackend::new()))
        }));
        RwLock::new(map)
    })
}

/// Register a physics backend under a name (gold's plugin registration).
pub fn register_physics_backend(name: &str, factory: PhysicsFactory) {
    backends().write().unwrap().insert(name.to_string(), factory);
}

/// Create a physics world backend by name.
///
/// # Errors
/// Unknown backend name, or the backend's factory failure.
pub fn create_physics_backend(
    name: &str,
    config: &Var,
) -> Result<Box<dyn PhysicsWorldBackend>, String> {
    let factory = backends()
        .read()
        .unwrap()
        .get(name)
        .cloned()
        .ok_or_else(|| format!("unknown physics backend {name:?}"))?;
    factory(config)
}

/// The built-in no-op backend (gold's "none"): worlds initialize, bodies
/// register, nothing simulates, raycasts miss — and entity transforms stay
/// under script control.
#[derive(Default)]
pub struct NoPhysicsBackend;

impl PhysicsWorldBackend for NoPhysicsBackend {
    fn name(&self) -> &str { "none" }
    fn set_gravity(&mut self, _gravity: Vec3) {}
    fn step(&mut self, _dt: f32) {}
    fn add_body(
        &mut self,
        _name: &str,
        _shape: &Shape,
        _body_type: BodyType,
        _position: Vec3,
        _rotation: Quat,
        _gravity_scale: f32,
        _mass: f32,
    ) {
    }
    fn body_pose(&self, _name: &str) -> Option<(Vec3, Quat)> { None }
    fn raycast(&mut self, _origin: Vec3, _direction: Vec3, _max_distance: f32) -> Option<RayHit> {
        None
    }
}

/// The world facade (gold's `world`): named bodies, gravity, stepping,
/// raycasts — over whichever backend it was created with.
pub struct PhysicsWorld {
    backend: Box<dyn PhysicsWorldBackend>,
}

impl PhysicsWorld {
    /// A world over the named backend.
    ///
    /// # Errors
    /// Unknown backend name, or the backend's factory failure.
    pub fn create(backend: &str, config: &Var) -> Result<PhysicsWorld, String> {
        Ok(PhysicsWorld { backend: create_physics_backend(backend, config)? })
    }

    /// A world over the built-in no-op backend.
    pub fn none() -> PhysicsWorld {
        PhysicsWorld { backend: Box::new(NoPhysicsBackend) }
    }

    pub fn backend_name(&self) -> &str { self.backend.name() }

    pub fn set_gravity(&mut self, gravity: Vec3) {
        self.backend.set_gravity(gravity);
    }

    // the whole body spec in one call — gold's createBody(comp, w)
    #[allow(clippy::too_many_arguments)]
    pub fn add_body(
        &mut self,
        name: &str,
        shape: &Shape,
        body_type: BodyType,
        position: Vec3,
        rotation: Quat,
        gravity_scale: f32,
        mass: f32,
    ) {
        self.backend.add_body(name, shape, body_type, position, rotation, gravity_scale, mass);
    }

    pub fn step(&mut self, dt: f32) {
        self.backend.step(dt);
    }

    pub fn body_pose(&self, name: &str) -> Option<(Vec3, Quat)> {
        self.backend.body_pose(name)
    }

    pub fn raycast(&mut self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<RayHit> {
        self.backend.raycast(origin, direction, max_distance)
    }

}
