//! The file-backed document store — gold's `dataStore` ("file" backend).
//!
//! A [`Store`] is a directory of collections; a collection is a directory of
//! JSON documents named by their `_id` (24 hex chars, Mongo-shaped).
//! Documents are plain [`Var`] objects, queries match fields exactly or via
//! `$in` (membership — against arrays it checks overlap), and updates speak
//! the familiar operators: `$set`, `$unset`, `$inc`. An update with none of
//! them is an error rather than a silent no-op; inserting an `_id` that
//! already exists is an error, not a replacement. Advisory index metadata
//! lives in a `.index.json` file that never surfaces as a document.
//!
//! ```
//! use hyg::web::store::Store;
//! use hyg::{o, Var};
//!
//! # fn demo(dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
//! let store = Store::open("mydb", dir)?;
//! store.create_collection("users")?;
//!
//! let doc = store.insert("users", o!{ "name": "alice", "tags": ["admin", "user"] })?;
//! let id = doc.get("_id").as_string().unwrap().to_string();
//! assert_eq!(id.len(), 24);
//!
//! let found = store.find_one("users", o!{ "name": "alice" })?;
//! assert_eq!(found.unwrap().get("_id").as_str(), Some(id.as_str()));
//!
//! store.update_one("users", o!{ "_id": id }, o!{ "$inc": o!{ "logins": 1 } })?;
//! # Ok(())
//! # }
//! ```

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::object::Object;
use crate::var::Var;

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// Filesystem trouble underneath the store.
    Io(std::io::Error),
    /// `insert` with an `_id` that already exists (an error, not a replace).
    DuplicateId(String),
    /// An update without `$set`/`$unset`/`$inc` — refused, not a silent no-op.
    NoUpdateOperator,
    /// No document matched the query.
    NotFound,
    /// The document argument wasn't an object.
    NotADocument,
    /// A `database` config asked for a backend that doesn't exist.
    UnknownBackend(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Io(e) => write!(f, "io error: {e}"),
            StoreError::DuplicateId(id) => write!(f, "_id {id:?} already exists"),
            StoreError::NoUpdateOperator => {
                write!(f, "update needs $set, $unset or $inc")
            }
            StoreError::NotFound => write!(f, "no matching document"),
            StoreError::NotADocument => write!(f, "document must be an object"),
            StoreError::UnknownBackend(name) => write!(f, "unknown backend {name:?}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self { StoreError::Io(e) }
}

static ID_COUNTER: AtomicU32 = AtomicU32::new(0);

/// A 24-hex-char id, Mongo-shaped: 8 bytes of nanos + 4 of counter.
fn fresh_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{nanos:016x}{counter:08x}")
}

/// A file-backed store: `path/name/{collection}/{_id}.json`.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// Open (creating if needed) the store directory `path/name`.
    pub fn open(name: &str, path: impl AsRef<Path>) -> std::io::Result<Store> {
        let root = path.as_ref().join(name);
        std::fs::create_dir_all(&root)?;
        Ok(Store { root })
    }

    fn coll_dir(&self, coll: &str) -> PathBuf { self.root.join(coll) }

    /// Create a collection directory.
    pub fn create_collection(&self, coll: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(self.coll_dir(coll))
    }

    /// Collections materialize on first write, Mongo-style.
    fn ensure_coll(&self, coll: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(self.coll_dir(coll))
    }

    fn write_doc(&self, coll: &str, id: &str, v: &Var) -> Result<(), StoreError> {
        self.ensure_coll(coll)?;
        Ok(write_json(
            &self.coll_dir(coll).join(format!("{id}.json")),
            v,
        )?)
    }

    /// Insert a document (object). A missing `_id` is generated; an existing
    /// one is an error. Returns the stored document including `_id`.
    pub fn insert(&self, coll: &str, doc: impl Into<Var>) -> Result<Var, StoreError> {
        let out = doc
            .into()
            .as_object()
            .cloned()
            .ok_or(StoreError::NotADocument)?;
        let id = match out.get_own("_id") {
            Some(id) => {
                let id = id.as_string().ok_or(StoreError::NotADocument)?;
                if self.coll_dir(coll).join(format!("{id}.json")).exists() {
                    return Err(StoreError::DuplicateId(id));
                }
                id
            }
            None => {
                let id = fresh_id();
                out.set("_id", Var::from(id.clone()));
                id
            }
        };
        self.write_doc(coll, &id, &Var::from(out.clone()))?;
        Ok(Var::from(out))
    }

    /// All documents matching the query, sorted by `_id` for determinism.
    /// An empty or non-object query matches everything.
    pub fn find(&self, coll: &str, query: impl Into<Var>) -> Result<Vec<Var>, StoreError> {
        let query = query.into();
        let mut docs = Vec::new();
        for id in self.list_ids(coll)? {
            let doc = read_json(&self.coll_dir(coll).join(format!("{id}.json")))?;
            if matches(&doc, &query) {
                docs.push(doc);
            }
        }
        Ok(docs)
    }

    /// The first matching document, or `None`.
    pub fn find_one(&self, coll: &str, query: impl Into<Var>) -> Result<Option<Var>, StoreError> {
        let query = query.into();
        Ok(self.find(coll, query)?.into_iter().next())
    }

    /// Apply `$set`/`$unset`/`$inc` to the first match and return the updated
    /// document. An update carrying none of the operators is an error.
    pub fn update_one(
        &self,
        coll: &str,
        query: impl Into<Var>,
        update: impl Into<Var>,
    ) -> Result<Var, StoreError> {
        let query = query.into();
        let update = update.into();
        let ops = update
            .as_object()
            .ok_or(StoreError::NoUpdateOperator)?;
        if !ops.owns("$set") && !ops.owns("$unset") && !ops.owns("$inc") {
            return Err(StoreError::NoUpdateOperator);
        }
        let (path, doc) = self.first_match(coll, &query)?;
        let doc_obj = doc.as_object().cloned().ok_or(StoreError::NotADocument)?;
        let doc_obj = doc_obj;

        if let Some(set) = ops.get_own("$set") {
            if let Some(set) = set.as_object() {
                for (k, v) in set.entries() {
                    doc_obj.set(k, v);
                }
            }
        }
        if let Some(unset) = ops.get_own("$unset") {
            if let Some(unset) = unset.as_object() {
                for (k, _) in unset.entries() {
                    doc_obj.remove(&k);
                }
            }
        }
        if let Some(inc) = ops.get_own("$inc") {
            if let Some(inc) = inc.as_object() {
                for (k, amount) in inc.entries() {
                    let updated = match (doc_obj.get(&k).as_f64(), amount.as_f64()) {
                        (Some(cur), Some(by)) => {
                            let sum = cur + by;
                            if cur.fract() == 0.0 && by.fract() == 0.0 {
                                Var::I64(sum as i64)
                            } else {
                                Var::F64(sum)
                            }
                        }
                        (None, Some(by)) => {
                            if by.fract() == 0.0 {
                                Var::I64(by as i64)
                            } else {
                                Var::F64(by)
                            }
                        }
                        _ => Var::I64(0),
                    };
                    doc_obj.set(k, updated);
                }
            }
        }

        let out = Var::from(doc_obj);
        write_json(&path, &out)?;
        Ok(out)
    }

    /// Replace the first match with `doc`, keeping the matched document's
    /// `_id` (the file name and payload stay consistent). Returns the new doc.
    pub fn replace(&self, coll: &str, query: impl Into<Var>, doc: impl Into<Var>) -> Result<Var, StoreError> {
        let query = query.into();
        let doc = doc.into();
        let (path, old) = self.first_match(coll, &query)?;
        let id = old.get("_id").as_string().unwrap_or_default().to_string();
        let out = doc.as_object().cloned().ok_or(StoreError::NotADocument)?;
        out.set("_id", Var::from(id));
        let out = Var::from(out);
        write_json(&path, &out)?;
        Ok(out)
    }

    /// Delete the first match; returns `{ "deletedCount": 0 | 1 }`.
    pub fn delete_one(&self, coll: &str, query: impl Into<Var>) -> Result<Var, StoreError> {
        let query = query.into();
        let n = match self.first_match(coll, &query) {
            Ok((path, _)) => {
                std::fs::remove_file(path)?;
                1
            }
            Err(StoreError::NotFound) => 0,
            Err(e) => return Err(e),
        };
        let o = Object::new();
        o.set("deletedCount", Var::I64(n));
        Ok(Var::from(o))
    }

    /// Advisory index bookkeeping (the "file" backend writes a metadata file
    /// that never appears as a document).
    pub fn add_indexes(&self, coll: &str, spec: impl Into<Var>) -> std::io::Result<()> {
        let spec = spec.into();
        self.ensure_coll(coll)?;
        write_json(&self.coll_dir(coll).join(".index.json"), &spec)
    }

    /// Drop a collection's index metadata.
    pub fn drop_index(&self, coll: &str) -> std::io::Result<()> {
        match std::fs::remove_file(self.coll_dir(coll).join(".index.json")) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// First matching document with its file path.
    fn first_match(&self, coll: &str, query: &Var) -> Result<(PathBuf, Var), StoreError> {
        for id in self.list_ids(coll)? {
            let path = self.coll_dir(coll).join(format!("{id}.json"));
            let doc = read_json(&path)?;
            if matches(&doc, query) {
                return Ok((path, doc));
            }
        }
        Err(StoreError::NotFound)
    }

    /// Sorted document ids (`.json` files; dot-files like `.index.json` are
    /// metadata, never documents).
    fn list_ids(&self, coll: &str) -> Result<Vec<String>, StoreError> {
        let mut ids = Vec::new();
        for entry in std::fs::read_dir(self.coll_dir(coll))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !name.ends_with(".json") {
                continue;
            }
            ids.push(name.trim_end_matches(".json").to_string());
        }
        ids.sort();
        Ok(ids)
    }
}

fn write_json(path: &Path, v: &Var) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(v).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, bytes)
}

fn read_json(path: &Path) -> std::io::Result<Var> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Field-exact match, or `$in` membership (against an array field: overlap).
fn matches(doc: &Var, query: &Var) -> bool {
    let Some(q) = query.as_object() else {
        return true;
    };
    for (k, cond) in q.entries() {
        let field = doc.get(&k);
        let ok = match cond.as_object().and_then(|c| c.get_own("$in")) {
            Some(allowed) => match (field.as_list(), allowed.as_list()) {
                (Some(arr), Some(list)) => list
                    .iter()
                    .into_iter()
                    .any(|want| arr.iter().into_iter().any(|got| got == want)),
                _ => allowed
                    .as_list()
                    .is_some_and(|list| list.iter().into_iter().any(|want| want == field)),
            },
            None => field == cond,
        };
        if !ok {
            return false;
        }
    }
    true
}
