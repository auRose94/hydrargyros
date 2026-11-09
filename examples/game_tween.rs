//! Tweens and timers (feature-free, pure data): the `Easing` curves, a
//! `Tween` stepped per frame that lands exactly on its target, and a
//! repeating `Timer` whose schedule can't drift under frame jitter.
//! Components compose these in `update(dt)`.
//!
//! Run:
//!   cargo run --example game_tween

use hyg::game::tween::{Easing, Timer, Tween};

fn main() {
    // ---- the curves, sampled at a quarter/half/three-quarters ----
    println!("Easing::ease(t) at t = 0.25 / 0.50 / 0.75:");
    for curve in [
        Easing::Linear,
        Easing::QuadIn,
        Easing::QuadOut,
        Easing::QuadInOut,
        Easing::CubicIn,
        Easing::CubicOut,
        Easing::CubicInOut,
        Easing::SineInOut,
        Easing::ElasticOut,
    ] {
        println!(
            "  {:>10}: {:.3}  {:.3}  {:.3}",
            format!("{curve:?}"),
            curve.ease(0.25),
            curve.ease(0.5),
            curve.ease(0.75),
        );
    }

    // ---- a tween steps to an eased value and lands EXACTLY on `to` ----
    let mut bob = Tween::from_values(0.0, 1.0, 1.0, Easing::ElasticOut);
    let mut steps = 0u32;
    let mut last = 0.0f64;
    while !bob.finished() {
        last = bob.step(0.1);
        steps += 1;
    }
    println!("elastic 0→1 over 1s: {steps} steps of dt=0.1, landed on {last:.6}");

    // ---- timers: repeating re-arm carries the leftover, so frame jitter
    // can't drift the schedule — ticks are a function of accumulated time ----
    let mut tick = Timer::new(0.5, true);
    let mut ticks = 0u32;
    for frame in 0..120 {
        // 2 simulated seconds; every 7th frame is a long 1/30s frame
        let dt = if frame % 7 == 0 { 1.0 / 30.0 } else { 1.0 / 60.0 };
        if tick.step(dt) {
            ticks += 1;
        }
    }
    println!("2s of jittery 60fps frames → {ticks} ticks (schedule says 4)");

    // one-shots fire once and stay finished
    let mut once = Timer::new(0.25, false);
    let mut fired = 0u32;
    for _ in 0..120 {
        if once.step(1.0 / 60.0) {
            fired += 1;
        }
    }
    println!("one-shot 0.25s timer fired {fired} time(s) in 2s");
}
