//! The build bundle: a scene plus every file it references, gathered
//! into one directory a runtime can run from anywhere. `hyg build
//! <scene.json.hyg>` produces it — the packaging story's data half
//! (the runtime binary is a build of the `hyg-runtime` bin).
//!
//! Dependency discovery walks the RAW document (no factories, no
//! engine): any component object carrying a `source` (script) or
//! `model` (asset mesh) key contributes a file reference, resolved
//! against the scene's directory — the same rule the loader applies.
//! The bundle preserves the scene's relative layout, so the copied
//! scene loads from the bundle root without edits. Feature-free: the
//! bundler is data in → files out.

use crate::carrier;
use crate::var::Var;
use std::path::{Path, PathBuf};

/// What a build produced: where the scene landed and which dependency
/// files were copied along (relative to the bundle root).
#[derive(Debug, Clone, PartialEq)]
pub struct BundleReport {
    /// The copied scene, inside the bundle root.
    pub scene: PathBuf,
    /// The dependency files copied with it, in discovery order,
    /// bundle-root-relative.
    pub files: Vec<PathBuf>,
}

/// Collect a scene's file references from its RAW document: every
/// object's `source` and `model` keys (script sources, asset meshes —
/// the two file-reference keys a component declares today) at any
/// depth. Discovery order, duplicates included (a copy overwrites a
/// copy — harmless).
fn collect_refs(value: &Var, refs: &mut Vec<String>) {
    match value {
        Var::Object(object) => {
            for (key, entry) in object.entries() {
                match key.as_str() {
                    "source" | "model" => {
                        if let Some(path) = entry.as_str() {
                            refs.push(path.to_string());
                        }
                    }
                    _ => collect_refs(&entry, refs),
                }
            }
        }
        Var::List(list) => {
            for item in list.iter() {
                collect_refs(&item, refs);
            }
        }
        _ => {}
    }
}

/// Build a bundle: read the scene, gather its references, copy the
/// scene + every dependency into `out_dir` preserving the scene's
/// relative layout. Overwrites anything already there — a build is
/// reproducible by construction.
///
/// # Errors
/// The scene doesn't read, a referenced file is missing (the message
/// names the scene and the missing path), or a copy fails.
pub fn build(scene_path: &Path, out_dir: &Path) -> Result<BundleReport, String> {
    let doc = carrier::read(scene_path).map_err(|e| format!("{}: {e}", scene_path.display()))?;
    let scene_dir = scene_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();

    let mut refs = Vec::new();
    collect_refs(&doc, &mut refs);

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let mut files = Vec::new();
    for rel in &refs {
        let source = scene_dir.join(rel);
        let dest = out_dir.join(rel);
        if !source.exists() {
            return Err(format!(
                "{} references {:?}, which is missing at {}",
                scene_path.display(),
                rel,
                source.display()
            ));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&source, &dest)
            .map_err(|e| format!("{} -> {}: {e}", source.display(), dest.display()))?;
        files.push(dest);
    }

    let Some(scene_name) = scene_path.file_name() else {
        return Err(format!("{} has no file name", scene_path.display()));
    };
    let scene_dest = out_dir.join(scene_name);
    std::fs::copy(scene_path, &scene_dest)
        .map_err(|e| format!("{} -> {}: {e}", scene_path.display(), scene_dest.display()))?;

    Ok(BundleReport {
        scene: scene_dest,
        files,
    })
}
