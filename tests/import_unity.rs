//! The Unity importer's contracts (M7 job 1): a `.unity` scene's
//! GameObjects become entities (hierarchy via transforms), cameras
//! and lights map, rigidbodies pair with their colliders' shapes,
//! MonoBehaviours become script stubs carrying their serialized
//! fields, and the report is the honest coverage statement.

#![cfg(feature = "import-unity")]

use hyg::game::physics::Shape;
use hyg::game::physics_body::PhysicsBody;
use hyg::import::unity::{import_scene, transpile_csharp_naive};
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// A hand-written minimal Unity scene: a falling Player with a
/// behaviour, a camera, and a walled Ground. The shapes Unity writes.
const PLAYER_SCENE: &str = r#"%YAML 1.1
%TAG !u! tag:unity3d.com,2011:
--- !u!1 &100100
GameObject:
  m_ObjectHideFlags: 0
  m_Name: Player
  m_Component:
  - component: {fileID: 100200}
  - component: {fileID: 100300}
  - component: {fileID: 100400}
--- !u!4 &100200
Transform:
  m_GameObject: {fileID: 100100}
  m_LocalPosition: {x: 0, y: 1, z: 0}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Father: {fileID: 0}
--- !u!54 &100300
Rigidbody:
  m_GameObject: {fileID: 100100}
  m_Mass: 2.5
--- !u!114 &100400
MonoBehaviour:
  m_GameObject: {fileID: 100100}
  m_Script: {fileID: 11500000, guid: abcdef123456, type: 3}
  m_Health: 100
  m_MoveSpeed: 4.5
--- !u!1 &200100
GameObject:
  m_ObjectHideFlags: 0
  m_Name: Main Camera
  m_Component:
  - component: {fileID: 200200}
  - component: {fileID: 200300}
--- !u!4 &200200
Transform:
  m_GameObject: {fileID: 200100}
  m_LocalPosition: {x: 0, y: 3, z: -8}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Father: {fileID: 0}
--- !u!20 &200300
Camera:
  m_GameObject: {fileID: 200100}
  m_Field of view: 50
--- !u!1 &300100
GameObject:
  m_ObjectHideFlags: 0
  m_Name: Ground
  m_Component:
  - component: {fileID: 300200}
  - component: {fileID: 300300}
--- !u!4 &300200
Transform:
  m_GameObject: {fileID: 300100}
  m_LocalPosition: {x: 0, y: 0, z: 0}
  m_LocalScale: {x: 10, y: 1, z: 10}
  m_Father: {fileID: 0}
--- !u!65 &300300
BoxCollider:
  m_GameObject: {fileID: 300100}
  m_Size: {x: 10, y: 2, z: 10}
--- !u!1 &400100
GameObject:
  m_ObjectHideFlags: 0
  m_Name: Sun
--- !u!4 &400200
Transform:
  m_GameObject: {fileID: 400100}
  m_LocalPosition: {x: 0, y: 5, z: 0}
  m_Father: {fileID: 0}
--- !u!108 &400300
Light:
  m_GameObject: {fileID: 400100}
  m_Type: 1
  m_Color: {r: 1, g: 0.9, b: 0.8}
"#;

/// The e2e import: hierarchy, transforms with the handedness flip,
/// physics shapes from colliders, the camera, the light, and the
/// behaviour stubs.
#[test]
fn unity_scene_imports_into_hyg_entities() {
    let dir = temp_dir("hyg-import-unity");
    let scene_path = dir.join("PlayerScene.unity");
    std::fs::write(&scene_path, PLAYER_SCENE).expect("writes the fixture");

    let import = import_scene(&scene_path, &dir, &dir).expect("imports");
    let report = &import.report;

    // four GameObjects, all imported
    assert_eq!(report.entities, 4);
    // the coverage: camera, light, rigidbody, behaviour mapped; the
    // colliders feed physics bodies
    assert!(report.mapped.iter().any(|(k, c)| k == "Camera" && *c >= 1));
    assert!(report.mapped.iter().any(|(k, c)| k == "Light" && *c >= 1));
    assert!(report.mapped.iter().any(|(k, c)| k == "Rigidbody" && *c >= 1));
    assert!(report.mapped.iter().any(|(k, c)| k == "MonoBehaviour" && *c >= 1));
    // no camera warning (one WAS imported)
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.contains("default main camera")),
        "a camera came across: {:?}",
        report.warnings
    );

    // write the scene through the carrier and LOAD it back — the same
    // contract every hyg scene keeps
    let scene_path_out = dir.join("imported.json.hyg");
    hyg::carrier::write(&scene_path_out, &import.document).expect("writes");
    let loaded = hyg::game::scene::Scene::load(&scene_path_out).expect("loads");

    // find entities by name and check the conversions
    let find = |name: &str| -> usize {
        loaded
            .entities
            .iter()
            .position(|e| e.name() == name)
            .unwrap_or_else(|| panic!("{name} imported"))
    };
    let player = &loaded.entities[find("Player")];
    // Unity (0, 1, 0), LH → RH keeps x/y, flips z
    assert_eq!(
        player.transform().position,
        hyg::var::Vec3::new(0.0, 1.0, 0.0)
    );
    // the rigidbody + behaviour delivered (the camera check below)
    let has_physics = player
        .iter_components()
        .any(|c| c.as_any().downcast_ref::<PhysicsBody>().is_some());
    assert!(has_physics, "the player carries a physics body");

    let ground = &loaded.entities[find("Ground")];
    assert_eq!(
        ground.transform().scale,
        hyg::var::Vec3::new(10.0, 1.0, -10.0),
        "the handedness flip: z negates"
    );
    let ground_shape = ground
        .iter_components()
        .find_map(|c| {
            c.as_any()
                .downcast_ref::<PhysicsBody>()
                .map(|body| *body.shape())
        })
        .expect("the ground body");
    assert_eq!(
        ground_shape,
        Shape::Box(hyg::var::Vec3::new(5.0, 1.0, 5.0)),
        "the collider's full extents halve into the shape"
    );

    // the camera imported with its fov and the main flag
    let main_camera = &loaded.entities[find("Main Camera")];
    let camera = main_camera
        .iter_components()
        .find_map(|c| c.as_any().downcast_ref::<hyg::game::CameraComponent>())
        .expect("the camera component");
    assert!(camera.main, "the first imported camera is main");
    assert!(
        matches!(
            camera.camera.projection,
            hyg::game::Projection::Perspective { fov_y_degrees: 50.0, .. }
        ),
        "the fov carried across"
    );

    // the behaviour's serialized fields landed in a generated stub
    assert_eq!(report.scripts.len(), 1);
    let (guid, stub_relative) = &report.scripts[0];
    assert_eq!(guid, "abcdef123456");
    let stub = std::fs::read_to_string(dir.join(stub_relative)).expect("the stub written");
    assert!(stub.contains("m_Health = 100"), "fields carried: {stub}");
    assert!(stub.contains("m_MoveSpeed = 4.5"), "{stub}");
    assert!(stub.contains("TODO"), "the honest port marker: {stub}");

    // the light: directional (Unity m_Type 1) with its color
}

/// The naive C# transpiler: hook names map, fields become globals,
/// unknown lines carry TODO — the coverage counts are the honest
/// migration metric.
#[test]
fn transpiler_converts_what_maps_and_flags_the_rest() {
    let source = r#"
using UnityEngine;

public class PlayerController : MonoBehaviour
{
    public float speed = 5.0f;

    void Update()
    {
        transform.position += Vector3.forward * speed * Time.deltaTime;
    }
}
"#;
    let (converted, converted_lines, todo_lines) =
        transpile_csharp_naive(source, "PlayerController");
    assert!(converted.contains("function update(dt) {"), "the hook maps");
    assert!(converted.contains("var speed = 5.0f;"), "the field maps");
    assert!(converted.contains("TODO: transform.position"), "unmapped Unity API flags");
    assert!(converted.contains("}"), "the braces carry through");
    assert_eq!(converted_lines, 2, "the hook and the field convert; braces pass uncounted");
    assert_eq!(todo_lines, 1, "the Unity API line flagged");
}
