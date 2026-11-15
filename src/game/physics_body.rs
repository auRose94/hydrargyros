//! The physics body component: config on an entity,
//! materialized into the engine's world on its initialize frame, pose
//! synced back to the entity transform every step.

use super::component::{Component, Priority};
use super::physics::{BodyType, Shape};
use super::scene::ComponentContext;
use crate::object::Object;
use crate::var::Var;
use crate::{FromVar, FromVarError};
/// A rigid body attached to an entity. The engine registers it with the
/// physics world (the `Priority::Physics` tier) and, after every step,
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

    /// The mass knob (default 0). Only affects DYNAMIC bodies:
    /// 0 keeps the shape's density-derived mass, an explicit value becomes
    /// the body's whole mass. Ignored on static bodies — `BodyType` stays
    /// explicit rather than overloading `mass != 0` to mean dynamic
    /// (we have both knobs).
    pub fn mass(mut self, mass: f32) -> Self {
        self.mass = mass;
        self
    }

    pub fn name(&self) -> &str { &self.name }
}

impl PhysicsBody {
    /// Build from a scene document's params object (the component
    /// registry's `physics_body` factory): `shape` (`"box"` with
    /// `half_extents` `[x, y, z]`, or `"sphere"` with `radius`), optional
    /// `name` (default: the entity's name — the script-facing body
    /// handle), `body` (`"dynamic"`/`"static"`), `gravity_scale`, `mass`.
    /// Strict, path-tracked reads.
    ///
    /// # Errors
    /// A `FromVarError` at the offending field.
    pub fn from_params(v: &Var, entity_name: &str) -> Result<PhysicsBody, FromVarError> {
        let shape = match super::scene::req_string(v, "shape")?.as_str() {
            "box" => {
                let Some(half_extents) = super::scene::opt_field(v, "half_extents") else {
                    return Err(FromVarError::new(
                        "shape \"box\" needs \"half_extents\" [x, y, z]",
                    )
                    .at("half_extents"));
                };
                Shape::Box(
                    super::scene::SceneVec3::from_var(&half_extents)
                        .map_err(|e| e.at("half_extents"))?
                        .0,
                )
            }
            "sphere" => {
                let Some(radius) = super::scene::opt_field(v, "radius") else {
                    return Err(
                        FromVarError::new("shape \"sphere\" needs \"radius\"").at("radius")
                    );
                };
                Shape::Sphere(super::scene::scene_f32(&radius).map_err(|e| e.at("radius"))?)
            }
            other => {
                return Err(FromVarError::new(format!(
                    "expected \"box\" or \"sphere\", found {other:?}"
                ))
                .at("shape"))
            }
        };

        let mut body = PhysicsBody::new(entity_name, shape);
        if let Some(name) = super::scene::opt_string(v, "name")? {
            body.name = name;
        }
        if let Some(body_type) = super::scene::opt_field(v, "body") {
            body.body_type = BodyType::from_var(&body_type).map_err(|e| e.at("body"))?;
        }
        if let Some(scale) = super::scene::opt_f32(v, "gravity_scale")? {
            body.gravity_scale = scale;
        }
        if let Some(mass) = super::scene::opt_f32(v, "mass")? {
            body.mass = mass;
        }
        Ok(body)
    }
}

impl Component for PhysicsBody {
    fn priority(&self) -> Priority { Priority::Physics }

    fn as_any(&self) -> &dyn std::any::Any { self }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }

    fn scene_type(&self) -> Option<&'static str> { Some("physics_body") }

    /// The params, defaults omitted: `name` only when it differs from the
    /// entity's name (the factory's default), `body` only when static,
    /// `gravity_scale` only when not 1, `mass` only when set.
    fn scene_params(&self, ctx: &ComponentContext) -> Option<Var> {
        let obj = Object::new();
        match &self.shape {
            Shape::Box(half_extents) => {
                obj.set("shape", Var::from("box"));
                obj.set("half_extents", super::scene::vec3_to_var(*half_extents));
            }
            Shape::Sphere(radius) => {
                obj.set("shape", Var::from("sphere"));
                obj.set("radius", super::scene::f32_to_var(*radius));
            }
        }
        if self.body_type != BodyType::Dynamic {
            obj.set("body", Var::from(self.body_type.as_str()));
        }
        if self.gravity_scale != 1.0 {
            obj.set("gravity_scale", super::scene::f32_to_var(self.gravity_scale));
        }
        if self.mass != 0.0 {
            obj.set("mass", super::scene::f32_to_var(self.mass));
        }
        if self.name != ctx.entity_name {
            obj.set("name", Var::from(self.name.as_str()));
        }
        Some(Var::from(obj))
    }
}
