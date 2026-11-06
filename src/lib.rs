//! # Hydrargyros ☿
//!
//! A Rust reimagining of **gold** — hydrargyros, the Greek name for mercury (Hg) — — a framework whose thesis is *high-level,
//! JavaScript-like ergonomics for prototyping without sacrificing speed*.
//!
//! Mercury ports gold's core (`var`/`object`/`list` with prototype
//! inheritance) and its sugar (`o!`, `a!`, `tpl!`, array helpers), and
//! improves on it where Rust lets us: explicit typed reads are borrow-checked
//! and footgun-free, and the shared-data model makes the `VarRef` proxy
//! mutate through handles the way gold does.
//!
//! ```
//! use hydrargyros::{o, Var};
//!
//! let user = o!{
//!     "name": "bob",
//!     "age": 30i64,
//!     "tags": hydrargyros::a!["admin", "dev"],
//!     "stats": o!{ "score": 9i64 },
//! };
//!
//! // write through the shared object (gold's v["k"] = ... semantics)
//! user.at("stats").at("score").put(10i64);
//! assert_eq!(user.get_path("stats.score").as_i64(), Some(10));
//!
//! // typed read
//! let name: String = user.get("name").into();
//! assert_eq!(name, "bob");
//! ```
//!
//! Payloads are `Arc<Mutex>`-shared, so the whole value graph is `Send +
//! Sync`: the same `Var` crosses threads, async tasks, and web handlers.

pub mod from_var;
pub mod game;
pub mod list;
pub mod object;
pub mod proxy;
#[cfg(feature = "script")]
pub mod script;
pub mod sugar;
pub mod var;
pub mod web;

pub use from_var::{FromVar, FromVarError};
pub use list::List;
pub use hydrargyros_derive::FromVar;
pub use object::Object;
pub use proxy::VarRef;
pub use var::{Func, Null, Quat, Ty, Var, Vec3, Vec4};

use std::sync::{Mutex, MutexGuard};

/// Lock a shared payload, ignoring mutex poisoning: a panic while one thread
/// held the lock must not brick the data for every later access (RefCell
/// never poisoned, and the JS-like spirit is "keep going").
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Compile-time proof that the whole value graph crosses threads — the web
// layer's multithreaded tokio runtime (and the game module's future job
// system) lean on it.
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Var>();
    assert_send_sync::<List>();
    assert_send_sync::<Object>();
    assert_send_sync::<Func>();
    assert_send_sync::<VarRef>();
};
