//! The single-file packaging story (M6, the LOVE2D shape): a game ships
//! as ONE file — the runtime binary with a tar archive of its bundle
//! appended to the tail, sealed by a footer.
//!
//! Layout of a fused file:
//!
//! ```text
//! [runtime binary, zero-padded to a 512-byte multiple]
//! [tar archive of the bundle]     ← the game: scene + scripts + assets
//! [footer]                         (52 bytes, see FUSE_FOOTER_SIZE)
//! ```
//!
//! The footer:
//!
//! ```text
//! [magic "HYGFUSE2"][u64 LE archive length][u32 LE flags][32-byte version, NUL-padded]
//! ```
//!
//! Flags (bit 0 set = the archive is gzip): the compression knob —
//! `hyg fuse --compress` shrinks the payload; a runtime built with the
//! `pack-compress` feature decompresses it, one built without refuses
//! with a message that names the rebuild. The version stamp records
//! the hyg that fused the game (the runtime prints it on launch — a
//! jam submission that misbehaves names its build).
//!
//! Padding aligns the archive so ordinary tar tools can also read it
//! straight off an UNcompressed fused file. The runtime, launched
//! bare, reads its OWN tail: footer present → unpack the archive into
//! a temp directory and play the scene it finds there.
//!
//! Feature `pack` (the `tar` crate): fuse + unpack are data in → files
//! out, so the round trip runs and tests headless. Feature
//! `pack-compress` (flate2, the miniz_oxide backend) adds the
//! compression knob.

use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

/// The footer's magic: eight bytes before the archive length.
const FUSE_MAGIC: &[u8; 8] = b"HYGFUSE2";
/// Archive flag bit 0: the archive is gzip (the compression knob).
const FLAG_GZIP: u32 = 1;
/// The version stamp's width — every hyg version fits.
const VERSION_WIDTH: usize = 32;
/// The footer's size: magic + u64 length + u32 flags + the version.
const FOOTER_SIZE: u64 = 8 + 8 + 4 + VERSION_WIDTH as u64;
/// The alignment the runtime is padded to before the archive appends.
const ALIGN: u64 = 512;

/// What a fuse produces: the sizes (the `--metrics` story), whether
/// the archive compressed, and the version stamped into the footer.
#[derive(Debug, Clone, PartialEq)]
pub struct FuseStats {
    /// The fused file's total size.
    pub fused_bytes: u64,
    /// The archive before compression.
    pub archive_raw: usize,
    /// The archive as stored (equal to `archive_raw` uncompressed).
    pub archive_stored: usize,
    /// Whether the archive is gzip.
    pub compressed: bool,
    /// The version stamped into the footer.
    pub version: &'static str,
}

/// Fuse knobs. Default: uncompressed (bandwidth over disk — the M6
/// decision; `--compress` opts into gzip).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FuseOptions {
    /// Gzip the archive before appending.
    pub compress: bool,
}

/// Detect a fused archive at `path`'s tail and read it in: the archive
/// bytes, its flags, and the version stamp. `None` when the file
/// carries no footer (a bare runtime or a too-short file).
///
/// # Errors
/// The file can't be read, the footer claims an archive longer than
/// the file, or a gzip archive meets a build without `pack-compress`.
fn read_footer(path: &Path) -> Result<Option<(Vec<u8>, bool, String)>, String> {
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
    let flags = u32::from_le_bytes(
        tail[16..20]
            .try_into()
            .expect("the footer carries 4 flag bytes"),
    );
    let version = String::from_utf8_lossy(&tail[20..20 + VERSION_WIDTH])
        .trim_end_matches('\0')
        .to_string();
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
    Ok(Some((
        archive_bytes,
        flags & FLAG_GZIP != 0,
        version,
    )))
}

/// The fused file's footer facts, without the archive bytes — the
/// runtime's launch line and the tooling's introspection.
///
/// # Errors
/// The file can't be read.
pub fn fuse_info(path: &Path) -> Result<Option<FuseInfo>, String> {
    Ok(read_footer(path)?.map(|(archive_bytes, compressed, version)| FuseInfo {
        archive_len: archive_bytes.len() as u64,
        compressed,
        version,
    }))
}

/// A fused file's footer facts (sizes only — no archive read).
#[derive(Debug, Clone, PartialEq)]
pub struct FuseInfo {
    /// The stored archive's length.
    pub archive_len: u64,
    /// Whether the archive is gzip.
    pub compressed: bool,
    /// The version stamp (the hyg that fused the game).
    pub version: String,
}

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

/// The hyg that fuses the game — stamped into every footer.
pub const FUSE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// FUSE: pad the runtime to [`ALIGN`], append the bundle's tar
/// (gzip-compressed under [`FuseOptions::compress`] — needs the
/// `pack-compress` feature), append the footer. Overwrites `out` — a
/// fuse is reproducible by construction.
///
/// # Errors
/// The runtime or the bundle can't be read, compression was requested
/// without the `pack-compress` feature, or the write fails.
pub fn fuse(
    runtime: &Path,
    bundle_dir: &Path,
    out: &Path,
    options: FuseOptions,
) -> Result<FuseStats, String> {
    let mut runtime_bytes = std::fs::read(runtime)
        .map_err(|e| format!("{}: {e}", runtime.display()))?;
    let archive_raw = tar_dir(bundle_dir)?;

    let (archive_stored, compressed) = if options.compress {
        #[cfg(feature = "pack-compress")]
        {
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder
                .write_all(&archive_raw)
                .map_err(|e| format!("compressing the archive: {e}"))?;
            (encoder.finish().map_err(|e| format!("compressing the archive: {e}"))?, true)
        }
        #[cfg(not(feature = "pack-compress"))]
        {
            let _ = &archive_raw;
            return Err(
                "compression needs the pack-compress feature (rebuild with --features pack-compress)"
                    .to_string(),
            );
        }
    } else {
        (archive_raw.clone(), false)
    };

    // zero-pad the runtime so the archive starts 512-aligned (ordinary
    // tar tools can then read an UNcompressed archive off the fused file)
    let remainder = runtime_bytes.len() as u64 % ALIGN;
    if remainder != 0 {
        runtime_bytes.resize(runtime_bytes.len() + (ALIGN - remainder) as usize, 0);
    }

    let mut version_bytes = [0u8; VERSION_WIDTH];
    let version = FUSE_VERSION;
    let copy_len = version.len().min(VERSION_WIDTH);
    version_bytes[..copy_len].copy_from_slice(&version.as_bytes()[..copy_len]);

    let mut fused = runtime_bytes;
    fused.extend_from_slice(&archive_stored);
    fused.extend_from_slice(FUSE_MAGIC);
    fused.extend_from_slice(&(archive_stored.len() as u64).to_le_bytes());
    fused.extend_from_slice(&(if compressed { FLAG_GZIP } else { 0 }).to_le_bytes());
    fused.extend_from_slice(&version_bytes);

    std::fs::write(out, &fused).map_err(|e| format!("{}: {e}", out.display()))?;

    // the fused game must RUN BARE — that's the whole shape — so the
    // runtime's permissions carry to the output (the executable bit
    // most of all; a chmod-by-hand after every fuse is a trip hazard)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(runtime)
            .map_err(|e| format!("{}: {e}", runtime.display()))?
            .permissions()
            .mode();
        std::fs::set_permissions(out, std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("{}: {e}", out.display()))?;
    }

    Ok(FuseStats {
        fused_bytes: fused.len() as u64,
        archive_raw: archive_raw.len(),
        archive_stored: archive_stored.len(),
        compressed,
        version,
    })
}

/// The archive of a fused file, decompressed when the footer says so.
///
/// # Errors
/// The file isn't fused, or a gzip archive meets a build without
/// `pack-compress` (the message names the rebuild).
fn archive_of(path: &Path) -> Result<Option<(Vec<u8>, bool, String)>, String> {
    let Some((archive_bytes, compressed, version)) = read_footer(path)? else {
        return Ok(None);
    };
    if compressed {
        #[cfg(feature = "pack-compress")]
        {
            let mut decoder = flate2::read::MultiGzDecoder::new(&archive_bytes[..]);
            let mut raw = Vec::new();
            decoder
                .read_to_end(&mut raw)
                .map_err(|e| format!("decompressing the archive: {e}"))?;
            return Ok(Some((raw, compressed, version)));
        }
        #[cfg(not(feature = "pack-compress"))]
        {
            let _ = &archive_bytes;
            return Err(format!(
                "{path:?} carries a COMPRESSED game archive — this runtime lacks the \
                 pack-compress feature (rebuild with --features pack-compress)"
            ));
        }
    }
    Ok(Some((archive_bytes, compressed, version)))
}

/// Detect a fused archive at the tail of `path` and unpack it into a
/// temp directory (`hyg-run-<exe-stem>`). `None` when the file isn't
/// fused (a bare runtime — nothing to extract, nothing cleaned).
///
/// # Errors
/// The file can't be read or the extraction fails.
pub fn unpack_self(path: &Path) -> Result<Option<PathBuf>, String> {
    let Some((archive_bytes, _compressed, _version)) = archive_of(path)? else {
        return Ok(None);
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

/// MODDING: loose files beside the executable OVERRIDE the fused
/// archive. Walks `override_root` (the directory the game file lives
/// in) and copies every file over its same-relative-path sibling in
/// `extraction` — replacing archive files, and ADDING files the
/// archive never had (a modded scene referencing new scripts/assets
/// brings them along). The union, mods winning: players own the game.
///
/// Guards: a root at the filesystem's edge (no parent, or the root
/// itself) is skipped — walking all of `/` would be catastrophic; a
/// per-file copy failure warns and continues (one unreadable mod file
/// must not brick the launch).
///
/// Returns how many files were overlaid.
pub fn overlay_loose(override_root: &Path, extraction: &Path) -> Result<usize, String> {
    // the exe's directory must be a real directory with a parent of its
    // own (the filesystem root and drive edges have no parent to speak
    // of — walking them would be catastrophic)
    let root = override_root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", override_root.display()))?;
    if root.parent().is_none() {
        return Ok(0);
    }
    let mut overlaid = 0usize;
    let mut stack = vec![root.clone()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(e) => {
                eprintln!("hyg: mods: skipping {}: {e}", current.display());
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = match path.strip_prefix(&root) {
                Ok(relative) => relative.to_path_buf(),
                Err(_) => continue,
            };
            let target = extraction.join(&relative);
            if let Some(parent) = target.parent() {
                if std::fs::create_dir_all(parent).is_err() {
                    continue;
                }
            }
            match std::fs::copy(&path, &target) {
                Ok(_) => overlaid += 1,
                Err(e) => {
                    eprintln!("hyg: mods: {}: {e}", path.display());
                }
            }
        }
    }
    Ok(overlaid)
}

/// Extract a fused file's archive into `out_dir`. With `skip_existing`,
/// a file already on disk is LEFT ALONE — nondestructive out of the
/// gate (the review's ask: `--extract` gives players the full asset
/// set without clobbering their edits; run it again after a mod and
/// the mod survives).
///
/// Returns how many entries were written (skipped ones don't count).
///
/// # Errors
/// The file isn't fused, or the extraction fails mid-way.
pub fn extract_archive(
    fused: &Path,
    out_dir: &Path,
    skip_existing: bool,
) -> Result<usize, String> {
    let Some((archive_bytes, _compressed, _version)) = archive_of(fused)? else {
        return Err(format!("{} carries no fused archive", fused.display()));
    };

    std::fs::create_dir_all(out_dir).map_err(|e| format!("{}: {e}", out_dir.display()))?;
    let mut written = 0usize;
    {
        let mut archive = tar::Archive::new(&archive_bytes[..]);
        let entries = archive
            .entries()
            .map_err(|e| format!("reading the archive: {e}"))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| format!("archive entry: {e}"))?;
            let path = entry.path().map_err(|e| format!("archive entry path: {e}"))?;
            let dest = out_dir.join(&path);
            if skip_existing && dest.exists() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            entry.unpack(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            written += 1;
        }
    }
    Ok(written)
}
