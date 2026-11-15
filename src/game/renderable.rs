//! Renderables — components the
//! engine's draw pass turns into geometry. A `MeshRenderer` draws either a
//! solid shape (rect or circle, the quad path) or a model (mesh parts with
//! optional textures) at its entity's world transform; the camera maps it
//! to the screen.

use super::component::{Component, Priority};
use super::model::Model;
use super::render::{Geometry, Quad};
use super::scene::ComponentContext;
use super::texture::Texture;
use super::transform::Transform;
use crate::object::Object;
use crate::var::{Var, Vec3};
use crate::{FromVar, FromVarError};

/// Something an entity owns that contributes geometry to the drawn scene.
/// The engine queries components for this trait during the draw pass.
pub trait Renderable {
    /// The geometry for this component, at the owning entity's world
    /// transform.
    fn geometry(&self, world: &Transform) -> Geometry;
}

/// What silhouette a shape-mode [`MeshRenderer`] draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeshShape {
    Rect,
    Circle,
}

impl MeshShape {
    /// The document string (`"rect"` / `"circle"`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            MeshShape::Rect => "rect",
            MeshShape::Circle => "circle",
        }
    }
}

impl FromVar for MeshShape {
    fn from_var(v: &Var) -> Result<Self, FromVarError> {
        match v.as_str() {
            Some("rect") => Ok(MeshShape::Rect),
            Some("circle") => Ok(MeshShape::Circle),
            _ => Err(FromVarError::new(format!(
                "expected \"rect\" or \"circle\", found {}",
                v.ty()
            ))),
        }
    }
}

/// What a [`MeshRenderer`] draws: a solid 2D shape (rendered in list
/// order, no depth) or a model — mesh parts with optional textures,
/// depth-tested, shaded, transformed by the full entity transform
/// (position, rotation and scale).
#[derive(Clone)]
pub enum MeshGeometry {
    Shape { shape: MeshShape, half_size: Vec3 },
    Model { model: std::sync::Arc<Model> },
}

impl std::fmt::Debug for MeshGeometry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshGeometry::Shape { shape, half_size } => f
                .debug_struct("Shape")
                .field("shape", shape)
                .field("half_size", half_size)
                .finish(),
            MeshGeometry::Model { model } => f
                .debug_struct("Model")
                .field("parts", &model.parts().len())
                .field(
                    "triangles",
                    &model
                        .parts()
                        .iter()
                        .map(|p| p.mesh.triangle_count())
                        .sum::<usize>(),
                )
                .finish(),
        }
    }
}

/// A solid shape or model at the entity's transform
/// (v0: lights are a fixed key light). Draw tier — rendered
/// after all updates. The entity's world position is the shape's center;
/// `half_size` is in world units. A model is scaled/rotated by the
/// entity's transform, and `color` tints it (multiplied over the parts'
/// baked vertex colors and sampled textures; tint white to keep them).
#[derive(Debug, Clone)]
pub struct MeshRenderer {
    pub geometry: MeshGeometry,
    pub color: [f32; 4],
}

impl MeshRenderer {
    pub fn new(geometry: MeshGeometry, color: [f32; 4]) -> Self {
        MeshRenderer { geometry, color }
    }

    /// A circular mesh.
    pub fn circle(radius: f32, color: [f32; 4]) -> Self {
        MeshRenderer::new(
            MeshGeometry::Shape {
                shape: MeshShape::Circle,
                half_size: Vec3::new(radius, radius, radius),
            },
            color,
        )
    }

    /// A rectangular mesh.
    pub fn rect(half_size: Vec3, color: [f32; 4]) -> Self {
        MeshRenderer::new(
            MeshGeometry::Shape {
                shape: MeshShape::Rect,
                half_size,
            },
            color,
        )
    }

    /// An untextured model (shared — pass the same `Arc` around;
    /// procedural builders make one).
    pub fn mesh(mesh: std::sync::Arc<super::mesh::Mesh>, color: [f32; 4]) -> Self {
        MeshRenderer::model(std::sync::Arc::new(Model::single(mesh)), color)
    }

    /// A textured model: one mesh, one texture — a sprite when the mesh is
    /// [`Mesh::quad`](super::mesh::Mesh::quad). The texture multiplies the
    /// vertex colors and the tint.
    pub fn textured(
        mesh: std::sync::Arc<super::mesh::Mesh>,
        texture: std::sync::Arc<Texture>,
        color: [f32; 4],
    ) -> Self {
        MeshRenderer::model(std::sync::Arc::new(Model::textured(mesh, texture)), color)
    }

    /// A whole model (what the glTF loader produces — one draw per
    /// texture-grouped part).
    pub fn model(model: std::sync::Arc<Model>, color: [f32; 4]) -> Self {
        MeshRenderer::new(MeshGeometry::Model { model }, color)
    }

    /// Build from a scene document's params object (the component
    /// registry's `mesh_renderer` factory): `shape` (`"rect"` or
    /// `"circle"`), optional `half_size` `[x, y, z]` (default unit) and
    /// `color` `[r, g, b, a]` (default white). Model-backed renderers are
    /// materialized by the asset pipeline, not scene params.
    ///
    /// # Errors
    /// A `FromVarError` at the offending field.
    pub fn from_params(v: &Var) -> Result<MeshRenderer, FromVarError> {
        let Some(shape_var) = super::scene::opt_field(v, "shape") else {
            return Err(
                FromVarError::new("\"shape\" is required — \"rect\" or \"circle\"").at("shape")
            );
        };
        let shape = MeshShape::from_var(&shape_var).map_err(|e| e.at("shape"))?;
        let mut half_size = Vec3::splat(1.0);
        if let Some(size) = super::scene::opt_field(v, "half_size") {
            half_size =
                super::scene::SceneVec3::from_var(&size).map_err(|e| e.at("half_size"))?.0;
        }
        let mut color = [1.0, 1.0, 1.0, 1.0];
        if let Some(tint) = super::scene::opt_field(v, "color") {
            color = super::scene::SceneVec4::from_var(&tint).map_err(|e| e.at("color"))?.0;
        }
        Ok(MeshRenderer::new(
            MeshGeometry::Shape { shape, half_size },
            color,
        ))
    }
}

impl Component for MeshRenderer {
    fn priority(&self) -> Priority {
        Priority::Draw
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn as_renderable(&self) -> Option<&dyn Renderable> {
        Some(self)
    }

    fn scene_type(&self) -> Option<&'static str> {
        Some("mesh_renderer")
    }

    /// The params for shape-mode renderers, defaults omitted. Model-backed
    /// renderers are runtime-only — the geometry is compiled (no
    /// provenance to re-load it from) until the asset pipeline gives
    /// models paths.
    fn scene_params(&self, _ctx: &ComponentContext) -> Option<Var> {
        let MeshGeometry::Shape { shape, half_size } = &self.geometry else {
            return None;
        };
        let obj = Object::new();
        obj.set("shape", Var::from(shape.as_str()));
        if *half_size != Vec3::splat(1.0) {
            obj.set("half_size", super::scene::vec3_to_var(*half_size));
        }
        if self.color != [1.0, 1.0, 1.0, 1.0] {
            obj.set("color", super::scene::f32s_to_var(&self.color));
        }
        Some(Var::from(obj))
    }
}

impl Renderable for MeshRenderer {
    fn geometry(&self, world: &Transform) -> Geometry {
        match &self.geometry {
            MeshGeometry::Shape { shape, half_size } => Geometry::Quad(Quad {
                pos: [world.position.x, world.position.y],
                size: [half_size.x, half_size.y],
                color: self.color,
                shape: match shape {
                    MeshShape::Rect => super::render::QuadShape::Rect,
                    MeshShape::Circle => super::render::QuadShape::Circle,
                },
            }),
            MeshGeometry::Model { model } => Geometry::Model {
                model: model.clone(),
                color: self.color,
            },
        }
    }
}
