//! The rapier3d physics backend (gold's libgoldBullet slot): rigid bodies
//! with box/sphere colliders, gravity, stepping, and raycasts, over
//! rapier's aggregate `PhysicsWorld`.

use std::collections::HashMap;

use rapier3d::prelude::*;

use crate::var::{Quat, Vec3};

use super::physics::{BodyType, PhysicsWorldBackend, RayHit, Shape};

/// A rapier-backed simulation. Named bodies map to rigid-body handles so
/// the facade and the raycasts stay name-addressable.
pub struct RapierBackend {
    world: PhysicsWorld,
    /// body name → handles (re-registering a name replaces).
    bodies_by_name: HashMap<String, NamedBody>,
    /// collider → owning body's name (raycast hits resolve through here).
    collider_owner: HashMap<ColliderHandle, String>,
}

#[derive(Clone, Copy)]
struct NamedBody {
    body: RigidBodyHandle,
    collider: ColliderHandle,
}

impl RapierBackend {
    pub fn new() -> Self {
        RapierBackend {
            world: PhysicsWorld::new(),
            bodies_by_name: HashMap::new(),
            collider_owner: HashMap::new(),
        }
    }

    /// The collider; an explicit mass rides it (rapier computes the shape's
    /// inertia at that mass — gold's `calculateLocalInertia(mass, …)`):
    /// dynamic bodies weigh the knob verbatim, everything else keeps the
    /// density default (mass 0, or static bodies where mass is ignored).
    fn collider(shape: &Shape, body_type: BodyType, mass: f32) -> Collider {
        let builder = match shape {
            Shape::Box(half_extents) => {
                ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
            }
            Shape::Sphere(radius) => ColliderBuilder::ball(*radius),
        };
        match body_type {
            BodyType::Dynamic if mass > 0.0 => builder.mass(mass).build(),
            _ => builder.build(),
        }
    }
}

impl Default for RapierBackend {
    fn default() -> Self { Self::new() }
}

impl RapierBackend {
    /// Mercury's quaternion → rapier's rotation vector (axis × angle).
    fn rotation_vector(rotation: Quat) -> Vector {
        let w = rotation.w.clamp(-1.0, 1.0);
        let angle = 2.0 * w.acos();
        let sin_half = (1.0 - w * w).sqrt();
        if sin_half < 1e-6 {
            return Vector::ZERO;
        }
        let k = angle / sin_half;
        Vector::new(rotation.x * k, rotation.y * k, rotation.z * k)
    }
}

impl PhysicsWorldBackend for RapierBackend {
    fn name(&self) -> &str { "rapier" }

    fn set_gravity(&mut self, gravity: Vec3) {
        self.world.gravity = Vector::new(gravity.x, gravity.y, gravity.z);
        // the knob takes effect immediately: settled bodies are ASLEEP, and
        // rapier only wakes a body on its own state changing — a gravity
        // flip would silently no-op on any resting scene (a live demo caught
        // this: the antigravity frame was identical to the settled one).
        // gold's bullet backend only ever sets gravity at world init, so
        // this wake-everything is the port's own semantic for a live knob.
        for named in self.bodies_by_name.values() {
            if let Some(body) = self.world.bodies.get_mut(named.body) {
                body.wake_up(true);
            }
        }
    }

    fn step(&mut self, dt: f32) {
        self.world.integration_parameters.dt = dt;
        self.world.step();
    }

    fn add_body(
        &mut self,
        name: &str,
        shape: &Shape,
        body_type: BodyType,
        position: Vec3,
        rotation: Quat,
        gravity_scale: f32,
        mass: f32,
    ) {
        // re-registering a name replaces the old body (gold's replace semantics)
        if let Some(old) = self.bodies_by_name.remove(name) {
            self.collider_owner.remove(&old.collider);
            let Self { world, .. } = self;
            world.bodies.remove(
                old.body,
                &mut world.islands,
                &mut world.colliders,
                &mut world.impulse_joints,
                &mut world.multibody_joints,
                &mut world.soft_bodies,
                true,
            );
        }
        let rigid = match body_type {
            BodyType::Dynamic => RigidBodyBuilder::dynamic(),
            BodyType::Static => RigidBodyBuilder::fixed(),
        }
        .translation(rapier3d::math::Vec3::new(position.x, position.y, position.z))
        .rotation(Self::rotation_vector(rotation))
        .gravity_scale(gravity_scale)
        .build();
        let (body_handle, collider_handle) =
            self.world.insert(rigid, Self::collider(shape, body_type, mass));
        self.collider_owner.insert(collider_handle, name.to_string());
        self.bodies_by_name
            .insert(name.to_string(), NamedBody { body: body_handle, collider: collider_handle });
        // flush the broad phase / query snapshot so raycasts see the new
        // body before the next real step (a zero-dt step moves nothing)
        self.world.integration_parameters.dt = 0.0;
        self.world.step();
        self.world.integration_parameters.dt = 1.0 / 60.0;
    }

    fn body_pose(&self, name: &str) -> Option<(Vec3, Quat)> {
        let named = self.bodies_by_name.get(name)?;
        let body = self.world.bodies.get(named.body)?;
        let t = body.translation();
        let r = body.rotation();
        Some((
            Vec3::new(t.x, t.y, t.z),
            Quat { x: r.x, y: r.y, z: r.z, w: r.w },
        ))
    }

    fn raycast(&mut self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<RayHit> {
        let dir_len =
            (direction.x * direction.x + direction.y * direction.y + direction.z * direction.z)
                .sqrt();
        if dir_len == 0.0 {
            return None;
        }
        let ray = Ray::new(
            rapier3d::math::Vec3::new(origin.x, origin.y, origin.z),
            rapier3d::math::Vec3::new(
                direction.x / dir_len,
                direction.y / dir_len,
                direction.z / dir_len,
            ),
        );
        let (collider_handle, intersection) = self
            .world
            .cast_ray_and_get_normal(&ray, max_distance, true, QueryFilter::default())?;
        let body = self.collider_owner.get(&collider_handle)?.clone();
        let hit_point = ray.point_at(intersection.time_of_impact);
        let n = intersection.normal;
        Some(RayHit {
            body,
            point: Vec3::new(hit_point.x, hit_point.y, hit_point.z),
            normal: Vec3::new(n.x, n.y, n.z),
            distance: intersection.time_of_impact,
        })
    }
}
