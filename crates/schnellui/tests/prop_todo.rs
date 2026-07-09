//! Property tests for an app whose structure changes: a todo list where every
//! add, toggle, delete and filter change replaces the list branch, as a host
//! applying `WindowUpdate::Subtrees` does. Input arrives by typing, assistive
//! clicks and keyboard activation, in any order.
//!
//! Two properties hold after any program:
//! * after every step the UI shows exactly what a plain model says, and focus
//!   never points at a removed widget;
//! * the retained app, updated in place step by step, equals an app freshly
//!   mounted from the final model, in semantics and painted geometry.
//!
//! The signal runtime is process-global; run with `--test-threads=1`.

use std::cell::RefCell;
use std::rc::Rc;

use proptest::prelude::*;
use schnellui::a11y::Role;
use schnellui::scene::ComponentRef;
use schnellui::signal::{create_scope, create_signal, dispose, run_in_scope, Signal};
use schnellui::testing::{paint_layout, structure, Simulator};
use schnellui::widgets::{Button, Checkbox, Column, Row, Text, TextInput, View};
use schnellui::{App, UiKey};

#[allow(dead_code)]
mod common;

const WIDTH: u32 = 360;
const HEIGHT: u32 = 900;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Filter {
    #[default]
    All,
    Open,
    Done,
}

#[derive(Clone, Debug, PartialEq)]
struct Task {
    id: u64,
    title: String,
    done: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Todo {
    tasks: Vec<Task>,
    draft: String,
    next_id: u64,
    filter: Filter,
}

impl Todo {
    fn visible(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| match self.filter {
                Filter::All => true,
                Filter::Open => !t.done,
                Filter::Done => t.done,
            })
            .collect()
    }

    fn status(&self) -> String {
        let done = self.tasks.iter().filter(|t| t.done).count();
        format!("{} open, {done} done", self.tasks.len() - done)
    }

    fn add(&mut self) -> bool {
        let title = self.draft.trim().to_owned();
        if title.is_empty() {
            return false;
        }
        self.next_id += 1;
        self.tasks.push(Task {
            id: self.next_id,
            title,
            done: false,
        });
        self.draft.clear();
        true
    }
}

fn toggle_name(task: &Task) -> String {
    format!("toggle {} {}", task.id, task.title)
}

fn delete_name(task: &Task) -> String {
    format!("delete {}", task.id)
}

/// App state shared by handlers, plus what the host must do after a step.
#[derive(Default)]
struct Host {
    todo: Todo,
    list_changed: bool,
    clear_input: bool,
}

type Shared = Rc<RefCell<Host>>;

/// The list branch, rebuilt from the model whenever it changes.
fn list_view(host: &Shared, revision: Signal<u64>) -> Column {
    let todo = host.borrow().todo.clone();
    let mut list = Column::new().gap(4.0);
    let visible = todo.visible();
    if visible.is_empty() {
        list = list.child(Text::new("nothing here"));
    }
    for task in visible {
        let id = task.id;
        let (toggle_host, delete_host) = (host.clone(), host.clone());
        list =
            list.child(
                Row::new()
                    .gap(6.0)
                    .child(Checkbox::new(task.done).name(toggle_name(task)).on_toggle(
                        move |done| {
                            let mut h = toggle_host.borrow_mut();
                            if let Some(t) = h.todo.tasks.iter_mut().find(|t| t.id == id) {
                                t.done = done;
                            }
                            h.list_changed = true;
                            revision.update(|r| *r += 1);
                        },
                    ))
                    .child(Button::new(delete_name(task)).on_click(move || {
                        let mut h = delete_host.borrow_mut();
                        h.todo.tasks.retain(|t| t.id != id);
                        h.list_changed = true;
                        revision.update(|r| *r += 1);
                    })),
            );
    }
    list
}

fn todo_view(host: &Shared, revision: Signal<u64>, list_ref: ComponentRef) -> impl View {
    let draft = host.borrow().todo.draft.clone();
    let (input_host, add_host, clear_host, status_host) =
        (host.clone(), host.clone(), host.clone(), host.clone());
    let mut filters = Row::new().gap(4.0);
    for (name, filter) in [
        ("show all", Filter::All),
        ("show open", Filter::Open),
        ("show done", Filter::Done),
    ] {
        let host = host.clone();
        filters = filters.child(Button::new(name).on_click(move || {
            let mut h = host.borrow_mut();
            h.todo.filter = filter;
            h.list_changed = true;
        }));
    }
    Column::new()
        .gap(6.0)
        .child(
            TextInput::new(draft)
                .label("New task")
                .on_input(move |s| input_host.borrow_mut().todo.draft = s.to_owned()),
        )
        .child(Button::new("add").on_click(move || {
            let mut h = add_host.borrow_mut();
            if h.todo.add() {
                h.list_changed = true;
                h.clear_input = true;
                revision.update(|r| *r += 1);
            }
        }))
        .child(filters)
        .child(Button::new("clear done").on_click(move || {
            let mut h = clear_host.borrow_mut();
            h.todo.tasks.retain(|t| !t.done);
            h.list_changed = true;
            revision.update(|r| *r += 1);
        }))
        .child(Text::dynamic(move || {
            revision.get();
            status_host.borrow().todo.status()
        }))
        .child(list_view(host, revision).with_ref(list_ref))
}

#[derive(Clone, Debug)]
enum Op {
    /// Focuses the input and types text at its end.
    Type(String),
    Backspace,
    Add,
    /// Adds by pressing Enter on the focused add button.
    AddByKeyboard,
    /// Toggles a visible task, chosen modulo the visible count.
    Toggle(usize),
    /// Toggles by focusing the checkbox and pressing Space.
    ToggleByKeyboard(usize),
    Delete(usize),
    DeleteByKeyboard(usize),
    Filter(Filter),
    ClearDone,
    Tab {
        back: bool,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => "[a-c é]{1,4}".prop_map(Op::Type),
        1 => Just(Op::Backspace),
        2 => Just(Op::Add),
        1 => Just(Op::AddByKeyboard),
        2 => (0usize..16).prop_map(Op::Toggle),
        1 => (0usize..16).prop_map(Op::ToggleByKeyboard),
        1 => (0usize..16).prop_map(Op::Delete),
        1 => (0usize..16).prop_map(Op::DeleteByKeyboard),
        1 => prop_oneof![Just(Filter::All), Just(Filter::Open), Just(Filter::Done)]
            .prop_map(Op::Filter),
        1 => Just(Op::ClearDone),
        1 => any::<bool>().prop_map(|back| Op::Tab { back }),
    ]
}

struct Harness {
    sim: Simulator,
    host: Shared,
    revision: Signal<u64>,
    list_ref: ComponentRef,
}

impl Harness {
    fn new() -> Harness {
        let host: Shared = Rc::default();
        let revision = create_signal(0u64);
        let list_ref = ComponentRef::new();
        let sim = Simulator::mount(todo_view(&host, revision, list_ref), WIDTH, HEIGHT);
        Harness {
            sim,
            host,
            revision,
            list_ref,
        }
    }

    /// What the native host does after an event: apply the structural update
    /// and clear the input after an add.
    fn settle(&mut self) {
        let (list_changed, clear_input) = {
            let mut h = self.host.borrow_mut();
            (
                std::mem::take(&mut h.list_changed),
                std::mem::take(&mut h.clear_input),
            )
        };
        if list_changed {
            let list = list_view(&self.host, self.revision);
            self.sim.replace_subtree(self.list_ref, list);
        }
        if clear_input {
            let input = self.sim.find(Role::TextInput, Some("New task")).unwrap();
            self.sim.app_mut().set_text_value(input, "");
            self.sim.frame();
        }
    }

    fn visible_task(&self, raw: usize) -> Option<Task> {
        let h = self.host.borrow();
        let visible = h.todo.visible();
        (!visible.is_empty()).then(|| visible[raw % visible.len()].clone())
    }

    fn apply(&mut self, op: &Op, model: &mut Todo) {
        match op {
            Op::Type(text) => {
                self.sim.focus(Role::TextInput, Some("New task")).unwrap();
                self.sim.key(UiKey::End { shift: false });
                self.sim.type_text(text);
                model.draft.push_str(text);
            }
            Op::Backspace => {
                self.sim.focus(Role::TextInput, Some("New task")).unwrap();
                self.sim.key(UiKey::End { shift: false });
                self.sim.key(UiKey::Backspace);
                model.draft.pop();
            }
            Op::Add => {
                self.sim.click(Role::Button, Some("add")).unwrap();
                model.add();
            }
            Op::AddByKeyboard => {
                self.sim.focus(Role::Button, Some("add")).unwrap();
                self.sim.key(UiKey::Enter);
                model.add();
            }
            Op::Toggle(raw) | Op::ToggleByKeyboard(raw) => {
                let Some(task) = self.visible_task(*raw) else {
                    return;
                };
                let name = toggle_name(&task);
                if matches!(op, Op::Toggle(_)) {
                    self.sim.click(Role::CheckBox, Some(&name)).unwrap();
                } else {
                    self.sim.focus(Role::CheckBox, Some(&name)).unwrap();
                    self.sim.key(UiKey::Space { shift: false });
                }
                let t = model.tasks.iter_mut().find(|t| t.id == task.id).unwrap();
                t.done = !t.done;
            }
            Op::Delete(raw) | Op::DeleteByKeyboard(raw) => {
                let Some(task) = self.visible_task(*raw) else {
                    return;
                };
                let name = delete_name(&task);
                if matches!(op, Op::Delete(_)) {
                    self.sim.click(Role::Button, Some(&name)).unwrap();
                } else {
                    self.sim.focus(Role::Button, Some(&name)).unwrap();
                    self.sim.key(UiKey::Enter);
                }
                model.tasks.retain(|t| t.id != task.id);
            }
            Op::Filter(filter) => {
                let name = match filter {
                    Filter::All => "show all",
                    Filter::Open => "show open",
                    Filter::Done => "show done",
                };
                self.sim.click(Role::Button, Some(name)).unwrap();
                model.filter = *filter;
            }
            Op::ClearDone => {
                self.sim.click(Role::Button, Some("clear done")).unwrap();
                model.tasks.retain(|t| !t.done);
            }
            Op::Tab { back } => {
                self.sim.key(UiKey::Tab { shift: *back });
            }
        }
        self.settle();
    }

    fn check(&self, model: &Todo, step: usize) {
        let ctx = format!("step {step}: {model:?}");
        assert_eq!(self.host.borrow().todo, *model, "{ctx}: app state");
        let expected: Vec<String> = model.visible().into_iter().map(toggle_name).collect();
        assert_eq!(self.sim.names(Role::CheckBox), expected, "{ctx}: rows");
        for task in model.visible() {
            assert_eq!(
                self.sim
                    .has_state(Role::CheckBox, Some(&toggle_name(task)), "checked"),
                task.done,
                "{ctx}: {}",
                task.title
            );
        }
        assert!(
            self.sim.names(Role::Label).contains(&model.status()),
            "{ctx}: status {:?} not in {:?}",
            model.status(),
            self.sim.names(Role::Label)
        );
        assert_eq!(
            self.sim
                .value(Role::TextInput, Some("New task"))
                .unwrap_or_default(),
            model.draft,
            "{ctx}: draft"
        );
        let tree = self.sim.tree();
        if let Some(focus) = tree.focus {
            assert!(
                self.sim.focused().is_some(),
                "{ctx}: focus {focus} points at no node in the tree"
            );
        }
    }
}

/// A fresh app mounted from `model`, settled and without focus.
fn fresh(model: &Todo) -> Simulator {
    let host: Shared = Rc::new(RefCell::new(Host {
        todo: model.clone(),
        ..Host::default()
    }));
    let revision = create_signal(0u64);
    Simulator::mount(
        todo_view(&host, revision, ComponentRef::new()),
        WIDTH,
        HEIGHT,
    )
}

fn run_program(ops: &[Op]) {
    let scope = create_scope();
    run_in_scope(scope, || {
        let mut harness = Harness::new();
        let mut model = Todo::default();
        for (step, op) in ops.iter().enumerate() {
            harness.apply(op, &mut model);
            harness.check(&model, step);
        }
        // Retained == reconstructed, once both are at rest.
        harness.sim.blur();
        harness
            .sim
            .run_until_idle(std::time::Duration::from_secs(2))
            .unwrap();
        let mut reference = fresh(&model);
        reference
            .run_until_idle(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(
            structure(&harness.sim.tree()),
            structure(&reference.tree()),
            "semantics after {ops:?}"
        );
        assert_eq!(
            paint_layout(harness.sim.app()),
            paint_layout(reference.app()),
            "painted geometry after {ops:?}"
        );
        drop(reference);
        let _: &App = harness.sim.app();
    });
    dispose(scope);
}

proptest! {
    #![proptest_config(common::cases(64))]

    #[test]
    fn any_todo_program_matches_the_model_and_a_fresh_mount(
        ops in prop::collection::vec(op(), 1..40)
    ) {
        run_program(&ops);
    }
}
