//! The Studio editor's headless core (M1): selection, hierarchy snapshots,
//! and inspector edits — the UI logic that runs without a window. The
//! shell's panels are a thin view over exactly these operations.

use hyg::game::editor::ui::EditorUi;
use hyg::game::light::Light;
use hyg::game::physics::Shape;
use hyg::game::physics_body::PhysicsBody;
use hyg::game::renderable::{MeshGeometry, MeshRenderer, MeshShape};
use hyg::game::{Engine, Entity};
use hyg::var::{Quat, Vec3};

/// The demo tree: two roots, the first with a child and a grandchild.
fn demo_engine() -> Engine {
    let mut engine = Engine::new("acme", "editor-test");

    let mut world = Entity::new("world");
    world.add_component(Box::new(Light::directional()));

    let mut stack = Entity::new("stack");
    stack.transform_mut().position = Vec3::new(0.0, 2.0, 0.0);
    stack.add_component(Box::new(
        PhysicsBody::new("stack", Shape::Box(Vec3::new(0.5, 0.5, 0.5))).static_body(),
    ));

    let mut block = Entity::new("block_3");
    block.set_enabled(false);
    block.add_component(Box::new(MeshRenderer::new(
        MeshGeometry::Shape {
            shape: MeshShape::Circle,
            half_size: Vec3::new(0.3, 0.3, 0.3),
        },
        [1.0, 0.4, 0.1, 1.0],
    )));
    stack.add_child(block);
    world.add_child(stack);
    engine.add_entity(world);

    let mut free = Entity::new("free");
    free.transform_mut().scale = Vec3::new(2.0, 2.0, 2.0);
    engine.add_entity(free);
    engine
}

/// The hierarchy snapshot is the tree: names, enabled flags, component
/// type names, and each node carrying the path that selects it back.
#[test]
fn hierarchy_snapshots_the_tree() {
    let engine = demo_engine();
    let nodes = EditorUi::hierarchy(&engine);

    assert_eq!(nodes.len(), 2, "two roots");
    let world = &nodes[0];
    assert_eq!(world.name, "world");
    assert_eq!(world.path, vec![0]);
    assert_eq!(world.component_types, vec!["light"], "scene type names");

    let stack = &world.children[0];
    assert_eq!(stack.name, "stack");
    assert_eq!(stack.path, vec![0, 0], "the child's full path");
    assert_eq!(stack.component_types, vec!["physics_body"]);

    let block = &stack.children[0];
    assert_eq!(block.name, "block_3");
    assert_eq!(block.path, vec![0, 0, 0]);
    assert!(!block.enabled, "the disabled flag rides the snapshot");
    assert_eq!(block.component_types, vec!["mesh_renderer"]);

    // every snapshot path resolves back to the same-named entity
    for node in [world, stack, block] {
        let entity = engine
            .entity_at(&node.path)
            .unwrap_or_else(|| panic!("path {:?} resolves", node.path));
        assert_eq!(entity.name(), node.name);
    }
}

/// Selection resolves through the engine; a stale path reads as no
/// entity, not a panic.
#[test]
fn selection_resolves_and_goes_stale() {
    let engine = demo_engine();
    let mut ui = EditorUi::new();

    assert!(ui.selected_entity(&engine).is_none(), "nothing selected");

    ui.select(vec![0, 0]);
    assert_eq!(
        ui.selected_entity(&engine).map(|e| e.name()),
        Some("stack")
    );

    // selecting the grandchild through the snapshot's path
    ui.select(vec![0, 0, 0]);
    assert_eq!(
        ui.selected_entity(&engine).map(|e| e.name()),
        Some("block_3")
    );

    // an out-of-tree path is stale, not a panic
    ui.select(vec![9, 9]);
    assert!(ui.selected_entity(&engine).is_none());

    ui.deselect();
    assert!(ui.selected_entity(&engine).is_none());
}

/// The inspector edits write the live engine through the selection.
#[test]
fn inspector_edits_write_the_engine() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![0, 0]); // "stack"

    // rename
    assert!(ui.rename_selected(&mut engine, "pile"));
    assert_eq!(engine.entity_at(&[0, 0]).map(|e| e.name()), Some("pile"));

    // enable toggle
    assert!(ui.set_selected_enabled(&mut engine, false));
    assert!(!engine.entity_at(&[0, 0]).unwrap().enabled());

    // position / scale / rotation
    assert!(ui.set_selected_position(&mut engine, Vec3::new(1.0, 2.0, 3.0)));
    assert!(ui.set_selected_scale(&mut engine, Vec3::new(2.0, 1.0, 1.0)));
    assert!(ui.set_selected_rotation(&mut engine, Quat::axis_angle(
        Vec3::new(0.0, 1.0, 0.0),
        std::f32::consts::FRAC_PI_2,
    )));
    let transform = engine.entity_at(&[0, 0]).unwrap().transform();
    assert_eq!(transform.position, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(transform.scale, Vec3::new(2.0, 1.0, 1.0));
    let quarter = std::f32::consts::FRAC_PI_2 / 2.0;
    assert_eq!(
        transform.rotation,
        Quat::new(0.0, quarter.sin(), 0.0, quarter.cos())
    );
}

/// Every edit is a no-op answer (`false`) with the engine untouched when
/// the selection is empty or stale.
#[test]
fn edits_without_a_live_selection_do_nothing() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();

    // nothing selected
    assert!(!ui.rename_selected(&mut engine, "ghost"));
    assert!(!ui.set_selected_enabled(&mut engine, false));
    assert!(!ui.set_selected_position(&mut engine, Vec3::splat(9.0)));
    assert!(!ui.set_selected_rotation(&mut engine, Quat::new(1.0, 0.0, 0.0, 0.0)));
    assert!(!ui.set_selected_scale(&mut engine, Vec3::splat(9.0)));

    // a stale selection (the tree moved under the editor)
    ui.select(vec![1, 2, 3]);
    assert!(!ui.rename_selected(&mut engine, "ghost"));

    // the engine is untouched
    assert_eq!(
        engine.entity_at(&[0, 0]).map(|e| e.name()),
        Some("stack"),
        "renames against dead selections never land"
    );
    assert_eq!(
        engine.entity_at(&[0, 0]).unwrap().transform().position,
        Vec3::new(0.0, 2.0, 0.0)
    );
}

/// The pump keeps updating while the editor edits: a transform edit is
/// visible to the next frame's update pass (the live-engine contract).
#[test]
fn edits_are_visible_to_the_next_pump_frame() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![1]); // "free"
    ui.set_selected_position(&mut engine, Vec3::new(5.0, 0.0, 0.0));
    engine.update(1.0 / 60.0);
    engine.draw();
    assert_eq!(
        engine.entity_at(&[1]).unwrap().transform().position,
        Vec3::new(5.0, 0.0, 0.0),
        "the edit survives frames (nothing overwrites an unmoved entity)"
    );
}
