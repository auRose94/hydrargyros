//! The camera component — an entity-attached [`Camera`] with a main
//! flag. The scene's MAIN camera decides the scene's dimensionality
//! (its projection: orthographic = 2D, perspective = 3D) and what play
//! mode renders through; the editor's viewport renders through its own
//! orbit camera instead (edit views are the editor's, never the
//! document's).
//!
//! v1 keeps the camera's position/target IN the component (the entity
//! transform is decorative for cameras); a transform-driven camera is
//! the natural follow-up once play mode owns entity transforms.

use super::camera::{Camera, Projection};
use super::component::{Component, Priority};
use super::scene::ComponentContext;
use crate::object::Object;
use crate::var::Var;
use crate::{FromVar, FromVarError};

#[derive(Debug, Clone)]
pub struct CameraComponent {
    pub camera: Camera,
    /// The scene's render camera (one per scene is the contract; the
    /// first main camera found wins).
    pub main: bool,
}

impl Default for CameraComponent {
    fn default() -> Self {
        CameraComponent {
            camera: Camera::default(),
            main: true,
        }
    }
}

impl CameraComponent {
    /// Build from a scene document's params (the `camera` factory):
    /// `projection` ("ortho" with `half_extents`, or "perspective" with
    /// `fov`/`near`/`far`/`aspect`), `position`, `target`, `main`.
    /// Strict, path-tracked reads.
    pub fn from_params(v: &Var) -> Result<CameraComponent, FromVarError> {
        let mut component = CameraComponent::default();
        if let Some(projection) = crate::game::scene::opt_field(v, "projection") {
            component.camera.projection = Projection::from_var(&projection)
                .map_err(|e| e.at("projection"))?;
        }
        let scene_f32 = |value: &Var| -> Result<f32, FromVarError> {
            match value {
                Var::I64(n) => Ok(*n as f32),
                Var::F64(n) => Ok(*n as f32),
                other => Err(FromVarError::new(format!(
                    "expected a number, found {}",
                    other.ty()
                ))),
            }
        };
        let vec3 = |key: &str| -> Result<Option<crate::var::Vec3>, FromVarError> {
            match crate::game::scene::opt_field(v, key) {
                None => Ok(None),
                Some(list) => {
                    let items = list
                        .as_list()
                        .ok_or_else(|| {
                            FromVarError::new(format!(
                                "expected [x, y, z], found {}",
                                list.ty()
                            ))
                        })
                        .and_then(|l| {
                            let items = l.iter();
                            if items.len() != 3 {
                                return Err(FromVarError::new(format!(
                                    "expected [x, y, z], got {} numbers",
                                    items.len()
                                )));
                            }
                            Ok([
                                scene_f32(&items[0])?,
                                scene_f32(&items[1])?,
                                scene_f32(&items[2])?,
                            ])
                        })?;
                    Ok(Some(crate::var::Vec3::new(items[0], items[1], items[2])))
                }
            }
        };
        if let Some(position) = vec3("position")? {
            component.camera.position = position;
        }
        if let Some(target) = vec3("target")? {
            component.camera.target = target;
        }
        if let Some(main) = crate::game::scene::opt_field(v, "main") {
            component.main = main.as_bool().unwrap_or(true);
        }
        Ok(component)
    }
}

impl Component for CameraComponent {
    fn priority(&self) -> Priority {
        Priority::Data
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn scene_type(&self) -> Option<&'static str> {
        Some("camera")
    }

    /// The params, defaults omitted: the projection tag with its
    /// numbers, position/target when non-default, `main` when set.
    fn scene_params(&self, _ctx: &ComponentContext) -> Option<Var> {
        let object = Object::new();
        match self.camera.projection {
            Projection::Orthographic { half_extents } => {
                object.set(
                    "projection",
                    Var::from(crate::o! {
                        "ortho": Var::from(crate::o! {
                            "half_extents": Var::from(crate::a![
                                Var::from(half_extents.x),
                                Var::from(half_extents.y),
                                Var::from(half_extents.z),
                            ]),
                        }),
                    }),
                );
            }
            Projection::Perspective {
                fov_y_degrees,
                near,
                far,
                aspect,
            } => {
                object.set(
                    "projection",
                    Var::from(crate::o! {
                        "perspective": Var::from(crate::o! {
                            "fov": Var::from(fov_y_degrees),
                            "near": Var::from(near),
                            "far": Var::from(far),
                            "aspect": Var::from(aspect),
                        }),
                    }),
                );
            }
        }
        let d = Camera::default();
        if self.camera.position != d.position {
            object.set(
                "position",
                Var::from(crate::a![
                    Var::from(self.camera.position.x),
                    Var::from(self.camera.position.y),
                    Var::from(self.camera.position.z),
                ]),
            );
        }
        if self.camera.target != d.target {
            object.set(
                "target",
                Var::from(crate::a![
                    Var::from(self.camera.target.x),
                    Var::from(self.camera.target.y),
                    Var::from(self.camera.target.z),
                ]),
            );
        }
        if self.main {
            object.set("main", Var::from(true));
        }
        Some(Var::from(object))
    }
}
