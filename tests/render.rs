//! wgpu render tests (behind the `wgpu` feature): the offscreen renderer
//! must produce actual, verifiable pixels — a solid test triangle over the
//! clear color. Skips quietly when the machine has no GPU stack at all.

#![cfg(feature = "wgpu")]

use hyg::game::RenderBackend;
use hyg::game::WgpuRenderer;

/// sRGB-encode a linear value the way the hardware does for an
/// Rgba8UnormSrgb target.
fn srgb8(linear: f64) -> u8 {
    let v = if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * width + x) * 4) as usize;
    [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
}

#[test]
fn offscreen_frame_has_expected_pixels() {
    let Ok(mut renderer) = pollster::block_on(WgpuRenderer::new_offscreen(64, 64)) else {
        eprintln!("skipping: no wgpu adapter on this machine");
        return;
    };
    renderer.set_clear_color([0.1, 0.2, 0.3, 1.0]);
    renderer.set_scene(&[
        hyg::game::Quad::rect([-0.3, 0.0], [0.2, 0.2], [0.0, 0.6, 0.0, 1.0]),
        hyg::game::Quad::rect([0.3, 0.0], [0.2, 0.2], [1.0, 0.4, 0.1, 1.0]),
    ]);
    renderer.render_frame();
    let frame = renderer.screenshot();
    assert_eq!(frame.len(), 64 * 64 * 4);

    // a corner: pure clear color
    let corner = pixel(frame, 64, 2, 2);
    let expected = [srgb8(0.1), srgb8(0.2), srgb8(0.3), 255];
    for (got, want) in corner.iter().zip(expected) {
        assert!((*got as i32 - want as i32).abs() <= 3, "corner {corner:?} vs {expected:?}");
    }

    // both quads drawn, left green and right orange (multi-quad scene)
    let left = pixel(frame, 64, 22, 32);
    let right = pixel(frame, 64, 42, 32);
    let want_left = [srgb8(0.0), srgb8(0.6), srgb8(0.0), 255];
    let want_right = [srgb8(1.0), srgb8(0.4), srgb8(0.1), 255];
    for (got, want) in left.iter().zip(want_left) {
        assert!((*got as i32 - want as i32).abs() <= 3, "left {left:?} vs {want_left:?}");
    }
    for (got, want) in right.iter().zip(want_right) {
        assert!((*got as i32 - want as i32).abs() <= 3, "right {right:?} vs {want_right:?}");
    }
}

#[test]
fn renderer_implements_the_backend_trait() {
    fn assert_backend<T: hyg::game::RenderBackend>() {}
    assert_backend::<WgpuRenderer>();
}
