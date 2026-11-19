//! The Hydrargyros runtime: `hyg-runtime <scene.json.hyg>` loads a
//! scene and PLAYS it in a window — the artifact `hyg build` packages
//! for. Winit owns the main thread (macOS requires it), the engine
//! pumps on another thread through the ordinary `WindowSystem` seam,
//! the wgpu renderer presents to the surface. Close the window to quit.
//!
//! Builds only with the runtime features (see the [[bin]] entry).

use hyg::game::scene::Scene;
use hyg::game::{Engine, PhysicsWorld, WgpuRenderer, WinitWindow};

fn main() {
    let mut args = std::env::args().skip(1);

    // --extract [dir]: unpack the fused archive's files (the full asset
    // set) and exit — NONDESTRUCTIVE, existing files are skipped, so
    // players get the defaults without clobbering their mods, and
    // modders get the loose layout to edit (the overlay then picks it
    // up on every later launch). Default destination: beside the exe,
    // where the loose-files overlay looks first.
    if args.next().map(|a| a == "--extract").unwrap_or(false) {
        let exe = std::env::current_exe()
            .map_err(|e| format!("current_exe: {e}"))
            .expect("exe path");
        let dir = args.next().map(std::path::PathBuf::from).unwrap_or_else(|| {
            exe.parent()
                .map(|p| p.to_path_buf())
                .expect("the exe has a parent")
        });
        match hyg::pack::extract_archive(&exe, &dir, true) {
            Ok(written) => {
                println!(
                    "extracted {} file{} to {} (existing files skipped)",
                    written,
                    if written == 1 { "" } else { "s" },
                    dir.display()
                );
            }
            Err(e) => {
                eprintln!("hyg-runtime: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    // BARE invocation: this binary may be FUSED (a bundle tarred onto
    // its tail) — detect our own archive, unpack it to a temp home,
    // and play the scene it carries. With a scene path, play that.
    let scene_path = match args.next() {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            let exe = std::env::current_exe()
                .map_err(|e| format!("current_exe: {e}"))
                .expect("exe path");
            let extracted = hyg::pack::unpack_self(&exe)
                .expect("reading our own tail")
                .unwrap_or_else(|| {
                    eprintln!("USAGE: hyg-runtime <scene.json.hyg>");
                    eprintln!("(this binary carries no fused game - pass a scene path)");
                    std::process::exit(2);
                });
            // MODDING: loose files BESIDE the executable override the
            // fused archive (replace files, add files - players own the
            // game; a modded scene.json.hyg IS a different game)
            if let Some(exe_dir) = exe.parent() {
                match hyg::pack::overlay_loose(exe_dir, &extracted) {
                    Ok(count) if count > 0 => {
                        println!("mods: {count} loose file(s) applied over the archive");
                    }
                    Ok(_) => {}
                    Err(e) => eprintln!("hyg: mods: {e}"),
                }
            }
            hyg::pack::scene_in(&extracted).unwrap_or_else(|| {
                eprintln!("the fused bundle carries no scene");
                std::process::exit(1);
            })
        }
    };
    let scene = match Scene::load(&scene_path) {
        Ok(scene) => scene,
        Err(e) => {
            eprintln!("hyg-runtime: {}: {e}", scene_path.display());
            std::process::exit(1);
        }
    };
    println!(
        "loaded {} ({} root entities) — playing",
        scene.name,
        scene.entities.len()
    );

    // the event loop + window: created and run on the main thread
    let title = format!("☿ {} — Hydrargyros runtime", scene.name);
    let window = match WinitWindow::new(&title, (1280, 720)) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("no windowing system: {e}");
            std::process::exit(1);
        }
    };
    let pump_window = window.pump_handle();
    let closer = window.pump_handle();

    let pump = std::thread::spawn(move || {
        // wait for the window (created when the event loop resumes)
        let mut winit_window = None;
        for _ in 0..200 {
            if let Some(w) = closer.window() {
                winit_window = Some(w);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let Some(winit_window) = winit_window else {
            eprintln!("window never appeared");
            closer.request_close();
            return;
        };
        let renderer =
            match pollster::block_on(WgpuRenderer::new_for_window(winit_window)) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("no wgpu adapter for the window: {e}");
                    closer.request_close();
                    return;
                }
            };

        let mut engine = Engine::new("acme", &scene.name);
        engine.frame_time(0.0);
        engine.fixed_dt(1.0 / 60.0);
        // the renderer ATTACHES — created alone, it draws nothing (the
        // blank-window bug Rose caught on the runtime's first run)
        engine.set_renderer(Box::new(renderer));
        // rapier for the scene's bodies (a body-less scene attaches an
        // empty world — harmless)
        match PhysicsWorld::create("rapier", &hyg::Var::Null) {
            Ok(world) => {
                engine.set_physics(world);
            }
            Err(e) => eprintln!("physics backend unavailable: {e}"),
        }
        scene.spawn_into(&mut engine);

        // the runtime PLAYS: update + draw + present until closed
        engine.start_with(Box::new(pump_window));
        closer.request_close();
    });

    // blocks until the pump asks to close or the user closes the window
    window.run();
    let _ = pump.join();
    println!("done");
}
