//! File dataStore tests: CRUD, update operators, metadata files, and
//! empty-query behavior against a real temp-dir store.

use hyg::web::store::Store;
use hyg::{a, o};

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("hyg_store_test_{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    root
}

#[test]
fn file_data_store_crud() {
    let root = temp_root("crud");
    let store = Store::open("testdb", &root).unwrap();
    store.create_collection("users").unwrap();

    let inserted = store
        .insert("users", o!{ "name": "alice", "tags": a!["admin", "user"] })
        .unwrap();
    let id = inserted.get("_id").as_string().unwrap().to_string();
    assert_eq!(id.len(), 24);

    let found = store.find_one("users", o!{ "name": "alice" }).unwrap().unwrap();
    assert_eq!(found.get("_id").as_str(), Some(id.as_str()));

    let updated = store
        .update_one("users", o!{ "_id": id.clone() }, o!{ "$set": o!{ "name": "bob" } })
        .unwrap();
    assert_eq!(updated.get("name").as_str(), Some("bob"));

    let found = store
        .find("users", o!{ "tags": o!{ "$in": a!["admin"] } })
        .unwrap();
    assert_eq!(found.len(), 1);

    let deleted = store.delete_one("users", o!{ "_id": id.clone() }).unwrap();
    assert_eq!(deleted.get("deletedCount").as_i64(), Some(1));
    assert!(store.find_one("users", o!{ "_id": id }).unwrap().is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn data_store_update_operators() {
    let root = temp_root("ops");
    let store = Store::open("opsdb", &root).unwrap();
    store.create_collection("c").unwrap();

    let doc = store.insert("c", o!{ "count": 5, "temp": "x" }).unwrap();
    let id = doc.get("_id").as_string().unwrap().to_string();

    // $inc adds to a numeric field
    store
        .update_one("c", o!{ "_id": id.clone() }, o!{ "$inc": o!{ "count": 3 } })
        .unwrap();
    let doc = store.find_one("c", o!{ "_id": id.clone() }).unwrap().unwrap();
    assert_eq!(doc.get("count").as_i64(), Some(8));

    // $unset removes fields
    store
        .update_one("c", o!{ "_id": id.clone() }, o!{ "$unset": o!{ "temp": 1 } })
        .unwrap();
    let doc = store.find_one("c", o!{ "_id": id.clone() }).unwrap().unwrap();
    assert_eq!(doc.get("temp").ty(), hyg::Ty::Null);

    // an update with no supported operator is an error, not a silent
    // success that rewrites nothing
    let err = store
        .update_one("c", o!{ "_id": id.clone() }, o!{ "$bump": o!{ "count": 1 } })
        .unwrap_err();
    assert!(matches!(err, hyg::web::store::StoreError::NoUpdateOperator));

    // queries miss cleanly
    assert!(store.find_one("c", o!{ "_id": "nope" }).unwrap().is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn data_store_metadata_files_are_not_documents() {
    let root = temp_root("meta");
    let store = Store::open("metadb", &root).unwrap();
    store.create_collection("c").unwrap();
    store.insert("c", o!{ "n": 1 }).unwrap();
    let id = store
        .find_one("c", o!{})
        .unwrap()
        .unwrap()
        .get("_id")
        .as_string()
        .unwrap()
        .to_string();

    // the advisory index metadata must never appear as a document...
    store.add_indexes("c", o!{ "n": 1 }).unwrap();
    assert_eq!(store.find("c", o!{}).unwrap().len(), 1);

    // ...and dropIndex clears it through the facade
    store.drop_index("c").unwrap();
    assert!(!root.join("metadb").join("c").join(".index.json").exists());

    // inserting an id that already exists is an error, not a replacement
    let err = store
        .insert("c", o!{ "_id": id.clone(), "n": 99 })
        .unwrap_err();
    assert!(matches!(err, hyg::web::store::StoreError::DuplicateId(_)));
    let doc = store.find_one("c", o!{ "_id": id.clone() }).unwrap().unwrap();
    assert_eq!(doc.get("n").as_i64(), Some(1));

    // replace() keeps the _id of the replaced document
    let replaced = store.replace("c", o!{ "_id": id.clone() }, o!{ "n": 9 }).unwrap();
    assert_eq!(replaced.get("_id").as_str(), Some(id.as_str()));
    let doc = store.find_one("c", o!{ "_id": id }).unwrap().unwrap();
    assert_eq!(doc.get("n").as_i64(), Some(9));

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn empty_query_matches_everything() {
    let root = temp_root("all");
    let store = Store::open("alldb", &root).unwrap();
    store.create_collection("c").unwrap();
    for n in 0..5 {
        store.insert("c", o!{ "n": n }).unwrap();
    }
    assert_eq!(store.find("c", hyg::Null).unwrap().len(), 5);
    assert_eq!(store.find("c", o!{}).unwrap().len(), 5);

    let _ = std::fs::remove_dir_all(&root);
}
