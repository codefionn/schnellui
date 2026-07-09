//! # schnellui-motion
//!
//! Declarative animations with two interchangeable execution backends.
//!
//! A [Motion] is a *declaration*: what animates (a rotation, a fade), for
//! how long, with which easing, and how often it repeats. It contains no
//! clock, no scene, and no renderer coupling, so one declaration can drive
//! every backend the umbrella ships:
//!
//! * GPU/native path - the retained-scene renderer calls
//!   [Motion::value] (or [Motion::progress]) with a monotonically
//!   increasing elapsed time each frame and re-emits the affected paint
//!   fragment. The result is frame-rate independent: the same declaration
//!   produces the same visual at 30, 60, or 144 Hz.
//! * CSS path - the HTML renderer calls [css_animation] once and receives
//!   the @keyframes block plus the animation shorthand for the exact same
//!   declaration, so the browser compositor (not JS) advances it.
//!
//! Both backends consume the same declaration, so a widget author writes
//! the animation exactly once and every renderer agrees on duration,
//! easing, and repeat count. Reduced-motion policy stays a *backend*
//! concern: the native host freezes the clock (and the CSS media query
//! sets `animation: none`) while the declaration itself never changes.

mod css;
pub use css::{css_animation, css_keyframes, CssAnimation};

/// Timing function applied to the normalized [0,1] loop progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    /// Maps normalized progress through the curve. Output is clamped to
    /// [0,1] so an eased sample can never overshoot a declared range.
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - 2.0 * (1.0 - t) * (1.0 - t)
                }
            }
        }
    }

    /// The CSS animation-timing-function keyword for this curve.
    pub fn css_keyword(self) -> &'static str {
        match self {
            Easing::Linear => "linear",
            Easing::EaseIn => "ease-in",
            Easing::EaseOut => "ease-out",
            Easing::EaseInOut => "ease-in-out",
        }
    }
}

/// How a declaration repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    /// Runs n times and holds the final keyframe value.
    Finite(u32),
    /// Repeats forever; progress wraps in [0,1).
    Infinite,
}

/// What a declaration animates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Property {
    /// A continuous rotation of `turns` revolutions per loop.
    Rotate { turns: f32 },
    /// A linear interpolation between two opacity factors in [0,1].
    Fade { from: f32, to: f32 },
}

/// One declarative animation, backend-neutral.
///
/// Build one with the named constructors ([Motion::rotate],
/// [Motion::fade]) or the full [Motion::new]; consume it with
/// [Motion::progress]/[Motion::value] on the GPU path or [css_animation]
/// on the CSS path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub property: Property,
    /// One loop duration in milliseconds.
    pub duration_ms: f32,
    pub easing: Easing,
    pub repeat: Repeat,
    /// Optional delay before the first loop starts, in milliseconds.
    pub delay_ms: f32,
}

impl Motion {
    pub fn new(property: Property, duration_ms: f32, easing: Easing, repeat: Repeat) -> Self {
        Motion {
            property,
            duration_ms: duration_ms.max(1.0),
            easing,
            repeat,
            delay_ms: 0.0,
        }
    }

    /// An infinite linear rotation taking one full turn per loop - the
    /// classic indeterminate-progress spinner declaration.
    pub fn rotate(period_ms: f32) -> Self {
        Self::new(
            Property::Rotate { turns: 1.0 },
            period_ms,
            Easing::Linear,
            Repeat::Infinite,
        )
    }

    /// A fade between two opacity factors over duration_ms.
    pub fn fade(duration_ms: f32, from: f32, to: f32) -> Self {
        Self::new(
            Property::Fade { from, to },
            duration_ms,
            Easing::EaseInOut,
            Repeat::Finite(1),
        )
    }

    /// Delays the animation start by delay_ms.
    pub fn with_delay(mut self, delay_ms: f32) -> Self {
        self.delay_ms = delay_ms.max(0.0);
        self
    }

    /// The looped, eased progress in [0,1] at elapsed_ms since the
    /// animation started. Elapsed time inside the delay window yields 0;
    /// a finished finite animation holds 1.
    ///
    /// This is the GPU path per-frame sampling entry point: feed it a
    /// monotonic elapsed time and multiply into whatever the backend
    /// draws.
    pub fn progress(&self, elapsed_ms: f32) -> f32 {
        if elapsed_ms <= self.delay_ms {
            return 0.0;
        }
        let active = elapsed_ms - self.delay_ms;
        let loops = active / self.duration_ms;
        let t = match self.repeat {
            Repeat::Infinite => loops.fract(),
            Repeat::Finite(n) => {
                let n = n.max(1);
                (loops as u32).min(n) as f32 / n as f32
            }
        };
        self.easing.apply(if t < 0.0 { t + 1.0 } else { t })
    }

    /// The eased animation value at elapsed_ms: rotation in radians for
    /// [Property::Rotate], the interpolated opacity factor for
    /// [Property::Fade]. Frame-rate independent by construction.
    pub fn value(&self, elapsed_ms: f32) -> f32 {
        let t = self.progress(elapsed_ms);
        match self.property {
            Property::Rotate { turns } => t * turns * std::f32::consts::TAU,
            Property::Fade { from, to } => from + (to - from) * t,
        }
    }

    /// Whether the declaration has finished (never true for [Repeat::Infinite]).
    pub fn is_finished(&self, elapsed_ms: f32) -> bool {
        match self.repeat {
            Repeat::Infinite => false,
            Repeat::Finite(n) => elapsed_ms >= self.delay_ms + self.duration_ms * n as f32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_infinite_rotation_is_frame_rate_independent() {
        let spin = Motion::rotate(900.0);
        assert!((spin.value(450.0) - std::f32::consts::PI).abs() < 1e-4);
        assert!((spin.value(1350.0) - std::f32::consts::PI).abs() < 1e-4);
        assert!((spin.value(900.0) % std::f32::consts::TAU).abs() < 1e-4);
    }

    #[test]
    fn eased_progress_stays_in_unit_range() {
        for easing in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
        ] {
            let m = Motion::new(
                Property::Fade { from: 0.0, to: 1.0 },
                100.0,
                easing,
                Repeat::Infinite,
            );
            for i in 0..=20 {
                let t = m.progress(i as f32 * 7.0);
                assert!((0.0..=1.0).contains(&t), "{easing:?} {t}");
            }
        }
    }

    #[test]
    fn finite_repeat_holds_final_value() {
        let fade = Motion::fade(200.0, 0.0, 1.0);
        assert!(!fade.is_finished(199.0));
        assert!(fade.is_finished(200.0));
        assert!((fade.value(1000.0) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn delay_holds_first_keyframe() {
        let spin = Motion::rotate(500.0).with_delay(250.0);
        assert_eq!(spin.progress(100.0), 0.0);
        assert!(spin.progress(300.0) > 0.0);
    }

    #[test]
    fn css_backend_matches_declaration() {
        let spin = Motion::rotate(900.0);
        let css = css_animation(&spin, "sui-spin");
        assert!(
            css.keyframes.contains("rotate(360deg)"),
            "{}",
            css.keyframes
        );
        assert!(css.shorthand.contains("900ms"), "{}", css.shorthand);
        assert!(css.shorthand.contains("infinite"), "{}", css.shorthand);
        assert!(css.shorthand.contains("linear"), "{}", css.shorthand);
    }

    #[test]
    fn css_fade_emits_both_keyframes() {
        let fade = Motion::new(
            Property::Fade { from: 0.2, to: 0.9 },
            250.0,
            Easing::EaseOut,
            Repeat::Finite(3),
        );
        let css = css_animation(&fade, "sui-fade");
        assert!(css.keyframes.contains("opacity: 0.2"), "{}", css.keyframes);
        assert!(css.keyframes.contains("opacity: 0.9"), "{}", css.keyframes);
        assert!(css.shorthand.contains(" 3"), "{}", css.shorthand);
    }
}
