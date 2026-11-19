//! The project document's contracts (M5 polish): identity and window
//! defaults round-trip canonically, a missing file is a valid default
//! project (a fresh directory IS a project), a malformed file is loud,
//! scaffolding lays a playable project down, and the bundle carries
//! the project beside the scene.

use hyg::bundle;
use hyg::carrier;
use hyg::game::project::{Project, WindowSize};
use hyg::game::Camera;
use hyg::var::Var;
use std::path::Path;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

use std::path::PathBuf;

/// Fields at their defaults are OMITTED (the canonical-form rule) and
/// a minimal document parses to defaults.
#[test]
fn project_round_trips_canonically() {
    // defaults: nothing written
    let default = Project::default();
    let doc = default.to_var();
    assert!(matches!(doc.get("name"), Var::Null), "defaults omitted");
    assert!(
        matches!(doc.get("window"), Var::Null),
        "default window omitted"
    );
    let reloaded = Project::from_var(&doc).expect("parses");
    assert_eq!(reloaded, default);

    // explicit fields round-trip
    let full = Project {
        name: "Star Sweeper".to_string(),
        version: "0.7.2".to_string(),
        entry: Some("scenes/level1.json.hyg".to_string()),
        window: WindowSize {
            width: 1920,
            height: 1080,
        },
    };
    let doc = full.to_var();
    assert_eq!(Project::from_var(&doc).expect("parses"), full);
}

/// A missing project.json.hyg is a VALID default (a fresh directory is
/// a project — the approachable default); a malformed file is LOUD
/// with the path.
#[test]
fn load_defaults_on_missing_and_fails_loud_on_malformed() {
    let dir = temp_dir("hyg-project-missing");
    let project = Project::load(&dir).expect("missing file = default project");
    assert_eq!(project.name, "untitled");
    assert_eq!(project.window.width, 1280);

    std::fs::write(dir.join("project.json.hyg"), "not json at all {").expect("junk");
    let err = Project::load(&dir).expect_err("malformed is loud");
    assert!(err.contains("project.json.hyg"), "names the path: {err}");
}

/// Scaffolding lays a playable project down: the document, the hello
/// scene, the starter script — and the scaffolded scene LOADS.
#[test]
fn scaffold_lays_a_playable_project() {
    let dir = temp_dir("hyg-scaffold");
    let project_path = Project::scaffold(&dir, "Star Sweeper").expect("scaffolds");

    assert!(project_path.exists(), "the project document");
    assert!(dir.join("scene.json.hyg").exists(), "the hello scene");
    assert!(dir.join("scripts/main.js").exists(), "the starter script");
    assert!(dir.join("assets").is_dir(), "the assets home");

    let project = Project::load(&dir).expect("loads");
    assert_eq!(project.name, "Star Sweeper");

    // the scaffolded scene LOADS (a valid document the runtime accepts)
    let scene = hyg::game::scene::Scene::load(&dir.join("scene.json.hyg")).expect("scene loads");
    assert!(scene.entities.len() >= 2, "camera + hello entity");
    let _ = Path::new(&project_path);
}

/// The bundler carries the project document beside the scene (the
/// runtime reads name/version/window from it).
#[test]
fn bundle_carries_the_project_document() {
    let dir = temp_dir("hyg-bundle-project");
    let project = Project {
        name: "bundled-game".to_string(),
        ..Project::default()
    };
    project.write(&dir).expect("writes the project");

    let doc = hyg::game::scene::Scene::document("s", &Camera::default(), &[]);
    let scene_path = dir.join("scene.json.hyg");
    carrier::write(&scene_path, &doc).expect("writes the scene");

    let out = dir.join("bundle");
    let report = bundle::build(&scene_path, &out).expect("builds");
    assert!(
        report
            .files
            .iter()
            .any(|f| f.ends_with(Project::FILE_NAME)),
        "the project rides in the bundle: {:?}",
        report.files
    );

    // and the runtime's read of the bundle resolves the project
    let loaded = Project::load(&out).expect("the bundle's project loads");
    assert_eq!(loaded.name, "bundled-game");
}
