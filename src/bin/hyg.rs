//! The `hyg` CLI — packaging. `hyg build <scene.json.hyg> [--out DIR]`
//! gathers a scene and every file it references into one bundle
//! directory a runtime can run from anywhere (see [`hyg::bundle`]).
//! Feature-free: the bundler is data in → files out.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        #[cfg(feature = "pack")]
        Some("fuse") => {
            let opt = |name: &str| {
                args.iter()
                    .position(|a| a == name)
                    .and_then(|i| args.get(i + 1))
                    .map(std::path::PathBuf::from)
            };
            let (Some(runtime), Some(scene)) = (opt("--runtime"), opt("--scene")) else {
                eprintln!("hyg fuse: --runtime <binary> and --scene <scene.json.hyg> are required");
                std::process::exit(2);
            };
            let out = opt("--out").unwrap_or_else(|| {
                scene
                    .parent()
                    .map(|p| p.join("game.bin"))
                    .unwrap_or_else(|| std::path::PathBuf::from("game.bin"))
            });
            // stage the bundle beside the scene (the standard build), then fuse
            let staging = out
                .parent()
                .map(|p| p.join("bundle-staging"))
                .unwrap_or_else(|| std::path::PathBuf::from("bundle-staging"));
            match hyg::bundle::build(&scene, &staging)
                .and_then(|_| hyg::pack::fuse(&runtime, &staging, &out))
            {
                Ok(size) => {
                    println!("fused {} ({} KB)", out.display(), size / 1024);
                    println!("the runtime runs the game bare: {} (no args)", out.display());
                }
                Err(e) => {
                    eprintln!("hyg fuse: {e}");
                    std::process::exit(1);
                }
            }
        }
        Some("build") => {
            let Some(scene) = args.get(1) else {
                usage();
                std::process::exit(2);
            };
            // --out DIR (default: the scene's stem + "-bundle", beside it)
            let out = args
                .iter()
                .position(|a| a == "--out")
                .and_then(|i| args.get(i + 1))
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
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
                }
                Err(e) => {
                    eprintln!("hyg build: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => usage(),
    }
}

fn usage() {
    eprintln!("hyg - the Hydrargyros packaging CLI");
    eprintln!();
    eprintln!("USAGE:");
    eprintln!("  hyg build <scene.json.hyg> [--out DIR]");
    #[cfg(feature = "pack")]
    eprintln!("  hyg fuse --runtime <hyg-runtime> --scene <scene.json.hyg> [--out game.bin]");
    eprintln!();
    eprintln!("Gathers the scene and every file it references (script sources,");
    eprintln!("asset meshes) into a bundle directory a runtime can run from");
    eprintln!("anywhere. Default --out: <scene-stem>-bundle beside the scene.");
}
