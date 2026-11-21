use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use schnellui::signal::create_signal;
use schnellui::template::{
    column, em, row, scroll, stack, Button, Checkbox, ComponentKind, ComponentProps, Flex,
    FlexChild, Pad, ResponsiveQuery, Slider, Spacer, Template, TemplateRenderer, Text, TextInput,
    Widget,
};

use super::ops::{event, kind, prop};
use super::*;

fn vp() -> Viewport {
    Viewport::new(360.0, 640.0, 2.0, false)
}

fn ops_of(bytes: &[u8]) -> Vec<Op> {
    decode(bytes).expect("valid buffer")
}

// ---- a model of the Java host, to check that a diff rebuilds the tree ----

#[derive(Clone, Debug, PartialEq)]
enum Val {
    S(String),
    F(u32),
    I(i32),
}

/// Kind, props and child ids of one host view.
type View = (u8, BTreeMap<u8, Val>, Vec<u32>);

#[derive(Default)]
struct Host {
    views: BTreeMap<u32, View>,
    root: Option<u32>,
    ever_created: Vec<u32>,
}

impl Host {
    fn apply(&mut self, ops: &[Op]) {
        for op in ops {
            match op {
                Op::Create { id, kind } => {
                    assert!(!self.ever_created.contains(id), "id {id} reused");
                    self.ever_created.push(*id);
                    self.views.insert(*id, (*kind, BTreeMap::new(), Vec::new()));
                }
                Op::SetStr { id, prop, value } => {
                    self.views
                        .get_mut(id)
                        .unwrap()
                        .1
                        .insert(*prop, Val::S(value.clone()));
                }
                Op::SetF32 { id, prop, value } => {
                    self.views
                        .get_mut(id)
                        .unwrap()
                        .1
                        .insert(*prop, Val::F(value.to_bits()));
                }
                Op::SetI32 { id, prop, value } => {
                    self.views
                        .get_mut(id)
                        .unwrap()
                        .1
                        .insert(*prop, Val::I(*value));
                }
                Op::Clear { id, prop } => {
                    self.views.get_mut(id).unwrap().1.remove(prop);
                }
                Op::Insert {
                    parent,
                    child,
                    index,
                } => {
                    for (_, _, children) in self.views.values_mut() {
                        children.retain(|c| c != child);
                    }
                    let children = &mut self.views.get_mut(parent).unwrap().2;
                    assert!(*index as usize <= children.len(), "index out of range");
                    children.insert(*index as usize, *child);
                }
                Op::Remove { id } => {
                    for (_, _, children) in self.views.values_mut() {
                        children.retain(|c| c != id);
                    }
                    let mut stack = vec![*id];
                    while let Some(id) = stack.pop() {
                        let (_, _, children) = self.views.remove(&id).expect("removed view exists");
                        stack.extend(children);
                    }
                }
                Op::SetRoot { id } => self.root = Some(*id),
            }
        }
    }

    /// The tree below the root without ids, so two hosts compare by shape.
    fn shape(&self) -> String {
        fn walk(host: &Host, id: u32, depth: usize, out: &mut String) {
            let (kind, props, children) = &host.views[&id];
            out.push_str(&format!("{}{kind} {props:?}\n", "  ".repeat(depth)));
            for c in children {
                walk(host, *c, depth + 1, out);
            }
        }
        let mut out = String::new();
        if let Some(root) = self.root {
            walk(self, root, 0, &mut out);
        }
        out
    }

    fn ids_of_kind(&self, k: u8) -> Vec<u32> {
        self.views
            .iter()
            .filter(|(_, v)| v.0 == k)
            .map(|(id, _)| *id)
            .collect()
    }

    fn text_of(&self, id: u32) -> Option<String> {
        match self.views[&id].1.get(&prop::TEXT) {
            Some(Val::S(s)) => Some(s.clone()),
            _ => None,
        }
    }
}

/// Mounts, applies the first render, and checks every later diff against a
/// fresh full render.
struct Harness {
    app: AndroidRenderer,
    host: Host,
    tick: f32,
}

impl Harness {
    fn new<V: Template + 'static>(view: impl FnMut() -> V + 'static) -> Self {
        let mut app = AndroidRenderer::new(vp(), view);
        let mut host = Host::default();
        host.apply(&ops_of(&app.render()));
        Self {
            app,
            host,
            tick: 1000.0,
        }
    }

    /// Re-renders after a signal changed, by nudging the viewport.
    fn refresh(&mut self) -> Vec<Op> {
        self.tick += 1.0;
        let diff = self.app.resize(self.tick, 640.0);
        self.apply(diff)
    }

    fn apply(&mut self, diff: Option<Vec<u8>>) -> Vec<Op> {
        let ops = diff.map(|d| ops_of(&d)).unwrap_or_default();
        self.host.apply(&ops);
        let mut fresh = Host::default();
        fresh.apply(&ops_of(&self.app.render()));
        assert_eq!(
            self.host.shape(),
            fresh.shape(),
            "diff and full render disagree"
        );
        ops
    }

    fn click(&mut self, id: u32) -> Vec<Op> {
        let diff = self.app.dispatch(id, Event::Click);
        self.apply(diff)
    }

    /// The host shows the user's change before it reports it.
    fn send(&mut self, id: u32, event: Event) -> Vec<Op> {
        let shown = match &event {
            Event::Toggle(on) => Some((prop::CHECKED, Val::I(*on as i32))),
            Event::Change(v) => Some((prop::VALUE, Val::F(v.to_bits()))),
            Event::Input(s) => Some((prop::TEXT, Val::S(s.clone()))),
            Event::Click => None,
        };
        if let Some((p, v)) = shown {
            self.host.views.get_mut(&id).unwrap().1.insert(p, v);
        }
        let diff = self.app.dispatch(id, event);
        self.apply(diff)
    }
}

// ---- codec ----

#[test]
fn every_op_round_trips() {
    let ops = vec![
        Op::Create {
            id: 1,
            kind: kind::ROW,
        },
        Op::SetStr {
            id: 1,
            prop: prop::TEXT,
            value: "héllo \u{1F600}".into(),
        },
        Op::SetStr {
            id: 1,
            prop: prop::A11Y_LABEL,
            value: String::new(),
        },
        Op::SetF32 {
            id: 1,
            prop: prop::WIDTH,
            value: 12.5,
        },
        Op::SetF32 {
            id: 1,
            prop: prop::GROW,
            value: -0.0,
        },
        Op::SetI32 {
            id: 1,
            prop: prop::JUSTIFY,
            value: -3,
        },
        Op::SetI32 {
            id: u32::MAX,
            prop: prop::VISIBLE,
            value: 1,
        },
        Op::Clear {
            id: 1,
            prop: prop::WIDTH,
        },
        Op::Insert {
            parent: 1,
            child: 2,
            index: 7,
        },
        Op::Remove { id: 2 },
        Op::SetRoot { id: 1 },
    ];
    let bytes = encode(&ops);
    assert_eq!(&bytes[..2], &[0x53, 0x01]);
    assert_eq!(ops_of(&bytes), ops);
}

#[test]
fn encoding_is_little_endian() {
    let bytes = encode(&[
        Op::SetStr {
            id: 0x0102_0304,
            prop: 30,
            value: "ab".into(),
        },
        Op::SetF32 {
            id: 1,
            prop: 1,
            value: 1.0,
        },
    ]);
    assert_eq!(
        bytes,
        [
            0x53, 0x01, 0x02, 0x04, 0x03, 0x02, 0x01, 30, 2, 0, 0, 0, b'a', b'b', 0x03, 1, 0, 0, 0,
            1, 0, 0, 0x80, 0x3f
        ]
    );
}

#[test]
fn decode_rejects_bad_buffers() {
    assert_eq!(decode(&[]), Err(DecodeError::Truncated));
    assert_eq!(decode(&[0x54, 1]), Err(DecodeError::BadMagic(0x54)));
    assert_eq!(decode(&[0x53, 2]), Err(DecodeError::BadVersion(2)));
    assert_eq!(decode(&[0x53, 1]), Ok(vec![]));
    assert_eq!(decode(&[0x53, 1, 0x7f]), Err(DecodeError::UnknownOp(0x7f)));
    assert_eq!(
        decode(&[0x53, 1, 0x01, 1, 0, 0]),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        decode(&[0x53, 1, 0x02, 1, 0, 0, 0, 30, 9, 0, 0, 0, b'x']),
        Err(DecodeError::Truncated)
    );
    assert_eq!(
        decode(&[0x53, 1, 0x02, 1, 0, 0, 0, 30, 1, 0, 0, 0, 0xff]),
        Err(DecodeError::BadUtf8)
    );
}

#[test]
fn unknown_kinds_and_props_still_decode() {
    let ops = ops_of(&encode(&[
        Op::Create { id: 1, kind: 200 },
        Op::SetI32 {
            id: 1,
            prop: 250,
            value: 1,
        },
    ]));
    assert_eq!(ops.len(), 2);
}

#[test]
fn events_decode_from_raw_codes() {
    assert_eq!(
        Event::from_raw(event::CLICK as i32, 0, 0.0, None),
        Some(Event::Click)
    );
    assert_eq!(Event::from_raw(2, 1, 0.0, None), Some(Event::Toggle(true)));
    assert_eq!(Event::from_raw(3, 0, 2.5, None), Some(Event::Change(2.5)));
    assert_eq!(
        Event::from_raw(4, 0, 0.0, Some("a".into())),
        Some(Event::Input("a".into()))
    );
    assert_eq!(Event::from_raw(4, 0, 0.0, None), None);
    assert_eq!(Event::from_raw(99, 0, 0.0, None), None);
    assert_eq!(Event::from_raw(-1, 0, 0.0, None), None);
}

// ---- full render ----

fn dump(ops: &[Op]) -> String {
    let mut out = String::new();
    for op in ops {
        out.push_str(&format!("{op:?}\n"));
    }
    out
}

#[test]
fn golden_full_render() {
    let mut app = AndroidRenderer::new(vp(), || {
        column()
            .gap(8.0)
            .child(Text::new("Title").size(20.0))
            .child(Button::new("Go").on_click(|| {}))
            .child(Checkbox::new(true).name("Agree"))
            .child(Slider::new(30.0, 0.0, 100.0).step(5.0))
            .child(TextInput::new("hi").label("Name"))
            .child(Pad::all(4.0).child(Spacer::new()))
    });
    let actual = dump(&ops_of(&app.render()));
    let expected = include_str!("snapshots/small_tree.ops");
    if std::env::var_os("SCHNELLUI_BLESS").is_some() {
        std::fs::write(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/render_android/snapshots/small_tree.ops"
            ),
            &actual,
        )
        .unwrap();
        return;
    }
    assert_eq!(actual, expected);
}

#[test]
fn full_render_ends_in_set_root_of_the_root() {
    let mut app = AndroidRenderer::new(vp(), || row().child(Text::new("a")));
    let ops = ops_of(&app.render());
    assert_eq!(
        ops.first(),
        Some(&Op::Create {
            id: 1,
            kind: kind::ROW
        })
    );
    assert_eq!(ops.last(), Some(&Op::SetRoot { id: 1 }));
}

// ---- diff ----

#[test]
fn changed_prop_emits_only_a_set() {
    let n = create_signal(0);
    let mut h =
        Harness::new(move || column().child(Text::dynamic(move || format!("n={}", n.get()))));
    n.set(5);
    let ops = h.refresh();
    assert_eq!(
        ops,
        vec![Op::SetStr {
            id: 2,
            prop: prop::TEXT,
            value: "n=5".into()
        }]
    );
}

#[test]
fn unchanged_render_emits_nothing() {
    let mut h = Harness::new(|| column().child(Text::new("same")));
    let diff = h.app.resize(361.0, 640.0);
    assert!(diff.is_none());
    h.apply(diff);
}

#[test]
fn dropped_prop_emits_clear() {
    let on = create_signal(true);
    let mut h = Harness::new(move || {
        let mut b = Button::new("b");
        if !on.get() {
            b = b.disabled(true);
        }
        column().child(b)
    });
    on.set(false);
    assert_eq!(
        h.refresh(),
        vec![Op::SetI32 {
            id: 2,
            prop: prop::ENABLED,
            value: 0
        }]
    );
    on.set(true);
    assert_eq!(
        h.refresh(),
        vec![Op::Clear {
            id: 2,
            prop: prop::ENABLED
        }]
    );
}

fn keyed_list(order: schnellui::signal::Signal<Vec<u32>>) -> impl Template {
    struct Items(schnellui::signal::Signal<Vec<u32>>);
    impl Template for Items {
        fn render<R: TemplateRenderer>(self, r: &mut R) -> R::Node {
            let nodes = self
                .0
                .get()
                .into_iter()
                .map(|i| {
                    Text::new(format!("item {i}"))
                        .key(format!("k{i}"))
                        .render(r)
                })
                .collect();
            r.container(
                schnellui::template::ContainerProps::new(
                    schnellui::template::ContainerKind::Column,
                ),
                nodes,
            )
        }
    }
    Items(order)
}

fn item_ids(host: &Host) -> Vec<(String, u32)> {
    let column = host.ids_of_kind(kind::COLUMN)[0];
    host.views[&column]
        .2
        .iter()
        .map(|c| (host.text_of(*c).unwrap(), *c))
        .collect()
}

#[test]
fn keyed_reorder_inserts_and_keeps_ids() {
    let order = create_signal(vec![1, 2, 3]);
    let mut h = Harness::new(move || keyed_list(order));
    let before = item_ids(&h.host);
    order.set(vec![3, 1, 2]);
    let ops = h.refresh();
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert!(matches!(ops[0], Op::Insert { .. }));
    let after = item_ids(&h.host);
    let find = |v: &[(String, u32)], t: &str| v.iter().find(|x| x.0 == t).unwrap().1;
    for t in ["item 1", "item 2", "item 3"] {
        assert_eq!(find(&before, t), find(&after, t), "{t} kept its id");
    }
    assert_eq!(
        after.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(),
        ["item 3", "item 1", "item 2"]
    );
}

#[test]
fn keyed_reorders_match_a_full_render() {
    let order = create_signal(vec![1, 2, 3, 4, 5, 6]);
    let mut h = Harness::new(move || keyed_list(order));
    let mut seed = 7u32;
    for _ in 0..40 {
        // A small deterministic shuffle with removals and additions.
        let mut next: Vec<u32> = (1..=9).collect();
        for i in (1..next.len()).rev() {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            next.swap(i, (seed >> 16) as usize % (i + 1));
        }
        next.truncate(2 + (seed >> 8) as usize % 7);
        order.set(next);
        h.refresh();
    }
}

#[test]
fn moving_one_item_to_the_end_is_one_insert() {
    let order = create_signal(vec![1, 2, 3, 4]);
    let mut h = Harness::new(move || keyed_list(order));
    order.set(vec![2, 3, 4, 1]);
    let ops = h.refresh();
    assert_eq!(ops.len(), 1, "{ops:?}");
}

#[test]
fn removed_node_emits_remove() {
    let order = create_signal(vec![1, 2, 3]);
    let mut h = Harness::new(move || keyed_list(order));
    let before = item_ids(&h.host);
    order.set(vec![1, 3]);
    let ops = h.refresh();
    let gone = before.iter().find(|x| x.0 == "item 2").unwrap().1;
    assert_eq!(ops, vec![Op::Remove { id: gone }]);
}

#[test]
fn added_keyed_node_creates_and_inserts() {
    let order = create_signal(vec![1, 3]);
    let mut h = Harness::new(move || keyed_list(order));
    order.set(vec![1, 2, 3]);
    let ops = h.refresh();
    assert!(matches!(ops[0], Op::Create { .. }));
    assert!(matches!(ops.last(), Some(Op::Insert { index: 1, .. })));
    assert!(!ops.iter().any(|o| matches!(o, Op::Remove { .. })));
}

#[test]
fn kind_change_recreates() {
    let button = create_signal(false);
    struct Pick(bool);
    impl Template for Pick {
        fn render<R: TemplateRenderer>(self, r: &mut R) -> R::Node {
            if self.0 {
                column().child(Button::new("x")).render(r)
            } else {
                column().child(Text::new("x")).render(r)
            }
        }
    }
    let mut h = Harness::new(move || Pick(button.get()));
    let old = h.host.ids_of_kind(kind::TEXT)[0];
    button.set(true);
    let ops = h.refresh();
    assert!(ops.contains(&Op::Remove { id: old }));
    assert!(ops
        .iter()
        .any(|o| matches!(o, Op::Create { kind: k, .. } if *k == kind::BUTTON)));
    assert!(!h.host.views.contains_key(&old));
}

#[test]
fn root_kind_change_replaces_the_root() {
    let wide = create_signal(false);
    let mut h = Harness::new(move || Switch(wide.get()));
    let old_root = h.host.root.unwrap();
    wide.set(true);
    let ops = h.refresh();
    assert!(ops.iter().any(|o| matches!(o, Op::SetRoot { .. })));
    assert!(ops.contains(&Op::Remove { id: old_root }));
    assert_ne!(h.host.root, Some(old_root));
}

struct Switch(bool);

impl Template for Switch {
    fn render<R: TemplateRenderer>(self, r: &mut R) -> R::Node {
        if self.0 {
            stack().child(Text::new("a")).render(r)
        } else {
            row().child(Text::new("a")).render(r)
        }
    }
}

#[test]
fn ids_are_never_reused() {
    let order = create_signal(vec![1, 2]);
    let mut h = Harness::new(move || keyed_list(order));
    let mut seen: Vec<u32> = h.host.ever_created.clone();
    for next in [vec![1], vec![1, 5], vec![7], vec![], vec![2, 1]] {
        order.set(next);
        h.refresh();
        for id in &h.host.ever_created {
            if !seen.contains(id) {
                assert!(
                    seen.iter().all(|s| s < id),
                    "new id {id} not above {seen:?}"
                );
                seen.push(*id);
            }
        }
    }
    let unique: std::collections::BTreeSet<_> = h.host.ever_created.iter().collect();
    assert_eq!(unique.len(), h.host.ever_created.len());
}

// ---- dispatch ----

#[test]
fn click_runs_the_handler_and_returns_the_diff() {
    let n = create_signal(0);
    let mut h = Harness::new(move || {
        column()
            .child(Text::dynamic(move || format!("{}", n.get())))
            .child(Button::new("+").on_click(move || n.update(|v| *v += 1)))
    });
    let button = h.host.ids_of_kind(kind::BUTTON)[0];
    let ops = h.click(button);
    assert_eq!(
        ops,
        vec![Op::SetStr {
            id: 2,
            prop: prop::TEXT,
            value: "1".into()
        }]
    );
}

#[test]
fn toggle_change_and_input_run_handlers() {
    let on = create_signal(false);
    let level = create_signal(1.0f32);
    let text = create_signal(String::new());
    let mut h = Harness::new(move || {
        column()
            .child(Checkbox::new(on.get()).on_toggle(move |v| on.set(v)))
            .child(Slider::new(level.get(), 0.0, 10.0).on_change(move |v| level.set(v)))
            .child(TextInput::new(text.get()).on_input(move |s| text.set(s.to_uppercase())))
            .child(Text::dynamic(move || {
                format!("{}|{}|{}", on.get(), level.get(), text.get())
            }))
    });
    let cb = h.host.ids_of_kind(kind::CHECKBOX)[0];
    let sl = h.host.ids_of_kind(kind::SLIDER)[0];
    let ti = h.host.ids_of_kind(kind::TEXT_INPUT)[0];
    let label = h.host.ids_of_kind(kind::TEXT)[0];

    // The host already shows the new checked state, so only the text moves.
    let ops = h.send(cb, Event::Toggle(true));
    assert_eq!(
        ops,
        vec![Op::SetStr {
            id: label,
            prop: prop::TEXT,
            value: "true|1|".into()
        }]
    );
    let ops = h.send(sl, Event::Change(4.5));
    assert_eq!(
        ops,
        vec![Op::SetStr {
            id: label,
            prop: prop::TEXT,
            value: "true|4.5|".into()
        }]
    );
    // The handler stores the upper-cased text, so the input is set to it.
    let ops = h.send(ti, Event::Input("ab".into()));
    assert!(ops.contains(&Op::SetStr {
        id: ti,
        prop: prop::TEXT,
        value: "AB".into()
    }));
    assert!(ops.contains(&Op::SetStr {
        id: label,
        prop: prop::TEXT,
        value: "true|4.5|AB".into()
    }));
}

#[test]
fn accepted_input_needs_no_set_on_the_input() {
    let text = create_signal(String::new());
    let mut h = Harness::new(move || {
        column().child(TextInput::new(text.get()).on_input(move |s| text.set(s.to_string())))
    });
    let ti = h.host.ids_of_kind(kind::TEXT_INPUT)[0];
    assert!(h.send(ti, Event::Input("abc".into())).is_empty());
}

#[test]
fn rejected_input_restores_the_text() {
    let mut h = Harness::new(|| column().child(TextInput::new("hi").on_input(|_| {})));
    let ti = h.host.ids_of_kind(kind::TEXT_INPUT)[0];
    let ops = h.send(ti, Event::Input("hix".into()));
    assert_eq!(
        ops,
        vec![Op::SetStr {
            id: ti,
            prop: prop::TEXT,
            value: "hi".into()
        }]
    );
}

#[test]
fn rejected_input_to_empty_restores_with_a_set() {
    let mut h = Harness::new(|| column().child(TextInput::new("").on_input(|_| {})));
    let ti = h.host.ids_of_kind(kind::TEXT_INPUT)[0];
    let ops = h.send(ti, Event::Input("x".into()));
    assert_eq!(
        ops,
        vec![Op::SetStr {
            id: ti,
            prop: prop::TEXT,
            value: String::new()
        }]
    );
}

#[test]
fn rejected_toggle_and_change_restore() {
    let mut h = Harness::new(|| {
        column()
            .child(Checkbox::new(false).on_toggle(|_| {}))
            .child(Slider::new(3.0, 0.0, 10.0).on_change(|_| {}))
    });
    let cb = h.host.ids_of_kind(kind::CHECKBOX)[0];
    let sl = h.host.ids_of_kind(kind::SLIDER)[0];
    assert_eq!(
        h.send(cb, Event::Toggle(true)),
        vec![Op::SetI32 {
            id: cb,
            prop: prop::CHECKED,
            value: 0
        }]
    );
    assert_eq!(
        h.send(sl, Event::Change(9.0)),
        vec![Op::SetF32 {
            id: sl,
            prop: prop::VALUE,
            value: 3.0
        }]
    );
}

#[test]
fn events_without_a_handler_are_ignored() {
    let mut h = Harness::new(|| {
        column()
            .child(Button::new("dead"))
            .child(Checkbox::new(false))
            .child(Text::new("t"))
    });
    for id in h.host.views.keys().copied().collect::<Vec<_>>() {
        for e in [
            Event::Click,
            Event::Toggle(true),
            Event::Change(1.0),
            Event::Input("x".into()),
        ] {
            assert_eq!(h.app.dispatch(id, e), None);
        }
    }
    assert_eq!(h.app.dispatch(9999, Event::Click), None);
}

#[test]
fn handlers_see_the_value_before_they_run() {
    // The diff is computed against the recorded value, so a handler that
    // reads nothing and stores nothing still yields a restoring SET.
    let seen = Rc::new(RefCell::new(Vec::new()));
    let s = seen.clone();
    let mut app = AndroidRenderer::new(vp(), move || {
        let s = s.clone();
        column().child(TextInput::new("a").on_input(move |t| s.borrow_mut().push(t.to_string())))
    });
    let mut host = Host::default();
    host.apply(&ops_of(&app.render()));
    let ti = host.ids_of_kind(kind::TEXT_INPUT)[0];
    app.dispatch(ti, Event::Input("ab".into()));
    assert_eq!(*seen.borrow(), ["ab"]);
}

// ---- responsive ----

#[test]
fn resize_flips_visible_for_a_viewport_query() {
    let mut h = Harness::new(|| {
        column()
            .child(Text::new("wide").show_when(ResponsiveQuery::viewport().min_width(500.0)))
            .child(Text::new("narrow").show_when(ResponsiveQuery::viewport().max_width(em(31.25))))
    });
    let texts = h.host.ids_of_kind(kind::TEXT);
    let (wide, narrow) = (texts[0], texts[1]);
    assert_eq!(h.host.views[&wide].1.get(&prop::VISIBLE), Some(&Val::I(0)));
    assert_eq!(h.host.views[&narrow].1.get(&prop::VISIBLE), None);

    let diff = h.app.resize(600.0, 640.0);
    let ops = h.apply(diff);
    assert_eq!(
        ops,
        vec![
            Op::Clear {
                id: wide,
                prop: prop::VISIBLE
            },
            Op::SetI32 {
                id: narrow,
                prop: prop::VISIBLE,
                value: 0
            }
        ]
    );
    // 31.25 em is 500 dp, and bounds are inclusive.
    let diff = h.app.resize(500.0, 640.0);
    let ops = h.apply(diff);
    assert_eq!(
        ops,
        vec![Op::Clear {
            id: narrow,
            prop: prop::VISIBLE
        }]
    );
    assert_eq!(h.app.resize(500.0, 640.0), None);
}

#[test]
fn parent_and_component_queries_stay_visible() {
    let mut app = AndroidRenderer::new(vp(), || {
        column().child(Text::new("p").show_when(ResponsiveQuery::parent().min_width(5000.0)))
    });
    let ops = ops_of(&app.render());
    assert!(!ops
        .iter()
        .any(|o| matches!(o, Op::SetI32 { prop: p, .. } if *p == prop::VISIBLE)));
}

// ---- template mapping ----

fn props_of(host: &Host, id: u32) -> &BTreeMap<u8, Val> {
    &host.views[&id].1
}

#[test]
fn pad_flex_and_spacer_follow_the_mapping() {
    let mut app = AndroidRenderer::new(vp(), || {
        column()
            .child(Pad::all(4.0).child(Text::new("p")))
            .child(Pad::all(2.0))
            .child(Spacer::new())
            .child(Flex::new().grow(2.0).basis(10.0).child(Text::new("f")))
            .child(Flex::new().min_width(5.0))
    });
    let mut host = Host::default();
    host.apply(&ops_of(&app.render()));
    let root = host.root.unwrap();
    let kids = host.views[&root].2.clone();
    assert_eq!(kids.len(), 5);
    let f = |x: f32| Val::F(x.to_bits());
    // pad with child: a Stack carrying PAD props
    assert_eq!(host.views[&kids[0]].0, kind::STACK);
    assert_eq!(props_of(&host, kids[0])[&prop::PAD_LEFT], f(4.0));
    assert_eq!(host.views[&kids[0]].2.len(), 1);
    // pad without child: a Spacer
    assert_eq!(host.views[&kids[1]].0, kind::SPACER);
    assert_eq!(props_of(&host, kids[1])[&prop::PAD_BOTTOM], f(2.0));
    // spacer: GROW 1
    assert_eq!(props_of(&host, kids[2])[&prop::GROW], f(1.0));
    // flex with child: props on the child's own node
    assert_eq!(host.views[&kids[3]].0, kind::TEXT);
    assert_eq!(props_of(&host, kids[3])[&prop::GROW], f(2.0));
    assert_eq!(props_of(&host, kids[3])[&prop::BASIS], f(10.0));
    // flex without child: a Spacer with those props only
    assert_eq!(host.views[&kids[4]].0, kind::SPACER);
    assert_eq!(props_of(&host, kids[4])[&prop::MIN_WIDTH], f(5.0));
    assert!(!props_of(&host, kids[4]).contains_key(&prop::GROW));
}

#[test]
fn flex_with_padding_wraps_the_child() {
    let mut template = AndroidTemplate::new(vp());
    let child = template.text(schnellui::template::TextProps {
        content: schnellui::template::TextContent::Static("x".into()),
        size: 16.0,
        role: schnellui::template::Role::Label,
        wrap: schnellui::template::WrapMode::NoWrap,
        align: schnellui::template::TextAlign::Start,
        ellipsis: false,
    });
    let child = template.keyed("k", child);
    let node = template.flex(
        FlexChild {
            padding: Some(schnellui::template::EdgeInsets::all(3.0)),
            width: Some(40.0),
            grow: Some(1.0),
            shrink: Some(1.0),
            ..FlexChild::default()
        },
        Some(child),
    );
    assert_eq!(node.kind, kind::STACK);
    assert_eq!(node.key.as_deref(), Some("k"));
    assert!(node.props.contains_key(&prop::WIDTH));
    assert!(node.props.contains_key(&prop::PAD_TOP));
    assert!(!node.children[0].props.contains_key(&prop::WIDTH));
    assert!(node.children[0].key.is_none());
}

#[test]
fn container_props_map_to_codes() {
    use schnellui::template::{Align, Justify};
    let mut app = AndroidRenderer::new(vp(), || {
        row()
            .gap(6.0)
            .justify(Justify::SpaceBetween)
            .align(Align::Stretch)
            .fill()
            .wrap()
            .child(
                scroll()
                    .scrollbar(true)
                    .height(50.0)
                    .child(Text::new("t").size(12.0)),
            )
    });
    let mut host = Host::default();
    host.apply(&ops_of(&app.render()));
    let root = host.root.unwrap();
    let p = props_of(&host, root);
    assert_eq!(p[&prop::JUSTIFY], Val::I(3));
    assert_eq!(p[&prop::ALIGN], Val::I(3));
    assert_eq!(p[&prop::FILL], Val::I(1));
    assert_eq!(p[&prop::WRAP], Val::I(1));
    assert_eq!(p[&prop::GAP], Val::F(6.0f32.to_bits()));
    let sc = host.views[&root].2[0];
    assert_eq!(host.views[&sc].0, kind::SCROLL);
    assert_eq!(props_of(&host, sc)[&prop::SCROLLBAR], Val::I(1));
}

#[test]
fn a_key_applied_twice_keeps_the_last() {
    let mut template = AndroidTemplate::new(vp());
    let node = Text::new("x").key("a").key("b").render(&mut template);
    assert_eq!(node.key.as_deref(), Some("b"));
}

#[test]
fn password_input_sets_password() {
    let mut app = AndroidRenderer::new(vp(), || schnellui::template::PasswordInput::new("s"));
    let ops = ops_of(&app.render());
    assert!(ops.contains(&Op::SetI32 {
        id: 1,
        prop: prop::PASSWORD,
        value: 1
    }));
}

// ---- components ----

#[test]
fn every_component_kind_renders() {
    for kind in ComponentKind::ALL {
        let mut app = AndroidRenderer::new(vp(), move || {
            Widget::new(kind).label("Label").child(Text::new("child"))
        });
        let ops = ops_of(&app.render());
        assert!(matches!(ops.last(), Some(Op::SetRoot { .. })), "{kind:?}");
        // Re-rendering the same view is stable.
        assert_eq!(app.resize(1.0, 1.0), None, "{kind:?}");
    }
}

#[test]
fn fallback_is_a_column_led_by_the_label() {
    let mut app = AndroidRenderer::new(vp(), || {
        Widget::new(ComponentKind::Dialog)
            .label("Hello")
            .child(Text::new("body"))
    });
    let mut host = Host::default();
    host.apply(&ops_of(&app.render()));
    let root = host.root.unwrap();
    assert_eq!(host.views[&root].0, kind::COLUMN);
    let kids = &host.views[&root].2;
    assert_eq!(kids.len(), 2);
    assert_eq!(host.text_of(kids[0]).as_deref(), Some("Hello"));
    assert_eq!(host.text_of(kids[1]).as_deref(), Some("body"));

    // No label, no leading text.
    let mut app = AndroidRenderer::new(vp(), || {
        Widget::new(ComponentKind::List).child(Text::new("body"))
    });
    let mut host = Host::default();
    host.apply(&ops_of(&app.render()));
    assert_eq!(host.views[&host.root.unwrap()].2.len(), 1);
}

#[test]
fn base_component_kinds_map_onto_views() {
    let cases = [
        (ComponentKind::Row, kind::ROW),
        (ComponentKind::Column, kind::COLUMN),
        (ComponentKind::Stack, kind::STACK),
        (ComponentKind::Scroll, kind::SCROLL),
        (ComponentKind::Text, kind::TEXT),
        (ComponentKind::Button, kind::BUTTON),
        (ComponentKind::Checkbox, kind::CHECKBOX),
        (ComponentKind::Slider, kind::SLIDER),
        (ComponentKind::TextInput, kind::TEXT_INPUT),
        (ComponentKind::Spacer, kind::SPACER),
    ];
    for (component, expected) in cases {
        let mut template = AndroidTemplate::new(vp());
        let node = template.component(ComponentProps::new(component), Vec::new());
        assert_eq!(node.kind, expected, "{component:?}");
    }
}

#[test]
fn rebuild_keeps_ids() {
    let mut app = AndroidRenderer::new(vp(), || {
        column().child(Text::new("a")).child(Text::new("b"))
    });
    let first = ops_of(&app.render());
    let second = ops_of(&app.render());
    assert_eq!(first, second);
}
