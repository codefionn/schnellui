//! Property test for single-line text editing with grapheme clusters that
//! span several code points: combining accents, emoji with skin-tone
//! modifiers, flags and Hangul. Typing, pasting, assistive `SetValue`,
//! deletion and caret/selection movement must match a model that edits a
//! list of grapheme clusters. The `on_input` signal and a memo derived from
//! it must follow every change.
//!
//! The signal runtime is process-global; run with `--test-threads=1`.

use proptest::prelude::*;
use schnellui::a11y::Role;
use schnellui::accesskit_action::{Action, ActionData};
use schnellui::signal::{create_memo, create_scope, create_signal, dispose, run_in_scope};
use schnellui::testing::Simulator;
use schnellui::widgets::{Column, Text, TextInput};
use schnellui::UiKey;
use unicode_segmentation::UnicodeSegmentation;

#[allow(dead_code)]
mod common;

/// Clusters that never merge with a neighbour, so the model can treat each
/// as one unit wherever it lands.
const CLUSTERS: &[&str] = &[
    "a",
    "Z",
    "7",
    " ",
    "ß",
    "é",
    "e\u{301}",
    "한",
    "👍🏽",
    "🇩🇪",
    "👩‍💻",
    "Ω",
];

#[derive(Debug, Default)]
struct Model {
    text: Vec<String>,
    caret: usize,
    anchor: usize,
}

impl Model {
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

    fn insert(&mut self, clusters: &[String]) {
        self.delete_selection();
        for (i, c) in clusters.iter().enumerate() {
            self.text.insert(self.caret + i, c.clone());
        }
        self.caret += clusters.len();
        self.anchor = self.caret;
    }

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
        self.text.concat()
    }

    fn selected(&self) -> String {
        self.selection()
            .map(|(s, e)| self.text[s..e].concat())
            .unwrap_or_default()
    }
}

fn clusters(text: &str) -> Vec<String> {
    text.graphemes(true).map(str::to_owned).collect()
}

#[derive(Clone, Debug)]
enum Op {
    Type(&'static str),
    Paste(Vec<&'static str>),
    SetValue(Vec<&'static str>),
    Backspace,
    Delete,
    Left { shift: bool },
    Right { shift: bool },
    Home { shift: bool },
    End { shift: bool },
    SelectAll,
}

fn cluster() -> impl Strategy<Value = &'static str> {
    prop::sample::select(CLUSTERS)
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        5 => cluster().prop_map(Op::Type),
        1 => prop::collection::vec(cluster(), 1..5).prop_map(Op::Paste),
        1 => prop::collection::vec(cluster(), 0..6).prop_map(Op::SetValue),
        2 => Just(Op::Backspace),
        1 => Just(Op::Delete),
        2 => any::<bool>().prop_map(|shift| Op::Left { shift }),
        2 => any::<bool>().prop_map(|shift| Op::Right { shift }),
        1 => any::<bool>().prop_map(|shift| Op::Home { shift }),
        1 => any::<bool>().prop_map(|shift| Op::End { shift }),
        1 => Just(Op::SelectAll),
    ]
}

fn run_program(ops: &[Op]) {
    let scope = create_scope();
    run_in_scope(scope, || {
        let typed = create_signal(String::new());
        let count = create_memo(move || typed.with(|t| t.graphemes(true).count()));
        let view = Column::new()
            .child(
                TextInput::new("")
                    .label("Text")
                    .on_input(move |s| typed.set(s.to_owned())),
            )
            .child(Text::dynamic(move || format!("clusters {}", count.get())));
        let mut sim = Simulator::mount(view, 600, 200);
        sim.focus(Role::TextInput, Some("Text")).unwrap();
        let mut model = Model::default();

        for (step, op) in ops.iter().enumerate() {
            match op {
                Op::Type(c) => {
                    sim.key(UiKey::Char(c));
                    model.insert(&clusters(c));
                }
                Op::Paste(parts) => {
                    let text = parts.concat();
                    sim.app_mut().paste_text(&text);
                    sim.frame();
                    model.insert(&clusters(&text));
                }
                Op::SetValue(parts) => {
                    let text = parts.concat();
                    sim.action(
                        Action::SetValue,
                        Role::TextInput,
                        Some("Text"),
                        Some(ActionData::Value(text.clone().into())),
                    )
                    .unwrap();
                    // A replaced value parks the caret at its end; setting
                    // the current value again is a no-op.
                    if text != model.string() {
                        model.text = clusters(&text);
                        model.caret = model.text.len();
                        model.anchor = model.caret;
                    }
                }
                Op::Backspace => {
                    sim.key(UiKey::Backspace);
                    if !model.delete_selection() && model.caret > 0 {
                        model.caret -= 1;
                        model.text.remove(model.caret);
                        model.anchor = model.caret;
                    }
                }
                Op::Delete => {
                    sim.key(UiKey::Delete);
                    if !model.delete_selection() && model.caret < model.text.len() {
                        model.text.remove(model.caret);
                    }
                }
                Op::Left { shift } => {
                    sim.key(UiKey::Left {
                        shift: *shift,
                        ctrl: false,
                    });
                    let start = model.selection().map(|(s, _)| s);
                    model.move_to(model.caret.saturating_sub(1), *shift, start);
                }
                Op::Right { shift } => {
                    sim.key(UiKey::Right {
                        shift: *shift,
                        ctrl: false,
                    });
                    let end = model.selection().map(|(_, e)| e);
                    let to = (model.caret + 1).min(model.text.len());
                    model.move_to(to, *shift, end);
                }
                Op::Home { shift } => {
                    sim.key(UiKey::Home { shift: *shift });
                    model.move_to(0, *shift, None);
                }
                Op::End { shift } => {
                    sim.key(UiKey::End { shift: *shift });
                    model.move_to(model.text.len(), *shift, None);
                }
                Op::SelectAll => {
                    sim.key(UiKey::SelectAll);
                    model.anchor = 0;
                    model.caret = model.text.len();
                }
            }
            let ctx = format!("step {step} ({op:?}): {model:?}");
            let expected = model.string();
            assert_eq!(
                sim.value(Role::TextInput, Some("Text")).unwrap_or_default(),
                expected,
                "{ctx}"
            );
            assert_eq!(
                sim.app().selected_text().unwrap_or_default(),
                model.selected(),
                "{ctx}: selection"
            );
            assert_eq!(typed.get(), expected, "{ctx}: on_input signal");
            assert!(
                sim.names(Role::Label)
                    .contains(&format!("clusters {}", model.text.len())),
                "{ctx}: derived count"
            );
        }
    });
    dispose(scope);
}

proptest! {
    #![proptest_config(common::cases(96))]

    #[test]
    fn editing_grapheme_clusters_matches_the_model(ops in prop::collection::vec(op(), 1..60)) {
        run_program(&ops);
    }
}
