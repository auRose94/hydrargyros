//! The `dynamic` module — behavior loaded and changed while the process
//! stays up ([dynamic-design.md](../../docs/dynamic-design.md)). M0 is the
//! foundation: the script-engine registry ([`engine`]), with the v12
//! JavaScript engine as its first backend ([`v12`], behind the `script`
//! feature). FFI plugins and visual graphs branch from the roadmap's
//! design; nothing here moves until the registry proves itself.

pub mod engine;
#[cfg(feature = "script")]
pub mod v12;
