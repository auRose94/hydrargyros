//! The scene model contracts (Studio M0): scenes (de)serialize headless,
//! `FromVar` rejects bad scenes with path errors, and the editor's git
//! contract holds — build → save → load → save is byte-identical.

use hyg::game::camera::Projection;
use hyg::game::component::Component;
use hyg::game::light::{Light, LightKind};
use hyg::game::physics::PhysicsWorld;
use hyg::game::physics_body::PhysicsBody;
use hyg::game::renderable::{MeshGeometry, MeshRenderer, MeshShape};
use hyg::game::scene::{self, Scene};
use hyg::game::{Camera, Entity};
use hyg::var::{Quat, Vec3};
use hyg::Var;
use std::path::PathBuf;

/// A test component: registered under "counter", scene-schema carrying —
/// the user-component story (a factory in the registry, params in, params
/// out).
struct Counter {
    start: i64,
    count: i64,
}

impl Component for Counter {
    fn update(&mut self, _dt: f64) {
        self.count += 1;
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn scene_type(&self) -> Option<&'static str> {
        Some("counter")
    }
    fn scene_params(&self, _ctx: &scene::ComponentContext) -> Option<Var> {
        // the live count is runtime state; the document carries the config
        Some(Var::from(hyg::o! { "start": Var::from(self.start) }))
    }
}

fn register_counter() {
    scene::register_component_factory(
        "counter",
        std::sync::Arc::new(|params, _ctx| {
            Ok(Box::new(Counter {
                start: params.get("start").as_i64().unwrap_or(0),
                count: 0,
            }) as Box<dyn Component>)
        }),
    );
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hyg-scene-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Parse a document Var from JSON text — scenes are JSON documents, and
/// the fixtures read exactly like what lands on disk.
fn doc(text: &str) -> Var {
    serde_json::from_str::<Var>(text).expect("fixture is valid JSON")
}

/// The M0 showcase scene: nested entities, non-default transforms
/// (including f32 shortest-decimal traps), all four ungated built-in
/// components, a non-default camera.
fn demo_scene() -> Scene {
    register_counter();
    let mut scene = Scene::new("demo");
    scene.camera = Camera {
        position: Vec3::new(0.0, 4.0, -8.0),
        target: Vec3::new(0.0, 1.0, 0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 70.0,
            near: 0.2,
            far: 50.0,
            aspect: 1.7777778,
        },
    };

    let mut root = Entity::new("stack");
    root.transform_mut().position = Vec3::new(0.0, 2.1, -3.5);
    root.transform_mut().rotation = Quat::new(0.2, 0.4, 0.6, 0.8);
    root.add_component(Box::new(
        Light::spot()
            .with_color([1.0, 0.9, 0.8])
            .with_intensity(2.5)
            .with_cone([0.8, 0.6])
            .with_range(30.0),
    ));
    root.add_component(Box::new(
        PhysicsBody::new(
            "stack_body",
            hyg::game::physics::Shape::Box(Vec3::new(0.5, 0.5, 0.5)),
        )
        .static_body()
        .mass(1.5),
    ));

    let mut block = Entity::new("block_3");
    block.transform_mut().position = Vec3::new(0.0, 1.0, 0.0);
    block.transform_mut().scale = Vec3::new(1.0, 2.0, 0.5);
    block.add_component(Box::new(MeshRenderer::new(
        MeshGeometry::Shape {
            shape: MeshShape::Circle,
            half_size: Vec3::new(0.3, 0.3, 0.3),
        },
        [0.8, 0.2, 0.1, 1.0],
    )));
    block.add_component(Box::new(Counter {
        start: 5,
        count: 0,
    }));

    let mut nested = Entity::new("deco");
    nested.set_enabled(false);
    nested.add_component(Box::new(Light::default()));
    block.add_child(nested);
    root.add_child(block);
    scene.entities.push(root);
    scene
}

/// THE milestone contract: build → save → load → save is byte-identical,
/// three generations deep.
#[test]
fn byte_identical_round_trip() {
    let dir = scratch("roundtrip");
    let path = dir.join("demo.json.hyg");

    let scene = demo_scene();
    scene.save(&path).unwrap();
    let gen_a = std::fs::read(&path).unwrap();

    let loaded = Scene::load(&path).unwrap();
    loaded.save(&path).unwrap();
    let gen_b = std::fs::read(&path).unwrap();
    assert_eq!(gen_a, gen_b, "generation 1 → 2 drifted");

    let reloaded = Scene::load(&path).unwrap();
    reloaded.save(&path).unwrap();
    let gen_c = std::fs::read(&path).unwrap();
    assert_eq!(gen_b, gen_c, "generation 2 → 3 drifted");

    // and the canonical document is stable in memory too
    assert_eq!(reloaded.to_var(), loaded.to_var());

    std::fs::remove_dir_all(dir).ok();
}

/// The canonical regeneration contract: a hand-written scene (integer
/// numbers, no camera, terse entities) loads, and its first canonical save
/// is already a fixed point — the next save matches byte-for-byte.
#[test]
fn canonical_regeneration() {
    let dir = scratch("canonical");
    let path = dir.join("hand.json.hyg");
    std::fs::write(
        &path,
        br#"{
  "scene": "hand-made",
  "format": 1,
  "entities": [
    {
      "name": "sun",
      "transform": { "position": [0, 4, 0] },
      "components": [ { "type": "light", "kind": "directional" } ]
    }
  ]
}"#,
    )
    .unwrap();

    let loaded = Scene::load(&path).unwrap();
    assert_eq!(loaded.name, "hand-made");
    loaded.save(&path).unwrap();
    let gen_a = std::fs::read(&path).unwrap();
    let reloaded = Scene::load(&path).unwrap();
    reloaded.save(&path).unwrap();
    assert_eq!(gen_a, std::fs::read(&path).unwrap(), "not a fixed point");

    // camera absent → engine default; light's non-default kind survived
    assert_eq!(reloaded.camera, Camera::default());
    let entity = &reloaded.entities[0];
    let light = entity.get_component::<Light>().expect("light materialized");
    assert_eq!(light.kind, LightKind::Directional);

    std::fs::remove_dir_all(dir).ok();
}

/// Bad scenes fail loud, with the `FromVar` path naming the offending
/// field — the load-side validator contract.
#[test]
fn from_var_rejections_carry_paths() {
    let bad_mass = doc(
        r#"{
        "scene": "bad", "format": 1,
        "entities": [
            { "name": "b", "components": [
                { "type": "physics_body", "shape": "box",
                  "half_extents": [0.5, 0.5, 0.5], "mass": "heavy" }
            ] }
        ] }"#,
    );
    let err = Scene::from_var(&bad_mass).err().expect("must fail");
    assert!(
        err.to_string().contains("entities.0.components.0.mass"),
        "path missing from: {err}"
    );
    assert!(err.to_string().contains("expected a number"), "{err}");

    // arity: a 2-number position
    let bad_vec = doc(
        r#"{ "scene": "bad", "format": 1, "entities": [
            { "name": "b", "transform": { "position": [1.0, 2.0] } } ] }"#,
    );
    let err = Scene::from_var(&bad_vec).err().expect("must fail");
    assert!(
        err.to_string().contains("entities.0.transform.position"),
        "{err}"
    );

    // rotation arity: a quaternion is 4 numbers
    let bad_quat = doc(
        r#"{ "scene": "bad", "format": 1, "entities": [
            { "name": "b", "transform": { "rotation": [0.0, 0.0, 1.0] } } ] }"#,
    );
    let err = Scene::from_var(&bad_quat).err().expect("must fail");
    assert!(
        err.to_string().contains("entities.0.transform.rotation"),
        "{err}"
    );

    // a nested child's failure names the whole walk
    let bad_child = doc(
        r#"{ "scene": "bad", "format": 1, "entities": [
            { "name": "root", "children": [
                { "name": "ok" },
                { "name": "" }
            ] } ] }"#,
    );
    let err = Scene::from_var(&bad_child).err().expect("must fail");
    assert!(
        err.to_string().contains("entities.0.children.1.name"),
        "{err}"
    );

    // a component without a type
    let no_type = doc(
        r#"{ "scene": "bad", "format": 1, "entities": [
            { "name": "b", "components": [ { "shape": "box" } ] } ] }"#,
    );
    let err = Scene::from_var(&no_type).err().expect("must fail");
    assert!(
        err.to_string().contains("entities.0.components.0.type"),
        "{err}"
    );

    // shape "box" without half_extents
    let no_half = doc(
        r#"{ "scene": "bad", "format": 1, "entities": [
            { "name": "b", "components": [
                { "type": "physics_body", "shape": "box" } ] } ] }"#,
    );
    let err = Scene::from_var(&no_half).err().expect("must fail");
    assert!(
        err.to_string()
            .contains("entities.0.components.0.half_extents"),
        "{err}"
    );

    // entities not a list
    let bad_roots = doc(
        r#"{ "scene": "bad", "format": 1, "entities": { "oops": true } }"#,
    );
    let err = Scene::from_var(&bad_roots).err().expect("must fail");
    assert!(err.to_string().contains("\"entities\""), "{err}");
}

/// The format gate and the name requirement — the document-level checks.
#[test]
fn document_gates() {
    let wrong_format = doc(r#"{ "scene": "x", "format": 99 }"#);
    let err = Scene::from_var(&wrong_format).err().expect("must fail");
    assert!(err.to_string().contains("format"), "{err}");

    let no_name = doc(r#"{ "format": 1 }"#);
    let err = Scene::from_var(&no_name).err().expect("must fail");
    assert!(err.to_string().contains("\"scene\""), "{err}");

    let not_object = Var::from(hyg::a![1, 2]);
    let err = Scene::from_var(&not_object).err().expect("must fail");
    assert!(err.to_string().contains("expected an object"), "{err}");
}

/// Unknown component types are explicit errors naming what IS registered.
#[test]
fn unknown_component_type_names_the_registry() {
    let bad = doc(
        r#"{ "scene": "bad", "format": 1, "entities": [
            { "name": "b", "components": [ { "type": "frobnicator" } ] } ] }"#,
    );
    let err = Scene::from_var(&bad).err().expect("must fail");
    let text = err.to_string();
    assert!(
        text.contains("unknown component type \"frobnicator\""),
        "{text}"
    );
    assert!(text.contains("light"), "{text}");
    assert!(text.contains("physics_body"), "{text}");
    assert!(text.contains("mesh_renderer"), "{text}");
}

/// Defaults are omitted on save, and explicit-default scenes serialize
/// byte-identically to default-built ones — the canonical form.
#[test]
fn defaults_are_omitted_canonically() {
    let mut plain = Scene::new("s");
    let mut e = Entity::new("lamp");
    e.add_component(Box::new(Light::default()));
    plain.entities.push(e);

    let mut explicit = Scene::new("s");
    let mut e = Entity::new("lamp");
    e.add_component(Box::new(Light {
        kind: LightKind::Point,
        color: [1.0, 1.0, 1.0],
        intensity: 1.0,
        cone: [1.0, 1.0],
        range: 0.0,
    }));
    explicit.entities.push(e);

    assert_eq!(
        plain.to_var(),
        explicit.to_var(),
        "explicit defaults must not change the canonical document"
    );
    // the default light's document IS just the type tag
    let doc = plain.to_var();
    let comp = doc
        .get("entities")
        .get_idx(0)
        .get("components")
        .get_idx(0);
    assert_eq!(comp.get("type").as_str(), Some("light"));
    assert!(comp.as_object().unwrap().len() == 1, "expected no params");

    // identity round trip through FromVar
    let loaded = Scene::from_var(&doc).unwrap();
    assert_eq!(loaded.to_var(), doc);
}

/// Runtime-only components are skipped by the save — and a scene carrying
/// only them still round-trips byte-identically.
#[test]
fn runtime_only_components_are_skipped() {
    // a component with no scene_type is code, not data
    struct Opaque(u64);
    impl Component for Opaque {
        fn update(&mut self, _dt: f64) {
            self.0 += 1;
        }
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
            self
        }
    }

    let mut scene = Scene::new("runtime");
    let mut e = Entity::new("mover");
    e.add_component(Box::new(Opaque(0)));
    scene.entities.push(e);

    let doc = scene.to_var();
    let comps = doc.get("entities").get_idx(0).get("components");
    assert_eq!(
        comps.as_list().unwrap().len(),
        0,
        "runtime-only component skipped"
    );

    let loaded = Scene::from_var(&doc).unwrap();
    assert_eq!(loaded.to_var(), doc);

    // model-backed mesh renderers are runtime-only too (no asset pipeline yet)
    let mut scene = Scene::new("models");
    let mut e = Entity::new("meshy");
    e.add_component(Box::new(MeshRenderer::mesh(
        std::sync::Arc::new(hyg::game::Mesh::cube(Vec3::splat(0.5))),
        [1.0, 1.0, 1.0, 1.0],
    )));
    scene.entities.push(e);
    let doc = scene.to_var();
    let comps = doc.get("entities").get_idx(0).get("components");
    assert_eq!(comps.as_list().unwrap().len(), 0, "model renderer skipped");
}

/// User components round trip through the registry — register, load,
/// save, byte-identical.
#[test]
fn registry_round_trip() {
    register_counter();
    let mut scene = Scene::new("registry");
    let mut e = Entity::new("ticker");
    e.add_component(Box::new(Counter {
        start: 42,
        count: 0,
    }));
    scene.entities.push(e);

    let doc = scene.to_var();
    let comp = doc
        .get("entities")
        .get_idx(0)
        .get("components")
        .get_idx(0);
    assert_eq!(comp.get("type").as_str(), Some("counter"));
    assert_eq!(comp.get("start").as_i64(), Some(42));

    let loaded = Scene::from_var(&doc).unwrap();
    let counter = loaded.entities[0]
        .get_component::<Counter>()
        .expect("counter materialized");
    assert_eq!(counter.start, 42);
    assert_eq!(counter.count, 0, "fresh state, not live state");
    assert_eq!(loaded.to_var(), doc);
}

/// Duplicate registration replaces (the registry pattern), and the type
/// list is sorted.
#[test]
fn registry_replace_and_listing() {
    register_counter();
    register_counter();
    let types = scene::registered_component_types();
    assert!(types.windows(2).all(|w| w[0] <= w[1]), "unsorted: {types:?}");
    for expected in ["light", "mesh_renderer", "physics_body"] {
        assert!(types.iter().any(|t| t == expected), "{expected:?} missing");
    }

    let loaded_doc = doc(
        r#"{ "scene": "r", "format": 1, "entities": [
            { "name": "e", "components": [
                { "type": "counter", "start": 7 } ] } ] }"#,
    );
    let loaded = Scene::from_var(&loaded_doc).unwrap();
    let counter = loaded.entities[0].get_component::<Counter>().unwrap();
    assert_eq!(counter.start, 7);
}

/// The script component: a scene names a script FILE, the factory loads
/// it (base-dir resolution), and the save side writes the path back —
/// the body is the file's, never inlined into the document.
#[cfg(feature = "script")]
#[test]
fn script_round_trip_through_file() {
    use hyg::game::scripted::ScriptedComponent;

    let dir = scratch("script");
    std::fs::write(
        dir.join("patroller.js"),
        "function update(dt) { transform.position.x += dt; }",
    )
    .unwrap();

    let mut scene = Scene::new("scripted");
    let mut e = Entity::new("patrol");
    // build with the same path the document carries, so the save side
    // writes what the load side read
    e.add_component(Box::new(
        ScriptedComponent::new(
            hyg::script::Script::new(
                std::fs::read_to_string(dir.join("patroller.js")).unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .with_source_path("patroller.js"),
    ));
    scene.entities.push(e);

    let path = dir.join("scene.json.hyg");
    scene.save(&path).unwrap();
    let gen_a = std::fs::read(&path).unwrap();

    let loaded = Scene::load(&path).unwrap();
    let scripted = loaded.entities[0]
        .get_component::<ScriptedComponent>()
        .expect("scripted component materialized");
    assert_eq!(scripted.source_path(), Some("patroller.js"));

    loaded.save(&path).unwrap();
    assert_eq!(gen_a, std::fs::read(&path).unwrap(), "script scene drifted");

    // a script component with no source path is runtime-only
    let mut scene = Scene::new("inline");
    let mut e = Entity::new("s");
    e.add_component(Box::new(
        ScriptedComponent::new(hyg::script::Script::new("1 + 1").unwrap()).unwrap(),
    ));
    scene.entities.push(e);
    let doc = scene.to_var();
    let comps = doc.get("entities").get_idx(0).get("components");
    assert_eq!(comps.as_list().unwrap().len(), 0, "inline script skipped");

    std::fs::remove_dir_all(dir).ok();
}

/// The script factory: scene → file → component, including custom hook
/// names, and the failure voices (missing file, bad script).
#[cfg(feature = "script")]
#[test]
fn script_factory_loads_from_document() {
    use hyg::game::scripted::{HookNames, ScriptedComponent};

    let dir = scratch("script-factory");
    std::fs::write(dir.join("tick.js"), "function tick(dt) { }").unwrap();

    let script_doc = doc(
        r#"{ "type": "script", "source": "tick.js",
             "hooks": { "update": "tick" } }"#,
    );
    let ctx = scene::ComponentContext {
        entity_name: "e".to_string(),
        base: dir.clone(),
    };
    let component = scene::create_component("script", &script_doc, &ctx).unwrap();
    let scripted = component
        .as_any()
        .downcast_ref::<ScriptedComponent>()
        .unwrap();
    assert_eq!(
        scripted.hooks(),
        &HookNames {
            initialize: "initialize".to_string(),
            update: "tick".to_string(),
            draw: "draw".to_string(),
        }
    );

    // a missing file is a path-tracked error, not a panic
    let missing = doc(r#"{ "type": "script", "source": "nope.js" }"#);
    let err = scene::create_component("script", &missing, &ctx).err().expect("must fail");
    assert!(err.to_string().contains("source"), "{err}");
    assert!(err.to_string().contains("nope.js"), "{err}");

    // an unloadable script (parse error) is loud at load
    std::fs::write(dir.join("broken.js"), "function {").unwrap();
    let broken = doc(r#"{ "type": "script", "source": "broken.js" }"#);
    let err = scene::create_component("script", &broken, &ctx).err().expect("must fail");
    assert!(err.to_string().contains("failed to load"), "{err}");

    std::fs::remove_dir_all(dir).ok();
}

/// Scenes spawn into engines: camera attached, roots added in order,
/// components update headless.
#[test]
fn spawn_into_engine() {
    register_counter();
    let mut scene = demo_scene();
    scene.entities.push({
        let mut e = Entity::new("ticker");
        e.add_component(Box::new(Counter {
            start: 0,
            count: 0,
        }));
        e
    });

    let camera = scene.camera;
    let mut engine = hyg::game::Engine::new("acme", "scene-test");
    engine.frame_time(0.0);
    engine.set_physics(PhysicsWorld::none());
    scene.spawn_into(&mut engine);

    assert_eq!(engine.entities().len(), 2);
    assert!(engine.entity_mut("stack").is_some());
    assert_eq!(engine.camera(), &camera, "the scene camera attached");

    // two frames: components initialize and update (the disabled subtree
    // stays inert — physics runs under the `none` backend, transforms
    // untouched)
    engine.update(1.0 / 60.0);
    engine.update(1.0 / 60.0);
    let ticker = engine
        .entities()
        .iter()
        .find(|e| e.name() == "ticker")
        .unwrap()
        .get_component::<Counter>()
        .unwrap();
    assert_eq!(ticker.count, 2);
}

/// Native math values (`Var::Vec3`/`Var::Quat`) are accepted where the
/// document wants vectors — the in-memory authoring path — and the
/// canonical save writes number lists.
#[test]
fn native_math_values_accepted() {
    let native = Var::from(hyg::o! {
        "scene": "native",
        "format": Var::from(1i64),
        "camera": hyg::o! {
            "position": Var::from(Vec3::new(1.0, 2.0, 3.0)),
            "projection": hyg::o! { "ortho": hyg::o! { "half_extents":
                Var::from(Vec3::new(8.0, 4.5, 64.0)) } },
        },
        "entities": hyg::a![
            hyg::o! {
                "name": "e",
                "transform": hyg::o! {
                    "position": Var::from(Vec3::new(1.0, 2.0, 3.0)),
                    "rotation": Var::from(Quat::new(0.0, 0.0, 0.0, 1.0)),
                    "scale": Var::from(Vec3::new(2.0, 2.0, 2.0)),
                },
            }
        ],
    });
    let loaded = Scene::from_var(&native).unwrap();
    let t = loaded.entities[0].transform();
    assert_eq!(t.position, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(t.rotation, Quat::new(0.0, 0.0, 0.0, 1.0));
    assert_eq!(t.scale, Vec3::new(2.0, 2.0, 2.0));
    assert_eq!(
        loaded.camera.position,
        Vec3::new(1.0, 2.0, 3.0),
        "camera position read"
    );

    // the canonical save writes lists (the document encoding), not the
    // native variants
    let out = loaded.to_var();
    let pos = out
        .get("entities")
        .get_idx(0)
        .get("transform")
        .get("position");
    assert!(pos.as_list().is_some(), "expected a list, got {pos}");
    let reloaded = Scene::from_var(&out).unwrap();
    assert_eq!(reloaded.to_var(), out);
}

/// Unknown extra fields are tolerated on load, dropped on regeneration —
/// forward compatibility without silent schema drift.
#[test]
fn unknown_fields_tolerated_and_dropped() {
    let forward = doc(
        r##"{ "scene": "fwd", "format": 1, "editor_color": "#ff0044",
             "entities": [
                { "name": "e", "editor_pos": [12.0, 30.0],
                  "components": [ { "type": "light", "gizmo": true } ] } ] }"##,
    );
    let loaded = Scene::from_var(&forward).unwrap();
    let out = loaded.to_var();
    let text = serde_json::to_string(&out).unwrap();
    assert!(!text.contains("editor_color"), "{text}");
    assert!(!text.contains("editor_pos"), "{text}");
    assert!(!text.contains("gizmo"), "{text}");
    let reloaded = Scene::from_var(&out).unwrap();
    assert_eq!(reloaded.to_var(), out);
}

/// Empty scenes are valid and byte-stable.
#[test]
fn empty_scene_round_trip() {
    let scene = Scene::new("empty");
    let doc = scene.to_var();
    let loaded = Scene::from_var(&doc).unwrap();
    assert_eq!(loaded.to_var(), doc);
    assert!(loaded.entities.is_empty());
    assert_eq!(loaded.camera, Camera::default());
}

/// Non-finite floats never corrupt a scene silently: serde_json encodes
/// them as `null` on write, and the loader rejects the field loudly —
/// the save→load cycle fails with a path, not a silent zero.
#[test]
fn non_finite_floats_fail_loudly() {
    let dir = scratch("nan");
    let path = dir.join("broken.json.hyg");

    let mut scene = Scene::new("broken");
    let mut e = Entity::new("e");
    e.transform_mut().position = Vec3::new(f32::NAN, 0.0, 0.0);
    scene.entities.push(e);
    scene.save(&path).unwrap(); // the write lands (nulls in place of NaN)…

    // …and the load names the corrupted field
    let err = Scene::load(&path).err().expect("NaN scene must not reload");
    assert!(
        err.to_string().contains("entities.0.transform.position"),
        "{err}"
    );
    assert!(err.to_string().contains("expected a number"), "{err}");

    // infinity likewise
    let mut scene = Scene::new("broken");
    let mut e = Entity::new("e");
    e.transform_mut().scale = Vec3::new(f32::INFINITY, 1.0, 1.0);
    scene.entities.push(e);
    scene.save(&path).unwrap();
    assert!(Scene::load(&path).is_err());

    std::fs::remove_dir_all(dir).ok();
}

/// The f32 shortest-decimal hop: precision-hostile values round trip
/// byte-identically (the save prints the f32's shortest decimal; the
/// load parses it back onto the same f32 through the f64 hop).
#[test]
fn precision_hostile_values_round_trip() {
    let mut scene = Scene::new("precision");
    let mut e = Entity::new("e");
    e.transform_mut().position = Vec3::new(
        0.1f32 + 0.2f32, // 0.30000001192092896 — shortest repr "0.3"
        8.89,            // the camera default's trap value
        -1e-45,          // smallest f32 subnormal
    );
    scene.entities.push(e);

    let saved = scene.to_var();
    let loaded = Scene::from_var(&saved).unwrap();
    let pos = loaded.entities[0].transform().position;
    assert_eq!(pos.x, 0.1f32 + 0.2f32);
    assert_eq!(pos.y, 8.89f32);
    assert_eq!(pos.z, -1e-45f32);
    assert_eq!(loaded.to_var(), saved);
}

/// A loaded scene resolves file references against its file's directory
/// ([`Scene::load`]); a bare [`Scene::from_var`] uses the working
/// directory.
#[test]
fn load_resolves_against_scene_dir() {
    let dir = scratch("base-dir");
    let scenes = dir.join("scenes");
    std::fs::create_dir_all(&scenes).unwrap();
    std::fs::write(
        scenes.join("s.json.hyg"),
        br#"{
  "scene": "base-dir",
  "format": 1,
  "entities": [
    { "name": "e", "components": [ { "type": "light", "intensity": 3 } ] }
  ]
}"#,
    )
    .unwrap();

    // the same document loads through both seams
    let text = std::fs::read_to_string(scenes.join("s.json.hyg")).unwrap();
    Scene::from_var(&doc(&text)).unwrap();
    let scene = Scene::load(&scenes.join("s.json.hyg")).unwrap();
    assert_eq!(scene.name, "base-dir");
    let light = scene.entities[0].get_component::<Light>().unwrap();
    assert_eq!(light.intensity, 3.0);

    std::fs::remove_dir_all(dir).ok();
}
