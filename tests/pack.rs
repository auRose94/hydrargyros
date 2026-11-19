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
