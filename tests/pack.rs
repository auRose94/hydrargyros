//! The single-file packaging contracts (M6): fuse appends a bundle's
//! tar to a runtime's tail under a footer; unpack detects and restores
//! the bundle exactly; an unfused binary carries no archive. The
//! round trip is the whole M6 story — one file that IS the game.

#![cfg(feature = "pack")]

use hyg::pack;
use std::path::Path;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

use std::path::PathBuf;

/// The full fuse round trip: a runtime + a bundle fuse into one file
/// whose tail is the archive; unpack restores every file (the scene
/// included) and the fused file STARTS with the original runtime bytes
/// (the binary still executes as itself).
#[test]
fn fuse_and_unpack_round_trips() {
    let dir = temp_dir("hyg-pack-fuse");

    // the "runtime": arbitrary bytes — the fuse never interprets them
    let runtime_bytes: Vec<u8> = (0..1024).map(|i| (i % 251) as u8).collect();
    let runtime_path = dir.join("runtime.bin");
    std::fs::write(&runtime_path, &runtime_bytes).expect("runtime");

    // the bundle: a scene + a script, laid out like a real project
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(bundle.join("scripts")).expect("bundle dirs");
    std::fs::write(bundle.join("scene.json.hyg"), "{\"scene\":\"packed\"}").expect("scene");
    std::fs::write(bundle.join("scripts/patrol.js"), "function update(dt) {}").expect("script");

    let fused = dir.join("mygame.bin");
    let size = pack::fuse(&runtime_path, &bundle, &fused).expect("fuses");

    // the tail: footer magic, then the archive length
    let fused_bytes = std::fs::read(&fused).expect("read the fused file");
    assert_eq!(fused_bytes.len() as u64, size, "the size matches");
    assert!(
        fused_bytes.starts_with(&runtime_bytes[..8.min(runtime_bytes.len())]),
        "the file still starts as the runtime binary"
    );
    assert_eq!(&fused_bytes[fused_bytes.len() - 16..fused_bytes.len() - 8], b"HYGFUSE1");

    // unpack restores the bundle: the scene loads FROM the extraction
    let extracted = pack::unpack_self(&fused).expect("reads its own tail").expect("fused");
    let scene = extracted.join("scene.json.hyg");
    assert!(scene.exists(), "the scene restored: {:?}", scene);
    let script = extracted.join("scripts/patrol.js");
    assert!(script.exists(), "the script restored");
    assert_eq!(
        std::fs::read_to_string(&script).expect("read"),
        "function update(dt) {}",
        "contents exact"
    );

    // the runtime bytes survive the padding untouched at the head
    let head = &fused_bytes[..runtime_bytes.len()];
    assert_eq!(head, runtime_bytes.as_slice(), "the binary head is intact");

    let _ = Path::new(&extracted);
}

/// A bare (unfused) binary carries no archive: unpack is None, not an
/// error — the runtime needs that distinction to fall back to usage.
#[test]
fn unfused_binary_carries_no_archive() {
    let dir = temp_dir("hyg-pack-unfused");
    let bare = dir.join("bare.bin");
    std::fs::write(&bare, b"just a program, no game appended").expect("write");
    assert!(pack::unpack_self(&bare).expect("reads fine").is_none());

    // tiny file: shorter than the footer
    let tiny = dir.join("tiny.bin");
    std::fs::write(&tiny, b"hi").expect("write");
    assert!(pack::unpack_self(&tiny).expect("reads fine").is_none());
}

/// The scene finder: the `.json.hyg` at an extraction's root, skipping
/// everything else.
#[test]
fn scene_in_finds_the_document() {
    use hyg::pack::scene_in;
    let dir = temp_dir("hyg-pack-scene");
    std::fs::create_dir_all(dir.join("scripts")).expect("dirs");
    std::fs::write(dir.join("game.json.hyg"), "{}").expect("scene");
    std::fs::write(dir.join("scripts/patrol.js"), "").expect("script");

    let found = scene_in(&dir).expect("finds the scene");
    assert_eq!(found.file_name().unwrap(), "game.json.hyg");
    assert!(scene_in(&dir.join("scripts")).is_none(), "no scene there");
}

// ---- MODDING: loose files beside the exe override the archive ----

/// The union with mods winning: a loose file REPLACES its archive
/// sibling, a loose-only file is ADDED, and untouched archive files
/// stay. That's the whole player-owns-the-game story — modify and even
/// become a different game.
#[test]
fn loose_files_beside_the_exe_override_the_archive() {
    let dir = temp_dir("hyg-pack-mods");

    // the runtime + the bundle (scene + script)
    let runtime_bytes: Vec<u8> = b"FAKERUNTIMEBYTES".to_vec();
    let runtime_path = dir.join("mygame.bin");
    std::fs::write(&runtime_path, &runtime_bytes).expect("runtime");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(bundle.join("scripts")).expect("bundle dirs");
    std::fs::write(
        bundle.join("scene.json.hyg"),
        "{\"scene\":\"shipped\"}",
    )
    .expect("scene");
    std::fs::write(bundle.join("scripts/patrol.js"), "SPEED = 1").expect("script");

    let fused = dir.join("mygame-fused.bin");
    pack::fuse(&runtime_path, &bundle, &fused).expect("fuses");

    // unpack (the runtime's first step), then MOD: the exe's own
    // directory carries loose files — a replaced script, a replaced
    // scene, and a brand-new file the archive never had. The player
    // creates the layout beside their game.
    std::fs::create_dir_all(fused.parent().unwrap().join("scripts")).expect("mod dirs");
    let extracted = pack::unpack_self(&fused).expect("reads").expect("fused");
    std::fs::write(fused.parent().unwrap().join("scripts/patrol.js"), "SPEED = 99")
        .expect("mod script");
    std::fs::write(
        fused.parent().unwrap().join("scene.json.hyg"),
        "{\"scene\":\"MODDED\"}",
    )
    .expect("mod scene");
    std::fs::write(fused.parent().unwrap().join("scripts/newmod.js"), "NEW").expect("new mod");

    let overlaid = pack::overlay_loose(fused.parent().unwrap(), &extracted).expect("overlays");
    assert!(overlaid >= 3, "scene + script + new file applied: {overlaid}");

    // the mod REPLACED the archive's script
    assert_eq!(
        std::fs::read_to_string(extracted.join("scripts/patrol.js")).expect("read"),
        "SPEED = 99",
        "the mod wins over the archive"
    );
    // the mod ADDED a file
    assert_eq!(
        std::fs::read_to_string(extracted.join("scripts/newmod.js")).expect("read"),
        "NEW"
    );
    // the modded scene IS a different game
    assert_eq!(
        std::fs::read_to_string(extracted.join("scene.json.hyg")).expect("read"),
        "{\"scene\":\"MODDED\"}"
    );
    // untouched archive files remain
    let _ = Path::new(&extracted);
}

/// A filesystem-edge executable (no parent worth walking) overlays
/// nothing rather than walking all of `/`.
#[test]
fn overlay_skips_a_rooted_executable() {
    let dir = temp_dir("hyg-pack-rootedge");
    std::fs::create_dir_all(dir.join("game")).expect("dirs");
    let overlaid = pack::overlay_loose(Path::new("/"), &dir.join("game")).expect("no error");
    assert_eq!(overlaid, 0, "nothing walked from the filesystem root");
}

// ---- the nondestructive extract (--extract) ----

/// Extract skips files that already exist: run it, MODIFY a file, run
/// it again — the modification SURVIVES (the review's "skipping if the
/// file exists so it's nondestructive out the gate"). A fresh
/// extraction still writes everything, and a forced overwrite works —
/// the caller chooses.
#[test]
fn extract_is_nondestructive_by_default() {
    let dir = temp_dir("hyg-pack-extract");

    let runtime_bytes: Vec<u8> = b"RUNTIMEPAYLOAD".to_vec();
    let runtime_path = dir.join("runtime.bin");
    std::fs::write(&runtime_path, &runtime_bytes).expect("runtime");
    let bundle = dir.join("bundle");
    std::fs::create_dir_all(&bundle).expect("bundle dirs");
    std::fs::write(bundle.join("scene.json.hyg"), "{\"scene\":\"v1\"}").expect("scene");
    std::fs::write(bundle.join("mod.js"), "SPEED = 1").expect("script");

    let fused = dir.join("game.bin");
    pack::fuse(&runtime_path, &bundle, &fused).expect("fuses");

    // first extract: both files land
    let out = dir.join("assets-out");
    let written = pack::extract_archive(&fused, &out, true).expect("extracts");
    assert_eq!(written, 2, "a fresh extraction writes everything");

    // the player edits the extracted script
    std::fs::write(out.join("mod.js"), "SPEED = 99 (player tuned)").expect("player edits");

    // re-extract, skip-existing: the edit SURVIVES, nothing rewrites
    let written = pack::extract_archive(&fused, &out, true).expect("re-extracts");
    assert_eq!(written, 0, "everything skipped");
    assert_eq!(
        std::fs::read_to_string(out.join("mod.js")).expect("read"),
        "SPEED = 99 (player tuned)",
        "the player's edit survives the re-extract"
    );

    // an explicit overwrite pass (skip_existing = false) replaces it —
    // the caller chooses destructiveness
    let written = pack::extract_archive(&fused, &out, false).expect("forced extract");
    assert_eq!(written, 2);
    assert_eq!(
        std::fs::read_to_string(out.join("mod.js")).expect("read"),
        "SPEED = 1",
        "the forced pass restores the shipped file"
    );
}
