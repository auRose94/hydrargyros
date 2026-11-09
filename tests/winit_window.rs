//! winit window backend tests. The event → `Var` mapping helpers are pure
//! functions, testable without a display; the actual event loop is skipped
//! when the machine has no windowing system. (winit's `WindowEvent` can't
//! be constructed outside winit — private platform fields — so the helpers
//! are the seam.)

#![cfg(feature = "winit")]

use hyg::game::winit_window::{key_var, mouse_button_var, mouse_move_var, quit_var, wheel_var};

#[test]
fn key_events_map_to_vars() {
    let down = key_var("w", true);
    assert_eq!(down.get("type").as_str(), Some("key"));
    assert_eq!(down.get("key").as_str(), Some("w"));
    assert_eq!(down.get("pressed").as_bool(), Some(true));

    let up = key_var("w", false);
    assert_eq!(up.get("pressed").as_bool(), Some(false));
}

#[test]
fn named_keys_map_to_lowercase_names() {
    // winit's Debug for NamedKey::Space is "Space" — the mapping lowercases
    let named = "Space".to_lowercase();
    let v = key_var(named, true);
    assert_eq!(v.get("key").as_str(), Some("space"));
}

#[test]
fn mouse_events_carry_button_and_position() {
    let down = mouse_button_var(0, true, 42, 17);
    assert_eq!(down.get("type").as_str(), Some("mouse"));
    assert_eq!(down.get("action").as_str(), Some("down"));
    assert_eq!(down.get("button").as_i64(), Some(0));
    assert_eq!(down.get("x").as_i64(), Some(42));
    assert_eq!(down.get("y").as_i64(), Some(17));

    let move_ = mouse_move_var(120, 64);
    assert_eq!(move_.get("action").as_str(), Some("move"));
    assert_eq!(move_.get("x").as_i64(), Some(120));
}

#[test]
fn wheel_and_quit_events() {
    let wheel = wheel_var(-2.5, 5, 6);
    assert_eq!(wheel.get("type").as_str(), Some("wheel"));
    assert_eq!(wheel.get("delta").as_f64(), Some(-2.5));

    let quit = quit_var();
    assert_eq!(quit.get("type").as_str(), Some("quit"));
}

// The event loop itself is main-thread-only (winit panics elsewhere), so
// its behavior is verified by examples/game_window.rs, not here.
