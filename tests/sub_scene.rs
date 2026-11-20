//! The sub-scene contracts (the prefab shape): instances splice their
//! referenced scene's entities in as children, the reference is the
//! truth (documents and snapshots carry the `sub_scene` component, not
//! the spliced content), cycles refuse loudly, nested references
//! resolve against the sub-scene's OWN directory, and the instancing
//! scene keeps its camera.

use hyg::carrier;
use hyg::game::scene::Scene;
use hyg::game::{Entity, Light, SubScene};
use hyg::var::Vec3;
use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// A scene document with one root entity, saved through the carrier.
fn write_scene(dir: &std::path::Path, rel: &str, name: &str, entity: Entity) -> PathBuf {
    let mut scene = Scene::new(name);
    scene.entities.push(entity);
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("mkdir for the scene file");
    }
    carrier::write(&path, &scene.to_var()).expect("writes the scene");
    path
}

/// A scene's bytes on disk, as text (documents are the carrier's job).
fn scene_text(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).expect("reads the saved document")
}

/// `Scene::load`'s error, or panic — `Scene` carries no `Debug`, so
/// `expect_err` is off the table.
fn load_err(path: &std::path::Path, what: &str) -> hyg::game::scene::SceneLoadError {
    match Scene::load(path) {
        Err(e) => e,
        Ok(_) => panic!("expected a load refusal: {what}"),
    }
}

fn instance_entity(name: &str, sub_rel: &str, position: [f32; 3]) -> Entity {
    let mut e = Entity::new(name);
    e.transform_mut().position = Vec3::new(position[0], position[1], position[2]);
    e.add_component(Box::new(SubScene::new(sub_rel)));
    e
}

/// The e2e instance: spliced children join after authored ones, marked
/// spliced, and the SAVED document carries the reference — not the
/// content — so the next load re-splices identically.
#[test]
fn instance_splices_children_and_the_reference_is_the_truth() {
    let dir = temp_dir("hyg-sub-scene-basic");

    // the prefab: a room with a lamp
    let mut lamp = Entity::new("lamp");
    lamp.add_component(Box::new(Light::default()));
    write_scene(&dir, "room.json.hyg", "room", lamp);

    // the world: an authored child under the instance root
    let mut world = instance_entity("room_3", "room.json.hyg", [12.0, 0.0, -4.0]);
    world.add_child(Entity::new("prop"));
    let world_path = write_scene(&dir, "world.json.hyg", "world", world);

    let loaded = Scene::load(&world_path).expect("loads");
    assert_eq!(loaded.entities.len(), 1, "one root: the instance");
    let room = &loaded.entities[0];
    assert_eq!(room.name(), "room_3");
    assert!(
        room.get_component::<SubScene>().is_some(),
        "the reference rides the live tree (the inspector sees it)"
    );

    // the splice: authored "prop" first, then the prefab's "lamp"
    let names: Vec<&str> = room.children().iter().map(|c| c.name()).collect();
    assert_eq!(names, ["prop", "lamp"], "authored children keep their slot");
    assert!(
        !room.children()[0].is_spliced() && room.children()[1].is_spliced(),
        "only the spliced child carries the mark"
    );
    // the instance entity positions its content (local poses untouched)
    assert_eq!(room.transform().position, Vec3::new(12.0, 0.0, -4.0));

    // save: the document carries the reference, not the spliced content
    let saved_path = dir.join("saved.json.hyg");
    carrier::write(&saved_path, &loaded.to_var()).expect("saves");
    let text = scene_text(&saved_path);
    assert!(text.contains("sub_scene") && text.contains("room.json.hyg"));
    assert!(
        !text.contains("\"lamp\""),
        "spliced children stay out of documents: {text}"
    );
    assert!(text.contains("\"prop\""), "authored children survive");

    // reload the saved bytes: the instance splices again (idempotent)
    let reloaded = Scene::load(&world_path).expect("reloads");
    let room = &reloaded.entities[0];
    assert_eq!(room.children().iter().map(|c| c.name()).collect::<Vec<_>>(), ["prop", "lamp"]);
}

/// Cycles refuse loudly — a scene including itself directly or through
/// a chain is an error naming the cycle, never a hang.
#[test]
fn cycles_refuse_loudly() {
    let dir = temp_dir("hyg-sub-scene-cycle");

    // a → b → a
    let mut a = instance_entity("a", "b.json.hyg", [0.0; 3]);
    a.add_child(Entity::new("a_body"));
    write_scene(&dir, "a.json.hyg", "a", a);
    let mut b = instance_entity("b", "a.json.hyg", [0.0; 3]);
    b.add_child(Entity::new("b_body"));
    let b_path = write_scene(&dir, "b.json.hyg", "b", b);

    let err = load_err(&dir.join("a.json.hyg"), "the chain refuses");
    let message = err.to_string();
    assert!(message.contains("cycle"), "names the cycle: {message}");

    // loading b alone is the SAME cycle from the other side (b → a → b)
    let err = load_err(&b_path, "b refuses too");
    assert!(err.to_string().contains("cycle"), "{err}");

    // self-inclusion
    let self_e = instance_entity("self", "self.json.hyg", [0.0; 3]);
    let self_path = write_scene(&dir, "self.json.hyg", "self", self_e);
    let err = load_err(&self_path, "self-inclusion refuses");
    assert!(err.to_string().contains("cycle"), "{err}");
}

/// A missing sub-scene file refuses the load with the referencing
/// entity in the path.
#[test]
fn missing_sub_scene_refuses_loudly() {
    let dir = temp_dir("hyg-sub-scene-missing");
    let world = instance_entity("room_3", "gone.json.hyg", [0.0; 3]);
    let world_path = write_scene(&dir, "world.json.hyg", "world", world);

    let err = load_err(&world_path, "the missing file refuses");
    let message = err.to_string();
    assert!(message.contains("gone.json.hyg"), "names the file: {message}");
    assert!(message.contains("missing"), "{message}");
}

/// Nested references resolve against the SUB-SCENE's own directory:
/// rooms/room.json.hyg names nested/desk.json.hyg relative to rooms/,
/// wherever the instancing scene lives. The sub-document's camera is
/// ignored — instances contribute entities only.
#[test]
fn nested_references_resolve_against_the_sub_scene_dir() {
    let dir = temp_dir("hyg-sub-scene-nested");

    // the deep prefab
    write_scene(
        &dir.join("rooms"),
        "nested/desk.json.hyg",
        "desk_scene",
        Entity::new("desk"),
    );
    // the mid prefab: its own lamp plus the desk instance
    let mut room = instance_entity("room_body", "nested/desk.json.hyg", [0.0; 3]);
    let mut lamp = Entity::new("lamp");
    lamp.add_component(Box::new(Light::default()));
    room.add_child(lamp);
    write_scene(&dir.join("rooms"), "room.json.hyg", "room", room);

    // the world, a directory ABOVE: room resolves, and room's own
    // nested reference resolves against rooms/ — not the world's dir
    let world = instance_entity("room_3", "rooms/room.json.hyg", [0.0; 3]);
    let world_path = write_scene(&dir, "world.json.hyg", "world", world);

    let loaded = Scene::load(&world_path).expect("the whole chain loads");
    let room = &loaded.entities[0];
    let names: Vec<&str> = room.children().iter().map(|c| c.name()).collect();
    assert_eq!(
        names,
        ["room_body"],
        "the mid prefab's root (the room scene has one entity)"
    );
    // the mid instance: its authored lamp, then its desk instance — the
    // desk resolved against rooms/nested/, NOT the world's directory
    let room_body = &room.children()[0];
    let inner: Vec<&str> = room_body.children().iter().map(|c| c.name()).collect();
    assert_eq!(inner, ["lamp", "desk"], "the deep splice landed");

    // the sub-document's camera is ignored: the world keeps its view
    let default = hyg::game::Camera::default();
    assert_eq!(
        format!("{:?}", loaded.camera.position),
        format!("{:?}", default.position),
        "the instance scene's camera wins, not the prefab's"
    );
}

/// The snapshot carries the reference too: spliced children stay out,
/// and a restore against the scene's directory re-splices them — the
/// restart contract (the instance rebuilds from its file, wherever the
/// process launched from).
#[test]
fn snapshot_carries_the_reference_and_restore_resplices() {
    let dir = temp_dir("hyg-sub-scene-snapshot");
    let mut lamp = Entity::new("lamp");
    lamp.add_component(Box::new(Light::default()));
    write_scene(&dir, "room.json.hyg", "room", lamp);
    let world = instance_entity("room_3", "room.json.hyg", [0.0; 3]);
    let world_path = write_scene(&dir, "world.json.hyg", "world", world);

    let loaded = Scene::load(&world_path).expect("loads");
    let snapshot = Scene::snapshot_document("world", &loaded.camera, &loaded.entities);
    let snapshot_path = dir.join("snapshot.json.hyg");
    carrier::write(&snapshot_path, &snapshot.clone()).expect("saves the snapshot");
    let text = scene_text(&snapshot_path);
    assert!(
        !text.contains("\"lamp\""),
        "the snapshot keeps the reference, not the content: {text}"
    );

    // restore with the scene's base (the editor's restore seam)
    let restored = Scene::from_var_with_base(&snapshot, dir.clone()).expect("restores");
    let room = &restored.entities[0];
    assert_eq!(
        room.children().iter().map(|c| c.name()).collect::<Vec<_>>(),
        ["lamp"],
        "the instance re-spliced from its file"
    );
}

/// The factory's contract: `sub_scene` requires a non-empty path,
/// error path-tracked; a valid params object materializes.
#[test]
fn factory_requires_a_path() {
    use hyg::game::scene::{create_component, ComponentContext};

    let ctx = ComponentContext::new("room_3");
    let empty = hyg::Var::from(hyg::o! { "path": hyg::Var::from("") });
    let err = match create_component("sub_scene", &empty, &ctx) {
        Err(e) => e,
        Ok(_) => panic!("an empty path refuses"),
    };
    assert!(err.to_string().contains("path"), "{err}");

    let ok = hyg::Var::from(hyg::o! { "path": hyg::Var::from("rooms/room.json.hyg") });
    let component = create_component("sub_scene", &ok, &ctx).expect("materializes");
    assert_eq!(component.scene_type(), Some("sub_scene"));
    let params = component
        .scene_params(&ctx)
        .expect("the params round-trip");
    assert_eq!(params.get("path").as_str(), Some("rooms/room.json.hyg"));
}
