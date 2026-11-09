//! `VarRef` — the read/write proxy, a faithful port of gold's `varRef`.
//!
//! Because `Var` payloads are `Arc<Mutex<…>>`, a `VarRef` can mutate the
//! original object/list in place through one handle — the behavior gold gets
//! from its mutex-guarded shared data. Reads chain (`v.at("a").at("b")`);
//! writes go through `.put()` / `.set()`.
//!
//! The one thing Rust can't reproduce is gold's `varRef::operator=` —
//! `v["k"] = 5` with a bare literal. Rust's `Index` can't run a user-defined
//! conversion during assignment, so writes are an explicit method call:
//!
//! ```
//! # use hyg::Var;
//! let v = Var::from(hyg::o!{ "score": 0i64 });
//! v.at("score").put(10i64);
//! assert_eq!(v.get("score").as_i64(), Some(10));
//! ```
//!
//! Reads are null-safe and feel JS-like: `v.get("k")` returns `Var::Null` on
//! a miss, and you convert with `.as_*()` or `Into<…>`.

use crate::var::Var;

/// A proxy to a single key/index of a `Var`.
pub struct VarRef {
    parent: Var,
    kind: Key,
}

#[derive(Clone)]
enum Key {
    Name(String),
    Index(usize),
}

impl VarRef {
    /// Proxy into an object key.
    pub fn object(parent: Var, key: &str) -> Self {
        VarRef { parent, kind: Key::Name(key.to_string()) }
    }

    /// Proxy into a list index.
    pub fn index(parent: Var, i: usize) -> Self {
        VarRef { parent, kind: Key::Index(i) }
    }

    /// Current value here, or `Null` if the parent isn't obj/list or the key
    /// is missing / index out of range. This is what lets chaining stop
    /// gracefully (`v.at("a").at("b")` on a missing `a` yields Null, not a
    /// panic).
    pub fn value(&self) -> Var {
        match &self.kind {
            Key::Name(k) => match &self.parent {
                Var::Object(o) => o.get(k),
                _ => Var::Null,
            },
            Key::Index(i) => match &self.parent {
                Var::List(l) => l.get(*i),
                _ => Var::Null,
            },
        }
    }

    /// Write through this position. Panics on a non-object/non-list parent
    /// (gold silently no-ops there; failing loudly is the safer default).
    pub fn put(&mut self, value: impl Into<Var>) {
        let value = value.into();
        match &mut self.kind {
            Key::Name(k) => match &self.parent {
                Var::Object(o) => o.set(k.clone(), value),
                _ => panic!("cannot write key into non-object {}", self.parent.ty()),
            },
            Key::Index(i) => match &self.parent {
                Var::List(l) => l.set(*i, value),
                _ => panic!("cannot write index into non-list {}", self.parent.ty()),
            },
        }
    }

    /// Read the type tag at this position (Null if missing).
    pub fn ty(&self) -> crate::var::Ty { self.value().ty() }

    /// Chain into a nested object key: `v.at("a").at("b")`.
    pub fn at(&self, key: &str) -> VarRef {
        VarRef::object(self.value(), key)
    }

    /// Chain into a nested list index.
    pub fn at_idx(&self, i: usize) -> VarRef {
        VarRef::index(self.value(), i)
    }

    /// Convert the current value to a typed read.
    pub fn as_i64(&self) -> Option<i64> { self.value().as_i64() }
    pub fn as_f64(&self) -> Option<f64> { self.value().as_f64() }
    pub fn as_bool(&self) -> Option<bool> { self.value().as_bool() }
    pub fn as_string(&self) -> Option<String> { self.value().as_string() }
}
