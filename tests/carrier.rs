//! The `.hyg` carrier contracts: the terminal `.hyg` suffix is the type,
//! the preceding suffix names the format, a bare `.hyg` is JSON, and the
//! round trip is Var-identical.

use hyg::{carrier, func, o, Var};
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hyg-carrier-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn json_round_trip_is_var_identical() {
    let dir = scratch("json");
    let path = dir.join("doc.json.hyg");
    // insertion order, nesting, numbers of both kinds
    let doc = Var::from(o! {
        "name": "order-flow",
        "format": 1i64,
        "score": 2.5f64,
        "tags": hyg::a!["a", "b"],
        "nested": o!{ "deep": o!{ "x": 1i64 } },
    });
    carrier::write(&path, &doc).unwrap();
    let back = carrier::read(&path).unwrap();
    assert_eq!(back, doc);

    // pretty-printed on disk — the human-readable contract
    let bytes = std::fs::read(&path).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains('\n'), "expected pretty-printed JSON");

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn bare_hyg_is_json() {
    let dir = scratch("bare");
    let json_path = dir.join("doc.json.hyg");
    let bare_path = dir.join("doc.hyg");
    let doc = Var::from(o!{ "bare": true });
    carrier::write(&json_path, &doc).unwrap();
    std::fs::copy(&json_path, &bare_path).unwrap();
    assert_eq!(carrier::read(&bare_path).unwrap(), doc);
    // and writing to a bare .hyg works too — the writer picks JSON
    let other = dir.join("other.hyg");
    carrier::write(&other, &doc).unwrap();
    assert_eq!(carrier::read(&other).unwrap(), doc);

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn unknown_format_is_explicit() {
    let dir = scratch("unknown");
    let path = dir.join("doc.cbor.hyg");
    std::fs::write(&path, b"whatever").unwrap();
    let err = carrier::read(&path).unwrap_err();
    assert!(matches!(err, carrier::CarrierError::UnsupportedFormat(ref f) if f == "cbor"));
    // the write side refuses the same filename — the format is a filename
    // contract, never a content-type guess
    assert!(carrier::write(&path, &Var::Null).is_err());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn not_carrier_is_explicit() {
    let dir = scratch("notcarrier");
    let path = dir.join("doc.json");
    std::fs::write(&path, b"{}").unwrap();
    let err = carrier::read(&path).unwrap_err();
    assert!(matches!(err, carrier::CarrierError::NotCarrier(_)));
    assert!(carrier::write(&path, &Var::Null).is_err());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn write_creates_parent_dirs() {
    let dir = scratch("parents");
    let path = dir.join("deep").join("nested").join("doc.json.hyg");
    carrier::write(&path, &Var::from(o!{ "ok": 1i64 })).unwrap();
    assert_eq!(carrier::read(&path).unwrap().get("ok").as_i64(), Some(1));

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn parse_failure_is_loud() {
    let dir = scratch("garbage");
    let path = dir.join("doc.json.hyg");
    std::fs::write(&path, b"{ not json").unwrap();
    let err = carrier::read(&path).unwrap_err();
    assert!(matches!(err, carrier::CarrierError::Parse(_)));
    // the error names the trouble
    assert!(!err.to_string().is_empty());

    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn func_handles_round_trip_within_process() {
    // the carrier is faithful Var serde — function handles ride it through
    // the process-global registry (unknown ids degrade to Null cross-process)
    let dir = scratch("func");
    let path = dir.join("doc.json.hyg");
    let doc = Var::from(o!{
        "run": func!(|| Var::from("ran")),
    });
    carrier::write(&path, &doc).unwrap();
    let back = carrier::read(&path).unwrap();
    assert_eq!(back.get("run").call(&[]).as_str(), Some("ran"));

    std::fs::remove_dir_all(dir).ok();
}
