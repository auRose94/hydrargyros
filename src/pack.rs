//! The single-file packaging story (M6, the LOVE2D shape): a game ships
//! as ONE file — the runtime binary with a tar archive of its bundle
//! appended to the tail, sealed by a footer.
//!
//! Layout of a fused file:
//!
//! ```text
//! [runtime binary, zero-padded to a 512-byte multiple]
//! [tar archive of the bundle]     ← the game: scene + scripts + assets
//! [magic "HYGFUSE1"][u64 LE tar length]   (the 16-byte footer)
//! ```
//!
//! Padding aligns the archive so ordinary tar tools can also read it
//! straight off the fused file. The runtime, launched bare, reads its
//! OWN tail: footer present → unpack the archive into a temp directory
//! and play the scene it finds there.
//!
//! Feature `pack` (the `tar` crate): fuse + unpack are data in → files
//! out, so the round trip runs and tests headless.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

/// The footer's magic: eight bytes before the archive length.
const FUSE_MAGIC: &[u8; 8] = b"HYGFUSE1";
/// The footer's size: magic + u64 archive length.
const FOOTER_SIZE: u64 = 16;
/// The alignment the runtime is padded to before the archive appends.
const ALIGN: u64 = 512;

/// Tar a directory tree in memory: files only (a bundle is files —
/// `bundle::build` lays the scene and its references out flat-ish),
/// paths relative to `dir` with forward slashes (ustar's convention).
fn tar_dir(dir: &Path) -> Result<Vec<u8>, String> {
    let mut tar_bytes = Vec::new();
    {
        let mut builder = tar::Builder::new(&mut tar_bytes);
        let mut stack = vec![dir.to_path_buf()];
        while let Some(current) = stack.pop() {
            let entries = std::fs::read_dir(&current)
                .map_err(|e| format!("{}: {e}", current.display()))?;
            for entry in entries {
                let entry = entry.map_err(|e| format!("{}: {e}", current.display()))?;
                let path = entry.path();
                let relative = path
                    .strip_prefix(dir)
                    .expect("walked inside dir")
                    .to_string_lossy()
                    .replace('\\', "/");
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let mut file = std::fs::File::open(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                builder
                    .append_file(relative, &mut file)
                    .map_err(|e| format!("archiving {}: {e}", path.display()))?;
            }
        }
        builder
            .finish()
            .map_err(|e| format!("finishing the archive: {e}"))?;
    }
    Ok(tar_bytes)
}

/// FUSE: pad the runtime to [`ALIGN`], append the bundle's tar, append
/// the footer. Overwrites `out` — a fuse is reproducible by
/// construction. Returns the fused file's size.
///
/// # Errors
/// The runtime or the bundle can't be read, or the write fails.
pub fn fuse(runtime: &Path, bundle_dir: &Path, out: &Path) -> Result<u64, String> {
    let mut runtime_bytes = std::fs::read(runtime)
        .map_err(|e| format!("{}: {e}", runtime.display()))?;
    let archive = tar_dir(bundle_dir)?;

    // zero-pad the runtime so the archive starts 512-aligned (ordinary
    // tar tools can then read it off the fused file)
    let remainder = runtime_bytes.len() as u64 % ALIGN;
    if remainder != 0 {
        runtime_bytes.resize(runtime_bytes.len() + (ALIGN - remainder) as usize, 0);
    }

    let mut fused = runtime_bytes;
    fused.extend_from_slice(&archive);
    fused.extend_from_slice(FUSE_MAGIC);
    fused.extend_from_slice(&(archive.len() as u64).to_le_bytes());

    std::fs::write(out, &fused).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(fused.len() as u64)
}

/// Detect a fused archive at the tail of `path` and unpack it into a
/// temp directory (`hyg-run-<exe-stem>`). `None` when the file isn't
/// fused (a bare runtime — nothing to extract, nothing cleaned).
///
/// # Errors
/// The file can't be read or the extraction fails.
pub fn unpack_self(path: &Path) -> Result<Option<PathBuf>, String> {
    let size = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size < FOOTER_SIZE {
        return Ok(None);
    }
    let tail = {
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.seek(std::io::SeekFrom::Start(size - FOOTER_SIZE))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut tail = Vec::new();
        file.read_to_end(&mut tail)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        tail
    };
    if tail[..8] != *FUSE_MAGIC {
        return Ok(None);
    }
    let archive_len = u64::from_le_bytes(
        tail[8..16]
            .try_into()
            .expect("the footer carries 8 length bytes"),
    );
    let archive_start = size
        .checked_sub(FOOTER_SIZE)
        .and_then(|tail_start| tail_start.checked_sub(archive_len))
        .ok_or_else(|| "corrupt footer: archive longer than the file".to_string())?;

    let archive_bytes = {
        let mut file = std::fs::File::open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        file.seek(std::io::SeekFrom::Start(archive_start))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let mut bytes = vec![0u8; archive_len as usize];
        file.read_exact(&mut bytes)
            .map_err(|e| format!("reading the archive: {e}"))?;
        bytes
    };

    // the extraction home: temp + the exe's stem (a fused "mygame.bin"
    // extracts to hyg-run-mygame — stable across runs of the same game)
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("game")
        .to_string();
    let extraction = std::env::temp_dir().join(format!("hyg-run-{stem}"));
    std::fs::create_dir_all(&extraction)
        .map_err(|e| format!("{}: {e}", extraction.display()))?;
    {
        let mut archive = tar::Archive::new(&archive_bytes[..]);
        archive
            .unpack(&extraction)
            .map_err(|e| format!("unpacking into {}: {e}", extraction.display()))?;
    }
    std::io::stdout().flush().ok();
    Ok(Some(extraction))
}

/// The scene file inside an extracted bundle: the `.json.hyg` at the
/// extraction's root (the bundler lays the scene there).
#[must_use]
pub fn scene_in(root: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| e == "hyg")
                    .unwrap_or(false)
        })
}
