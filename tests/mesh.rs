//! Mesh unit tests (no GPU): the procedural builders produce valid,
//! closed, outward-wound geometry, and `Mesh::new` rejects broken data.

use hydrargyros::game::mesh::{Mesh, MeshVertex};
use hydrargyros::var::Vec3;

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The cube: 24 verts (4 per face), 12 triangles, all inside the box,
/// unit axis-aligned normals, and every triangle wound CCW seen from
/// outside (the glTF convention the renderer back-face-culls against).
#[test]
fn cube_is_closed_and_wound_outward() {
    let he = Vec3::new(0.5, 1.0, 2.0);
    let mesh = Mesh::cube(he);
    assert_eq!(mesh.vertices().len(), 24);
    assert_eq!(mesh.triangle_count(), 12);

    for v in mesh.vertices() {
        assert!(v.pos[0].abs() <= he.x && v.pos[1].abs() <= he.y && v.pos[2].abs() <= he.z);
        // unit normal on one axis (flat faces)
        let len = dot(v.normal, v.normal);
        assert!((len - 1.0).abs() < 1e-6, "normal {v:?} not unit");
        assert!(v.normal[0].abs() < 1e-6 || v.normal[1].abs() < 1e-6 || v.normal[2].abs() < 1e-6);
        assert_eq!(v.color, [1.0; 4], "procedural meshes are white");
    }

    // winding: (b−a)×(c−a) agrees with the face normal
    for tri in mesh.indices().as_chunks::<3>().0 {
        let at = |i: u32| mesh.vertices()[i as usize].pos;
        let (a, b, c) = (at(tri[0]), at(tri[1]), at(tri[2]));
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = mesh.vertices()[tri[0] as usize].normal;
        assert!(dot(cross(e1, e2), n) > 0.0, "triangle {tri:?} wound inward");
    }

    // six faces, four vertices each (grouped by normal)
    let mut normals: Vec<[f32; 3]> = mesh.vertices().iter().map(|v| v.normal).collect();
    normals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut runs: Vec<([f32; 3], usize)> = Vec::new();
    for n in &normals {
        if runs.last().map(|(prev, _)| *prev == *n).unwrap_or(false) {
            runs.last_mut().unwrap().1 += 1;
        } else {
            runs.push((*n, 1));
        }
    }
    assert_eq!(runs.len(), 6, "six distinct face normals");
    assert!(runs.iter().all(|&(_, count)| count == 4));
}

/// The sphere: every vertex on the radius, normals pointing outward.
#[test]
fn sphere_wraps_the_radius() {
    let (radius, segments, rings) = (1.0, 8, 6);
    let mesh = Mesh::sphere(radius, segments, rings);
    assert_eq!(
        mesh.vertices().len(),
        ((rings + 1) * (segments + 1)) as usize
    );
    assert_eq!(mesh.triangle_count(), (rings * segments * 2) as usize);

    for v in mesh.vertices() {
        let len = (v.pos[0] * v.pos[0] + v.pos[1] * v.pos[1] + v.pos[2] * v.pos[2]).sqrt();
        assert!((len - radius).abs() < 1e-5, "vertex {v:?} off the radius");
        // smooth normals equal the normalized position
        for (n, p) in v.normal.iter().zip(v.pos) {
            assert!((n - p / radius).abs() < 1e-5);
        }
    }
    for tri in mesh.indices().as_chunks::<3>().0 {
        assert!(tri.iter().all(|&i| (i as usize) < mesh.vertices().len()));
    }
}

/// Small builders clamp degenerate segment counts instead of panicking.
#[test]
fn sphere_clamps_degenerate_counts() {
    let mesh = Mesh::sphere(1.0, 0, 1);
    assert!(!mesh.vertices().is_empty());
    assert_eq!(mesh.triangle_count() * 3, mesh.indices().len());
}

/// `Mesh::new` validates: no empty meshes, whole triangles only, indices
/// in range.
#[test]
fn new_rejects_broken_data() {
    assert!(Mesh::new(vec![], vec![]).is_err(), "no vertices");

    let tri = vec![
        MeshVertex::new([0.0, 0.0, 0.0]),
        MeshVertex::new([1.0, 0.0, 0.0]),
        MeshVertex::new([0.0, 1.0, 0.0]),
    ];
    assert!(
        Mesh::new(tri.clone(), vec![0, 1]).is_err(),
        "not whole triangles"
    );
    assert!(
        Mesh::new(tri.clone(), vec![0, 1, 3]).is_err(),
        "index out of range"
    );

    let mesh = Mesh::new(tri, vec![0, 1, 2]).expect("valid triangle");
    assert_eq!(mesh.id(), mesh.id());
    // a second mesh gets a distinct id (GPU backends cache against it)
    let other = Mesh::new(vec![MeshVertex::new([0.0; 3]); 3], vec![0, 1, 2]).unwrap();
    assert_ne!(mesh.id(), other.id());
}
