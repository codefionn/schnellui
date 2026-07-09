//! Randomized user-input simulations: pointer clicks, keyboard activation,
//! assistive actions and typing, each checked against a plain model of what
//! the app should show. Handlers write signals, memos derive from them, and
//! dynamic text shows the result, so every check also covers the path from a
//! user event through the signal graph into the accessibility tree.
//!
//! Each seed also picks the host (viewport, scale factor, frame rate, frame
//! jitter), the order and starting state of the controls, how long a script
//! runs, and idle time and window resizes between inputs.
//!
//! The signal runtime is process-global; run with `--test-threads=1`.

use schnellui::a11y::Role;
use schnellui::accesskit_action::{Action, ActionData};
use schnellui::scene::Point;
use schnellui::signal::{create_memo, create_scope, create_signal, dispose, run_in_scope};
use schnellui::testing::{for_each_seed, SimHost, SimRng, Simulator};
use schnellui::widgets::{Button, Checkbox, Column, Slider, Text, TextInput};
use schnellui::UiKey;

/// The text of the first label whose name starts with `prefix`.
fn label(sim: &Simulator, prefix: &str) -> String {
    sim.names(Role::Label)
        .into_iter()
        .find_map(|name| name.strip_prefix(prefix).map(str::to_owned))
        .unwrap_or_else(|| panic!("no label starting with {prefix:?}"))
}

/// The centre of a widget's laid-out rect, for pointer input.
fn center(sim: &Simulator, role: Role, name: &str) -> Point {
    let id = sim.find(role, Some(name)).unwrap();
    let rect = sim.app().scene().layout(id).unwrap().rect;
    Point {
        x: rect.x + rect.width / 2.0,
        y: rect.y + rect.height / 2.0,
    }
}

/// Mounts and simulates a view built inside a fresh signal scope, then
/// disposes the scope so seeds do not leak nodes into each other.
fn with_scope(run: impl FnOnce()) {
    let scope = create_scope();
    run_in_scope(scope, run);
    dispose(scope);
}

/// Between inputs, sometimes lets time pass or resizes the window to a size
/// no smaller than `min`, the way a real session interleaves them.
fn interlude(sim: &mut Simulator, rng: &mut SimRng, min: (u32, u32)) {
    match rng.below(8) {
        0 | 1 => sim.advance(rng.millis(1, 800)),
        2 => {
            let width = min.0 + rng.below(u64::from(min.0) + 1) as u32;
            let height = min.1 + rng.below(u64::from(min.1) + 1) as u32;
            sim.resize(width as f32, height as f32);
        }
        _ => {}
    }
}

#[derive(Default, Debug)]
struct CounterModel {
    count: i64,
    double: bool,
    level: i64,
    min: i64,
    max: i64,
}

impl CounterModel {
    fn total(&self) -> i64 {
        let base = if self.double {
            self.count * 2
        } else {
            self.count
        };
        base + self.level
    }
}

/// Activates a control the way a random user would: an assistive click, a
/// pointer click at its centre, or keyboard focus followed by Enter/Space.
fn activate(sim: &mut Simulator, rng: &mut SimRng, role: Role, name: &str) {
    match rng.below(4) {
        0 => {
            assert!(sim.click(role, Some(name)).unwrap(), "click {name}");
        }
        1 => {
            let point = center(sim, role, name);
            assert!(sim.click_at(point), "pointer click {name}");
        }
        2 => {
            sim.focus(role, Some(name)).unwrap();
            assert!(
                sim.key(UiKey::Enter) || role == Role::CheckBox,
                "Enter on {name}"
            );
            if role == Role::CheckBox && !sim.has_state(role, Some(name), "focused") {
                panic!("checkbox lost focus");
            }
            // Enter does not toggle a checkbox in browsers; Space does.
            if role == Role::CheckBox {
                assert!(sim.key(UiKey::Space { shift: false }), "Space on {name}");
            }
        }
        _ => {
            sim.focus(role, Some(name)).unwrap();
            assert!(sim.key(UiKey::Space { shift: false }), "Space on {name}");
        }
    }
}

#[test]
fn mixed_input_drives_signals_memos_and_dynamic_text() {
    const MIN: (u32, u32) = (240, 380);
    for_each_seed("events/counter-panel", 60, |rng| {
        with_scope(|| {
            let min = rng.range_i64(-5, 0);
            let max = min + rng.range_i64(2, 15);
            let start_level = rng.range_i64(min, max);
            let start_count = rng.range_i64(-3, 3);
            let start_double = rng.chance(1, 2);
            let count = create_signal(start_count);
            let double = create_signal(start_double);
            let level = create_signal(start_level);
            let total = create_memo(move || {
                let base = if double.get() {
                    count.get() * 2
                } else {
                    count.get()
                };
                base + level.get()
            });
            let mut order: Vec<usize> = (0..7).collect();
            rng.shuffle(&mut order);
            let mut view = Column::new();
            for k in order {
                view = match k {
                    0 => view.child(Text::dynamic(move || format!("count {}", count.get()))),
                    1 => view.child(Text::dynamic(move || format!("total {}", total.get()))),
                    2 => view.child(
                        Button::new("increment").on_click(move || count.update(|c| *c += 1)),
                    ),
                    3 => view.child(
                        Button::new("decrement").on_click(move || count.update(|c| *c -= 1)),
                    ),
                    4 => view.child(Button::new("reset").on_click(move || count.set(0))),
                    5 => view.child(
                        Checkbox::new(start_double)
                            .name("double")
                            .on_toggle(move |on| double.set(on)),
                    ),
                    _ => view.child(
                        Slider::new(start_level as f32, min as f32, max as f32)
                            .step(1.0)
                            .name("level")
                            .on_change(move |v| level.set(v.round() as i64)),
                    ),
                };
            }
            let host = SimHost::random(rng, MIN.0, MIN.1);
            let mut sim = host.mount(view);
            let mut model = CounterModel {
                count: start_count,
                double: start_double,
                level: start_level,
                min,
                max,
            };

            for step in 0..20 + rng.index(41) {
                interlude(&mut sim, rng, MIN);
                match rng.below(8) {
                    0 | 1 => {
                        activate(&mut sim, rng, Role::Button, "increment");
                        model.count += 1;
                    }
                    2 => {
                        activate(&mut sim, rng, Role::Button, "decrement");
                        model.count -= 1;
                    }
                    3 => {
                        activate(&mut sim, rng, Role::Button, "reset");
                        model.count = 0;
                    }
                    4 => {
                        activate(&mut sim, rng, Role::CheckBox, "double");
                        model.double = !model.double;
                    }
                    5 => {
                        let up = rng.chance(1, 2);
                        let action = if up {
                            Action::Increment
                        } else {
                            Action::Decrement
                        };
                        sim.action(action, Role::Slider, Some("level"), None)
                            .unwrap();
                        model.level =
                            (model.level + if up { 1 } else { -1 }).clamp(model.min, model.max);
                    }
                    6 => {
                        let v = rng.range_i64(model.min - 3, model.max + 3);
                        sim.action(
                            Action::SetValue,
                            Role::Slider,
                            Some("level"),
                            Some(ActionData::Value(v.to_string().into())),
                        )
                        .unwrap();
                        model.level = v.clamp(model.min, model.max);
                    }
                    _ => {
                        // Keyboard arrows on a focused slider.
                        sim.focus(Role::Slider, Some("level")).unwrap();
                        let up = rng.chance(1, 2);
                        sim.key(if up {
                            UiKey::Right {
                                shift: false,
                                ctrl: false,
                            }
                        } else {
                            UiKey::Left {
                                shift: false,
                                ctrl: false,
                            }
                        });
                        model.level =
                            (model.level + if up { 1 } else { -1 }).clamp(model.min, model.max);
                    }
                }
                let ctx = format!("step {step}: {model:?} on {host:?}");
                assert_eq!(label(&sim, "count "), model.count.to_string(), "{ctx}");
                assert_eq!(label(&sim, "total "), model.total().to_string(), "{ctx}");
                assert_eq!(
                    sim.has_state(Role::CheckBox, Some("double"), "checked"),
                    model.double,
                    "{ctx}"
                );
                assert_eq!(
                    sim.value(Role::Slider, Some("level")),
                    Some(model.level.to_string()),
                    "{ctx}"
                );
            }
        });
    });
}

/// A single-line editor: characters, a caret and an optional selection anchor.
#[derive(Debug, Default)]
struct EditModel {
    text: Vec<char>,
    caret: usize,
    anchor: usize,
}

impl EditModel {
    fn selection(&self) -> Option<(usize, usize)> {
        (self.anchor != self.caret)
            .then(|| (self.anchor.min(self.caret), self.anchor.max(self.caret)))
    }

    fn delete_selection(&mut self) -> bool {
        let Some((start, end)) = self.selection() else {
            return false;
        };
        self.text.drain(start..end);
        self.caret = start;
        self.anchor = start;
        true
    }

    fn insert(&mut self, c: char) {
        self.delete_selection();
        self.text.insert(self.caret, c);
        self.caret += 1;
        self.anchor = self.caret;
    }

    fn backspace(&mut self) {
        if !self.delete_selection() && self.caret > 0 {
            self.caret -= 1;
            self.text.remove(self.caret);
            self.anchor = self.caret;
        }
    }

    fn delete(&mut self) {
        if !self.delete_selection() && self.caret < self.text.len() {
            self.text.remove(self.caret);
        }
    }

    /// Moves the caret to `to`. Without `extend`, an active selection first
    /// collapses to the edge in the direction of travel, as browsers do.
    fn move_to(&mut self, to: usize, extend: bool, collapse_to: Option<usize>) {
        match (extend, self.selection(), collapse_to) {
            (false, Some(_), Some(edge)) => self.caret = edge,
            _ => self.caret = to,
        }
        if !extend {
            self.anchor = self.caret;
        }
    }

    fn string(&self) -> String {
        self.text.iter().collect()
    }
}

#[test]
fn random_editing_keys_match_a_caret_and_selection_model() {
    const ALPHABET: &[char] = &['a', 'b', 'z', 'W', 'é', 'ß', '7', ' ', '-', '中', '😀'];
    const MIN: (u32, u32) = (160, 140);
    for_each_seed("events/text-editing", 60, |rng| {
        with_scope(|| {
            let initial: String = (0..rng.index(12)).map(|_| *rng.pick(ALPHABET)).collect();
            let text = create_signal(initial.clone());
            let length = create_memo(move || text.with(|t| t.chars().count()));
            let view = Column::new()
                .child(
                    TextInput::new(initial.clone())
                        .label("Name")
                        .on_input(move |s| text.set(s.to_owned())),
                )
                .child(Text::dynamic(move || format!("length {}", length.get())))
                .child(Button::new("other"));
            let host = SimHost::random(rng, MIN.0, MIN.1);
            let mut sim = host.mount(view);
            sim.focus(Role::TextInput, Some("Name")).unwrap();
            // Where focus puts the caret is not the point here; pin it.
            let mut model = EditModel {
                text: initial.chars().collect(),
                ..EditModel::default()
            };
            if rng.chance(1, 2) {
                sim.key(UiKey::Home { shift: false });
            } else {
                sim.key(UiKey::End { shift: false });
                model.caret = model.text.len();
                model.anchor = model.caret;
            }

            for step in 0..20 + rng.index(101) {
                interlude(&mut sim, rng, MIN);
                let shift = rng.chance(1, 3);
                let op = rng.below(12);
                match op {
                    0..=4 => {
                        let c = *rng.pick(ALPHABET);
                        sim.key(UiKey::Char(c.encode_utf8(&mut [0; 4])));
                        model.insert(c);
                    }
                    5 => {
                        sim.key(UiKey::Backspace);
                        model.backspace();
                    }
                    6 => {
                        sim.key(UiKey::Delete);
                        model.delete();
                    }
                    7 => {
                        sim.key(UiKey::Left { shift, ctrl: false });
                        let start = model.selection().map(|(s, _)| s);
                        model.move_to(model.caret.saturating_sub(1), shift, start);
                    }
                    8 => {
                        sim.key(UiKey::Right { shift, ctrl: false });
                        let end = model.selection().map(|(_, e)| e);
                        let to = (model.caret + 1).min(model.text.len());
                        model.move_to(to, shift, end);
                    }
                    9 => {
                        sim.key(UiKey::Home { shift });
                        model.move_to(0, shift, None);
                    }
                    10 => {
                        sim.key(UiKey::End { shift });
                        model.move_to(model.text.len(), shift, None);
                    }
                    _ => {
                        sim.key(UiKey::SelectAll);
                        model.anchor = 0;
                        model.caret = model.text.len();
                    }
                }
                let expected = model.string();
                let ctx = format!("step {step} (op {op}, shift {shift}): {model:?} on {host:?}");
                assert_eq!(
                    sim.value(Role::TextInput, Some("Name")).unwrap_or_default(),
                    expected,
                    "{ctx}"
                );
                assert_eq!(
                    sim.app().selected_text().unwrap_or_default(),
                    model
                        .selection()
                        .map(|(s, e)| model.text[s..e].iter().collect::<String>())
                        .unwrap_or_default(),
                    "{ctx}: selection"
                );
                assert_eq!(
                    label(&sim, "length "),
                    expected.chars().count().to_string(),
                    "{ctx}"
                );
            }
        });
    });
}

#[test]
fn tab_navigation_cycles_through_controls_in_order() {
    for_each_seed("events/tab-order", 40, |rng| {
        with_scope(|| {
            // A random mix of focusable controls with plain text between them.
            let mut order: Vec<(Role, String)> = Vec::new();
            let mut column = Column::new();
            for i in 0..1 + rng.index(7) {
                while rng.chance(1, 3) {
                    column = column.child(Text::new("not focusable"));
                }
                let name = format!("control {i}");
                let role = *rng.pick(&[Role::Button, Role::CheckBox, Role::TextInput]);
                column = match role {
                    Role::Button => column.child(Button::new(name.clone())),
                    Role::CheckBox => {
                        column.child(Checkbox::new(rng.chance(1, 2)).name(name.clone()))
                    }
                    _ => column.child(TextInput::new("").label(name.clone())),
                };
                order.push((role, name));
            }
            let host = SimHost::random(rng, 240, 120 + 60 * order.len() as u32);
            let mut sim = host.mount(column);
            // Nothing is focused yet; the first Tab lands on the first control
            // and the first Shift+Tab on the last.
            let mut at: Option<usize> = None;
            if rng.chance(1, 3) {
                // Start from a control focused some other way.
                let i = rng.index(order.len());
                let (role, name) = &order[i];
                sim.focus(*role, Some(name)).unwrap();
                at = Some(i);
            }
            for step in 0..10 + rng.index(31) {
                if rng.chance(1, 5) {
                    sim.advance(rng.millis(1, 500));
                }
                let back = rng.chance(1, 3);
                sim.key(UiKey::Tab { shift: back });
                let n = order.len();
                at = Some(match (at, back) {
                    (None, false) => 0,
                    (None, true) => n - 1,
                    (Some(i), false) => (i + 1) % n,
                    (Some(i), true) => (i + n - 1) % n,
                });
                let focused = sim.focused().expect("Tab always focuses a control");
                assert_eq!(
                    focused.name.as_deref(),
                    Some(order[at.unwrap()].1.as_str()),
                    "step {step} (shift {back}) in {order:?}"
                );
            }
        });
    });
}
