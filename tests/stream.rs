//! Streaming-window tests: the render buffer becomes a web page, frames
//! stream over WebSocket, and browser events come back through
//! `WindowSystem::poll` as `Var` objects — the whole loop an AI (or a test)
//! can drive without a GUI.

#![cfg(all(feature = "stream", feature = "wgpu"))]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use hydrargyros::game::render::FrameSink;
use hydrargyros::game::window::WindowSystem;
use hydrargyros::game::{RenderBackend, StreamingWindow, WgpuRenderer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMsg;

/// A rendered frame over the sink, so the stream has something to push.
fn render_one_frame(sink: &FrameSink) {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(64, 48))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    renderer.render_frame();
}

#[tokio::test]
async fn page_serves_and_frames_stream() {
    let sink = FrameSink::new();
    render_one_frame(&sink);

    let window = StreamingWindow::new("127.0.0.1:0", sink.clone(), false).unwrap();
    let url = window.url();
    let addr = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    tokio::time::sleep(Duration::from_millis(5000)).await;

    // the page is a real HTML canvas app
    let mut tcp = TcpStream::connect(&addr).await.unwrap();
    tcp.write_all(b"GET / HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let page = String::from_utf8_lossy(&buf);
    assert!(page.contains("200 OK"), "{}", &page[..page.len().min(80)]);
    assert!(page.contains("<canvas"));
    assert!(page.contains("putImageData"));

    // the binary frame arrives as [w: u32][h: u32][rgba…]
    let (stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let (_ws_tx, mut ws_rx) = stream.split();
    let first = tokio::time::timeout(Duration::from_secs(5), ws_rx.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let WsMsg::Binary(data) = first else {
        panic!("expected a binary frame")
    };
    assert!(data.len() > 8, "frame payload");
    let w = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let h = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    assert_eq!((w, h), (64, 48));
    assert_eq!(data.len(), 8 + (w * h * 4) as usize);
    // the renderer's clear color, sRGB-encoded, in a corner pixel
    let want = 89; // srgb8(0.1)
    assert!(
        (data[8] as i32 - want).abs() <= 3,
        "corner red: {} vs {want}",
        data[8]
    );

    // a second render produces a second frame (generation bump)
    render_one_frame(&sink);
    let second = tokio::time::timeout(Duration::from_secs(5), ws_rx.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(second, WsMsg::Binary(_)),
        "new frame after re-render"
    );
}

#[tokio::test]
async fn browser_events_arrive_as_vars() {
    let sink = FrameSink::new();
    render_one_frame(&sink);

    let mut window = StreamingWindow::new("127.0.0.1:0", sink, false).unwrap();
    let addr = window
        .url()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();

    let (stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws"))
        .await
        .unwrap();
    let (mut ws_tx, mut ws_rx) = stream.split();
    // consume the first pushed frame so the send/recv interleave cleanly
    let _ = tokio::time::timeout(Duration::from_secs(5), ws_rx.next()).await;

    // the "browser": key down, key up, a click
    for event in [
        r#"{"type":"key","key":"w","pressed":true}"#,
        r#"{"type":"key","key":"w","pressed":false}"#,
        r#"{"type":"mouse","action":"down","button":0,"x":42,"y":17}"#,
    ] {
        ws_tx.send(WsMsg::Text(event.into())).await.unwrap();
    }

    let events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut got = Vec::new();
        while got.len() < 3 {
            for ev in window.poll() {
                got.push(ev);
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        got
    })
    .await
    .unwrap();

    assert_eq!(events[0].get("type").as_str(), Some("key"));
    assert_eq!(events[0].get("key").as_str(), Some("w"));
    assert_eq!(events[0].get("pressed").as_bool(), Some(true));
    assert_eq!(events[1].get("pressed").as_bool(), Some(false));
    assert_eq!(events[2].get("type").as_str(), Some("mouse"));
    assert_eq!(events[2].get("x").as_i64(), Some(42));
    assert_eq!(events[2].get("y").as_i64(), Some(17));

    // the page's quit button is just another event — the AI's stop button
    ws_tx
        .send(WsMsg::Text(r#"{"type":"quit"}"#.into()))
        .await
        .unwrap();
    let quit = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            for ev in window.poll() {
                if ev.get("type").as_str() == Some("quit") {
                    return ev;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(quit.get("type").as_str(), Some("quit"));
}

/// A raw HTTP GET over a real TCP connection (the tests' real-traffic
/// pattern) — returns the full response text.
async fn http_get(addr: &str, path: &str) -> String {
    let mut tcp = TcpStream::connect(addr).await.unwrap();
    tcp.write_all(
        format!("GET {path} HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf).into_owned()
}

#[tokio::test]
async fn toolbox_and_qr_are_served() {
    let sink = FrameSink::new();
    render_one_frame(&sink);

    let window = StreamingWindow::new("127.0.0.1:0", sink.clone(), false).unwrap();
    let addr = window
        .url()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    tokio::time::sleep(Duration::from_millis(5000)).await;

    // the page: toolbox menu with the QR reference, plus the mobile
    // surface (viewport meta, no-touch-zoom canvas, bilinear fit upscale)
    let page = http_get(&addr, "/").await;
    assert!(page.contains("200 OK"), "{}", &page[..page.len().min(80)]);
    assert!(page.contains("toolbox"), "toolbox menu present");
    assert!(page.contains("src=\"/qr\""), "the QR image rides the menu");
    assert!(!page.contains("__HANDOFF_URL__"), "URL injected");
    assert!(page.contains("http://"), "a hand-off URL is in the page");
    assert!(page.contains("viewport-fit=cover"), "mobile viewport meta");
    assert!(
        page.contains("touch-action: none"),
        "no canvas double-tap zoom"
    );
    assert!(
        page.contains("image-rendering: auto"),
        "bilinear fit upscale"
    );

    // /qr is a real SVG QR: svg + path markers, image/svg+xml content type
    let qr = http_get(&addr, "/qr").await;
    assert!(qr.contains("200 OK"), "{}", &qr[..qr.len().min(80)]);
    assert!(qr.contains("image/svg+xml"), "svg content type");
    assert!(qr.contains("<svg"), "svg root");
    assert!(qr.contains("<path"), "module path");

    // no token armed: the URL stays clean
    assert!(!window.url().contains("?t="), "token is opt-in");
}

#[tokio::test]
async fn token_gates_page_and_ws() {
    let sink = FrameSink::new();
    render_one_frame(&sink);

    let window = StreamingWindow::new("127.0.0.1:0", sink, false)
        .unwrap()
        .with_token();
    let url = window.url().to_string();
    let (base, query) = url.split_once("?t=").expect("tokenized URL");
    let token = query.to_string();
    assert!(
        token.len() >= 16 && token.chars().all(|c| c.is_ascii_hexdigit()),
        "a random hex token: {token}"
    );
    let addr = base
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    tokio::time::sleep(Duration::from_millis(5000)).await;

    // page without the token → 403 with a one-line page
    let denied = http_get(&addr, "/").await;
    assert!(
        denied.contains("403"),
        "{}",
        &denied[..denied.len().min(80)]
    );
    // with the token → the real page
    let granted = http_get(&addr, &format!("/?t={token}")).await;
    assert!(
        granted.contains("200 OK"),
        "{}",
        &granted[..granted.len().min(80)]
    );
    assert!(granted.contains("<canvas"));

    // the WS upgrade validates too: refused without the token, works with
    let refused = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await;
    match refused {
        Err(tokio_tungstenite::tungstenite::Error::Http(resp)) => {
            assert_eq!(resp.status(), 403, "upgrade refused with 403");
        }
        other => panic!("expected an HTTP-level refusal, got: {other:?}"),
    }
    let (stream, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?t={token}"))
        .await
        .unwrap();
    let (_ws_tx, mut ws_rx) = stream.split();
    let first = tokio::time::timeout(Duration::from_secs(5), ws_rx.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(first, WsMsg::Binary(_)),
        "frames flow with the token"
    );

    // /qr always answers, and with a token armed it embeds the tokenized URL
    let qr = http_get(&addr, "/qr").await;
    assert!(qr.contains("200 OK") && qr.contains("<svg"), "qr serves");
}

#[cfg(all(feature = "winit", feature = "wgpu"))]
mod native_view_helpers {
    // the native view's pure seams: the loop can't run in CI, but the
    // letterbox fit math and the frame-skip rule are extracted and tested
    use hydrargyros::game::native_view::{fit_extents, should_present};

    #[test]
    fn fit_extents_letterboxes_any_aspect() {
        // same aspect → full-bleed
        assert_eq!(fit_extents((1920, 1080), (640, 360)), [1.0, 1.0]);
        // image wider than the surface → full width, bars top/bottom;
        // the on-screen rect keeps the image's aspect exactly
        let wide = fit_extents((1920, 1080), (640, 240));
        assert_eq!(wide[0], 1.0);
        assert!((wide[1] - (1920.0 / 1080.0) / (640.0 / 240.0)).abs() < 1e-6);
        assert!(wide[1] < 1.0);
        let on_screen = (wide[0] * 1920.0) / (wide[1] * 1080.0);
        assert!((on_screen - 640.0 / 240.0).abs() < 1e-4, "aspect preserved");
        // image taller → pillars left/right
        let tall = fit_extents((1920, 1080), (360, 640));
        assert_eq!(tall[1], 1.0);
        assert!(tall[0] < 1.0);
        let on_screen = (tall[0] * 1920.0) / (tall[1] * 1080.0);
        assert!((on_screen - 360.0 / 640.0).abs() < 1e-4, "aspect preserved");
    }

    #[test]
    fn should_present_skips_seen_frames() {
        assert!(!should_present(5, None), "no frame → nothing to present");
        assert!(!should_present(5, Some(5)), "same generation → skip");
        assert!(!should_present(5, Some(3)), "older → skip");
        assert!(should_present(5, Some(6)), "newer generation → present");
        assert!(should_present(0, Some(1)), "first frame always presents");
    }
}
