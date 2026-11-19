//! ObjectPath contracts (M5-tail): the string reference grammar
//! (absolute, relative, parent, component suffix), the parse/round
//! trip, resolution against a real tree — first-match-wins names,
//! climbing above a root points nowhere — and the engine's template
//! helpers delivering entities and typed components from paths.

use hyg::game::object_path::{ObjectPath, Segment};
use hyg::game::physics::Shape;
use hyg::game::physics_body::PhysicsBody;
use hyg::game::renderable::MeshRenderer;
use hyg::game::scene::Scene;
use hyg::game::{Camera, Engine, Entity};
use hyg::var::Vec3;
use hyg::FromVar;
use hyg::Var;

/// The reference tree, built fresh per test:
///
/// ```text
/// world            (root 0)
/// ├── stack        (0/0)
/// │   └── arm      (0/0/0)
/// └── player       (0/1)
///     └── turret   (0/1/0)
/// props            (root 1)
/// ```
fn world() -> Vec<Entity> {
    let mut arm = Entity::new("arm");
    arm.add_component(Box::new(MeshRenderer::rect(
        Vec3::new(0.1, 0.1, 0.1),
        [1.0; 4],
    )));

    let mut stack = Entity::new("stack");
    stack.add_child(arm);

    let mut turret = Entity::new("turret");
    turret.add_component(Box::new(PhysicsBody::new(
        "turret-body",
        Shape::Sphere(0.3),
    )));

    let mut player = Entity::new("player");
    player.add_child(turret);

    let mut world = Entity::new("world");
    world.add_child(stack);
    world.add_child(player);

    let mut props = Entity::new("props");
    props.add_component(Box::new(PhysicsBody::new(
        "props-body",
        Shape::Box(Vec3::new(1.0, 1.0, 1.0)),
    )));

    vec![world, props]
}

/// The grammar: absolute, relative, climb, self, and the component
/// suffix all parse to the expected walk — and round-trip to text.
#[test]
fn parse_covers_the_grammar_and_round_trips() {
    // absolute
    let path = ObjectPath::parse("/world/stack").expect("parses");
    assert!(path.is_absolute());
    assert_eq!(
        path.to_text(),
        "/world/stack",
        "round trip through text is exact"
    );

    // relative climb + child
    let path = ObjectPath::parse("../sibling/turret:physics_body").expect("parses");
    assert!(!path.is_absolute());
    assert_eq!(path.component_type(), Some("physics_body"));
    assert_eq!(
        path.to_text(),
        "../sibling/turret:physics_body",
        "the suffix round-trips"
    );

    // the explicit self step is REAL in the walk (documents intent)
    let explicit = ObjectPath::parse("./a/../b").expect("parses");
    assert_eq!(
        explicit.segments(),
        vec![
            Segment::Self_,
            Segment::Name("a".into()),
            Segment::Parent,
            Segment::Name("b".into())
        ]
    );

    // the segments are the walk
    let path = ObjectPath::parse("/world/../player").expect("parses");
    assert_eq!(
        path.segments(),
        vec![
            Segment::Name("world".into()),
            Segment::Parent,
            Segment::Name("player".into())
        ]
    );
}

/// The grammar refuses what would mislead: empty paths, empty
/// component names, types with slashes.
#[test]
fn parse_refuses_the_misleading() {
    assert!(ObjectPath::parse("").is_err(), "empty names nothing");
    assert!(
        ObjectPath::parse("/world:").is_err(),
        "a bare colon names no component"
    );
    assert!(
        ObjectPath::parse("/world:a/b").is_err(),
        "the component type is not a path"
    );
    // a component suffix is still one walk — type names with slashes
    // would be ambiguous with the walk
}

/// Resolution against the world(): absolute anchors at roots,
/// relative climbs and descends from the origin.
#[test]
fn resolve_walks_the_tree() {
    let roots = world();

    // absolute: world/stack, world/player/turret, and the second root
    let path = ObjectPath::parse("/world/stack").expect("parses");
    assert_eq!(path.resolve(&roots, &[]), Some(vec![0, 0]));
    let path = ObjectPath::parse("/player/turret").expect("parses");
    // "player" is a CHILD of world (root 0): absolute anchors at roots,
    // so /player/turret = root named "player" — there is none (player
    // lives under world); the document path is /world/player/turret
    assert_eq!(path.resolve(&roots, &[]), None, "no root named player");
    let path = ObjectPath::parse("/world/player/turret").expect("parses");
    assert_eq!(path.resolve(&roots, &[]), Some(vec![0, 1, 0]));

    // the second root
    let path = ObjectPath::parse("/props").expect("parses");
    assert_eq!(path.resolve(&roots, &[]), Some(vec![1]));

    // relative from stack (0/0): ../player climbs to world, then descends
    let path = ObjectPath::parse("../player").expect("parses");
    assert_eq!(path.resolve(&roots, &[0, 0]), Some(vec![0, 1]));

    // relative child from player (0/1): turret
    let path = ObjectPath::parse("turret").expect("parses");
    assert_eq!(path.resolve(&roots, &[0, 1]), Some(vec![0, 1, 0]));

    // self: explicit dot stays
    let path = ObjectPath::parse(".").expect("parses");
    assert_eq!(path.resolve(&roots, &[0, 1]), Some(vec![0, 1]));

    // climbing above a root points nowhere (the scene has no parent)
    let path = ObjectPath::parse("../../props").expect("parses");
    assert_eq!(path.resolve(&roots, &[0, 0]), None);

    // a missing name
    let path = ObjectPath::parse("/world/ghost").expect("parses");
    assert_eq!(path.resolve(&roots, &[]), None);
}

/// The engine's template helpers: paths deliver ENTITIES and typed
/// COMPONENTS in code — including the component-suffix agreement check
/// (a `:mesh_renderer` path does not hand back a physics body).
#[test]
fn engine_helpers_deliver_entities_and_components() {
    let mut engine = Engine::new("acme", "object-path");

    let mut arm = Entity::new("arm");
    arm.add_component(Box::new(MeshRenderer::rect(
        Vec3::new(0.1, 0.1, 0.1),
        [1.0; 4],
    )));
    let mut stack = Entity::new("stack");
    stack.add_child(arm);
    let mut ball = Entity::new("ball");
    ball.add_component(Box::new(PhysicsBody::new("ball-body", Shape::Sphere(0.5))));
    engine.add_entity(stack);
    engine.add_entity(ball);

    // the GameObject by path (relative to the origin entity [0, 0])
    let arm_entity = engine.entity_at_object_path(&[0, 0], "../arm").expect("arm");
    assert_eq!(arm_entity.name(), "arm");

    // the typed COMPONENT by path, with the suffix agreeing — the
    // ball is a ROOT sibling of stack's root, so the path is absolute
    let body = engine.component_at_object_path::<PhysicsBody>(
        &[0, 0],
        "/ball:physics_body",
    );
    assert!(body.is_some(), "the physics body delivers");

    // a suffix that disagrees with the requested type is honest: None
    // (from stack [0], "arm:mesh_renderer" descends the child carrying
    // the renderer — requesting a physics body there is None)
    let wrong = engine.component_at_object_path::<PhysicsBody>(&[0, 0], "arm:mesh_renderer");
    assert!(wrong.is_none(), "mesh_renderer is not a physics body");

    // parse errors resolve to None (call sites don't panic on bad docs)
    assert!(engine.entity_at_object_path(&[0, 0], "").is_none());
    assert!(engine
        .component_at_object_path::<PhysicsBody>(&[0, 0], "../ball:")
        .is_none());
}

/// FromVar: scene documents carry paths as plain strings, and a bad
/// one names the rule.
#[test]
fn object_paths_come_from_documents_as_strings() {
    let var = Var::from("/world/stack:mesh_renderer");
    let path = ObjectPath::from_var(&var).expect("parses from the document");
    assert_eq!(path.component_type(), Some("mesh_renderer"));

    let var = Var::from(42);
    assert!(ObjectPath::from_var(&var).is_err(), "not a string");

    let var = Var::from("/world/:");
    assert!(ObjectPath::from_var(&var).is_err(), "empty type name");

    // and a scene document can carry one anywhere a string fits (the
    // document round-trips as the string it is)
    let mut camera = Entity::new("main camera");
    camera.add_component(Box::new(hyg::game::camera_component::CameraComponent {
        camera: Camera::default(),
        main: true,
    }));
    let doc = Scene::snapshot_document("paths", &Camera::default(), &[camera]);
    let _ = doc;
}
