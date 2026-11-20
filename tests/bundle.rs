//! The build bundle's contracts (Studio M5 packaging): `hyg build`
//! gathers a scene and every file it references (script sources, asset
//! meshes) into one directory, preserving the scene's relative layout
//! so the copied scene loads from the bundle root without edits. A
//! missing reference refuses the build loudly, naming the path.

use hyg::bundle;
use hyg::carrier;
use hyg::game::scene::Scene;
#[cfg(feature = "script")]
use hyg::game::{Camera, Entity, MeshRenderer, ScriptedComponent};
use hyg::Var;
use std::path::Path;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

use std::path::PathBuf;

/// A scene with a file-backed script and an asset model builds a
/// bundle that contains the scene, the script, and the asset — laid
/// out exactly as the scene references them.
#[test]
#[cfg(feature = "script")]
fn build_gathers_the_scene_and_its_references() {
    let dir = temp_dir("hyg-bundle-build");
    std::fs::create_dir_all(dir.join("assets")).expect("assets dir");
    std::fs::create_dir_all(dir.join("scripts")).expect("scripts dir");

    // the dependency files, as a real project lays them out
    std::fs::write(dir.join("scripts/patrol.js"), "function update(dt) {}").expect("script");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube.gltf"),
        dir.join("assets/cube.gltf"),
    )
    .expect("asset");

    // the scene: a scripted drifter + an asset-backed crate
    let mut drifter = Entity::new("drifter");
    drifter.transform_mut().position = hyg::var::Vec3::new(0.0, 1.5, -2.0);
    let script = hyg::script::Script::new(
        std::fs::read_to_string(dir.join("scripts/patrol.js")).expect("read"),
    )
    .expect("compiles");
    drifter.add_component(
        Box::new(
            ScriptedComponent::new(script)
                .expect("arms")
                .with_source_path("scripts/patrol.js"),
        ),
    );
    drifter.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(hyg::game::Model::single(std::sync::Arc::new(
            hyg::game::Mesh::sphere(0.25, 16, 12),
        ))),
        [0.4, 0.9, 0.6, 1.0],
    )));
    let mut crate_e = Entity::new("crate");
    crate_e.add_component(Box::new(MeshRenderer::asset(
        "assets/cube.gltf",
        [1.0; 4],
    )));

    // save into the same dir (the references resolve against it)
    let scene_path = dir.join("project.json.hyg");
    let doc = Scene::snapshot_document("bundle-me", &Camera::default(), &[drifter, crate_e]);
    carrier::write(&scene_path, &doc).expect("saves");

    // the bundler needs gltf to MATERIALIZE model-backed components in
    // the from_var path — but the bundle walks the RAW document, so
    // the asset reference is discovered regardless of the feature
    let out = dir.join("bundle");
    let report = bundle::build(&scene_path, &out).expect("builds");

    assert!(report.scene.exists(), "the scene copied into the bundle");
    let copied: Vec<String> = report
        .files
        .iter()
        .map(|p| p.strip_prefix(&out).unwrap().to_string_lossy().to_string())
        .collect();
    assert!(
        copied.iter().any(|f| f.ends_with("patrol.js")),
        "the script copied: {copied:?}"
    );
    assert!(
        copied.iter().any(|f| f.ends_with("cube.gltf")),
        "the asset copied: {copied:?}"
    );

    // the copied scene loads FROM THE BUNDLE (paths resolve against it)
    let scene = Scene::load(&report.scene).expect("the bundled scene loads in place");
    assert_eq!(scene.entities.len(), 2, "both entities");

    let _ = Var::Null; // the var crate re-export stays referenced
}

/// A missing reference refuses the build loudly, naming the scene and
/// the missing path — nothing is left half-copied as a surprise.
#[test]
fn build_refuses_on_a_missing_reference() {
    let dir = temp_dir("hyg-bundle-missing");
    let script_path = dir.join("scripts/gone.js");

    let doc = Var::from(hyg::o! {
        "scene": Var::from("missing"),
        "format": Var::from(1),
        "camera": Var::from(hyg::o! {
            "ortho": Var::from(hyg::o! { "half_extents": Var::from(hyg::a![
                Var::from(8.9f64), Var::from(5.0f64), Var::from(64.0f64)
            ]) }),
        }),
        "entities": Var::from(hyg::a![
            Var::from(hyg::o! {
                "name": Var::from("drifter"),
                "transform": Var::from(hyg::o! {}),
                "children": Var::from(hyg::a![]),
                "components": Var::from(hyg::a![
                    Var::from(hyg::o! {
                        "type": Var::from("script"),
                        "source": Var::from("scripts/gone.js"),
                    }),
                ]),
            }),
        ]),
    });
    let scene_path = dir.join("scene.json.hyg");
    carrier::write(&scene_path, &doc).expect("writes the scene");
    let _ = script_path;

    let out = dir.join("bundle");
    let err = bundle::build(&scene_path, &out).expect_err("refuses");
    assert!(
        err.contains("scripts/gone.js"),
        "the error names the missing path: {err}"
    );
    assert!(
        !out.join("scene.json.hyg").exists(),
        "the refusal lands nothing (the scene copies last)"
    );
}

/// Sub-scenes ride the bundle at their own positions: a prefab in
/// `rooms/` keeps its layout under the bundle root, so ITS references
/// (scripts, models, nested sub-scenes) land where the copied scene
/// resolves them — and the bundled world plays from the bundle root
/// with the instances spliced.
#[test]
fn build_carries_sub_scenes_at_their_own_positions() {
    let dir = temp_dir("hyg-bundle-sub-scene");
    // the project tree: world.json.hyg → rooms/room.json.hyg →
    // rooms/nested/desk.json.hyg; the room names a script relative to
    // ITSELF, the desk names a model relative to ITSELF
    std::fs::create_dir_all(dir.join("rooms/models")).expect("rooms/models");
    std::fs::create_dir_all(dir.join("rooms/nested/models")).expect("rooms/nested/models");
    // real glTF bytes: under the gltf feature the LOAD parses models,
    // not just the bundler
    let cube = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube.gltf"))
        .expect("the repo cube");
    std::fs::write(dir.join("rooms/models/lamp.glb"), &cube).expect("lamp model");
    std::fs::write(dir.join("rooms/nested/models/desk.glb"), &cube).expect("desk model");

    // the desk prefab: a model-backed body (the bundler reads RAW
    // documents — the file just needs to exist)
    let desk = Var::from(hyg::o! {
        "scene": Var::from("desk"),
        "format": Var::from(1),
        "entities": Var::from(hyg::a![
            Var::from(hyg::o! {
                "name": Var::from("desk"),
                "transform": Var::from(hyg::o! {}),
                "children": Var::from(hyg::a![]),
                "components": Var::from(hyg::a![
                    Var::from(hyg::o! {
                        "type": Var::from("mesh_renderer"),
                        "model": Var::from("models/desk.glb"),
                    }),
                ]),
            }),
        ]),
    });
    carrier::write(&dir.join("rooms/nested/desk.json.hyg"), &desk).expect("desk scene");

    // the room prefab: a model-backed lamp plus a desk instance
    let room = Var::from(hyg::o! {
        "scene": Var::from("room"),
        "format": Var::from(1),
        "entities": Var::from(hyg::a![
            Var::from(hyg::o! {
                "name": Var::from("lamp"),
                "transform": Var::from(hyg::o! {}),
                "children": Var::from(hyg::a![]),
                "components": Var::from(hyg::a![
                    Var::from(hyg::o! {
                        "type": Var::from("mesh_renderer"),
                        "model": Var::from("models/lamp.glb"),
                    }),
                ]),
            }),
            Var::from(hyg::o! {
                "name": Var::from("desk_spot"),
                "transform": Var::from(hyg::o! {}),
                "children": Var::from(hyg::a![]),
                "components": Var::from(hyg::a![
                    Var::from(hyg::o! {
                        "type": Var::from("sub_scene"),
                        "path": Var::from("nested/desk.json.hyg"),
                    }),
                ]),
            }),
        ]),
    });
    carrier::write(&dir.join("rooms/room.json.hyg"), &room).expect("room scene");

    // the world: one instance of the room
    let world = Var::from(hyg::o! {
        "scene": Var::from("world"),
        "format": Var::from(1),
        "entities": Var::from(hyg::a![
            Var::from(hyg::o! {
                "name": Var::from("room_3"),
                "transform": Var::from(hyg::o! {}),
                "children": Var::from(hyg::a![]),
                "components": Var::from(hyg::a![
                    Var::from(hyg::o! {
                        "type": Var::from("sub_scene"),
                        "path": Var::from("rooms/room.json.hyg"),
                    }),
                ]),
            }),
        ]),
    });
    let scene_path = dir.join("world.json.hyg");
    carrier::write(&scene_path, &world).expect("world scene");

    let out = dir.join("bundle");
    let report = bundle::build(&scene_path, &out).expect("builds");
    let copied: Vec<String> = report
        .files
        .iter()
        .map(|p| p.strip_prefix(&out).unwrap().to_string_lossy().to_string())
        .collect();
    // every dependency at ITS position: the room keeps `rooms/`, its
    // script resolves from there, the desk keeps `rooms/nested/`
    for expected in [
        "rooms/room.json.hyg",
        "rooms/models/lamp.glb",
        "rooms/nested/desk.json.hyg",
        "rooms/nested/models/desk.glb",
    ] {
        assert!(
            copied.iter().any(|f| f == expected),
            "{expected} missing from the bundle: {copied:?}"
        );
    }

    // the bundled world plays from the bundle root — the whole chain
    // splices (sub_scene loads are feature-free)
    let played = Scene::load(&report.scene).expect("the bundled world loads in place");
    let room_3 = &played.entities[0];
    let desk_spot = room_3
        .children()
        .iter()
        .find(|c| c.name() == "desk_spot")
        .expect("the room's instance spliced");
    assert!(
        desk_spot.children().iter().any(|c| c.name() == "desk"),
        "the desk spliced inside the instance"
    );
}

/// A sub-scene cycle refuses the build loudly (the bundler catches the
/// migration mistake before any runtime does).
#[test]
fn build_refuses_a_sub_scene_cycle() {
    let dir = temp_dir("hyg-bundle-cycle");
    let scene = |name: &str, other: &str| {
        Var::from(hyg::o! {
            "scene": Var::from(name),
            "format": Var::from(1),
            "entities": Var::from(hyg::a![
                Var::from(hyg::o! {
                    "name": Var::from(name),
                    "transform": Var::from(hyg::o! {}),
                    "children": Var::from(hyg::a![]),
                    "components": Var::from(hyg::a![
                        Var::from(hyg::o! {
                            "type": Var::from("sub_scene"),
                            "path": Var::from(other),
                        }),
                    ]),
                }),
            ]),
        })
    };
    carrier::write(&dir.join("a.json.hyg"), &scene("a", "b.json.hyg")).expect("a");
    let top = dir.join("top.json.hyg");
    carrier::write(&top, &scene("top", "a.json.hyg")).expect("top");

    let err = bundle::build(&top, &dir.join("bundle")).expect_err("the cycle refuses");
    assert!(err.contains("cycle"), "names the cycle: {err}");
}

/// A reference that climbs out of the project tree refuses — a bundle
/// that reaches outside itself cannot be tared, fused, or moved as
/// one file.
#[test]
fn build_refuses_root_escapes() {
    let dir = temp_dir("hyg-bundle-escape");
    std::fs::write(dir.join("outside.txt"), b"not in the bundle").expect("outside file");
    let doc = Var::from(hyg::o! {
        "scene": Var::from("escape"),
        "format": Var::from(1),
        "entities": Var::from(hyg::a![
            Var::from(hyg::o! {
                "name": Var::from("thief"),
                "transform": Var::from(hyg::o! {}),
                "children": Var::from(hyg::a![]),
                "components": Var::from(hyg::a![
                    Var::from(hyg::o! {
                        "type": Var::from("script"),
                        "source": Var::from("../outside.txt"),
                    }),
                ]),
            }),
        ]),
    });
    let scene_path = dir.join("scene.json.hyg");
    carrier::write(&scene_path, &doc).expect("writes the scene");

    let err = bundle::build(&scene_path, &dir.join("bundle")).expect_err("refuses");
    assert!(err.contains("escapes"), "names the escape: {err}");
}
