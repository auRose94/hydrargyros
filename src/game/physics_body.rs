//! The physics body component — gold's `physicsBody`: config on an entity,
//! materialized into the engine's world on its initialize frame, pose
//! synced back to the entity transform every step.

use super::component::{Component, Priority};
use super::physics::{BodyType, Shape};
/// A rigid body attached to an entity. The engine registers it with the
/// physics world (gold's `physicsPriority` tier) and, after every step,
/// writes the body's pose back into the entity's transform — except under
/// the `none` backend, where transforms stay under script control.
pub struct PhysicsBody {
    pub(crate) name: String,
    pub(crate) shape: Shape,
    pub(crate) body_type: BodyType,
    pub(crate) gravity_scale: f32,
    pub(crate) mass: f32,
    pub(crate) materialized: bool,
}

impl PhysicsBody {
    pub fn new(name: impl Into<String>, shape: Shape) -> Self {
        PhysicsBody {
            name: name.into(),
            shape,
            body_type: BodyType::Dynamic,
            gravity_scale: 1.0,
            mass: 0.0,
            materialized: false,
        }
    }

    /// Static (immovable) instead of dynamic.
    pub fn static_body(mut self) -> Self {
        self.body_type = BodyType::Static;
        self
    }

    /// 0 = immune to gravity, 2 = double.
    pub fn gravity_scale(mut self, scale: f32) -> Self {
        self.gravity_scale = scale;
        self
    }

    /// gold's `mass` proto knob (default 0). Only affects DYNAMIC bodies:
    /// 0 keeps the shape's density-derived mass, an explicit value becomes
    /// the body's whole mass. Ignored on static bodies — a deliberate
    /// divergence, since gold collapses static/dynamic into `mass != 0`
    /// while the port keeps `BodyType` explicit (we have both knobs).
    pub fn mass(mut self, mass: f32) -> Self {
        self.mass = mass;
        self
    }

    pub fn name(&self) -> &str { &self.name }
}

impl Component for PhysicsBody {
    fn priority(&self) -> Priority { Priority::Physics }

    fn as_any(&self) -> &dyn std::any::Any { self }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}
