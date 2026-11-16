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

    let mut near = Entity::new("near");
    near.transform_mut().position = Vec3::new(0.0, 0.0, 0.0);
    near.add_component(Box::new(MeshRenderer::new(
        MeshGeometry::Shape {
            shape: MeshShape::Rect,
            half_size: Vec3::new(1.0, 1.0, 1.0),
        },
        [1.0, 1.0, 1.0, 1.0],
    )));
    let mut far = Entity::new("far");
    far.transform_mut().position = Vec3::new(4.0, 0.0, 2.0);
    far.add_component(Box::new(MeshRenderer::new(
        MeshGeometry::Shape {
            shape: MeshShape::Rect,
            half_size: Vec3::new(1.0, 1.0, 1.0),
        },
        [1.0, 1.0, 1.0, 1.0],
    )));
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
    )
    .expect("lays out");
    let x = &layout.axes[0];
    let drag = [x.screen_dir[0] * 50.0, x.screen_dir[1] * 50.0];
    let delta = gizmo::scale_delta(x, drag);
    let expected = 50.0 / x.px_per_unit;
    assert!((delta - expected).abs() < 0.05, "additive drag: {delta}");
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
    let layout = gizmo::layout(GizmoMode::Translate, &camera, &view, origin, identity)
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
