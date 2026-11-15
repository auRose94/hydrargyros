//! The `.hyg` carrier type — one file type for every structured hyg
//! document (scenes, graph documents, project configs, theme templates,
//! exported stores).
//!
//! A `.hyg` file is a serialized [`Var`]. The **terminal `.hyg` suffix is
//! the type** — one file association, one "Open with Studio" — and the
//! **preceding suffix names the serde carrier format**: `.json.hyg` is the
//! default and the human-readable home for most files (diffable, mergeable,
//! greppable). Anything serde can parse qualifies — that is the point of
//! "one `Serialize`/`Deserialize` impl on `Var`, any format for free",
//! surfaced as a naming convention instead of a hidden choice.
//!
//! The two rules, as designed:
//!
//! - **Loading:** strip `.hyg`, match the preceding suffix against the
//!   known serde formats; a bare `.hyg` file is JSON (the accessible
//!   default). An unknown suffix is an explicit error — the format is a
//!   filename contract, never a content-type guess.
//! - **Writing:** the writer picks the format from the requested filename,
//!   likewise. `.json.hyg` writes pretty-printed JSON.
//!
//! Dense carriers (`.cbor.hyg`/`.bson.hyg`) are the designed per-file
//! upgrade for heavy documents; JSON is the only format this build carries
//! (no extra deps in the core), and unsupported suffixes name that.
//!
//! ```
//! use hyg::{o, Var};
//!
//! let dir = std::env::temp_dir().join(format!("hyg-carrier-doctest-{}", std::process::id()));
//! let path = dir.join("demo.json.hyg");
//! let doc = Var::from(o!{ "demo": "hello ☿" });
//! hyg::carrier::write(&path, &doc).unwrap();
//!
//! let back = hyg::carrier::read(&path).unwrap();
//! assert_eq!(back, doc);
//! // the bare suffix is the same document — JSON is the accessible default
//! let bare = dir.join("demo.hyg");
//! std::fs::copy(&path, &bare).unwrap();
//! assert_eq!(hyg::carrier::read(&bare).unwrap(), doc);
//!
//! std::fs::remove_dir_all(dir).ok();
//! ```

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::var::Var;

/// Why a carrier operation failed.
#[derive(Debug)]
pub enum CarrierError {
    /// Filesystem trouble along the path.
    Io(std::io::Error),
    /// The filename does not end in `.hyg` — not a hyg document.
    NotCarrier(PathBuf),
    /// The preceding suffix names a format this build can't parse (the
    /// dense carriers are a designed upgrade, not shipped yet).
    UnsupportedFormat(String),
    /// The bytes didn't parse as the filename's format.
    Parse(String),
}

impl std::fmt::Display for CarrierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CarrierError::Io(e) => write!(f, "{e}"),
            CarrierError::NotCarrier(path) => {
                write!(f, "{path:?}: not a .hyg document (the terminal .hyg suffix is the type)")
            }
            CarrierError::UnsupportedFormat(format) => write!(
                f,
                ".{format}.hyg — unknown carrier format (this build reads: .json.hyg, and a bare .hyg as JSON)"
            ),
            CarrierError::Parse(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CarrierError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CarrierError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// The carrier format a filename names: `Some("json")` for `.json.hyg`,
/// `None` for a bare `.hyg` (the JSON default). `Err` when the terminal
/// suffix isn't `.hyg` at all, or the preceding suffix names no known
/// format.
fn format_of(path: &Path) -> Result<Option<String>, CarrierError> {
    if path.extension() != Some(OsStr::new("hyg")) {
        return Err(CarrierError::NotCarrier(path.to_path_buf()));
    }
    let stem = path.file_stem().unwrap_or_default();
    let format = Path::new(stem)
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_string);
    if let Some(format) = &format {
        if format != "json" {
            return Err(CarrierError::UnsupportedFormat(format.clone()));
        }
    }
    Ok(format)
}

/// Read a `.hyg` document into a `Var`.
///
/// # Errors
/// [`CarrierError::NotCarrier`] when the filename doesn't end in `.hyg`,
/// [`CarrierError::UnsupportedFormat`] when the preceding suffix names no
/// known format, [`CarrierError::Parse`] when the bytes don't parse,
/// [`CarrierError::Io`] for filesystem trouble.
pub fn read(path: &Path) -> Result<Var, CarrierError> {
    // the format gate already rejected unknown suffixes; what remains is
    // .json.hyg and the bare .hyg sniff — JSON is the accessible default,
    // so "sniff" is simply the JSON parse (its error names the trouble)
    format_of(path)?;
    let bytes = std::fs::read(path).map_err(CarrierError::Io)?;
    serde_json::from_slice(&bytes).map_err(|e| CarrierError::Parse(e.to_string()))
}

/// Write a `Var` as a `.hyg` document, the format picked from the
/// filename. Creates parent directories. `.json.hyg` (and a bare `.hyg`)
/// writes pretty-printed JSON — scenes and configs are code-adjacent data.
///
/// # Errors
/// [`CarrierError::NotCarrier`] / [`CarrierError::UnsupportedFormat`] for
/// filenames that don't name a known format, [`CarrierError::Io`] for
/// filesystem trouble.
pub fn write(path: &Path, doc: &Var) -> Result<(), CarrierError> {
    format_of(path)?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(CarrierError::Io)?;
        }
    }
    let bytes = serde_json::to_vec_pretty(doc).map_err(|e| CarrierError::Parse(e.to_string()))?;
    std::fs::write(path, bytes).map_err(CarrierError::Io)
}
