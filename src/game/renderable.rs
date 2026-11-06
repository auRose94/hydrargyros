//! Renderables — gold's `renderable`/`meshRenderer`: components the
//! engine's draw pass turns into geometry. A `MeshRenderer` draws either a
//! solid shape (rect or circle, the quad path) or a model (mesh parts with
//! optional textures) at its entity's world transform; the camera maps it
//! to the screen.

use super::component::{Component, Priority};
use super::model::Model;
use super::render::{Geometry, Quad};
use super::texture::Texture;
use super::transform::Transform;
use crate::var::Vec3;

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

/// A solid shape or model at the entity's transform (gold's
/// `meshRenderer`; v0: lights are a fixed key light). Draw tier — rendered
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
