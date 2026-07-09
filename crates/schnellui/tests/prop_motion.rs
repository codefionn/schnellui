//! Property test for overlapping theme transitions. Programs start new
//! transitions before old ones finish, switch reduced motion on and off,
//! jump straight to a theme and let random amounts of time pass, at a random
//! frame rate. After every step the active theme must equal what a model of
//! the documented rules gives for the time of the last frame:
//!
//! * a transition interpolates from the theme shown when it started;
//! * a zero duration, reduced motion or an unchanged target applies at once;
//! * reduced motion finishes a running transition immediately;
//! * the app wants frames exactly while a transition runs.
//!
//! The signal runtime is process-global; run with `--test-threads=1`.

use std::time::Duration;

use proptest::prelude::*;
use schnellui::signal::{create_scope, create_signal, dispose, run_in_scope};
use schnellui::testing::Simulator;
use schnellui::theme::{DARK, LIGHT};
use schnellui::widgets::{Button, Column, Text, Theme};
use schnellui::App;

#[allow(dead_code)]
mod common;

fn palette(i: u8) -> Theme {
    match i % 3 {
        0 => LIGHT,
        1 => DARK,
        _ => LIGHT.lerp(DARK, 0.5),
    }
}

#[derive(Clone, Debug)]
enum Op {
    Transition { to: u8, ms: u16 },
    SetTheme(u8),
    ReducedMotion(bool),
    Advance(u16),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0u8..3, prop_oneof![Just(0u16), 1u16..600])
            .prop_map(|(to, ms)| Op::Transition { to, ms }),
        1 => (0u8..3).prop_map(Op::SetTheme),
        1 => any::<bool>().prop_map(Op::ReducedMotion),
        4 => (1u16..400).prop_map(Op::Advance),
    ]
}

#[derive(Clone, Copy, Debug)]
struct Running {
    from: Theme,
    to: Theme,
    start: Duration,
    duration: Duration,
}

#[derive(Debug)]
struct Model {
    shown: Theme,
    running: Option<Running>,
    reduced: bool,
}

impl Model {
    /// The app samples a running transition once per frame.
    fn frame(&mut self, at: Duration) {
        let Some(r) = self.running else { return };
        let elapsed = at.saturating_sub(r.start);
        let amount = (elapsed.as_secs_f32() / r.duration.as_secs_f32()).min(1.0);
        self.shown = r.from.lerp(r.to, amount);
        if amount >= 1.0 {
            self.running = None;
        }
    }
}

fn run_program(hz: u32, ops: &[Op]) {
    let scope = create_scope();
    run_in_scope(scope, || {
        let count = create_signal(0i64);
        let app = App::mount_themed_with_size_scaled(
            LIGHT,
            move || {
                Column::new()
                    .child(Text::dynamic(move || format!("count {}", count.get())))
                    .child(Button::new("add").on_click(move || count.update(|c| *c += 1)))
            },
            240,
            120,
            1.0,
        );
        let mut sim = Simulator::new(app);
        sim.set_frame_rate(hz);
        let mut model = Model {
            shown: LIGHT,
            running: None,
            reduced: false,
        };

        for (step, op) in ops.iter().enumerate() {
            match *op {
                Op::Transition { to, ms } => {
                    let to = palette(to);
                    let duration = Duration::from_millis(ms.into());
                    sim.app_mut().transition_theme(to, duration);
                    if duration.is_zero() || model.reduced || model.shown == to {
                        model.shown = to;
                        model.running = None;
                    } else {
                        model.running = Some(Running {
                            from: model.shown,
                            to,
                            start: sim.elapsed(),
                            duration,
                        });
                    }
                    // The host redraws after the call.
                    sim.frame();
                    model.frame(sim.last_frame_at());
                }
                Op::SetTheme(to) => {
                    sim.app_mut().set_theme(palette(to));
                    model.shown = palette(to);
                    model.running = None;
                    sim.frame();
                }
                Op::ReducedMotion(reduce) => {
                    sim.set_reduced_motion(reduce);
                    if reduce {
                        if let Some(r) = model.running.take() {
                            model.shown = r.to;
                        }
                    }
                    model.reduced = reduce;
                    model.frame(sim.last_frame_at());
                }
                Op::Advance(ms) => {
                    sim.advance(Duration::from_millis(ms.into()));
                    model.frame(sim.last_frame_at());
                }
            }
            let ctx = format!(
                "{hz} Hz, step {step} ({op:?}) at {:?}: {model:?}",
                sim.elapsed()
            );
            assert_eq!(sim.app().active_theme(), model.shown, "{ctx}");
            assert_eq!(sim.is_idle(), model.running.is_none(), "{ctx}: idle");
        }

        // State survives every rebuild the transitions caused.
        sim.click(schnellui::a11y::Role::Button, Some("add"))
            .unwrap();
        assert!(sim
            .names(schnellui::a11y::Role::Label)
            .contains(&"count 1".to_owned()));
    });
    dispose(scope);
}

proptest! {
    #![proptest_config(common::cases(256))]

    #[test]
    fn overlapping_theme_transitions_follow_the_model(
        hz in prop::sample::select(&[24u32, 30, 60, 144][..]),
        ops in prop::collection::vec(op(), 1..30),
    ) {
        run_program(hz, &ops);
    }
}
