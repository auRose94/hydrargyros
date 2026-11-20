//! The `hyg` CLI — packaging. `hyg build <scene.json.hyg> [--out DIR]`
//! gathers a scene and every file it references into one bundle
//! directory a runtime can run from anywhere (see [`hyg::bundle`]);
//! `hyg fuse` seals the bundle onto the runtime's tail — the single
//! game file. Feature-free except `fuse` (`pack`) and `--compress`
//! (`pack-compress`). Reports are human lines by default; `--metrics`
//! appends a one-line JSON document for build scripts.

#[path = "cli.rs"]
mod cli;

use cli::{metrics, value};
#[cfg(feature = "pack")]
use cli::{flag, human};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        #[cfg(feature = "pack")]
        Some("fuse") => fuse_command(&args),
        Some("new") => new_command(&args),
        Some("build") => build_command(&args),
        _ => usage(),
    }
}

/// `hyg fuse --runtime <bin> --scene <scene.json.hyg> [--out game.bin]
/// [--compress] [--metrics]`: stage the bundle beside the scene (the
/// standard build), then seal it onto the runtime's tail.
#[cfg(feature = "pack")]
fn fuse_command(args: &[String]) {
    let (Some(runtime), Some(scene)) = (value(args, "--runtime"), value(args, "--scene")) else {
        eprintln!("hyg fuse: --runtime <binary> and --scene <scene.json.hyg> are required");
        std::process::exit(2);
    };
    let out = value(args, "--out").unwrap_or_else(|| {
        scene
            .parent()
            .map(|p| p.join("game.bin"))
            .unwrap_or_else(|| std::path::PathBuf::from("game.bin"))
    });
    // stage the bundle beside the game file (the standard build)
    let staging = out
        .parent()
        .map(|p| p.join("bundle-staging"))
        .unwrap_or_else(|| std::path::PathBuf::from("bundle-staging"));

    let started = std::time::Instant::now();
    let bundle = match hyg::bundle::build(&scene, &staging) {
        Ok(bundle) => bundle,
        Err(e) => {
            eprintln!("hyg fuse: {e}");
            std::process::exit(1);
        }
    };
    let options = hyg::pack::FuseOptions {
        compress: flag(args, "--compress"),
    };
    match hyg::pack::fuse(&runtime, &staging, &out, options) {
        Ok(stats) => {
            println!("fused {} ({})", out.display(), human(stats.fused_bytes));
            println!(
                "  archive: {} → {} ({} file{})",
                human(stats.archive_raw as u64),
                human(stats.archive_stored as u64),
                bundle.files.len(),
                if bundle.files.len() == 1 { "" } else { "s" }
            );
            println!(
                "  version {} stamped — the runtime runs the game bare (no args)",
                stats.version
            );
            metrics(
                args,
                serde_json::json!({
                    "command": "fuse",
                    "out": out.display().to_string(),
                    "runtime_bytes": std::fs::metadata(&runtime).map(|m| m.len()).unwrap_or(0),
                    "archive_raw_bytes": stats.archive_raw,
                    "archive_stored_bytes": stats.archive_stored,
                    "fused_bytes": stats.fused_bytes,
                    "files": bundle.files.len(),
                    "compressed": stats.compressed,
                    "version": stats.version,
                    "duration_ms": started.elapsed().as_millis() as u64,
                }),
            );
        }
        Err(e) => {
            eprintln!("hyg fuse: {e}");
            std::process::exit(1);
        }
    }
}

/// `hyg new <name> [--out DIR]`: scaffold a project.
fn new_command(args: &[String]) {
    let Some(name) = args.get(1) else {
        eprintln!("hyg new: a project name is required");
        eprintln!("USAGE: hyg new <name> [--out DIR]");
        std::process::exit(2);
    };
    let out = value(args, "--out").unwrap_or_else(|| std::path::PathBuf::from(name));
    match hyg::game::project::Project::scaffold(&out, name) {
        Ok(project_path) => {
            println!("new project: {}", project_path.display());
            println!("  scene:  {}", out.join("scene.json.hyg").display());
            println!("  script: {}", out.join("scripts/main.js").display());
            println!(
                "play it: hyg-runtime {} (or open it in the Studio)",
                out.join("scene.json.hyg").display()
            );
        }
        Err(e) => {
            eprintln!("hyg new: {e}");
            std::process::exit(1);
        }
    }
}

/// `hyg build <scene.json.hyg> [--out DIR] [--metrics]`: the bundle
/// directory (scene + every referenced file, sub-scenes recursed).
fn build_command(args: &[String]) {
    let Some(scene) = args.get(1) else {
        usage();
        std::process::exit(2);
    };
    // --out DIR (default: the scene's stem + "-bundle", beside it)
    let out = value(args, "--out").unwrap_or_else(|| {
        let scene_path = std::path::Path::new(scene);
        let stem = scene_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("scene")
            .trim_end_matches(".json")
            .to_string();
        scene_path
            .parent()
            .map(|p| p.join(format!("{stem}-bundle")))
            .unwrap_or_else(|| std::path::PathBuf::from(format!("{stem}-bundle")))
    });
    match hyg::bundle::build(std::path::Path::new(scene), &out) {
        Ok(report) => {
            println!("bundle: {}", report.scene.display());
            for file in &report.files {
                println!("  + {}", file.display());
            }
            println!(
                "{} file{} bundled",
                report.files.len(),
                if report.files.len() == 1 { "" } else { "s" }
            );
            metrics(
                args,
                serde_json::json!({
                    "command": "build",
                    "scene": report.scene.display().to_string(),
                    "files": report.files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>(),
                    "file_count": report.files.len(),
                }),
            );
        }
        Err(e) => {
            eprintln!("hyg build: {e}");
            std::process::exit(1);
        }
    }
}

fn usage() {
    eprintln!("hyg - the Hydrargyros packaging CLI");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("  hyg new <name> [--out DIR]");
    eprintln!("  hyg build <scene.json.hyg> [--out DIR] [--metrics]");
    #[cfg(feature = "pack")]
    eprintln!("  hyg fuse --runtime <hyg-runtime> --scene <scene.json.hyg>");
    #[cfg(feature = "pack")]
    eprintln!("       [--out game.bin] [--compress] [--metrics]");
    eprintln!();
    eprintln!("Gathers the scene and every file it references (script sources,");
    eprintln!("asset meshes, sub-scenes) into a bundle directory a runtime can");
    eprintln!("run from anywhere. Default --out: <scene-stem>-bundle beside the");
    eprintln!("scene. Every command prints --metrics JSON for build scripts.");
}
