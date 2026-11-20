//! The mesh-import contracts (the `mesh-import` feature, polysmelt):
//! OBJ/STL/PLY files land as real hyg models — material colors baked
//! into vertex colors (the glTF path's convention), faceted sources
//! get smooth normals, vertex colors pass through, textures decode —
//! and `MeshRenderer::resolve` dispatches by extension.

#![cfg(feature = "mesh-import")]

use hyg::game::mesh_import::load_model;
use hyg::game::MeshRenderer;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// polysmelt normalizes color spaces: a file's sRGB colors (MTL `Kd`)
/// arrive LINEAR (glTF's convention). This mirrors its conversion so
/// the tests assert the relationship, not magic numbers.
fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A quad with normals, UVs, and an MTL material (base color blue).
const QUAD_OBJ: &str = "\
mtllib quad.mtl
usemtl blue
v -1.0 0.0 0.0
v 1.0 0.0 0.0
v 1.0 2.0 0.0
v -1.0 2.0 0.0
vt 0.0 0.0
vt 1.0 0.0
vt 1.0 1.0
vt 0.0 1.0
vn 0.0 0.0 1.0
f 1/1/1 2/2/1 3/3/1
f 1/1/1 3/3/1 4/4/1
";

const QUAD_MTL: &str = "\
newmtl blue
Kd 0.2 0.4 0.8
";

/// A faceted bracket: STL carries positions only — no shared indices,
/// no vertex normals.
const BRACKET_STL: &str = "\
solid bracket
facet normal 0 0 1
  outer loop
    vertex 0 0 0
    vertex 2 0 0
    vertex 2 1 0
  endloop
endfacet
facet normal 0 0 1
  outer loop
    vertex 0 0 0
    vertex 2 1 0
    vertex 0 1 0
  endloop
endfacet
endsolid bracket
";

/// A triangle with per-vertex colors (the PLY uchar RGB convention).
const TRI_PLY: &str = "\
ply
format ascii 1.0
element vertex 3
property float x
property float y
property float z
property uchar red
property uchar green
property uchar blue
element face 1
property list uchar int vertex_indices
end_header
0 0 0 255 0 0
1 0 0 0 255 0
0.5 1 0 0 0 255
3 0 1 2
";

/// OBJ: indexed geometry, the material's base color baked into vertex
/// colors (no per-vertex colors in the file), normals and UVs through.
#[test]
fn obj_imports_material_color_normals_uvs() {
    let dir = temp_dir("hyg-mesh-import-obj");
    std::fs::write(dir.join("quad.obj"), QUAD_OBJ).expect("obj");
    std::fs::write(dir.join("quad.mtl"), QUAD_MTL).expect("mtl");

    let model = load_model(&dir.join("quad.obj"), "obj").expect("imports");
    assert_eq!(model.parts().len(), 1, "one material, one part");
    let mesh = model.parts()[0].mesh.clone();
    assert_eq!(mesh.vertices().len(), 4, "indexed: four corners");
    assert_eq!(mesh.indices().len(), 6, "two triangles");
    let expect = [
        srgb_to_linear(0.2),
        srgb_to_linear(0.4),
        srgb_to_linear(0.8),
        1.0,
    ];
    for v in mesh.vertices() {
        assert_eq!(
            v.color,
            expect,
            "the MTL's Kd fills the vertex colors (sRGB arrives linear)"
        );
        assert_eq!(v.normal, [0.0, 0.0, 1.0], "the file's normals ride");
    }
    // OBJ's bottom-left UV origin flips to glTF's top-left convention
    assert_eq!(mesh.vertices()[0].uv, [0.0, 1.0], "UVs arrive top-left");

    // bounds over the baked positions (the pick path's proxy box)
    let (center, half) = model.bounds();
    assert_eq!(center.x, 0.0);
    assert_eq!(half.x, 1.0);
    assert_eq!(half.y, 1.0);
}

/// STL: a faceted soup — smooth normals computed over the coplanar
/// triangles, the default gray filling vertex colors.
#[test]
fn stl_faceted_gets_smooth_normals_and_gray() {
    let dir = temp_dir("hyg-mesh-import-stl");
    std::fs::write(dir.join("bracket.stl"), BRACKET_STL).expect("stl");

    let model = load_model(&dir.join("bracket.stl"), "stl").expect("imports");
    let mesh = model.parts()[0].mesh.clone();
    assert_eq!(mesh.triangle_count(), 2);
    // STL is Z-up; polysmelt normalizes to glTF's Y-up — the z-facing
    // bracket faces +y now
    for v in mesh.vertices() {
        // the rotation's fp dust (~2e-16) — approximate on each axis
        for (got, want) in v.normal.iter().zip([0.0f32, 1.0, 0.0].iter()) {
            assert!(
                (got - want).abs() < 1e-5,
                "coplanar facets smooth to the shared face normal (Y-up): {:?}",
                v.normal
            );
        }
        assert_eq!(
            v.color,
            [0.72, 0.72, 0.74, 1.0],
            "the unspecified-material gray fills"
        );
    }
    let (_, half) = model.bounds();
    assert_eq!(half.z, 0.5, "the bracket's unit thickness now spans z");
}

/// PLY: per-vertex colors carry through in linear RGBA.
#[test]
fn ply_carries_vertex_colors() {
    let dir = temp_dir("hyg-mesh-import-ply");
    std::fs::write(dir.join("tri.ply"), TRI_PLY).expect("ply");

    let model = load_model(&dir.join("tri.ply"), "ply").expect("imports");
    let mesh = model.parts()[0].mesh.clone();
    assert_eq!(mesh.vertices().len(), 3);
    assert_eq!(mesh.vertices()[0].color, [1.0, 0.0, 0.0, 1.0]);
    assert_eq!(mesh.vertices()[1].color, [0.0, 1.0, 0.0, 1.0]);
    assert_eq!(mesh.vertices()[2].color, [0.0, 0.0, 1.0, 1.0]);
}

/// The renderer's resolve dispatches by extension: an OBJ-backed
/// renderer materializes a real model through the same path the scene
/// factory drives.
#[test]
fn renderer_resolve_dispatches_by_extension() {
    let dir = temp_dir("hyg-mesh-import-resolve");
    std::fs::write(dir.join("quad.obj"), QUAD_OBJ).expect("obj");
    std::fs::write(dir.join("quad.mtl"), QUAD_MTL).expect("mtl");

    let mut renderer = MeshRenderer::asset("quad.obj", [1.0; 4]);
    renderer.resolve(&dir).expect("resolves");
    let model = match &renderer.geometry {
        hyg::game::MeshGeometry::Asset { model: Some(m), .. } => m.clone(),
        other => panic!("a resolved model: {other:?}"),
    };
    assert_eq!(model.parts().len(), 1);

    // an unresolved extension refuses with the honest message
    let mut bad = MeshRenderer::asset("quad.nope", [1.0; 4]);
    let err = bad.resolve(&dir).expect_err("refuses");
    assert!(
        err.contains("no importer") || err.contains("mesh import"),
        "names the failure: {err}"
    );
}

/// A textured material: the MTL's `map_Kd` decodes through the
/// `image` feature into the part's texture.
#[test]
#[cfg(feature = "image")]
fn obj_texture_decodes_into_the_part() {
    use std::io::Cursor;

    let dir = temp_dir("hyg-mesh-import-tex");
    std::fs::write(dir.join("quad.obj"), QUAD_OBJ).expect("obj");
    std::fs::write(
        dir.join("quad.mtl"),
        "newmtl blue\nKd 0.2 0.4 0.8\nmap_Kd quad.png\n",
    )
    .expect("mtl");
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255u8, 0, 0, 255]));
    let mut png = Cursor::new(Vec::new());
    img.write_to(&mut png, image::ImageFormat::Png).expect("png");
    std::fs::write(dir.join("quad.png"), png.into_inner()).expect("texture");

    let model = load_model(&dir.join("quad.obj"), "obj").expect("imports");
    let texture = model.parts()[0]
        .texture
        .as_ref()
        .expect("the map_Kd decoded");
    assert_eq!(texture.width(), 2);
    assert_eq!(texture.height(), 2);
    assert_eq!(texture.rgba()[0], 255, "the red pixel carried");
    let _ = Arc::clone(texture);
}

/// A file a `Path` reference resolves against: `quad.obj` names its
/// MTL and the MTL names its texture relative to the OBJ's own
/// directory (the loader's base-dir rule, mirrored by polysmelt's
/// Folder assets).
#[test]
fn references_resolve_against_the_models_dir() {
    let dir = temp_dir("hyg-mesh-import-nested");
    std::fs::create_dir_all(dir.join("models")).expect("models dir");
    std::fs::write(dir.join("models/quad.obj"), QUAD_OBJ).expect("obj");
    std::fs::write(dir.join("models/quad.mtl"), QUAD_MTL).expect("mtl");

    let model = load_model(&dir.join("models/quad.obj"), "obj").expect("imports");
    assert_eq!(model.parts().len(), 1, "the MTL resolved from the subdir");
    let expect = [
        srgb_to_linear(0.2),
        srgb_to_linear(0.4),
        srgb_to_linear(0.8),
        1.0,
    ];
    assert_eq!(
        model.parts()[0].mesh.vertices()[0].color,
        expect,
        "the MTL's color carried (the file was found)"
    );
    let _ = Path::new(".");
}
