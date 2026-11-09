//! Timers and tweening — Hydrargyros-native (gold has no tween/timer
//! surface: the lineage check over gold's src/include turns up only the UI
//! style system's CSS timing functions — style.cpp `easeValue`, cubic-bézier
//! curves interpolating style properties, not game values). This is the
//! app-development fill, the way [`super::camera::OrbitController`] is:
//! an [`Easing`] curve, a [`Tween`] interpolating a scalar over a duration,
//! and a [`Timer`] that ticks.
//!
//! v1 is a pure-data utility, **not** a component — no engine wiring;
//! examples and components compose it directly (step your tween/timer in
//! `update(dt)`). Everything is plain `f64` data, so it's `Send`/`Sync`
//! for free.
//!
//! ```
//! use hyg::game::tween::{Easing, Tween};
//!
//! let mut tween = Tween::from_values(0.0, 100.0, 2.0, Easing::Linear);
//! for _ in 0..2 {
//!     tween.step(0.5); // half a second per frame
//! }
//! assert_eq!(tween.value(), 50.0); // half-way through a 2s tween
//! tween.step(1.0);
//! assert_eq!(tween.value(), 100.0); // clamped at the end — no overshoot past duration
//! assert!(tween.finished());
//! ```

use std::f64::consts::PI;

/// The floor [`Tween::from_values`] and [`Timer::new`] clamp durations to.
/// A zero duration would divide by zero in the progress math and tick on
/// every step — the OrbitController-style clamp, documented once here.
pub const MIN_DURATION: f64 = 1e-6; // seconds

/// The easing curves. All map normalized progress `t` (0..1) to eased
/// progress (0..1, `ElasticOut` overshooting mid-flight). The In curves
/// start slow and end fast, the Out curves mirror them, InOut combines
/// both (slow–fast–slow, symmetric around t = 0.5 → 0.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Easing {
    /// Identity — the value moves at constant speed.
    #[default]
    Linear,
    /// t² — starts slow, ends fast.
    QuadIn,
    /// 1 − (1−t)² — starts fast, ends slow.
    QuadOut,
    /// QuadIn for the first half, QuadOut for the second.
    QuadInOut,
    /// t³ — a steeper [`Easing::QuadIn`].
    CubicIn,
    /// 1 − (1−t)³ — a steeper [`Easing::QuadOut`].
    CubicOut,
    /// CubicIn for the first half, CubicOut for the second.
    CubicInOut,
    /// (1 − cos(πt)) / 2 — a sine half-period: gentle on both ends.
    SineInOut,
    /// The classic elastic out: the value overshoots `to` (~1.35 at peak)
    /// and oscillates, decaying to exactly 1 at t = 1.
    ElasticOut,
}

impl Easing {
    /// Map normalized progress `t` to eased progress. Values outside 0..1
    /// clamp into the range (a stepped tween never produces them, but the
    /// curve is usable standalone).
    #[must_use]
    pub fn ease(self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::QuadIn => t * t,
            Easing::QuadOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::QuadInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - 2.0 * (1.0 - t) * (1.0 - t)
                }
            }
            Easing::CubicIn => t * t * t,
            Easing::CubicOut => 1.0 - (1.0 - t).powi(3),
            Easing::CubicInOut => {
                if t < 0.5 {
                    4.0 * t * t * t
                } else {
                    1.0 - 4.0 * (1.0 - t).powi(3)
                }
            }
            Easing::SineInOut => (1.0 - (PI * t).cos()) * 0.5,
            Easing::ElasticOut => {
                // the canonical easeOutElastic curve, with the endpoints
                // pinned: the raw formula is 1.00049 at t = 1, and the
                // contract is settle-to-exactly-1 (a tween's last frame
                // must land on `to`)
                if t <= 0.0 {
                    return 0.0;
                }
                if t >= 1.0 {
                    return 1.0;
                }
                const C4: f64 = (2.0 * PI) / 3.0;
                2.0f64.powf(-10.0 * t) * ((t * 10.0 - 0.75) * C4).sin() + 1.0
            }
        }
    }
}

/// One scalar interpolated from `from` to `to` over `duration` seconds,
/// through an [`Easing`]. Step it once per frame — [`Tween::step`] returns
/// the current eased value, so a component can be one line:
/// `entity.transform_mut().position.x = tween.step(dt);`
///
/// ```
/// use hyg::game::tween::{Easing, Tween};
///
/// let mut tween = Tween::from_values(10.0, 20.0, 1.0, Easing::QuadIn);
/// assert_eq!(tween.step(0.5), 12.5); // halfway through, quad-in eased
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tween {
    /// The value at t = 0.
    pub from: f64,
    /// The value at t = 1.
    pub to: f64,
    /// The tween's length in seconds — clamped to [`MIN_DURATION`] at
    /// construction (a zero duration would divide by zero).
    pub duration: f64,
    /// Seconds stepped so far (clamped at `duration`).
    pub elapsed: f64,
    /// The curve the progress maps through.
    pub ease: Easing,
}

impl Tween {
    /// A tween from `from` to `to` over `duration` seconds through `ease`.
    #[must_use]
    pub fn from_values(from: f64, to: f64, duration: f64, ease: Easing) -> Tween {
        Tween {
            from,
            to,
            duration: duration.max(MIN_DURATION),
            elapsed: 0.0,
            ease,
        }
    }

    /// Advance by `dt` seconds and return the CURRENT eased value.
    /// Negative `dt` is ignored (time never runs backwards); `elapsed`
    /// clamps at `duration`, so the value never moves past `to` — an
    /// [`Easing::ElasticOut`] overshoot still shows mid-flight, but the
    /// tween's last frame lands exactly on `to`.
    pub fn step(&mut self, dt: f64) -> f64 {
        self.elapsed = (self.elapsed + dt.max(0.0)).min(self.duration);
        self.value()
    }

    /// The current eased value (what the last [`Tween::step`] returned).
    #[must_use]
    pub fn value(&self) -> f64 {
        let t = self.elapsed / self.duration;
        self.from + (self.to - self.from) * self.ease.ease(t)
    }

    /// Whether `elapsed` reached `duration` (the tween is done; stepping
    /// further keeps returning `to`).
    #[must_use]
    pub fn finished(&self) -> bool {
        self.elapsed >= self.duration
    }

    /// Restart from t = 0 (keeps from/to/duration/ease).
    pub fn reset(&mut self) {
        self.elapsed = 0.0;
    }
}

/// A countdown: [`Timer::step`] returns `true` on a tick. Repeating timers
/// re-arm after each tick and keep ticking; one-shots fire once and then
/// fall silent.
///
/// ```
/// use hyg::game::tween::Timer;
///
/// let mut timer = Timer::new(1.0, false); // one-shot, one second
/// for _ in 0..3 {
///     assert!(!timer.step(0.25));
/// }
/// assert!(timer.step(0.25)); // the fourth quarter crosses the second
/// assert!(!timer.step(1.0)); // one-shots don't tick twice
/// assert_eq!(timer.ticks(), 1);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timer {
    /// Seconds between ticks — clamped to [`MIN_DURATION`] at construction.
    pub duration: f64,
    /// Seconds stepped so far (one-shots clamp at `duration`).
    pub elapsed: f64,
    /// Repeating timers re-arm after each tick, carrying the leftover time
    /// — ticks stay on the absolute multiples of `duration`, so frame
    /// jitter can't drift the schedule. One-shots stop ticking after the
    /// first.
    pub repeating: bool,
    ticks: u64,
}

impl Timer {
    /// A timer ticking every `duration` seconds; `repeating` re-arms after
    /// each tick, one-shots fire once.
    #[must_use]
    pub fn new(duration: f64, repeating: bool) -> Timer {
        Timer {
            duration: duration.max(MIN_DURATION),
            elapsed: 0.0,
            repeating,
            ticks: 0,
        }
    }

    /// Advance by `dt` seconds; `true` when the timer TICKED this step
    /// (possibly several times — one huge `dt` counts every whole period it
    /// crosses, the floor semantics a run of small steps would produce).
    /// Negative `dt` is ignored (time never runs backwards).
    pub fn step(&mut self, dt: f64) -> bool {
        self.elapsed += dt.max(0.0);
        if self.elapsed < self.duration {
            return false;
        }
        if self.repeating {
            let crossed = (self.elapsed / self.duration) as u64;
            self.elapsed -= crossed as f64 * self.duration;
            self.ticks += crossed;
            true
        } else {
            let fire = self.ticks == 0; // one-shots fire once, then stop ticking
            if fire {
                self.ticks += 1;
            }
            self.elapsed = self.duration;
            fire
        }
    }

    /// How many times this timer has ticked.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
}

// Compile-time proof the utilities cross threads freely (plain f64 data —
// the value-graph assertion in lib.rs covers the Var side).
const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Easing>();
    assert_send_sync::<Tween>();
    assert_send_sync::<Timer>();
};
