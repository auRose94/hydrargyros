//! The web-layer demo: routes are `Var -> Var` closures, state is one shared
//! `Var` handle cloned into each route, JSON rides serde for free.
//!
//! Run:
//!   cargo run --example web_demo --features web
//!
//! Try:
//!   curl -X POST localhost:8377/echo -H 'content-type: application/json' -d '{"who":"you"}'
//!   curl -X POST localhost:8377/hit  -H 'content-type: application/json' -d '{"who":"bob"}'
//!   curl localhost:8377/stats

use hyg::web::{get_var, post_var, serve};
use hyg::{a, o, Var};

#[tokio::main]
async fn main() {
    // Shared state: one Var handle, cloned per route. Writes through any of
    // them are visible everywhere — the shared-data model, now across an
    // async runtime.
    let state = Var::from(o!{ "hits": 0i64, "log": a![] });
    let hit_state = state.clone();
    let stats_state = state.clone();

    let app = post_var("/echo", |req: Var| req).merge(post_var(
        "/hit",
        move |req: Var| {
            let hits = hit_state.get("hits").as_i64().unwrap_or(0) + 1;
            hit_state.set("hits", hits);
            if let Some(log) = hit_state.get("log").as_list() {
                log.push(req.clone());
            }
            o!{ "hits": hits, "echo": req }
        },
    )).merge(get_var("/stats", move || {
        let log_len = stats_state.get("log").as_list().map(|l| l.len()).unwrap_or(0);
        o!{ "hits": stats_state.get("hits").as_i64().unwrap_or(0), "log_len": log_len }
    }));

    println!("listening on http://127.0.0.1:8377  (POST /echo, POST /hit, GET /stats)");
    serve(app, "127.0.0.1:8377").await.unwrap();
}
