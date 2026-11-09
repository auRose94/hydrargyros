//! Tween/timer tests (feature-free — the core CI): the easing curves' defining
//! values, deterministic Tween stepping, and Timer tick semantics.

use hyg::game::tween::{Easing, Timer, Tween, MIN_DURATION};

/// Every curve pins its endpoints, and the middle points carry the curve's
/// defining shape (exact values — the math is closed-form).
#[test]
fn easing_curves_hit_their_defining_values() {
    let cases: &[(Easing, f64, f64, f64, f64)] = &[
        // (curve, ease(0.25), ease(0.5), ease(0.75), ease(1.0))
        (Easing::Linear, 0.25, 0.5, 0.75, 1.0),
        (Easing::QuadIn, 0.0625, 0.25, 0.5625, 1.0),
        (Easing::QuadOut, 0.4375, 0.75, 0.9375, 1.0),
        (Easing::QuadInOut, 0.125, 0.5, 0.875, 1.0),
        (Easing::CubicIn, 0.015625, 0.125, 0.421875, 1.0),
        (Easing::CubicOut, 0.578125, 0.875, 0.984375, 1.0),
        (Easing::CubicInOut, 0.0625, 0.5, 0.9375, 1.0),
        // (1 − cos(πt)) / 2
        (
            Easing::SineInOut,
            (1.0 - std::f64::consts::FRAC_1_SQRT_2) * 0.5,
            0.5,
            (1.0 + std::f64::consts::FRAC_1_SQRT_2) * 0.5,
            1.0,
        ),
        // 2^(−10t)·sin((10t − 0.75)·2π/3) + 1, endpoints pinned:
        // t = 0.25 → sin(7π/6) = −0.5 → 1 − 0.5·2^(−2.5)
        // t = 0.5 → sin(5π/6) = 0.5 → 1 + 0.5·2^(−5)
        // t = 0.75 → sin(4.5π) = 1 → 1 + 2^(−7.5)
        (
            Easing::ElasticOut,
            1.0 - 0.5 * 2.0f64.powf(-2.5),
            1.015625,
            1.0 + 2.0f64.powf(-7.5),
            1.0,
        ),
    ];
    for (ease, at_25, at_50, at_75, _at_100) in cases {
        assert_eq!(ease.ease(0.0), 0.0, "{ease:?} at t=0");
        assert_eq!(ease.ease(1.0), 1.0, "{ease:?} at t=1");
        assert!(
            (ease.ease(0.25) - at_25).abs() < 1e-12,
            "{ease:?} at t=0.25: {} != {at_25}",
            ease.ease(0.25)
        );
        assert!(
            (ease.ease(0.5) - at_50).abs() < 1e-12,
            "{ease:?} at t=0.5: {} != {at_50}",
            ease.ease(0.5)
        );
        assert!(
            (ease.ease(0.75) - at_75).abs() < 1e-12,
            "{ease:?} at t=0.75: {} != {at_75}",
            ease.ease(0.75)
        );
    }
}

/// In starts slow and ends fast; Out is the exact mirror; InOut is
/// symmetric around 0.5.
#[test]
fn easing_in_out_mirror_laws() {
    // In: accelerating — each quarter of progress covers more than the
    // last, the first slower than linear, the last faster
    for ease in [Easing::QuadIn, Easing::CubicIn] {
        let quarters: Vec<f64> = (1..=4)
            .map(|k| ease.ease(k as f64 * 0.25) - ease.ease((k - 1) as f64 * 0.25))
            .collect();
        assert!(quarters[0] < 0.25, "{ease:?} starts slow");
        assert!(quarters[3] > 0.25, "{ease:?} ends fast");
        assert!(
            quarters.windows(2).all(|w| w[1] > w[0]),
            "{ease:?} accelerates throughout: {quarters:?}"
        );
    }
    // Out: the mirror — ease_out(t) = 1 − ease_in(1 − t)
    for (out, inn) in [
        (Easing::QuadOut, Easing::QuadIn),
        (Easing::CubicOut, Easing::CubicIn),
    ] {
        for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let mirrored = 1.0 - inn.ease(1.0 - t);
            assert!(
                (out.ease(t) - mirrored).abs() < 1e-12,
                "{out:?}({t}) should mirror {inn:?}"
            );
        }
    }
    // InOut: symmetric — f(t) = 1 − f(1 − t), with f(0.5) = 0.5 (epsilon
    // for the sine curve: cos(π/2) rounds, the power curves are exact)
    for ease in [Easing::QuadInOut, Easing::CubicInOut, Easing::SineInOut] {
        assert!(
            (ease.ease(0.5) - 0.5).abs() < 1e-12,
            "{ease:?} crosses the middle straight"
        );
        for t in [0.1, 0.25, 0.4, 0.6, 0.75, 0.9] {
            let mirrored = 1.0 - ease.ease(1.0 - t);
            assert!(
                (ease.ease(t) - mirrored).abs() < 1e-12,
                "{ease:?}({t}) should be symmetric around 0.5"
            );
        }
    }
}

/// ElasticOut overshoots `to` mid-flight (peak ≈ 1.354 at t = 0.15) and
/// settles onto exactly 1 by the end.
#[test]
fn elastic_out_overshoots_then_settles() {
    let mut peak = f64::MIN;
    let mut i = 0;
    while i <= 1000 {
        let t = i as f64 / 1000.0;
        peak = peak.max(Easing::ElasticOut.ease(t));
        i += 1;
    }
    assert!(peak > 1.3, "elastic out should overshoot `to`: peak {peak}");
    // settled: within one amplitude quantum of 1 late in the curve
    assert!((Easing::ElasticOut.ease(0.9) - 1.0).abs() < 0.002);
    // and the endpoints are exact, overshoot or not
    assert_eq!(Easing::ElasticOut.ease(0.0), 0.0);
    assert_eq!(Easing::ElasticOut.ease(1.0), 1.0);
}

/// Stepping a linear tween in dt slices lands on exact values at key times;
/// the end is clamped (no overshoot past duration).
#[test]
fn tween_steps_deterministically_and_clamps() {
    let mut tween = Tween::from_values(0.0, 100.0, 2.0, Easing::Linear);
    assert_eq!(tween.value(), 0.0);
    assert!(!tween.finished());

    assert_eq!(tween.step(0.5), 25.0); // t = 0.25
    assert_eq!(tween.step(0.5), 50.0); // t = 0.5
    assert_eq!(tween.step(0.5), 75.0);
    assert_eq!(tween.value(), 75.0);

    assert_eq!(tween.step(0.5), 100.0); // t = 1.0
    assert!(tween.finished());
    // clamped: stepping past the end keeps the value at `to`
    assert_eq!(tween.step(0.5), 100.0);
    assert_eq!(tween.step(10.0), 100.0);

    tween.reset();
    assert_eq!(tween.value(), 0.0);
    assert!(!tween.finished());
}

/// Every easing's tween value equals the curve evaluated at elapsed/duration
/// — exact at 0.25 / 0.5 / 1.0 of the duration.
#[test]
fn tween_value_is_the_eased_progress() {
    for ease in [
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
        let mut tween = Tween::from_values(0.0, 1.0, 4.0, ease);
        assert_eq!(tween.step(1.0), ease.ease(0.25), "{ease:?} at 1/4");
        assert_eq!(tween.step(1.0), ease.ease(0.5), "{ease:?} at 1/2");
        tween.step(1.0);
        assert_eq!(tween.step(1.0), ease.ease(1.0), "{ease:?} at the end");
    }
}

/// from→to interpolation lands on `from` at the start and `to` at the end
/// for any curve, and negative dt is ignored (time never runs backwards).
#[test]
fn tween_endpoints_and_negative_dt() {
    let mut tween = Tween::from_values(-5.0, 5.0, 1.0, Easing::CubicInOut);
    assert_eq!(tween.value(), -5.0);
    tween.step(-1.0); // ignored
    assert_eq!(tween.elapsed, 0.0);
    tween.step(1.0);
    assert_eq!(tween.value(), 5.0);

    // an elastic tween overshoots MID-flight but lands exactly on `to`
    let mut elastic = Tween::from_values(0.0, 1.0, 1.0, Easing::ElasticOut);
    assert!(elastic.step(0.15) > 1.0, "mid-flight overshoot");
    elastic.step(0.85);
    assert_eq!(elastic.value(), 1.0);
}

/// Zero and negative durations clamp to MIN_DURATION (the OrbitController
/// clamp pattern — documented once, shared by Tween and Timer).
#[test]
fn durations_clamp_to_the_minimum() {
    assert_eq!(
        Tween::from_values(0.0, 1.0, 0.0, Easing::Linear).duration,
        MIN_DURATION
    );
    assert_eq!(
        Tween::from_values(0.0, 1.0, -5.0, Easing::Linear).duration,
        MIN_DURATION
    );
    assert_eq!(Timer::new(0.0, false).duration, MIN_DURATION);
    assert_eq!(Timer::new(-1.0, true).duration, MIN_DURATION);
}

/// A one-shot fires exactly once — the step crossing the duration — and
/// then stays silent.
#[test]
fn one_shot_timer_fires_once() {
    let mut timer = Timer::new(1.0, false);
    for _ in 0..3 {
        assert!(!timer.step(0.25), "not yet at a quarter-steps");
    }
    assert!(timer.step(0.25), "the fourth quarter crosses the second");
    assert_eq!(timer.ticks(), 1);
    assert_eq!(timer.elapsed, 1.0); // clamped at duration
                                    // one-shots stop ticking
    assert!(!timer.step(0.25));
    assert!(!timer.step(10.0));
    assert_eq!(timer.ticks(), 1);
}

/// A repeating timer fires floor(total/duration) times — in uniform dt
/// slices AND when one huge step crosses several periods at once.
#[test]
fn repeating_timer_fires_floor_total_over_duration() {
    // 0.25s slices over 10s with a 1s period → exactly 10 ticks
    let mut timer = Timer::new(1.0, true);
    let mut ticks = 0;
    for _ in 0..40 {
        if timer.step(0.25) {
            ticks += 1;
        }
    }
    assert_eq!(ticks, 10);
    assert_eq!(timer.ticks(), 10);
    assert_eq!(timer.elapsed, 0.0); // re-armed, nothing left over

    // one big step crossing three periods fires three times
    let mut chunky = Timer::new(1.0, true);
    assert!(chunky.step(3.5));
    assert_eq!(chunky.ticks(), 3);
    assert_eq!(chunky.elapsed, 0.5); // the remainder carries
}

/// The tick schedule is a function of ACCUMULATED time only: uneven frame
/// jitter (jittery slices vs uniform slices vs one big step) yields the same
/// tick count, and the timer's leftover elapsed equals the true remainder
/// of the accumulated time — the carry keeps the schedule from drifting.
#[test]
fn repeating_timer_does_not_drift() {
    // jittery: frames alternate 0.1s / 0.95s — 20 frames, 10.5s total
    let mut jittery = Timer::new(1.0, true);
    for i in 0..20 {
        let dt = if i % 2 == 0 { 0.1 } else { 0.95 };
        jittery.step(dt);
    }
    assert_eq!(jittery.ticks(), 10, "floor(10.5 / 1.0) ticks");
    let remainder = 10.5 - 10.0;
    assert!(
        (jittery.elapsed - remainder).abs() < 1e-9,
        "leftover should be the true remainder: {}",
        jittery.elapsed
    );

    // uniform slices over the same 10.5s: same count, same remainder
    let mut uniform = Timer::new(1.0, true);
    for _ in 0..42 {
        uniform.step(0.25);
    }
    assert_eq!(uniform.ticks(), jittery.ticks());
    assert!(
        (uniform.elapsed - jittery.elapsed).abs() < 1e-9,
        "the schedule cannot tell the slicing apart"
    );
}

/// A negative dt never ticks a timer (time never runs backwards).
#[test]
fn timer_ignores_negative_dt() {
    let mut timer = Timer::new(1.0, true);
    assert!(!timer.step(-0.5));
    assert_eq!(timer.elapsed, 0.0);
    assert_eq!(timer.ticks(), 0);
}
