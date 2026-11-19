//! The asset pipeline's core contracts (Studio M4): asset-backed mesh
//! renderers round-trip their PATH through scenes feature-free, the gltf
//! load materializes them against the scene directory, pick proxies use
//! the model's real bounds, and the debug snapshots read the physics and
//! audio worlds back.

use hyg::game::component::Component;
use hyg::game::editor::debug::PhysicsSnapshot;
use hyg::game::physics::{BodyType, PhysicsWorld, Shape};
use hyg::game::renderable::{MeshGeometry, MeshRenderer, Renderable};
use hyg::game::scene::Scene;
use hyg::game::{Camera, Entity};
use hyg::var::Vec3;
use hyg::Var;

/// `{"model": path, "color": ...}` builds an asset-backed renderer; the
/// path round-trips through scene_params feature-free (the model is
/// compiled geometry — the PATH is the data).
#[test]
fn asset_params_round_trip_feature_free() {
    let v = hyg::o! { "model": Var::from("assets/crate.glb") };
    let renderer = MeshRenderer::from_params(&Var::from(v.clone())).expect("asset params");
    assert_eq!(renderer.asset_path(), Some("assets/crate.glb"));
    assert!(
        matches!(
            renderer.geometry,
            MeshGeometry::Asset { ref model, .. } if model.is_none()
        ),
        "unresolved at construction: {:?}",
        renderer.geometry
    );
    // the save writes the path back (color defaulted out)
    let params = renderer.scene_params(&hyg::game::scene::ComponentContext {
        entity_name: "crate".into(),
        base: std::path::PathBuf::from("."),
    });
    let params = params.expect("asset renderers save");
    assert_eq!(params.get("model").as_str(), Some("assets/crate.glb"));
    assert!(
        matches!(params.get("shape"), Var::Null),
        "no shape key on an asset renderer"
    );

    // and the saved params rebuild the same renderer
    let rebuilt = MeshRenderer::from_params(&params).expect("rebuilt");
    assert_eq!(rebuilt.asset_path(), Some("assets/crate.glb"));
    let _ = v; // the params object is consumed by from_params via Var

    // unresolved: the frame contribution is NOTHING (not a placeholder)
    let world = hyg::game::Transform::default();
    assert!(
        matches!(
            renderer.geometry(&world),
            hyg::game::render::Geometry::None
        ),
        "an unresolved asset draws nothing"
    );
}

/// One geometry per renderer: both `model` and `shape` (or neither) is a
/// named error, not a silent pick.
#[test]
fn from_params_rejects_ambiguous_geometry() {
    let both = Var::from(hyg::o! {
        "model": Var::from("assets/a.glb"),
        "shape": Var::from("rect"),
    });
    let err = MeshRenderer::from_params(&both).expect_err("both present");
    assert!(format!("{err}").contains("both"), "{err}");

    let neither = Var::from(hyg::o! { "color": Var::from("1 1 1 1") });
    let err = MeshRenderer::from_params(&neither).expect_err("neither present");
    assert!(format!("{err}").contains("required"), "{err}");
}

/// An unresolved asset round-trips a scene BY PATH feature-free: without
/// the `gltf` feature nothing resolves, so the document cycles
/// byte-identically with no asset file present.
#[cfg(not(feature = "gltf"))]
#[test]
fn asset_scene_round_trips_by_path() {
    let mut entity = Entity::new("crate");
    entity.transform_mut().position = Vec3::new(2.0, 0.0, 0.0);
    entity.add_component(Box::new(MeshRenderer::asset(
        "assets/crate.glb",
        [1.0, 1.0, 1.0, 1.0],
    )));

    let doc = Scene::document("assets-test", &Camera::default(), &[entity]);
    let reloaded = Scene::from_var(&doc).expect("loads feature-free");
    let reloaded_doc = reloaded.to_var();
    assert_eq!(doc, reloaded_doc, "byte-identical cycle (Var equality)");

    let renderer = reloaded.entities[0]
        .iter_components()
        .find_map(|c| c.as_any().downcast_ref::<MeshRenderer>())
        .expect("the renderer survived the round trip");
    assert_eq!(renderer.asset_path(), Some("assets/crate.glb"));
}

/// The gltf-gated LOAD path: a scene written next to its asset, loaded
/// through the carrier (the real editor flow — paths resolve against the
/// scene file's directory), materializes a RESOLVED renderer whose
/// document round-trips byte-identically.
#[cfg(feature = "gltf")]
#[test]
fn scene_load_materializes_asset_renderers() {
    use hyg::carrier;

    let dir = std::env::temp_dir().join("hyg-assets-test-load");
    std::fs::create_dir_all(dir.join("assets")).expect("mkdir");
    let asset_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube.gltf");
    std::fs::copy(&asset_src, dir.join("assets/cube.gltf")).expect("copy the asset");

    let mut entity = Entity::new("crate");
    entity.add_component(Box::new(MeshRenderer::asset(
        "assets/cube.gltf",
        [1.0, 1.0, 1.0, 1.0],
    )));
    let doc = Scene::document("assets-load", &Camera::default(), &[entity]);

    let path = dir.join("scene.json.hyg");
    carrier::write(&path, &doc).expect("writes the scene");
    let scene = Scene::load(&path).expect("the load resolves the asset");
    let renderer = scene.entities[0]
        .iter_components()
        .find_map(|c| c.as_any().downcast_ref::<MeshRenderer>())
        .expect("the renderer");
    assert!(
        matches!(renderer.geometry, MeshGeometry::Asset { model: Some(_), .. }),
        "materialized at load: {:?}",
        renderer.geometry
    );

    // and the save→load→save cycle stays byte-identical WITH the model
    let doc2 = scene.to_var();
    carrier::write(&path, &doc2).expect("writes again");
    let scene2 = Scene::load(&path).expect("reload");
    assert_eq!(doc2, scene2.to_var(), "byte-identical with assets");
}

/// The gltf-gated half: resolve() loads the model against the scene
/// directory, the geometry becomes drawable, and the pick box uses the
/// model's REAL bounds.
#[cfg(feature = "gltf")]
#[test]
fn resolve_materializes_against_the_scene_dir() {
    // the scene dir + its asset, laid out like a real project
    let dir = std::env::temp_dir().join("hyg-assets-test");
    std::fs::create_dir_all(dir.join("assets")).expect("mkdir");
    let asset_src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube.gltf");
    std::fs::copy(&asset_src, dir.join("assets/cube.gltf")).expect("copy the asset");

    let mut renderer_component = MeshRenderer::asset("assets/cube.gltf", [1.0; 4]);
    renderer_component.resolve(&dir).expect("resolves");
    let model = match &renderer_component.geometry {
        MeshGeometry::Asset { model: Some(m), .. } => m.clone(),
        other => panic!("expected a loaded asset: {other:?}"),
    };
    assert!(!model.parts().is_empty(), "the cube's part");

    // the entity renders: geometry() yields a Model draw for the frame
    let world = hyg::game::Transform {
        position: Vec3::new(0.0, 0.0, 0.0),
        ..hyg::game::Transform::default()
    };
    assert!(
        matches!(
            renderer_component.geometry(&world),
            hyg::game::render::Geometry::Model { .. }
        ),
        "resolved assets draw"
    );

    // an entity carrying it PICKS through the model's real bounds (the
    // cube asset spans ±0.5 — a click at the corner's projection hits)
    let mut entity = Entity::new("crate");
    entity.add_component(Box::new(renderer_component));
    let mut engine = hyg::game::Engine::new("acme", "assets");
    engine.add_entity(entity);

    let camera = hyg::game::Camera {
        position: Vec3::new(0.0, 0.0, -4.0),
        target: Vec3::splat(0.0),
        projection: hyg::game::Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
        },
    };
    let view = hyg::game::editor::pick::Viewport::new(0.0, 0.0, 320.0, 180.0);
    let px = hyg::game::editor::pick::project_point(&camera, &view, Vec3::new(0.45, 0.45, 0.45))
        .expect("a cube corner is on screen");
    let picked = hyg::game::editor::pick::click_pick(&engine, &camera, &view, px);
    assert_eq!(picked, Some(vec![0]), "the corner click hits the real bounds");

    let _ = &model; // bounds exercised via pick; keep the handle alive
}

/// The physics snapshot: the none backend's recorded declarations come
/// back as read-only rows, replace semantics included.
#[test]
fn physics_snapshot_lists_declared_bodies() {
    let mut world = PhysicsWorld::none();
    world.set_gravity(Vec3::new(0.0, -9.8, 0.0));
    world.add_body(
        "floor",
        &Shape::Box(Vec3::new(6.0, 0.5, 6.0)),
        BodyType::Static,
        Vec3::splat(0.0),
        hyg::var::Quat::new(0.0, 0.0, 0.0, 1.0),
        1.0,
        0.0,
    );
    world.add_body(
        "ball",
        &Shape::Sphere(0.5),
        BodyType::Dynamic,
        Vec3::new(0.0, 8.0, 0.0),
        hyg::var::Quat::new(0.0, 0.0, 0.0, 1.0),
        1.0,
        0.0,
    );
    // replace semantics: re-registering "ball" doesn't duplicate
    world.add_body(
        "ball",
        &Shape::Sphere(0.25),
        BodyType::Dynamic,
        Vec3::new(1.0, 8.0, 0.0),
        hyg::var::Quat::new(0.0, 0.0, 0.0, 1.0),
        1.0,
        0.0,
    );

    let snap = PhysicsSnapshot::from_world(&world);
    assert_eq!(snap.backend, "none");
    assert_eq!(snap.gravity, Some(Vec3::new(0.0, -9.8, 0.0)));
    assert_eq!(snap.bodies.len(), 2, "replace, not append: {snap:?}");
    assert_eq!(snap.bodies[0].name, "ball"); // sorted
    assert_eq!(snap.bodies[0].shape, Shape::Sphere(0.25));
    assert_eq!(snap.bodies[1].name, "floor");
    assert_eq!(snap.bodies[1].body_type, BodyType::Static);
}

/// The audio snapshot: the none recorder's play records and master
/// volume read back through the introspection seam.
#[cfg(feature = "audio")]
#[test]
fn audio_snapshot_records_plays() {
    use hyg::game::audio::{AudioSource, AudioWorld, PlaySpec, WaveShape};
    use hyg::game::editor::debug::AudioSnapshot;

    let mut world = AudioWorld::none();
    let tone = AudioSource::Tone {
        frequency: 440.0,
        duration: 0.2,
        shape: WaveShape::Sine,
    };
    world
        .play(&tone, &PlaySpec::new(0.5, false))
        .expect("the none recorder accepts tones");
    world.set_master_volume(0.25);

    let snap = AudioSnapshot::from_world(&world);
    assert_eq!(snap.backend, "none");
    assert_eq!(snap.master_volume, Some(0.25));
    let plays = snap.plays.expect("the none recorder keeps records");
    assert_eq!(plays.len(), 1);
    assert!((plays[0].volume - 0.5).abs() < 1e-6, "the spec volume recorded");
}
