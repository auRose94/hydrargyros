//! The web module — HTML, CSS, storage, and HTTP.
//!
//! Four siblings:
//!
//! - **[`html`]** — the HTML5 builder and parser (elements as data, escaping,
//!   entity decoding, malformed-input recovery). Feeds this module *and* the
//!   future ui renderer.
//! - **[`css`]** — the CSS parser (rules, typed declarations, `!important`).
//! - **[`store`]** — the file-backed document store (collections, `$set`/
//!   `$unset`/`$inc` updates, `$in` queries).
//! - **`http`** — the expressjs-like [`Server`](http::Server) facade over
//!   axum: verbs, `:param`/`*` patterns, WebSocket routes, static mounts,
//!   graceful `start()`/`stop()`. This part is behind the `web` feature (it
//!   pulls axum + tokio); the parsers and the store are pure and always
//!   available.
//!
//! ```ignore
//! // (doctests are feature-gated; see examples/web_site.rs and tests/web.rs)
//! let mut serv = Server::new(());
//! serv.get("/users/:id", |req| {
//!     let id = req.get("params").get("id").as_string().unwrap_or_default();
//!     Response::json(o!{ "id": id })
//! });
//! serv.start().await?;
//! ```

pub mod css;
pub mod database;
pub mod html;
pub mod store;

#[cfg(feature = "web")]
pub mod http;

#[cfg(feature = "web")]
pub use http::{get_var, post_var, req_accepting_html, req_accepting_json, req_header,
    req_is_json, req_is_www_form_urlencoded, serve, Response, Server, WsHandlers, WsSock};
