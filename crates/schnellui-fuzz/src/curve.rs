//! Window sizes over time: one jump, or a gradual drag of the window edge
//! whose speed follows a random acceleration curve.

use proptest::prelude::*;

/// The smallest window a person can drag to.
pub const MIN: (f32, f32) = (120.0, 90.0);
/// The largest window the properties try.
pub const MAX: (f32, f32) = (1600.0, 1200.0);

/// A drag of the window corner from `from` to `to` over `steps` frames. Each
/// axis moves with its own acceleration curve: a few segments of constant
/// acceleration, some of them braking or pulling back, so the edge can speed
/// up, stall, overshoot and settle, as a hand does. One pixel and a half of
/// jitter per frame stands in for an unsteady hand. The last frame lands on
/// `to` exactly.
#[derive(Clone, Debug)]
pub struct Gradual {
    pub from: (f32, f32),
    pub to: (f32, f32),
    pub steps: usize,
    pub seed: u64,
}

impl Gradual {
    /// The window size at every frame of the drag.
    pub fn sizes(&self) -> Vec<(f32, f32)> {
        let mut rng = SplitMix(self.seed);
        let xs = progress(&mut rng, self.steps);
        let ys = progress(&mut rng, self.steps);
        let mut out = Vec::with_capacity(self.steps);
        for (i, (px, py)) in xs.into_iter().zip(ys).enumerate() {
            if i + 1 == self.steps {
                out.push(self.to);
                break;
            }
            let jitter = |rng: &mut SplitMix| (rng.unit() - 0.5) * 3.0;
            let w = self.from.0 + (self.to.0 - self.from.0) * px + jitter(&mut rng);
            let h = self.from.1 + (self.to.1 - self.from.1) * py + jitter(&mut rng);
            out.push((w.round().clamp(MIN.0, MAX.0), h.round().clamp(MIN.1, MAX.1)));
        }
        out
    }
}

/// Progress along one axis at each of `steps` frames, ending at 1.
fn progress(rng: &mut SplitMix, steps: usize) -> Vec<f32> {
    let segments = 1 + (rng.next() % 4) as usize;
    let mut cuts: Vec<usize> = (0..segments - 1)
        .map(|_| (rng.next() % steps.max(1) as u64) as usize)
        .collect();
    cuts.sort_unstable();
    // Mostly forward, sometimes braking hard enough to pull back.
    let accels: Vec<f32> = (0..segments).map(|_| rng.unit() * 6.0 - 2.0).collect();
    let mut velocity = 0.0f32;
    let mut position = 0.0f32;
    let mut out = Vec::with_capacity(steps);
    for step in 0..steps {
        let segment = cuts.iter().filter(|cut| **cut <= step).count();
        velocity += accels[segment];
        position += velocity;
        out.push(position);
    }
    let end = out.last().copied().unwrap_or(0.0);
    if end.abs() < 1e-3 {
        // The curve went nowhere: fall back to an even drag.
        return (1..=steps).map(|i| i as f32 / steps as f32).collect();
    }
    out.iter().map(|p| p / end).collect()
}

/// SplitMix64, so a seed means the same drag on every platform.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

/// A window size anywhere from [`MIN`] to [`MAX`], biased towards the small
/// end where clipping and wrapping change most.
pub fn size() -> impl Strategy<Value = (f32, f32)> {
    let axis = |min: f32, max: f32| {
        prop_oneof![
            3 => min..min * 3.0,
            2 => min..max,
        ]
        .prop_map(f32::round)
    };
    (axis(MIN.0, MAX.0), axis(MIN.1, MAX.1))
}

/// A list of one to eight window sizes, each set in one jump.
pub fn jumps() -> impl Strategy<Value = Vec<(f32, f32)>> {
    proptest::collection::vec(size(), 1..8)
}

/// A gradual drag from one random size to another over 2 to 90 frames.
pub fn gradual() -> impl Strategy<Value = Gradual> {
    (size(), size(), 2usize..90, any::<u64>()).prop_map(|(from, to, steps, seed)| Gradual {
        from,
        to,
        steps,
        seed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drag_ends_on_its_target_and_stays_in_bounds() {
        for seed in 0..500 {
            let drag = Gradual {
                from: (300.0, 200.0),
                to: (900.0, 140.0),
                steps: 1 + (seed as usize % 60),
                seed,
            };
            let sizes = drag.sizes();
            assert_eq!(sizes.len(), drag.steps);
            assert_eq!(*sizes.last().unwrap(), drag.to);
            for (w, h) in sizes {
                assert!((MIN.0..=MAX.0).contains(&w) && (MIN.1..=MAX.1).contains(&h));
            }
        }
    }

    #[test]
    fn some_drags_overshoot_and_come_back() {
        let overshoots = (0..200)
            .filter(|seed| {
                let drag = Gradual {
                    from: (300.0, 300.0),
                    to: (600.0, 300.0),
                    steps: 40,
                    seed: *seed,
                };
                drag.sizes().iter().any(|(w, _)| *w > 610.0)
            })
            .count();
        assert!(overshoots > 0, "the acceleration curves never overshoot");
    }
}
