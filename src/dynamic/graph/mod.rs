//! The graph tier — Blueprint-style visual programming over `Var`
//! ([dynamic-design.md](../../docs/dynamic-design.md)). The carrier is a
//! `.hyg` graph document: a serialized `Var` (`graph.json.hyg` while
//! authoring, `.cbor.hyg` when machines exchange big graphs).
//!
//! [`model`] is the document schema and its structural validation —
//! pure data, no feature gate, like the web siblings. [`spec`] is the
//! vocabulary and the ONE conversion checker both compilers share —
//! conversion IS validation, and its failures carry graph coordinates.
//! [`to_js`] (M3) is the script seam's compiler; [`route`] runs compiled
//! graphs on a live web server; [`to_rust`] (M4) is the native path. The
//! WASM (M6) compiler branches from here.

pub mod model;
pub mod spec;
pub mod to_js;
pub mod to_rust;

// a graph route needs an engine to run the generated JS and the web
// layer to serve it
#[cfg(all(feature = "script", feature = "web"))]
pub mod route;
