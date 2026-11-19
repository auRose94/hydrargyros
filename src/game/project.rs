//! The project document: the configuration centerpiece a Studio
//! session and every build reads — the game's name and version, the
//! entry scene, the window shape. One `project.json.hyg` beside the
//! scenes; `hyg new` scaffolds one, `hyg build`/`hyg fuse` carry it
//! into the bundle, the runtime announces it, and the editor's
//! Project Settings panel edits it. Plain data: a Project is a Var in,
//! a Var out — byte-identical round trips like everything else that
//! rides the carrier.

use crate::carrier;
use crate::game::scene::scene_f32;
use crate::object::Object;
use crate::var::{Var, Vec3};
use std::path::{Path, PathBuf};

/// The window shape a runtime opens (the editor's Project Settings
/// panel edits the same numbers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSize {
    pub width: u32,
    pub height: u32,
}

/// A project document: identity and defaults for a game.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    /// The game's display name (window titles, the bundle manifest).
    pub name: String,
    /// Semver-ish version string, announced at runtime launch.
    pub version: String,
    /// The entry scene, relative to the project file (the file a bare
    /// `hyg-runtime` runs).
    pub entry: Option<String>,
    /// The window shape the runtime opens.
    pub window: WindowSize,
}

impl Default for Project {
    /// A fresh project: untitled, 1.0.0, no entry yet, a 1280×720
    /// window.
    fn default() -> Self {
        Project {
            name: "untitled".to_string(),
            version: "1.0.0".to_string(),
            entry: None,
            window: WindowSize {
                width: 1280,
                height: 720,
            },
        }
    }
}

impl Project {
    /// The document name projects carry (`project.json.hyg`).
    pub const FILE_NAME: &'static str = "project.json.hyg";

    /// Parse from a document object (the fields are optional —
    /// defaults apply, so a hand-written project can start minimal).
    ///
    /// # Errors
    /// A field of the wrong shape (the error names the field).
    pub fn from_var(v: &Var) -> Result<Project, crate::FromVarError> {
        let name = crate::game::scene::opt_field(v, "name")
            .map(|n| {
                n.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| crate::FromVarError::new("expected a name string").at("name"))
            })
            .transpose()?;
        let version = crate::game::scene::opt_field(v, "version")
            .map(|n| {
                n.as_str().map(str::to_string).ok_or_else(|| {
                    crate::FromVarError::new("expected a version string").at("version")
                })
            })
            .transpose()?;
        let entry = crate::game::scene::opt_field(v, "entry")
            .map(|n| {
                n.as_str().map(str::to_string).ok_or_else(|| {
                    crate::FromVarError::new("expected an entry scene path").at("entry")
                })
            })
            .transpose()?;
        let window = match crate::game::scene::opt_field(v, "window") {
            Some(window) => {
                let width = super::scene::opt_field(&window, "width")
                    .map(|w| scene_f32(&w).map(|f| f as u32).map_err(|e| e.at("window.width")))
                    .transpose()?
                    .unwrap_or(1280);
                let height = super::scene::opt_field(&window, "height")
                    .map(|h| scene_f32(&h).map(|f| f as u32).map_err(|e| e.at("window.height")))
                    .transpose()?
                    .unwrap_or(720);
                WindowSize { width, height }
            }
            None => WindowSize {
                width: 1280,
                height: 720,
            },
        };
        Ok(Project {
            name: name.unwrap_or_else(|| "untitled".to_string()),
            version: version.unwrap_or_else(|| "1.0.0".to_string()),
            entry,
            window,
        })
    }

    /// The document object (fields at their defaults are omitted —
    /// the canonical-form rule scenes follow).
    #[must_use]
    pub fn to_var(&self) -> Var {
        let obj = Object::new();
        if self.name != "untitled" {
            obj.set("name", Var::from(self.name.as_str()));
        }
        if self.version != "1.0.0" {
            obj.set("version", Var::from(self.version.as_str()));
        }
        if let Some(entry) = &self.entry {
            obj.set("entry", Var::from(entry.as_str()));
        }
        if self.window.width != 1280 || self.window.height != 720 {
            obj.set(
                "window",
                Var::from(crate::o! {
                    "width": Var::from(self.window.width as f64),
                    "height": Var::from(self.window.height as f64),
                }),
            );
        }
        Var::from(obj)
    }

    /// Load `project.json.hyg` from a directory. Missing FILE = the
    /// default project (a fresh directory is a valid project — the
    /// approachable default); a malformed file is LOUD.
    ///
    /// # Errors
    /// The file exists but doesn't read or parse.
    pub fn load(dir: &Path) -> Result<Project, String> {
        let path = dir.join(Self::FILE_NAME);
        if !path.exists() {
            return Ok(Project::default());
        }
        let doc = carrier::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Project::from_var(&doc).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Write the project document into a directory (canonical JSON via
    /// the carrier).
    ///
    /// # Errors
    /// The write fails.
    pub fn write(&self, dir: &Path) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join(Self::FILE_NAME);
        carrier::write(&path, &self.to_var()).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(path)
    }

    /// SCAFFOLD a fresh project for `hyg new`: the project document, a
    /// hello scene (a main camera entity), and a commented starter
    /// script — the newcomer opens the folder and presses play.
    /// Returns the scaffolded project file's path.
    ///
    /// # Errors
    /// Any write fails (the message names the path).
    pub fn scaffold(dir: &Path, name: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(dir.join("scripts"))
            .map_err(|e| format!("scripts dir: {e}"))?;
        std::fs::create_dir_all(dir.join("assets")).map_err(|e| format!("assets dir: {e}"))?;

        let project = Project {
            name: name.to_string(),
            ..Project::default()
        };
        let project_path = project.write(dir)?;

        // the hello scene: a main camera + one visible crate of a
        // shape (feature-free: a rect renders through the quad path)
        let mut camera = crate::game::Entity::new("main camera");
        camera.add_component(Box::new(crate::game::camera_component::CameraComponent {
            camera: crate::game::Camera {
                position: Vec3::new(0.0, 0.0, -6.0),
                target: Vec3::splat(0.0),
                projection: crate::game::Projection::Perspective {
                    fov_y_degrees: 60.0,
                    near: 0.1,
                    far: 100.0,
                    aspect: 16.0 / 9.0,
                },
            },
            main: true,
        }));
        let mut hello = crate::game::Entity::new("hello");
        hello.add_component(Box::new(crate::game::renderable::MeshRenderer::rect(
            Vec3::new(0.5, 0.5, 0.5),
            [0.4, 0.9, 0.6, 1.0],
        )));
        let scene = crate::game::scene::Scene::document(
            name,
            &crate::game::Camera::default(),
            &[camera, hello],
        );
        carrier::write(&dir.join("scene.json.hyg"), &scene)
            .map_err(|e| format!("scene.json.hyg: {e}"))?;

        // the starter script: commented hello (a real file the scene
        // can attach later — the newcomer edits and re-attaches)
        std::fs::write(
            dir.join("scripts/main.js"),
            "// your first script.\n// attach it to an entity in the editor,\n\
             // then: function update(dt) { transform.rotation.y += dt; }\n",
        )
        .map_err(|e| format!("scripts/main.js: {e}"))?;

        Ok(project_path)
    }
}

/// The project document's canonical existence check: a path IS a
/// project document when it ends in the canonical file name.
#[must_use]
pub fn is_project_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == Project::FILE_NAME)
        .unwrap_or(false)
}
