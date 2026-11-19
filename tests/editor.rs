//! The Studio editor's headless core (M1 + M2): selection, hierarchy
//! snapshots, inspector edits, the undo/redo stack with per-drag
//! coalescing, viewport picking, gizmo math, and the scene-io ops the
//! shell's buttons drive — the UI logic that runs without a window. The
//! shell's panels are a thin view over exactly these operations.

use hyg::game::camera::{Camera, Projection};
use hyg::game::editor::gizmo::{self, GizmoMode};
use hyg::game::editor::pick::{self, Viewport};
use hyg::game::editor::ui::EditorUi;
use hyg::game::light::Light;
use hyg::game::physics::Shape;
use hyg::game::physics_body::PhysicsBody;
use hyg::game::renderable::{MeshGeometry, MeshRenderer, MeshShape};
use hyg::game::scene::Scene;
use hyg::game::Mesh;
use hyg::game::Model;
use hyg::game::{Engine, Entity};
use hyg::var::{Quat, Vec3};

/// A picking-friendly camera: perspective, looking +z from (0, 0, -8) at
/// the origin, 16:9.
fn pick_camera() -> Camera {
    Camera {
        position: Vec3::new(0.0, 0.0, -8.0),
        target: Vec3::new(0.0, 0.0, 0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 60.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
        },
    }
}

fn view() -> Viewport {
    Viewport::new(0.0, 0.0, 960.0, 540.0)
}

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

// ---- M1: hierarchy, selection, inspector edits ----

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

    ui.select(vec![0, 0, 0]);
    assert_eq!(
        ui.selected_entity(&engine).map(|e| e.name()),
        Some("block_3")
    );

    ui.select(vec![9, 9]);
    assert!(ui.selected_entity(&engine).is_none());

    ui.deselect();
    assert!(ui.selected_entity(&engine).is_none());
}

/// The inspector edits write the live engine through the selection, and
/// each records an undo command.
#[test]
fn inspector_edits_write_the_engine() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![0, 0]); // "stack"

    assert!(ui.rename_selected(&mut engine, "pile", 0));
    assert_eq!(engine.entity_at(&[0, 0]).map(|e| e.name()), Some("pile"));

    assert!(ui.set_selected_enabled(&mut engine, false, 10));
    assert!(!engine.entity_at(&[0, 0]).unwrap().enabled());

    assert!(ui.set_selected_position(&mut engine, Vec3::new(1.0, 2.0, 3.0), 20));
    assert!(ui.set_selected_scale(&mut engine, Vec3::new(2.0, 1.0, 1.0), 30));
    let spin = Quat::axis_angle(Vec3::new(0.0, 1.0, 0.0), std::f32::consts::FRAC_PI_2);
    assert!(ui.set_selected_rotation(&mut engine, spin, 40));
    let transform = engine.entity_at(&[0, 0]).unwrap().transform();
    assert_eq!(transform.position, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(transform.scale, Vec3::new(2.0, 1.0, 1.0));
    assert_eq!(transform.rotation, spin);

    // the different fields DID NOT coalesce — five edits, five steps
    let mut undone = 0;
    while ui.undo(&mut engine) {
        undone += 1;
    }
    assert_eq!(undone, 5, "one undo step per edited field");
    let transform = engine.entity_at(&[0, 0]).unwrap().transform();
    assert_eq!(transform.position, Vec3::new(0.0, 2.0, 0.0), "back to start");
    assert_eq!(engine.entity_at(&[0, 0]).map(|e| e.name()), Some("stack"));
}

/// Every edit is a no-op answer (`false`) with the engine untouched when
/// the selection is empty or stale.
#[test]
fn edits_without_a_live_selection_do_nothing() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();

    assert!(!ui.rename_selected(&mut engine, "ghost", 0));
    assert!(!ui.set_selected_enabled(&mut engine, false, 0));
    assert!(!ui.set_selected_position(&mut engine, Vec3::splat(9.0), 0));
    assert!(!ui.set_selected_rotation(&mut engine, Quat::new(1.0, 0.0, 0.0, 0.0), 0));
    assert!(!ui.set_selected_scale(&mut engine, Vec3::splat(9.0), 0));

    ui.select(vec![1, 2, 3]);
    assert!(!ui.rename_selected(&mut engine, "ghost", 0));

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
    ui.set_selected_position(&mut engine, Vec3::new(5.0, 0.0, 0.0), 0);
    engine.update(1.0 / 60.0);
    engine.draw();
    assert_eq!(
        engine.entity_at(&[1]).unwrap().transform().position,
        Vec3::new(5.0, 0.0, 0.0),
        "the edit survives frames (nothing overwrites an unmoved entity)"
    );
}

// ---- M2: undo/redo ----

/// A drag's per-frame edits coalesce: same field, same entity, inside
/// the window → ONE undo step that returns to the gesture's start.
#[test]
fn undo_coalesces_a_drag_into_one_step() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![1]);
    let start = engine.entity_at(&[1]).unwrap().transform().position;

    // the drag: three frames, 30ms apart
    ui.set_selected_position(&mut engine, start + Vec3::new(1.0, 0.0, 0.0), 0);
    ui.set_selected_position(&mut engine, start + Vec3::new(2.0, 0.0, 0.0), 30);
    ui.set_selected_position(&mut engine, start + Vec3::new(3.0, 0.0, 0.0), 60);
    let end = engine.entity_at(&[1]).unwrap().transform().position;
    assert_eq!(end, start + Vec3::new(3.0, 0.0, 0.0));

    // ONE undo returns to where the gesture started
    assert!(ui.undo(&mut engine));
    assert_eq!(
        engine.entity_at(&[1]).unwrap().transform().position,
        start,
        "the coalesced command keeps the gesture's start"
    );
    assert!(!ui.undo(&mut engine), "one drag, one step");
    // and the redo lands back at the gesture's end
    assert!(ui.redo(&mut engine));
    assert_eq!(engine.entity_at(&[1]).unwrap().transform().position, end);
}

/// Edits outside the coalescing window are separate steps — two
/// deliberate clicks undo one at a time.
#[test]
fn undo_window_splits_deliberate_edits() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![1]);
    let start = engine.entity_at(&[1]).unwrap().transform().position;

    ui.set_selected_position(&mut engine, start + Vec3::new(1.0, 0.0, 0.0), 0);
    ui.set_selected_position(&mut engine, start + Vec3::new(2.0, 0.0, 0.0), 1_000);

    assert!(ui.undo(&mut engine));
    assert_eq!(
        engine.entity_at(&[1]).unwrap().transform().position,
        start + Vec3::new(1.0, 0.0, 0.0),
        "the second edit undoes alone"
    );
    assert!(ui.undo(&mut engine));
    assert_eq!(engine.entity_at(&[1]).unwrap().transform().position, start);
}

/// A new command clears the redo stack (linear history).
#[test]
fn redo_clears_on_a_fresh_edit() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![1]);
    ui.set_selected_position(&mut engine, Vec3::new(1.0, 0.0, 0.0), 0);
    assert!(ui.undo(&mut engine));
    assert!(ui.undo_stack().can_redo());

    ui.set_selected_position(&mut engine, Vec3::new(2.0, 0.0, 0.0), 5_000);
    assert!(
        !ui.undo_stack().can_redo(),
        "a fresh edit invalidates the redo branch"
    );
}

/// Undo covers every kind: rename, enable, and the delete's subtree
/// restore (the whole tree comes back, redo removes it again).
#[test]
fn undo_and_redo_cover_rename_enable_delete() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![0, 0]); // stack (has a child)

    ui.rename_selected(&mut engine, "pile", 0);
    ui.set_selected_enabled(&mut engine, false, 10);
    ui.set_selected_position(&mut engine, Vec3::new(7.0, 7.0, 7.0), 20);

    // delete the whole subtree
    ui.select(vec![0, 0]);
    assert!(ui.delete_selected(&mut engine, 30));
    assert!(engine.entity_at(&[0, 0]).is_none(), "the subtree is gone");
    assert!(ui.selection().is_none(), "a deleted entity cannot stay selected");

    // walk everything back
    while ui.undo(&mut engine) {}
    let restored = engine.entity_at(&[0, 0]).expect("the subtree restored");
    assert_eq!(restored.name(), "stack");
    assert!(restored.enabled());
    assert_eq!(restored.transform().position, Vec3::new(0.0, 2.0, 0.0));
    assert_eq!(restored.children().len(), 1, "the child came back too");

    // and forward again: the delete re-applies (the subtree is re-removed)
    while ui.redo(&mut engine) {}
    assert!(engine.entity_at(&[0, 0]).is_none(), "deleted again");
    assert_eq!(engine.entities().len(), 2, "the roots survived the cycle");
}

// ---- M2: viewport picking ----

/// A click through an entity's projected position picks it; the nearest
/// box wins when two overlap; empty space deselects.
#[test]
fn picking_hits_the_frontmost_entity() {
    let mut engine = Engine::new("acme", "pick-test");
    let camera = pick_camera();
    let view = view();

    // MESH entities — 3D scenes render through the mesh/VP path, and
    // the pick/gizmo must match THAT (quads are the 2D/ortho path;
    // their x maps oppositely under perspective cameras)
    let box_renderer = || {
        MeshRenderer::model(
            std::sync::Arc::new(hyg::game::Model::single(std::sync::Arc::new(
                hyg::game::Mesh::cube(Vec3::new(1.0, 1.0, 1.0)),
            ))),
            [1.0, 1.0, 1.0, 1.0],
        )
    };
    let mut near = Entity::new("near");
    near.transform_mut().position = Vec3::new(0.0, 0.0, 0.0);
    near.add_component(Box::new(box_renderer()));
    let mut far = Entity::new("far");
    far.transform_mut().position = Vec3::new(4.0, 0.0, 2.0);
    far.add_component(Box::new(box_renderer()));
    engine.add_entity(near);
    engine.add_entity(far);

    // the center of the screen is the origin — BOTH boxes cover it; the
    // nearer box (smaller ray distance) wins
    let center = [
        view.x + view.width * 0.5,
        view.y + view.height * 0.5,
    ];
    assert_eq!(
        pick::click_pick(&engine, &camera, &view, center),
        Some(vec![0]),
        "the nearer entity wins"
    );

    // "far" sits off to the side (+x) — a click exactly through its
    // center hits IT (near's box does not cover that screen point)
    let far_pos = pick::project_point(&camera, &view, Vec3::new(4.0, 0.0, 2.0))
        .expect("far is in front of the camera");
    assert_eq!(
        pick::click_pick(&engine, &camera, &view, far_pos),
        Some(vec![1]),
        "a click through the far box's center picks the far box"
    );

    // empty space: far off to the corner
    assert_eq!(
        pick::click_pick(&engine, &camera, &view, [view.x + 2.0, view.y + 2.0]),
        None,
        "the corner misses everything"
    );

    // disabled entities are not pickable: with the only entity under the
    // center disabled, the same click now misses (a pickable disabled box
    // would have returned Some([0]))
    engine.entity_at_mut(&[0]).unwrap().set_enabled(false);
    assert_eq!(
        pick::click_pick(&engine, &camera, &view, center),
        None,
        "the disabled box is transparent to picks"
    );
}

/// Picking reaches nested entities through their composed world
/// transform: a child moved by its parent is hit where the parent put it.
#[test]
fn picking_reaches_nested_entities() {
    let mut engine = Engine::new("acme", "pick-nested");
    let camera = pick_camera();
    let view = view();

    let mut parent = Entity::new("parent");
    parent.transform_mut().position = Vec3::new(3.0, 0.0, 0.0);
    let mut child = Entity::new("child");
    child.transform_mut().position = Vec3::new(2.0, 0.0, 0.0); // world (5, 0, 0)
    child.add_component(Box::new(MeshRenderer::new(
        MeshGeometry::Shape {
            shape: MeshShape::Rect,
            half_size: Vec3::new(0.5, 0.5, 0.5),
        },
        [1.0, 1.0, 1.0, 1.0],
    )));
    parent.add_child(child);
    engine.add_entity(parent);

    // click through the CHILD's world position
    let child_world = engine
        .world_transform_at(&[0, 0])
        .expect("nested transform")
        .position;
    let px = pick::project_point(&camera, &view, child_world).expect("on screen");
    assert_eq!(pick::click_pick(&engine, &camera, &view, px), Some(vec![0, 0]));
}

// ---- M2: gizmo math ----

/// The projection: an entity in front of the camera lands at the
/// viewport's center (that is where the camera looks); behind the camera
/// is `None`, never a mirror.
#[test]
fn projection_centers_and_refuses_behind() {
    let camera = pick_camera();
    let view = view();

    let center = pick::project_point(&camera, &view, Vec3::new(0.0, 0.0, 0.0))
        .expect("the look target projects");
    assert!(
        (center[0] - view.width * 0.5).abs() < 0.6,
        "x centered: {center:?}"
    );
    assert!(
        (center[1] - view.height * 0.5).abs() < 0.6,
        "y centered: {center:?}"
    );

    assert!(
        pick::project_point(&camera, &view, Vec3::new(0.0, 0.0, -10.0)).is_none(),
        "behind the camera (z < -8) is None"
    );
}

/// A translate drag along an axis's screen direction moves the entity
/// along that WORLD axis, scaled by the axis's px-per-unit at that depth.
#[test]
fn translate_delta_follows_the_screen_axis() {
    let camera = pick_camera();
    let view = view();
    let origin = Vec3::new(0.0, 0.0, 0.0);
    let layout = gizmo::layout(
        GizmoMode::Translate,
        &camera,
        &view,
        origin,
        Quat::new(0.0, 0.0, 0.0, 1.0),
        1.4,
    )
    .expect("the gizmo lays out");

    // the +x axis (not foreshortened from this camera)
    let x = &layout.axes[0];
    assert!(!x.foreshortened, "the x axis faces the camera");
    assert_eq!(x.world_dir, Vec3::new(1.0, 0.0, 0.0));

    // a 30px drag along the handle's screen direction = 30/px_per_unit
    // world units along world +x
    let drag = [x.screen_dir[0] * 30.0, x.screen_dir[1] * 30.0];
    let delta = gizmo::translate_delta(x, drag);
    let expected = 30.0 / x.px_per_unit;
    assert!(
        (delta.x - expected).abs() < 0.05 && delta.y.abs() < 0.01 && delta.z.abs() < 0.01,
        "the drag maps along world +x: {delta:?} (expected x ≈ {expected})"
    );
}

/// The rotate ring's contract: a positive screen-angle sweep (atan2 in
/// y-down space = the drag's visual clockwise) rotates around the
/// camera's forward axis — an object's +x arrow (screen left from this
/// camera) turns toward +y (screen up), following the hand.
#[test]
fn rotate_delta_turns_with_the_drag() {
    let delta = gizmo::rotate_delta(std::f32::consts::FRAC_PI_2, Vec3::new(0.0, 0.0, 1.0));
    let moved = delta.compose(Quat::new(0.0, 0.0, 0.0, 1.0)).rotate(Vec3::new(1.0, 0.0, 0.0));
    assert!(
        (moved.x - 0.0).abs() < 1e-4 && (moved.y - 1.0).abs() < 1e-4,
        "+x rotated a quarter turn to +y: {moved:?}"
    );
}

/// Scale drags are additive along the axis (small scales stay usable).
#[test]
fn scale_delta_is_additive() {
    let camera = pick_camera();
    let view = view();
    let layout = gizmo::layout(
        GizmoMode::Scale,
        &camera,
        &view,
        Vec3::new(0.0, 0.0, 0.0),
        Quat::new(0.0, 0.0, 0.0, 1.0),
        1.4,
    )
    .expect("lays out");
    let x = &layout.axes[0];
    let drag = [x.screen_dir[0] * 50.0, x.screen_dir[1] * 50.0];
    let delta = gizmo::scale_delta(x, drag);
    let expected = 50.0 / x.px_per_unit;
    assert!((delta - expected).abs() < 0.05, "additive drag: {delta}");
}

// ---- round 5/6: the camera component ----

/// The camera component round-trips through the scene document:
/// projection kind, main flag, position/target all survive save/load,
/// and the scene's dimensionality follows the main camera.
#[test]
fn camera_component_round_trips_through_the_scene() {
    use hyg::game::CameraComponent;

    let mut engine = Engine::new("acme", "camera-test");
    let mut entity = Entity::new("main camera");
    entity.add_component(Box::new(CameraComponent {
        camera: Camera {
            position: Vec3::new(2.0, 3.0, -6.0),
            target: Vec3::new(0.0, 1.0, 0.0),
            projection: Projection::Perspective {
                fov_y_degrees: 55.0,
                near: 0.2,
                far: 80.0,
                aspect: 1.6,
            },
        },
        main: true,
    }));
    engine.add_entity(entity);

    // round trip through the document
    let doc = Scene::document("cams", engine.camera(), engine.entities());
    let loaded = Scene::from_var(&doc).expect("the scene loads");
    let found = loaded
        .entities
        .first()
        .and_then(|e| e.get_component::<CameraComponent>())
        .expect("the camera component survives");
    assert!(found.main);
    assert!(matches!(
        found.camera.projection,
        Projection::Perspective { .. }
    ));
    assert_eq!(found.camera.position, Vec3::new(2.0, 3.0, -6.0));
    assert_eq!(found.camera.target, Vec3::new(0.0, 1.0, 0.0));
}

/// The play camera: the main camera component wins; a scene without one
/// plays blank (the shell's play() resolves through this helper's
/// semantics — find_main_camera lives in the shell, so here we pin the
/// ENGINE-side contract the shell relies on: components materialize and
/// are discoverable).
#[test]
fn main_camera_component_is_discoverable() {
    use hyg::game::CameraComponent;

    let mut engine = Engine::new("acme", "camera-test");
    let mut entity = Entity::new("main camera");
    entity.add_component(Box::new(CameraComponent::default()));
    engine.add_entity(entity);

    let found = engine.entities().first().and_then(|e| {
        e.iter_components()
            .find_map(|c| c.as_any().downcast_ref::<CameraComponent>().map(|c| (c.main, c.camera)))
    });
    let (main, _) = found.expect("the camera component is discoverable");
    assert!(main, "a fresh camera component defaults to main");
}

// ---- M2: scene save/load from the editor ----

/// The scene-io op the Save button drives: the live tree's canonical
/// document through the carrier, loaded back into another engine —
/// roots, names, and the camera all survive, and the undo stack resets
/// (stored paths moved with the old tree).
#[test]
fn scene_io_round_trips_through_the_engine() {
    let dir = std::env::temp_dir().join(format!("hyg-editor-io-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("level.json.hyg");

    let mut engine = demo_engine();
    engine.set_camera(Camera {
        position: Vec3::new(1.0, 2.0, 3.0),
        target: Vec3::new(0.0, 1.0, 0.0),
        projection: Projection::Perspective {
            fov_y_degrees: 55.0,
            near: 0.2,
            far: 60.0,
            aspect: 1.7,
        },
    });
    let mut ui = EditorUi::new();
    ui.select(vec![0, 0]);
    ui.set_selected_position(&mut engine, Vec3::new(9.0, 9.0, 9.0), 0);

    // save: the M0 canonical document of the live tree
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap();
    let doc = Scene::document(name, engine.camera(), engine.entities());
    hyg::carrier::write(&path, &doc).expect("the carrier writes");

    // load into a fresh engine (the shell's load path)
    let loaded = Scene::load(&path).expect("the scene loads");
    let mut engine_b = Engine::new("acme", "editor-test-b");
    engine_b.set_camera(loaded.camera);
    engine_b.set_roots(loaded.entities);
    ui = EditorUi::new(); // the shell's reset

    assert_eq!(engine_b.entities().len(), 2);
    assert_eq!(
        engine_b.entity_at(&[0, 0]).map(|e| e.name()),
        Some("stack")
    );
    assert_eq!(
        engine_b.entity_at(&[0, 0]).unwrap().transform().position,
        Vec3::new(9.0, 9.0, 9.0),
        "the dragged edit was in the saved document"
    );
    assert_eq!(
        engine_b.entity_at(&[0, 0, 0]).map(|e| e.name()),
        Some("block_3"),
        "the subtree survives"
    );
    assert_eq!(
        engine_b.camera().position,
        Vec3::new(1.0, 2.0, 3.0),
        "the camera rides the document"
    );
    assert!(!ui.undo_stack().can_undo(), "load clears the history");

    std::fs::remove_dir_all(dir).ok();
}

// ---- round 5: the orthographic gizmo is 2D ----

/// Under an orthographic camera the gizmo is the 2D variant: exactly two
/// screen-aligned axes (X right, Y up), no Z handle.
#[test]
fn ortho_gizmo_is_two_screen_aligned_axes() {
    let camera = Camera {
        position: Vec3::new(0.0, 0.0, 0.0),
        target: Vec3::new(0.0, 0.0, -1.0),
        projection: Projection::Orthographic {
            half_extents: Vec3::new(8.0, 4.5, 64.0),
        },
    };
    let view = view();
    let layout = gizmo::layout(
        GizmoMode::Translate,
        &camera,
        &view,
        Vec3::new(0.0, 0.0, 0.0),
        hyg::var::Quat::new(0.0, 0.0, 0.0, 1.0),
        1.4,
    )
    .expect("lays out under ortho");

    assert_eq!(layout.axes.len(), 2, "the 2D gizmo: X and Y only, no Z");
    assert!(layout.axes.iter().all(|h| !h.foreshortened));
    let x = &layout.axes[0];
    let y = &layout.axes[1];
    assert!(x.screen_dir[0] > 0.9, "X points screen-right: {:?}", x.screen_dir);
    assert!(y.screen_dir[1] < -0.9, "Y points screen-up (pixel y down): {:?}", y.screen_dir);

    // uniform window scale: px/unit = viewport / (2 * half_extent)
    let px_x = view.width / (2.0 * 8.0);
    assert!((x.px_per_unit - px_x).abs() < 0.01, "{} vs {px_x}", x.px_per_unit);

    // and the drag chain holds: 30px along X moves the entity 30px right
    let delta = gizmo::translate_delta(x, [30.0, 0.0]);
    let before = pick::project_point(&camera, &view, Vec3::new(0.0, 0.0, 0.0)).unwrap();
    let after = pick::project_point(&camera, &view, Vec3::new(0.0, 0.0, 0.0) + delta).unwrap();
    assert!((after[0] - before[0] - 30.0).abs() < 0.5, "ortho drags map 1:1");
}

// ---- M3: script live reload ----

/// The editor's save-equals-reload seam: rewrite the script file,
/// reload, and the NEXT update runs the new code (state resets — the
/// v12 contract). `--features script`.
#[cfg(feature = "script")]
#[test]
fn live_reload_swaps_script_behavior() {
    use hyg::game::component::Component;
    use hyg::game::scripted::ScriptedComponent;

    let dir = std::env::temp_dir().join(format!("hyg-live-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script_path = dir.join("mover.js");
    std::fs::write(&script_path, "function update(dt) { transform.position.x += dt; }")
        .unwrap();

    let script = hyg::script::Script::new("function update(dt) { transform.position.x += dt; }")
        .expect("compiles");
    let mut component = ScriptedComponent::new(script)
        .expect("arms")
        .with_source_path(script_path.to_string_lossy().to_string());
    let mut transform = hyg::game::Transform::default();

    // seed the shared transform + run one frame: x moves by dt
    component.set_transform(transform);
    component.update(1.0);
    transform = component.transform();
    assert!((transform.position.x - 1.0).abs() < 1e-5, "v1 behavior");

    // rewrite the file: the new script moves z instead
    std::fs::write(
        &script_path,
        "function update(dt) { transform.position.z += dt * 2; }",
    )
    .unwrap();
    component.reload_from_file().expect("reloads");

    // script STATE resets on reload (the v12 contract) — the component's
    // shared transform re-seeds the script's `transform` global
    component.set_transform(transform);
    component.update(1.0);
    let transform = component.transform();
    assert!(
        (transform.position.z - 2.0).abs() < 1e-5,
        "v2 behavior: z moves by 2dt ({:?})",
        transform.position
    );

    std::fs::remove_dir_all(dir).ok();
}

/// A FAILED reload keeps the old script running (a broken save is never
/// a dead component). `--features script`.
#[cfg(feature = "script")]
#[test]
fn failed_reload_keeps_the_old_script() {
    use hyg::game::component::Component;
    use hyg::game::scripted::ScriptedComponent;

    let dir = std::env::temp_dir().join(format!("hyg-live-reload-bad-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script_path = dir.join("mover.js");
    std::fs::write(&script_path, "function update(dt) { transform.position.x += dt; }")
        .unwrap();

    let script = hyg::script::Script::new("function update(dt) { transform.position.x += dt; }")
        .expect("compiles");
    let mut component = ScriptedComponent::new(script)
        .expect("arms")
        .with_source_path(script_path.to_string_lossy().to_string());

    // break the syntax: the reload REFUSES it (the source compiles at
    // the load boundary) — and the component now carries the broken
    // source, so its NEXT hook call fails LOUD. That panic is what the
    // editor's pump catches into the console, dropping to edit mode and
    // keeping the editor alive; a good reload heals it.
    std::fs::write(&script_path, "function {{{").unwrap();
    assert!(
        component.reload_from_file().is_err(),
        "broken source refuses at the boundary"
    );
    let broke = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        component.set_transform(hyg::game::Transform::default());
        component.update(1.0);
    }));
    assert!(broke.is_err(), "the broken script fails loud");

    // the heal: a good reload restores behavior
    std::fs::write(&script_path, "function update(dt) { transform.position.x += dt; }")
        .unwrap();
    assert!(component.reload_from_file().is_ok(), "a good reload heals");
    component.set_transform(hyg::game::Transform::default());
    component.update(1.0);
    let transform = component.transform();
    assert!((transform.position.x - 1.0).abs() < 1e-5, "healed and running");

    std::fs::remove_dir_all(dir).ok();
}

// ---- round 9: the gizmo placement vs THE RENDERER (the real contract) ----

/// THE placement contract, pinned against the renderer's actual pixels:
/// a bright marker rendered at a known world position must appear at
/// exactly the pixel `project_point` predicts. The drag math was self-
/// consistent (project_point on both sides); THIS is the test that
/// would catch a convention mismatch between the editor's math and the
/// render pipeline (transpose, y-flip, aspect).
#[cfg(feature = "editor")]
#[test]
fn gizmo_projection_matches_the_rendered_pixels() {
    use hyg::game::editor::pick::{project_point, Viewport};
    use hyg::game::render_wgpu::WgpuRenderer;
    use hyg::game::render::RenderBackend;

    let camera = pick_camera();
    let (width, height) = (960u32, 540u32);
    let viewport = Viewport::new(0.0, 0.0, width as f32, height as f32);
    let world = Vec3::new(1.2, 0.8, 0.0); // off-center, so the test has teeth

    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(width, height))
        .expect("a wgpu adapter for the pixel test");
    renderer.begin_scene(&camera, &[]);
    // a MESH marker (red cube) — 3D scenes render through the mesh/VP
    // path, so the placement contract pins to THAT path (the quad
    // pipeline maps x differently under perspective cameras — the two
    // paths are the known 2D/3D split)
    let model = std::sync::Arc::new(hyg::game::Model::single(std::sync::Arc::new(
        hyg::game::Mesh::cube(Vec3::new(0.08, 0.08, 0.08)),
    )));
    renderer.push_model(
        &hyg::game::Transform {
            position: world,
            ..hyg::game::Transform::default()
        },
        &model,
        [2.5, 0.1, 0.1, 1.0], // bright red over the 0.45 ambient
    );
    renderer.render_frame();

    // find the red marker's pixel centroid in the readback
    let shot = renderer.screenshot();
    assert_eq!(shot.len(), (width * height * 4) as usize, "rgba readback");
    let mut sum_x = 0.0f64;
    let mut sum_y = 0.0f64;
    let mut count = 0u64;
    for py in 0..height as usize {
        for px in 0..width as usize {
            let i = (py * width as usize + px) * 4;
            let (r, g, b) = (shot[i], shot[i + 1], shot[i + 2]);
            if r > 140 && g < 80 && b < 80 {
                sum_x += px as f64;
                sum_y += py as f64;
                count += 1;
            }
        }
    }
    assert!(count > 4, "the marker rendered: {count} red pixels");
    let rendered = [(sum_x / count as f64) as f32, (sum_y / count as f64) as f32];

    let predicted = project_point(&camera, &viewport, world).expect("in front");
    assert!(
        (rendered[0] - predicted[0]).abs() < 3.0 && (rendered[1] - predicted[1]).abs() < 3.0,
        "project_point predicted {predicted:?}, the renderer drew {rendered:?}"
    );
}

// ---- round 6: the camera component defaults ----

/// A fresh camera component is the 3D default (perspective, framed on
/// the origin) and defaults to main — the new-scene contract.
#[test]
fn camera_component_defaults_to_3d_and_main() {
    use hyg::game::CameraComponent;

    let camera = CameraComponent::default();
    assert!(
        matches!(
            camera.camera.projection,
            Projection::Perspective { .. }
        ),
        "new scenes default to 3D"
    );
    assert!(camera.main);
    assert_eq!(camera.camera.position, Vec3::new(0.0, 0.0, -8.0));
}

// ---- round 8: the pick ray must round-trip through the projection ----

/// pick_ray(project_point(p)) must be the ray through p — the pick and
/// the placement share one convention. This pins it at (4, 0, 2): the
/// click through the projected point must pass through the point.
#[test]
fn pick_ray_round_trips_through_the_projection() {
    let camera = pick_camera();
    let view = view();
    let point = Vec3::new(4.0, 0.0, 2.0);

    let px = pick::project_point(&camera, &view, point).expect("projects");
    let ray = pick::pick_ray(&camera, &view, px).expect("ray");

    // the ray's closest approach to the point
    let to_point = point - ray.origin;
    let t = to_point.dot(ray.dir);
    let closest = ray.origin + ray.dir.scaled(Vec3::splat(t));
    let miss = (closest - point).length();
    assert!(miss < 0.05, "the ray misses the point by {miss}");
}

// ---- round 7: the gizmo respects zoom ----

/// The gizmo's handles are true MVP projections of world points: zoom
/// in (small orbit distance) and the handles grow in pixels; zoom out
/// and they shrink. The review's ask — the gizmo scales with the
/// geometry instead of floating as a fixed screen overlay.
#[test]
fn gizmo_handles_respect_zoom() {
    let view = view();
    let identity = hyg::var::Quat::new(0.0, 0.0, 0.0, 1.0);

    let handle_len = |distance: f32| {
        let camera = Camera {
            position: Vec3::new(0.0, 0.0, -distance),
            target: Vec3::new(0.0, 0.0, 0.0),
            projection: Projection::Perspective {
                fov_y_degrees: 60.0,
                near: 0.1,
                far: 500.0,
                aspect: 16.0 / 9.0,
            },
        };
        let layout = gizmo::layout(
            GizmoMode::Translate,
            &camera,
            &view,
            Vec3::new(0.0, 0.0, 0.0),
            identity,
            gizmo::GIZMO_WORLD_LEN, // world-locked: zoom rescales it
        )
        .expect("lays out");
        let x = &layout.axes[0];
        let dx = x.screen_end[0] - layout.center[0];
        let dy = x.screen_end[1] - layout.center[1];
        (dx * dx + dy * dy).sqrt()
    };

    let close = handle_len(3.0);
    let far = handle_len(30.0);
    assert!(close > far, "zoomed in = bigger handles: {close} vs {far}");
    // perspective projection: ~10x distance ≈ ~10x smaller
    let ratio = close / far;
    assert!(
        (ratio - 10.0).abs() < 3.0,
        "10x closer = ~10x bigger (perspective): ratio {ratio}"
    );
}

// ---- round 5: the inspector growth probe ----

/// THE ballooning bug, reproduced against the REAL panel: a widget
/// desiring available+k px per frame grows the resizable panel a few px
/// at a time until its cap (~3s of frames). This drives the actual
/// inspector panel headlessly for 240 frames with an entity selected
/// and asserts the width stays put.
#[cfg(feature = "editor")]
#[test]
fn inspector_panel_does_not_grow_over_time() {
    use hyg::game::editor::shell::{draw_inspector_panel, EditorState, ShellShared};

    let mut engine = demo_engine();
    let mut state = EditorState::new(hyg::game::Camera::default());
    state.ui.select(vec![0, 0]); // stack (physics + a child with a renderer)
    let shared = ShellShared {
        playing: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        dirty: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        quit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        frame_size: std::sync::Arc::new(std::sync::Mutex::new([960, 540])),
        pip_camera: std::sync::Arc::new(std::sync::Mutex::new(None)),
        console: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        ai_busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    let ctx = egui::Context::default();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 800.0));
    let mut widths = Vec::new();
    for _ in 0..240 {
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(screen),
            time: Some(0.0),
            ..egui::RawInput::default()
        });
        let mut output = ctx.end_pass();
        // consume texture deltas (egui asserts at drop otherwise)
        let _ = std::mem::take(&mut output.textures_delta.set);
        let _ = std::mem::take(&mut output.textures_delta.free);
        // the panel needs the pass ALIVE while drawing — redo properly:
        // begin, draw into root_ui, end
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(screen),
            time: Some(0.0),
            ..egui::RawInput::default()
        });
        let width = ctx.root_ui(|root| draw_inspector_panel(root, &mut state, &mut engine, &shared));
        let mut output = ctx.end_pass();
        let _ = std::mem::take(&mut output.textures_delta.set);
        let _ = std::mem::take(&mut output.textures_delta.free);
        widths.push(width);
    }
    let first = widths[10];
    let last = *widths.last().unwrap();
    assert!(
        (last - first).abs() < 2.0,
        "the inspector must not grow over time: frame 10 = {first}, frame 240 = {last}"
    );
}

// ---- round 4: reparenting ----

/// Drag-to-reparent: subtrees move (children ride along), cycles are
/// refused, and sibling indices survive the removal shift.
#[test]
fn reparent_moves_subtrees_and_refuses_cycles() {
    let mut engine = demo_engine();
    // tree: world > stack > block_3, plus root "free"
    assert!(engine.reparent(&[1], &[0]), "free -> world"); // free under world
    let free = engine.entity_at(&[0, 1]).expect("free moved under world");
    assert_eq!(free.name(), "free");
    assert_eq!(
        engine.entity_at(&[0, 0]).map(|e| e.name()),
        Some("stack"),
        "stack unshifted? no — free appended AFTER stack: stack stays [0,0]"
    );

    // children ride along: stack (carrying block_3) moves under free
    assert!(engine.reparent(&[0, 0], &[0, 1]), "stack (with block_3) -> free");
    let free = engine.entity_at(&[0, 0]).expect("free shifted to [0,0] after stack left");
    assert_eq!(free.children().len(), 1, "stack rode along");
    assert_eq!(free.children()[0].name(), "stack");
    assert_eq!(
        free.children()[0].children()[0].name(),
        "block_3",
        "the grandchild rode along too"
    );

    // cycles refused: world under its own descendant
    assert!(!engine.reparent(&[0], &[0, 0, 0]), "no cycles");
    assert!(engine.entity_at(&[0]).is_some(), "world survived the refusal");
}

// ---- round 4: end-to-end gizmo direction ----

/// THE reversal hunt, end to end: dragging 30px along the X handle's
/// screen arrow must move the entity 30px along that same screen
/// direction — the full layout→delta→project chain, no links missing.
/// (If this passes, on-screen reversal is a VIEW-convention matter, not
/// math — see the orbit default-yaw note in camera_ctl.)
#[test]
fn gizmo_drag_moves_the_entity_along_the_arrow() {
    let camera = pick_camera();
    let view = view();
    let origin = Vec3::new(0.0, 0.0, 0.0);
    let identity = hyg::var::Quat::new(0.0, 0.0, 0.0, 1.0);
    let layout =
        gizmo::layout(GizmoMode::Translate, &camera, &view, origin, identity, 1.4)
            .expect("lays out");

    for handle in &layout.axes {
        let drag = [handle.screen_dir[0] * 30.0, handle.screen_dir[1] * 30.0];
        let delta = gizmo::translate_delta(handle, drag);
        if handle.foreshortened {
            // an axis pointing at the lens cannot move on screen — but
            // the CLAMP must bound it (a raw floor = a 600-unit launch)
            let healthiest = layout
                .axes
                .iter()
                .map(|h| h.px_per_unit)
                .fold(0.0f32, f32::max);
            let bound = 30.0 / (healthiest * 0.15);
            assert!(
                delta.length() <= bound + 0.01,
                "axis {}: foreshortened drags stay bounded ({} <= {bound})",
                handle.axis,
                delta.length()
            );
            continue;
        }
        let before = pick::project_point(&camera, &view, origin).expect("projects");
        let after =
            pick::project_point(&camera, &view, origin + delta).expect("projects");
        let moved = [after[0] - before[0], after[1] - before[1]];
        let dot = moved[0] * handle.screen_dir[0] + moved[1] * handle.screen_dir[1];
        assert!(
            dot > 20.0,
            "axis {}: a 30px drag along the arrow must move the entity \u{2248}30px the SAME way (moved {:?}, dir {:?})",
            handle.axis,
            moved,
            handle.screen_dir
        );
    }
}

// ---- round 3: the editor camera + the view basis ----

/// The view basis's `right` matches the SCREEN's right: projecting a
/// step along it lands at larger pixel x. (The forward×up order built
/// the LEFT vector and inverted every view-plane drag — the gold center
/// square moved opposite the cursor.)
#[test]
fn view_basis_right_matches_the_screen() {
    use hyg::game::editor::gizmo::view_basis;

    let camera = pick_camera();
    let (_, right, up) = view_basis(&camera);
    let view = view();
    let origin_px = pick::project_point(&camera, &view, Vec3::new(0.0, 0.0, 0.0))
        .expect("origin projects");
    let right_px = pick::project_point(&camera, &view, right)
        .expect("the step stays in front");
    assert!(
        right_px[0] > origin_px[0],
        "a step along `right` must move screen-x positive: {origin_px:?} -> {right_px:?}"
    );
    // and up moves screen-y NEGATIVE (pixel y grows down)
    let up_px = pick::project_point(&camera, &view, up).expect("projects");
    assert!(
        up_px[1] < origin_px[1],
        "a step along `up` must move screen-y negative: {origin_px:?} -> {up_px:?}"
    );
}

/// The orbit camera's default looks at its target from above/behind
/// along +z (the scenes' build axis): target at clip center, eye on the
/// -z side, elevated.
#[test]
fn orbit_default_frames_the_target() {
    use hyg::game::editor::camera_ctl::Orbit;

    let orbit = Orbit::default();
    let camera = orbit.camera(16.0 / 9.0);
    // the view basis's right/up contract carries to the orbit camera too
    let (_, right, up) = gizmo::view_basis(&camera);
    let view = Viewport::new(0.0, 0.0, 960.0, 540.0);
    let center = pick::project_point(&camera, &view, orbit.target).expect("target projects");
    let right_px = pick::project_point(&camera, &view, orbit.target + right).expect("projects");
    let up_px = pick::project_point(&camera, &view, orbit.target + up).expect("projects");
    assert!(right_px[0] > center[0], "orbit right = screen right");
    assert!(up_px[1] < center[1], "orbit up = screen up");
    // the default view: from +z looking −z (world +x renders
    // screen-RIGHT — the gizmo convention), elevated above the target
    assert!(camera.position.z > orbit.target.z);
    assert!(camera.position.y > orbit.target.y);
    let x_px = pick::project_point(&camera, &view, orbit.target + Vec3::new(1.0, 0.0, 0.0))
        .expect("projects");
    assert!(x_px[0] > center[0], "world +x renders screen-RIGHT by default");

    // orbit_by pans the view around; zoom_by dollies in
    let mut orbit = Orbit::default();
    let d0 = orbit.distance;
    orbit.zoom_by(100.0); // wheel up = zoom in
    assert!(orbit.distance < d0);
    // drag right = the eye moves along the camera's CURRENT screen-right
    // (view-invariant — a world-frame assertion flips with yaw and once
    // enshrined the wrong sign)
    let before = orbit.camera(16.0 / 9.0);
    let (_, right, _) = gizmo::view_basis(&before);
    orbit.orbit_by(100.0, 0.0);
    let after = orbit.camera(16.0 / 9.0);
    let eye_delta = after.position - before.position;
    let along_right = eye_delta.dot(right);
    assert!(along_right > 0.0, "drag right moves the eye screen-right");
}

// ---- M2 fix round: the history panel's timeline ----

/// The history panel: entries describe the timeline (applied vs
/// un-done), and clicking a row travels there — undoing or redoing as
/// many commands as it takes.
#[test]
fn history_panel_travels_the_timeline() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![1]);
    let start = engine.entity_at(&[1]).unwrap().transform().position;

    // three deliberate edits (outside the coalescing window)
    ui.set_selected_position(&mut engine, start + Vec3::new(1.0, 0.0, 0.0), 0);
    ui.set_selected_position(&mut engine, start + Vec3::new(2.0, 0.0, 0.0), 1_000);
    ui.set_selected_position(&mut engine, start + Vec3::new(3.0, 0.0, 0.0), 2_000);

    // undo the last one: 2 applied, 1 un-done
    assert!(ui.undo(&mut engine));
    let history = ui.undo_stack().history(&engine);
    assert_eq!(history.len(), 3, "the whole timeline shows");
    assert_eq!(ui.undo_stack().cursor(), 2);
    assert!(history[0].done && history[1].done, "applied rows");
    assert!(!history[2].done, "the un-done row dims");
    assert!(
        history[0].label.starts_with("move free"),
        "labels describe: {:?}",
        history[0].label
    );

    // click the FIRST row: travel back to just after edit #1
    assert!(ui.undo_stack_mut().goto(history[0].timeline_index, &mut engine));
    assert_eq!(
        engine.entity_at(&[1]).unwrap().transform().position,
        start + Vec3::new(1.0, 0.0, 0.0),
        "travel to timeline index 1 = after the first edit"
    );

    // click the LAST row: travel forward to the newest state
    assert!(ui
        .undo_stack_mut()
        .goto(history[2].timeline_index, &mut engine));
    assert_eq!(
        engine.entity_at(&[1]).unwrap().transform().position,
        start + Vec3::new(3.0, 0.0, 0.0),
        "travel forward redoes through the newest edit"
    );
}

/// A delete's history row names the deleted entity (the subtree rides
/// in the command, out of the engine).
#[test]
fn history_names_deleted_entities() {
    let mut engine = demo_engine();
    let mut ui = EditorUi::new();
    ui.select(vec![0, 0]);
    assert!(ui.delete_selected(&mut engine, 0));

    let history = ui.undo_stack().history(&engine);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].label, "delete stack", "{}", history[0].label);
}

/// The file dialog repairs a save name into a `.hyg` carrier path.
#[cfg(feature = "editor")]
#[test]
fn hyg_suffix_repair() {
    use hyg::game::editor::shell::ensure_hyg_suffix;
    assert_eq!(ensure_hyg_suffix("level"), "level.json.hyg");
    assert_eq!(ensure_hyg_suffix("level.json"), "level.json.hyg");
    assert_eq!(ensure_hyg_suffix("level.json.hyg"), "level.json.hyg");
    assert_eq!(ensure_hyg_suffix("dense.cbor"), "dense.cbor.json.hyg");
}

// ---- M2: gizmo rendering (geometry-level assertions) ----

/// The gizmo's paint pass produces real tessellated geometry inside the
/// viewport — the rendering assertion at the geometry level (the pixel
/// pass composites it in the shell; `--features editor`).
#[cfg(feature = "editor")]
#[test]
fn gizmo_paint_tessellates_in_the_viewport() {
    use hyg::game::editor::shell;

    let ctx = egui::Context::default();
    let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 500.0));
    ctx.begin_pass(egui::RawInput {
        screen_rect: Some(rect),
        time: Some(0.0),
        ..egui::RawInput::default()
    });

    let camera = pick_camera();
    let view = view();
    let layout = gizmo::layout(
        GizmoMode::Translate,
        &camera,
        &view,
        Vec3::new(0.0, 0.0, 0.0),
        Quat::new(0.0, 0.0, 0.0, 1.0),
        1.4,
    )
    .expect("lays out");
    shell::paint_gizmo(ctx.debug_painter(), &layout, GizmoMode::Translate, 1.0, false);

    let mut output = ctx.end_pass();
    // egui asserts at drop when texture deltas go unhandled — consume
    // them (the shell feeds them to the renderer; the test only cares
    // about geometry)
    let _ = std::mem::take(&mut output.textures_delta.set);
    let _ = std::mem::take(&mut output.textures_delta.free);
    let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    let vertices: usize = primitives
        .iter()
        .filter_map(|p| match &p.primitive {
            egui::epaint::Primitive::Mesh(mesh) => Some(mesh.vertices.len()),
            _ => None,
        })
        .sum();
    // three axis segments + three arrowheads + the center square: the
    // mesher merges same-material geometry, so the COUNT is in meshes —
    // the VERTEX count is the honest measure
    assert!(
        vertices >= 20,
        "axes + arrowheads + center square tessellate: {vertices} vertices across {} primitives",
        primitives.len()
    );
    // every vertex lands inside the viewport (the gizmo hugs its center,
    // and the center projects inside)
    for primitive in &primitives {
        let mesh = match &primitive.primitive {
            egui::epaint::Primitive::Mesh(mesh) => mesh,
            _ => continue,
        };
        for vertex in &mesh.vertices {
            assert!(
                vertex.pos.x >= view.x - 1.0
                    && vertex.pos.x <= view.x + view.width + 1.0
                    && vertex.pos.y >= view.y - 1.0
                    && vertex.pos.y <= view.y + view.height + 1.0,
                "gizmo geometry escapes the viewport: {:?}",
                vertex.pos
            );
        }
    }
}

// ---- glass chrome paint ----

/// Paint one closure into its own headless pass — the gizmo paint
/// test's pipeline (begin pass, paint, end pass, consume the texture
/// deltas, tessellate) wrapped for the glass assertions below. Returns
/// the pass's SHAPES and the tessellated primitives. The shape count is
/// the honest LAYER count: egui coalesces same-texture shapes into one
/// mesh at tessellation time, so primitive counts say nothing about how
/// many layers were painted.
#[cfg(feature = "editor")]
fn tessellate_paint(
    paint: impl FnOnce(&egui::Painter),
) -> (Vec<egui::epaint::ClippedShape>, Vec<egui::ClippedPrimitive>) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(400.0, 300.0),
        )),
        time: Some(0.0),
        ..egui::RawInput::default()
    });
    paint(&ctx.debug_painter());
    let mut output = ctx.end_pass();
    // egui asserts at drop when texture deltas go unhandled — consume
    // them (the shell feeds them to the renderer; the test only cares
    // about geometry)
    let _ = std::mem::take(&mut output.textures_delta.set);
    let _ = std::mem::take(&mut output.textures_delta.free);
    let primitives = ctx.tessellate(output.shapes.clone(), output.pixels_per_point);
    (output.shapes, primitives)
}

/// The glass look is LAYERED: body, vertical fade, border, specular,
/// label — strictly more painted shapes than the flat body's fill +
/// border + label. The assertion is on the shape count (one shape per
/// painted layer), the honest geometry.
#[cfg(feature = "editor")]
#[test]
fn glass_button_paints_more_layers_than_flat() {
    use hyg::game::editor::style::{self, ButtonState};

    let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(64.0, 24.0));
    let (glass_shapes, _) = tessellate_paint(|p| {
        style::paint_glass_button(p, rect, "play", style::PLAY_GREEN, ButtonState::Idle, true);
    });
    let (flat_shapes, _) = tessellate_paint(|p| {
        style::paint_flat_button(p, rect, "play", style::PLAY_GREEN, ButtonState::Idle, true);
    });
    assert!(
        glass_shapes.len() > flat_shapes.len(),
        "glass paints more layers than flat: {} vs {}",
        glass_shapes.len(),
        flat_shapes.len()
    );
}

/// Hover paints the fake glow — concentric outline layers that live
/// OUTSIDE the button body (egui has no blur; the halo is real
/// geometry). The hovered paint's vertices must reach past the rect;
/// the idle paint's must not.
#[cfg(feature = "editor")]
#[test]
fn hovered_glass_paints_a_glow_beyond_the_body() {
    use hyg::game::editor::style::{self, ButtonState};

    let rect = egui::Rect::from_min_size(egui::pos2(30.0, 30.0), egui::vec2(64.0, 24.0));
    let (idle_shapes, idle) = tessellate_paint(|p| {
        style::paint_glass_button(p, rect, "stop", style::STOP_RED, ButtonState::Idle, true);
    });
    let (hovered_shapes, hovered) = tessellate_paint(|p| {
        style::paint_glass_button(p, rect, "stop", style::STOP_RED, ButtonState::Hovered, true);
    });
    assert!(
        hovered_shapes.len() > idle_shapes.len(),
        "hover adds glow outline layers: {} vs {}",
        hovered_shapes.len(),
        idle_shapes.len()
    );

    let bounds = |primitives: &[egui::ClippedPrimitive]| {
        let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for primitive in primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = &primitive.primitive {
                for vertex in &mesh.vertices {
                    min.x = min.x.min(vertex.pos.x);
                    min.y = min.y.min(vertex.pos.y);
                    max.x = max.x.max(vertex.pos.x);
                    max.y = max.y.max(vertex.pos.y);
                }
            }
        }
        (min, max)
    };
    let (hovered_min, hovered_max) = bounds(&hovered);
    assert!(
        hovered_min.x <= rect.left() - 2.0 && hovered_max.x >= rect.right() + 2.0,
        "hover glow reaches outside the body: {:?}..{:?} vs {}..{}",
        hovered_min.x,
        hovered_max.x,
        rect.left(),
        rect.right()
    );
    let (idle_min, idle_max) = bounds(&idle);
    assert!(
        idle_min.x >= rect.left() - 1.0 && idle_max.x <= rect.right() + 1.0,
        "idle paint stays inside the body: {:?}..{:?}",
        idle_min.x,
        idle_max.x
    );
}

/// The label sits INSIDE the glass: every paint helper (both styles,
/// both themes, idle and engaged) must return a label rect contained
/// in the button rect — the "text inside the glass" contract.
#[cfg(feature = "editor")]
#[test]
fn button_labels_land_inside_the_body() {
    use hyg::game::editor::style::{self, ButtonState};

    let rect = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(72.0, 26.0));
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(200.0, 100.0),
        )),
        time: Some(0.0),
        ..egui::RawInput::default()
    });
    let painter = ctx.debug_painter();
    let mut text_rects = Vec::new();
    for accent in [
        style::PLAY_GREEN,
        style::STOP_RED,
        style::RESTART_ORANGE,
        style::ACCENT_DIM,
    ] {
        for dark in [true, false] {
            text_rects.push(style::paint_glass_button(
                &painter,
                rect,
                "step",
                accent,
                ButtonState::Idle,
                dark,
            ));
            text_rects.push(style::paint_glass_button(
                &painter,
                rect,
                "step",
                accent,
                ButtonState::Active,
                dark,
            ));
            text_rects.push(style::paint_flat_button(
                &painter,
                rect,
                "step",
                accent,
                ButtonState::Hovered,
                dark,
            ));
        }
    }
    let mut output = ctx.end_pass();
    let _ = std::mem::take(&mut output.textures_delta.set);
    let _ = std::mem::take(&mut output.textures_delta.free);
    for text_rect in text_rects {
        assert!(
            rect.contains_rect(text_rect),
            "label escapes the glass: {text_rect:?} not inside {rect:?}"
        );
    }
}

// ---- M4: the model import flow ----

/// Import copies the model into the active scene's assets/ tree, spawns
/// a root entity carrying an asset-backed renderer (path relative to the
/// scene file), and selects it. Under the gltf feature the copy is
/// parse-checked and the renderer materializes immediately.
#[cfg(all(feature = "editor", feature = "gltf"))]
#[test]
fn import_model_copies_spawns_and_selects() {
    use hyg::game::editor::shell::{import_model, EditorState};
    use std::path::Path;

    let dir = std::env::temp_dir().join("hyg-import-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scene dir");

    let mut state = EditorState::new(Camera::default());
    state.scene_path = dir.join("scene.json.hyg").to_string_lossy().to_string();
    let mut engine = Engine::new("acme", "import-test");

    // the "picked" file lives OUTSIDE the scene tree — import COPIES it in
    let src = dir.join("external").join("cube.gltf");
    std::fs::create_dir_all(src.parent().expect("parent")).expect("src dir");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/cube.gltf"),
        &src,
    )
    .expect("seed the external model");

    let name = import_model(&mut state, &mut engine, &src).expect("imports");
    assert_eq!(name, "cube.gltf");

    // the copy landed in the scene's assets/
    assert!(dir.join("assets/cube.gltf").exists(), "copied into assets/");

    // the entity: last root, named after the file, selected, resolved
    let idx = engine.entities().len() - 1;
    assert_eq!(state.ui.selection(), Some(&[idx][..]), "the import selects");
    let entity = engine.entity_at(&[idx]).expect("spawned");
    assert_eq!(entity.name(), "cube");
    let renderer = entity
        .iter_components()
        .find_map(|c| c.as_any().downcast_ref::<MeshRenderer>())
        .expect("carries the renderer");
    assert_eq!(renderer.asset_path(), Some("assets/cube.gltf"));
    assert!(
        matches!(
            renderer.geometry,
            MeshGeometry::Asset { model: Some(_), .. }
        ),
        "materialized immediately — no reload needed"
    );
}

/// A broken model refuses the import LOUDLY and lands nothing: no copy
/// reaches assets/, no entity spawns, the selection is untouched.
#[cfg(all(feature = "editor", feature = "gltf"))]
#[test]
fn import_model_refuses_a_broken_file() {
    use hyg::game::editor::shell::{import_model, EditorState};

    let dir = std::env::temp_dir().join("hyg-import-broken");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scene dir");

    let mut state = EditorState::new(Camera::default());
    state.scene_path = dir.join("scene.json.hyg").to_string_lossy().to_string();
    let mut engine = Engine::new("acme", "import-broken");

    let src = dir.join("junk.gltf");
    std::fs::write(&src, "{ not a gltf document").expect("write junk");

    let err = import_model(&mut state, &mut engine, &src).expect_err("refuses");
    assert!(err.contains("junk.gltf"), "names the path: {err}");
    assert!(
        !dir.join("assets").exists(),
        "the parse runs before the copy lands (nothing in assets/)"
    );
    assert!(engine.entities().is_empty(), "no entity spawned");
    assert_eq!(state.ui.selection(), None, "selection untouched");
}

// ---- the rainbow beam ----

/// The wheel turns while processing: the same segment's color differs
/// across the phase (the shell advances the clock only while the sim
/// runs or the AI seam is busy — at rest the ribbon holds still, and
/// the sparkle collapses to its resting level).
#[cfg(feature = "editor")]
#[test]
fn beam_hues_rotate_with_the_processing_clock() {
    use hyg::game::editor::style::beam_color;
    let a = beam_color(8.0, 64.0, 0.0, true);
    let b = beam_color(8.0, 64.0, 0.5, true);
    assert!(a != b, "the hues rotate with the clock");

    // the hot spots rest when nothing is processing
    let hot = hyg::game::editor::style::beam_sparkle(9.0, 64.0, 4.0, true);
    let rest = hyg::game::editor::style::beam_sparkle(9.0, 64.0, 4.0, false);
    assert_eq!(rest, 0.0, "idle = no sparkle lift");
    assert!(hot > rest, "processing blazes above the rest state");
}

/// The hot spots are a deterministic random pattern: same inputs give
/// the same intensity, segments differ, and most segments stay calm —
/// a few blaze, not all.
#[cfg(feature = "editor")]
#[test]
fn beam_sparkle_is_deterministic_and_localized() {
    use hyg::game::editor::style::beam_sparkle;
    assert_eq!(
        beam_sparkle(7.0, 64.0, 1.25, true),
        beam_sparkle(7.0, 64.0, 1.25, true),
        "deterministic"
    );
    let hot = (0..64)
        .map(|i| beam_sparkle(i as f32, 64.0, 2.0, true))
        .filter(|s| *s > 0.7)
        .count();
    assert!(hot < 48, "the hot spots stay sparse ({hot}/64)");
    assert!(hot > 0, "something actually blazes at this phase");
}

/// The restart bug (round 13): hitting restart repeatedly STRIPPED the
/// scene — the play snapshot was file-shaped, and compiled models are
/// runtime-only in files, so restore rebuilt entities with no render
/// components (nothing drawn, inspector intact). The lossless snapshot
/// keeps them: restart cycles preserve every renderer, the entity
/// count, and the snapshot poses.
#[cfg(all(feature = "editor", feature = "physics"))]
#[test]
fn restart_cycles_preserve_the_scene() {
    use hyg::game::editor::shell::{play, restart, stop, EditorState, ShellShared};
    use hyg::game::Model;
    use std::sync::Arc;

    let mut engine = Engine::new("acme", "restart-test");
    let mut floor = Entity::new("floor");
    floor.add_component(Box::new(MeshRenderer::model(
        Arc::new(Model::single(Arc::new(Mesh::cube(Vec3::new(6.0, 0.5, 6.0))))),
        [0.30, 0.34, 0.40, 1.0],
    )));
    engine.add_entity(floor);
    let mut ball = Entity::new("ball");
    ball.transform_mut().position = Vec3::new(0.0, 8.0, 0.0);
    ball.add_component(Box::new(PhysicsBody::new("ball", Shape::Sphere(0.5))));
    ball.add_component(Box::new(MeshRenderer::model(
        Arc::new(Model::single(Arc::new(Mesh::sphere(0.5, 24, 16)))),
        [1.0, 0.4, 0.1, 1.0],
    )));
    engine.add_entity(ball);

    let mut state = EditorState::new(hyg::game::Camera::default());
    let shared = ShellShared {
        playing: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        dirty: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        quit: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        frame_size: std::sync::Arc::new(std::sync::Mutex::new([960, 540])),
        pip_camera: std::sync::Arc::new(std::sync::Mutex::new(None)),
        console: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        ai_busy: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    // play, let the sim mutate (the ball drops), then restart hard
    play(&mut state, &mut engine, &shared);
    for _ in 0..10 {
        engine.update(1.0 / 60.0);
    }
    restart(&mut state, &mut engine, &shared);
    restart(&mut state, &mut engine, &shared);
    stop(&mut state, &mut engine, &shared);

    assert_eq!(engine.entities().len(), 2, "no duplication across cycles");
    for name in ["floor", "ball"] {
        let entity = engine
            .entities()
            .iter()
            .find(|e| e.name() == name)
            .unwrap_or_else(|| panic!("{name} survived"));
        let renderer = entity
            .iter_components()
            .find_map(|c| c.as_any().downcast_ref::<MeshRenderer>())
            .unwrap_or_else(|| panic!("{name} kept its renderer"));
        assert!(
            matches!(renderer.geometry, MeshGeometry::Model { .. }),
            "{name} renders through the compiled model"
        );
    }
    // the ball restored to its SNAPSHOT pose (play had dropped it)
    let ball = engine
        .entities()
        .iter()
        .find(|e| e.name() == "ball")
        .expect("checked");
    assert_eq!(
        ball.transform().position,
        Vec3::new(0.0, 8.0, 0.0),
        "restore returns the snapshot pose"
    );
}

// ---- M5: the fly camera ----

/// Fly thrust along the view axes at the DEFAULT pose (yaw π = looking
/// −z from +z, world +x = screen right): forward moves −z, strafe moves
/// +x, up moves +y — screen-relative like every editor trains.
#[cfg(feature = "editor")]
#[test]
fn fly_thrust_is_screen_relative() {
    use hyg::game::editor::camera_ctl::Fly;
    // LEVEL (pitch 0): the axis assertions are then exact — the default
    // fly has a 0.15 pitch whose cosine scales the ground-plane thrust
    let mut fly = Fly {
        pitch: 0.0,
        ..Fly::default()
    };
    assert_eq!(fly.yaw, std::f32::consts::PI, "the default looks −z");

    fly.thrust(1.0, 0.0, 0.0, 1.0); // 1s forward
    assert!(
        (fly.position.z - (9.0 - fly.speed)).abs() < 1e-4,
        "forward = −z at the default pose: {:?}",
        fly.position
    );

    fly.thrust(0.0, 1.0, 0.0, 1.0); // 1s right
    assert!(
        (fly.position.x - fly.speed).abs() < 1e-4,
        "strafe = +x (screen right): {:?}",
        fly.position
    );

    fly.thrust(0.0, 0.0, 1.0, 1.0); // 1s up
    assert!(
        (fly.position.y - (2.0 + fly.speed)).abs() < 1e-4,
        "up = +y: {:?}",
        fly.position
    );
}

/// Look turns the heading; thrust then follows the NEW forward (a
/// 180° look reverses forward's world direction), and the pitch clamps
/// before the poles.
#[cfg(feature = "editor")]
#[test]
fn fly_look_then_thrust_follows_the_heading() {
    use hyg::game::editor::camera_ctl::Fly;
    // LEVEL: exact axis math (look's sensitivity: 0.005 rad/px — a
    // yaw π drag is π/0.005 ≈ 628 px, flipping the heading 180°)
    let mut fly = Fly {
        pitch: 0.0,
        ..Fly::default()
    };
    let speed = fly.speed;

    fly.look(std::f32::consts::PI / 0.005, 0.0);
    fly.thrust(1.0, 0.0, 0.0, 1.0);
    assert!(
        (fly.position.z - (9.0 + speed)).abs() < 1e-4,
        "a 180° look flipped forward: thrust moves +z now"
    );

    // pitch clamps before the pole
    let mut steep = Fly::default();
    for _ in 0..40 {
        steep.look(0.0, 100.0);
    }
    assert!(
        steep.pitch.abs() <= 1.54,
        "pitch clamps near the poles: {}",
        steep.pitch
    );
}

/// The fly's camera looks down its heading: target = eye + forward,
/// and `from_camera` round-trips a pose (eye kept, heading recovered).
#[cfg(feature = "editor")]
#[test]
fn fly_camera_looks_down_the_heading() {
    use hyg::game::editor::camera_ctl::Fly;
    let fly = Fly {
        position: Vec3::new(3.0, 2.0, 7.0),
        yaw: 1.2,
        pitch: 0.3,
        speed: 6.0,
    };
    let camera = fly.camera(16.0 / 9.0);
    assert_eq!(camera.position, fly.position, "eye kept");
    let forward = (camera.target - camera.position).normalize();
    let expected = Vec3::new(
        fly.yaw.sin() * fly.pitch.cos(),
        -fly.pitch.sin(),
        fly.yaw.cos() * fly.pitch.cos(),
    );
    assert!(
        (forward - expected).length() < 1e-4,
        "the camera aims down the heading: {forward:?} vs {expected:?}"
    );

    // from_camera recovers the same heading from a plain camera
    let round = Fly::from_camera(&camera);
    assert!((round.yaw - fly.yaw).abs() < 1e-3, "yaw recovered");
    assert!((round.pitch - fly.pitch).abs() < 1e-3, "pitch recovered");
    assert_eq!(round.position, fly.position, "position kept");
}

/// The editor's export: the WHOLE flow inside the Studio — bundle the
/// saved scene, fuse it onto a runtime binary, land ONE game file
/// beside the scene. A missing scene or runtime fails with the fix in
/// the message (the approachable contract).
#[cfg(all(feature = "editor", feature = "pack"))]
#[test]
fn export_game_bundles_and_fuses() {
    use hyg::game::editor::shell::{export_game, EditorState};

    let dir = temp_dir("hyg-export-test");
    std::fs::create_dir_all(dir.join("scripts")).expect("dirs");
    std::fs::write(dir.join("scripts/patrol.js"), "function update(dt) {}").expect("script");

    // the scene SAVED beside its script (export packages the saved file)
    let mut drifter = Entity::new("drifter");
    drifter.add_component(Box::new(MeshRenderer::model(
        std::sync::Arc::new(Model::single(std::sync::Arc::new(Mesh::sphere(0.25, 16, 12)))),
        [0.4, 0.9, 0.6, 1.0],
    )));
    let script = hyg::script::Script::new(
        std::fs::read_to_string(dir.join("scripts/patrol.js")).expect("read"),
    )
    .expect("compiles");
    drifter.add_component(Box::new(
        hyg::game::ScriptedComponent::new(script)
            .expect("arms")
            .with_source_path("scripts/patrol.js"),
    ));
    let scene = Scene::snapshot_document("export-me", &hyg::game::Camera::default(), &[drifter]);
    hyg::carrier::write(&dir.join("my-game.json.hyg"), &scene).expect("saves");

    let mut state = EditorState::new(hyg::game::Camera::default());
    state.scene_path = dir.join("my-game.json.hyg").to_string_lossy().to_string();

    // a "runtime" binary: fuse accepts any bytes (the header of the
    // fused file is the runtime verbatim)
    let runtime = dir.join("hyg-runtime");
    std::fs::write(&runtime, b"FAKERUNTIMEDATA".repeat(64)).expect("runtime");

    let out = export_game(&mut state, &runtime).expect("exports");
    assert!(out.exists(), "the single game file landed");
    assert_eq!(out.file_name().unwrap(), "my-game.bin");
    // the bundle sits beside it (the visible artifact, CLI-shaped)
    assert!(dir.join("my-game-bundle/scripts/patrol.js").exists(), "the bundle");
}

/// The approachable failures: an unsaved scene names the fix, and a
/// missing runtime binary names the build command.
#[cfg(all(feature = "editor", feature = "pack"))]
#[test]
fn export_game_failures_teach() {
    use hyg::game::editor::shell::{export_game, EditorState};

    let dir = temp_dir("hyg-export-miss");
    let mut state = EditorState::new(hyg::game::Camera::default());
    state.scene_path = dir.join("never-saved.json.hyg").to_string_lossy().to_string();
    let runtime = dir.join("hyg-runtime");
    std::fs::write(&runtime, b"fake").expect("runtime");

    // unsaved scene: the fix is "save first"
    let err = export_game(&mut state, &runtime).expect_err("no saved scene");
    assert!(err.contains("save the scene first"), "{err}");

    // saved scene + missing runtime: the fix is the build command
    let scene = Scene::snapshot_document("x", &hyg::game::Camera::default(), &[]);
    hyg::carrier::write(state.scene_path.as_ref(), &scene).expect("writes a valid scene");
    let missing = dir.join("no-such-runtime");
    let err = export_game(&mut state, &missing).expect_err("no such runtime");
    assert!(
        err.contains("no-such-runtime") || err.contains("No such"),
        "the error names the runtime: {err}"
    );
}

/// A fresh temp directory for one test (cleared on entry).
fn temp_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}
