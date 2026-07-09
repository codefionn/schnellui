//! Animation simulations on a virtual clock: loading spinners, floating
//! labels, theme transitions and debounced scroll callbacks, alone and all at
//! once. The checks are about time, not frames: an animation must look the
//! same at any frame rate, stop when it is done, freeze under reduced motion,
//! and a whole run must replay identically from its seed. Seeds also pick the
//! host each run sees: viewport, scale factor, frame rate and late frames.
//!
//! The signal runtime is process-global; run with `--test-threads=1`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use schnellui::a11y::Role;
use schnellui::scene::{Point, Primitive, WidgetId};
use schnellui::signal::{create_scope, create_signal, dispose, run_in_scope};
use schnellui::testing::{for_each_seed, SimHost, SimRng, Simulator};
use schnellui::theme::{DARK, LIGHT};
use schnellui::widgets::{Button, Column, LoadingSpinner, Scroll, Text, TextInput, Theme};
use schnellui::UiKey;

const RATES: [u32; 5] = SimHost::FRAME_RATES;

fn with_scope<R>(run: impl FnOnce() -> R) -> R {
    let scope = create_scope();
    let result = run_in_scope(scope, run);
    dispose(scope);
    result
}

/// The alpha of every spinner stroke: the visible animation frame.
fn spinner_alphas(sim: &Simulator, id: WidgetId) -> Vec<u8> {
    sim.app()
        .scene()
        .paint(id)
        .unwrap()
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Line { color, .. } => Some(color.a),
            _ => None,
        })
        .collect()
}

fn assert_frames_close(a: &[u8], b: &[u8], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: stroke count");
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        assert!(
            x.abs_diff(*y) <= 1,
            "{what}: stroke {i} alpha {x} vs {y}\n{a:?}\n{b:?}"
        );
    }
}

fn spinner_sim(host: &SimHost) -> (Simulator, WidgetId) {
    let sim = host.mount(LoadingSpinner::new());
    let id = sim.find(Role::ProgressIndicator, None).unwrap();
    (sim, id)
}

#[test]
fn spinner_frame_depends_on_elapsed_time_not_frame_rate() {
    for_each_seed("animations/spinner-rate", 20, |rng| {
        let at = rng.millis(50, 4000);
        // One host per refresh rate, each with its own size, scale and jitter.
        let frames: Vec<(SimHost, Vec<u8>)> = RATES
            .iter()
            .map(|&hz| {
                let host = SimHost {
                    frame_rate: hz,
                    ..SimHost::random(rng, 100, 100)
                };
                let (mut sim, id) = spinner_sim(&host);
                sim.advance(at);
                // Present one frame exactly at `at`, whatever the rate.
                sim.frame();
                (host, spinner_alphas(&sim, id))
            })
            .collect();
        for (host, alphas) in &frames[1..] {
            assert_frames_close(
                alphas,
                &frames[0].1,
                &format!("{host:?} vs {:?} at {at:?}", frames[0].0),
            );
        }
    });
}

#[test]
fn spinner_wants_frames_at_the_host_rate_and_never_goes_idle() {
    for_each_seed("animations/spinner-pacing", 20, |rng| {
        let host = SimHost::random(rng, 100, 100);
        let (mut sim, _) = spinner_sim(&host);
        let span = rng.millis(200, 3000);
        let before = sim.frames();
        sim.advance(span);
        let ran = sim.frames() - before;
        // Late frames push the next slot back, so jitter only lowers the count.
        let fastest = (span.as_secs_f64() * f64::from(host.frame_rate)) as u64;
        let slowest = (span.as_secs_f64() / host.slowest_frame().as_secs_f64()) as u64;
        assert!(
            (slowest.saturating_sub(1)..=fastest + 1).contains(&ran),
            "{ran} frames in {span:?} on {host:?}, expected {slowest}..={fastest}"
        );
        assert!(!sim.is_idle());
        assert!(sim.run_until_idle(Duration::from_secs(2)).is_err());
    });
}

#[test]
fn reduced_motion_freezes_a_spinner_and_resumes_from_the_same_frame() {
    for_each_seed("animations/spinner-pause", 20, |rng| {
        let before_pause = rng.millis(20, 2000);
        let paused_for = rng.millis(100, 10_000);
        let after = rng.millis(16, 1000);

        let (mut paused, id) = spinner_sim(&SimHost::random(rng, 100, 100));
        paused.advance(before_pause);
        paused.frame();
        assert!(paused.set_reduced_motion(true));
        let frozen = spinner_alphas(&paused, id);
        let frames = paused.frames();
        paused.advance(paused_for);
        assert_eq!(paused.frames(), frames, "a frozen spinner wants no frames");
        assert!(paused.is_idle());
        assert_eq!(spinner_alphas(&paused, id), frozen);
        assert!(paused.set_reduced_motion(false));
        assert_frames_close(
            &spinner_alphas(&paused, id),
            &frozen,
            "the first frame after resuming continues the frozen frame",
        );
        paused.advance(after);
        paused.frame();

        // The same animation without the pause, on another host.
        let (mut reference, reference_id) = spinner_sim(&SimHost::random(rng, 100, 100));
        reference.advance(before_pause);
        reference.frame();
        reference.advance(after);
        reference.frame();
        assert_frames_close(
            &spinner_alphas(&paused, id),
            &spinner_alphas(&reference, reference_id),
            &format!("{after:?} after a {paused_for:?} pause"),
        );
    });
}

#[test]
fn floating_label_settles_in_the_same_time_at_any_frame_rate() {
    const LABELS: [&str; 4] = ["Project", "E-mail", "Name of the project owner", "中文"];
    for_each_seed("animations/floating-label", 12, |rng| {
        let label = *rng.pick(&LABELS);
        let initial = if rng.chance(1, 3) { "" } else { "typed" };
        let idle_first = rng.millis(0, 500);
        let mut settled = Vec::new();
        for hz in RATES {
            let host = SimHost {
                frame_rate: hz,
                ..SimHost::random(rng, 300, 100)
            };
            let took = with_scope(|| {
                let mut sim = host.mount(TextInput::new(initial).label(label));
                sim.advance(idle_first);
                sim.focus(Role::TextInput, Some(label)).unwrap();
                if initial.is_empty() {
                    assert!(sim.app().wants_animation_frame(), "focus floats the label");
                }
                sim.run_until_idle(Duration::from_secs(5)).unwrap()
            });
            settled.push((host, took));
        }
        let (reference_host, reference) = settled[2].clone();
        for (host, took) in &settled {
            let diff = took.abs_diff(reference);
            let slack = host.slowest_frame().max(reference_host.slowest_frame());
            assert!(
                diff <= slack,
                "{label:?} settles in {took:?} on {host:?} but {reference:?} on \
                 {reference_host:?}"
            );
        }
    });
}

#[test]
fn theme_transition_frames_sample_the_clock() {
    for_each_seed("animations/theme-transition", 12, |rng| {
        with_scope(|| {
            let host = SimHost::random(rng, 300, 200);
            let duration = rng.millis(100, 900);
            let (from, to) = if rng.chance(1, 2) {
                (LIGHT, DARK)
            } else {
                (DARK, LIGHT)
            };
            let count = create_signal(0i64);
            let mut sim = host.mount_themed(from, move || {
                Column::new()
                    .child(Text::dynamic(move || format!("count {}", count.get())))
                    .child(Button::new("add").on_click(move || count.update(|c| *c += 1)))
            });
            sim.advance(rng.millis(0, 300));
            sim.click(Role::Button, Some("add")).unwrap();
            sim.focus(Role::Button, Some("add")).unwrap();

            let started = sim.elapsed();
            assert!(sim.app_mut().transition_theme(to, duration));
            sim.frame();
            let interval = sim.frame_interval();
            let mut k = 0u32;
            loop {
                k += 1;
                sim.advance(interval);
                // With jitter a step may run no frame; the theme is whatever
                // the last frame sampled.
                let t = sim.last_frame_at() - started;
                let amount = (t.as_secs_f32() / duration.as_secs_f32()).min(1.0);
                let expected: Theme = from.lerp(to, amount);
                assert_eq!(sim.app().active_theme(), expected, "{host:?}, step {k}");
                // Every frame is a rebuild; state, focus and the click handler survive.
                assert!(sim.names(Role::Label).contains(&"count 1".to_owned()));
                assert_eq!(sim.focused().and_then(|n| n.name).as_deref(), Some("add"));
                if amount >= 1.0 {
                    break;
                }
            }
            assert_eq!(sim.app().active_theme(), to);
            assert!(sim.is_idle(), "a finished transition wants no frames");
            sim.key(UiKey::Enter);
            assert!(sim.names(Role::Label).contains(&"count 2".to_owned()));
        });
    });
}

/// Trailing-edge debounce, as documented on `Scroll::on_scroll_debounced`.
#[derive(Default)]
struct DebounceModel {
    burst_start: Option<Duration>,
    deadline: Option<Duration>,
    latest: f32,
    fired: Vec<f32>,
}

impl DebounceModel {
    fn scrolled(&mut self, at: Duration, offset: f32, delay: Duration, max_wait: Duration) {
        let start = *self.burst_start.get_or_insert(at);
        self.deadline = Some((at + delay).min(start + max_wait));
        self.latest = offset;
    }

    fn advance_to(&mut self, now: Duration) {
        if self.deadline.is_some_and(|d| d <= now) {
            self.fired.push(self.latest);
            self.deadline = None;
            self.burst_start = None;
        }
    }
}

#[test]
fn debounced_scroll_callbacks_follow_the_trailing_edge_schedule() {
    for_each_seed("animations/scroll-debounce", 40, |rng| {
        with_scope(|| {
            let delay = rng.millis(20, 200);
            let max_wait = delay * rng.range_i64(1, 4) as u32;
            let fired = Rc::new(RefCell::new(Vec::new()));
            let sink = fired.clone();
            let mut rows = Column::new();
            for i in 0..5 + rng.index(76) {
                rows = rows.child(Text::new(format!("row {i}")));
            }
            let view = Scroll::new()
                .label("list")
                .size(200.0, 120.0)
                .on_scroll_debounced(delay, max_wait, move |offset| {
                    sink.borrow_mut().push(offset)
                })
                .child(rows);
            let host = SimHost::random(rng, 200, 120);
            let mut sim = host.mount(view);
            let inside = Point {
                x: rng.range_i64(5, 195) as f32,
                y: rng.range_i64(5, 115) as f32,
            };
            let mut model = DebounceModel::default();
            let mut offset = 0.0f32;

            for step in 0..10 + rng.index(71) {
                if rng.chance(2, 3) {
                    let mut delta = rng.range_i64(-120, 120) as f32;
                    if delta == 0.0 {
                        delta = 1.0;
                    }
                    sim.wheel(inside, delta);
                    let now: f32 = sim
                        .value(Role::ScrollView, Some("list"))
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0.0);
                    assert!(
                        now == (offset + delta).max(0.0) || (delta > 0.0 && now < offset + delta),
                        "step {step}: offset {offset} + {delta} -> {now}"
                    );
                    if now != offset {
                        model.scrolled(sim.elapsed(), now, delay, max_wait);
                    }
                    offset = now;
                } else {
                    let gap = rng.millis(1, 2 * delay.as_millis() as u64);
                    sim.advance(gap);
                    model.advance_to(sim.elapsed());
                }
                let rounded: Vec<f32> = fired.borrow().iter().map(|v| v.round()).collect();
                assert_eq!(
                    rounded,
                    model.fired,
                    "step {step} at {:?} on {host:?}",
                    sim.elapsed()
                );
            }
            sim.advance(max_wait);
            model.advance_to(sim.elapsed());
            assert_eq!(fired.borrow().len(), model.fired.len());
            assert!(sim.is_idle());
        });
    });
}

/// Everything at once: a themed app with a spinner, a text field with a
/// floating label, a debounced scroll list and a counter, driven by a random
/// script of input, theme switches, reduced-motion toggles and time steps.
/// Returns a trace of what an observer would see after every step.
fn run_concurrent_script(seed: u64) -> (Vec<String>, i64, String) {
    with_scope(|| {
        let mut rng = SimRng::new(seed);
        let count = create_signal(0i64);
        let typed = create_signal(String::new());
        let settled = Rc::new(RefCell::new(Vec::<f32>::new()));
        let sink = settled.clone();
        let host = SimHost::random(&mut rng, 320, 400);
        let row_count = 5 + rng.index(46);
        let (delay, max_wait) = (rng.millis(10, 120), rng.millis(120, 400));
        let mut sim = host.mount_themed(LIGHT, move || {
            let sink = sink.clone();
            let mut rows = Column::new();
            for i in 0..row_count {
                rows = rows.child(Text::new(format!("row {i}")));
            }
            Column::new()
                .child(LoadingSpinner::new())
                .child(Text::dynamic(move || format!("count {}", count.get())))
                .child(Button::new("add").on_click(move || count.update(|c| *c += 1)))
                .child(
                    TextInput::new(typed.read_untracked(String::clone))
                        .label("Note")
                        .on_input(move |s| typed.set(s.to_owned())),
                )
                .child(
                    Scroll::new()
                        .label("list")
                        .size(200.0, 100.0)
                        .on_scroll_debounced(delay, max_wait, move |o| sink.borrow_mut().push(o))
                        .child(rows),
                )
        });
        let mut expected_count = 0;
        let mut expected_text = String::new();
        let mut trace = Vec::new();
        let mut dark = false;
        for _ in 0..15 + rng.index(36) {
            match rng.below(9) {
                0 => {
                    sim.click(Role::Button, Some("add")).unwrap();
                    expected_count += 1;
                }
                1 => {
                    sim.focus(Role::TextInput, Some("Note")).unwrap();
                    sim.key(UiKey::End { shift: false });
                    let c = *rng.pick(&['x', 'y', 'é']);
                    sim.type_text(&c.to_string());
                    expected_text.push(c);
                }
                2 => {
                    sim.blur();
                }
                3 => {
                    dark = !dark;
                    let to = if dark { DARK } else { LIGHT };
                    let duration = rng.millis(0, 400);
                    sim.app_mut().transition_theme(to, duration);
                    sim.frame();
                }
                4 => {
                    let reduce = rng.chance(1, 2);
                    sim.set_reduced_motion(reduce);
                }
                5 => {
                    let list = sim.find(Role::ScrollView, Some("list")).unwrap();
                    let rect = sim.app().scene().layout(list).unwrap().rect;
                    let point = Point {
                        x: rect.x + 10.0,
                        y: rect.y + 10.0,
                    };
                    sim.wheel(point, rng.range_i64(-120, 120) as f32);
                }
                6 => {
                    let width = 320 + rng.below(321) as u32;
                    let height = 400 + rng.below(401) as u32;
                    sim.resize(width as f32, height as f32);
                }
                _ => sim.advance(rng.millis(1, 300)),
            }
            let theme = sim.app().active_theme();
            trace.push(format!(
                "t={:?} frames={} theme={:?} settled={:?}\n{}",
                sim.elapsed(),
                sim.frames(),
                theme.page,
                settled.borrow(),
                sim.app().a11y_json()
            ));
        }
        // Let every transition finish; with motion reduced the app goes idle.
        sim.set_reduced_motion(true);
        sim.run_until_idle(Duration::from_secs(2))
            .expect("reduced motion settles every animation");
        assert_eq!(sim.app().active_theme(), if dark { DARK } else { LIGHT });
        assert!(sim
            .names(Role::Label)
            .contains(&format!("count {expected_count}")));
        assert_eq!(
            sim.value(Role::TextInput, Some("Note")).unwrap_or_default(),
            expected_text
        );
        assert_eq!(typed.get(), expected_text);
        (trace, count.get(), typed.get())
    })
}

#[test]
fn concurrent_animations_and_input_replay_identically_from_a_seed() {
    for_each_seed("animations/concurrent", 12, |rng| {
        let seed = rng.next_u64();
        let first = run_concurrent_script(seed);
        let second = run_concurrent_script(seed);
        for (i, (a, b)) in first.0.iter().zip(&second.0).enumerate() {
            assert_eq!(a, b, "step {i} diverged between two runs of seed {seed:#x}");
        }
        assert_eq!(first.1, second.1);
        assert_eq!(first.2, second.2);
    });
}

#[test]
fn idle_apps_run_no_frames_while_time_passes() {
    with_scope(|| {
        let mut sim = Simulator::mount(
            Column::new()
                .child(Text::new("static"))
                .child(TextInput::new("").label("Field")),
            200,
            100,
        );
        assert!(sim.is_idle());
        let frames = sim.frames();
        sim.advance(Duration::from_secs(3600));
        assert_eq!(sim.frames(), frames);
    });
}
