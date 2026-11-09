//! The tray glyph (the `tray` feature): the ☿ mark in the system tray with
//! a Quit menu item. Headless machines degrade gracefully — a note on
//! stderr, exit 0, no panic.
//!
//! Run:
//!   cargo run --example game_tray --features tray
//!
//! (This blocks until Quit is chosen — the tray's GTK main loop on Linux.)

fn main() {
    println!("putting the ☿ glyph in the system tray — its menu has Quit");
    let result = hyg::game::spawn_tray_quit("hyg — tray demo", |_event| {
        println!("Quit chosen — bye");
    });
    if let Err(e) = result {
        println!("tray unavailable here: {e}");
    }
}
