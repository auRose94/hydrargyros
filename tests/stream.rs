//! Streaming-window tests: the render buffer becomes a web page, frames
//! stream over WebSocket, and browser events come back through
//! `WindowSystem::poll` as `Var` objects — the whole loop an AI (or a test)
//! can drive without a GUI.

#![cfg(feature = "stream")]

use std::time::Duration;

// the websocket-driving imports ride the wgpu-gated frame tests (AGENTS.md:
// gate the import, never delete it — clippy without features lies)
#[cfg(feature = "wgpu")]
use futures_util::{SinkExt, StreamExt};
use hydrargyros::game::render::FrameSink;
#[cfg(feature = "wgpu")]
use hydrargyros::game::window::WindowSystem;
use hydrargyros::game::StreamingWindow;
#[cfg(feature = "wgpu")]
use hydrargyros::game::{RenderBackend, WgpuRenderer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
#[cfg(feature = "wgpu")]
use tokio_tungstenite::tungstenite::Message as WsMsg;

/// A rendered frame over the sink, so the stream has something to push.
#[cfg(feature = "wgpu")]
fn render_one_frame(sink: &FrameSink) {
    let mut renderer = pollster::block_on(WgpuRenderer::new_offscreen(64, 48))
        .expect("no wgpu adapter on this machine");
    renderer.attach_sink(sink.clone());
    renderer.render_frame();
}

#[cfg(feature = "wgpu")]
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

#[cfg(feature = "wgpu")]
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

/// The page/QR surface needs no frames — this runs with just `stream`.
#[tokio::test]
async fn toolbox_and_qr_are_served() {
    let sink = FrameSink::new();

    let window = StreamingWindow::new("127.0.0.1:0", sink.clone(), false).unwrap();
    let addr = window
        .url()
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string();
    tokio::time::sleep(Duration::from_millis(5000)).await;

    // the page: the centered symbol IS the inline QR (with the mark on its
    // plate and a hover/tap popup), the toolbox menu keeps the URL, plus
    // the mobile surface (viewport meta, no-touch-zoom canvas, bilinear fit)
    let page = http_get(&addr, "/").await;
    assert!(page.contains("200 OK"), "{}", &page[..page.len().min(80)]);
    assert!(page.contains("toolbox"), "toolbox menu present");
    assert!(page.contains("id=\"glyph\""), "centered symbol present");
    assert!(page.contains("qr-glyph"), "the symbol is the inline QR");
    assert!(page.contains("glyph-info"), "hover/tap popup present");
    assert!(page.contains("another device"), "popup explains hand-off");
    assert!(
        page.contains("toolbox (top right) has the controls"),
        "popup points at the toolbox"
    );
    assert!(!page.contains("__QR_SVG__"), "inline QR injected");
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

    // /qr is a real SVG QR: svg + module path + the logo plate and mark,
    // image/svg+xml content type
    let qr = http_get(&addr, "/qr").await;
    assert!(qr.contains("200 OK"), "{}", &qr[..qr.len().min(80)]);
    assert!(qr.contains("image/svg+xml"), "svg content type");
    assert!(qr.contains("<svg"), "svg root");
    assert!(qr.contains("<path"), "module path");
    assert!(qr.contains("qr-plate"), "white logo plate");
    assert!(qr.contains("qr-glyph"), "the mercury mark rides the QR");
    assert!(qr.contains("<circle"), "the mark's ring");

    // no token armed: the URL stays clean
    assert!(!window.url().contains("?t="), "token is opt-in");
}

#[cfg(feature = "wgpu")]
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
    // with the token → the real page, inline QR and all
    let granted = http_get(&addr, &format!("/?t={token}")).await;
    assert!(
        granted.contains("200 OK"),
        "{}",
        &granted[..granted.len().min(80)]
    );
    assert!(granted.contains("<canvas"));
    assert!(
        granted.contains("qr-glyph"),
        "inline QR carries the token URL"
    );

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
    // (decoded back to text by qr_with_logo_still_decodes below)
    let qr = http_get(&addr, "/qr").await;
    assert!(qr.contains("200 OK") && qr.contains("<svg"), "qr serves");
    assert!(qr.contains("qr-glyph"), "tokenized qr keeps the mark");
}

/// The QR scannability contract: the logo'd QR — plate + ☿ mark, exactly as
/// `stream_window::qr_svg` lays it out — must still decode to its URL.
/// rqrr locates and decodes the painted pixels; the with-logo pass pins
/// `QR_LOGO_PLATE_FRACTION`/`QR_LOGO_GLYPH_FRACTION` as decode-verified.
/// Pure CPU: runs with just the `stream` feature (no frames, no wgpu).
mod qr_contract {
    use hydrargyros::game::stream_window::{QR_LOGO_GLYPH_FRACTION, QR_LOGO_PLATE_FRACTION};

    /// Rasterize the QR (with or without the logo) the way the painter lays
    /// it out: 14 px per module keeps both test grids well over 400 px.
    fn paint_qr(text: &str, with_logo: bool) -> image::RgbaImage {
        // same encode as the painter: ECC High — the logo spends codewords
        let qr = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::High).unwrap();
        let n = qr.size();
        let scale = 14u32;
        let side = (n as u32) * scale;
        let mut img = image::RgbaImage::from_pixel(side, side, image::Rgba([255, 255, 255, 255]));
        // the painter's plate geometry (the ratio under contract)
        let grid = n as f64;
        let plate = grid * QR_LOGO_PLATE_FRACTION;
        let half = plate / 2.0;
        let center = grid / 2.0;
        for y in 0..n {
            for x in 0..n {
                if !qr.get_module(x, y) {
                    continue;
                }
                // module centers inside the plate are cleared for the logo
                let (mx, my) = (x as f64 + 0.5, y as f64 + 0.5);
                if with_logo && (mx - center).abs() < half && (my - center).abs() < half {
                    continue;
                }
                for py in 0..scale {
                    for px in 0..scale {
                        img.put_pixel(
                            x as u32 * scale + px,
                            y as u32 * scale + py,
                            image::Rgba([20, 22, 26, 255]), // #14161a, the painter's ink
                        );
                    }
                }
            }
        }
        if !with_logo {
            return img;
        }
        // the ☿ mark, SDF-painted from the tray.rs anatomy (read-only
        // there): ring (16,19.5) r5.5 half-thickness 1.2; the cup = the
        // lower half of the arc (16,9) r3.5 half 1.1, clipped to y ≥ 9,
        // horns up; stem and crossbar capsules. Ink #262e3d — the painter's
        // mercury-steel (the tray's bright silver would vanish on white).
        let s = plate * QR_LOGO_GLYPH_FRACTION / 22.2; // mark bbox height, tray units
        let (tx, ty) = (center - 16.0 * s, center - 20.1 * s); // bbox center (16, 20.1)
        let ink = [38u8, 46, 61];
        let ring = |x: f64, y: f64, cx: f64, cy: f64, r: f64, half: f64| -> f64 {
            (((x - cx) * (x - cx) + (y - cy) * (y - cy)).sqrt() - r).abs() - half
        };
        let capsule = |px: f64, py: f64, ax: f64, ay: f64, bx: f64, by: f64, half: f64| -> f64 {
            let (abx, aby) = (bx - ax, by - ay);
            let (apx, apy) = (px - ax, py - ay);
            let t = ((apx * abx + apy * aby) / (abx * abx + aby * aby)).clamp(0.0, 1.0);
            let (cx, cy) = (ax + abx * t, ay + aby * t);
            ((px - cx) * (px - cx) + (py - cy) * (py - cy)).sqrt() - half
        };
        for py in 0..side {
            for px in 0..side {
                // pixel → grid coords → tray-native mark coords
                let gx = (px as f64 + 0.5) / scale as f64;
                let gy = (py as f64 + 0.5) / scale as f64;
                let (nx, ny) = ((gx - tx) / s, (gy - ty) / s);
                let cup = ring(nx, ny, 16.0, 9.0, 3.5, 1.1).max(9.0 - ny);
                let d = ring(nx, ny, 16.0, 19.5, 5.5, 1.2)
                    .min(cup)
                    .min(capsule(nx, ny, 16.0, 24.5, 16.0, 30.0, 1.2))
                    .min(capsule(nx, ny, 12.5, 28.0, 19.5, 28.0, 1.2));
                // the tray's anti-alias band, in native mark units
                let a = ((0.5 - d / 0.75).clamp(0.0, 1.0) * 255.0) as u8;
                if a == 0 {
                    continue;
                }
                let base = img.get_pixel(px, py).0;
                let mix = |fg: u8, bg: u8| {
                    ((fg as u16 * a as u16 + bg as u16 * (255 - a as u16)) / 255) as u8
                };
                img.put_pixel(
                    px,
                    py,
                    image::Rgba([
                        mix(ink[0], base[0]),
                        mix(ink[1], base[1]),
                        mix(ink[2], base[2]),
                        255,
                    ]),
                );
            }
        }
        img
    }

    /// rqrr over the painted pixels: locate the grid, decode, return text.
    fn decode(img: &image::RgbaImage) -> String {
        let gray = image::DynamicImage::ImageRgba8(img.clone()).to_luma8();
        let mut prepared = rqrr::PreparedImage::prepare(gray);
        let grids = prepared.detect_grids();
        assert!(!grids.is_empty(), "no QR located in the rendered image");
        grids
            .iter()
            .find_map(|grid| grid.decode().ok())
            .expect("the located QR decodes")
            .1
    }

    #[test]
    fn qr_with_logo_still_decodes() {
        // a plain LAN URL and a token-length one (the 32-hex token forces a
        // bigger grid: the plate ratio must hold at both sizes)
        let urls = [
            "http://192.168.1.50:8400/".to_string(),
            format!(
                "http://192.168.1.50:8400/?t={}",
                "d3adbeefc0ffee42".repeat(2)
            ),
        ];
        // the visual deliverable: keep the painted logo'd QR for a human
        // (or agent) to read — the tray glyph test dumps its PNG the same way
        paint_qr(&urls[0], true)
            .save("/tmp/hydrargyros_qr_logo.png")
            .unwrap();
        for url in &urls {
            assert_eq!(decode(&paint_qr(url, false)), *url, "baseline, no logo");
            assert_eq!(decode(&paint_qr(url, true)), *url, "with the ☿ logo");
        }
    }
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
