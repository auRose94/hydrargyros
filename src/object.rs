//! `Object` — an ordered map of named `Var`, shared via `Arc<Mutex<…>>`, with
//! prototype inheritance (`set_parent`/`inherits`).
//!
//! Insertion order is preserved, so we use
//! `indexmap::IndexMap` — the wheel for that, no hand-rolled ordered map. The
//! payload is `Arc<Mutex<IndexMap>>` so the `VarRef` proxy can mutate the
//! shared object in place — and so the whole value graph is `Send + Sync`.
//! The prototype is `Arc<Object>` (an `Object` is already internally shared).

use std::fmt;
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;

use crate::var::{set_path_impl, Var};

#[derive(Clone, Default)]
pub struct Object {
    /// Own keys (insertion-ordered), interior-mutable so handles can mutate
    /// through one another.
    entries: OwnEntries,
    /// The prototype chain head. Lookups that miss own data walk this.
    parent: Option<Arc<Object>>,
}

/// Shared, interior-mutable ordered map of own keys.
type OwnEntries = Arc<Mutex<IndexMap<String, Var>>>;

impl std::fmt::Debug for Object {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.entries()).finish()
    }
}

impl PartialEq for Object {
    fn eq(&self, other: &Self) -> bool {
        // Snapshot each side (locks released) so comparing an object against
        // its own clone can't self-deadlock.
        let a = self.entries();
        let b = other.entries();
        a.len() == b.len()
            && a.iter()
                .all(|(k, v)| b.iter().any(|(bk, bv)| bk == k && bv == v))
    }
}

// ---- serde ----
//
// An Object serializes as a JSON object of its own keys (the prototype chain
// is not serialized — it's behavior, not data).

impl serde::Serialize for Object {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (k, v) in self.entries() {
            map.serialize_entry(k.as_str(), &v)?;
        }
        map.end()
    }
}

impl<'de> serde::Deserialize<'de> for Object {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{MapAccess, Visitor};
        use std::fmt;

        struct ObjectVisitor;

        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Object;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a JSON object")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Object, A::Error> {
                let o = Object::new();
                while let Some((k, v)) = map.next_entry::<String, Var>()? {
                    o.set(k, v);
                }
                Ok(o)
            }
        }

        deserializer.deserialize_map(ObjectVisitor)
    }
}

impl Object {
    pub fn new() -> Self { Object::default() }

    /// True if `key` is declared in this object's own data (not the parent).
    pub fn owns(&self, key: &str) -> bool { crate::lock(&self.entries).contains_key(key) }

    /// Own value for `key`, ignoring the prototype chain.
    pub fn get_own(&self, key: &str) -> Option<Var> {
        crate::lock(&self.entries).get(key).cloned()
    }

    /// Value for `key`, walking the prototype chain on miss.
    /// Returns `Null` on a miss — the JS `obj[k]` → `undefined` semantics.
    /// Locks one object at a time, so a chain walk never holds two locks.
    pub fn get(&self, key: &str) -> Var {
        if let Some(v) = self.get_own(key) {
            return v;
        }
        self.parent.as_ref().map(|p| p.get(key)).unwrap_or(Var::Null)
    }

    /// Proxy into `key` (the `obj["k"]` slot), for chained read/write.
    pub fn at(&self, key: &str) -> crate::proxy::VarRef {
        crate::proxy::VarRef::object(Var::from(self.clone()), key)
    }

    /// Read a dotted path: `o.get_path("stats.score")`.
    pub fn get_path(&self, path: &str) -> Var {
        let mut cur = Var::from(self.clone());
        for part in path.split('.') {
            cur = cur.get(part);
        }
        cur
    }

    /// Write a dotted path: `o.set_path("stats.score", 10)`, creating missing
    /// intermediate objects (JS `obj.a.b = 1` semantics).
    pub fn set_path(&self, path: &str, value: impl Into<Var>) {
        set_path_impl(&Var::from(self.clone()), path, value.into());
    }

    /// Set an own key. Mutates the shared map in place.
    pub fn set(&self, key: impl Into<String>, value: Var) {
        crate::lock(&self.entries).insert(key.into(), value);
    }

    /// Insert `key` as the *first* entry (used by the `o!` macro, which builds
    /// the tail recursively and prepends the head to preserve source order).
    pub fn prepend(&self, key: impl Into<String>, value: Var) {
        let key = key.into();
        let mut entries = crate::lock(&self.entries);
        let rest: Vec<(String, Var)> = entries
            .iter()
            .filter(|(k, _)| k.as_str() != key)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        entries.clear();
        entries.insert(key, value);
        for (k, v) in rest {
            entries.insert(k, v);
        }
    }

    /// Copy all own entries from another object (used by the `o!` macro to
    /// build forward order). Later keys overwrite earlier ones.
    pub fn extend(&self, other: &Object) {
        // Snapshot `other` first: if it shares this object's payload, locking
        // both sides at once would deadlock.
        let snapshot = other.entries();
        let mut entries = crate::lock(&self.entries);
        for (k, v) in snapshot {
            entries.insert(k, v);
        }
    }

    /// Remove an own key.
    pub fn remove(&self, key: &str) -> Option<Var> {
        crate::lock(&self.entries).shift_remove(key)
    }

    pub fn len(&self) -> usize { crate::lock(&self.entries).len() }
    pub fn is_empty(&self) -> bool { crate::lock(&self.entries).is_empty() }

    /// Snapshot of own entries (cloned). Holding a lock across user code isn't
    /// expressible, so this returns owned copies — fine for display/debug/
    /// iteration; hot paths should use `get`/`get_own`.
    pub fn entries(&self) -> Vec<(String, Var)> {
        crate::lock(&self.entries)
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// Attach a prototype.
    pub fn set_parent(&mut self, parent: Object) {
        self.parent = Some(Arc::new(parent));
    }

    /// Does `self` inherit (directly or transitively) from `other`? Identity is
    /// checked via the shared `entries` allocation, which uniquely tags an
    /// object instance.
    pub fn inherits(&self, other: &Object) -> bool {
        let other_ptr = Arc::as_ptr(&other.entries);
        let mut cur = &self.parent;
        while let Some(p) = cur {
            if Arc::as_ptr(&p.entries) == other_ptr { return true; }
            cur = &p.parent;
        }
        false
    }
}

impl fmt::Display for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{{")?;
        for (i, (k, v)) in self.entries().iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            write!(f, "{k}: {v}")?;
        }
        write!(f, "}}")
    }
}
