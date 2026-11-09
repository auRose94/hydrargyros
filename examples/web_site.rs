//! A multi-page homage to gold's `myWebProject` example: shared state,
//! pages built with the HTML builder, form handling, a JSON API, path
//! params, query strings, WebSocket chat, static assets, and styled 404s —
//! all over `Var`.
//!
//! Run:
//!   cargo run --example web_site --features web
//!
//! Then: http://127.0.0.1:8377 (users, login form, chat, /api/*, a 404 page).

use hyg::web::html::{self as h, Node};
use hyg::web::{Response, Server, WsHandlers};
use hyg::{o, Var};

// ---- page frame (the builder in its natural habitat) ----

fn page(title: &str, active: &str, body: Node) -> Response {
    Response::html(
        h::html([
            Node::attrs(o!{ "lang": "en" }),
            h::head([
                h::meta([Node::attrs(o!{ "charset": "utf-8" })]),
                h::title([Node::text(format!("{title} — Hg"))]),
                h::link([Node::attrs(o!{ "rel": "stylesheet", "href": "/assets/site.css" })]),
            ]),
            h::body([navbar(active), h::main([body])]),
        ])
        .render(),
    )
}

fn navbar(active: &str) -> Node {
    let link = |href: &str, label: &str| {
        let cls = if href == active { "active" } else { "" };
        h::a([Node::text(label), Node::attrs(o!{ "href": href, "class": cls })])
    };
    h::nav([
        h::strong([Node::text("Hg")]),
        link("/", "home"),
        link("/users", "users"),
        link("/chat", "chat"),
    ])
}

const DEFAULT_CSS: &str = r#"
:root { color-scheme: dark; }
body { font-family: system-ui, sans-serif; margin: 0; background: #14161a; color: #e8e6e3; }
nav { display: flex; gap: 1rem; padding: 1rem 1.5rem; background: #1d2026; border-bottom: 1px solid #2a2e36; }
nav a { color: #9aa3b2; text-decoration: none; padding: .2rem .5rem; border-radius: 6px; }
nav a.active, nav a:hover { color: #fff; background: #2a2e36; }
main { max-width: 40rem; margin: 2rem auto; padding: 0 1rem; }
h1 { font-weight: 600; } code, pre { background: #1d2026; border-radius: 6px; }
code { padding: .1rem .35rem; } pre { padding: 1rem; overflow-x: auto; min-height: 3rem; }
label { display: block; margin: 1rem 0 .3rem; }
input { background: #1d2026; color: inherit; border: 1px solid #2a2e36; border-radius: 6px; padding: .5rem .75rem; }
button { background: #4f6df5; color: #fff; border: 0; border-radius: 6px; padding: .5rem 1rem; margin-top: 1rem; }
.error { color: #ff7b72; } .card { background: #1d2026; border: 1px solid #2a2e36; border-radius: 10px; padding: 1rem 1.25rem; margin: 1rem 0; }
"#;

fn ensure_assets() {
    std::fs::create_dir_all("public").ok();
    let css = std::path::Path::new("public/site.css");
    if !css.exists() {
        std::fs::write(css, DEFAULT_CSS).ok();
    }
}

fn login_form(error: Option<&str>) -> Response {
    let mut items = vec![h::h1([Node::text("Log in")])];
    if let Some(e) = error {
        items.push(h::p([Node::text(e), Node::attrs(o!{ "class": "error" })]));
    }
    items.push(h::form([
        h::label([Node::text("Name"), Node::attrs(o!{ "for": "name" })]),
        h::input([Node::attrs(o!{ "id": "name", "name": "name" })]),
        h::button([Node::text("go")]),
        Node::attrs(o!{ "method": "post", "action": "/login" }),
    ]));
    page("login", "/login", h::div(items))
}

#[tokio::main]
async fn main() {
    ensure_assets();

    // The "database": one shared Var handle. Each route captures its own
    // clone — writes through any of them are visible everywhere.
    let users = Var::from(o!{
        "1": o!{ "name": "alice", "role": "admin" },
        "2": o!{ "name": "bob", "role": "user" },
    });
    let users_home = users.clone();
    let users_list = users.clone();
    let users_detail = users.clone();
    let users_api = users.clone();

    let mut serv = Server::with_config(o!{ "host": "127.0.0.1", "port": 8377 });

    serv
        // pages
        .get("/", move |_req| {
            let n = users_home.as_object().map(|u| u.len()).unwrap_or(0);
            page(
                "home",
                "/",
                h::div([
                    h::h1([Node::text("Mercury web")]),
                    h::p([Node::text("Gold's expressjs-like server, in Rust: routes are Var closures.")]),
                    h::ul([
                        h::li([Node::text(format!("{n} registered users"))]),
                        h::li([Node::text("try "), h::code([Node::text("/hello/hyg?shout=1")])]),
                        h::li([Node::text("chat over WebSocket at "), h::code([Node::text("/chat")])]),
                        h::li([Node::text("JSON API at "), h::code([Node::text("/api/users")])]),
                    ]),
                ]),
            )
        })
        .get("/login", move |_req| login_form(None))
        .post("/login", |req| {
            // form bodies arrive as a Var object, decoded
            let name = req.get("body").get("name").as_string().unwrap_or_default();
            if name.is_empty() {
                return login_form(Some("A name is required."));
            }
            page(
                "welcome",
                "/",
                h::div([
                    h::h1([Node::text(format!("Hi {name}!"))]),
                    h::p([Node::text("Form handling, gold-style.")]),
                ]),
            )
        })
        .get("/users", move |_req| {
            let mut rows: Vec<Node> = Vec::new();
            for (id, user) in users_list.as_object().map(|u| u.entries()).unwrap_or_default() {
                rows.push(h::div([
                    h::a([
                        Node::text(format!("#{id} {}", user.get("name").as_string().unwrap_or_default())),
                        Node::attrs(o!{ "href": format!("/users/{id}") }),
                    ]),
                    Node::text(format!(" — {}", user.get("role").as_string().unwrap_or_default())),
                    Node::attrs(o!{ "class": "card" }),
                ]));
            }
            page("users", "/users", h::div([h::h1([Node::text("Users")]), h::div(rows)]))
        })
        .get("/users/:id", move |req| {
            let id = req.get("params").get("id").as_string().unwrap_or_default();
            match users_detail.get(&id) {
                user if user.ty() == hyg::Ty::Object => page(
                    "user",
                    "/users",
                    h::div([
                        h::h1([Node::text(format!("#{id} {}", user.get("name").as_string().unwrap_or_default()))]),
                        h::p([Node::text(format!("role: {}", user.get("role").as_string().unwrap_or_default()))]),
                    ]),
                ),
                _ => Response::text("no such user").status(404),
            }
        })
        .get("/hello/:name", |req| {
            let name = req.get("params").get("name").as_string().unwrap_or_default();
            let shout = req.get("query").get("shout").as_string().unwrap_or_default() == "1";
            let greeting = format!("hello, {name}!");
            if shout {
                Var::from(greeting.to_uppercase())
            } else {
                Var::from(greeting)
            }
        })
        // JSON API
        .get("/api/users", move |_req| Response::json(users_api.clone()))
        .post("/api/echo", |req| {
            // JSON bodies arrive as Var; malformed input arrives as Null
            let body = req.get("body");
            Response::json(o!{ "echo": body, "ok": body.ty() != hyg::Ty::Null }).status(201)
        })
        // WebSocket chat (echo)
        .ws(
            "/ws",
            WsHandlers::new()
                .on_open(|sock| {
                    sock.send(r#"{"hello": "click anywhere, then say something — I'll echo it"}"#);
                })
                .on_message(|sock, data| {
                    sock.send(data.as_string().unwrap_or_default());
                })
                .on_close(|_sock| {}),
        )
        .get("/chat", move |_req| {
            page(
                "chat",
                "/chat",
                h::div([
                    h::h1([Node::text("Chat")]),
                    h::pre([Node::attrs(o!{ "id": "log" })]),
                    h::script([Node::text(
                        r#"const log = document.getElementById("log");
const ws = new WebSocket("ws://" + location.host + "/ws");
ws.onmessage = (e) => { log.textContent += e.data + "\n"; };
document.body.addEventListener("click", () => { const m = prompt("message"); if (m) ws.send(m); });"#,
                    )]),
                ]),
            )
        })
        // static files: ./public served under /assets
        .mount("/assets", "public")
        // styled 404 — gold's setErrorHandler hook
        .on_error(|req| {
            let path = req.get("path").as_string().unwrap_or_default();
            page(
                "404",
                "",
                h::div([
                    h::h1([Node::text("404")]),
                    h::p([
                        Node::text("No route or asset matches "),
                        h::code([Node::text(path)]),
                        Node::text("."),
                    ]),
                ]),
            )
            .status(404)
        });

    println!("serving on http://127.0.0.1:8377  (pages, /api/*, /chat, /assets/*)");
    serv.start().await.unwrap();
}
