//! glTF loader tests (behind the `gltf` feature): the checked-in cube
//! asset loads through the data-URI path, node transforms bake into
//! vertices, primitives without normals get flat ones computed, and
//! materials with base-color textures split the model into parts.

#![cfg(feature = "gltf")]

use hydrargyros::game::model::Model;
use std::io::Cursor;

/// Pack a GLB: magic + version + length, a JSON chunk (space-padded to 4),
/// a BIN chunk (zero-padded to 4). Lets tests build models in memory —
/// node transforms, missing normals, non-indexed primitives — without
/// asset files.
fn build_glb(json: serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(&json).expect("json");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let total = (12 + 8 + json_bytes.len()) as u32
        + if bin.is_empty() {
            0
        } else {
            8 + (bin.len().div_ceil(4) * 4) as u32
        };
    let mut out = Vec::with_capacity(total as usize);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // "glTF"
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&total.to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_bytes);
    if !bin.is_empty() {
        let mut bin_padded = bin.to_vec();
        while !bin_padded.len().is_multiple_of(4) {
            bin_padded.push(0);
        }
        out.extend_from_slice(&(bin_padded.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin_padded);
    }
    out
}

/// The same 24-vertex cube `assets/cube.gltf` carries: positions +
/// per-face normals + u16 indices. `(with_normals, with_indices)` toggles
/// accessors so tests can exercise the fallback paths.
fn cube_gltf_json(
    node: serde_json::Value,
    with_normals: bool,
    with_indices: bool,
) -> serde_json::Value {
    let faces = [
        ([1.0f32, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
        ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
        ([0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, -1.0]),
        ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
        ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        ([0.0, 0.0, -1.0], [-1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
    ];
    let h = 0.5f32;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    for &(n, u, v) in &faces {
        let fc = [n[0] * h, n[1] * h, n[2] * h];
        let us = [u[0] * h, u[1] * h, u[2] * h];
        let vs = [v[0] * h, v[1] * h, v[2] * h];
        let base = positions.len() as u16;
        let corners = [
            [
                fc[0] - us[0] - vs[0],
                fc[1] - us[1] - vs[1],
                fc[2] - us[2] - vs[2],
            ],
            [
                fc[0] + us[0] - vs[0],
                fc[1] + us[1] - vs[1],
                fc[2] + us[2] - vs[2],
            ],
            [
                fc[0] + us[0] + vs[0],
                fc[1] + us[1] + vs[1],
                fc[2] + us[2] + vs[2],
            ],
            [
                fc[0] - us[0] + vs[0],
                fc[1] - us[1] + vs[1],
                fc[2] - us[2] + vs[2],
            ],
        ];
        if with_indices {
            positions.extend(corners);
            normals.extend(std::iter::repeat_n(n, 4));
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        } else {
            // non-indexed: each triangle carries its own three corners
            for tri in [[0usize, 1, 2], [0, 2, 3]] {
                for &c in &tri {
                    positions.push(corners[c]);
                    normals.push(n);
                }
            }
        }
    }

    let mut blob: Vec<u8> = Vec::new();
    let push_f32s = |vals: &[[f32; 3]], blob: &mut Vec<u8>| {
        for v in vals {
            for f in v {
                blob.extend_from_slice(&f.to_le_bytes());
            }
        }
    };
    push_f32s(&positions, &mut blob);
    let normals_offset = blob.len();
    if with_normals {
        push_f32s(&normals, &mut blob);
    }
    let indices_offset = blob.len();
    if with_indices {
        for i in &indices {
            blob.extend_from_slice(&i.to_le_bytes());
        }
    }

    let mut attributes = serde_json::Map::new();
    attributes.insert("POSITION".into(), 0.into());
    if with_normals {
        attributes.insert("NORMAL".into(), 1.into());
    }
    let indices_view = if with_normals { 2 } else { 1 };
    let mut primitives = serde_json::Map::new();
    primitives.insert("attributes".into(), attributes.into());
    if with_indices {
        primitives.insert("indices".into(), indices_view.into());
    }

    let mut accessors = vec![serde_json::json!({
        "bufferView": 0, "componentType": 5126, "count": positions.len(), "type": "VEC3",
        "min": [-h, -h, -h], "max": [h, h, h],
    })];
    let mut buffer_views =
        vec![serde_json::json!({"buffer": 0, "byteOffset": 0, "byteLength": positions.len() * 12})];
    if with_normals {
        accessors.push(serde_json::json!({
            "bufferView": 1, "componentType": 5126, "count": normals.len(), "type": "VEC3",
        }));
        buffer_views.push(serde_json::json!({
            "buffer": 0, "byteOffset": normals_offset, "byteLength": normals.len() * 12
        }));
    }
    if with_indices {
        accessors.push(serde_json::json!({
            "bufferView": indices_view, "componentType": 5123, "count": indices.len(), "type": "SCALAR",
        }));
        buffer_views.push(serde_json::json!({
            "buffer": 0, "byteOffset": indices_offset, "byteLength": indices.len() * 2
        }));
    }

    serde_json::json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [node],
        "meshes": [{"primitives": [primitives]}],
        "buffers": [{
            "uri": format!("data:application/octet-stream;base64,{}", data_base64(&blob)),
            "byteLength": blob.len(),
        }],
        "bufferViews": buffer_views,
        "accessors": accessors,
    })
}

/// A 2×2 red PNG (via the `image` dev-dependency) for texture tests.
fn tiny_png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(2, 2, image::Rgba([255u8, 0, 0, 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("png encode");
    out.into_inner()
}

/// The cube from `cube_gltf_json` plus TEXCOORD_0 and a textured material,
/// with a second untextured material sharing the same accessors — one
/// primitive per material, so the loader must split the model in two.
/// Returns (json, bin): the PNG texture rides in the GLB blob via a
/// bufferView image (slice imports don't decode URI images).
fn two_material_gltf_json() -> (serde_json::Value, Vec<u8>) {
    let mut cube = cube_gltf_json(serde_json::json!({"mesh": 0}), true, true);

    // the buffer becomes the GLB blob: decode it, extend with UVs + PNG
    let old_uri = cube["buffers"][0]["uri"].as_str().unwrap().to_string();
    let mut blob = data_decode(old_uri.split("base64,").nth(1).unwrap());
    let uv_blob: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let uv_offset = blob.len();
    blob.extend_from_slice(&uv_blob);
    while !blob.len().is_multiple_of(4) {
        blob.push(0);
    }
    let png_offset = blob.len();
    let png = tiny_png();
    blob.extend_from_slice(&png);
    while !blob.len().is_multiple_of(4) {
        blob.push(0);
    }

    // buffer 0 is now the GLB blob itself (no URI)
    cube["buffers"] = serde_json::json!([{ "byteLength": blob.len() }]);

    let uv_view = cube["bufferViews"].as_array_mut().unwrap().len();
    cube["bufferViews"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "buffer": 0, "byteOffset": uv_offset, "byteLength": uv_blob.len(),
        }));
    let png_view = cube["bufferViews"].as_array_mut().unwrap().len();
    cube["bufferViews"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "buffer": 0, "byteOffset": png_offset, "byteLength": png.len(),
        }));
    let uv_accessor = cube["accessors"].as_array_mut().unwrap().len();
    cube["accessors"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "bufferView": uv_view, "componentType": 5126, "count": 24, "type": "VEC2",
        }));

    // materials: 0 = textured, 1 = plain orange
    cube["textures"] = serde_json::json!([{"sampler": 0, "source": 0}]);
    cube["images"] = serde_json::json!([{ "bufferView": png_view, "mimeType": "image/png" }]);
    cube["samplers"] = serde_json::json!([{}]);
    cube["materials"] = serde_json::json!([
        {"pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}},
        {"pbrMetallicRoughness": {"baseColorFactor": [0.9, 0.3, 0.1, 1.0]}},
    ]);
    // two primitives sharing accessors, one per material
    let mut prim_a = cube["meshes"][0]["primitives"][0].clone();
    let mut prim_b = prim_a.clone();
    prim_b["material"] = serde_json::json!(1);
    prim_a["material"] = serde_json::json!(0);
    prim_a["attributes"]["TEXCOORD_0"] = serde_json::json!(uv_accessor);
    cube["meshes"][0]["primitives"] = serde_json::json!([prim_a, prim_b]);
    (cube, blob)
}

/// Minimal base64 (RFC 4648, padding) — three lines, no dependency.
fn data_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// The inverse of `data_base64` (a test helper re-extending buffers).
fn data_decode(b64: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let value = |c: char| ALPHABET.iter().position(|&a| a as char == c).unwrap() as u32;
    let mut out = Vec::new();
    let bytes: Vec<char> = b64.chars().filter(|c| *c != '=').collect();
    for chunk in bytes.chunks(4) {
        let n = (value(chunk[0]) << 18)
            | (value(chunk[1]) << 12)
            | (value(chunk[2]) << 6)
            | value(chunk[3]);
        out.push((n >> 16) as u8);
        out.push((n >> 8) as u8);
        out.push(n as u8);
    }
    out
}

/// The checked-in asset loads through `from_gltf_bytes` (data-URI buffer,
/// GLB-less JSON) with the material base color baked into every vertex.
#[test]
fn checked_in_cube_loads_from_bytes() {
    let bytes = include_bytes!("../assets/cube.gltf");
    let model = Model::from_gltf_bytes(bytes).expect("cube.gltf loads");
    assert_eq!(model.parts().len(), 1, "one material → one part");
    assert!(
        model.parts()[0].texture.is_none(),
        "cube.gltf has no texture"
    );
    let mesh = &model.parts()[0].mesh;
    assert_eq!(mesh.vertices().len(), 24);
    assert_eq!(mesh.triangle_count(), 12);
    for v in mesh.vertices() {
        assert_eq!(
            v.color,
            [0.9, 0.3, 0.1, 1.0],
            "material base color bakes in"
        );
        assert!(v.pos[0].abs() <= 0.5 && v.pos[1].abs() <= 0.5 && v.pos[2].abs() <= 0.5);
        let len =
            (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
                .sqrt();
        assert!((len - 1.0).abs() < 1e-6);
    }
}

/// The same asset loads from disk through `from_gltf_file`.
#[test]
fn checked_in_cube_loads_from_file() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube.gltf");
    let model = Model::from_gltf_file(&path).expect("cube.gltf loads");
    assert_eq!(model.parts()[0].mesh.triangle_count(), 12);
}

/// Node translation + scale compose down the tree and bake into the
/// vertices — the loader hands back world-space geometry.
#[test]
fn node_transforms_bake_into_vertices() {
    let json = cube_gltf_json(
        serde_json::json!({"mesh": 0, "translation": [1.0, 2.0, 3.0], "scale": [2.0, 1.0, 1.0]}),
        true,
        true,
    );
    let model = Model::from_gltf_bytes(&build_glb(json, &[])).expect("glb loads");
    let mesh = &model.parts()[0].mesh;
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in mesh.vertices() {
        for axis in 0..3 {
            min[axis] = min[axis].min(v.pos[axis]);
            max[axis] = max[axis].max(v.pos[axis]);
        }
    }
    // ±0.5 cube, scaled (2, 1, 1), translated (1, 2, 3)
    assert!((min[0] - 0.0).abs() < 1e-5 && (max[0] - 2.0).abs() < 1e-5);
    assert!((min[1] - 1.5).abs() < 1e-5 && (max[1] - 2.5).abs() < 1e-5);
    assert!((min[2] - 2.5).abs() < 1e-5 && (max[2] - 3.5).abs() < 1e-5);
}

/// A primitive without NORMAL accessors gets flat per-triangle normals
/// computed from the (transformed) positions.
#[test]
fn missing_normals_come_out_flat() {
    let json = cube_gltf_json(serde_json::json!({"mesh": 0}), false, true);
    let model = Model::from_gltf_bytes(&build_glb(json, &[])).expect("glb loads");
    let mesh = &model.parts()[0].mesh;
    for v in mesh.vertices() {
        let len =
            (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1] + v.normal[2] * v.normal[2])
                .sqrt();
        assert!((len - 1.0).abs() < 1e-5, "computed normal is unit");
    }
    // a triangle lying entirely in one z = ±0.5 plane is a ±z face: its
    // flat normal must point outward (the face corners share edges with
    // neighboring faces, so per-vertex checks can't discriminate)
    for tri in mesh.indices().as_chunks::<3>().0 {
        let at = |i: u32| mesh.vertices()[i as usize];
        let (a, b, c) = (at(tri[0]), at(tri[1]), at(tri[2]));
        let same_z = (b.pos[2] - a.pos[2]).abs() < 1e-5 && (c.pos[2] - a.pos[2]).abs() < 1e-5;
        if same_z && (a.pos[2].abs() - 0.5).abs() < 1e-5 {
            let want = if a.pos[2] > 0.0 {
                [0.0, 0.0, 1.0]
            } else {
                [0.0, 0.0, -1.0]
            };
            for v in [a, b, c] {
                for (got, w) in v.normal.iter().zip(want) {
                    assert!((got - w).abs() < 1e-5, "flat normal {v:?} vs {want:?}");
                }
            }
        }
    }
}

/// A non-indexed primitive (no `indices`) still loads — vertices become
/// their own triangle list (36 per-triangle corners, 12 triangles).
#[test]
fn non_indexed_primitives_load() {
    let json = cube_gltf_json(serde_json::json!({"mesh": 0}), true, false);
    let model = Model::from_gltf_bytes(&build_glb(json, &[])).expect("glb loads");
    let mesh = &model.parts()[0].mesh;
    assert_eq!(mesh.vertices().len(), 36);
    assert_eq!(mesh.triangle_count(), 12);
}

/// A material with a base-color texture resolves into a `Texture` part —
/// the checked-in checker cube carries one.
#[test]
fn checked_in_checker_cube_loads_its_texture() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let model = Model::from_gltf_file(dir.join("cube_checker.gltf")).expect("checker cube loads");
    assert_eq!(model.parts().len(), 1);
    let part = &model.parts()[0];
    let texture = part
        .texture
        .as_ref()
        .expect("the material samples a texture");
    assert_eq!((texture.width(), texture.height()), (8, 8));
    // the checker's first block is the light shade
    assert_eq!(&texture.rgba()[..4], &[245, 240, 225, 255]);
    // UVs survive the load (full 0..1 coverage on every face)
    assert!(part.mesh.vertices().iter().any(|v| v.uv == [0.0, 0.0]));
    assert!(part.mesh.vertices().iter().any(|v| v.uv == [1.0, 1.0]));
}

/// Primitives with different materials split into parts: one textured,
/// one plain with its base color baked into the vertices.
#[test]
fn materials_split_into_parts() {
    let (json, bin) = two_material_gltf_json();
    let model = Model::from_gltf_bytes(&build_glb(json, &bin)).expect("two-material glb loads");
    assert_eq!(model.parts().len(), 2, "one part per base-color texture");
    let textured = model
        .parts()
        .iter()
        .find(|p| p.texture.is_some())
        .expect("textured part");
    assert_eq!(textured.texture.as_ref().unwrap().width(), 2);
    let plain = model
        .parts()
        .iter()
        .find(|p| p.texture.is_none())
        .expect("plain part");
    assert!(plain
        .mesh
        .vertices()
        .iter()
        .all(|v| v.color == [0.9, 0.3, 0.1, 1.0]));
}

/// Garbage input is an error, not a panic.
#[test]
fn garbage_bytes_error() {
    assert!(Model::from_gltf_bytes(b"definitely not a gltf").is_err());
}
