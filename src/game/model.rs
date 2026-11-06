//! Models — meshes with their textures, the shape glTF files load into
//! (gold's `meshRenderer` carried one mesh; a model is what a real asset
//! file expands to).
//!
//! glTF primitives are grouped by their `baseColorTexture`: everything
//! sharing a texture (or none) merges into one [`ModelPart`], so a model
//! is one draw per material — a single-material file is a single part.
//! Material base colors still bake into vertex colors (multiplied by the
//! texture at draw time, glTF's `factor × texture` convention); the
//! renderer's tint multiplies on top of everything.

// glTF-gated: textures resolve once per image in `from_gltf*` (used only there)
#[cfg(feature = "gltf")]
use std::collections::HashMap;
use std::sync::Arc;

use crate::game::mesh::Mesh;
use crate::game::texture::Texture;

/// One drawable piece of a [`Model`]: triangle soup plus the texture it
/// samples (`None` = untextured, vertex colors only).
#[derive(Debug, Clone)]
pub struct ModelPart {
    pub mesh: Arc<Mesh>,
    pub texture: Option<Arc<Texture>>,
}

/// A whole model: parts drawn together at one entity transform.
#[derive(Debug, Clone)]
pub struct Model {
    parts: Vec<ModelPart>,
}

impl Model {
    /// A one-part untextured model (the procedural-mesh path).
    pub fn single(mesh: Arc<Mesh>) -> Model {
        Model {
            parts: vec![ModelPart {
                mesh,
                texture: None,
            }],
        }
    }

    /// A one-part textured model (the sprite path).
    pub fn textured(mesh: Arc<Mesh>, texture: Arc<Texture>) -> Model {
        Model {
            parts: vec![ModelPart {
                mesh,
                texture: Some(texture),
            }],
        }
    }

    /// Build from parts (validated non-empty).
    ///
    /// # Errors
    /// No parts.
    pub fn from_parts(parts: Vec<ModelPart>) -> Result<Model, String> {
        if parts.is_empty() {
            return Err("model: no parts".into());
        }
        Ok(Model { parts })
    }

    pub fn parts(&self) -> &[ModelPart] {
        &self.parts
    }

    /// Parse a glTF document from memory — `.glb` binary, or JSON with
    /// data-URI / GLB-blob buffers. Textures must be embedded in the GLB
    /// blob (a bufferView image); data-URI and external image files
    /// resolve only through [`from_gltf_file`](Self::from_gltf_file)
    /// (the glTF crate's slice import doesn't decode URI images).
    ///
    /// Every node of the default scene (or the first scene) is walked with
    /// its transforms composed and baked into the vertices.
    ///
    /// # Errors
    /// The bytes don't parse, a referenced buffer/accessor/image is
    /// missing or malformed, or the file contains no meshes.
    #[cfg(feature = "gltf")]
    pub fn from_gltf_bytes(bytes: &[u8]) -> Result<Model, String> {
        let (doc, buffers, images) = gltf::import_slice(bytes).map_err(|e| format!("glTF: {e}"))?;
        bake_gltf(&doc, &buffers, &images)
    }

    /// Load a `.gltf`/`.glb` model from disk (external `.bin` and image
    /// files resolve relative to it).
    ///
    /// # Errors
    /// The file can't be read or doesn't parse, or it contains no meshes.
    #[cfg(feature = "gltf")]
    pub fn from_gltf_file(path: impl AsRef<std::path::Path>) -> Result<Model, String> {
        let (doc, buffers, images) = gltf::import(path).map_err(|e| format!("glTF: {e}"))?;
        bake_gltf(&doc, &buffers, &images)
    }
}

/// One texture-group's accumulator: everything sharing a base-color
/// texture merges into these vertices/indices.
#[cfg(feature = "gltf")]
struct Group {
    texture: Option<usize>, // glTF texture index
    vertices: Vec<crate::game::mesh::MeshVertex>,
    indices: Vec<u32>,
}

#[cfg(feature = "gltf")]
fn group_for(groups: &mut Vec<Group>, texture: Option<usize>) -> &mut Group {
    if let Some(at) = groups.iter().position(|g| g.texture == texture) {
        return &mut groups[at];
    }
    groups.push(Group {
        texture,
        vertices: Vec::new(),
        indices: Vec::new(),
    });
    groups.last_mut().expect("just pushed")
}

/// Flatten a glTF scene graph into texture-grouped parts.
#[cfg(feature = "gltf")]
fn bake_gltf(
    doc: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    images: &[gltf::image::Data],
) -> Result<Model, String> {
    let mut groups: Vec<Group> = Vec::new();

    for scene in doc.default_scene().into_iter().chain(doc.scenes()) {
        for node in scene.nodes() {
            append_node(node, Mat4::identity(), &mut groups, buffers);
        }
        if !groups.is_empty() && groups.iter().any(|g| !g.vertices.is_empty()) {
            break; // the default scene (or first with content) is enough
        }
    }
    if groups.iter().all(|g| g.vertices.is_empty()) {
        return Err("glTF: no meshes in the file".into());
    }

    // textures resolve once per image (several textures may share one)
    let mut resolved: HashMap<usize, Arc<Texture>> = HashMap::new();
    let mut parts = Vec::new();
    for group in groups {
        if group.vertices.is_empty() {
            continue;
        }
        let texture = match group.texture {
            None => None,
            Some(index) => {
                let texture = doc
                    .textures()
                    .nth(index)
                    .ok_or_else(|| format!("glTF: texture {index} out of range"))?;
                let image = texture.source().index();
                let data = images
                    .get(image)
                    .ok_or_else(|| format!("glTF: image {image} out of range"))?
                    .clone();
                let arc = match resolved.get(&image) {
                    Some(arc) => arc.clone(),
                    None => {
                        let arc = Arc::new(Texture::from_gltf_data(data)?);
                        resolved.insert(image, arc.clone());
                        arc
                    }
                };
                Some(arc)
            }
        };
        parts.push(ModelPart {
            mesh: Arc::new(Mesh::build(group.vertices, group.indices)),
            texture,
        });
    }
    Model::from_parts(parts)
}

/// Depth-first walk: compose node transforms down the tree, route each
/// mesh primitive into its texture group.
#[cfg(feature = "gltf")]
fn append_node(
    node: gltf::Node,
    parent: Mat4,
    groups: &mut Vec<Group>,
    buffers: &[gltf::buffer::Data],
) {
    let local = Mat4::from_gltf(node.transform().matrix());
    let world = parent.mul(&local);
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            let texture = primitive
                .material()
                .pbr_metallic_roughness()
                .base_color_texture()
                .map(|info| info.texture().index());
            let group = group_for(groups, texture);
            append_primitive(primitive, &world, group, buffers);
        }
    }
    for child in node.children() {
        append_node(child, world, groups, buffers);
    }
}

#[cfg(feature = "gltf")]
fn append_primitive(
    primitive: gltf::Primitive,
    world: &Mat4,
    group: &mut Group,
    buffers: &[gltf::buffer::Data],
) {
    let reader =
        primitive.reader(|buffer| buffers.get(buffer.index()).map(|data| data.0.as_slice()));
    let base = group.vertices.len() as u32;
    let color = primitive
        .material()
        .pbr_metallic_roughness()
        .base_color_factor();
    let needs_normals = reader.read_normals().is_none();

    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .map(|r| r.collect())
        .unwrap_or_default();
    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|r| r.collect())
        .unwrap_or_default();
    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|tc| tc.into_f32().collect())
        .unwrap_or_default();
    let primitive_indices: Vec<u32> = match reader.read_indices() {
        Some(idx) => idx.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };

    for (i, pos) in positions.iter().enumerate() {
        group.vertices.push(crate::game::mesh::MeshVertex {
            pos: world.transform_point(*pos),
            normal: normals
                .get(i)
                .map(|n| world.transform_dir(*n))
                .unwrap_or([0.0, 0.0, 0.0]),
            uv: uvs.get(i).copied().unwrap_or([0.0, 0.0]),
            color,
        });
    }
    group
        .indices
        .extend(primitive_indices.iter().map(|&i| base + i));

    // primitives without normals get flat per-triangle ones (baked after
    // the transform, from the transformed positions)
    if needs_normals {
        let start = group.indices.len() - primitive_indices.len();
        for tri in group.indices[start..].as_chunks::<3>().0 {
            let (a, b, c) = (
                group.vertices[tri[0] as usize].pos,
                group.vertices[tri[1] as usize].pos,
                group.vertices[tri[2] as usize].pos,
            );
            let e1 = crate::var::Vec3::new(b[0] - a[0], b[1] - a[1], b[2] - a[2]);
            let e2 = crate::var::Vec3::new(c[0] - a[0], c[1] - a[1], c[2] - a[2]);
            let n = e1.cross(e2);
            let len = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt();
            let normal = if len > 0.0 {
                [n.x / len, n.y / len, n.z / len]
            } else {
                [0.0, 0.0, 1.0]
            };
            for t in tri {
                group.vertices[*t as usize].normal = normal;
            }
        }
    }
}

/// Column-major 4×4 (m[col][row]), glTF's storage order.
#[cfg(feature = "gltf")]
#[derive(Clone, Copy)]
struct Mat4([[f32; 4]; 4]);

#[cfg(feature = "gltf")]
impl Mat4 {
    fn identity() -> Mat4 {
        Mat4([
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ])
    }

    fn from_gltf(m: [[f32; 4]; 4]) -> Mat4 {
        Mat4(m)
    }

    /// `self * rhs` (apply `rhs` first — child-local then parent).
    fn mul(&self, rhs: &Mat4) -> Mat4 {
        let mut out = [[0.0f32; 4]; 4];
        for (c, out_col) in out.iter_mut().enumerate() {
            for (r, cell) in out_col.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.0[k][r] * rhs.0[c][k]).sum();
            }
        }
        Mat4(out)
    }

    /// Transform a position (translation included).
    fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (r, out_r) in out.iter_mut().enumerate() {
            *out_r = self.0[0][r] * p[0] + self.0[1][r] * p[1] + self.0[2][r] * p[2] + self.0[3][r];
        }
        out
    }

    /// Transform a direction (no translation — normals).
    fn transform_dir(&self, v: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (r, out_r) in out.iter_mut().enumerate() {
            *out_r = self.0[0][r] * v[0] + self.0[1][r] * v[1] + self.0[2][r] * v[2];
        }
        let len = (out[0] * out[0] + out[1] * out[1] + out[2] * out[2]).sqrt();
        if len > 0.0 {
            [out[0] / len, out[1] / len, out[2] / len]
        } else {
            [0.0, 0.0, 1.0]
        }
    }
}
