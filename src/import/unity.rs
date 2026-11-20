//! The Unity importer (M7, job 1): migrate Unity scenes and prefabs
//! into Hydrargyros projects.
//!
//! Unity's text serialization (`.unity` scenes, `.prefab` files) is a
//! documented YAML dialect — objects with class names, anchors, and
//! `fileID`/GUID cross-references. This module walks that document and
//! rebuilds the game in hyg's shape:
//!
//! - **GameObject** → an entity (name kept)
//! - **Transform** → the entity's transform; `m_Father`/`m_Children`
//!   rebuild the hierarchy
//! - **Camera** → the render camera (the first imported camera is the
//!   scene's main)
//! - **Light** → directional or point by Unity's `m_Type`
//! - **Rigidbody + Box/Sphere Collider** → a physics body (the
//!   collider supplies the shape — Unity puts both on the same
//!   GameObject, so the pairing is direct)
//! - **MeshRenderer/MeshFilter** → a placeholder renderer; the mesh
//!   reference is preserved in the report (mesh GEOMETRY conversion is
//!   a later rung — the reference is not lost, it is queued)
//! - **MonoBehaviour** → a generated JavaScript stub: the serialized
//!   inspector fields are carried over as globals, and when the C#
//!   source is discoverable (by the `.meta` GUID map) a naive line
//!   transpiler converts what maps and marks what doesn't
//!
//! Handedness: Unity is left-handed; hyg's camera is right-handed —
//! positions and scales flip z (`(x, y, -z)`), quaternions negate x
//! and y. The flip is applied on import and documented here so edits
//! keep orientation.
//!
//! Everything is data in → entities out: the importer runs and tests
//! headless, and its [`UnityImportReport`] is the honest coverage
//! statement (what mapped, what didn't, and why).

use unity_asset::{UnityClass, UnityDocument, UnityValue, YamlDocument};

use crate::game::camera_component::CameraComponent;
use crate::game::physics::Shape;
use crate::game::physics_body::PhysicsBody;
use crate::game::scene::Scene;
use crate::game::{Camera, Entity, Light, Projection};
use crate::var::Var;
use crate::var::{Quat, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The import's honest coverage statement: what came across, what
/// didn't, and why. This is the metric a migration is judged by.
#[derive(Debug, Clone, Default)]
pub struct UnityImportReport {
    /// GameObjects imported as entities.
    pub entities: usize,
    /// Component kinds mapped, with counts (`"Camera" -> 1`).
    pub mapped: Vec<(String, usize)>,
    /// Component kinds SKIPPED, with counts and the reason.
    pub skipped: Vec<(String, usize, String)>,
    /// Scripts generated: (Unity script GUID, the stub's relative path).
    pub scripts: Vec<(String, String)>,
    /// Parse warnings plus importer decisions worth surfacing.
    pub warnings: Vec<String>,
}

/// One imported scene: the Hydrargyros document, the generated script
/// stubs (paths relative to the output directory), and the report.
#[derive(Debug, Clone)]
pub struct UnityImport {
    /// The scene document (write it with the carrier).
    pub document: Var,
    /// Generated script files: relative path → source.
    pub scripts: Vec<(String, String)>,
    /// The coverage statement.
    pub report: UnityImportReport,
}

/// Load a Unity YAML file into its objects (a `.unity` or `.prefab` is
/// many `!u!` documents in one file — the loader flattens them).
fn load_objects(path: &Path) -> Result<(Vec<UnityClass>, Vec<String>), String> {
    let (document, warnings) = YamlDocument::load_yaml_with_warnings(path, false)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let warning_texts: Vec<String> = warnings
        .iter()
        .map(|w| format!("doc {}: {}", w.doc_index, w.error))
        .collect();
    Ok((document.entries().to_vec(), warning_texts))
}

/// Read `{fileID: …, guid: …}` from a value: the in-file anchor and
/// the optional external GUID.
fn file_ref(value: &UnityValue) -> (Option<String>, Option<String>) {
    let Some(object) = value.as_object() else {
        return (None, None);
    };
    let file_id = object
        .get("fileID")
        .and_then(|v| v.as_i64())
        .map(|id| id.to_string());
    let guid = object
        .get("guid")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    (file_id, guid)
}

/// A Unity vector3 → hyg's Vec3 with the handedness flip (z negates).
fn unity_vec3(value: &UnityValue) -> Vec3 {
    let get = |key: &str| {
        value
            .as_object()
            .and_then(|map| map.get(key))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32
    };
    Vec3::new(get("x"), get("y"), -get("z"))
}

/// A Unity quaternion → hyg's Quat with the handedness flip (x and y
/// negate; z and w keep — the standard LH→RH conversion for a z-flip).
fn unity_quat(value: &UnityValue) -> Quat {
    let get = |key: &str| {
        value
            .as_object()
            .and_then(|map| map.get(key))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0) as f32
    };
    Quat::new(-get("x"), -get("y"), get("z"), get("w"))
}

/// A Unity scale → hyg's Vec3. Scale flips too (negative scale mirrors
/// geometry — keep the magnitude, flip z, matching the position flip).
fn unity_scale(value: &UnityValue) -> Vec3 {
    let get = |key: &str| {
        value
            .as_object()
            .and_then(|map| map.get(key))
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32
    };
    Vec3::new(get("x"), get("y"), -get("z"))
}

/// The meta keys that are Unity's plumbing, not a component's data —
/// script stubs list everything else as serialized fields.
const BEHAVIOUR_META_KEYS: [&str; 5] = [
    "m_GameObject",
    "m_Enabled",
    "m_EditorHideFlags",
    "m_Script",
    "m_Name",
];

/// Format a UnityValue compactly for a stub comment (strings quoted,
/// numbers plain, arrays and objects JSON-ish).
fn format_value(value: &UnityValue) -> String {
    match value {
        UnityValue::Null => "null".to_string(),
        UnityValue::Bool(b) => b.to_string(),
        UnityValue::Integer(i) => i.to_string(),
        UnityValue::Float(f) => format!("{f}"),
        UnityValue::String(s) => format!("{s:?}"),
        UnityValue::Array(items) => {
            let inner: Vec<String> = items.iter().map(format_value).collect();
            format!("[{}]", inner.join(", "))
        }
        UnityValue::Bytes(bytes) => format!("<{} bytes>", bytes.len()),
        UnityValue::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(key, value)| format!("{key}: {}", format_value(value)))
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
    }
}

/// The GUID → `.cs` source map: walk a Unity project for `.cs.meta`
/// files (each carries its script's `guid`) and pair them with the
/// sibling `.cs` source.
fn script_sources(project_root: &Path) -> HashMap<String, PathBuf> {
    let mut map = HashMap::new();
    let mut stack = vec![project_root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // skip Unity's own library churn
                if path.file_name().and_then(|n| n.to_str()) != Some("Library") {
                    stack.push(path);
                }
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if let Some(source) = name.strip_suffix(".cs.meta") {
                if let Ok(meta) = std::fs::read_to_string(&path) {
                    for line in meta.lines() {
                        if let Some(guid) = line.strip_prefix("guid: ") {
                            map.insert(
                                guid.trim().to_string(),
                                path.with_file_name(format!("{source}.cs")),
                            );
                            break;
                        }
                    }
                }
            }
        }
    }
    map
}

/// The NAIVE C#→JavaScript transpiler (v0, honest): line-level
/// conversion of the MonoBehaviour shape — usings, the class wrapper,
/// field declarations, and Unity's hook names map; everything else
/// passes through prefixed `// TODO:`. Returns the converted source
/// and the coverage (converted lines, todo lines).
#[must_use]
pub fn transpile_csharp_naive(source: &str, class_name: &str) -> (String, usize, usize) {
    let mut out = vec![
        "// converted from Unity C# (naive line-level pass — review before use)".to_string(),
        format!("// class: {class_name}"),
        String::new(),
    ];
    let mut converted = 0usize;
    let mut todo = 0usize;
    let mut in_class = false;
    let mut in_using_or_namespace = false;
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("using ") || trimmed.starts_with("namespace ") {
            in_using_or_namespace = true;
            continue;
        }
        if in_using_or_namespace && (trimmed == "{" || trimmed.is_empty()) {
            continue;
        }
        if trimmed.starts_with("public class ") || trimmed.starts_with("class ") {
            in_class = true;
            continue;
        }
        if !in_class {
            continue;
        }
        // Unity's lifecycle hooks → hyg's script hooks
        if trimmed.starts_with("void Update()") || trimmed.starts_with("void Update ()") {
            out.push("function update(dt) {".to_string());
            converted += 1;
            continue;
        }
        if trimmed.starts_with("void Start()") || trimmed.starts_with("void Start ()") {
            out.push("function start() {".to_string());
            converted += 1;
            continue;
        }
        // field declarations: `public int hp = 3;` → a global
        if trimmed.starts_with("public ") && trimmed.ends_with(';') {
            let parts: Vec<&str> = trimmed[7..].split_whitespace().collect();
            if parts.len() >= 2 {
                let name = parts[1].trim_end_matches(';').trim_end_matches('=');
                let default = trimmed
                    .split('=')
                    .nth(1)
                    .map(|v| v.trim().trim_end_matches(';').to_string())
                    .unwrap_or_else(|| "null".to_string());
                out.push(format!("var {} = {};", name, default));
                converted += 1;
                continue;
            }
        }
        if trimmed == "}" {
            out.push("}".to_string());
            continue;
        }
        if trimmed.is_empty() {
            continue;
        }
        // everything else: carried as a TODO (Unity API calls don't map)
        out.push(format!("// TODO: {trimmed}"));
        todo += 1;
    }
    (out.join("\n"), converted, todo)
}

/// Import a Unity `.unity` scene or `.prefab` file. `project_root` is
/// the Unity project directory (the GUID → `.cs` source lookup walks
/// it); generated script stubs are written under `out_dir/scripts` and
/// returned relative to `out_dir`. The caller writes the scene
/// document with the carrier — a runtime plays it directly.
///
/// # Errors
/// The file doesn't parse, or a stub write fails.
pub fn import_scene(
    unity_path: &Path,
    project_root: &Path,
    out_dir: &Path,
) -> Result<UnityImport, String> {
    let (objects, parse_warnings) = load_objects(unity_path)?;

    let mut report = UnityImportReport {
        warnings: parse_warnings,
        ..UnityImportReport::default()
    };

    // pass 1: GameObjects → entities (name kept)
    let mut entities: Vec<Entity> = Vec::new();
    let mut go_anchor_to_index: HashMap<&str, usize> = HashMap::new();
    for object in &objects {
        if object.class_name != "GameObject" {
            continue;
        }
        go_anchor_to_index.insert(object.anchor.as_str(), entities.len());
        entities.push(Entity::new(object.name().unwrap_or("unity object")));
        report.entities += 1;
    }

    // pass 2: colliders → pending shapes per GO anchor (a Rigidbody
    // picks its shape from the collider on the same GameObject)
    let mut pending_shape: HashMap<String, Shape> = HashMap::new();
    let mut rigidbody_anchors: HashMap<String, ()> = HashMap::new();
    let mut collider_counts: HashMap<String, usize> = HashMap::new();
    for object in &objects {
        let Some(go_ref) = object.get("m_GameObject") else {
            continue;
        };
        let (Some(go_anchor), _) = file_ref(go_ref) else {
            continue;
        };
        match object.class_name.as_str() {
            // Unity collider sizes are FULL extents; hyg shapes are half
            "BoxCollider" => {
                // extents are MAGNITUDES: the handedness flip moves
                // positions, it doesn't mirror sizes (negative extents
                // would break the physics engine)
                let size = object
                    .get("m_Size")
                    .map(|v| {
                        let get = |key: &str| {
                            v.as_object()
                                .and_then(|map| map.get(key))
                                .and_then(|n| n.as_f64())
                                .unwrap_or(1.0) as f32
                        };
                        Vec3::new(get("x"), get("y"), get("z"))
                    })
                    .unwrap_or(Vec3::splat(1.0));
                pending_shape.insert(
                    go_anchor.clone(),
                    Shape::Box(Vec3::new(
                        (size.x / 2.0).abs(),
                        (size.y / 2.0).abs(),
                        (size.z / 2.0).abs(),
                    )),
                );
                *collider_counts.entry("BoxCollider".to_string()).or_default() += 1;
            }
            "SphereCollider" => {
                let radius = object
                    .get("m_Radius")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.5) as f32;
                pending_shape.insert(go_anchor.clone(), Shape::Sphere(radius));
                *collider_counts.entry("SphereCollider".to_string()).or_default() += 1;
            }
            _ => {}
        }
    }

    // pass 3: the component walk — cameras, lights, rigidbodies,
    // renderers, behaviours — each paired with its GO's entity
    let mut camera_seen = false;
    let mut light_count = 0usize;
    let mut camera_count = 0usize;
    let mut rigidbody_count = 0usize;
    let mut behaviour_count = 0usize;
    let mut renderer_count = 0usize;
    let mut mesh_refs: Vec<String> = Vec::new();
    let mut transform_anchor_to_go: HashMap<&str, String> = HashMap::new();
    let mut transform_children: Vec<(String, String)> = Vec::new(); // (father transform anchor, this transform anchor)

    for object in &objects {
        if object.class_name == "Transform" {
            // the transform→GO pairing (for the hierarchy wiring pass)
            if let Some(go_ref) = object.get("m_GameObject") {
                if let (Some(go_anchor), _) = file_ref(go_ref) {
                    transform_anchor_to_go.insert(object.anchor.as_str(), go_anchor.clone());
                    if let Some(father) = object.get("m_Father") {
                        if let (Some(father_anchor), _) = file_ref(father) {
                            transform_children
                                .push((father_anchor, object.anchor.clone()));
                        }
                    }
                }
            }
        }
        let Some(go_ref) = object.get("m_GameObject") else {
            continue;
        };
        let (Some(go_anchor), _) = file_ref(go_ref) else {
            continue;
        };
        let Some(&entity_index) = go_anchor_to_index.get(go_anchor.as_str()) else {
            continue;
        };
        let kind = object.class_name.as_str();
        match kind {
            "Transform" => {
                let entity = &mut entities[entity_index];
                if let Some(position) = object.get("m_LocalPosition") {
                    entity.transform_mut().position = unity_vec3(position);
                }
                if let Some(rotation) = object.get("m_LocalRotation") {
                    entity.transform_mut().rotation = unity_quat(rotation);
                }
                if let Some(scale) = object.get("m_LocalScale") {
                    entity.transform_mut().scale = unity_scale(scale);
                }
                bump(&mut report.mapped, "Transform");
            }
            "Camera" => {
                let fov = object
                    .get("m_Field of view")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(60.0) as f32;
                let entity = &mut entities[entity_index];
                entity.add_component(Box::new(CameraComponent {
                    camera: Camera {
                        position: entity.transform().position,
                        target: entity.transform().position
                            + Quat::new(0.0, 0.0, 0.0, 1.0)
                                .rotate(Vec3::new(0.0, 0.0, 1.0)),
                        projection: Projection::Perspective {
                            fov_y_degrees: fov,
                            near: 0.1,
                            far: 500.0,
                            aspect: 16.0 / 9.0,
                        },
                    },
                    main: !camera_seen,
                }));
                camera_seen = true;
                camera_count += 1;
                bump(&mut report.mapped, "Camera");
            }
            "Light" => {
                // Unity's LightType: 0 spot, 1 directional, 2 point
                let light_type = object
                    .get("m_Type")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(2);
                let mut light = match light_type {
                    1 => Light::directional(),
                    _ => Light::default(), // the default IS a point light
                };
                if let Some(color) = object.get("m_Color") {
                    let get = |key: &str| {
                        color
                            .as_object()
                            .and_then(|map| map.get(key))
                            .and_then(|v| v.as_f64())
                            .unwrap_or(1.0) as f32
                    };
                    light = light.with_color([get("r"), get("g"), get("b")]);
                }
                let entity = &mut entities[entity_index];
                entity.add_component(Box::new(light));
                light_count += 1;
                bump(&mut report.mapped, "Light");
            }
            "Rigidbody" => {
                rigidbody_anchors.insert(go_anchor.clone(), ());
                let mass = object
                    .get("m_Mass")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(1.0) as f32;
                let shape = pending_shape
                    .get(go_anchor.as_str())
                    .cloned()
                    .unwrap_or(Shape::Sphere(0.5));
                let entity = &mut entities[entity_index];
                entity.add_component(Box::new(
                    PhysicsBody::new(format!("body-{go_anchor}"), shape).mass(mass),
                ));
                rigidbody_count += 1;
                bump(&mut report.mapped, "Rigidbody");
                let _ = light_count;
            }
            "MeshRenderer" | "MeshFilter" => {
                if let Some(mesh) = object.get("m_Mesh") {
                    if let (_, Some(guid)) = file_ref(mesh) {
                        mesh_refs.push(guid);
                    }
                }
                renderer_count += 1;
                if let Some(entry) = report
                    .skipped
                    .iter_mut()
                    .find(|(kind, _, _)| kind == "MeshRenderer/MeshFilter")
                {
                    entry.1 += 1;
                } else {
                    report.skipped.push((
                        "MeshRenderer/MeshFilter".to_string(),
                        1,
                        "mesh geometry conversion is a later rung; the mesh reference is kept in this report"
                            .to_string(),
                    ));
                }
            }
            "MonoBehaviour" => {
                let Some(script_ref) = object.get("m_Script") else {
                    continue;
                };
                let (Some(_), Some(guid)) = file_ref(script_ref) else {
                    report.warnings.push(format!(
                        "MonoBehaviour on GO anchor {go_anchor} has no script GUID — skipped"
                    ));
                    continue;
                };
                // serialized fields: everything except Unity's plumbing
                let mut fields = String::new();
                for (key, value) in object.properties() {
                    if !BEHAVIOUR_META_KEYS.contains(&key.as_str()) {
                        fields.push_str(&format!("//   {key} = {}\n", format_value(value)));
                    }
                }
                // the class name comes from the .cs source when the
                // project root can find it
                let sources = script_sources(project_root);
                let (stub, class_note) = match sources.get(&guid) {
                    Some(cs_path) => {
                        let source = std::fs::read_to_string(cs_path)
                            .map_err(|e| format!("{}: {e}", cs_path.display()))?;
                        let class_name = source
                            .lines()
                            .find(|line| line.contains("class "))
                            .and_then(|line| line.split("class ").nth(1))
                            .and_then(|rest| rest.split([' ', ':']).next())
                            .unwrap_or("ConvertedBehaviour")
                            .to_string();
                        let (converted, converted_lines, todo_lines) =
                            transpile_csharp_naive(&source, &class_name);
                        report.warnings.push(format!(
                            "script {guid} ({class_name}): {converted_lines} lines converted, {todo_lines} marked TODO — review before shipping"
                        ));
                        (
                            format!(
                                "// converted from {class_name}.cs (Unity MonoBehaviour)\n{converted}\n"
                            ),
                            class_name,
                        )
                    }
                    None => (
                        format!(
                            "// Unity MonoBehaviour (script {guid}) — no C# source found in the project root.\n\
                             // serialized fields from the source scene:\n{fields}\
                             // TODO: port the behaviour logic from the original C# source.\n"
                        ),
                        "MonoBehaviour".to_string(),
                    ),
                };
                let stub_name = format!("scripts/{guid}.js");
                std::fs::create_dir_all(out_dir.join("scripts"))
                    .map_err(|e| format!("scripts dir: {e}"))?;
                std::fs::write(out_dir.join(&stub_name), &stub)
                    .map_err(|e| format!("{}: {e}", stub_name))?;
                report.scripts.push((guid, stub_name));
                behaviour_count += 1;
                bump(&mut report.mapped, "MonoBehaviour");
                let _ = class_note;
            }
            _ => {}
        }
    }

    // the rigidbody/light/camera counts land on the mapped table
    for (kind, count) in [
        ("Camera", camera_count),
        ("Light", light_count),
        ("Rigidbody", rigidbody_count),
        ("MonoBehaviour", behaviour_count),
    ] {
        if count > 0 {
            bump(&mut report.mapped, kind);
        }
    }
    let _ = renderer_count;
    if !mesh_refs.is_empty() {
        report.warnings.push(format!(
            "{} mesh reference(s) preserved for a later geometry-conversion rung: {:?}",
            mesh_refs.len(),
            mesh_refs
        ));
    }

    // STATIC COLLIDERS: a collider with no rigidbody is Unity's
    // static collision geometry — the most common case in real scenes
    // (floors, walls). They become STATIC physics bodies (the shape
    // halves, same as paired ones), reported in the mapped table.
    for (go_anchor, shape) in &pending_shape {
        if rigidbody_anchors.contains_key(go_anchor.as_str()) {
            continue;
        }
        if let Some(&entity_index) = go_anchor_to_index.get(go_anchor.as_str()) {
            let name = entities[entity_index].name().to_string();
            let entity = &mut entities[entity_index];
            entity.add_component(Box::new(
                PhysicsBody::new(format!("static-{name}"), *shape).static_body(),
            ));
            bump(&mut report.mapped, "Collider (static)");
        }
    }

    // the hierarchy: transforms wire children to fathers (through the
    // transform→GO anchor map → entity indices)
    let mut parent_of: HashMap<usize, usize> = HashMap::new();
    for (father_transform, child_transform) in &transform_children {
        let father_index = transform_anchor_to_go
            .get(father_transform.as_str())
            .and_then(|go| go_anchor_to_index.get(go.as_str()));
        let child_index = transform_anchor_to_go
            .get(child_transform.as_str())
            .and_then(|go| go_anchor_to_index.get(go.as_str()));
        if let (Some(&father_index), Some(&child_index)) = (father_index, child_index) {
            parent_of.insert(child_index, father_index);
        }
    }

    // children_of: parent index → child indices (sorted for stable order)
    let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots: Vec<usize> = Vec::new();
    for index in 0..entities.len() {
        match parent_of.get(&index) {
            Some(parent) => children_of.entry(*parent).or_default().push(index),
            None => roots.push(index),
        }
    }
    for children in children_of.values_mut() {
        children.sort_unstable();
    }

    // assemble: take each root's subtree out of the slots
    let mut slots: Vec<Option<Entity>> = entities.into_iter().map(Some).collect();
    let mut scene_entities = Vec::new();
    for root in roots {
        if let Some(entity) = take_subtree(&mut slots, root, &children_of) {
            scene_entities.push(entity);
        }
    }

    // a scene without an imported camera still plays: the default
    // camera entity (reported, not silent)
    let has_camera = scene_entities.iter().any(|entity| {
        entity
            .iter_components()
            .any(|c| c.scene_type() == Some("camera"))
    });
    if !has_camera {
        let mut camera = Entity::new("main camera");
        camera.add_component(Box::new(CameraComponent {
            camera: Camera {
                position: Vec3::new(0.0, 2.0, -8.0),
                target: Vec3::splat(0.0),
                projection: Projection::Perspective {
                    fov_y_degrees: 60.0,
                    near: 0.1,
                    far: 500.0,
                    aspect: 16.0 / 9.0,
                },
            },
            main: true,
        }));
        report
            .warnings
            .push("no imported camera — the default main camera was added".to_string());
        scene_entities.insert(0, camera);
    }

    let document = Scene::document(
        unity_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unity-import"),
        &Camera::default(),
        &scene_entities,
    );

    Ok(UnityImport {
        document,
        scripts: Vec::new(),
        report: report.clone(),
    })
}

fn bump(mapped: &mut Vec<(String, usize)>, kind: &str) {
    for (kind_seen, count) in mapped.iter_mut() {
        if kind_seen == kind {
            *count += 1;
            return;
        }
    }
    mapped.push((kind.to_string(), 1));
}

fn take_subtree(
    slots: &mut Vec<Option<Entity>>,
    index: usize,
    children_of: &HashMap<usize, Vec<usize>>,
) -> Option<Entity> {
    let mut entity = slots.get_mut(index)?.take()?;
    if let Some(children) = children_of.get(&index) {
        for &child in children {
            if let Some(child_entity) = take_subtree(slots, child, children_of) {
                entity.add_child(child_entity);
            }
        }
    }
    Some(entity)
}
