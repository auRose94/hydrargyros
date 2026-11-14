//! The graph tier — Blueprint-style visual programming over `Var`
//! ([dynamic-design.md](../../docs/dynamic-design.md)). The carrier is a
//! `.hyg` graph document: a serialized `Var` (`graph.json.hyg` while
//! authoring, `.cbor.hyg` when machines exchange big graphs).
//!
//! [`model`] is the document schema and its structural validation —
//! pure data, no feature gate, like the web siblings. The compilers
//! (graph → JavaScript, graph → Rust, graph → WASM) branch from here:
//! **a graph is valid if and only if it converts**, so every structural
//! question this layer can answer, it answers before a compiler ever
//! sees the document.

pub mod model;
