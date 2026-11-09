//! Web-layer tests: drive the Router through tower's oneshot — real
//! request/response cycles without binding a port. The shared-state test runs
//! on a multithreaded tokio runtime, exactly the configuration the old
//! `Rc`/`RefCell` core would have refused to compile under.

#![cfg(feature = "web")]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use hyg::web::{get_var, post_var};
use hyg::{o, Var};
use tower::ServiceExt;

fn post_json(uri: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_var(res: axum::response::Response) -> Var {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn echo_roundtrip() {
    let app = post_var("/echo", |req: Var| req);
    let res = app.oneshot(post_json("/echo", r#"{"who":"you","n":3}"#)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_var(res).await;
    assert_eq!(v.get("who").as_str(), Some("you"));
    assert_eq!(v.get("n").as_i64(), Some(3));
}

#[tokio::test]
async fn get_route_returns_json() {
    let app = get_var("/hello", || o!{ "msg": "hi" });
    let res = app
        .oneshot(Request::builder().uri("/hello").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let v = body_var(res).await;
    assert_eq!(v.get("msg").as_str(), Some("hi"));
}

#[tokio::test]
async fn malformed_json_is_rejected() {
    let app = post_var("/echo", |req: Var| req);
    let res = app.oneshot(post_json("/echo", "{not json")).await.unwrap();
    assert!(res.status().is_client_error());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_state_across_requests_multithreaded() {
    // one shared Var handle cloned into a route closure; writes persist across
    // requests — and this compiles and runs on a multithreaded runtime only
    // because the whole value graph is Send + Sync.
    let state = Var::from(o!{ "hits": 0i64 });
    let hit_state = state.clone();
    let app = post_var("/hit", move |req: Var| {
        let hits = hit_state.get("hits").as_i64().unwrap_or(0) + req.as_i64().unwrap_or(1);
        hit_state.set("hits", hits);
        o!{ "hits": hits }
    });

    for expected in [5i64, 10] {
        let res = app.clone().oneshot(post_json("/hit", "5")).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(body_var(res).await.get("hits").as_i64(), Some(expected));
    }
}
