//! The `database` facade — a config object in, collection handles out.

use hyg::web::database::Database;
use hyg::{a, o};

fn temp_root(tag: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("hyg_db_test_{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    root
}

#[test]
fn connect_get_collection_crud() {
    let root = temp_root("crud");
    let db = Database::connect(o!{ "backend": "file", "name": "mydb", "path": root.display().to_string() }).unwrap();
    let users = db.get_collection("users");

    let doc = users.insert(o!{ "name": "alice", "logins": 0 }).unwrap();
    let id = doc.get("_id").as_string().unwrap().to_string();
    assert_eq!(id.len(), 24);

    users
        .update_one(o!{ "_id": id.clone() }, o!{ "$inc": o!{ "logins": 1 } })
        .unwrap();
    let found = users.find_one(o!{ "name": "alice" }).unwrap().unwrap();
    assert_eq!(found.get("logins").as_i64(), Some(1));

    // $in against an array field
    users
        .insert(o!{ "name": "bob", "tags": a!["admin", "user"] })
        .unwrap();
    let admins = users.find(o!{ "tags": o!{ "$in": a!["admin"] } }).unwrap();
    assert_eq!(admins.len(), 1);
    assert_eq!(admins[0].get("name").as_str(), Some("bob"));

    let replaced = users
        .replace(o!{ "name": "alice" }, o!{ "name": "alice", "role": "admin" })
        .unwrap();
    assert_eq!(replaced.get("_id").as_str(), Some(id.as_str()));

    let deleted = users.delete_one(o!{ "name": "alice" }).unwrap();
    assert_eq!(deleted.get("deletedCount").as_i64(), Some(1));
    assert!(users.find_one(o!{ "name": "alice" }).unwrap().is_none());

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn unknown_backend_is_refused() {
    match Database::connect(o!{ "backend": "redis", "name": "x", "path": "." }) {
        Ok(_) => panic!("unknown backend must not connect"),
        Err(hyg::web::store::StoreError::UnknownBackend(name)) => assert_eq!(name, "redis"),
        Err(e) => panic!("wrong error: {e}"),
    }
}

#[test]
fn defaults_apply_when_config_is_null() {
    // Null config = file backend, "db" name, "." path — connect succeeds
    let db = Database::connect(hyg::Null).unwrap();
    let c = db.get_collection("smoke");
    let doc = c.insert(o!{ "ok": true }).unwrap();
    assert!(doc.get("_id").as_string().is_some());
    // clean up the default-dir store created under ./
    let _ = std::fs::remove_dir_all("./db");
}
