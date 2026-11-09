//! `List` — an ordered dynamic array of `Var`, shared via `Arc<Mutex<…>>`.
//!
//! The ordered-array shape, with `Arc<Mutex<Vec>>` for the
//! payload: cheap copy-by-pointer (shared-mutable) and interior mutability,
//! which is what lets the `VarRef` proxy mutate the original list in place
//! through one handle. `Arc`/`Mutex` (rather than `Rc`/`RefCell`) is what
//! makes the whole value graph `Send + Sync`, so the same list crosses
//! threads, async tasks, and route handlers.

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::var::Var;

#[derive(Clone, Default)]
pub struct List {
    items: Items,
}

/// Shared, interior-mutable ordered array.
type Items = Arc<Mutex<Vec<Var>>>;

impl std::fmt::Debug for List {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl PartialEq for List {
    fn eq(&self, other: &Self) -> bool {
        // Snapshot each side and drop its lock before touching the other:
        // comparing a list against its own clone would self-deadlock a
        // non-reentrant Mutex held across both sides.
        let a: Vec<Var> = crate::lock(&self.items).clone();
        let b: Vec<Var> = crate::lock(&other.items).clone();
        a == b
    }
}

impl FromIterator<Var> for List {
    fn from_iter<T: IntoIterator<Item = Var>>(iter: T) -> Self {
        let items: Vec<Var> = iter.into_iter().collect();
        List { items: Arc::new(Mutex::new(items)) }
    }
}

impl List {
    pub fn new() -> Self { List::default() }

    pub fn len(&self) -> usize { crate::lock(&self.items).len() }

    pub fn is_empty(&self) -> bool { crate::lock(&self.items).is_empty() }

    /// Read index `i`. Panics out of range (could return `Option` later).
    pub fn get(&self, i: usize) -> Var {
        crate::lock(&self.items)[i].clone()
    }

    /// Alias for `get` matching the `Var`-style `get_idx` naming.
    pub fn get_idx(&self, i: usize) -> Var { self.get(i) }

    /// Snapshot iterator (clones each element). Holding a lock across user
    /// code isn't expressible (and could self-deadlock), so this returns
    /// owned `Var`s.
    pub fn iter(&self) -> Vec<Var> {
        crate::lock(&self.items).iter().cloned().collect()
    }

    /// Push an element onto the shared array. Takes `&self` — the payload is
    /// interior-mutable, so pushes through any handle are seen by all.
    pub fn push(&self, v: Var) {
        crate::lock(&self.items).push(v);
    }

    /// Set index `i` (used by the `VarRef` proxy). Grows with nulls if needed,
    /// JS-style (`arr[i] = x` extends the array).
    pub fn set(&self, i: usize, v: Var) {
        let mut items = crate::lock(&self.items);
        while items.len() <= i {
            items.push(Var::Null);
        }
        items[i] = v;
    }

    /// Insert an element at the *front* (used by the `a!` macro, which builds
    /// the tail recursively and prepends the head to preserve source order).
    pub fn prepend(&self, v: Var) {
        crate::lock(&self.items).insert(0, v);
    }

    /// Append all elements of another list (used by the `a!` macro to build
    /// forward order). Takes `&self` — interior-mutable payload.
    pub fn extend(&self, other: &List) {
        // Snapshot `other` first: if it shares this list's payload, locking
        // both sides at once would deadlock.
        let snapshot = other.iter();
        crate::lock(&self.items).extend(snapshot);
    }
}

impl fmt::Display for List {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let items = crate::lock(&self.items);
        write!(f, "[")?;
        for (i, v) in items.iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            write!(f, "{v}")?;
        }
        write!(f, "]")
    }
}
