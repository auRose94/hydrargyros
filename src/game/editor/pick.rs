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
//!   of its shape-mode renderer (or a unit cube for model-backed ones;
//!   physics-only entities are not pickable in the viewport). Nearest
//!   wins. Proxies are boxes by design: one slab test, no per-shape math.

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

/// A 4×4 matrix in ROW-major math convention (`m[row][col]`), the shape
/// the inverse and multipliers below work in. The camera's
/// [`view_projection`](crate::game::Camera::view_projection) stores
/// columns (`m[col][row]`) — [`matrix_from_camera`] transposes on the
/// way in.
type Mat4 = [[f32; 4]; 4];

/// The camera's view·projection as a row-major math matrix.
#[must_use]
fn matrix_from_camera(camera: &Camera) -> Mat4 {
    let vp = camera.view_projection();
    let mut m = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            m[row][col] = vp[col][row];
        }
    }
    m
}

/// Matrix·vector (w = 1).
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
        // row·(nx, ny, z_ndc, 1), then the perspective divide
        let c = [
            nx * 2.0 - 1.0,
            (1.0 - ny) * 2.0 - 1.0, // pixel y down → clip y up
            z_ndc,
            1.0,
        ];
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

/// The entity's pick proxy: half-extents of a box in entity-local space.
/// Shape-mode renderers use their own `half_size`; model-backed ones (and
/// physics bodies without a renderer) use a unit cube — a coarse but
/// honest proxy until the asset pipeline knows real bounds.
#[must_use]
fn pick_half_size(entity: &Entity) -> Option<Vec3> {
    for component in entity.iter_components() {
        if let Some(renderer) = component
            .as_any()
            .downcast_ref::<MeshRenderer>()
        {
            if let MeshGeometry::Shape { half_size, .. } = &renderer.geometry {
                return Some(*half_size);
            }
            return Some(Vec3::splat(0.5)); // model proxy: a unit cube
        }
    }
    None
}

/// Ray vs an axis-aligned box in LOCAL space (center 0, half-extents
/// `h`): slab test. Returns the entry distance along the ray (> 0).
#[must_use]
fn ray_box(ray: &Ray, h: Vec3) -> Option<f32> {
    let mut t_enter = 0.0f32;
    let mut t_exit = f32::INFINITY;
    for (o, d, half) in [
        (ray.origin.x, ray.dir.x, h.x),
        (ray.origin.y, ray.dir.y, h.y),
        (ray.origin.z, ray.dir.z, h.z),
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
    let h = pick_half_size(entity)?;
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
    ray_box(&local, h).map(|t| t / len)
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
