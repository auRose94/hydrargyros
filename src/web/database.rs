//! The database facade — config in, collection handles out, over a
//! store backend.
//!
//! `database({{"backend","file"},{"name","mydb"},{"path","./data"}})` +
//! `connect()` becomes [`Database::connect`]; `getCollection("users")`
//! becomes [`Database::get_collection`]. The `"file"` backend wraps
//! [`Store`]; the facade is where other backends would slot in.
//!
//! ```
//! use hyg::web::database::Database;
//! use hyg::o;
//!
//! # fn demo(dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
//! let db = Database::connect(o!{ "backend": "file", "name": "mydb", "path": dir })?;
//! let users = db.get_collection("users");
//!
//! let doc = users.insert(o!{ "name": "alice", "logins": 0 })?;
//! let id = doc.get("_id").as_string().unwrap().to_string();
//! users.update_one(o!{ "_id": id }, o!{ "$inc": o!{ "logins": 1 } })?;
//! assert_eq!(users.find_one(o!{ "name": "alice" })?.unwrap().get("logins").as_i64(), Some(1));
//! # Ok(())
//! # }
//! ```

use super::store::{Store, StoreError};
use crate::var::Var;

/// The database facade over one backend store.
#[derive(Clone)]
pub struct Database {
    store: Store,
}

/// A named collection handle — the [`Database::get_collection`] return value.
#[derive(Clone)]
pub struct Collection {
    store: Store,
    name: String,
}

impl Database {
    /// Connect from config: `{ "backend": "file", "name": ...,
    /// "path": ... }` (`name` defaults to `"db"`, `path` to `"."`). Only the
    /// `"file"` backend exists.
    pub fn connect(config: impl Into<Var>) -> Result<Database, StoreError> {
        let cfg = config.into();
        let backend = cfg.get("backend").as_string().unwrap_or_else(|| "file".to_string());
        if backend != "file" {
            return Err(StoreError::UnknownBackend(backend));
        }
        let name = cfg.get("name").as_string().unwrap_or_else(|| "db".to_string());
        let path = cfg.get("path").as_string().unwrap_or_else(|| ".".to_string());
        Ok(Database { store: Store::open(&name, path)? })
    }

    /// A handle to one collection (created on first write).
    pub fn get_collection(&self, name: &str) -> Collection {
        Collection { store: self.store.clone(), name: name.to_string() }
    }
}

impl Collection {
    /// Insert a document; a missing `_id` is generated, a duplicate is an
    /// error. Returns the stored document including `_id`.
    pub fn insert(&self, doc: impl Into<Var>) -> Result<Var, StoreError> {
        self.store.insert(&self.name, doc)
    }

    /// All documents matching the query, sorted by `_id`.
    pub fn find(&self, query: impl Into<Var>) -> Result<Vec<Var>, StoreError> {
        self.store.find(&self.name, query)
    }

    /// The first matching document, or `None`.
    pub fn find_one(&self, query: impl Into<Var>) -> Result<Option<Var>, StoreError> {
        self.store.find_one(&self.name, query)
    }

    /// Apply `$set`/`$unset`/`$inc` to the first match; returns the updated doc.
    pub fn update_one(
        &self,
        query: impl Into<Var>,
        update: impl Into<Var>,
    ) -> Result<Var, StoreError> {
        self.store.update_one(&self.name, query, update)
    }

    /// Replace the first match, keeping its `_id`; returns the new document.
    pub fn replace(
        &self,
        query: impl Into<Var>,
        doc: impl Into<Var>,
    ) -> Result<Var, StoreError> {
        self.store.replace(&self.name, query, doc)
    }

    /// Delete the first match; returns `{ "deletedCount": 0 | 1 }`.
    pub fn delete_one(&self, query: impl Into<Var>) -> Result<Var, StoreError> {
        self.store.delete_one(&self.name, query)
    }

    /// Advisory index bookkeeping.
    pub fn add_indexes(&self, spec: impl Into<Var>) -> std::io::Result<()> {
        self.store.add_indexes(&self.name, spec)
    }

    /// Drop this collection's index metadata.
    pub fn drop_index(&self) -> std::io::Result<()> {
        self.store.drop_index(&self.name)
    }
}
