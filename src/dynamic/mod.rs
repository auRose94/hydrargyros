//! The `dynamic` module — behavior loaded and changed while the process
//! stays up ([dynamic-design.md](../../docs/dynamic-design.md)).
//!
//! Three faces of one function table:
//!
//! - [`engine`] — the script-engine registry (M0): script BACKENDS are
//!   pluggable; the v12 JavaScript engine is the first ([`v12`], behind
//!   the `script` feature).
//! - [`functions`] — the callable-function table: every callable is a
//!   [`HostFn`](engine::HostFn) closure over `Var` args plus a
//!   [`FunctionMeta`](functions::FunctionMeta) descriptor. Plugins,
//!   scripts, and (at the graph milestone) function nodes all land here.
//! - [`ffi`] — native FFI plugins (M1, behind `dynamic-ffi`): the
//!   trusted tier, hot-swappable while the process stays up.
//! - [`graph`] — the graph tier (M2): the `.hyg` graph-document schema
//!   and its structural validation (feature-free). The compilers —
//!   graph → JavaScript / Rust / WASM, where **conversion IS
//!   validation** — branch from here.
//!
//! and the WASM tier branches after the compilers.

pub mod engine;
#[cfg(feature = "dynamic-ffi")]
pub mod ffi;
pub mod functions;
pub mod graph;
#[cfg(feature = "script")]
pub mod v12;
#[cfg(feature = "dynamic-wasm")]
pub mod wasm;
