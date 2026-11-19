//! Renderables — components the
//! engine's draw pass turns into geometry. A `MeshRenderer` draws either a
//! solid shape (rect or circle, the quad path), a model (mesh parts with
//! optional textures), or an ASSET reference (a model file path the load
//! resolves) at its entity's world transform; the camera maps it to the
//! screen.

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
/// order, no depth), a model — mesh parts with optional textures,
/// depth-tested, shaded, transformed by the full entity transform
/// (position, rotation and scale) — or an ASSET reference: a file path
/// the scene stores and the load materializes (the `gltf` feature).
#[derive(Clone)]
pub enum MeshGeometry {
    Shape { shape: MeshShape, half_size: Vec3 },
    Model { model: std::sync::Arc<Model> },
    /// An asset file reference (`.glb`/`.gltf`), stored AS the path so
    /// scenes round-trip feature-free: the path is data, the model is
    /// compiled geometry that can't serialize. [`MeshRenderer::resolve`]
    /// materializes it against the scene's directory; while unresolved
    /// nothing draws (a `Geometry::None` frame contribution, not a
    /// placeholder — the pick path still hits a small proxy box).
    Asset {
        path: String,
        model: Option<std::sync::Arc<Model>>,
    },
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
            MeshGeometry::Asset { path, model } => f
                .debug_struct("Asset")
                .field("path", path)
                .field("loaded", &model.is_some())
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

    /// An asset-backed renderer: the scene stores `path` (relative to the
    /// scene file); [`resolve`](Self::resolve) loads the model. The
    /// constructor stays feature-free — a path is data, loading is a
    /// feature.
    #[must_use]
    pub fn asset(path: impl Into<String>, color: [f32; 4]) -> Self {
        MeshRenderer::new(
            MeshGeometry::Asset {
                path: path.into(),
                model: None,
            },
            color,
        )
    }

    /// The asset path when this renderer is asset-backed (the inspector's
    /// model row and the scene save both read it).
    #[must_use]
    pub fn asset_path(&self) -> Option<&str> {
        match &self.geometry {
            MeshGeometry::Asset { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Materialize an asset-backed renderer: load the model at
    /// `<base>/<path>`. The scene loader calls this once per component at
    /// creation time (gltf-gated there); resolving again re-loads (an
    /// edited path picks up the new file). Unresolved asset renderers
    /// draw nothing but keep their path — the scene round-trips.
    ///
    /// # Errors
    /// The file can't be read or doesn't parse (the message names the
    /// path).
    #[cfg(feature = "gltf")]
    pub fn resolve(&mut self, base: &std::path::Path) -> Result<(), String> {
        let MeshGeometry::Asset { path, model } = &mut self.geometry else {
            return Ok(());
        };
        let full = base.join(&*path);
        let loaded = Model::from_gltf_file(&full).map_err(|e| format!("{}: {e}", full.display()))?;
        *model = Some(std::sync::Arc::new(loaded));
        Ok(())
    }

    /// Build from a scene document's params object (the component
    /// registry's `mesh_renderer` factory): either `model` (an asset
    /// path, resolved by the load against the scene directory),
    /// `runtime_model` (a session registry id — the in-memory play
    /// snapshot's shape), or `shape` (`"rect"` / `"circle"`) with
    /// optional `half_size` `[x, y, z]` (default unit); optional `color`
    /// `[r, g, b, a]` (default white).
    ///
    /// # Errors
    /// A `FromVarError` at the offending field (or no geometry key
    /// present, or a `runtime_model` id the session's registry doesn't
    /// hold).
    pub fn from_params(v: &Var) -> Result<MeshRenderer, FromVarError> {
        // the snapshot shape first: a registry id resolves to the
        // session's compiled model
        if let Some(id_var) = super::scene::opt_field(v, "runtime_model") {
            let id = id_var
                .as_i64()
                .ok_or_else(|| FromVarError::new("expected a model id (int)").at("runtime_model"))?
                as u64;
            let model = crate::game::model::runtime_model(id).ok_or_else(|| {
                FromVarError::new(format!("runtime model #{id} is not in this session's registry"))
                    .at("runtime_model")
            })?;
            let mut color = [1.0, 1.0, 1.0, 1.0];
            if let Some(tint) = super::scene::opt_field(v, "color") {
                color = super::scene::SceneVec4::from_var(&tint).map_err(|e| e.at("color"))?.0;
            }
            return Ok(MeshRenderer::new(MeshGeometry::Model { model }, color));
        }
        let model = super::scene::opt_field(v, "model");
        let shape_var = super::scene::opt_field(v, "shape");
        let geometry = match (model, shape_var) {
            (Some(m), None) => {
                let path = m
                    .as_str()
                    .ok_or_else(|| FromVarError::new("expected a path string").at("model"))?
                    .to_string();
                MeshGeometry::Asset { path, model: None }
            }
            (None, Some(shape_var)) => {
                let shape = MeshShape::from_var(&shape_var).map_err(|e| e.at("shape"))?;
                let mut half_size = Vec3::splat(1.0);
                if let Some(size) = super::scene::opt_field(v, "half_size") {
                    half_size =
                        super::scene::SceneVec3::from_var(&size).map_err(|e| e.at("half_size"))?.0;
                }
                MeshGeometry::Shape { shape, half_size }
            }
            (Some(_), Some(_)) => {
                return Err(FromVarError::new(
                    "one geometry per renderer — both \"model\" and \"shape\" present",
                ));
            }
            (None, None) => {
                return Err(FromVarError::new(
                    "\"model\" (an asset path) or \"shape\" (\"rect\"/\"circle\") is required",
                ));
            }
        };
        let mut color = [1.0, 1.0, 1.0, 1.0];
        if let Some(tint) = super::scene::opt_field(v, "color") {
            color = super::scene::SceneVec4::from_var(&tint).map_err(|e| e.at("color"))?.0;
        }
        Ok(MeshRenderer::new(geometry, color))
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

    /// The params for shape-mode renderers (`shape`/`half_size`/`color`,
    /// defaults omitted) and asset-backed renderers (`model` path +
    /// `color` — the path round-trips feature-free; the load re-resolves
    /// it). Compiled-model renderers stay runtime-only: procedural
    /// geometry has no file to point back at.
    fn scene_params(&self, _ctx: &ComponentContext) -> Option<Var> {
        let obj = Object::new();
        match &self.geometry {
            MeshGeometry::Shape { shape, half_size } => {
                obj.set("shape", Var::from(shape.as_str()));
                if *half_size != Vec3::splat(1.0) {
                    obj.set("half_size", super::scene::vec3_to_var(*half_size));
                }
            }
            MeshGeometry::Asset { path, .. } => {
                obj.set("model", Var::from(path.as_str()));
            }
            // a compiled model has no provenance — skip the component on
            // save rather than write params that can't rebuild it
            MeshGeometry::Model { .. } => return None,
        }
        if self.color != [1.0, 1.0, 1.0, 1.0] {
            obj.set("color", super::scene::f32s_to_var(&self.color));
        }
        Some(Var::from(obj))
    }

    /// The LOSSLESS snapshot params: compiled models reference the
    /// session's registry (`{"runtime_model": id}`) — the play
    /// snapshot's in-memory round trip is exact where the file save
    /// must skip.
    fn runtime_params(&self, ctx: &super::scene::ComponentContext) -> Option<Var> {
        match &self.geometry {
            MeshGeometry::Model { model } => {
                let obj = Object::new();
                obj.set(
                    "runtime_model",
                    Var::from(model.id() as i64),
                );
                if self.color != [1.0, 1.0, 1.0, 1.0] {
                    obj.set("color", super::scene::f32s_to_var(&self.color));
                }
                Some(Var::from(obj))
            }
            _ => self.scene_params(ctx),
        }
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
            // unresolved asset: nothing yet (the load fills the model);
            // resolved: exactly the Model path
            MeshGeometry::Asset { model, .. } => match model {
                None => Geometry::None,
                Some(model) => Geometry::Model {
                    model: model.clone(),
                    color: self.color,
                },
            },
        }
    }
}
