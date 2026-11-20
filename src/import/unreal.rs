//! The Unreal importer (M7, job 2): migrate UE5 levels into Hydrargyros
//! projects.
//!
//! UE5's World Partition changes the shape of a level: the `.umap`
//! itself holds almost no actors — it declares a World Partition grid,
//! and every ACTOR lives in its own `.uasset` under
//! `Content/__ExternalActors__/<level-path>/<hash>/`. This importer:
//!
//! 1. parses the `.umap` (via the `rust-uasset` crate — UE5-capable)
//!    and detects the partition (reported: a World Partition level's
//!    actors come from step 2, not the map body),
//! 2. locates the level's external-actor directory (the `.umap`'s
//!    path under `Content/` mirrors the `__ExternalActors__` layout),
//! 3. parses every actor file and rebuilds it as an entity: the Actor
//!    export gives the name/label, its RootComponent gives the
//!    transform, and the file's other component exports map to hyg
//!    components where they have counterparts (lights, cameras,
//!    static-mesh placeholders) — everything else is counted in the
//!    report as a skipped component reference with its class name.
//!
//! Units and handedness: Unreal is centimeters and left-handed; hyg
//! runs meters and right-handed — positions divide by 100 and flip z,
//! rotations negate pitch/yaw, scales keep magnitude and flip z. The
//! conversion is applied on import and documented here.
//!
//! The JSON walk is pure (serde_json values in, entities out) so the
//! contracts test against synthetic documents without binary
//! fixtures; the fixture-dependent test runs when `HYG_UE_PROJECT`
//! points at a real project.

use crate::game::camera_component::CameraComponent;
use crate::game::physics::Shape;
use crate::game::physics_body::PhysicsBody;
use crate::game::renderable::MeshRenderer;
use crate::game::scene::Scene;
use crate::game::{Camera, Entity, Light, Projection};
use crate::var::{Quat, Var, Vec3};
use rust_uasset::Asset;
use std::path::{Path, PathBuf};

/// UE centimeters → hyg meters, with the LH→RH z flip.
fn unreal_location(value: &serde_json::Value) -> Vec3 {
    let get = |key: &str| value.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    Vec3::new(get("X") / 100.0, get("Y") / 100.0, -get("Z") / 100.0)
}

/// A UE rotator (pitch/yaw/roll, degrees) → hyg's quaternion, pitch
/// and yaw negated for the handedness mirror.
fn unreal_rotation(value: &serde_json::Value) -> Quat {
    let get = |key: &str| value.get(key).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
    let (pitch, yaw, roll) = (
        get("Pitch").to_radians(),
        get("Yaw").to_radians(),
        get("Roll").to_radians(),
    );
    let (p, y, r) = (-pitch, -yaw, roll);
    let (cp, sp) = (p.cos() * 0.5, p.sin() * 0.5);
    let (cy, sy) = (y.cos() * 0.5, y.sin() * 0.5);
    let (cr, sr) = (r.cos() * 0.5, r.sin() * 0.5);
    Quat::new(
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
        cr * cp * cy + sr * sp * sy,
    )
}

/// A UE scale3D → hyg's Vec3 (magnitudes kept, z flipped to match the
/// location mirror).
fn unreal_scale(value: &serde_json::Value) -> Vec3 {
    let get = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0)
            .abs() as f32
    };
    Vec3::new(get("X"), get("Y"), -get("Z"))
}

/// The import's honest coverage statement for a level.
#[derive(Debug, Clone, Default)]
pub struct UnrealImportReport {
    /// The level's package name (`/Game/Level/TheIsland`).
    pub level: String,
    /// Whether the level uses World Partition (its actors live in
    /// external files — the importer walks them).
    pub world_partition: bool,
    /// Actors imported as entities.
    pub actors: usize,
    /// Component classes seen and MAPPED, with counts.
    pub mapped: Vec<(String, usize)>,
    /// Component classes seen and SKIPPED (no hyg counterpart yet),
    /// with counts and the standing reason.
    pub skipped: Vec<(String, usize, String)>,
    /// The external-actor files walked.
    pub actor_files: usize,
    /// Warnings: parse fallbacks, unmapped classes, decisions.
    pub warnings: Vec<String>,
}

/// One imported level: the scene document and the report.
#[derive(Debug, Clone)]
pub struct UnrealLevelImport {
    /// The scene document (write it with the carrier).
    pub document: Var,
    /// The report.
    pub report: UnrealImportReport,
}

impl UnrealImportReport {
    /// Count one mapped component class.
    fn bump_mapped(&mut self, class: &str) {
        for (seen, count) in self.mapped.iter_mut() {
            if seen == class {
                *count += 1;
                return;
            }
        }
        self.mapped.push((class.to_string(), 1));
    }

    /// Count one skipped component class (no hyg counterpart yet).
    fn skip_component(&mut self, class: &str) {
        for (seen, count, _) in self.skipped.iter_mut() {
            if seen == class {
                *count += 1;
                return;
            }
        }
        self.skipped.push((
            class.to_string(),
            1,
            "no hyg counterpart yet".to_string(),
        ));
    }
}

/// One imported actor, assembled from an external-actor file: the
/// entity (named from the actor label), the transform (from the root
/// component), and any hyg components that mapped.
struct ActorDraft {
    entity: Entity,
    mapped_classes: Vec<String>,
}

/// Map one UE component class to hyg components on `entity`. Returns
/// the mapped class name, or `None` when nothing maps — the caller
/// records the skip into the report.
fn map_component(
    class: &str,
    entity: &mut Entity,
    properties: &serde_json::Value,
) -> Option<String> {
    // Lights: any UE light component becomes a point light, carrying
    // the UE color (BGRA order in the Color struct)
    if class.ends_with("LightComponent") {
        let mut light = Light::default();
        if let Some(color) = properties.get("LightColor") {
            let get = |key: &str| {
                color.get(key).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32
            };
            light = light.with_color([get("B"), get("G"), get("R")]);
        }
        entity.add_component(Box::new(light));
        return Some(class.to_string());
    }
    // Cameras: fov across, positioned by its relative location
    if class == "CameraComponent" {
        let fov = properties
            .get("FieldOfView")
            .and_then(|v| v.as_f64())
            .unwrap_or(90.0) as f32;
        let position = properties
            .get("RelativeLocation")
            .map(unreal_location)
            .unwrap_or(Vec3::splat(0.0));
        entity.add_component(Box::new(CameraComponent {
            camera: Camera {
                position,
                target: position + Vec3::new(0.0, 0.0, 1.0),
                projection: Projection::Perspective {
                    fov_y_degrees: fov,
                    near: 0.1,
                    far: 500.0,
                    aspect: 16.0 / 9.0,
                },
            },
            main: false,
        }));
        return Some(class.to_string());
    }
    // Static meshes: a placeholder renderer stands in (geometry
    // conversion is a later rung — the mesh name lands in the report)
    if class == "StaticMeshComponent" {
        let mesh = properties
            .get("StaticMesh")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string();
        entity.add_component(Box::new(MeshRenderer::rect(
            Vec3::splat(0.5),
            [0.35, 0.5, 0.75, 1.0],
        )));
        return Some(format!("StaticMeshComponent ({mesh})"));
    }
    // Box colliders: static physics bodies (the shape halves — UE
    // sizes are full extents)
    if class == "BoxComponent" {
        let size = properties.get("BoxExtent").map(unreal_scale).unwrap_or(Vec3::splat(0.5));
        entity.add_component(Box::new(
            PhysicsBody::new(
                        "imported-box",
                        Shape::Box(Vec3::new(
                            size.x / 2.0,
                            size.y / 2.0,
                            size.z / 2.0,
                        )),
                    )
                    .static_body(),
        ));
        return Some(class.to_string());
    }
    None
}

/// The ACTOR export of an external-actor file: the one carrying an
/// `ActorLabel` in its properties.
fn actor_of(exports: &[(String, String, serde_json::Value)]) -> Option<usize> {
    exports
        .iter()
        .position(|(class, _, properties)| {
            class != "WorldPartitionRuntimeCellData"
                && properties.get("ActorLabel").is_some()
        })
}

/// Parse one external-actor file's exports into an [`ActorDraft`] (or
/// `None` when the file isn't an actor file). `root_component_name` is
/// the actor's `RootComponent` reference — its properties carry the
/// actor's transform.
fn parse_actor_file(
    doc: &serde_json::Value,
    report: &mut UnrealImportReport,
) -> Option<ActorDraft> {
    let exports = exports_of(doc);
    let actor_index = actor_of(&exports)?;

    // the actor export: label, root component reference
    let (_, actor_name, actor_properties) = &exports[actor_index];
    let label = actor_properties
        .get("ActorLabel")
        .and_then(|v| v.as_str())
        .unwrap_or(actor_name)
        .to_string();
    let root_component = actor_properties
        .get("RootComponent")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let mut entity = Entity::new(&label);
    let mut mapped_classes = Vec::new();

    // the transform comes from the ROOT component's properties
    if let Some(root_name) = &root_component {
        for (class, name, properties) in &exports {
            if name != root_name {
                continue;
            }
            entity.transform_mut().position = properties
                .get("RelativeLocation")
                .map(unreal_location)
                .unwrap_or(Vec3::splat(0.0));
            entity.transform_mut().rotation = properties
                .get("RelativeRotation")
                .map(unreal_rotation)
                .unwrap_or(Quat::new(0.0, 0.0, 0.0, 1.0));
            entity.transform_mut().scale = properties
                .get("RelativeScale3D")
                .map(unreal_scale)
                .unwrap_or(Vec3::splat(1.0));
            let _ = (class, name);
            break;
        }
    }

    // the file's other exports: components on this actor
    for (class, name, properties) in &exports {
        let _ = name;
        if class.contains("Actor") && !class.ends_with("Component") {
            continue; // the actor itself / actor-level helpers
        }
        if let Some(mapped) = map_component(class, &mut entity, properties) {
            mapped_classes.push(mapped);
        } else {
            report.skip_component(class);
        }
    }

    report.actors += 1;
    Some(ActorDraft {
        entity,
        mapped_classes,
    })
}

/// The exports of a parsed asset: `(class, name, properties)` triples
/// in file order.
fn exports_of(doc: &serde_json::Value) -> Vec<(String, String, serde_json::Value)> {
    let mut out = Vec::new();
    if let Some(exports) = doc.get("exports").and_then(|v| v.as_array()) {
        for export in exports {
            let class = export
                .get("class")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let name = export
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();
            let properties =
                export.get("properties").cloned().unwrap_or(serde_json::Value::Null);
            out.push((class, name, properties));
        }
    }
    out
}

/// Parse one external-actor file into its exports (see
/// [`exports_of`]).
fn parse_asset_file(path: &Path) -> Result<serde_json::Value, String> {
    let asset = Asset::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json = asset.to_json(&path.to_string_lossy());
    serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display()))
}

/// Import a UE5 level: the `.umap` plus (for World Partition levels)
/// every external-actor file under the level's `__ExternalActors__`
/// directory. The scene document lands as the caller's to write.
///
/// # Errors
/// The map doesn't parse, or an actor file fails mid-walk.
pub fn import_level(umap_path: &Path) -> Result<UnrealLevelImport, String> {
    let asset = Asset::open(umap_path).map_err(|e| format!("{}: {e}", umap_path.display()))?;
    let json = asset.to_json(&umap_path.to_string_lossy());
    let doc: serde_json::Value =
        serde_json::from_str(&json).map_err(|e| format!("{}: {e}", umap_path.display()))?;

    let level = doc
        .get("package")
        .and_then(|v| v.as_str())
        .unwrap_or("?")
        .to_string();
    let world_partition = exports_of(&doc)
        .iter()
        .any(|(class, _, _)| class == "WorldPartition");

    let mut report = UnrealImportReport {
        level: level.clone(),
        world_partition,
        ..UnrealImportReport::default()
    };

    // the map body itself: count what it declares (World Partition
    // levels hold structure, not actors)
    let body_exports = exports_of(&doc);
    report.warnings.push(format!(
        "the map body declares {} export(s); World Partition actors arrive from the external files",
        body_exports.len()
    ));

    // the external actors: Content/<sub>/<name>.umap mirrors
    // Content/__ExternalActors__/<sub>/<name>/<hash>.uasset
    let mut scene_entities: Vec<Entity> = Vec::new();
    let external_root = external_actors_dir(umap_path)?;
    if external_root.exists() {
        let mut files: Vec<PathBuf> = Vec::new();
        let mut stack = vec![external_root.clone()];
        while let Some(current) = stack.pop() {
            for entry in std::fs::read_dir(&current)
                .map_err(|e| format!("{}: {e}", current.display()))?
                .flatten()
            {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().and_then(|e| e.to_str()) == Some("uasset") {
                    files.push(path);
                }
            }
        }
        files.sort();
        report.actor_files = files.len();

        for file in &files {
            match parse_asset_file(file) {
                Ok(doc) => {
                    if let Some(draft) = parse_actor_file(&doc, &mut report) {
                        scene_entities.push(draft.entity);
                        for mapped in draft.mapped_classes {
                            report.bump_mapped(&mapped);
                        }
                    }
                }
                Err(e) => report.warnings.push(e),
            }
        }
    } else if world_partition {
        report.warnings.push(
            "World Partition detected but no __ExternalActors__ directory was found".to_string(),
        );
    }

    // a scene without a camera still plays: the default camera entity
    let has_camera = scene_entities.iter().any(|entity| {
        entity
            .iter_components()
            .any(|c| c.scene_type() == Some("camera"))
    });
    if !has_camera {
        let mut camera = Entity::new("main camera");
        camera.add_component(Box::new(CameraComponent {
            camera: Camera {
                position: Vec3::new(0.0, 20.0, -40.0),
                target: Vec3::splat(0.0),
                projection: Projection::Perspective {
                    fov_y_degrees: 60.0,
                    near: 0.1,
                    far: 5000.0,
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
        umap_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unreal-level"),
        &Camera::default(),
        &scene_entities,
    );

    Ok(UnrealLevelImport {
        document,
        report,
    })
}

/// The `__ExternalActors__` directory for a level: the `.umap`'s
/// `Content/`-relative parent path mirrors the external layout
/// (`Content/Level/TheIsland.umap` →
/// `Content/__ExternalActors__/Level/TheIsland`).
fn external_actors_dir(umap_path: &Path) -> Result<PathBuf, String> {
    // walk up to the Content dir
    let mut content: Option<&Path> = None;
    let mut current = umap_path.parent();
    while let Some(dir) = current {
        if dir.file_name().and_then(|n| n.to_str()) == Some("Content") {
            content = Some(dir);
            break;
        }
        current = dir.parent();
    }
    let content = content.ok_or_else(|| {
        format!("{}: no Content directory above the level", umap_path.display())
    })?;
    let relative = umap_path
        .parent()
        .and_then(|p| p.strip_prefix(content).ok())
        .ok_or_else(|| {
            format!("{}: not under {}", umap_path.display(), content.display())
        })?;
    Ok(content.join("__ExternalActors__").join(relative))
}

/// Locate the level's external-actor directory (the importer's own
/// use; exposed for tools that report on the layout).
#[must_use]
pub fn external_actors_path(umap_path: &Path) -> Option<PathBuf> {
    external_actors_dir(umap_path).ok()
}
