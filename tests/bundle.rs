//! The build bundle's contracts (Studio M5 packaging): `hyg build`
//! gathers a scene and every file it references (script sources, asset
//! meshes) into one directory, preserving the scene's relative layout
//! so the copied scene loads from the bundle root without edits. A
//! missing reference refuses the build loudly, naming the path.

use hyg::bundle;
use hyg::carrier;
use hyg::game::scene::Scene;
use hyg::game::{Camera, Entity};
use hyg::game::{MeshRenderer, ScriptedComponent};
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
