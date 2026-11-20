//! Shared CLI plumbing for the `hyg` and `hyg-runtime` binaries: the
//! argument helpers and the report style — human lines by default,
//! one machine-readable JSON document under `--metrics` (the corporate
//! ask: metric collections a build script can consume, `jq`-shaped).

/// Whether `name` appears as a bare flag.
// shared module, two binaries, different feature sets — a helper this
// build never calls is another build's plumbing
#[allow(dead_code)]
pub fn flag(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// The value following `name`, if present.
#[allow(dead_code)]
pub fn value(args: &[String], name: &str) -> Option<std::path::PathBuf> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(std::path::PathBuf::from)
}

/// Print a metrics document: compact JSON on one line — build scripts
/// pipe it, humans ignore it. Only under `--metrics`.
#[allow(dead_code)]
pub fn metrics(args: &[String], document: serde_json::Value) {
    if flag(args, "--metrics") {
        println!("{}", serde_json::to_string(&document).expect("metrics serialize"));
    }
}

/// A `K`/`M` human size.
#[allow(dead_code)]
pub fn human(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes / 1024)
    }
}
