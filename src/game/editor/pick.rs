//! Viewport picking — the math that turns a click into an entity. Pure
//! data and camera math: no egui, no wgpu, no window, so it runs and
//! tests headless.
//!
//! Conventions this module pins (matching the engine's render path — the
//! native view's flip comment is the authority: **clip +y is the image's
//! TOP**):
//!
//! - [`project_point`]: world → pixel coordinates, y growing DOWN (egui's
//!   screen space), `None` behind the camera.
//! - [`pick_ray`]: a click's pixel → a world ray through the camera's
//!   near/far planes (the engine maps depth to z ∈ [0, 1]).
//! - [`pick_entity`]: the ray against each entity's pick proxy — the box
//!   of its shape-mode renderer, the model's real bounds for model-backed
//!   ones (a unit cube until an asset resolves; physics-only entities
//!   are not pickable in the viewport). Nearest wins. Proxies are boxes
//!   by design: one slab test, no per-shape math.

use crate::game::camera::Camera;
use crate::game::entity::Entity;
use crate::game::renderable::{MeshGeometry, MeshRenderer};
use crate::game::transform::Transform;
use crate::var::{Quat, Vec3};

/// A world-space ray: origin, unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

/// A viewport in pixels: the blit rect the scene renders into (the
/// letterboxed frame — picking answers in the SAME rect the pixels live
/// in, or clicks land one place and select another).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// Top-left corner in window pixels.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    #[must_use]
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Viewport {
        Viewport {
            x,
            y,
            width: width.max(1.0),
            height: height.max(1.0),
        }
    }

    /// `px` in window pixels → normalized inside the viewport ([0, 1]²,
    /// y down).
    #[must_use]
    fn normalize(&self, px: [f32; 2]) -> [f32; 2] {
        [
            (px[0] - self.x) / self.width,
            (px[1] - self.y) / self.height,
        ]
    }
}

/// A 4×4 matrix in ROW-major storage: `m[row]` is a clip row and
/// `clip[r] = dot(m[r], world)` — the convention every function in this
/// module consumes. The camera hands out column-major data (WGSL's
/// order — see [`crate::game::camera::Camera::view_projection`]), so
/// [`matrix_from_camera`] transposes once at the boundary and the rest
/// of the module is plain row-major math.
type Mat4 = [[f32; 4]; 4];

/// The camera's view·projection, transposed from WGSL's column-major
/// storage into this module's row-major [`Mat4`]. Skipping the transpose
/// silently applies the matrix's transpose — for an eye on the world z
/// axis the x/y rows coincide with their columns, so projected PIXELS
/// still match the render while every depth-dependent answer (ray
/// direction, near/far unprojection, the behind-camera w test) is bent
/// (caught 2026-11-19: the center-pixel pick ray pointed −z, AWAY from
/// the scene).
#[must_use]
fn matrix_from_camera(camera: &Camera) -> Mat4 {
    let m = camera.view_projection();
    let mut out = [[0.0f32; 4]; 4];
    for (r, out_row) in out.iter_mut().enumerate() {
        for (c, cell) in out_row.iter_mut().enumerate() {
            *cell = m[c][r];
        }
    }
    out
}

/// Matrix·vector (w = 1): clip[r] = dot(m[r], world).
#[must_use]
fn mul_point(m: &Mat4, p: Vec3) -> [f32; 4] {
    let v = [p.x, p.y, p.z, 1.0];
    let mut out = [0.0f32; 4];
    for row in 0..4 {
        out[row] = m[row][0] * v[0] + m[row][1] * v[1] + m[row][2] * v[2] + m[row][3] * v[3];
    }
    out
}

/// General 4×4 inverse (Gauss-Jordan with partial pivoting) — the pick
/// path unprojections need the inverse of whatever projection the camera
/// carries (ortho and perspective both). `None` when singular.
#[must_use]
fn mat4_inverse(m: &Mat4) -> Option<Mat4> {
    // augmented [M | I], row-reduced
    let mut a = *m;
    let mut inv = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for col in 0..4 {
        // pivot: the largest |value| in this column at or below the diagonal
        let (pivot_row, _) = (col..4)
            .map(|r| (r, a[r][col].abs()))
            .max_by(|x, y| x.1.total_cmp(&y.1))
            .unwrap_or((col, 0.0));
        if a[pivot_row][col].abs() < 1e-9 {
            return None;
        }
        a.swap(col, pivot_row);
        inv.swap(col, pivot_row);
        let pivot = a[col][col];
        for j in 0..4 {
            a[col][j] /= pivot;
            inv[col][j] /= pivot;
        }
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor == 0.0 {
                continue;
            }
            for j in 0..4 {
                a[row][j] -= factor * a[col][j];
                inv[row][j] -= factor * inv[col][j];
            }
        }
    }
    Some(inv)
}

/// World → viewport pixels (y down, egui's space). `None` behind the
/// camera (the clip w flips sign there — projecting would mirror).
#[must_use]
pub fn project_point(camera: &Camera, viewport: &Viewport, world: Vec3) -> Option<[f32; 2]> {
    let m = matrix_from_camera(camera);
    let clip = mul_point(&m, world);
    if clip[3] <= 0.0 {
        return None;
    }
    let ndc = [clip[0] / clip[3], clip[1] / clip[3]];
    // clip +y is the image's TOP (the native view's flip contract), so
    // pixel y grows downward from it
    Some([
        viewport.x + (ndc[0] + 1.0) * 0.5 * viewport.width,
        viewport.y + (1.0 - (ndc[1] + 1.0) * 0.5) * viewport.height,
    ])
}

/// A click's pixel → a world ray through the camera (near plane origin,
/// unit direction toward the far plane). The engine maps depth to
/// z ∈ [0, 1], so the ray spans the whole depth window.
#[must_use]
pub fn pick_ray(camera: &Camera, viewport: &Viewport, px: [f32; 2]) -> Option<Ray> {
    let m = matrix_from_camera(camera);
    let inv = mat4_inverse(&m)?;
    let [nx, ny] = viewport.normalize(px);
    let unproject = |z_ndc: f32| -> Vec3 {
        // pixel y down → clip y up; the engine maps depth to z ∈ [0, 1]
        let c = [nx * 2.0 - 1.0, (1.0 - ny) * 2.0 - 1.0, z_ndc, 1.0];
        // ROW-dot, same as mul_point: mul_point consumes the array
        // row-major (clip[r] = dot(vp[r], world)) and mat4_inverse
        // inverts THAT in place, so the unproject is dot(inv[r], clip)
        // — NOT inv[·][r], which applies the transpose and bends the
        // ray (caught 2026-11-19: the round trip missed (4,0,2) by
        // 2.29 world units with the transposed application)
        let mut out = [0.0f32; 4];
        for row in 0..4 {
            out[row] = inv[row][0] * c[0]
                + inv[row][1] * c[1]
                + inv[row][2] * c[2]
                + inv[row][3] * c[3];
        }
        Vec3::new(out[0] / out[3], out[1] / out[3], out[2] / out[3])
    };
    let near = unproject(0.0);
    let far = unproject(1.0);
    let dir = far - near;
    let len = dir.length();
    if len <= 0.0 {
        return None;
    }
    Some(Ray {
        origin: near,
        dir: dir.scaled(Vec3::splat(1.0 / len)),
    })
}

/// The entity's pick proxy: (center, half-extents) of a box in
/// entity-local space. Shape-mode renderers use their own `half_size`
/// (centered at the entity); model-backed ones use the model's real
/// bounds (`Model::bounds` — center may sit off the entity origin, so
/// the slab test shifts by it); an UNRESOLVED asset keeps a unit cube
/// until the load fills the model.
#[must_use]
fn pick_box(entity: &Entity) -> Option<(Vec3, Vec3)> {
    for component in entity.iter_components() {
        if let Some(renderer) = component
            .as_any()
            .downcast_ref::<MeshRenderer>()
        {
            return Some(match &renderer.geometry {
                MeshGeometry::Shape { half_size, .. } => (Vec3::splat(0.0), *half_size),
                MeshGeometry::Model { model } => model_pick_box(model),
                MeshGeometry::Asset { model, .. } => match model {
                    Some(model) => model_pick_box(model),
                    None => (Vec3::splat(0.0), Vec3::splat(0.5)),
                },
            });
        }
    }
    None
}

/// The bounds box for a loaded model; a degenerate (vertex-less) model
/// falls back to a unit cube so the entity stays pickable.
#[must_use]
fn model_pick_box(model: &crate::game::model::Model) -> (Vec3, Vec3) {
    let (center, half) = model.bounds();
    if half.x + half.y + half.z <= 0.0 {
        (Vec3::splat(0.0), Vec3::splat(0.5))
    } else {
        (center, half)
    }
}

/// Ray vs an axis-aligned box in LOCAL space (center `c`, half-extents
/// `h`): slab test. Returns the entry distance along the ray (> 0).
#[must_use]
fn ray_box(ray: &Ray, c: Vec3, h: Vec3) -> Option<f32> {
    let mut t_enter = 0.0f32;
    let mut t_exit = f32::INFINITY;
    for (o, d, half) in [
        (ray.origin.x - c.x, ray.dir.x, h.x),
        (ray.origin.y - c.y, ray.dir.y, h.y),
        (ray.origin.z - c.z, ray.dir.z, h.z),
    ] {
        if d.abs() < 1e-9 {
            if o.abs() > half {
                return None; // parallel and outside this slab
            }
            continue;
        }
        let mut t0 = (-half - o) / d;
        let mut t1 = (half - o) / d;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        t_enter = t_enter.max(t0);
        t_exit = t_exit.min(t1);
        if t_enter > t_exit {
            return None;
        }
    }
    (t_exit > 0.0).then_some(t_enter.max(0.0))
}

/// The ray vs one entity's proxy, in its LOCAL space (ray transformed by
/// the inverse world transform — translate, rotate⁻¹, scale⁻¹).
#[must_use]
fn ray_vs_entity(ray: &Ray, world: &Transform, entity: &Entity) -> Option<f32> {
    let (c, h) = pick_box(entity)?;
    let inv_rotation = world.rotation.compose(Quat::new(
        -world.rotation.x,
        -world.rotation.y,
        -world.rotation.z,
        world.rotation.w,
    ));
    let local_origin = inv_rotation.rotate(ray.origin - world.position);
    let local_origin = Vec3::new(
        local_origin.x / world.scale.x,
        local_origin.y / world.scale.y,
        local_origin.z / world.scale.z,
    );
    let local_dir = inv_rotation.rotate(ray.dir);
    let local_dir = Vec3::new(
        local_dir.x / world.scale.x,
        local_dir.y / world.scale.y,
        local_dir.z / world.scale.z,
    );
    // non-uniform scale changes the ray's length scale; renormalize so
    // the returned distance stays in world units along the ORIGINAL ray
    let len = local_dir.length();
    if len <= 0.0 {
        return None;
    }
    let local = Ray {
        origin: local_origin,
        dir: local_dir.scaled(Vec3::splat(1.0 / len)),
    };
    ray_box(&local, c, h).map(|t| t / len)
}

/// The nearest entity the ray hits, as its tree path — the viewport
/// click's pick. Walks the whole tree, composing world transforms
/// parent-first (the hierarchy's own rule). Disabled entities are not
/// pickable (they don't render).
#[must_use]
pub fn pick_entity(engine: &crate::game::Engine, ray: &Ray) -> Option<Vec<usize>> {
    let mut best: Option<(f32, Vec<usize>)> = None;
    fn walk(
        entity: &Entity,
        path: &mut Vec<usize>,
        parent_world: &Transform,
        ray: &Ray,
        best: &mut Option<(f32, Vec<usize>)>,
    ) {
        if !entity.enabled() {
            return;
        }
        let world = entity.transform().compose(parent_world);
        if let Some(distance) = ray_vs_entity(ray, &world, entity) {
            let better = match best {
                Some((best_distance, _)) => distance < *best_distance,
                None => true,
            };
            if better {
                *best = Some((distance, path.clone()));
            }
        }
        for (i, child) in entity.children().iter().enumerate() {
            path.push(i);
            walk(child, path, &world, ray, best);
            path.pop();
        }
    }
    for (i, root) in engine.entities().iter().enumerate() {
        let mut path = vec![i];
        let identity = Transform::default();
        walk(root, &mut path, &identity, ray, &mut best);
    }
    best.map(|(_, path)| path)
}

/// The whole click: ray through `px`, nearest entity, its path. The
/// shell's viewport calls this on a plain click.
#[must_use]
pub fn click_pick(
    engine: &crate::game::Engine,
    camera: &Camera,
    viewport: &Viewport,
    px: [f32; 2],
) -> Option<Vec<usize>> {
    let ray = pick_ray(camera, viewport, px)?;
    pick_entity(engine, &ray)
}
