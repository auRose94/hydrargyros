//! The build bundle: a scene plus every file it references, gathered
//! into one directory a runtime can run from anywhere. `hyg build
//! <scene.json.hyg>` produces it — the packaging story's data half
//! (the runtime binary is a build of the `hyg-runtime` bin).
//!
//! Dependency discovery walks the RAW document (no factories, no
//! engine): any component object carrying a `source` (script) or
//! `model` (asset mesh) key contributes a file reference, resolved
//! against its scene's directory — the same rule the loader applies.
//! A `sub_scene` component contributes the referenced scene file AND
//! recurses into it: a prefab's own scripts, models, and nested
//! sub-scenes ride along, each resolved against the sub-scene's own
//! directory (the loader's rule). The bundle preserves the scene's
//! relative layout, so the copied scene loads from the bundle root
//! without edits. Feature-free: the bundler is data in → files out.

use crate::carrier;
use crate::var::Var;
use std::path::{Component, Path, PathBuf};

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

/// One discovered reference: where it copies to (bundle-root-relative,
/// layout-preserving) and where it copies from, plus the scene that
/// referenced it — the missing-file error names the referrer, not just
/// the missing path.
struct FileRef {
    dest_rel: PathBuf,
    source: PathBuf,
    from_scene: PathBuf,
    from_rel: String,
}

/// Discover one scene's references, recursing through `sub_scene`
/// instances. `bundle_dir` is THIS scene's directory relative to the
/// bundle root (`""` for the top scene — a copied sub-scene keeps its
/// relative position, so its own references land where IT will resolve
/// them when played from the bundle root). `stack` carries the
/// canonical paths of the scenes currently being scanned — the cycle
/// guard (a sub-scene including itself, directly or through a chain,
/// refuses loudly here too: the bundler is where migration-time
/// mistakes get caught, before any runtime).
fn collect_scene_refs(
    scene_file: &Path,
    bundle_dir: &Path,
    stack: &mut Vec<PathBuf>,
    refs: &mut Vec<FileRef>,
) -> Result<(), String> {
    let doc = carrier::read(scene_file).map_err(|e| format!("{}: {e}", scene_file.display()))?;
    let canonical = std::fs::canonicalize(scene_file)
        .map_err(|e| format!("{}: {e}", scene_file.display()))?;
    if let Some(start) = stack.iter().position(|p| *p == canonical) {
        let mut cycle: Vec<String> = stack[start..]
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        cycle.push(canonical.display().to_string());
        return Err(format!(
            "sub-scene cycle (a scene may not include itself, directly or through a chain): {}",
            cycle.join(" → ")
        ));
    }
    let dir = canonical
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_default();
    stack.push(canonical);
    let walked = walk_refs(&doc, &dir, bundle_dir, scene_file, stack, refs);
    stack.pop();
    walked
}

/// Walk a raw document value collecting file references. `dir` is the
/// directory references resolve against ON DISK; `bundle_dir` is where
/// this scene lives relative to the bundle root; `scene_file` names
/// the referrer for error messages.
fn walk_refs(
    value: &Var,
    dir: &Path,
    bundle_dir: &Path,
    scene_file: &Path,
    stack: &mut Vec<PathBuf>,
    refs: &mut Vec<FileRef>,
) -> Result<(), String> {
    match value {
        Var::Object(object) => {
            // a sub_scene object is a scene reference: copy the scene
            // file at ITS bundle position, then recurse into it — its
            // `path` is the only reference key it carries
            if object.get("type").as_str() == Some("sub_scene") {
                // the temporary lives to the end of the statement —
                // bind it, then take the string (the re-reading trap)
                let path_var = object.get("path");
                let sub = path_var
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        format!(
                            "{}: a sub_scene component carries no readable \"path\"",
                            scene_file.display()
                        )
                    })?
                    .to_string();
                let sub_dest = bundle_dest(bundle_dir, &sub, scene_file)?;
                let sub_bundle_dir = sub_dest
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_default();
                push_ref(dir, bundle_dir, &sub, scene_file, refs)?;
                return collect_scene_refs(&dir.join(&sub), &sub_bundle_dir, stack, refs);
            }
            for (key, entry) in object.entries() {
                match key.as_str() {
                    "source" | "model" => {
                        if let Some(path) = entry.as_str() {
                            push_ref(dir, bundle_dir, path, scene_file, refs)?;
                        }
                    }
                    _ => walk_refs(&entry, dir, bundle_dir, scene_file, stack, refs)?,
                }
            }
            Ok(())
        }
        Var::List(list) => {
            for item in list.iter() {
                walk_refs(&item, dir, bundle_dir, scene_file, stack, refs)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Record one reference: destination normalized against the
/// referrer's bundle position (refusing paths that escape the bundle
/// root through `..` — a bundle that reaches outside itself cannot be
/// tared, fused, or moved as one file), deduplicated by destination
/// (same relative slot = same file, first discovery wins).
fn push_ref(
    dir: &Path,
    bundle_dir: &Path,
    rel: &str,
    scene_file: &Path,
    refs: &mut Vec<FileRef>,
) -> Result<(), String> {
    let dest_rel = bundle_dest(bundle_dir, rel, scene_file)?;
    if refs.iter().any(|r| r.dest_rel == dest_rel) {
        return Ok(());
    }
    refs.push(FileRef {
        dest_rel,
        source: dir.join(rel),
        from_scene: scene_file.to_path_buf(),
        from_rel: rel.to_string(),
    });
    Ok(())
}

/// The bundle-root destination for a reference: the referrer's bundle
/// position + the reference's scene-relative path, `..`-climbs
/// resolved inside the tree.
fn bundle_dest(bundle_dir: &Path, rel: &str, scene_file: &Path) -> Result<PathBuf, String> {
    let joined = if bundle_dir.as_os_str().is_empty() {
        PathBuf::from(rel)
    } else {
        bundle_dir.join(rel)
    };
    normalize_rel(&joined.to_string_lossy()).ok_or_else(|| {
        format!(
            "{}: reference {rel:?} escapes the bundle root — every dependency must live inside the project's directory tree",
            scene_file.display()
        )
    })
}

/// The bundle-root-relative destination for a scene-relative reference:
/// `.` segments drop, `..` climbs within the tree. `None` = the path
/// escapes the root (or reduces to nothing).
fn normalize_rel(rel: &str) -> Option<PathBuf> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in Path::new(rel).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::Normal(c) => parts.push(c),
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    if parts.is_empty() {
        return None;
    }
    Some(parts.iter().collect())
}

/// Build a bundle: read the scene, gather its references (sub-scenes
/// recursed), copy the scene + every dependency into `out_dir`
/// preserving the scene's relative layout. Overwrites anything already
/// there — a build is reproducible by construction.
///
/// # Errors
/// The scene doesn't read, a referenced file is missing (the message
/// names the scene that referenced it and the missing path), a
/// reference escapes the bundle root, or a copy fails.
pub fn build(scene_path: &Path, out_dir: &Path) -> Result<BundleReport, String> {
    let mut stack = Vec::new();
    let mut refs = Vec::new();
    collect_scene_refs(scene_path, Path::new(""), &mut stack, &mut refs)?;

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;

    let mut files = Vec::new();
    for r in &refs {
        if !r.source.exists() {
            return Err(format!(
                "{} references {:?}, which is missing at {}",
                r.from_scene.display(),
                r.from_rel,
                r.source.display()
            ));
        }
        let dest = out_dir.join(&r.dest_rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&r.source, &dest)
            .map_err(|e| format!("{} -> {}: {e}", r.source.display(), dest.display()))?;
        files.push(dest);
    }

    let Some(scene_name) = scene_path.file_name() else {
        return Err(format!("{} has no file name", scene_path.display()));
    };
    let scene_dest = out_dir.join(scene_name);
    std::fs::copy(scene_path, &scene_dest)
        .map_err(|e| format!("{} -> {}: {e}", scene_path.display(), scene_dest.display()))?;

    // the project document rides along when one sits beside the scene
    // (the runtime reads name/version/window from it)
    let project = scene_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(crate::game::project::Project::FILE_NAME);
    if project.exists() {
        let dest = out_dir.join(crate::game::project::Project::FILE_NAME);
        std::fs::copy(&project, &dest)
            .map_err(|e| format!("{} -> {}: {e}", project.display(), dest.display()))?;
        files.push(dest);
    }

    Ok(BundleReport {
        scene: scene_dest,
        files,
    })
}
