//! Meshes — real triangle geometry, the rung past the quad renderer.
//!
//! A [`Mesh`] is indexed vertex data shared through an `Arc`: the CPU owns
//! the bytes once, every renderer draws from them. Procedural builders
//! ([`Mesh::cube`], [`Mesh::sphere`], [`Mesh::quad`]) cover tests, demos
//! and sprites with no asset files; glTF files load through
//! [`Model`](crate::game::model::Model) (behind the `gltf` feature), which
//! splits them into mesh + texture parts.

use std::sync::atomic::{AtomicU64, Ordering};

/// One mesh vertex: position, normal, texture coordinates, and a per-vertex
/// color (48 bytes). glTF material base colors bake in here; procedural
/// meshes are white so the renderer's tint *is* the color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeshVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

impl MeshVertex {
    /// A vertex at `pos` with everything else neutral: +z normal, no UV,
    /// white color.
    pub fn new(pos: [f32; 3]) -> Self {
        MeshVertex {
            pos,
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
            color: [1.0; 4],
        }
    }
}

static MESH_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Indexed triangle mesh: `indices` are triples into `vertices`, wound
/// counter-clockwise when viewed from outside (glTF's convention). Cheap to
/// clone the handle — share it as an `Arc<Mesh>` between components.
#[derive(Debug, Clone)]
pub struct Mesh {
    id: u64,
    vertices: Vec<MeshVertex>,
    indices: Vec<u32>,
}

impl Mesh {
    /// A validated mesh from raw data.
    ///
    /// # Errors
    /// No vertices, the index list isn't a whole number of triangles, or an
    /// index points past the vertex list.
    pub fn new(vertices: Vec<MeshVertex>, indices: Vec<u32>) -> Result<Mesh, String> {
        if vertices.is_empty() {
            return Err("mesh: no vertices".into());
        }
        if !indices.len().is_multiple_of(3) {
            return Err(format!(
                "mesh: {} indices is not whole triangles",
                indices.len()
            ));
        }
        if let Some(&bad) = indices.iter().find(|&&i| i as usize >= vertices.len()) {
            return Err(format!(
                "mesh: index {bad} out of range ({} vertices)",
                vertices.len()
            ));
        }
        Ok(Mesh {
            id: MESH_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            vertices,
            indices,
        })
    }

    /// Build without re-validating — the constructors in this module (and
    /// the glTF loader) produce valid data by construction.
    pub(crate) fn build(vertices: Vec<MeshVertex>, indices: Vec<u32>) -> Mesh {
        Mesh {
            id: MESH_ID_COUNTER.fetch_add(1, Ordering::Relaxed),
            vertices,
            indices,
        }
    }

    /// The stable id (GPU backends cache buffer uploads against it).
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn vertices(&self) -> &[MeshVertex] {
        &self.vertices
    }

    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// An axis-aligned box: per-face normals (flat shading), centered at
    /// the origin, `half_extents` per axis, white vertex colors.
    pub fn cube(half_extents: crate::var::Vec3) -> Mesh {
        // per face: outward normal n, tangents u and v with u×v = n —
        // corners then wind CCW seen from outside
        const FACES: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            ([1.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]), // +x
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]), // −x
            ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]), // +y
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]), // −y
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),  // +z
            ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), // −z
        ];
        let he = [half_extents.x, half_extents.y, half_extents.z];
        let mut vertices = Vec::with_capacity(24);
        let mut indices = Vec::with_capacity(36);
        for &(n, u, v) in &FACES {
            let face_center = [n[0] * he[0], n[1] * he[1], n[2] * he[2]];
            let us = [u[0] * he[0], u[1] * he[1], u[2] * he[2]];
            let vs = [v[0] * he[0], v[1] * he[1], v[2] * he[2]];
            let base = vertices.len() as u32;
            let corners = [
                sub(sub(face_center, us), vs),
                add(sub(face_center, vs), us),
                add(add(face_center, us), vs),
                add(sub(face_center, us), vs),
            ];
            for (corner, uv) in corners
                .iter()
                .zip([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]])
            {
                vertices.push(MeshVertex {
                    pos: *corner,
                    normal: n,
                    uv,
                    color: [1.0; 4],
                });
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        Mesh::build(vertices, indices)
    }

    /// A UV sphere: smooth normals (they equal the normalized position),
    /// centered at the origin. `segments` slices around the equator,
    /// `rings` bands pole to pole (clamped to at least 3 and 2).
    pub fn sphere(radius: f32, segments: u32, rings: u32) -> Mesh {
        let (segments, rings) = (segments.max(3), rings.max(2));
        let mut vertices = Vec::with_capacity(((rings + 1) * (segments + 1)) as usize);
        for ring in 0..=rings {
            let theta = std::f32::consts::PI * ring as f32 / rings as f32;
            let (st, ct) = theta.sin_cos();
            for seg in 0..=segments {
                let phi = 2.0 * std::f32::consts::PI * seg as f32 / segments as f32;
                let (sp, cp) = phi.sin_cos();
                let pos = [radius * st * cp, radius * ct, radius * st * sp];
                let len = (pos[0] * pos[0] + pos[1] * pos[1] + pos[2] * pos[2]).sqrt();
                let normal = [pos[0] / len, pos[1] / len, pos[2] / len];
                vertices.push(MeshVertex {
                    pos,
                    normal,
                    uv: [seg as f32 / segments as f32, ring as f32 / rings as f32],
                    color: [1.0; 4],
                });
            }
        }
        let row = segments + 1;
        let mut indices = Vec::with_capacity((rings * segments * 6) as usize);
        for ring in 0..rings {
            for seg in 0..segments {
                let a = ring * row + seg; // this band, this slice
                let b = (ring + 1) * row + seg; // band below, this slice
                let c = b + 1; // band below, next slice
                let d = a + 1; // this band, next slice
                indices.extend([a, c, b, a, d, c]);
            }
        }
        Mesh::build(vertices, indices)
    }

    /// A flat rectangle in the XY plane facing +z — the sprite base: pair
    /// with a texture via `MeshRenderer::textured` and the entity
    /// transform places it. UVs map the image upright on screen (+y up):
    /// the image's top-left (glTF's uv (0, 0)) lands on the quad's
    /// top-left corner.
    pub fn quad(half_size: crate::var::Vec3) -> Mesh {
        let he = [half_size.x, half_size.y, half_size.z];
        let n = [0.0f32, 0.0, 1.0];
        // corners BL, BR, TR, TL (CCW from +z); uv v flips so the image
        // stands upright on the +y-up screen
        let corners = [
            [-he[0], -he[1], 0.0],
            [he[0], -he[1], 0.0],
            [he[0], he[1], 0.0],
            [-he[0], he[1], 0.0],
        ];
        let uvs = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
        let vertices: Vec<MeshVertex> = corners
            .iter()
            .zip(uvs)
            .map(|(pos, uv)| MeshVertex {
                pos: *pos,
                normal: n,
                uv,
                color: [1.0; 4],
            })
            .collect();
        Mesh::build(vertices, vec![0, 1, 2, 0, 2, 3])
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
