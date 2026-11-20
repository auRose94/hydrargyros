//! Mesh import (the `mesh-import` feature): real geometry from the
//! mesh formats the glTF path doesn't cover — FBX, OBJ, STL, PLY, VOX,
//! 3DS — through **polysmelt** (pure Rust, MIT-0). This is the
//! migration rung's geometry half: an imported Unity/Unreal project
//! names its meshes, and asset-backed renderers resolve them through
//! the same [`Model`] registry the glTF path feeds. `.gltf`/`.glb`
//! keep the proven glTF-crate path (`MeshRenderer::resolve`
//! dispatches by extension).
//!
//! # The conversion
//!
//! polysmelt reads a file into a glTF-shaped document, and
//! [`Document::flatten`](polysmelt::document::Document::flatten)
//! poses the scene graph into flat triangle lists grouped by
//! material — one prim per material. Each prim becomes a hyg
//! [`ModelPart`]:
//!
//! - **Vertices** carry the prim's own vertex colors where present;
//!   where it has none, the material's base color fills them (linear
//!   RGBA throughout — the glTF path's convention, so lighting sees
//!   one color space).
//! - **Normals** come from the file; a faceted source (STL carries
//!   none) gets `smooth_normals` computed over the triangles.
//! - **Textures** decode through the `image` feature (a material's
//!   base-color image rides in as PNG/JPEG bytes, deduplicated per
//!   index like the glTF path). Without `image`, textured materials
//!   still import — the texture is skipped, the base color carries.
//! - **UVs** pass through when the file carries them (glTF's
//!   top-left convention, the renderer's sampling rule).
//!
//! Deliberately dropped, stated: metallicity/roughness/emissive
//! scalars and double-sided flags have no hyg material model yet.
//!
//! # Verified against real projects, limits stated
//!
//! Static FBX imports live (a 23-part, 52,742-triangle custom drone
//! from a real UE 5.4 project, ~100 ms). Three boundaries met in the
//! wild, each honest: a 293 MB wall FBX refuses on polysmelt's
//! hostile-file triangle cap (`MAX_TRIANGLES` — the message says so);
//! **skinned meshes flatten to nothing yet** (a Mixamo walk reports
//! "no meshes" — the skin path needs hyg's animation rung, which
//! doesn't exist yet, before it has a consumer); and polysmelt
//! normalizes what it reads (MTL sRGB → linear color, OBJ UVs →
//! top-left, Z-up STL → Y-up) — the tests pin those conventions.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use super::mesh::{Mesh, MeshVertex};
use super::model::{Model, ModelPart};
use super::texture::Texture;

/// Load the mesh file at `path` into a hyg model. `ext` names the
/// format (lowercase, no dot — one of polysmelt's `EXTENSIONS` this
/// build enables: stl, ply, vox, obj, fbx, 3ds).
///
/// # Errors
/// The file doesn't parse, it contains no meshes, or a texture fails
/// to decode (with the `image` feature).
pub fn load_model(path: &Path, ext: &str) -> Result<Model, String> {
    let folder = polysmelt::assets::Folder::new(path.parent().unwrap_or_else(|| Path::new(".")));
    let doc = polysmelt::formats::load(path, ext, &folder, polysmelt::document::Keep::default())
        .map_err(|e| format!("mesh import ({ext}): {e}"))?;
    let mesh = doc
        .flatten(polysmelt::document::Pose::Rest)
        .map_err(|e| format!("mesh import ({ext}): flatten: {e}"))?
        .ok_or_else(|| format!("mesh import ({ext}): the file contains no meshes"))?;
    model_from_polysmelt(&mesh)
}

/// Convert polysmelt's material-grouped triangle lists into hyg parts.
fn model_from_polysmelt(mesh: &polysmelt::mesh::Mesh) -> Result<Model, String> {
    // textures resolve once per image (several materials may share one)
    let mut resolved: HashMap<usize, Arc<Texture>> = HashMap::new();
    let mut parts = Vec::new();

    for prim in &mesh.prims {
        if prim.indices.is_empty() {
            continue;
        }
        let normals = if prim.normals.len() == prim.positions.len() {
            prim.normals.clone()
        } else {
            polysmelt::mesh::smooth_normals(&prim.positions, &prim.indices)
        };
        let material = mesh.materials.get(prim.material);
        let base_color = material.map(|m| m.color).unwrap_or(polysmelt::mesh::GRAY);
        let has_colors = prim.colors.len() == prim.positions.len();
        let has_uvs = prim.uvs.len() == prim.positions.len();

        let vertices: Vec<MeshVertex> = prim
            .positions
            .iter()
            .enumerate()
            .map(|(i, &pos)| MeshVertex {
                pos,
                normal: normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                uv: if has_uvs { prim.uvs[i] } else { [0.0; 2] },
                color: if has_colors {
                    prim.colors[i]
                } else {
                    base_color
                },
            })
            .collect();

        // the material's base-color texture, decoded once per image
        let texture = match material.and_then(|m| m.color_image) {
            None => None,
            Some(image_index) => {
                let image = mesh.images.get(image_index).ok_or_else(|| {
                    format!(
                        "mesh import: material {} references image {image_index}, which is out of range",
                        material.map(|m| m.name.as_str()).unwrap_or("?")
                    )
                })?;
                let arc = match resolved.get(&image_index) {
                    Some(arc) => arc.clone(),
                    None => {
                        // the `image` feature decodes PNG/JPEG; without
                        // it a textured material keeps its base color
                        #[cfg(feature = "image")]
                        let arc = {
                            let decoded = Texture::from_bytes(&image.bytes)
                                .map_err(|e| format!("mesh import: texture {image_index}: {e}"))?;
                            let arc = Arc::new(decoded);
                            resolved.insert(image_index, arc.clone());
                            arc
                        };
                        #[cfg(not(feature = "image"))]
                        let arc: Arc<Texture> = {
                            let _ = image;
                            resolved.entry(image_index).or_insert_with(|| {
                                Arc::new(Texture::white())
                            }).clone()
                        };
                        arc
                    }
                };
                Some(arc)
            }
        };

        parts.push(ModelPart {
            mesh: Arc::new(Mesh::new(vertices, prim.indices.clone())?),
            texture,
        });
    }

    Model::from_parts(parts)
}
