//! The graph tier — Blueprint-style visual programming over `Var`
//! ([dynamic-design.md](../../docs/dynamic-design.md)). The carrier is a
//! `.hyg` graph document: a serialized `Var` (`graph.json.hyg` while
//! authoring, `.cbor.hyg` when machines exchange big graphs).
//!
//! [`model`] is the document schema and its structural validation —
//! pure data, no feature gate, like the web siblings. [`to_js`] is the
//! first compiler (M3): conversion IS validation, and its failures walk
//! the flow chain. [`route`] runs compiled graphs through the script
//! seam on a live web server. The Rust (M4) and WASM (M6) compilers
//! branch from here.

pub mod model;
pub mod to_js;

// a graph route needs an engine to run the generated JS and the web
// layer to serve it
#[cfg(all(feature = "script", feature = "web"))]
pub mod route;
